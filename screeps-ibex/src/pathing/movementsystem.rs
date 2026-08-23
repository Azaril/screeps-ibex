use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::room::data::*;
use crate::room::room_status_cache::RoomStatusCache;
use crate::visualize::Visualizer;
use screeps::*;
use screeps_rover::screeps_impl::{ScreepsCostMatrixDataSource, ScreepsPathfinder};
use screeps_rover::*;
use serde::*;
use shrinkwraprs::*;
use specs::prelude::*;
use specs::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Shrinkwrap, Component, Serialize, Deserialize, Clone, Default)]
#[shrinkwrap(mutable)]
#[serde(transparent)]
pub struct CreepRoverData(pub CreepMovementData);

// ─── G-13 TRUE wasted-move counter (ADR 0033 §D8 L6; the bench.rs "G-13 canary alignment" note
// is the spec, gap (1)) ────────────────────────────────────────────────────────────────────────
//
// Definition: a creep that was ISSUED a move intent last tick (a real `move_direction` /
// `move_pulled_by` call the engine accepted at registration), did NOT change position, and has
// `fatigue == 0` this tick = ONE wasted move — the live analogue of the offline `IntentAudit`
// failed-move classes ("intent spent, no action"), reconciled issued-vs-moved. This is an EVENT
// count (one per dropped intent), unlike `move_failures` below, which is the rover SELF-REPORTED
// give-up LEVEL (`Failed` + `Stuck ≥ 10`, re-counted every tick an episode persists) — both are
// emitted side by side in the seg-57 `pathing` block so the soak can compare them.
//
// Issue-time capture is the [`IntentRecordingCreep`] seam: rover only ever touches creeps
// through `MovementSystemExternal::get_creep`, so wrapping the returned handle records EXACTLY
// the intents rover spent — resolver denial-stays, stationary occupants (fatigued / spawning /
// border-crossers), and CPU-cap skips all issue no intent and are structurally excluded (no
// `Moving`-result heuristics, which conflate all of those).

/// One issued-move record: rover spent a `move_direction`/`move_pulled_by` intent for this creep
/// this tick. Keyed by [`ObjectId`] (stable across ticks, immune to specs entity recycling).
struct IssuedMoveIntent {
    id: ObjectId<Creep>,
    /// Position at issue time — the wasted predicate: any EXECUTED move changes it.
    from: Position,
    /// `from + dir` (the puller's tile for a pull follower). Attribution/debug resolution only;
    /// not consulted by the predicate.
    #[allow(dead_code)]
    expected: Position,
}

thread_local! {
    /// The tick-to-tick issue buffer: written after `process()`, drained + reconciled at the
    /// START of the next tick's run, before new requests. EPHEMERAL heap state by design — NOT a
    /// serialized component, NO WFV interaction; a VM reset loses exactly one tick of the metric.
    static G13_ISSUED_LAST_TICK: RefCell<Vec<IssuedMoveIntent>> = const { RefCell::new(Vec::new()) };
}

/// [`CreepHandle`](screeps_rover::traits::CreepHandle) wrapper that delegates to the real
/// [`screeps::Creep`] and records accepted move intents into the shared per-tick log. Rc clone
/// per `get_creep` + one Vec push per issued move — the whole counter's per-tick cost.
struct IntentRecordingCreep {
    inner: Creep,
    log: Rc<RefCell<Vec<IssuedMoveIntent>>>,
}

impl IntentRecordingCreep {
    fn record(&self, expected: Position) {
        // `try_id` is None only while spawning; rover never issues moves for spawning creeps
        // (stationary-occupant model), so a skip here loses nothing.
        if let Some(id) = self.inner.try_id() {
            self.log.borrow_mut().push(IssuedMoveIntent {
                id,
                from: HasPosition::pos(&self.inner),
                expected,
            });
        }
    }
}

impl screeps_rover::traits::CreepHandle for IntentRecordingCreep {
    fn pos(&self) -> Position {
        HasPosition::pos(&self.inner)
    }

    fn fatigue(&self) -> u32 {
        Creep::fatigue(&self.inner)
    }

    fn spawning(&self) -> bool {
        Creep::spawning(&self.inner)
    }

