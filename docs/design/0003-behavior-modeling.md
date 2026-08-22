# ADR 0003 — Behavior Modeling (jobs & squads)

- **Status:** Decided
- **Date:** 2026-06-09
- **Related:** Field Report F (FSM friction), Field Report A (squad cohesion), Field Report B (lifecycle hangs); IBEX-006, IBEX-029, IBEX-042, IBEX-001, IBEX-002, IBEX-002b, IBEX-015. Cross-ADR: 0001 (entity model), 0004 (CPU governance), 0005 (runtime/scheduling), 0006 (eval harness / combat sim), 0008 (combat & squad architecture — owns the squad-cohesion mechanism). See review report §1, §3, §4, §5, §8 (Behavior modeling + Squad cohesion pillars).

## Context
Jobs are state machines via **`screeps-machine`** (`MAX_STATE_TRANSITIONS=20`/tick; multi-transition-per-tick). Useful but **inflexible / hard to understand**, and the multi-transition model was suspected of underlying double-fire hazards (register-pickup/deposit). Squad/combat behavior coordinates poorly (creeps scatter instead of forming quads; cohesion requires staying **in range** to act on teammates). Prior art: Overmind's `CombatOverlords` / swarm cohesion (`../references/external-references.md`).

## Decision

Two intertwined pillars — job/mission behavior and squad cohesion — share one principle: **funnel every side effect through one guarded sink, make invariants explicit, and tolerate transient faults instead of tearing down.** Everything lands behind the **unchanged `Job`/`Mission` trait seams** so the running bot is never broken mid-increment.

### A. Jobs & missions: one guarded sink behind the existing trait seam

1. **The seam is the `Job` trait (`describe`/`pre_run_job`/`run_job`) and the `Mission` trait, verbatim.** Whatever per-job representation is used lives entirely behind it. The friction Field Report F identifies is specific and is addressed by the items below rather than by a representation swap (review §1 IBEX-006, §8 Behavior): (a) split-pass side effects (reservation in `gather_data` vs the intent in `tick()`), (b) opaque `Option` control flow with a silent 20-transition cap, and (c) untyped, unguarded intents.

2. **One guarded intent sink.** ALL intents — including combat intents (IBEX-029: bare `creep.attack/ranged_attack/heal/move` calls, "safe by luck of return value" only) — flow through a single `SimultaneousActionFlags`-style sink that does check-and-set per `(creep, intent-category)` per tick. Combat is no exception: squad-combat routes its returned intents back through the same guarded sink as every other job (`jobs/squad_combat.rs:598`, `:607`, `:655`, `.consume(SimultaneousActionFlags::ATTACK_CONTROLLER)` at `:1415`). A debug-assert backs the invariant that no intent fires twice per creep per tick.

3. **Reservations computed once per tick.** A state computes its reservation and emits its intent in one place, eliminating the gather-vs-act drift of the split-pass model.

4. **Utility AI for SELECTION only.** Utility scoring picks targets/roles (which hostile to focus, which delivery to take); it does NOT drive sequential execution. Control flow owns sequencing; utility owns choice. This avoids per-tick behavior-tree re-eval cost while keeping flexible prioritization (see Alternatives). Tactics themselves — authoritative focus-fire, kiting, centralized heal, coupled-hysteresis engage/retreat — are computed once by the `SquadManager` (ADR 0008 §4) and merely executed at the job layer, which is exactly this split. The tactical decision sits behind a JS-free DTO seam (`CombatView`→`Vec<CombatIntent>`, the combat-scoped `GameView`) so the same code runs in the deterministic combat sim (ADR 0006).

5. **REFUTATION — multi-transition is NOT the double-fire source (Field Report F reframed).** Each `run_job` threads ONE `SimultaneousActionFlags`, and `consume()` is **check-and-set**, so a guarded intent fires at most once even across multiple transitions in a tick. Do not "fix" the multi-transition model as if it caused double-firing — this refutation is recorded so it is never resurrected (review §1 IBEX-006, §8 Behavior). The genuine risk was the *unguarded* combat path (§A.2), not the transition count.

