# ADR 0003 — Behavior Modeling (jobs & squads)

- **Status:** Proposed
- **Date:** 2026-06-09
- **Related:** Field Report F (FSM friction), Field Report A (squad cohesion), Field Report B (lifecycle hangs); IBEX-006, IBEX-029, IBEX-042, IBEX-001, IBEX-002, IBEX-002b, IBEX-015. Cross-ADR: 0001 (entity model / SquadId), 0004 (CPU governance), 0005 (runtime/scheduling). See review report §1, §3, §4, §5, §8 (Behavior modeling + Squad cohesion pillars).

## Context
Current: jobs are state machines via **`screeps-machine`** (`MAX_STATE_TRANSITIONS=20`/tick; multi-transition-per-tick). Useful but **inflexible / hard to understand**, and the multi-transition model may underlie double-fire hazards (register-pickup/deposit). Squad/combat behavior coordinates poorly (creeps scatter instead of forming quads; cohesion requires staying **in range** to act on teammates). Prior art: Overmind's `CombatOverlords` / swarm cohesion (`../references/external-references.md`).

## Decision

The recommendation below matches review report §8 (authoritative). Two intertwined pillars — job/mission behavior and squad cohesion — share one principle: **funnel every side effect through one guarded sink, make invariants explicit, and tolerate transient faults instead of tearing down.** Both land behind the **unchanged `Job`/`Mission` trait seams** so the running bot is never broken mid-increment.

### A. Jobs & missions: data-driven FSM behind the existing trait seam

1. **Replace the per-job representation, not the seam.** Keep the `Job` trait (`describe`/`pre_run_job`/`run_job`) and the `Mission` trait verbatim; swap the `screeps-machine` body for a **data-driven FSM** — explicit transition tables over `Option`-as-control-flow. This removes the Field Report F friction the report actually identifies (review §1 IBEX-006, §8 Behavior): (a) split-pass side effects (reservation in `gather_data` vs the intent in `tick()`), (b) opaque `Option` control flow with a silent 20-transition cap, and (c) untyped, unguarded intents.

2. **One guarded intent sink.** ALL intents — including the currently-**UNGUARDED** combat intents (IBEX-029, `jobs/squad_combat.rs:994` UNSET created but never consumed; ~12 bare `creep.attack/ranged_attack/heal/move` sites listed at §1 IBEX-029) — must flow through a single `SimultaneousActionFlags`-style sink that does check-and-set per `(creep, intent-category)` per tick. Today haul/staticmine guard MOVE/TRANSFER/HARVEST while combat fires bare `let _ = creep.attack(...)`, "safe by luck of return value" only. Routing combat through the sink makes squad-combat the same reasoning model as every other job and adds a debug-assert that no intent fires twice per creep per tick.

3. **Reservations computed once per tick.** Fold the split-pass reservation/action into the FSM so a state computes its reservation and emits its intent in one place, eliminating the gather-vs-act drift.

4. **Utility AI for SELECTION only.** Use utility scoring to pick targets/roles (which hostile to focus, which delivery to take), NOT to drive sequential execution. The FSM owns control flow; utility owns choice. This avoids the per-tick behavior-tree re-eval cost while keeping flexible prioritization (see Alternatives).

5. **REFUTATION — multi-transition is NOT the double-fire source (Field Report F reframed).** Each `run_job` threads ONE `SimultaneousActionFlags`, and `consume()` is **check-and-set**, so a guarded intent fires at most once even across multiple transitions in a tick. Do not "fix" the multi-transition model as if it caused double-firing — record this refutation so it is never resurrected (review §1 IBEX-006, §8 Behavior). The genuine residual risk is the *unguarded* combat path (§A.2), not the transition count.

6. **Transient-error tolerance (IBEX-042).** Today `Mission::run_mission` returns `Result<MissionResult, ...>` where `MissionResult` is only `{ Running, Success }` (`missions/missionsystem.rs:117–120`), and `missionsystem.rs:254–266` deletes the mission on any `Err` — so a one-tick room/visibility loss (`miningoutpost.rs:119/129`, `defend.rs:215`) destroys a long-running campaign and its children. Add a **`MissionResult::Wait`/`Idle`** so a momentary fault parks the mission for the tick instead of tearing it down. This is the mission-layer complement to the job-layer stuck recovery (IBEX-015): wire the dead `check_movement_failure` into job move states so a rover-abandoned creep transitions to Wait/Idle/abandon-target rather than re-issuing the same blocked move forever.