    fn move_direction(&self, dir: Direction) -> Result<(), String> {
        let result = Creep::move_direction(&self.inner, dir).map_err(|e| format!("{:?}", e));
        if result.is_ok() {
            let from = HasPosition::pos(&self.inner);
            // Off-edge steps can't fail here (rover models border crossers as stationary
            // occupants and never issues them), but degrade to `from` rather than panic.
            self.record(from.checked_add_direction(dir).unwrap_or(from));
        }
        result
    }

    fn pull(&self, other: &Self) -> Result<(), String> {
        // The puller's own step is a separate `move_direction`; `pull` itself moves nobody.
        Creep::pull(&self.inner, &other.inner).map_err(|e| format!("{:?}", e))
    }

    fn move_pulled_by(&self, other: &Self) -> Result<(), String> {
        let result = Creep::move_pulled_by(&self.inner, &other.inner).map_err(|e| format!("{:?}", e));
        if result.is_ok() {
            // A pull follower steps into the puller's vacated tile.
            self.record(HasPosition::pos(&other.inner));
        }
        result
    }
}

/// The G-13 predicate over one issued move, observed one tick later: `observed` is
/// `Some((position, fatigue))` for a live creep, `None` for one that died (or is otherwise
/// unresolvable — never counted). Wasted ⇔ position unchanged ∧ unfatigued: an executed move
/// always changes position (border relocation included), and a fatigued immobile creep is the
/// offline `failed_fatigued` class the live definition deliberately EXCLUDES (fatigue is
/// self-explaining in live telemetry; bench.rs spec gap (1)).
fn issued_move_wasted(issued_from: Position, observed: Option<(Position, u32)>) -> bool {
    match observed {
        Some((pos, fatigue)) => pos == issued_from && fatigue == 0,
        None => false,
    }
}

/// Fold [`issued_move_wasted`] over the tick's reconciliation set. Creeps with NO issued record
/// never enter the iterator — "no issue ⇒ not counted" is structural. Order-independent count:
/// no map-iteration-order dependence can affect the value.
fn count_wasted_moves(records: impl IntoIterator<Item = (Position, Option<(Position, u32)>)>) -> u32 {
    records
        .into_iter()
        .filter(|(from, observed)| issued_move_wasted(*from, *observed))
        .count() as u32
}

#[derive(SystemData)]
pub struct MovementUpdateSystemData<'a> {
#[allow(dead_code)] // FOLLOW-UP (ws-triage 2026-08-23): unused fetch/field — remove in the SystemData cleanup pass
    entities: Entities<'a>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    movement_results: WriteExpect<'a, MovementResults<Entity>>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    creep_movement_data: WriteStorage<'a, CreepRoverData>,
    job_data: ReadStorage<'a, crate::jobs::data::JobData>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    cost_matrix_cache: WriteExpect<'a, CostMatrixCache>,
    room_status_cache: ReadExpect<'a, RoomStatusCache>,
    visualizer: Option<Write<'a, Visualizer>>,
    governor: Read<'a, crate::cpugovernor::GovernorSnapshot>,
    metrics: Write<'a, crate::metrics::MetricsState>,
    features: Read<'a, crate::features::Features>,
}

/// Movement visualizer that pushes intents to the screeps-ibex room
/// visualizer system, which batches and flushes all visuals at end of tick.
struct IbexMovementVisualizer<'a> {
    visualizer: &'a mut Visualizer,
}