6. **Transient-error tolerance (IBEX-042).** `Mission::run_mission` returns `Result<MissionResult, ...>`, and the mission system deletes the mission on any `Err` — so a one-tick room/visibility loss (`miningoutpost.rs:119/129`, `defend.rs:215`) destroys a long-running campaign and its children. `MissionResult` therefore carries a **`Wait`/`Idle`** variant extending the `{ Running, Success }` enum (`enum MissionResult` in `missions/missionsystem.rs`), so a momentary fault **parks** the mission for the tick instead of tearing it down. This is a self-contained change: a new variant plus park-don't-delete at the transient-fault sites, with no serialized-shape change. It is the mission-layer complement to job-layer stuck recovery (IBEX-015): `check_movement_failure` is wired into job move states so a rover-abandoned creep transitions to Wait/Idle/abandon-target rather than re-issuing the same blocked move forever.

### B. Squad cohesion (Field Report A)

**Ownership.** The squad-cohesion *mechanism* — formation, orientation, rally, movement and lifecycle — is specified by **ADR 0008** and lives in `SquadManager` (`military/squad_manager.rs`) together with the combat-decision kernel and rover formation movement. This ADR does not prescribe a mover. What it owns are the invariants below and the movement-model analysis that constrains any implementation.

1. **Defense is never squad-less by construction.** A defense path that sets `squad_entity=None` and builds creeps with the squad-LESS `SquadCombatJob::new` makes `get_squad_state`/`get_tick_orders`/`get_formation_target` short-circuit to `None`, so every defense creep targets its own nearest hostile independently — scatter by construction, independent of any movement model (IBEX-001 Level 1). Defense creeps are built through `SquadCombatJob::new_with_squad` backed by a `SquadContext`. This is the dominant, always-present break and is orthogonal to formation quality.

2. **Cohesion is a measured INVARIANT with a force-abort.** Max member spread and ticks-in-non-cohesive-mode are telemetry, and **any squad non-cohesive for N ticks is force-aborted.** This closes the Field Report B hang (IBEX-002): `Rallying→Engaging` requires `squad_is_cohesive` (a scattered squad never satisfies it) while `Rallying` renews members below TTL, so the all-dead terminator never fires and the campaign neither engages nor tears down. A non-cohesive force-abort lets `all-dead` fire and gives the lifecycle (per-state wall-clock deadlines, top-down abort from `WarOperation`) a definite terminator. Members of a squad non-cohesive for >N ticks are not renewed.

3. **One squad behavior model, not three.** Exactly one squad-combat job backed by one squad model; parallel assault/harass mission types are not part of the design.

#### Movement-model analysis (retained rationale)

A deep evaluation verified against the engine source and both movement implementations found that an **anchor** — a shared coordinate frame of `virtual_pos` plus orientation — is the only model that supports orientation, rotation, present-fresh-armor, turn-from-tower, and footprint-aware pathing. **Lead-follower** (the rover `Follow`/`desired_offset`, `screeps-rover/src/movementsystem.rs:531–567,1071–1156`) carries a *fixed* dx,dy offset with **no facing**, and its leader pathfinds as a single creep, which walks the block into a 1-wide trap. Any cohesion mechanism must therefore carry a facing, not just relative offsets.

Engine ground truth confirmed firsthand and load-bearing for any mover:

- A creep may enter a tile a *moving* teammate vacates (`movement.js:22`, `!objects[i._id]` in `checkObstacleAtXY`), so a lockstep block advance and an in-place rotation both resolve within a single tick. Rotation-in-place is a 4-cycle.
- `pull` exists for creeps that cannot move themselves.

Consequent design constraints:

