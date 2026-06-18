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
- [ ] **Step 3 — pure `plan_kite_anchor`** (`kite.rs`, add `screeps-rover` dep to the decision crate):
  `plan_kite_anchor(view, pf: &mut dyn PathfindingProvider, room_callback, max_ops) -> Option<KitePlan{goal}>`.
  ONE `search_scored` from the centroid pricing tiles with `score_tile`. `MAX_KITE_OPS ≈ 400`. Tests with `LocalPathfinder` + synthetic matrices.
- [ ] **Step 4 — `SquadMovement` directive + `decide_squad_with_pathing`** (`lib.rs`): `enum SquadMovement { Advance{goal,range}|Kite{goal}|Hold }`;
  `SquadMemberView` gains `pos: Option<Position>` + `has_ranged`; `SquadDecision` gains `movement`,
  `center: Option<Position>`, `cohesion_radius` (and becomes `Clone`, not `Copy`, ahead of Step 7).
  `decide_squad_with_pathing(view, pf, room_callback, max_ops)` runs focus+hysteresis then `plan_kite_anchor`
  only when kiting/retreating; `decide_squad` = the same with a `NullPathfinder` (existing tests stay byte-identical).
  Consts `SQUAD_COHESION_RADIUS=2`, `CRITICAL_HP_FRACTION=0.30`.
- [ ] **Step 5 — rewrite `decide_movement` to consume the squad goal** (`lib.rs`): `SquadStateDto` gains
  `movement: SquadMovement` + `cohesion_radius`; `center` becomes the REAL centroid. Precedence:
  (1) critical-HP raw-flee → (2) immediate melee-evade [byte-identical to today, the SK-duo guard] →
  (3) squad Kite/Advance goal → (4) out-of-cohesion rejoin → (5) today's exact fallback. Pinned regression tests.
- [ ] **Step 6 — live wiring + ⚑ `WORLD_FORMAT_VERSION` 11→12** (`squad_manager.rs`, `squad_combat.rs`,
  `squad.rs`, `game_loop.rs`): build `member_views` with `pos`/`has_ranged` + the kite cost matrix (terrain+threats+towers)
  + a `ScreepsPathfinder`; call `decide_squad_with_pathing`; `TickOrders` gains `center`/`cohesion_radius`,
  `TickMovement` gains `KiteTo(Position)` (**serialized → the bump**); `execute_combat_via_seam` passes the
  REAL centroid (`tick_orders.center.unwrap_or(creep_pos)`); route the anchorless `Formation` branch through the
  pure `decide_movement` (wire it LIVE — today live still uses `fallback_movement`). Anchored AttackMission
  squads keep `execute_formation_movement`; orphan `tick_orders==None` keeps `fallback_movement`.
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