impl<'a> MovementVisualizer for IbexMovementVisualizer<'a> {
    fn visualize_path(&mut self, creep_pos: Position, path: &[Position]) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let points: Vec<(f32, f32)> = path.iter().map(|p| (p.x().u8() as f32, p.y().u8() as f32)).collect();
        let style = PolyStyle::default().stroke("blue").stroke_width(0.2).opacity(0.5);
        room_vis.poly(points, Some(style));
    }

    fn visualize_anchor(&mut self, creep_pos: Position, anchor_pos: Position) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;

        let circle_style = CircleStyle::default()
            .fill("#ff8800")
            .radius(0.15)
            .opacity(0.5)
            .stroke("#ff8800")
            .stroke_width(0.02);
        room_vis.circle(cx, cy, Some(circle_style));

        let ax = anchor_pos.x().u8() as f32;
        let ay = anchor_pos.y().u8() as f32;
        if (ax - cx).abs() > 0.01 || (ay - cy).abs() > 0.01 {
            let line_style = LineStyle::default().color("#ff8800").opacity(0.25);
            room_vis.line((cx, cy), (ax, ay), Some(line_style));
        }
    }

    fn visualize_immovable(&mut self, creep_pos: Position) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;
        let d = 0.15;
        let style = LineStyle::default().color("#ff4444").opacity(0.6);
        room_vis.line((cx - d, cy - d), (cx + d, cy + d), Some(style.clone()));
        room_vis.line((cx - d, cy + d), (cx + d, cy - d), Some(style));
    }

    fn visualize_stuck(&mut self, creep_pos: Position, ticks: u16) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;

        let circle_style = CircleStyle::default()
            .fill("#ffcc00")
            .radius(0.2)
            .opacity(0.6)
            .stroke("#ffcc00")
            .stroke_width(0.03);
        room_vis.circle(cx, cy, Some(circle_style));

        let text_style = TextStyle::default().color("#ffcc00").font(0.4).stroke("#000000").stroke_width(0.03);
        room_vis.text(cx, cy + 0.55, format!("{}", ticks), Some(text_style));
    }

    fn visualize_failed(&mut self, creep_pos: Position) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;

        let circle_style = CircleStyle::default()
            .fill("#ff0000")
            .radius(0.2)
            .opacity(0.7)
            .stroke("#ff0000")
            .stroke_width(0.03);
        room_vis.circle(cx, cy, Some(circle_style));
    }
}

#[allow(dead_code)] // FOLLOW-UP (ws-triage 2026-08-23): unused fetch/field — remove in the SystemData cleanup pass
struct MovementSystemExternalProvider<'a, 'b> {
    entities: &'b Entities<'a>,
    creep_owner: &'b ReadStorage<'a, CreepOwner>,
    creep_movement_data: &'b mut WriteStorage<'a, CreepRoverData>,
    room_data: &'b ReadStorage<'a, RoomData>,
    mapping: &'b Read<'a, EntityMappingData>,
    room_status_cache: &'b RoomStatusCache,
    derelict_features: crate::features::DerelictFeatures,
    /// The G-13 issue-time recorder sink (see the counter block above): every creep handle rover
    /// receives shares this log; drained into `G13_ISSUED_LAST_TICK` after `process()`.
    issued_moves: Rc<RefCell<Vec<IssuedMoveIntent>>>,
}

impl<'a, 'b> MovementSystemExternal<Entity> for MovementSystemExternalProvider<'a, 'b> {
    type Creep = IntentRecordingCreep;

