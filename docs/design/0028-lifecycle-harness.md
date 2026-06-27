# ADR 0028 — Engine-backed Offline Lifecycle Harness (P-OBJ)

Status: IN PROGRESS 2026-06-27. Kernels **K0 (rally), K1 (spawn-throughput), K2 (FSM
next_state) LANDED**; the colony driver + K3/K4 remaining. Companion to ADR 0008 (squad
lifecycle), ADR 0027 (objective/squad lifecycle rework), ADR 0023/0023a (the combat sim
harness), ADR 0026 §9 (doctrine sizing). Task #23 / #25.

## Problem — live tuning is not converging

After ADR 0027 the offense still did not work end-to-end on the Docker private server, and
a session of **live fixes each only exposed the next layer**:

1. Squads sent a lone slot-0 lead in (no rally-until-full) → it died → wipe → re-field loop.
2. With a rally gate, rosters **stalled at 3/5** — forming members were starved below economy
   by the spawnsystem **head-of-line break** (`spawnsystem.rs:379-418`: a request with
   `body_cost > available_energy` but `<= energy_capacity` → `break`, reserving the home's
   energy for the higher-priority request and spawning nothing below it).
3. Even bankable members stalled: a force at MEDIUM only **tied** economy; bumping it to 87.5
   above economy with `forming-cap=1` **backfired** (combat creeps → ~0, cores accumulated)
   and was reverted.
4. A separate sizing bug produced one ~5000e max member that never banks (the operator
   caught it via live body logging — two code-analysis workflows had wrongly cleared it).

Each Docker capture is ~4 minutes and intel is intermittent, so a squad is rarely reliably
fielded to watch. **Verdict: live spawn-priority/lifecycle tuning does not converge** — each
guess changes the failure mode, the latest degraded it. The operator's standing call (the
offline-harness strategy) is the resolution: make the full chain
`objective → field → spawn → rally → depart → travel → engage → kill` **deterministically
reproducible offline**, driving the **real** coordination decisions as shared pure kernels
against the authoritative `screeps-combat-engine` sim.

## Design

### Architecture — a colony driver wrapping `ManagedSimSquad`

`ManagedSimSquad` (`screeps-combat-agent/src/squad.rs:213`) is the **engaged-tactics
vehicle** and correctly assumes a complete, co-located roster: it takes pre-placed members,
overwrites `Forming` on tick 0, and its only travel gate is "all members already in the
objective room." It has **no spawn, no rally, no lease** — the coordination layer is its
blind spot. We do **not** rewrite it; a new **colony driver** (in `screeps-combat-eval`)
supplies the missing layer and reuses `ManagedSimSquad` for the engaged phase.

The only new state is a `Colony` model that lives entirely OUTSIDE `CombatWorld`:

```
LifecycleScenario { world: CombatWorld, objective, colony: Colony, lease_ticks, seed }
Colony { homes: Vec<Home{ room, energy_capacity, energy_available, income, idle_spawns }>,
         economy_demand_fn(tick) -> [QueuedSpawn] }      // CRITICAL/HIGH lane contention
```

Creeps are materialized into `CombatWorld` only on spawn **completion** (placed at a home
staging tile, id appended to `ManagedSimSquad.members`). The engine stays unchanged — all
spawn economy is in `Colony`.

### Tick loop (the live SquadManager order: A reconcile → C claim → B field/spawn → B2 orders)

```
reconcile      lifecycle::reconcile(snapshot)              // DONE (ADR 0027, shared kernel)
claim-pacing   claim_pacing::plan_claims(...)              // K4 (MAX_FORMING/CONCURRENT)
field          fielding::slots_to_spawn(objective, colony) // K3 (wraps sized_for + build_body)
spawn          spawn_throughput::spawn_step(home, queue)   // K1 DONE — head-of-line break model
  + economy_demand_fn(tick) contends for the same lanes
rally          rally::squad_ready_to_depart / should_hold_at_boundary  // K0 DONE
engage         squad.step(world); defender(world); resolve_tick(world) // DONE — eval drives it
```

### Pure-vs-ECS seam (kernels in `screeps-combat-decision`, adapters in the bot)

| Layer | Pure kernel (eval + bot share) | ECS-bound (bot adapter) |
|---|---|---|
| Reconcile | `lifecycle::reconcile` (ADR 0027) | snapshot from `objective_queue`/`squad_contexts` |
| Rally | **`rally::{squad_ready_to_depart, should_hold_at_boundary}` (K0)** | the anchor write |
| Spawn throughput | **`spawn_throughput::spawn_step` (K1)** | `SpawnQueue`/`spawnsystem` |
| FSM transitions | **`squad_fsm::next_state` (K2)** | per-tick movement/combat/recall |
| Fielding | `composition::sized_for`/`build_body` (DONE) wrapped by `fielding::slots_to_spawn` (K3) | `queue_slot_spawn` token broadcast |
| Claim pacing | `claim_pacing::plan_claims` (K4) | entity mint (`field_new_squad`) |
| Tactics/travel/kill | `decide_squad` + engine (DONE) | — |

