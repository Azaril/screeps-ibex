use crate::cleanup::*;
use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::globals::*;
use crate::jobs::data::*;
use crate::jobs::jobsystem::*;
use crate::memorysystem::*;
use crate::military::boostqueue::*;
use crate::military::economy::*;
use crate::military::squad::*;
use crate::military::threatmap::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::operations::data::*;
use crate::operations::managersystem::*;
use crate::operations::operationsystem::*;
use crate::pathing::costmatrixsystem::*;
use crate::pathing::movementsystem::*;
use crate::repairqueue::RepairQueueClearSystem;
use crate::room::createroomsystem::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::room::roomplanvisualizesystem::*;
use crate::room::updateroomsystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::stats_history::StatsHistorySystem;
use crate::statssystem::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use crate::visualization::{
    AggregateSummarySystem, ClearVisualizationSystem, CpuHistory, CpuTrackingSystem, RenderSystem, SummarizeJobSystem,
    SummarizeMissionSystem, SummarizeOperationSystem, SummarizeRoomVisibilitySystem, VisualizationData,
};
use crate::visualize::*;
use bincode::{DefaultOptions, Deserializer, Serializer};
use log::*;
use screeps::*;
use screeps_rover::*;
use specs::{
    prelude::*,
    saveload::{DeserializeComponents, SerializeComponents},
};
use std::cell::RefCell;
use std::collections::HashSet;

/// Apply an operation to every system in the tick system list.
///
/// The system list is defined once here; both `setup_systems` and
/// `run_systems` expand from this single definition so they cannot
/// drift out of sync.
///
/// `$op` is a macro name that will be invoked as `$op!(SystemInstance, "label")`
/// where `SystemInstance` is a value expression (unit struct literal).
macro_rules! for_each_system {
    ($op:ident) => {
        // === Pre-pass ===
        $op!(WaitForSpawnSystem, "wait_for_spawn");
        $op!(CleanupCreepsSystem, "cleanup_creeps");
        // Flush creep deaths immediately so missions see accurate counts.
        // The system is a no-op when the queue is empty, so the second
        // invocation after RunJobSystem costs nothing when there are no
        // mid-tick mission/operation deletions.
        $op!(EntityCleanupSystem, "entity_cleanup_prepass");
        $op!(CreateRoomDataSystem, "create_room_data");
        $op!(UpdateRoomDataSystem, "update_room_data");
        $op!(EntityMappingSystem, "entity_mapping");
        $op!(ThreatAssessmentSystem, "threat_assessment");
        $op!(EconomyAssessmentSystem, "economy_assessment");
        // === Main-pass: Cleanup ===
        $op!(RepairQueueClearSystem, "repair_queue_clear");
        $op!(ClearVisualizationSystem, "clear_visualization");
        $op!(VisibilityQueueCleanupSystem, "visibility_cleanup");
        $op!(CostMatrixClearSystem, "cost_matrix_clear");
        // === Main-pass: Pre-run ===
        $op!(OperationManagerSystem, "operations_manager");
        $op!(PreRunOperationSystem, "pre_run_operations");
        $op!(PreRunMissionSystem, "pre_run_missions");
        $op!(PreRunSquadUpdateSystem, "pre_run_squad_update");
        $op!(PreRunJobSystem, "pre_run_jobs");
        // === Main-pass: Execution ===
        $op!(RunOperationSystem, "run_operations");
        $op!(RunMissionSystem, "run_missions");
        $op!(RunSquadUpdateSystem, "run_squad_update");
        $op!(RunJobSystem, "run_jobs");
        // === Entity cleanup: process all pending deletions ===
        $op!(EntityCleanupSystem, "entity_cleanup");
        $op!(MovementUpdateSystem, "movement");
        // === Main-pass: Observer ===
        $op!(ObserverSystem, "observer");
        // === Main-pass: Summarization ===
        $op!(SummarizeOperationSystem, "summarize_operations");
        $op!(SummarizeMissionSystem, "summarize_missions");
        $op!(SummarizeJobSystem, "summarize_jobs");
        $op!(SummarizeRoomVisibilitySystem, "summarize_room_visibility");
        $op!(VisibilityVisualizationSystem, "visibility_viz");
        $op!(TransferStatsSnapshotSystem, "transfer_stats_snapshot");
        $op!(AggregateSummarySystem, "aggregate_summary");
        // === Main-pass: Queues ===
        $op!(SpawnQueueSystem, "spawn_queue");
        $op!(TransferQueueUpdateSystem, "transfer_queue");
        $op!(OrderQueueSystem, "order_queue");
        // === Main-pass: Room Planning ===
        $op!(RoomPlanSystem, "room_plan");
        $op!(RoomPlanVisualizeSystem, "room_plan_visualize");
        // === Main-pass: Stats and Visualization ===
        $op!(StatsSystem, "stats");
        $op!(StatsHistorySystem, "stats_history");
        $op!(CpuTrackingSystem, "cpu_tracking");
        $op!(RenderSystem, "render");
        $op!(ApplyVisualsSystem, "apply_visuals");
        // === Main-pass: Persistence ===
        $op!(VisibilityQueueSyncSystem, "visibility_sync");
        $op!(CostMatrixStoreSystem, "cost_matrix_store");
        $op!(MemoryArbiterSystem, "memory");
    };
}