    fn get_creep(&self, entity: Entity) -> Result<IntentRecordingCreep, MovementError> {
        let creep_owner = self.creep_owner.get(entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        Ok(IntentRecordingCreep {
            inner: creep,
            log: self.issued_moves.clone(),
        })
    }

    fn get_creep_movement_data(&mut self, entity: Entity) -> Result<&mut CreepMovementData, MovementError> {
        if !self.creep_movement_data.contains(entity) {
            let _ = self.creep_movement_data.insert(entity, CreepRoverData::default());
        }

        self.creep_movement_data
            .get_mut(entity)
            .map(|m| &mut m.0)
            .ok_or("Failed to get creep movement data".to_owned())
    }

    fn get_room_cost(&self, from_room_name: RoomName, to_room_name: RoomName, room_options: &RoomOptions) -> Option<f64> {
        let from_status = self.room_status_cache.get_or_insert(from_room_name);
        let to_status = self.room_status_cache.get_or_insert(to_room_name);
        if !can_traverse_between_room_status(from_status, to_status) {
            return None;
        }

        let dynamic_visibility_data = self
            .mapping
            .get_room(&to_room_name)
            .and_then(|target_room_entity| self.room_data.get(target_room_entity))
            .and_then(|target_room_data| target_room_data.get_dynamic_visibility_data());

        if let Some(dynamic_visibility_data) = dynamic_visibility_data {
            // REC-024 parity-by-construction: the hostile predicate and the
            // room-price tiers are the SHARED `routepricing` kernels — the same
            // derivation `economy_route_cost` prices claim/economy routes with —
            // so a route the router accepts is never a route this mover refuses.
            // The derivation (rationale in `routepricing`):
            // - a DERELICT room (hostile-owned but militarily dead at the last
            //   sighting; raw `derelict()`, deliberately not the stricter
            //   `confirmed_derelict` — fresh-intel gating deadlocked the very
            //   creeps that refresh intel) is not hostile for pathing;
            // - hostile-CREEP sightings age out (`HOSTILE_SIGHTING_MAX_AGE`) —
            //   creeps live ≤ 1500 ticks, an old sighting says nothing today;
            // - hostile PLAYER reservations age out (`RESERVATION_MAX_AGE`) —
            //   reservations decay unless renewed;
            // - NPC "Invader" reservations are never a movement hazard —
            //   passable-dispreferred below;
            // - SK rooms, armed towers, and live hostile owners stay hostile.
            let intel = super::routepricing::RouteRoomIntel::from_dynamic(dynamic_visibility_data);
            let derelict_pathing_on = self.derelict_features.on;

            if super::routepricing::is_hostile_for_movement(&intel, derelict_pathing_on) {
                match room_options.hostile_behavior() {
                    HostileBehavior::Allow => {}
                    HostileBehavior::HighCost => return Some(10.0),
                    HostileBehavior::Deny => return None,
                }
            }

            // The ONE shared tier chain (friendly 1.0 / dispreferred 2.5 /
            // neutral 2.0) — see `passable_room_cost`'s docs.
            return Some(super::routepricing::passable_room_cost(&intel, derelict_pathing_on));
        }

        Some(2.0)
    }

    fn get_entity_position(&self, entity: Entity) -> Option<Position> {
        let creep_owner = self.creep_owner.get(entity)?;
        let creep = creep_owner.id().resolve()?;
        Some(HasPosition::pos(&creep))
    }
}

pub struct MovementUpdateSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MovementUpdateSystem {
    type SystemData = MovementUpdateSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // G-13 reconciliation — FIRST, before any of this tick's movement work: drain last
        // tick's issued-intent log and count intents the engine did not execute (position
        // unchanged ∧ fatigue == 0; dead creeps skipped). Positions/fatigue are start-of-tick
        // constants in the engine model, so "before new requests" is about the buffer swap, not
        // observation timing. The count is order-independent (a pure filter-count), so resolve
        // order can never affect the reported value.
        let wasted_moves = G13_ISSUED_LAST_TICK.with(|cell| {
            let issued = cell.take();
            count_wasted_moves(issued.iter().map(|rec| {
                let observed = rec.id.resolve().map(|creep| (HasPosition::pos(&creep), creep.fatigue()));
                (rec.from, observed)
            }))
        });
        data.metrics.record_wasted_moves(wasted_moves);

        let mut movement_data = std::mem::replace(&mut *data.movement, MovementData::new());

        // IDLE DISPOSITION (ADR 0033 §M4 F2 / M5 live adoption, operator-ratified 2026-07-01
        // decision (3)): every living owned creep with NO movement request this tick becomes a
        // resolver-known stationary occupant, split by job class (`JobData::is_military`):
        //
        //  - MILITARY → an `Immovable` HOLD request (move_to own tile, range 0): NEVER displaced
        //    (try_shove's enum check, rover resolver.rs) — a request-less fighter is holding
        //    formation, and shoving it out was the combat-sim finding that forced combat-agent's
        //    `register_idle_creeps: false` opt-out. `allow_shove(true)` does NOT consent to
        //    displacement here (the `Immovable` enum vetoes first); it only keeps the arrived
        //    hold in the resolver's occupancy view — Pass 1 drops no-shove/no-swap arrived
        //    requests to `Arrived` without a `ResolvedCreep` entry, which would leave the holder
        //    invisible and movers pathing into it optimistically (the `failed_into_parked`
        //    class). `allow_swap(false)` is inert defence-in-depth (no desired tile ⇒ never a
        //    swap candidate).
        //  - CIVILIAN → a shoveable Low idle via `set_idle_creep_positions` (below): movers route
        //    around it deliberately, displace it outright when they carry real priority
        //    (synthesized lowest-anchor entries), and idle-denial dances still climb the stuck
        //    ladder (denial-as-stuck) — the two mechanisms that made registration safe in the
        //    rover-eval corpus.
        //  - UNKNOWN/no job → MILITARY: mis-classifying a fighter as shoveable breaks formations;
        //    a parked civilian held `Immovable` merely costs passers-by a detour.
        //
        // CPU shape: ONE pass over owned creeps; `resolve()` + `pos()` paid only for creeps
        // WITHOUT a request (requesters skip at `contains_request`). Holds add to
        // `request_count()` and thus slightly overstate the 0.2-CPU/move reserve below (they
        // never issue a move) — conservative direction, small military-idle counts. The idle map
        // is the only per-tick allocation. Creeps with `CreepOwner` are post-spawn by
        // construction (`WaitForSpawnSystem` inserts it only once `spawning()` is false).
        //
        // Determinism: the specs join iterates ascending entity index; the idle map keeps the
        // first-seen (lowest) entity on a (degenerate, live-impossible) stacked tile via
        // `or_insert` — a pure function of the world, never of HashMap iteration order (the
        // sim-core `rover_driver` registration pattern, kept in LIVE parity).
        let mut idle_creep_positions: std::collections::HashMap<Position, Entity> = std::collections::HashMap::new();
        for (entity, creep_owner) in (&data.entities, &data.creep_owner).join() {
            if movement_data.contains_request(&entity) {
                continue;
            }
            let creep = match creep_owner.id().resolve() {
                Some(creep) => creep,
                None => continue, // dead this tick; CleanupCreepsSystem reaps the entity
            };
            let creep_pos = HasPosition::pos(&creep);

            let military = data.job_data.get(entity).map(|job| job.is_military()).unwrap_or(true);
            if military {
                movement_data
                    .move_to(entity, creep_pos)
                    .range(0)
                    .priority(MovementPriority::Immovable)
                    .allow_shove(true)
                    .allow_swap(false);
            } else {
                idle_creep_positions.entry(creep_pos).or_insert(entity);
            }
        }

