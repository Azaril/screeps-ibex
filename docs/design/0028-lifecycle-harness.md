# ADR 0028 — Engine-backed Offline Lifecycle Harness (P-OBJ)

Status: IN PROGRESS 2026-06-27. **All five kernels K0–K4 + the forming-phase driver + the
ENGINE-ENGAGE HANDOFF LANDED** (`screeps-combat-eval/src/harness/lifecycle.rs` — the full
`objective→form→travel→engage→kill` chain is offline + deterministic). The harness then
diagnosed the live 87.5 backfire (see §Diagnosis) and now tests single/multi-room spawning +
rally/renew. **Both layers are broken: the spawn/form layer (no renew → stuck forms lose
members; + lane contention) AND combat effectiveness (squads lose defended fights).** Companion to ADR 0008 (squad
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
- **K3 — fielding (LANDED).** `fielding::slots_to_spawn(composition, filled, best_capacity,
  per_member_cap, priority, move_profile)` wraps the shared `sized_for`/`build_body`/
  `PREFERRED_MEMBER_ENERGY`: one `QueuedSpawn` per UNFILLED slot, body built at
  `min(best_capacity, per_member_cap)`, a slot no in-range home can build is skipped (the
  `None` stall), `id` = slot index. 3 tests.
- **K4 — claim pacing (LANDED).** `claim_pacing::claims_allowed(active, forming,
  max_concurrent, max_forming)` = the tighter of the two headrooms — reproduces the
  forming-cap LOCKUP (a stuck-forming squad blocks all new claims, the `forming-cap=1`
  zeroing seen live). 3 tests.

### Forming-phase colony driver (LANDED)

`screeps-combat-eval/src/harness/lifecycle.rs` — `run_forming(ColonyFormingScenario) ->
FormingOutcome`. A deterministic tick loop over a `Colony` (homes with capacity/income, a
per-tick `EconomyPressure` of a HIGH hauler ± CRITICAL miner) that drives the REAL kernel
chain: K3 fields the unfilled slots → K1 `spawn_step` runs each home's head-of-line lane
contest (combat vs economy, cross-home de-duped) → spawns occupy a home for `part_count*3`
ticks → K0 `squad_ready_to_depart` decides departure. Reproduces the live behavior OFFLINE:
**MEDIUM combat stalls below economy; above-economy combat completes the roster** — the
spawn-priority lever, now tunable offline instead of guessed on Docker. 3 tests (stall,
complete, determinism). The engage handoff (place the formed roster → `ManagedSimSquad` →
`resolve_tick` to a dead core) is the next phase; multi-squad + K4 claim-pacing reproduces
the `forming-cap=1` backfire (a single squad does not show it).

## Live fixes shipped this session (the harness must reproduce these, and the next layers)

All bot-only, no `WORLD_FORMAT_VERSION` change:
- Rally-until-full gate (K0 logic) — squads group up, no lone lead.
- `spawn_priority_for` MEDIUM+ → HIGH (forming combat above the economy bulk) + a forming-cap.
- Per-member energy cap (`PREFERRED_MEMBER_ENERGY=3000`) in BOTH `sized_for` and
  `queue_slot_spawn` — every spawned member (sized OR template-fallback) is bankable.
- **Reverted:** forming combat at 87.5 + `forming-cap=1` (it zeroed combat spawning). The
  current deployed state is HIGH + forming-cap=2 + the bankable-body cap.

## Diagnosis — the 87.5 backfire (live, captured 2026-06-27, then reverted)

Re-deployed the backfired config (forming combat at 87.5 + `forming-cap=1`) with two captures:

```
total:  118  99  85  84 100 107 107    (dipped ~30%, RECOVERED — not a collapse)
combat:   2   2   2   2   0   2   3     (near-zero throughout)
carry:   93  81  64  65  79  80  81     (haulers dipped, recovered to ~80)
[Lifecycle]: squad 327 RALLY 0→2/3 (DOES form);  RETIRE squad=144 reason=Wiped engaged_once=true
```

It is **neither** energy-collapse (economy recovers) **nor** a forming-cap lockup (squads form).
The mechanism: squads **form → depart → engage → get WIPED (lose the fight)** → re-form → churn
at ~0–2 standing combat creeps, with a *transient* economy drag from the 87.5 preemption.

**⇒ The spawn-priority knob is a RED HERRING.** HIGH stalls squads before they fight; 87.5 lets
them form-then-lose. The real failure is **combat effectiveness: squads lose their defended
engagements.** The spawn-priority/forming-cap tuning is parked at the safe **HIGH + forming-cap=2**
(deployed); the offline harness `run_lifecycle` proved the engage WORKS against an *undefended*
core, so the open question is the *defended* case.

## Spawn/form layer — single/multi-room + rally/renew (operator-requested, tested 2026-06-27)

`run_forming` now models member TTL (`CREEP_LIFE_TIME`) + death-by-age + optional renew. Findings
(10 lifecycle tests):
- **Single-room** spawning forms the roster (serial); **multi-room** forms it FASTER (parallel,
  asserted `multi < single`).
- **No-renew member-death is REAL.** A stuck/slow form (forming-span > a member's life) loses its
  early members to old age → they drop to unfilled → re-spawn → the roster never has the full set
  present at once → never departs. **The live bot has exactly this**: `request_renew` has zero
  callers, and live forms were stuck >1500t (> `CREEP_LIFE_TIME`), so the early members aged out.
- **Renew fixes it**: keeping the rallying roster alive (at a spawn-lane cost) completes the stuck
  form. ⇒ **Implement renew live** (wire `request_renew` for rallying members with low TTL).

## Remaining work

1. **Implement renew live** (the harness-validated spawn/form fix) — wire `request_renew` for a
   squad's present members while it rallies, so a slow/contested form doesn't lose its early members.
   Mind the energy cost (the colony's economy is fragile); the harness can tune the lane/energy budget.
2. **Graded-defender engage tests (combat effectiveness).** `assemble_single_room` already takes
   `towers`, `ForceSpec`, `rampart_hits`, `safe_mode`. Run a force-sized squad through
   `run_lifecycle` against a DEFENDED core/room and ask "does the sized force WIN?" If a
   winnability-gated (`force_sizing`) squad gets wiped, the gate is mis-calibrated OR the tactics
   under-perform — both now offline-testable.
3. **Multi-squad + K4 in the driver** — extend `run_forming` to several objectives gated by
   `claim_pacing::claims_allowed` (the claim-throttle interaction; secondary now that the backfire
   is understood as a fight-loss, not a lockup).
4. **Stale-intel give-up scenario** — the give-up *decision* is already covered by the reconcile
   kernel; a multi-tick scenario test is optional polish.

Done: K0–K4 kernels; the forming-phase driver (3/5 stall + above-economy-completes); the
**engine-engage handoff** (`run_lifecycle` — full form→engage→kill offline + deterministic);
the 87.5 backfire diagnosis (combat-effectiveness, not spawn-priority).

## What the harness CANNOT catch (keep a thin live canary)

The model omits real pathing/CPU, true intel-staleness timing, and engine quirks the sim
doesn't implement. A small live `[Lifecycle]`/`[SpawnQueue]` capture stays the final check
before trusting any deploy.
