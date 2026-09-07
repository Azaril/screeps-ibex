# ADR 0028 — Engine-backed Offline Lifecycle Harness (P-OBJ)

- **Status:** Decided

An offline, deterministic harness that drives the whole `objective → form → travel → engage → kill`
chain against the authoritative `screeps-combat-engine` sim
(`screeps-combat-eval/src/harness/lifecycle.rs`): `run_forming`, `run_lifecycle`, and the defended
drivers `run_defended_lifecycle` / `run_defended_lifecycle_with(_params)`. The coordination layer —
claim pacing, fielding, spawn throughput, rally, and the engage handoff — is driven as **shared pure
kernels** so the harness exercises the *real* decisions rather than a mirror of them. This harness is
the offline gate for full lifecycle / force-sizing validation. Companion to ADR 0008 (squad
lifecycle), ADR 0027 (objective/squad lifecycle rework), ADR 0023/0023a (the combat sim harness),
ADR 0026 §9 (doctrine sizing). Task #23 / #25.

> **Substrate note.** [ADR 0033](0033-rover-pathing-sim-and-benchmark.md) extracts the engine's
> movement mechanism into `screeps-sim-core`; this lifecycle harness is a consumer of `sim-core`, and
> the `CombatWorld` / `resolve_tick` references below read as their `Sim*` / split successors. No
> design change here. (The `Colony` model stays in `combat-eval`, outside the kernel.)

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
reconcile      lifecycle::reconcile(snapshot)              // ADR 0027, shared kernel
claim-pacing   claim_pacing::plan_claims(...)              // K4 (MAX_FORMING/CONCURRENT)
field          fielding::slots_to_spawn(objective, colony) // K3 (wraps sized_for + build_body)
spawn          spawn_queue::spawn_step(home, queue)        // K1 — home crate screeps-econ-engine (ADR 0040)
  + economy_demand_fn(tick) contends for the same lanes
