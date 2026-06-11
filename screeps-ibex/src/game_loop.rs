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
use crate::room::room_status_cache::{RoomStatusCache, RoomStatusCacheClearSystem};
use crate::room::roomplansystem::*;
use crate::room::roomplanvisualizesystem::*;
use crate::room::updateroomsystem::*;
use crate::room::visibilitysystem::*;
use crate::metrics::MetricsSystem;
use crate::segments::*;
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
        // P1.C5 (ADR 0005): the scheduler seam. Every system carries a
        // shed class; declaration order IS execution order (parity with
        // the pre-seam flat list by construction). `Always` = the
        // never-shed set and its inputs; `SkipUnderCritical` = work
        // whose absence for a few ticks is harmless by design
        // (visual/observational output, resumable planning). Adding a
        // system without thinking about its class is impossible now —
        // that's the seam's point.
        // === Pre-pass (inputs for everything incl. defense) ===
        $op!(WaitForSpawnSystem, "wait_for_spawn", StageClass::Always);
        $op!(CleanupCreepsSystem, "cleanup_creeps", StageClass::Always);
        // Flush creep deaths immediately so missions see accurate counts.
        // The system is a no-op when the queue is empty, so the second
        // invocation after RunJobSystem costs nothing when there are no
        // mid-tick mission/operation deletions.
        $op!(EntityCleanupSystem, "entity_cleanup_prepass", StageClass::Always);
        $op!(CreateRoomDataSystem, "create_room_data", StageClass::Always);
        $op!(UpdateRoomDataSystem, "update_room_data", StageClass::Always);
        $op!(EntityMappingSystem, "entity_mapping", StageClass::Always);
        $op!(ThreatAssessmentSystem, "threat_assessment", StageClass::Always);
        $op!(EconomyAssessmentSystem, "economy_assessment", StageClass::Always);
        // === Main-pass: Cleanup ===
        $op!(RepairQueueClearSystem, "repair_queue_clear", StageClass::Always);
        $op!(ClearVisualizationSystem, "clear_visualization", StageClass::Always);
        $op!(VisibilityQueueCleanupSystem, "visibility_cleanup", StageClass::Always);
        $op!(CostMatrixClearSystem, "cost_matrix_clear", StageClass::Always);
        $op!(RoomStatusCacheClearSystem, "room_status_cache_clear", StageClass::Always);
        // === Main-pass: Pre-run (defense/spawn/haul live below) ===
        $op!(OperationManagerSystem, "operations_manager", StageClass::Always);
        $op!(PreRunOperationSystem, "pre_run_operations", StageClass::Always);
        $op!(PreRunMissionSystem, "pre_run_missions", StageClass::Always);
        $op!(PreRunSquadUpdateSystem, "pre_run_squad_update", StageClass::Always);
        $op!(PreRunJobSystem, "pre_run_jobs", StageClass::Always);
        // === Main-pass: Execution ===
        $op!(RunOperationSystem, "run_operations", StageClass::Always);
        $op!(RunMissionSystem, "run_missions", StageClass::Always);
        $op!(RunSquadUpdateSystem, "run_squad_update", StageClass::Always);
        $op!(RunJobSystem, "run_jobs", StageClass::Always);
        // === Entity cleanup: process all pending deletions ===
        $op!(EntityCleanupSystem, "entity_cleanup", StageClass::Always);
        $op!(MovementUpdateSystem, "movement", StageClass::Always);
        // === Main-pass: Observer (intel — shed-first class, ADR 0004) ===
        $op!(ObserverSystem, "observer", StageClass::SkipUnderCritical);
        // === Main-pass: Summarization (feeds visualization only) ===
        $op!(SummarizeOperationSystem, "summarize_operations", StageClass::SkipUnderCritical);
        $op!(SummarizeMissionSystem, "summarize_missions", StageClass::SkipUnderCritical);
        $op!(SummarizeJobSystem, "summarize_jobs", StageClass::SkipUnderCritical);
        $op!(SummarizeRoomVisibilitySystem, "summarize_room_visibility", StageClass::SkipUnderCritical);
        $op!(VisibilityVisualizationSystem, "visibility_viz", StageClass::SkipUnderCritical);
        $op!(TransferStatsSnapshotSystem, "transfer_stats_snapshot", StageClass::SkipUnderCritical);
        $op!(AggregateSummarySystem, "aggregate_summary", StageClass::SkipUnderCritical);
        // === Main-pass: Queues (spawn/haul — never shed) ===
        $op!(SpawnQueueSystem, "spawn_queue", StageClass::Always);
        $op!(TransferQueueUpdateSystem, "transfer_queue", StageClass::Always);
        $op!(OrderQueueSystem, "order_queue", StageClass::Always);
        // === Main-pass: Room Planning (resumable by design — seg-60) ===
        $op!(RoomPlanSystem, "room_plan", StageClass::SkipUnderCritical);
        $op!(RoomPlanVisualizeSystem, "room_plan_visualize", StageClass::SkipUnderCritical);
        // === Main-pass: Stats and Visualization (telemetry NEVER sheds
        // — the governor is blind without it; render is visual-only) ===
        $op!(StatsSystem, "stats", StageClass::Always);
        $op!(StatsHistorySystem, "stats_history", StageClass::Always);
        $op!(CpuTrackingSystem, "cpu_tracking", StageClass::Always);
        $op!(MetricsSystem, "metrics", StageClass::Always);
        $op!(RenderSystem, "render", StageClass::SkipUnderCritical);
        $op!(ApplyVisualsSystem, "apply_visuals", StageClass::SkipUnderCritical);
        // === Main-pass: Persistence (never shed) ===
        $op!(VisibilityQueueSyncSystem, "visibility_sync", StageClass::Always);
        $op!(CostMatrixStoreSystem, "cost_matrix_store", StageClass::Always);
        $op!(MemoryArbiterSystem, "memory", StageClass::Always);
    };
}