### B. Squad cohesion: wire defense, then lead-follower with hard wait-gates (Field Report A)

Cohesion breaks at two independent levels (review §Executive A, §1 IBEX-001). Fix them in order:

1. **FIRST wire `SquadDefenseMission` onto `SquadContext` / `new_with_squad` (the dominant break).** The live defense path sets `squad_entity=None` (`missions/squad_defense.rs:455`) and builds creeps with the squad-LESS `SquadCombatJob::new` (`squad_combat.rs:889`), so `get_squad_state`/`get_tick_orders`/`get_formation_target` short-circuit to `None` and every defense creep targets its own nearest hostile independently (`squad_combat.rs:591/606/627`). That is scatter **by construction**. Switch defense to `SquadCombatJob::new_with_squad` (`squad_combat.rs:900`) backed by a `SquadContext`. This is the quick-win that fixes the dominant, always-present break before any Level-2 work.

2. **THEN replace the Level-2 model.** The offense path (`AttackMission`) currently uses **N independent `move_to(slot).range(0)` toward a separately-advanced shared virtual anchor** (`military/formation.rs:163` "No Follow intents are used"), gated SOFTLY: Strict advances on a 75%/3-tick quorum then drops to Loose after `STRICT_HOLD_MAX_TICKS=15` and never reliably re-tightens (`formation.rs:313–350`), while `squad_is_cohesive` drops the offset requirement entirely once `strict_hold_ticks>=15` (`attack_mission.rs:752–756`). Replace this with **lead-follower + HARD in-range wait-gates**: the rover already exposes `Follow { desired_offset }` (`screeps-rover/src/movementsystem.rs:503–534, 987–1030`), currently unused — the design-debt fork the report flags in §3 ("two parallel formation movement APIs"). Followers issue `Follow` intents off the lead; the squad **advances only when every live member can step**. Escalate to a single "fat-position" group mover only if lead-follower proves insufficient.

3. **Cohesion as an INVARIANT with a force-abort.** Make cohesion a measured invariant (max member spread, ticks-in-Loose — also the pre-rewrite telemetry quick-win, §4) and **force-abort any squad non-cohesive for N ticks.** This simultaneously closes the Field Report B hang (IBEX-002): today `Rallying→Engaging` requires `squad_is_cohesive` (a scattered squad never satisfies it) while `Rallying` renews members below TTL, so the all-dead terminator never fires and the campaign neither engages nor tears down. A non-cohesive force-abort lets `all-dead` fire and gives the supervised FSM (per-state wall-clock deadlines, top-down abort from `WarOperation`) a definite terminator. Stop renewing members of a squad non-cohesive for >N ticks.

4. **Retire orphaned dead code.** `SquadAssaultMission` / `SquadHarassMission` (`missions/squad_assault.rs`, `missions/squad_harass.rs`, registered in `missions/data.rs`) are never instantiated by `war.rs` — delete them so there is ONE squad behavior model, not three.

### Cross-ADR ordering (per §8 Sequencing — authoritative)

This ADR's work is gated and must NOT precede its dependencies:

- **ADR 0006 (eval harness, Increment 0)** and **ADR 0004 (CPU governance + budgeted pathfinding facade, Increment 1)** land first; the replay/intent-diff infra and cohesion telemetry this ADR relies on come from them.
- **ADR 0001 (entity model)** lands the **stable-ID `SquadStore`** in Increment 3. The squad-cohesion work here (Increment 4) is gated on "store stable + cohesion metric emitting." Until then the IBEX-002b raw-u32 squad-ref aliasing (`squad_combat.rs:18` `squad_entity: Option<u32>`, generation-erased) is closed by ADR 0001's generation-carrying-handle interim, NOT by this ADR.
- **The data-driven FSM (Behavior, §A) is Increment 6** — the LAST increment, gated on "replay parity infra." Squad cohesion (§B) is Increment 4 and ships earlier. They are one ADR but two separately-sequenced workstreams.

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep `screeps-machine`, simplify transition semantics | least change | friction & opacity largely remain |
| **Behavior trees** | composable, debuggable, reactive | new framework; per-tick eval cost |
| **Utility AI** (score actions) | flexible priorities; good for economy/target choice | tuning; less explicit control flow |
| **Data-driven / declarative FSM** | clarity; testable transition tables | expressiveness limits |

**Squad cohesion** (Field Report A): explicit **lead-follower with hard in-range wait-gates**, or **single "fat-position" group movement**, with cohesion as an invariant.