rally          rally::squad_ready_to_depart / should_hold_at_boundary  // K0
engage         squad.step(world); defender(world); resolve_tick(world) // eval drives it
```

### Pure-vs-ECS seam (kernels in `screeps-combat-decision`, adapters in the bot)

| Layer | Pure kernel (eval + bot share) | ECS-bound (bot adapter) |
|---|---|---|
| Reconcile | `lifecycle::reconcile` (ADR 0027) | snapshot from `objective_queue`/`squad_contexts` |
| Rally | **`rally::{squad_ready_to_depart, should_hold_at_boundary}` (K0)** | the anchor write |
| Spawn throughput | **`screeps_econ_engine::spawn_queue::spawn_step` (K1; home crate per ADR 0040)** | `SpawnQueue`/`spawnsystem` |
| FSM transitions | **`squad_fsm::next_state` (K2)** | per-tick movement/combat/recall |
| Fielding | `composition::sized_for`/`build_body` wrapped by `fielding::slots_to_spawn` (K3) | `queue_slot_spawn` token broadcast |
| Claim pacing | `claim_pacing::plan_claims` (K4) | entity mint (`field_new_squad`) |
| Tactics/travel/kill | `decide_squad` + engine | — |

### Determinism

Builds on `evaluate::run` + seeded `Rng` (the `sim_is_deterministic_over_rounds` fence,
spread-0). The only new ordering surface is the spawn queue — modeled as a **descending
`Vec`** exactly as `SpawnQueue` (no `HashMap` iteration, per the determinism-fence memory).

## Kernels

- **K0 — rally.** `squad_ready_to_depart` + `should_hold_at_boundary` (+ the
  `STRICT_QUORUM_RATIO=0.75` const and the private `is_near_room_edge_toward`) live in
  `screeps_combat_decision::rally` rather than the bot's `military::formation`; the bot re-exports
  them (`formation.rs`, `squad.rs`) so no call site has to know where they live.
- **K1 — spawn-throughput.** `spawn_step` is a deterministic, value-type mirror of the live per-room
  head-of-line spawn loop (descending priority; skip-over-capacity; **break-on-unaffordable** =
  reserve; else spawn+debit). Its driver test **reproduces the 3/5 stall offline + deterministically**:
  MEDIUM combat starves below economy, above-economy combat completes. This is where the
  spawn-priority lever is tuned — instead of guessed live. Its shared home is
  `screeps_econ_engine::spawn_queue` (ADR 0040); `fielding` emits its `QueuedSpawn` request type.
- **K2 — FSM next_state.** `squad_fsm::next_state` is the pure transition table of
  `jobs/squad_combat.rs` (MoveToRoom/CombatResponse/Engaged/Retreating), in the same priority
  order, over a `SquadFsmSnapshot`. Coverage spans every transition incl. the anti-ping-pong
  guard (never re-engage while the squad signals retreat) and the HP bars (40% respond /
  50% engaged / 80%·60% re-engage).
  - **Decision — the bot does not call `next_state`.** Each live `*::tick` interleaves its
    transition checks with movement (the arrival-engage fires AFTER the formation move, not
    before), and two transitions carry side-effects (`combat_response_start` set/clear).
    Calling `next_state` up-front would move those, a behavior risk on a *working* FSM that is
    not the bug. So the kernel is the canonical, tested spec (a sync note sits above the live
    `machine!`) and the harness drives it; bot adoption is a consequence of a tick refactor,
    not of this ADR.
- **K3 — fielding.** `fielding::slots_to_spawn(composition, filled, best_capacity,
  per_member_cap, priority, move_profile)` wraps the shared
  `sized_for`/`composition::build_body`/`PREFERRED_MEMBER_ENERGY`: one `QueuedSpawn` per UNFILLED
  slot, body built at `min(best_capacity, per_member_cap)`, a slot no in-range home can build is
  skipped (the `None` stall), `id` = slot index. Home: `screeps-combat-decision::fielding`.
  SHARING SHAPE (as built): the shared decision content — which body at what energy — IS the
  decision-crate `build_body` + `PREFERRED_MEMBER_ENERGY` both drivers call; `slots_to_spawn` is
  the HARNESS's adapter of it onto the econ spawn-queue model, and the bot's `queue_slot_spawn` is
  the LIVE adapter (token broadcast + REC-037/REC-015b stall latches + the WvC-1 defender
  spawn-readiness downsize). Routing the bot through the harness's `QueuedSpawn` shape would
  duplicate the policy, not unify it — the two adapters stay separate by design.
- **K4 — claim pacing.** SHARED KERNEL (as built, WvC-1): the live Phase C policy —
  `claim_pacing::claim_admission` (defense-aware: offense under the cap AND the forming pace;
  defense within cap + `DEFENSE_SURGE_SQUADS`) over the empire-scaled
  `claim_pacing::max_concurrent_squads` (S5-CAP) — lives in `screeps-combat-decision::claim_pacing`
  and the bot imports it (one policy, live + harness drivers). The scalar
  `claims_allowed(active, forming, max_concurrent, max_forming)` remains the OFFENSE-only budget
  the harness's forming-lockup beds exercise (it reproduces the `forming-cap=1` zeroing seen live);
  it has no defense dimension and must never be wired back into the live loop (it would re-open
  the REC-008/S5-CAP defense starvation).

### Forming-phase colony driver

`screeps-combat-eval/src/harness/lifecycle.rs` — `run_forming(ColonyFormingScenario) ->
FormingOutcome`. A deterministic tick loop over a `Colony` (homes with capacity/income, a
per-tick `EconomyPressure` of a HIGH hauler ± CRITICAL miner) that drives the REAL kernel
chain: K3 fields the unfilled slots → K1 `spawn_step` runs each home's head-of-line lane
contest (combat vs economy, cross-home de-duped) → spawns occupy a home for `part_count*3`
ticks → K0 `squad_ready_to_depart` decides departure. It reproduces the live behavior OFFLINE:
**MEDIUM combat stalls below economy; above-economy combat completes the roster** — the
spawn-priority lever, tunable offline instead of guessed on Docker. Covered by a stall case, a
complete case, and a determinism case.

The engage handoff places the formed roster into `ManagedSimSquad` and steps `resolve_tick`
against a core. The `forming-cap=1` backfire only reproduces with **multiple** squads under K4
claim pacing — a single-squad scenario cannot show it.

## Coordination-layer settings the harness must reproduce

- Rally-until-full gate (K0 logic) — squads group up, no lone lead.
- `spawn_priority_for` MEDIUM+ → HIGH (forming combat above the economy bulk) + a forming-cap.
- Per-member energy cap (`PREFERRED_MEMBER_ENERGY=3000`) in BOTH `sized_for` and
  `queue_slot_spawn` — every spawned member (sized OR template-fallback) is bankable.
- **Rejected setting:** forming combat at 87.5 + `forming-cap=1` — measured to zero combat
  spawning (see Diagnosis). The settled point is HIGH + forming-cap=2 + the bankable-body cap.

## Reach bug #2 — the proceed gate is Lanchester P(win)-driven (win-or-stall), not composition-completeness

**Operator directive (memory `combat-ev-economic-and-pwin-gating`):** the gate to PROCEED — stop
forming/holding and deploy/assault — must fire when the CURRENT PRESENT force's Lanchester P(win) meets
the requirement: the force will **WIN or STALL** (won't lose), REGARDLESS of whether the expected
archetypes are all present. If the squad as-is will win or stall, holding for more roster is pointless.
Only HOLD (wait for more) if the present force would LOSE. **Composition still SIZES the spawn; P(win)
GATES the proceed.**

This is the structural fix for the 87.5-backfire diagnosis below ("squads form → depart → engage → get
WIPED"): a proceed gate keyed on roster COUNT (`rally::ready_to_depart_gate` →
`squad_ready_to_depart` / the quorum) lets a squad depart on a count it cannot win with, and makes a
winnable-but-incomplete squad needlessly HOLD. The gate instead decides from the **same Lanchester outcome on
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
`decide_squad` assessed this tick. This is a pure read — no serialized shape changes; the win-or-stall
predicate is derived fresh each tick, with no stored field.

**Offline gates (`screeps-combat-decision` lib tests):**
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
  proceed-GATE changed (the `assemble_force_*` sizing tests are untouched).

## Diagnosis — the 87.5 backfire

Two live captures under the backfired config (forming combat at 87.5 + `forming-cap=1`):

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
engagements.** The spawn-priority/forming-cap tuning therefore settles at the safe **HIGH +
forming-cap=2**; `run_lifecycle` proved the engage WORKS against an *undefended* core, which
makes the *defended* case the question the harness has to answer.

## Spawn/form layer — single/multi-room + rally/renew

`run_forming` models member TTL (`CREEP_LIFE_TIME`) + death-by-age + optional renew. Findings:

- **Single-room** spawning forms the roster (serial); **multi-room** forms it FASTER (parallel,
  asserted `multi < single`).
- **No-renew member-death is REAL.** A stuck/slow form (forming-span > a member's life) loses its
  early members to old age → they drop to unfilled → re-spawn → the roster never has the full set
  present at once → never departs. The live bot exhibited exactly this: `request_renew` had zero
  callers, and live forms were stuck >1500t (> `CREEP_LIFE_TIME`), so the early members aged out.
- **Renew fixes it**, at a spawn-lane cost: keeping the rallying roster alive completes the stuck
  form.

⇒ **Renew is part of the design.** Phase B-renew in `squad_manager` requests `request_renew` for a
forming squad's present members with TTL < 300, and the rally point is a home SPAWN so members are
renewable at all. It is gated on the spawn renew pass's free-spawn + room-energy checks, so it never
starves spawning/economy. Caveat that follows from that gating: under heavy spawn contention there
are few free spawns to renew with — renew helps a slow form on a colony with idle capacity more than
a contended one. The rally gate departs on requested-present, which keeps it robust to an oscillating
requested size.

## Scenario coverage

The harness's scenario set, and the failure classes each one exists to pin:

1. **Forming under lane contention** — the 3/5 stall and the above-economy completion (K1 × K3 × K0).
2. **Graded-defender engage (combat effectiveness).** `assemble_single_room` takes `towers`,
   `ForceSpec`, `rampart_hits`, `safe_mode`. A force-sized squad runs through `run_lifecycle`
   against a DEFENDED core/room, asking "does the sized force WIN?" If a winnability-gated
   (`force_sizing`) squad gets wiped, either the gate is mis-calibrated OR the tactics
   under-perform — both offline-testable, which is the whole point of the harness.
3. **Multi-squad + K4 claim pacing** — several objectives gated by `claim_pacing::claims_allowed`,
   the only shape that reproduces the `forming-cap=1` claim-throttle lockup.
4. **Stale-intel give-up** — the give-up *decision* is covered by the reconcile kernel; a
   multi-tick scenario adds the timing dimension.
5. **Spawn-contention starvation** — a home whose spawn is monopolized by economy fields only 1 of
   3 members (the live W6N4 shape): the contention the priority bump alone cannot solve.
6. **Requested-size oscillation** — an objective's requested-slot count flapping 1↔2 each tick
   because the producer re-sizes a player room (the live W9N8 shape). The rally gate must be robust
   to the flap; the oscillation itself indicts the sizing/combat-effectiveness layer.

## What the harness CANNOT catch (keep a thin live canary)

The model omits real pathing/CPU, true intel-staleness timing, and engine quirks the sim
doesn't implement. A small live `[Lifecycle]`/`[SpawnQueue]` capture stays the final check
before trusting any deploy.

## Design deltas (2026-09-07)

- **The four Seam-7 reconcile inputs are shared-kernel computations, exercised end-to-end (WS-CLOSE
  lane (b1), parity M20–M23, RULING-10 (iv)).** The "kernels in `screeps-combat-decision`, adapters in
  the bot" seam above now also covers the lifecycle CLOCKS, not only the verdicts: the REC-003 / ADR
  0035 FU2 give-up clock is `lifecycle::{RetreatClock, MAX_RETREAT_BUDGET}` and the REC-036 enemy-HP
  stall streak is `lifecycle::EnemyStallTracker` (both pure, `Copy`, ephemeral — never serialized);
  the ADR 0042 §5 economic give-up latch is `screeps_econ_decision::spawn_policy::{EconomicGiveUp,
  FORMING_ABANDON_STREAK}`. The live `SquadManager` and every harness reconcile driver advance the
  SAME objects, so the harness's "MUST mirror the bot's" constant block does not grow (only
  `COMMITMENT_BUDGET` / `MAX_FORMING_BUDGET` / `MAX_TRAVEL_BUDGET` remain mirrored — a follow-up of
  the same shape).
- **`forming_in_flight` is MEASURED on both sides (decision D3, Option A).** Live: `queue_slot_spawn`
  returns whether it queued; the ephemeral `queued_last_tick` set feeds `forming && (queued_last_tick
  || a member is still spawning)`. The harness churn drivers' composite (`completing || syncing ||
  slots_to_spawn non-empty`) is the same signal one tick earlier; the one-member flow drivers keep
  in-flight ≡ forming because their forming IS the one spawn in flight. Consequence: a forming roster
  whose remainder can never be queued lapses its +400 lease instead of holding a claim slot to the
  3000t backstop (`unbuildable_remainder_lapses_the_forming_lease`).
- **Economic give-up in the drivers.** `forming_budget_remaining = clock && !economic_giveup`, with
  the burn priced from the PRESENT slots' K3 body cost and `ChurnTarget.target_safe_mode` carrying the
  exemption. Existing multi-slot fixtures are re-based from `objective_rate_milli: 0` (which now means
  "worthless objective") to a covering rate, so they keep pinning the lease/travel envelope.
- **D28 vacuous clear in the flows.** `ChurnTarget.live_visible_clear` and
  `V1FlowScenario.{is_defend, live_visible_clear}` feed the manager's exact evidence form; the new
  `ChurnOutcome::VacuouslyResolved` reports the kernel's literal verdict and
  `ChurnOutcome::Reassigned.vacuous_reassignments` counts rebinds driven by it.
- **New driver `run_stall_flow` (the in-room phase every other driver exits before).** Scripts the
  fight per tick (`StallScript`), runs Phase A → Phase B in the manager's order, and returns the
  kernel's literal terminal (`StallOutcome`). It is where the FU2 probe-bounce zombie, the frozen
  disengaged streak, and REC-061 (resolved dominates the exhaust tick) are pinned end-to-end.
- **Scenario coverage note.** `oversized_defense_roster_churns_never_deploys` no longer exercises the
  in-flight refresh: today's `optimize_composition` sizes that roster small enough to complete inside
  one lease window (engage at ~t330). The trickle-bank M23 bed (two 3000e members, 3e/t) is now the
  fixture whose forming genuinely outlasts `COMMITMENT_BUDGET`.
- **Bed 3 is a CLAIM BOARD, and it drives BOTH K4 arms (WS-CLOSE Phase B, 2026-09-07).**
  `run_multi_forming(MultiSquadFormingScenario)` runs the live `SquadManager` phase order per tick —
  Phase A `lifecycle::reconcile` (shared kernel; M20 measured in-flight, M23 economic give-up) → Phase C
  K4 claim → Phase B K3 field + K1 `spawn_step` over the SHARED home lanes (claim-order requests,
  cross-home de-dup, per-objective `homes_in_range` = the `MAX_SPAWN_DISTANCE` filter) → K0 proceed gate
  — over a ranked `Vec` board (`BoardObjective`: composition, `is_defense`, `available_at`, static or
  ADR 0042 value bid, `fight_ticks`, `wins_or_stalls_at`). `ClaimArm::ClaimsAllowed{max_concurrent,
  max_forming}` is the offense-only budget the bed text names (it REPRODUCES the `forming-cap=1`
  lockup); `ClaimArm::ClaimAdmission` is the live S5-CAP policy over `max_concurrent_squads(homes)`
  (it SHOWS the fix). The proceed gate is `d9_proceed_gate`: the live composition
  `winnable_fast_path_allowed || ready_to_depart_gate || deploy_then_retreat_allowed` (ADR 0029 D9 AS
  BUILT is the P(win) fast-path over a live-visible owned room — there is no kind-based bypass; the
  Lanchester verdict is a fixture present-count, the driver has no room view) vs the pre-D9 count-only
  `squad_ready_to_depart`. A `Resolved` fight withdraws the objective from the board; a `GaveUp +
  mark_unwinnable` backs it off for the rest of the budget (the live floor ≥2000t exceeds every bed's
  remainder); a Defend `GaveUp` returns to the board and re-claims (generation counted). Refills for a
  departed squad and renew are out of the bed's scope. Beds 5/6 stay out (WvC-2 ruling names 1+3).
- **What bed 3 measured.** (i) The lockup shape (one trickle home, a heavy quad whose 4th member lands
  after its 1st aged out, two finishable duos ranked behind it): `claims_allowed(max_forming=1)` at
  87.5 claims only the quad, standing combat peaks at N-1 for 2500t, the duos are never claimed, and the
  unaffordable 87.5 slot head-of-line-breaks the HIGH hauler to ZERO spawns — the live capture's
  "combat: 2 2 2 2 0 2 3 / carry dipped" in one fixture. Under `claim_admission` + value bids both duos
  form, depart and resolve beside the still-stuck quad. (ii) The defender board (ADR 0029 §11, four
  defense quads two-per-home at 5e/t): under the count-only gate + offense-shaped pace the first
  defender per home sits at 3/4 for the whole budget and the one queued behind it never fields a member
  — it re-claims every +400 (a Defend GaveUp is never backed off), 6 generations; under D9/D10 as built
  all four deploy at their winning duo. (iii) S5-CAP: a defense claim appearing with the offense board
  at `max_concurrent_squads(2) = 3` is admitted the same tick (`active_at_claim == cap`) while the 4th
  offense claim stays refused; the offense-only budget never claims it. (iv) The completing board (six
  duos, four homes): `forming-cap=2` runs exactly two rosters in parallel, keeps the hauler spawning, and
  finishes the board earlier than `forming-cap=1`. Bed 1 at N=2 over shared lanes: MEDIUM fields no
  member on either home (both lapse at +400); 87.5 completes both, serialized, the second later than the
  lone roster.
- **The 0041 §7 P3 boosted lifecycle bed lives here too** (`run_boosted_forming`) — see ADR 0041's
  2026-09-07 delta.

## Landed

- 2026-09-07 WS-CLOSE Phase B (lane B(i)): bed 3 (`run_multi_forming`, both K4 arms, pins
  `forming_cap_one_locks_the_claim_board` / `claim_admission_unlocks_the_board_behind_the_stuck_roster`
  / `forming_cap_two_at_high_serializes_and_completes` / `forming_cap_one_finishes_the_completing_board_later`
  / `four_defenders_stall_at_n_minus_one_under_the_count_gate` / `four_defenders_deploy_with_d9_d10` /
  `defense_claim_admitted_past_a_full_offense_board` / `multi_forming_is_deterministic`) + bed 1 re-run
  at N=2 over shared lanes (`n_squads_below_economy_never_field_over_shared_lanes` /
  `n_squads_above_economy_complete_every_roster_over_shared_lanes`); every pin RED-verified against the
  un-fixed arm. The thin live canary (offense-soak `[Lifecycle]`/`[SpawnQueue]` capture) is the Phase C
  Docker pass.
- 2026-09-07 WS-CLOSE lane (b1): M20–M23 both sides (shared `RetreatClock`/`EnemyStallTracker`/
  `EconomicGiveUp`, measured `forming_in_flight`, `run_stall_flow`, D28 flow pins).
- `ebf3623` Phase B-renew + spawn-adjacent rally point for forming squads
- `bf021dd` rally gate departs on requested-present, robust to oscillating requested size