- **A shared anchor must follow a cached tile-path**, pathfound once through the pathfinding system and followed step-by-step, re-pathed only on invalidation/stuck (the rover `CreepPathData.path` discipline). Advancing an anchor by a straight-line `signum` step with **no pathfind** (`formation.rs:380–414`) is the actual defect behind observed scatter — not the anchor concept. The path is built with a **footprint-aware cost transform** (the Overmind "moving-maximum" `applyMovingMaximum(w,h)` recipe, generalizing `apply_quad_cost_overlay`/`apply_formation_cost_overlay` parameterized by W×H so duo/quad/larger share one path) plus `apply_tower_avoidance_costs` as pricing. Members then move in lockstep from the anchor's next step. This is *one* pathfind plus N cheap direction-moves — cheaper than N per-member `move_to` — and honors "pathfinding lives in the pathfinding system; modules supply pricing only".
- **Orientation is the anchor's payoff.** `threat_direction` plus `orient_toward()`, `reassign_slots()` (tanks/high-HP on threat-facing slots, healers in back), `mirror_y()` on retreat so the armored edge stays toward the enemy while kiting, and rotate-in-place to swap a damaged front creep for a fresh one. The corresponding primitives already exist (`squad.rs:151–183`, `squad.rs:799–897`, `formation.rs:25–139`).
- **The cohesion gate must be hard, and fatigue is answered by bodies.** Advance only when all live members are on their oriented offset tiles; a mode that *ratchets* into a looser tolerance and never re-tightens (`formation.rs:313–350`, `STRICT_HOLD_MAX_TICKS=15`) defeats the invariant. Gate the whole squad on aggregate fatigue (Overmind `swarmMove → ERR_TIRED`). For a self-mobile combat squad the fatigue answer is MOVE-balanced bodies, not `pull`.
- **Corridors relax the same mover; they do not switch primitives.** Scatter from independent pathing only happens in *open* terrain; in a 1-wide corridor there is exactly one path, so independent member moves converge — the terrain enforces single-file. When the footprint will not fit, relax two parameters: pathfind the anchor with **footprint width 1** (the moving-maximum transform at w=h=1 is a normal pathfind), and drop to a travel-oriented line / loose tolerance so members file through instead of demanding box-offset tiles that are walls. The hard gate re-forms the box on the open side automatically. The rigid↔relaxed switch keys on one signal: "does a footprint path exist". This is *one mover on a continuum of tightness* (exact offsets in the open → line/loose in chokes → loose-centroid for blobs), not two primitives with a handoff.
- **`Follow`/`pull` is reserved for its real niche, NOT corridors.** For a self-mobile squad, `Follow`'s `desired_offset` is leader-relative positioning that an anchor does better because it adds orientation. `Follow`'s unique value is `pull` integration for creeps that cannot move themselves — no-MOVE / under-MOVE'd compositions (a pulled high-part attacker, a dedicated puller/train on roads). That stays an optional rover capability for such bodies. Only if such compositions are fielded does the rover's fatigue short-circuit (which skips `pull` for a fatigued follower) need attention.
- **Arbitrary N.** Rigid offsets are reserved for **N ≤ 4** (duo = 1×2 — a pull-pair only if one member is under-MOVE'd; quad = 2×2; triangle = 3). For **5+ "blobs"**, drop rigid offsets for **loose-centroid cohesion** ("stay within N tiles of the squad centroid", path-from-center) or split into multiple anchored sub-squads under one objective.

Prior art: Overmind `Swarm.ts` rigid anchor + orientation enum + `pivot`/`swap`; footprint via the moving-maximum cost transform; consensus is rigid-in-open plus collapse-to-single-file-for-travel, the auto-switch being unpublished competitive knowledge.

### Cross-ADR ordering

- **ADR 0006 (eval harness)** supplies the replay/intent-diff infrastructure and **ADR 0004 (CPU governance + budgeted pathfinding facade)** the budget seam and cohesion telemetry that this ADR's validation depends on; both are prerequisites for a representation change or a cohesion change to be verifiable.
- Squad identity/reference stability is **ADR 0001**'s concern, not this ADR's; behavior code consumes whatever reload-stable squad reference 0001 specifies.
- The squad-cohesion mechanism is **ADR 0008**'s; the invariants in §B are the contract between them.

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep `screeps-machine`, address the specific defects (chosen) | least churn on the most-churned subsystem; each defect has a targeted fix | friction & opacity of the representation remain |
| **Behavior trees** | composable, debuggable, reactive | new framework; per-tick eval cost; heavier than the problem warrants |
| **Utility AI** (score actions) for control flow | flexible priorities | tuning; less explicit control flow — adopted for SELECTION only |
| **Data-driven / declarative FSM** rewrite | clarity; testable transition tables | highest-churn change in the most-churned subsystem, for friction that the guarded sink, single-pass reservations and transient tolerance already remove; **not adopted** |

**Squad cohesion** (Field Report A): explicit **lead-follower with hard in-range wait-gates**, or **anchor-based rigid-body movement**, or **single "fat-position" group movement** — with cohesion as an invariant in every case. Lead-follower is rejected as the primary combat model because it has no orientation and so cannot present armor or turn from a tower; it is the pulled-composition tool. The mechanism finally selected is specified in ADR 0008; the constraints any such mechanism must satisfy are §B's analysis.

## Consequences

**Positive**
- One guarded intent sink across ALL jobs (incl. combat) makes double-fire a structural impossibility rather than "safe by luck" — squad-combat becomes reasonable under the same model as every other job (IBEX-029 closed).
- Transient faults (one-tick room/visibility loss, a stuck creep) park work via `MissionResult::Wait` / job Wait/Idle instead of tearing down campaigns and their children (IBEX-042, IBEX-015 closed).
- Defense squads form up at all (IBEX-001 Level-1 wiring); squads that hold formation into combat make heal/focus — which require in-range — actually work (Field Report A).
- Cohesion-as-invariant with a force-abort gives the war lifecycle a definite terminator, closing the Field Report B hang (IBEX-002).
- The SELECTION/EXECUTION split keeps target scoring and formation geometry as pure kernels — host-target testable against fixtures per §9.

**Negative / new risks**
- Keeping the existing representation keeps its opacity: the 20-transition cap and `Option`-as-control-flow remain a readability cost that must be paid down by documentation and tests rather than by a rewrite.
- Hard wait-gates can stall a squad if a single member is permanently blocked; the **N-tick non-cohesive force-abort is the required backstop** so a stall converts to a clean teardown rather than a new hang.
- Hard wait-gates trade raw travel speed for cohesion — a squad moves at the pace of its slowest live member. Acceptable: a slow cohesive quad beats a fast scattered one, and the force-abort bounds the downside.

**CPU & tick-safety**
- The guarded sink is O(members) per squad — negligible against the pathfinding/transfer load ADR 0004 governs; squad movement draws from the single budgeted pathfinding facade (ADR 0004), never a private path.
- The utility SELECTION pass must respect the CpuGovernor (ADR 0004) and shed to a cheap fallback target under Critical; scoring must not become an un-budgeted hot loop.
- No new panic surfaces: combat intents ignore their `Result`; routing them through the sink keeps log-and-continue semantics under the ADR 0005 tick-level containment boundary.

## Incremental Migration Path

**Job/mission behavior** (prerequisite: replay-parity infrastructure from ADR 0006):
- Route combat intents through the guarded sink (IBEX-029) and add `MissionResult::Wait` transient tolerance (IBEX-042) as independent, self-contained changes; neither requires a representation change.
- Any change to a job's internal representation is piloted on **one job** behind the unchanged `Job` trait and validated by **replay intent-diff parity** — record real GameView reads/intents from the existing job, replay through the new implementation, assert byte-identical intent streams — before touching any other job.
- **Breaking change: None.** These are internal to each job; no serialized shape changes.

**Squad cohesion** (prerequisite: cohesion metric emitting):
- Wire the defense path onto `SquadContext` / `new_with_squad` (the dominant fix, independent of the mover).
- Add the N-tick non-cohesive force-abort; stop renewing non-cohesive members.
- The mover itself is ADR 0008's migration path.
- **Breaking change: Behavioral only.** `SquadContext` already serializes `virtual_pos`/`formation_mode`, so no Memory/format break and no state drop is required.

**Validation (per §7 register):** launch an attack at an unreachable room and assert teardown within the deadline; log per-tick member spread + ticks-in-loose-mode and assert the cohesion rate (fraction of combat ticks with all members in-range) rises; assign an unreachable target to a job and assert it leaves the move state within N ticks; inject a one-tick `room_data == None` and assert the mission waits rather than being deleted.