*Resolution:* a **data-driven FSM** for control flow (testable transition tables outweigh the expressiveness limit; the bot's per-job logic is small and finite), with **utility AI scoped to SELECTION only** so the flexible-priority benefit is captured without behavior-tree per-tick eval cost. Behavior trees are rejected as a heavier framework than the problem warrants. For cohesion, **lead-follower + hard wait-gates** is chosen first (reuses the already-built, unused rover `Follow`/`desired_offset`); single-fat-position is held as an escalation only.

## Consequences

**Positive**
- One guarded intent sink across ALL jobs (incl. combat) means double-fire is a structural impossibility, not "safe by luck" — squad-combat becomes reasonable under the same model as every other job (IBEX-029 closed).
- Transient faults (one-tick room/visibility loss, a stuck creep) park work via `MissionResult::Wait`/job Wait/Idle instead of tearing down campaigns and their children (IBEX-042, IBEX-015 closed).
- Defense squads form up at all (IBEX-001 Level-1 wiring); offense squads hold formation into combat so heal/focus (which require in-range) actually work — the war system becomes trustworthy (Field Report A).
- Cohesion-as-invariant with a force-abort gives the war lifecycle a definite terminator, jointly closing the Field Report B hang (IBEX-002) and retiring two dead mission types collapses three squad models to one.
- Data-driven transition tables and the SELECTION/EXECUTION split are pure kernels — host-target testable against fixtures (formation geometry, utility scoring) per §9.

**Negative / new risks**
- The FSM rewrite touches the most-churned subsystem; mitigated by piloting on **HaulJob with replay intent-diff PARITY** before any rollout (see Migration) and by keeping the trait seam frozen.
- Lead-follower wait-gates can stall a squad if a single member is permanently blocked; the **N-tick non-cohesive force-abort is the required backstop** so a stall converts to a clean teardown rather than a new hang.
- Hard wait-gates trade raw travel speed for cohesion — a squad moves at the pace of its slowest live member. Acceptable: a slow cohesive quad beats a fast scattered one, and the force-abort bounds the downside.

**CPU & tick-safety**
- `Follow` intents and the guarded sink are O(members) per squad — negligible vs the pathfinding/transfer load ADR 0004 governs; squad movement still draws from the single budgeted pathfinding facade (ADR 0004), not a private path.
- The utility SELECTION pass must respect the CpuGovernor (ADR 0004) and shed to a cheap fallback target under Critical; do not let scoring become an un-budgeted hot loop.
- No new panic surfaces: combat intents already ignore their `Result`; routing them through the sink keeps log-and-continue semantics under the ADR 0005 tick-level containment boundary.

## Incremental Migration Path

**Behavior FSM (Increment 6, gate: replay-parity infra from ADR 0006):**
- Pilot the data-driven FSM on **HaulJob ONLY**, behind the unchanged `Job` trait. Validate by **replay intent-diff parity** — record real GameView reads/intents from the `screeps-machine` HaulJob, replay through the new FSM, assert byte-identical intent streams — before touching any other job.
- Roll out **job-by-job** once HaulJob is at parity; combat-intent guarding (IBEX-029) and `MissionResult::Wait` transient tolerance (IBEX-042) land alongside.
- **Breaking change: None.** The FSM swap is internal to each job; no serialized shape changes. `JobData` is untouched here (its raw-u32 squad ref is ADR 0001's concern).

**Squad cohesion (Increment 4, gate: ADR 0001 SquadStore stable + cohesion metric emitting):**
- Step 1: wire `SquadDefenseMission` → `SquadContext` / `new_with_squad` (dominant fix).
- Step 2: replace virtual-anchor MoveTo with lead-follower `Follow`/`desired_offset` + hard wait-gates.
- Step 3: add the N-tick non-cohesive force-abort; stop renewing non-cohesive members.
- Step 4: delete `SquadAssaultMission` / `SquadHarassMission`.
- **Breaking change: Behavioral only.** `SquadContext` already serializes `virtual_pos`/`formation_mode`, so no Memory/format break and no state drop is required for the cohesion change.

**Validation (per §7 register):** launch an attack at an unreachable room and assert teardown within the deadline; log per-tick member spread + ticks-in-Loose and assert the cohesion rate (fraction of combat ticks with all members in-range) rises; assign an unreachable target to a job and assert it leaves the move state within N ticks; inject a one-tick `room_data == None` and assert the mission waits rather than being deleted.