        // The G-13 issue-time log for THIS tick (shared by every creep handle rover receives;
        // becomes next tick's reconciliation set after `process()`).
        let issued_moves: Rc<RefCell<Vec<IssuedMoveIntent>>> = Rc::new(RefCell::new(Vec::new()));

        let mut external = MovementSystemExternalProvider {
            entities: &data.entities,
            creep_owner: &data.creep_owner,
            creep_movement_data: &mut data.creep_movement_data,
            room_data: &data.room_data,
            mapping: &data.mapping,
            room_status_cache: &data.room_status_cache,
            derelict_features: data.features.derelict,
            issued_moves: issued_moves.clone(),
        };

        let mut pathfinder = ScreepsPathfinder;
        let mut ibex_visualizer = data.visualizer.as_deref_mut().map(|v| IbexMovementVisualizer { visualizer: v });

        let mut cost_matrix_system = CostMatrixSystem::new(&mut data.cost_matrix_cache, Box::new(ScreepsCostMatrixDataSource));

        let mut system = MovementSystem::new(
            &mut cost_matrix_system,
            &mut pathfinder,
            ibex_visualizer.as_mut().map(|v| v as &mut dyn MovementVisualizer),
        );

        let pathing_features = data.features.pathing;
        system.set_reuse_path_length(pathing_features.reuse_path_length);
        system.set_max_shove_depth(pathing_features.max_shove_depth);
        system.set_friendly_creep_distance(pathing_features.friendly_creep_distance);

        // Civilian idlers collected above: consumed by exactly one process() (rover takes the
        // map per tick, so a stale registration can never leak).
        system.set_idle_creep_positions(idle_creep_positions);

        // Spawn keep-clear (ADR-follow-up, issue #2 backstop): a creep BEING spawned is not a mover
        // and cannot shove for itself, so a spawn whose range-1 ring is fully ringed by idle creeps
        // cannot place its new creep and stalls. Register every actively-spawning spawn as an
        // eviction point — the rover steps any idle occupant on its ring one tile out (a synthesized
        // low-priority flee). Cheap (a handful of spawns; only idles on the ring move) and empty when
        // nothing is spawning. A creep with a real job move, or an `Immovable` fighter, is untouched.
        let eviction_points: Vec<Position> = screeps::game::spawns()
            .values()
            .filter(|spawn| spawn.spawning().is_some())
            .map(|spawn| spawn.pos())
            .collect();
        system.set_eviction_points(eviction_points);

        let tick_limit = screeps::game::cpu::tick_limit();
        let get_cpu = screeps::game::cpu::get_used;
        let cpu_limit = screeps::game::cpu::limit() as f64;
        // Governor snapshot is the one CPU-pressure truth (M1): no raw
        // bucket reads bypassing it.
        let bucket = data.governor.bucket;
        // Under normal conditions use GCL limit; when bucket is at/above threshold allow burst up to tick_limit.
        let budget_ceiling = if pathing_features.bucket_burst_threshold == 0 || bucket >= pathing_features.bucket_burst_threshold {
            tick_limit
        } else {
            cpu_limit
        };

