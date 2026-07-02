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

#[derive(Shrinkwrap, Component, Serialize, Deserialize, Clone, Default)]
#[shrinkwrap(mutable)]
#[serde(transparent)]
pub struct CreepRoverData(pub CreepMovementData);

#[derive(SystemData)]
pub struct MovementUpdateSystemData<'a> {
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

struct MovementSystemExternalProvider<'a, 'b> {
    entities: &'b Entities<'a>,
    creep_owner: &'b ReadStorage<'a, CreepOwner>,
    creep_movement_data: &'b mut WriteStorage<'a, CreepRoverData>,
    room_data: &'b ReadStorage<'a, RoomData>,
    mapping: &'b Read<'a, EntityMappingData>,
    room_status_cache: &'b RoomStatusCache,
    derelict_features: crate::features::DerelictFeatures,
}

impl<'a, 'b> MovementSystemExternal<Entity> for MovementSystemExternalProvider<'a, 'b> {
    type Creep = screeps::Creep;

    fn get_creep(&self, entity: Entity) -> Result<screeps::Creep, MovementError> {
        let creep_owner = self.creep_owner.get(entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        Ok(creep)
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
            // A derelict room — hostile-owned but militarily dead at the last
            // sighting (no spawns, armed towers, or combat/threat creeps) — is
            // simply NOT hostile for pathing: it has no defenders, so the owner
            // name alone can't hurt a creep. Uses the raw `derelict()`
            // classification (last sighting), NOT the stricter
            // `confirmed_derelict`: pathing into a defenseless room is safe on
            // older intel too, and gating it on fresh intel deadlocked outpost
            // economy creeps — the default `HostileBehavior::Deny` blocked them
            // the moment the confirmation lapsed, and they could never enter to
            // refresh the very intel the gate required. Anything armed at the
            // last sighting — towers with energy, combat creeps, a hostile
            // reservation, source keepers — still reads hostile. (Action
            // decisions — salvage / de-claim / outpost admission — keep the
            // stricter `confirmed_derelict`; only pathing is loosened here.)
            let derelict_features = &self.derelict_features;
            let derelict = derelict_features.on && dynamic_visibility_data.derelict();

            let is_hostile = dynamic_visibility_data.source_keeper()
                || dynamic_visibility_data.reservation().hostile()
                || dynamic_visibility_data.hostile_creeps()
                || dynamic_visibility_data.hostile_towers()
                || (dynamic_visibility_data.owner().hostile() && !derelict);

            if is_hostile {
                match room_options.hostile_behavior() {
                    HostileBehavior::Allow => {}
                    HostileBehavior::HighCost => return Some(10.0),
                    HostileBehavior::Deny => return None,
                }
            }

            if dynamic_visibility_data.owner().mine()
                || dynamic_visibility_data.owner().friendly()
                || dynamic_visibility_data.reservation().mine()
                || dynamic_visibility_data.reservation().friendly()
            {
                return Some(1.0);
            } else if derelict {
                // Passable, but prefer truly neutral routes on ties.
                return Some(2.5);
            } else {
                return Some(2.0);
            }
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

        let mut external = MovementSystemExternalProvider {
            entities: &data.entities,
            creep_owner: &data.creep_owner,
            creep_movement_data: &mut data.creep_movement_data,
            room_data: &data.room_data,
            mapping: &data.mapping,
            room_status_cache: &data.room_status_cache,
            derelict_features: data.features.derelict,
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

        let movement_cpu_used = get_cpu() - movement_start_cpu;
        if movement_cpu_used > 80.0 {
            log::info!("movement: {:.1} CPU, {} requests", movement_cpu_used, request_count);
        }

        *data.movement_results = results;
    }
}
