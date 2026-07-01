# ADR 0028 — Engine-backed Offline Lifecycle Harness (P-OBJ)

Status: IN PROGRESS 2026-06-27. **All five kernels K0–K4 + the forming-phase driver + the
ENGINE-ENGAGE HANDOFF LANDED** (`screeps-combat-eval/src/harness/lifecycle.rs` — the full
`objective→form→travel→engage→kill` chain is offline + deterministic). The harness then
diagnosed the live 87.5 backfire (see §Diagnosis) and now tests single/multi-room spawning +
rally/renew. **Both layers are broken: the spawn/form layer (no renew → stuck forms lose
members; + lane contention) AND combat effectiveness (squads lose defended fights).** Companion to ADR 0008 (squad
lifecycle), ADR 0027 (objective/squad lifecycle rework), ADR 0023/0023a (the combat sim
harness), ADR 0026 §9 (doctrine sizing). Task #23 / #25.

> **Current state as of 2026-07-01 (combat-feature-set audit).** Still IN PROGRESS. Sharpened
> open items: **K0 (rally), K1 (spawn-throughput reproducing the 3/5 stall), K2 (FSM transitions)
> are LANDED + GREEN** — K0/K1 are live via the bot re-exports, K2 is the canonical unit-tested
> spec that the harness drives (bot `machine!` adoption still deferred by design). **K3 (fielding —
> `slots_to_spawn` wrapping `sized_for`/`build_body`) and K4 (claim pacing — `claim_pacing`) have
> their pure kernels BUILT + tested in `screeps-combat-decision`, but the BOT adapter wiring is
> still PENDING** (no live caller of `slots_to_spawn`/`claims_allowed` yet — the harness driver is
> their only consumer). This harness is **the final offline gate for full lifecycle / force-sizing
> validation.** The surrounding combat work has since landed + deployed to MMO: rally/travel
> convergence (ADR 0034), scout-before-commit/abandon-on-contact (ADR 0035), opportunistic
> structure targeting (ADR 0036), tower-aware neighbour defense (ADR 0037), plus the
> capability-driven composition + EV assignment reworks (ADR 0031/0031a/0031b/0032).

> **Forward note (2026-06-29):** [ADR 0033](0033-rover-pathing-sim-and-benchmark.md) (Proposed) extracts the engine's movement mechanism into `screeps-sim-core` and renames `CombatWorld`→`SimWorld` / `resolve_tick`→`resolve_combat_tick`; this lifecycle harness becomes a consumer of `sim-core`, and the `CombatWorld` / `resolve_tick` references below read as their `Sim*` / split successors. No design change here. (The `Colony` model stays in `combat-eval`, outside the kernel.)

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
- **K3 — fielding (KERNEL BUILT; BOT WIRING PENDING).** `fielding::slots_to_spawn(composition,
  filled, best_capacity, per_member_cap, priority, move_profile)` wraps the shared
  `sized_for`/`composition::build_body`/`PREFERRED_MEMBER_ENERGY`: one `QueuedSpawn` per UNFILLED
  slot, body built at `min(best_capacity, per_member_cap)`, a slot no in-range home can build is
  skipped (the `None` stall), `id` = slot index. 3 tests. **Pure kernel is landed + green in
  `screeps-combat-decision::fielding`; the harness driver consumes it, but no BOT adapter calls it
  yet** (the live fielding path — `slots_to_spawn`→`build_body` token broadcast — is still to be
  wired to `queue_slot_spawn`).
- **K4 — claim pacing (KERNEL BUILT; BOT WIRING PENDING).** `claim_pacing::claims_allowed(active,
  forming, max_concurrent, max_forming)` = the tighter of the two headrooms — reproduces the
  forming-cap LOCKUP (a stuck-forming squad blocks all new claims, the `forming-cap=1` zeroing seen
  live). 3 tests. **Pure kernel is landed + green in `screeps-combat-decision::claim_pacing`; the
  harness driver consumes it, but the BOT claim-pacing adapter (gating `field_new_squad` on
  `claims_allowed`) is still PENDING.**

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

## Reach bug #2 — the proceed gate is Lanchester P(win)-driven (win-or-stall), not composition-completeness