/// Call `RunNow::setup` for every system in the tick list.
fn setup_systems(world: &mut World) {
    macro_rules! do_setup {
        ($sys:expr, $label:expr) => {
            RunNow::setup(&mut $sys, world);
        };
    }
    for_each_system!(do_setup);
}

/// Run every system in the tick list sequentially.
/// Each system call is followed by `world.maintain()`.
/// When `timing` is true, per-system CPU cost is measured and logged.
fn run_systems(world: &mut World, timing: bool) {
    macro_rules! do_run {
        ($sys:expr, $label:expr) => {
            if timing {
                let before = game::cpu::get_used();
                $sys.run_now(world);
                world.maintain();
                let after = game::cpu::get_used();
                info!("[timing] {}: {:.2} cpu", $label, after - before);
            } else {
                $sys.run_now(world);
                world.maintain();
            }
        };
    }
    for_each_system!(do_run);
}

/// Pre-serialization integrity check and repair.
///
/// Scans every serializable component that contains `Entity` references and
/// verifies that the referenced entities are alive and carry a
/// `SerializeMarker`. Dangling references are logged and repaired:
///
/// - `RoomData.missions`: dead entries are removed from the list.
/// - `MissionData.owner`: dead owner is cleared; the mission is deleted
///   (orphaned missions would never be cleaned up otherwise).
/// - `MissionData.room`: dead room reference causes the mission to be deleted.
/// - `MissionData` children: dead children are cleared via `child_complete`.
///
/// This acts as a safety net so that `ConvertSaveload` never panics on a
/// dangling entity during serialization, regardless of the specific cleanup
/// ordering that caused the inconsistency.
fn repair_entity_integrity(world: &mut World) {
    // Collect missions that need deletion and missions with dead children.
    let mut missions_to_delete: Vec<Entity> = Vec::new();
    let mut dead_children: Vec<(Entity, Vec<Entity>)> = Vec::new();

    {
        let entities = world.entities();
        let markers = world.read_storage::<SerializeMarker>();
        let missions = world.read_storage::<MissionData>();
        let mut room_data_storage = world.write_storage::<RoomData>();

        let is_valid = |e: Entity| -> bool { entities.is_alive(e) && markers.get(e).is_some() };

        // ── RoomData.missions: remove dead entries ──────────────────────
        for (entity, rd) in (&entities, &mut room_data_storage).join() {
            let before = rd.get_missions().len();
            rd.retain_missions(|e| {
                let ok = is_valid(e);
                if !ok {
                    error!("INTEGRITY: dead mission entity {:?} removed from RoomData {:?}", e, entity);
                }
                ok
            });
            let after = rd.get_missions().len();
            if after < before {
                warn!("INTEGRITY: removed {} dead mission(s) from RoomData {:?}", before - after, entity);
            }
        }

        // ── MissionData: scan for dangling references ───────────────────
        for (entity, md) in (&entities, &missions).join() {
            let mission = md.as_mission();

            // Owner
            if let Some(owner) = *mission.get_owner() {
                if !is_valid(owner) {
                    error!("INTEGRITY: dead owner {:?} on mission {:?}, scheduling deletion", owner, entity);
                    missions_to_delete.push(entity);
                    continue; // skip further checks; entity will be deleted
                }
            }

            // Room
            let room = mission.get_room();
            if !is_valid(room) {
                error!("INTEGRITY: dead room {:?} on mission {:?}, scheduling deletion", room, entity);
                missions_to_delete.push(entity);
                continue;
            }

            // Children
            let bad_children: Vec<Entity> = mission
                .get_children()
                .into_iter()
                .filter(|child| {
                    let ok = is_valid(*child);
                    if !ok {
                        error!("INTEGRITY: dead child {:?} on mission {:?}", child, entity);
                    }
                    !ok
                })
                .collect();

            if !bad_children.is_empty() {
                dead_children.push((entity, bad_children));
            }
        }
    }

    // ── OperationData: clean dangling entity references ─────────────────
    {
        let entities = world.entities();
        let markers = world.read_storage::<SerializeMarker>();
        let mut operations = world.write_storage::<OperationData>();

        let is_valid = |e: Entity| -> bool { entities.is_alive(e) && markers.get(e).is_some() };

        for (_entity, od) in (&entities, &mut operations).join() {
            od.as_operation().repair_entity_refs(&is_valid);
        }
    }

    // ── MissionData: clean dangling internal entity references ────────
    {
        let entities = world.entities();
        let markers = world.read_storage::<SerializeMarker>();
        let missions = world.read_storage::<MissionData>();

        let is_valid = |e: Entity| -> bool { entities.is_alive(e) && markers.get(e).is_some() };

        for (_entity, md) in (&entities, &missions).join() {
            md.as_mission_mut().repair_entity_refs(&is_valid);
        }
    }

    // ── SquadContext: clean dangling member and heal_priority refs ────
    {
        let entities = world.entities();
        let markers = world.read_storage::<SerializeMarker>();
        let mut squads = world.write_storage::<SquadContext>();

        let is_valid = |e: Entity| -> bool { entities.is_alive(e) && markers.get(e).is_some() };

        for (entity, sc) in (&entities, &mut squads).join() {
            let before = sc.members.len();
            sc.members.retain(|m| {
                let ok = is_valid(m.entity);
                if !ok {
                    error!("INTEGRITY: dead member entity {:?} removed from SquadContext {:?}", m.entity, entity);
                }
                ok
            });
            let after = sc.members.len();
            if after < before {
                warn!("INTEGRITY: removed {} dead member(s) from SquadContext {:?}", before - after, entity);
            }

            if let Some(heal_entity) = *sc.heal_priority {
                if !is_valid(heal_entity) {
                    error!("INTEGRITY: dead heal_priority entity {:?} cleared from SquadContext {:?}", heal_entity, entity);
                    *sc.heal_priority = None;
                }
            }
        }
    }

    // ── Repair dead children (clear via child_complete) ─────────────────
    if !dead_children.is_empty() {
        let missions = world.read_storage::<MissionData>();
        for (entity, children) in &dead_children {
            if let Some(md) = missions.get(*entity) {
                let mut mission = md.as_mission_mut();
                for child in children {
                    warn!("INTEGRITY: clearing dead child {:?} from mission {:?}", child, entity);
                    mission.child_complete(*child);
                }
            }
        }
    }

    // ── Delete orphaned missions ────────────────────────────────────────
    //
    // These missions had dead owners or dead room references and cannot
    // function. Clear the owner, remove from rooms, notify the owner (if
    // still alive), and delete the entity.
    for mission_entity in &missions_to_delete {
        // Read the owner before we clear it.
        let owner = world
            .read_storage::<MissionData>()
            .get(*mission_entity)
            .map(|md| *md.as_mission().get_owner())
            .unwrap_or(None);

        // Clear the dead owner reference so serialization of any
        // intermediate state cannot panic.
        if let Some(dead_owner) = owner {
            if let Some(md) = world.read_storage::<MissionData>().get(*mission_entity) {
                md.as_mission_mut().owner_complete(dead_owner);
            }
        }

        // Remove from rooms.
        {
            let mut room_data_storage = world.write_storage::<RoomData>();
            for rd in (&mut room_data_storage).join() {
                rd.remove_mission(*mission_entity);
            }
        }

        // Notify the (alive) owner that this child is gone.
        if let Some(owner) = owner {
            if world.entities().is_alive(owner) {
                if let Some(od) = world.write_storage::<OperationData>().get_mut(owner) {
                    od.as_operation().child_complete(*mission_entity);
                }
                if let Some(md) = world.write_storage::<MissionData>().get_mut(owner) {
                    md.as_mission_mut().child_complete(*mission_entity);
                }
            }
        }

        if let Err(err) = world.delete_entity(*mission_entity) {
            warn!("INTEGRITY: failed to delete orphaned mission {:?}: {}", mission_entity, err);
        } else {
            warn!("INTEGRITY: deleted orphaned mission {:?}", mission_entity);
        }
    }

    if !missions_to_delete.is_empty() {
        world.maintain();
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn serialize_world(world: &World, segments: &[u32]) {
    struct Serialize<'a> {
        segments: &'a [u32],
    }

    #[derive(SystemData)]
    struct SerializeSystemData<'a> {
        memory_arbiter: WriteExpect<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_allocator: Write<'a, SerializeMarkerAllocator>,
        markers: ReadStorage<'a, SerializeMarker>,
        creep_spawnings: ReadStorage<'a, CreepSpawning>,
        creep_owners: ReadStorage<'a, CreepOwner>,
        creep_movement_data: ReadStorage<'a, CreepRoverData>,
        room_data: ReadStorage<'a, RoomData>,
        room_plan_data: ReadStorage<'a, RoomPlanData>,
        job_data: ReadStorage<'a, JobData>,
        operation_data: ReadStorage<'a, OperationData>,
        mission_data: ReadStorage<'a, MissionData>,
        squad_context: ReadStorage<'a, SquadContext>,
        visibility_queue_data: ReadStorage<'a, VisibilityQueueData>,
        room_threat_data: ReadStorage<'a, RoomThreatData>,
    }

    impl<'a, 'b> System<'a> for Serialize<'b> {
        type SystemData = SerializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            let mut serialized_data = Vec::<u8>::with_capacity(1024 * 50);

            let mut serializer = Serializer::new(&mut serialized_data, DefaultOptions::new());

            SerializeComponents::<std::convert::Infallible, SerializeMarker>::serialize(
                &(
                    &data.creep_spawnings,
                    &data.creep_owners,
                    &data.creep_movement_data,
                    &data.room_data,
                    &data.room_plan_data,
                    &data.job_data,
                    &data.operation_data,
                    &data.mission_data,
                    &data.squad_context,
                    &data.visibility_queue_data,
                    &data.room_threat_data,
                ),
                &data.entities,
                &data.markers,
                &mut serializer,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
            .unwrap_or_else(|e| error!("Failed serialization: {}", e));

            let encoded_data = match encode_buffer_to_string(&serialized_data) {
                Ok(s) => s,
                Err(e) => {
                    error!("Encode failed: {}", e);
                    return;
                }
            };

            let mut segments = self.segments.iter();

            for chunk in encoded_data.as_bytes().chunks(1024 * 50) {
                if let Some(segment) = segments.next() {
                    //
                    // NOTE: This relies on not using multi-byte characters for encoding. (This is valid from base64 encoding.)
                    //
                    let chunk_str = unsafe { std::str::from_utf8_unchecked(chunk) };

                    data.memory_arbiter.set(*segment, chunk_str);
                } else {
                    error!(
                        "Not enough segments available to store all state. Segment count: {} - Needed segments: {}",
                        self.segments.len(),
                        encoded_data.len() as f32 / (1024.0 * 50.0)
                    );
                }
            }

            for segment in segments {
                data.memory_arbiter.set(*segment, "");
            }
        }
    }

    let mut sys = Serialize { segments };

    sys.run_now(world);
}