### Determinism

Builds on `evaluate::run` + seeded `Rng` (the `sim_is_deterministic_over_rounds` fence,
spread-0). The only new ordering surface is the spawn queue — modeled as a **descending
`Vec`** exactly as `SpawnQueue` (no `HashMap` iteration, per the determinism-fence memory).

## Kernels — status

- **K0 — rally (LANDED).** `squad_ready_to_depart` + `should_hold_at_boundary` (+ the
  `STRICT_QUORUM_RATIO=0.75` const and the private `is_near_room_edge_toward`) moved from the
  bot's `military::formation` into `screeps_combat_decision::rally`; the bot re-exports them
  (`formation.rs`, `squad.rs`) so all call sites are unchanged. 4 tests carried over.
- **K1 — spawn-throughput (LANDED).** `spawn_throughput::spawn_step` is a deterministic,
  value-type mirror of the live per-room head-of-line spawn loop (descending priority;
  skip-over-capacity; **break-on-unaffordable** = reserve; else spawn+debit). A driver test
  **reproduces the 3/5 stall offline + deterministically**: MEDIUM combat starves below
  economy, above-economy combat completes. This is where the spawn-priority lever is tuned
  now — instead of guessing live.
- **K2 — FSM next_state (LANDED).** `squad_fsm::next_state` is the pure transition table of
  `jobs/squad_combat.rs` (MoveToRoom/CombatResponse/Engaged/Retreating), in the same priority
  order, over a `SquadFsmSnapshot`. 4 tests cover every transition incl. the anti-ping-pong
  guard (never re-engage while the squad signals retreat) and the HP bars (40% respond /
  50% engaged / 80%·60% re-engage).
  - **Decision — bot adoption of `next_state` is DEFERRED.** Each live `*::tick` interleaves
    its transition checks with movement (the arrival-engage fires AFTER the formation move,
    not before), and two transitions carry side-effects (`combat_response_start` set/clear).
    Calling `next_state` up-front would move those, a behavior risk on a *working* FSM that is
    not the bug. So the kernel is the canonical, tested spec (a sync note sits above the live
    `machine!`); the harness drives `next_state`; full bot adoption waits for a tick refactor.
- **K3 — fielding (TODO).** `fielding::slots_to_spawn(objective, colony)` wrapping the
  already-shared `sized_for`/`build_body`/`PREFERRED_MEMBER_ENERGY` so the harness queues the
  same bodies the bot does (incl. the `None`-on-unbuildable stall).
- **K4 — claim pacing (TODO).** `claim_pacing::plan_claims` (counts) for `MAX_FORMING_SQUADS`
  / `MAX_CONCURRENT_SQUADS` so the harness reproduces the claim-throttle interactions (the
  `forming-cap=1` lockup that backfired live).

## Live fixes shipped this session (the harness must reproduce these, and the next layers)

All bot-only, no `WORLD_FORMAT_VERSION` change:
- Rally-until-full gate (K0 logic) — squads group up, no lone lead.
- `spawn_priority_for` MEDIUM+ → HIGH (forming combat above the economy bulk) + a forming-cap.
- Per-member energy cap (`PREFERRED_MEMBER_ENERGY=3000`) in BOTH `sized_for` and
  `queue_slot_spawn` — every spawned member (sized OR template-fallback) is bankable.
- **Reverted:** forming combat at 87.5 + `forming-cap=1` (it zeroed combat spawning). The
  current deployed state is HIGH + forming-cap=2 + the bankable-body cap.

## Remaining work

1. K3 + K4 kernels.
2. The colony driver + tick loop in `screeps-combat-eval`.
3. **First red tests:** the 3/5 spawn stall (DONE via K1); the **stale-intel give-up** (an
   engaged squad whose objective goes producer-silent — reproduce, then confirm the ADR 0027
   lease behavior offline).
4. Use the harness to find the **correct** spawn-priority + forming-cap values (the live
   guesses did not converge), validate, then deploy.

## What the harness CANNOT catch (keep a thin live canary)

The model omits real pathing/CPU, true intel-staleness timing, and engine quirks the sim
doesn't implement. A small live `[Lifecycle]`/`[SpawnQueue]` capture stays the final check
before trusting any deploy.