        let max_budget = budget_ceiling * pathing_features.movement_cpu_budget_pct;
        let remaining = (tick_limit - get_cpu()).max(0.0);
        let cpu_budget = remaining.min(max_budget);
        system.set_cpu_budget(get_cpu, cpu_budget);

        let repath_budget = pathing_features.repath_cpu_budget;
        system.set_repath_budget(get_cpu, repath_budget);

        // Pathfinding ops: never use more than remaining CPU (1 op ≈ 0.001 CPU). Reserve
        // a fraction of the budget ceiling for cost matrices, resolver, and rest of tick.
        const MOVEMENT_RESERVE_FRACTION: f64 = 0.2;
        const MOVEMENT_RESERVE_FLOOR: f64 = 2.0;
        let reserve = (budget_ceiling * MOVEMENT_RESERVE_FRACTION).max(MOVEMENT_RESERVE_FLOOR);
        // Each move/pull action has a 0.2 CPU artificial cost; reserve so we don't exhaust the tick.
        const MOVE_ACTION_CPU: f64 = 0.2;
        let move_action_reserve = movement_data.request_count() as f64 * MOVE_ACTION_CPU;
        let pathfinding_cpu_available = (remaining - reserve - move_action_reserve).max(0.0);
        let pathfinding_cpu_cap = pathing_features.pathfinding_cpu_budget.min(pathfinding_cpu_available);
        let mut pathfinding_ops = (pathfinding_cpu_cap * 1000.0) as u32;

        // P1.B4 governor coordination: movement is never-shed but its
        // pathfinding generosity scales with the tier (the MIN floor
        // below still applies — creeps never fully freeze, ADR 0004's
        // non-negotiable). Movement does NOT draw from the mission
        // pool; this is its independent budget, tier-scaled.
        pathfinding_ops = match data.governor.tier {
            crate::cpugovernor::Tier::Normal => pathfinding_ops,
            crate::cpugovernor::Tier::Conserve => pathfinding_ops / 2,
            crate::cpugovernor::Tier::Critical => pathfinding_ops / 4,
        };
        // Ensure at least one pathfinding can run to avoid deadlock (no progress across ticks).
        const MIN_PATHFIND_OPS: u32 = 2000;
        if movement_data.request_count() > 0 && pathfinding_ops == 0 && remaining > (MIN_PATHFIND_OPS as f64 / 1000.0) + MOVE_ACTION_CPU {
            pathfinding_ops = MIN_PATHFIND_OPS;
        }
        // Absolute ceiling so we never grant more than ~50 CPU worth of pathfinding ops per tick.
        const PATHFIND_OPS_CEILING: u32 = 50_000;
        pathfinding_ops = pathfinding_ops.min(PATHFIND_OPS_CEILING);
        system.set_pathfinding_ops_budget(pathfinding_ops);

        system.set_tick_limit(get_cpu, tick_limit);

        // Hard cap on movement CPU per tick; stay within budget_ceiling so we don't consume bucket unnecessarily.
        // In normal (non-burst) mode, apply an absolute ceiling so we don't give movement more than 80 CPU.
        // In burst mode use a higher cap so one pathfinding can run (headroom then limits blow-through).
        let movement_start_cpu = get_cpu();
        const MIN_MOVEMENT_CPU: f64 = 5.0;
        const NORMAL_MODE_MOVEMENT_CEILING: f64 = 80.0;
        /// In burst mode allow one pathfinding; headroom 80 means we only start when used <= cap - 80.
        const BURST_MODE_MOVEMENT_CAP: f64 = 150.0;
        let normal_mode = (budget_ceiling - cpu_limit).abs() < 0.01;
        let movement_cap_max = if normal_mode {
            pathing_features.movement_max_cpu.min(NORMAL_MODE_MOVEMENT_CEILING)
        } else {
            BURST_MODE_MOVEMENT_CAP
        };
        let ceiling_remaining = (budget_ceiling - get_cpu()).max(0.0);
        let movement_cap = (remaining - reserve)
            .max(0.0)
            .min(ceiling_remaining)
            .min(movement_cap_max)
            .max(MIN_MOVEMENT_CPU);
        system.set_movement_cpu_cap(get_cpu, movement_start_cpu, movement_cap);
        // Pathfinding headroom: do not start find_route unless (used + headroom) <= cap (find_route is unbounded).
        // Normal mode: headroom = cap so we never start pathfinding (saves CPU).
        // Burst mode: headroom 80 so we only start when we have 80 CPU headroom, allowing one pathfind and capping blow-through.
        let pathfinding_headroom = if normal_mode { Some(movement_cap) } else { Some(80.0) };
        system.set_pathfinding_headroom(pathfinding_headroom);