/// Loads world state from RawMemory segments. On deserialization failure we log and continue;
/// the only supported recovery is a full reset (environment and optionally memory via reset flags).
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn deserialize_world(world: &World, segments: &[u32]) {
    struct Deserialize<'a> {
        segments: &'a [u32],
    }

    #[derive(SystemData)]
    struct DeserializeSystemData<'a> {
        memory_arbiter: WriteExpect<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_alloc: Write<'a, SerializeMarkerAllocator>,
        markers: WriteStorage<'a, SerializeMarker>,
        creep_spawnings: WriteStorage<'a, CreepSpawning>,
        creep_owners: WriteStorage<'a, CreepOwner>,
        creep_movement_data: WriteStorage<'a, CreepRoverData>,
        room_data: WriteStorage<'a, RoomData>,
        room_plan_data: WriteStorage<'a, RoomPlanData>,
        job_data: WriteStorage<'a, JobData>,
        operation_data: WriteStorage<'a, OperationData>,
        mission_data: WriteStorage<'a, MissionData>,
        squad_context: WriteStorage<'a, SquadContext>,
        visibility_queue_data: WriteStorage<'a, VisibilityQueueData>,
        room_threat_data: WriteStorage<'a, RoomThreatData>,
    }

    impl<'a, 'b> System<'a> for Deserialize<'b> {
        type SystemData = DeserializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            //
            // NOTE: System assumes that segment is available and will panic if data is not accesible.
            //

            use itertools::*;

            let encoded_data = self
                .segments
                .iter()
                .filter_map(|segment| data.memory_arbiter.get(*segment))
                .join("");

            if !encoded_data.is_empty() {
                let decoded_data = decode_buffer_from_string(&encoded_data).unwrap_or_else(|_| Vec::new());

                let mut deserializer = Deserializer::from_slice(&decoded_data, DefaultOptions::new());

                DeserializeComponents::<std::convert::Infallible, SerializeMarker>::deserialize(
                    &mut (
                        &mut data.creep_spawnings,
                        &mut data.creep_owners,
                        &mut data.creep_movement_data,
                        &mut data.room_data,
                        &mut data.room_plan_data,
                        &mut data.job_data,
                        &mut data.operation_data,
                        &mut data.mission_data,
                        &mut data.squad_context,
                        &mut data.visibility_queue_data,
                        &mut data.room_threat_data,
                    ),
                    &data.entities,
                    &mut data.markers,
                    &mut data.marker_alloc,
                    &mut deserializer,
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
                .unwrap_or_else(|e| error!("Failed deserialization: {}", e));
            }
        }
    }

    let mut sys = Deserialize { segments };

    sys.run_now(world);
}

