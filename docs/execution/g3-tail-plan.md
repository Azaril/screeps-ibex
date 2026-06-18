# P2.G3-tail — Cohesive, pathfinding-scored squad movement (kite/flee) + heal-port + sim wiring

> Synthesized from the design workflow (`wpk8wt7bo`, 2026-06-18). This is the execution checklist;
> rationale is in [ADR 0008 §4.1/§5](../design/0008-combat-and-squad-architecture.md). Each step is
> **independently committable + green** (host tests + wasm build + clippy). Only **Step 6** bumps
> `WORLD_FORMAT_VERSION`.

## Decision — ANGLE B (squad owns one directive) fused with the pathfinding-scored position

The squad picks **one** movement directive + **one** pathfinding-scored goal tile per tick; every
member targets that shared goal. **Cohesion is a term in the tile score, not a per-creep clamp** —
the "centroid-clamp" (ANGLE A) is the disorganized pattern the operator named (each creep flees
independently then gets yanked back; double-prices cohesion; one search per creep). ANGLE B gives
**one bounded search per squad, cohesion as a single scoring function, the squad (not the creep)
owning the goal** — the no-one-off-pathfinding rule + "don't be disorganized with squads."

- **Critical-HP override** is the *only* sanctioned cohesion break: a member ≤ `CRITICAL_HP_FRACTION`
  (0.30) with a threat near raw-`Flee`s individually.
- **SK-duo non-regression** is structural: the per-creep "melee-only within range 2 → Flee range 3"
  evade is evaluated **before** the squad goal; when `plan_kite_anchor` returns `None`/`Hold` or
  there's no squad order, members fall through to today's exact kiting. The existing
  `decide_movement` tests + `ibex_agent_kites_a_melee_chaser` are the pinned regression gates.

## Steps

- [x] **Step 2 — pure scoring primitives** (`screeps-combat-decision`): `cohesion::centroid(&[Position])`
  (extracted, shared by live+sim); new `kite.rs` = `ThreatKind`/`KiteThreat`/`KiteTower`/
  `KiteScoreParams`/`SquadKiteView`, `tower_dps_at_range`, `score_tile(view, tile, walkable_neighbors) -> i64`
  (**lower = better cost**: SAFETY ≫ COHESION > VALUE > openness). 6 kernel tests. **DONE.**
- [x] **Step 1 — rover `search_scored`** (`screeps-rover` submodule `e6d996f`): added to `PathfindingProvider`;
  `LocalPathfinder` floods bounded (Dijkstra by `g` — `run` gained a `dijkstra` flag) + returns the min-`cost`
  reachable tile; `ScreepsPathfinder` **delegates to `LocalPathfinder`** (a bounded single-room scored search
  has no server-PF analog, and delegating makes live kite positioning byte-identical to the sim). 3 rover tests
  (rover 19). **DONE** (superproject ptr bumped).
- [x] **Step 3 — pure `plan_kite_anchor`** (`kite.rs`; `screeps-rover` dep added to the decision crate, pure
  default features): `plan_kite_anchor(view, pf, room_callback, max_ops) -> Option<KitePlan{goal}>` runs ONE
  `search_scored` from the centroid pricing tiles with `score_tile` (+ a `walkable_neighbors` openness lookup);
  `None` = holding is optimal. `MAX_KITE_OPS=400`. 2 tests (flee-to-safe-near-centroid, hold-when-safe).
  decision crate 31; bot wasm + agent crate still green (rover feature unification OK). **DONE.**
- [x] **Step 4 — `SquadMovement` directive + `decide_squad_with_pathing`** (`lib.rs`): `enum SquadMovement
  { Advance{goal,range}|Kite{goal}|Hold }`; `SquadMemberView` gained `pos`+`has_ranged`; `SquadDecision` gained
  `movement`/`center`/`cohesion_radius` (now `Clone`). `decide_squad_with_pathing(view, room_callback, max_ops)`
  runs focus+hysteresis then `plan_kite_anchor` only when kiting/retreating; `decide_squad` = the no-pathing path
  (Advance to weapon range / Hold). `kite_threats` (melee/keeper reach 2, ranged-only 0) + `kite_towers`. Consts
  `SQUAD_COHESION_RADIUS=2`/`CRITICAL_HP_FRACTION=0.30`. Manager fills the new member fields (still `decide_squad`;
  live switch is Step 6). **DONE** (decision 34).