**Operator directive (memory `combat-ev-economic-and-pwin-gating`):** the gate to PROCEED — stop
forming/holding and deploy/assault — must fire when the CURRENT PRESENT force's Lanchester P(win) meets
the requirement: the force will **WIN or STALL** (won't lose), REGARDLESS of whether the expected
archetypes are all present. If the squad as-is will win or stall, holding for more roster is pointless.
Only HOLD (wait for more) if the present force would LOSE. **Composition still SIZES the spawn; P(win)
GATES the proceed.**

This is the structural fix for the 87.5-backfire diagnosis below ("squads form → depart → engage → get
WIPED"): the pre-fix proceed gate was a roster COUNT (`rally::ready_to_depart_gate` →
`squad_ready_to_depart` / the quorum), so a squad departed on a count it could not win with, and a
winnable-but-incomplete squad needlessly HELD. The new gate decides from the **same Lanchester outcome on
the ACTUAL present force that the retreat gate uses** — so the proceed gate and the retreat gate can never
disagree about what "losing" means.

**Kernel (`screeps_combat_decision::present_force_wins_or_stalls`, lib.rs):** REUSES the private
`assess_engage` (the EXACT model the retreat gate in `decide_squad` consumes — consistency with the retreat
fix). "Win or stall" is the precise INVERSE of the present-force RETREAT (lose) condition
(`balance_retreat = our_strength > 0 && balance <= -ENGAGE_BALANCE_BAND`, plus the `unwinnable` bleed-out
veto):

```
present_force_wins_or_stalls = our_strength > 0       // a PRESENT fighting force (never trickle a
                                                       //   zero-strength roster — roster-incompleteness
                                                       //   is the rally/lifecycle layer's job; cf. #1)
                            && !unwinnable             // no irremovable incoming we can't out-heal / safe-mode
                            && balance > -ENGAGE_BALANCE_BAND  // not in the retreat/lose band:
                                                       //   a clear WIN, or a sustainable STALL around parity
```

**Wiring (`military::squad_manager`):** `present_wins_or_stalls` is OR'd into BOTH cohesion gates:
- the rally PROCEED gate — `ready_to_depart = present_wins_or_stalls || ready_to_depart_gate(count…)`;
- the gather→assault transition — `quorum_now = present_wins_or_stalls || gather_quorum_met(count…)`.

The count gates stay as the legacy/uncontested/under-strength path (a force that does NOT yet win-or-stall
still masses before committing — **no trickle-to-death**). The view + centroid passed are the SAME ones
`decide_squad` assessed this tick. Bot-only; **no `WORLD_FORMAT_VERSION` bump** (a pure read; no serialized
shape changes — the win-or-stall predicate is derived fresh each tick, no stored field).

**Offline proof (RED→GREEN, `screeps-combat-decision` lib tests):**
- `proceed_gate_fires_for_a_winning_incomplete_force` — a lone fighter (no healer archetype) that
  out-matches a weak target PROCEEDS, and the same force does not retreat (consistency).
- `proceed_gate_fires_for_a_stalling_force` — a force tuned to near-parity (our_strength ==
  enemy_strength, balance ~0) PROCEEDS (a stall, won't lose). The test pins the balance INSIDE the GENUINE
  stall band on BOTH sides — `> -ENGAGE_BALANCE_BAND` AND `<= +ENGAGE_BALANCE_BAND` — so it provably
  exercises the novel middle region the win-or-stall predicate introduces, NOT a disguised clear win.
- `proceed_gate_holds_for_a_losing_force` — an outmatched force HOLDS; and a zero-fighting-strength
  (healers-only) roster never proceeds into a defended room. The held force is exactly the one the retreat
  gate sends retreating (consistency).
- Sizing is UNCHANGED: composition (`RequiredForce`/`sized_for`) still sizes the spawn; only the
  proceed-GATE changed (the `assemble_force_*` sizing tests are untouched and green).

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

1. ~~**Implement renew live**~~ **DONE** (`ebf3623`): Phase B-renew in `squad_manager` requests
   `request_renew` for a forming squad's present members with TTL < 300, and the rally point moved to a
   home SPAWN so members are renewable. Gated on the spawn renew pass's free-spawn + room-energy checks
   (never starves spawning/economy). Caveat: under heavy spawn contention there are few free spawns to
   renew with — renew helps a slow form on a colony with idle capacity more than a contended one.
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

Still-open spawn/form issues surfaced by live verification (2026-06-27):
- **SK forming-contention** — W6N4 stuck at `1/3` (only 1 of 3 members ever spawns; the spawn is
  busy with economy). The deeper contention the priority bump couldn't fully solve.
- **Requested-size oscillation** — W9N8's objective requested-slot count flaps 1↔2 each tick (the
  producer re-sizes a player room to 1-2 members — under-sized = the combat-effectiveness layer).
  The rally gate is now robust to it, but the oscillation/under-sizing itself wants a fix.

Done: K0–K2 kernels LANDED + GREEN (K0/K1 live via bot re-exports, K2 canonical spec driven by
the harness); K3/K4 pure kernels BUILT + green (BOT adapter wiring still PENDING — the harness
driver is their only consumer today); the forming-phase driver (3/5 stall + above-economy-completes);
the **engine-engage handoff** (`run_lifecycle` — full form→engage→kill offline + deterministic);
the 87.5 backfire diagnosis (combat-effectiveness, not spawn-priority); **renew** (Phase B-renew
+ spawn-adjacent rally, `ebf3623`); the **rally-gate fix** (depart on requested-present, robust to
oscillating size — `bf021dd`, the live W9N8 stuck-at-1/1).

## What the harness CANNOT catch (keep a thin live canary)

The model omits real pathing/CPU, true intel-staleness timing, and engine quirks the sim
doesn't implement. A small live `[Lifecycle]`/`[SpawnQueue]` capture stays the final check
before trusting any deploy.