/// Shed class for the scheduler seam (P1.C5). `Always` = the ADR 0004
/// never-shed set (defense, spawn, haul, movement, persistence) plus
/// their inputs and the telemetry the governor itself depends on.
/// `SkipUnderCritical` = work whose absence is harmless by design:
/// visual/observational output and seg-60-resumable planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageClass {
    Always,
    SkipUnderCritical,
}

impl StageClass {
    fn runs(self, tier: crate::cpugovernor::Tier) -> bool {
        match self {
            StageClass::Always => true,
            StageClass::SkipUnderCritical => tier != crate::cpugovernor::Tier::Critical,
        }
    }
}

/// Call `RunNow::setup` for every system in the tick list (shed class
/// irrelevant at setup — every system's resources must register).
fn setup_systems(world: &mut World) {
    macro_rules! do_setup {
        ($sys:expr, $label:expr, $class:expr) => {
            RunNow::setup(&mut $sys, world);
        };
    }
    for_each_system!(do_setup);
}

/// The scheduler (P1.C5): run the tick list in declaration order, each
/// system followed by `world.maintain()`, skipping systems whose shed
/// class doesn't run at the tick's governor tier. The tier is read
/// ONCE so the whole tick sees a consistent shedding decision.
/// When `timing` is true, per-system CPU cost is measured and logged.
fn run_systems(world: &mut World, timing: bool) {
    let tier = world.read_resource::<crate::cpugovernor::GovernorSnapshot>().tier;
    let mut shed_count = 0u32;
    macro_rules! do_run {
        ($sys:expr, $label:expr, $class:expr) => {
            if !$class.runs(tier) {
                shed_count += 1;
            } else if timing {
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
    if shed_count > 0 {
        debug!("scheduler: shed {} system(s) under {:?}", shed_count, tier);
    }
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

            // Room (None = degraded mission with no room reference; the
            // mission's own failure path handles its teardown).
            if let Some(room) = mission.get_room() {
                if !is_valid(room) {
                    error!("INTEGRITY: dead room {:?} on mission {:?}, scheduling deletion", room, entity);
                    missions_to_delete.push(entity);
                    continue;
                }
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
                    error!(
                        "INTEGRITY: dead member entity {:?} removed from SquadContext {:?}",
                        m.entity, entity
                    );
                }
                ok
            });
            let after = sc.members.len();
            if after < before {
                warn!(
                    "INTEGRITY: removed {} dead member(s) from SquadContext {:?}",
                    before - after,
                    entity
                );
            }

            if let Some(heal_entity) = *sc.heal_priority {
                if !is_valid(heal_entity) {
                    error!(
                        "INTEGRITY: dead heal_priority entity {:?} cleared from SquadContext {:?}",
                        heal_entity, entity
                    );
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
        metrics: Write<'a, crate::metrics::MetricsState>,
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

            // Chunk-count watermark (IBEX-013/014): track how close the
            // encoded payload is to exhausting the component segments so the
            // 50..=54 shrink stays demonstrably safe as the empire grows.
            // Also routed into the seg-57 metrics block (Inc-2 rescope).
            let chunk_count = encoded_data.len().div_ceil(1024 * 50).max(1);
            data.metrics.record_segment_chunks(chunk_count as u32);
            if chunk_count + 1 >= self.segments.len() {
                warn!(
                    "Serialized world state near segment capacity: {} of {} chunk(s) used ({} encoded bytes)",
                    chunk_count,
                    self.segments.len(),
                    encoded_data.len()
                );
            } else {
                debug!(
                    "Serialized world state: {} of {} chunk(s) used ({} encoded bytes)",
                    chunk_count,
                    self.segments.len(),
                    encoded_data.len()
                );
            }

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
        metrics: Write<'a, crate::metrics::MetricsState>,
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
                // The decode path was previously SILENT (decode→empty =
                // a spontaneous empty world). Loud + counted now
                // (P1.A1 / Inc-2 rescope); falling through to an empty
                // payload is the reset itself — sanctioned, with a cause.
                let decoded_data = decode_buffer_from_string(&encoded_data).unwrap_or_else(|e| {
                    error!("Failed deserialization: segment decode failed, resetting world state: {}", e);
                    data.metrics.record_deser_failure();
                    Vec::new()
                });

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
                .unwrap_or_else(|e| {
                    error!("Failed deserialization: {}", e);
                    data.metrics.record_deser_failure();
                });
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
        SegmentRequirement::new("stats_history", vec![STATS_HISTORY_SEGMENT]).on_load(Box::new(|world: &mut World| {
            crate::stats_history::load_stats_history(world);
        })),
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
    world.insert(crate::pathing::pathfinderservice::PathfinderService::default());
    // Explicit (not just setup-derived): the metrics state must exist
    // before deserialize_world's run_now, whose SystemData is never
    // setup() (M3 — deser failures count into it).
    world.insert(crate::metrics::MetricsState::default());
    world.insert(RoomStatusCache::new());
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
    // Load feature flags from Memory (after resets, so the result
    // reflects any prepare() defaults). Inserted into the world below
    // as the per-tick Features Resource (M5).
    //

    let features = crate::features::load();

    ENVIRONMENT.with(|env_cell| {
        let mut env_ref = env_cell.borrow_mut();
        let env = env_ref.get_or_insert_with(create_environment);

        env.world.insert(features);

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

        // env.tick advances AFTER a successful serialize (end of this
        // function), not here (P1.C2 / ADR 0005): a tick that aborts
        // mid-way must not leave the surviving environment claiming it
        // completed. (Under the loader's halt-containment the env dies
        // with the VM anyway — this is correctness-by-construction for
        // any future containment that keeps the env alive.)

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
        // Tick-start metrics sample + CpuGovernor snapshot refresh
        // (P1.A1/P1.B3): every system reads ONE consistent governor
        // view for the whole tick.
        //

        crate::metrics::tick_start(&mut env.world);

        //
        // Harness fault injection (P1.A5): synthetic CPU burn, set via
        // `Memory._features.eval.cpu_burn_ms` by pressure scenarios.
        // Burned at the top of the tick so the governor and everything
        // it sheds see honest pressure.
        //

        let burn_ms = features.eval.cpu_burn_ms;
        if burn_ms > 0 {
            let end = game::cpu::get_used() + burn_ms as f64;
            while game::cpu::get_used() < end {
                std::hint::black_box(());
            }
            debug!("eval cpu burner: consumed {} ms", burn_ms);
        }

        // Containment acceptance probe (P1.C2): deliberate panic at an
        // exact tick — exercises the loader's catch/halt boundary and
        // the abort accounting end to end. Self-disarms (see features).
        if features.eval.panic_at_tick > 0 && game::time() == features.eval.panic_at_tick {
            panic!("eval fault injection: deliberate panic at tick {}", game::time());
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

        // The tick is committed only once its state is serialized
        // (P1.C2 — see the note at the old advance site).
        env.tick = Some(current_time);
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