struct GameEnvironment {
    world: World,
    loaded: bool,
    tick: Option<u32>,
}

thread_local! {
    static ENVIRONMENT: RefCell<Option<GameEnvironment>> = const { RefCell::new(None) };
}

/// Segment IDs used for ECS component serialization (world state).
const COMPONENT_SEGMENTS: &[u32] = &[50, 51, 52, 53];

fn create_environment() -> GameEnvironment {
    info!("Initializing game environment");

    crate::features::prepare();

    let mut world = World::new();

    let mut arbiter = MemoryArbiter::new();

    // ─── Segment requirements ────────────────────────────────────────────────
    //
    // Each subsystem declares which segments it needs. The arbiter uses these
    // to request segments, gate execution on readiness, and run first-load
    // callbacks — no hardcoded segment IDs in tick().

    // Component segments: core ECS world state. Gate execution — the bot
    // cannot run until these are available for deserialization.
    arbiter.register(SegmentRequirement::new("components", COMPONENT_SEGMENTS.to_vec()).gates_execution(true));

    // Cost matrix cache (screeps-rover): needs segment active so the system
    // can read/write via raw_memory directly. Not gating.
    arbiter.register(SegmentRequirement::new("cost_matrix", vec![COST_MATRIX_SEGMENT]));

    // Stats history (visualization): persisted across VM restarts. Load
    // callback deserializes the data into a world resource on first use.
    arbiter.register(
        SegmentRequirement::new("stats_history", vec![crate::stats_history::STATS_HISTORY_SEGMENT]).on_load(Box::new(
            |world: &mut World| {
                crate::stats_history::load_stats_history(world);
            },
        )),
    );

    world.insert(arbiter);

    world.insert(SerializeMarkerAllocator::new());
    world.register::<SerializeMarker>();

    let cost_matrix_cache = crate::pathing::costmatrixsystem::load_cost_matrix_cache(COST_MATRIX_SEGMENT);

    world.insert(cost_matrix_cache);

    let movement_data = MovementData::<Entity>::new();
    world.insert(movement_data);

    let movement_results = MovementResults::<Entity>::new();
    world.insert(movement_results);

    // Military systems.
    world.register::<RoomThreatData>();
    world.insert(BoostQueue::new());
    world.insert(EconomySnapshot::default());
    world.insert(SpawnQueueSnapshot::default());
    world.insert(RoomRouteCache::new());
    world.register::<SquadContext>();

    // Repair queue (ephemeral -- rebuilt each tick by missions).
    world.insert(crate::repairqueue::RepairQueue::default());

    // Entity cleanup queue (ephemeral -- drained each tick by EntityCleanupSystem).
    world.insert(EntityCleanupQueue::default());

    // Per-room supply structure cache (ephemeral -- lazily populated each tick).
    world.insert(crate::missions::localsupply::structure_data::SupplyStructureCache::new());

    // Register components and resources for every system in the tick list.
    setup_systems(&mut world);

    GameEnvironment {
        world,
        loaded: false,
        tick: None,
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick() {
    //
    // Handle reset flags (before feature cache or environment are touched).
    //

    let reset = crate::features::load_reset();

    //
    // Deserialize world state.
    //

    let current_time = game::time();

    let should_reset = reset.environment
        || ENVIRONMENT.with(|e| {
            e.borrow()
                .as_ref()
                .and_then(|env| env.tick)
                .map(|t| t + 1 != current_time)
                .unwrap_or(false)
        });

    if should_reset {
        info!("Resetting environment");
        ENVIRONMENT.with(|e| *e.borrow_mut() = None);
    }

    // Memory and room-plan resets are deferred until we have the ECS world
    // (inside the ENVIRONMENT closure). We just remember the flags here.
    let needs_memory_reset = reset.memory;
    let needs_room_plan_reset = reset.room_plans;

    crate::features::clear_reset();

    //
    // Load feature flags from Memory into per-tick cache
    // (after resets, so the cache reflects any prepare() defaults).
    //

    crate::features::load();
    let features = crate::features::features();

    ENVIRONMENT.with(|env_cell| {
        let mut env_ref = env_cell.borrow_mut();
        let env = env_ref.get_or_insert_with(create_environment);

        //
        // Memory reset — clear all registered segments.
        //

        if needs_memory_reset {
            info!("Resetting memory");

            let mut arbiter = env.world.write_resource::<MemoryArbiter>();
            for seg in arbiter.all_registered_segments() {
                raw_memory::segments().set(seg as u8, String::new());
            }
            arbiter.reset_load_state();
        }

        //
        // Segment pre-pass: request all registered segments and check readiness gates.
        //

        let is_data_ready = {
            let mut arbiter = env.world.write_resource::<MemoryArbiter>();
            arbiter.request_registered();
            arbiter.gates_ready()
        };

        if !is_data_ready {
            info!("Segment data is not ready, delaying execution");

            MemoryArbiterSystem.run_now(&env.world);

            return;
        }

        //
        // Run segment load callbacks (first time only per environment lifecycle).
        //

        run_pending_segment_loads(&mut env.world);

        //
        // Add dynamic resources.
        //

        if features.visualize.on {
            // Visualizer and VisualizationData are recreated each tick (ephemeral draw state).
            env.world.insert(Visualizer::new());
            env.world.insert(VisualizationData::new());
            env.world.insert(TransferStatsSnapshot::default());
            // CpuHistory accumulates across ticks; only create if absent.
            if env.world.try_fetch::<CpuHistory>().is_none() {
                env.world.insert(CpuHistory::new());
            }
        } else {
            env.world.remove::<Visualizer>();
            env.world.remove::<VisualizationData>();
            env.world.remove::<TransferStatsSnapshot>();
            env.world.remove::<CpuHistory>();
            env.world.remove::<crate::stats_history::StatsHistoryData>();
        }

        if !env.loaded {
            info!("Deserializing world state to environment");

            deserialize_world(&env.world, COMPONENT_SEGMENTS);

            env.loaded = true;
        }

        env.tick = Some(current_time);

        //
        // Room plan reset — remove all plans so every room replans.
        //

        if needs_room_plan_reset {
            info!("Resetting all room plans");

            // Clear all RoomPlanData components so rooms have no plan.
            let mut plan_storage = env.world.write_storage::<RoomPlanData>();
            plan_storage.clear();

            // Clear the planner running state (segment 60) so any in-progress
            // planning is abandoned cleanly.
            let mut arbiter = env.world.write_resource::<MemoryArbiter>();
            if arbiter.is_active(PLANNER_MEMORY_SEGMENT) {
                arbiter.set(PLANNER_MEMORY_SEGMENT, "");
            }
        }

        //
        // Prepare globals
        //

        let username = game::rooms()
            .values()
            .filter_map(|room| {
                if let Some(controller) = room.controller() {
                    if controller.my() {
                        return controller.owner().map(|o| o.username());
                    }
                }
                None
            })
            .next();

        if let Some(username) = username {
            user::set_name(&username);
        }

        //
        // Execution — systems run sequentially with maintain() after each.
        //

        run_systems(&mut env.world, features.system_timing);

        //
        // Cleanup memory.
        //

        if let Err(e) = cleanup_memory() {
            warn!("cleanup_memory: {}", e);
        }

        //
        // Pre-serialization integrity check: detect dangling entity references
        // that would panic inside specs ConvertSaveload.
        //

        repair_entity_integrity(&mut env.world);

        //
        // Serialize world state.
        //

        serialize_world(&env.world, COMPONENT_SEGMENTS);
    });
}

fn cleanup_memory() -> Result<(), Box<dyn ::std::error::Error>> {
    let alive_creeps: HashSet<String> = screeps::game::creeps().keys().map(|k| k.to_string()).collect();

    let screeps_memory = match crate::memory_helper::dict("creeps") {
        Some(v) => v,
        None => {
            return Ok(());
        }
    };

    for mem_name in crate::memory_helper::keys(&screeps_memory) {
        if !alive_creeps.contains(&mem_name) {
            debug!("cleaning up creep memory of dead creep {}", mem_name);
            crate::memory_helper::del(&screeps_memory, &mem_name);
        }
    }

    Ok(())
}