- [x] **Step 5 — rewrite `decide_movement` to consume the squad goal** (`lib.rs`): `SquadStateDto` gained
  `movement`+`cohesion_radius`. Precedence: (1) critical-HP raw-flee → (2) immediate melee-evade [byte-identical
  to today, the SK-duo guard] → (3) squad Kite/Advance goal → (4) out-of-cohesion rejoin → (5) the prior
  `decide_movement_fallback` (only when `cohesion_radius==0`, solo/unmanaged). The 6 existing per-creep tests hit
  (5) byte-identical (live + sim adapters set `Hold`/`0` for now); 3 new precedence tests. **DONE** (decision 37,
  agent 17, bot 164, wasm green).
- [x] **Step 6 — live wiring (NO WFV bump after all)** (`squad_manager.rs`, `squad_combat.rs`, `squad.rs`): the
  manager builds `member_views` (`pos`/`has_ranged` resolved from the body) + the room's terrain-baked cost matrix
  (the formation.rs recipe) and calls `decide_squad_with_pathing`; `apply_squad_decision` stamps the squad directive
  (`squad_movement`/`squad_center`/`squad_cohesion_radius`) onto each member's `TickOrders`. The job's anchorless
  Engaged `Formation` branch now routes through the pure `decide_movement` (`execute_decide_movement` — builds the
  CombatView with the real squad goal, translates `MoveTo`/`Flee` to rover). **WFV stays 11** — the squad context
  is conveyed via `#[serde(skip)]` `TickOrders` fields (ephemeral, rewritten each tick), so the serialized shape is
  unchanged; no `TickMovement::KiteTo` was needed (the directive rides `squad_movement`). The plan's 11→12 bump was
  over-cautious. Anchored AttackMission squads keep `execute_formation_movement`; the orphan/solo `tick_orders==None`
  path keeps `fallback_movement`. **DONE** (bot 164, wasm + clippy clean).
- [ ] **Step 7 — heal-assignment → pure** (`lib.rs`, `squad_manager.rs`, `squad.rs`): port
  `SquadContext::compute_heal_assignments` (the greedy: urgency sort, range bands 12@≤1 / 4@≤3, over-heal cap,
  preemptive pass) to pure over member INDICES → `SquadDecision.heal_assignments: Vec<HealAssignment{healer_idx,target_idx,expected_heal}>`;
  `SquadMemberView` gains `id`/`damage_taken_last_tick`. Adapter maps indices→entity→`ObjectId` via `CreepOwner`.
  No WFV bump.
- [ ] **Step 8 — sim wiring + cohesion regression scenario** (`screeps-combat-agent`): `SimView::from_world`
  derives `center = cohesion::centroid(friends)`; `SimSquad::step` calls `decide_squad_with_pathing` (sim
  `LocalPathfinder` + `build_combat_matrix`), persists state, stamps the goal into each per-creep view.
  New self-play scenario EXP-SQUAD-KITE-1: ranged duo+healer vs a 5000HP hard-hitting melee keeper; assert
  focus-fire + `max_pairwise ≤ K` throughout + survival ≥ today's fallback. Closes the self-play loop (no fork).

## Future — attack-positioning via the SAME scored search (operator 2026-06-18; explore AFTER flee/kiting)

The `search_scored` + combat-pricing machinery is goal-agnostic. Once flee/kiting (Steps 1–8) lands,
explore an **attack** pricing: find the formation/positions that **maximize expected value** (damage to
the focus incl. RMA stacking, heal coverage, optimal weapon range) and **minimize damage taken**, for
the desired goal/focus target. Same `plan_*_anchor` shape, a different `score_tile`-style pricing
(e.g. `plan_engage_anchor`). Tracked as a **T-POS experiment** in [ADR 0008a](../design/0008a-combat-tactics.md);
sequenced strictly after the flee/kiting positioning is complete + validated.