        let request_count = movement_data.request_count();
        let results = system.process(&mut external, movement_data);

        // P1.B2: per-tick pathfinding telemetry into the seg-57 block.
        data.metrics.record_movement_stats(system.tick_stats());

        // P1.D6 / IBEX-015: surface the give-up results the jobs used
        // to silently ignore (recovery wiring = Inc 6, ADR 0003 A6).
        //
        // KEPT DELIBERATELY alongside the G-13 counter above: this is the rover SELF-REPORTED
        // give-up LEVEL (a persisting `Stuck ≥ 10` episode is re-counted every tick), the alarm
        // stream live telemetry has history against; `wasted_moves` is the TRUE per-event
        // issued-vs-moved rate. Both ride the seg-57 `pathing` block so the soak can compare.
        let move_failures = results
            .results
            .values()
            .filter(|result| match result {
                MovementResult::Failed(_) => true,
                MovementResult::Stuck { ticks } => *ticks >= crate::jobs::utility::movebehavior::STUCK_REPORT_THRESHOLD,
                _ => false,
            })
            .count() as u32;
        data.metrics.record_movement_failures(move_failures);

        // Stash this tick's issued intents for next tick's G-13 reconciliation (top of `run`).
        G13_ISSUED_LAST_TICK.with(|cell| *cell.borrow_mut() = issued_moves.take());

        let movement_cpu_used = get_cpu() - movement_start_cpu;
        if movement_cpu_used > 80.0 {
            log::info!("movement: {:.1} CPU, {} requests", movement_cpu_used, request_count);
        }

        *data.movement_results = results;
    }
}

#[cfg(test)]
mod g13_tests {
    use super::*;

    fn pos(x: u8, y: u8) -> Position {
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            "W5N5".parse::<RoomName>().unwrap(),
        )
    }

    /// Issue → moved (any position change, adjacent or border-relocated): the intent executed,
    /// nothing wasted — fatigue value is irrelevant on this branch (moving GENERATES fatigue).
    #[test]
    fn issued_then_moved_is_not_wasted() {
        assert!(!issued_move_wasted(pos(10, 10), Some((pos(11, 10), 0))));
        assert!(!issued_move_wasted(pos(10, 10), Some((pos(11, 11), 4))));
    }

    /// Issue → blocked while unfatigued: THE wasted class — intent spent, engine executed
    /// nothing, and fatigue can't explain it (the offline `failed_wall`/`failed_into_parked`/
    /// `failed_coordination` family's live aggregate).
    #[test]
    fn issued_then_blocked_unfatigued_is_wasted() {
        assert!(issued_move_wasted(pos(10, 10), Some((pos(10, 10), 0))));
    }

    /// Issue → immobile but fatigued: the offline `failed_fatigued` class, deliberately EXCLUDED
    /// from the live counter (bench.rs spec gap (1): fatigue is self-explaining live).
    #[test]
    fn issued_then_fatigued_is_not_wasted() {
        assert!(!issued_move_wasted(pos(10, 10), Some((pos(10, 10), 6))));
    }

    /// Issue → died before reconciliation (unresolvable id): never counted.
    #[test]
    fn issued_then_dead_is_not_counted() {
        assert!(!issued_move_wasted(pos(10, 10), None));
    }

    /// No issue ⇒ not counted is STRUCTURAL (only recorded intents enter the fold), and the fold
    /// itself sums exactly the wasted subset of a mixed reconciliation set.
    #[test]
    fn count_folds_only_wasted_records() {
        assert_eq!(count_wasted_moves(std::iter::empty()), 0);
        let records = vec![
            (pos(10, 10), Some((pos(11, 10), 0))), // moved
            (pos(20, 20), Some((pos(20, 20), 0))), // wasted
            (pos(30, 30), Some((pos(30, 30), 8))), // fatigued
            (pos(40, 40), None),                   // died
            (pos(5, 5), Some((pos(5, 5), 0))),     // wasted
        ];
        assert_eq!(count_wasted_moves(records), 2);
    }
}
