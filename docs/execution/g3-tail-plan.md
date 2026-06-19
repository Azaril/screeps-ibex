# P2.G3-tail — Cohesive, pathfinding-scored squad movement (kite/flee) + heal-port + sim wiring

> **⚠ ARCHIVED (2026-06-18).** All 8 steps are DONE. The remaining forward-looking items (T-POS attack-positioning, L1 cross-room flee, L2 trait-based lazy view) are tracked in the master doc [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) §5; landed history with SHAs is in [`phase-2.md`](phase-2.md) §2.0 (ao/ap/aq). NOTE: the "Only Step 6 bumps WORLD_FORMAT_VERSION" line below is stale — no g3-tail step bumped WFV (Step 6 itself records the 11→12 bump was over-cautious; WFV stayed 11). Preserved as the kiting/cohesion design record.

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
- [x] **Step 7 — heal-assignment → pure** (`lib.rs`, `squad_manager.rs`): ported the greedy
  (`assign_heals`: urgency sort, range bands 12@≤1 / 4@≤3, over-heal cap, preemptive pass) pure over member
  INDICES → `SquadDecision.heal_assignments: Vec<HealAssignment{healer_idx,target_idx,expected_heal}>`;
  `SquadMemberView` gained `damage_taken_last_tick` (the index resolution makes `id` unnecessary — the adapter
  has the entity). The manager resolves `target_idx → entity → ObjectId` via `CreepOwner` and stamps each
  assigned healer's `tick_orders.heal_target`, replacing the `ctx.compute_heal_assignments`/`apply` calls (those
  stay for the retreat path). 2 tests. **DONE** (decision 39, bot 164, wasm + clippy clean).
  No WFV bump.
- [x] **Step 8 — sim wiring + cohesion regression scenario** (`screeps-combat-agent`): new `ManagedSimSquad`
  (the anchorless manager path: builds a `SquadView` from living members, runs `decide_squad_with_pathing` with the
  sim's `build_combat_matrix`, then per-creep `decide_combat`+`decide_movement` via the new `SimView::view_for_with`
  / `hostiles()`/`structures()` accessors) — exactly the live `SquadManager`+`SquadCombatJob` path, no fork. New
  self-play scenario **EXP-SQUAD-KITE-1**: a ranged attacker + healer vs a high-HP melee keeper — asserts the squad
  focus-fires it down, the duo stays cohesive (`max_pairwise ≤ 4` every tick), and survives (kited to shooting range,
  no melee taken). **DONE** (agent 18, decision 39, bot 164, wasm + clippy clean). *(Tuning: `should_kite` fires only
  when a melee threat is ≤3 of the centroid — farther out the squad Advances to weapon range first, else the cohesion
  term out-weighs value and it sits out of range.)*

**G3-tail COMPLETE** — cohesive, pathfinding-scored kiting is pure + simulatable + wired live + self-play-validated.

## Future — attack-positioning via the SAME scored search (operator 2026-06-18; explore AFTER flee/kiting)

The `search_scored` + combat-pricing machinery is goal-agnostic. Once flee/kiting (Steps 1–8) lands,
explore an **attack** pricing: find the formation/positions that **maximize expected value** (damage to
the focus incl. RMA stacking, heal coverage, optimal weapon range) and **minimize damage taken**, for
the desired goal/focus target. Same `plan_*_anchor` shape, a different `score_tile`-style pricing
(e.g. `plan_engage_anchor`). Tracked as a **T-POS experiment** in [ADR 0008a](../design/0008a-combat-tactics.md);
sequenced strictly after the flee/kiting positioning is complete + validated.

## Known limitations & future evaluations (operator 2026-06-18)

### L1 — single-room flee/kite (the local search never leaves the room) ⚠
`LocalPathfinder::search_scored` (and therefore `plan_kite_anchor`, `decide_movement`'s goal, and the
retreat centroid) search a **single room**. The kite/flee goal is always *within the current room* — a
squad **cannot flee across a room boundary**. This is correct for the normal case (a squad fights in one
room; cross-room travel is the separate `MoveToRoom`/objective phase that uses the multi-room server
pathfinder). But it is a real edge: a squad **cornered at a room edge**, with the threat between it and
the room interior, can't escape into the (safer) adjacent room — it picks the best in-room tile and may
stay in danger / get pinned. **Document + watch on the Docker soak.** **Concrete recurring trigger (P2.K5, 2026-06-18):** when an invader stronghold appears in a farmed SK room, the SK farm stands down (withdraws the objective + halts mining), but creeps already inside can't flee *across* the boundary and the K0 reflex ignores towers — so the last in-flight duo/miners can take tower fire on the way out. This is the first standing consumer that makes the L1 fix worth scheduling. Fixes if it bites: (a) a **hybrid**
— when the local scored search can't find a safe in-room tile (cornered), fall back to the server
`PathFinder`'s **multi-room flee** (`search_many(flee)` is multi-room) for the "just get out" case
(live-only; the sim stays single-room, so this path wouldn't be self-play-validated); or (b) extend the
scored search to multi-room (heavier — multi-room cost matrices + an exit-aware score). Prefer (a) as a
narrow cornered-escape fallback; keep the single-room scored search as the primary (cohesive) kite.

### L2 — evaluate a trait-based combat view (avoid the per-tick DTO copy on the live path)
Today the live adapter **eagerly copies** every `Creep`/`StructureObject` into JS-free DTOs
(`CombatCreepDto`/`CombatStructureDto` via `creep_to_dto`/`structure_to_dto`) **each combat tick**, and
builds `Vec<DTO>` per creep decision. The **pathfinding system instead abstracts the data source with
traits** (`CreepHandle`, `CostMatrixDataSource`, `PathfindingProvider`): the live impl reads `game::*`
**lazily** through the trait, the sim impl reads its own world — **no eager copy**. **Evaluate applying
the same pattern to the combat seam:** make the creep/structure view a **trait** (e.g. `CombatCreep`,
live `impl` over `screeps::Creep`, sim `impl` over `SimCreep`) and make the decisions generic over it,
so the live path reads `game::*` lazily with no per-tick DTO allocation. **Trade-offs:** *pro* — drops
the per-tick copy/alloc (CPU + GC on the hot live combat path); *con* — the decisions become generic over
a trait (lifetimes, heavier signatures), and the current **value-over-DTO** design is what keeps the
`IntentRecorder` digest + the kernel/sim tests simple (DTOs are trivially constructed in tests, and the
sim builds them once from `CombatWorld`). **Measure first** — the DTO build reads *cached* `RoomData`
(cheap-ish), so the copy may not be worth the trait complexity; gate the change on a measured live CPU
win. Mirrors the rover trait pattern (the precedent this would extend).
