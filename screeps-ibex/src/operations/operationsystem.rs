use super::data::*;
use crate::cleanup::*;
use crate::entitymappingsystem::EntityMappingData;
use crate::military::economy::*;
use crate::military::threatmap::RoomThreatData;
use crate::missions::data::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::room::visibilitysystem::*;
use crate::visualization::{MapVisualizationData, SummaryContent, VisualizationData};
use log::*;
use specs::prelude::*;

#[derive(SystemData)]
pub struct OperationSystemData<'a> {
    operations: WriteStorage<'a, OperationData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    room_plan_data: ReadStorage<'a, RoomPlanData>,
    room_plan_queue: Write<'a, RoomPlanQueue>,
    mission_data: ReadStorage<'a, MissionData>,
    mapping: Read<'a, EntityMappingData>,
    visibility: Write<'a, VisibilityQueue>,
    visualization_data: Option<Write<'a, VisualizationData>>,
    cleanup_queue: Write<'a, EntityCleanupQueue>,
    economy: Write<'a, EconomySnapshot>,
    route_cache: Write<'a, RoomRouteCache>,
    threat_data: ReadStorage<'a, RoomThreatData>,
}

pub struct OperationExecutionSystemData<'a, 'b> {
    pub updater: &'b Read<'a, LazyUpdate>,
    pub entities: &'b Entities<'a>,
    pub room_data: &'b mut WriteStorage<'a, RoomData>,
    pub room_plan_data: &'b ReadStorage<'a, RoomPlanData>,
    pub room_plan_queue: &'b mut RoomPlanQueue,
    pub mission_data: &'b ReadStorage<'a, MissionData>,
    pub mapping: &'b Read<'a, EntityMappingData>,
    pub visibility: &'b mut VisibilityQueue,
    pub map_viz_data: Option<&'b mut MapVisualizationData>,
    pub economy: &'b mut EconomySnapshot,
    pub route_cache: &'b mut RoomRouteCache,
    pub threat_data: &'b ReadStorage<'a, RoomThreatData>,
}

pub struct OperationExecutionRuntimeData {
    pub entity: Entity,
}

pub enum OperationResult {
    Running,
    Success,
}

/// Read-only context passed to `Operation::describe_operation` for summarization.
pub struct OperationDescribeContext<'a> {
    pub mission_data: &'a ReadStorage<'a, MissionData>,
    pub room_data: &'a ReadStorage<'a, RoomData>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Operation {
    fn get_owner(&self) -> &Option<Entity>;

    fn owner_complete(&mut self, owner: Entity);

    fn child_complete(&mut self, _child: Entity) {}

    /// Remove any internal entity references that fail the validity check.
    ///
    /// Called by `repair_entity_integrity` before serialization to prevent
    /// `ConvertSaveload` panics on dangling entities. The provided closure
    /// returns `true` if the entity is alive and has a `SerializeMarker`.
    /// Default implementation is a no-op (safe for operations without
    /// entity-valued fields beyond `owner`).
    fn repair_entity_refs(&mut self, _is_valid: &dyn Fn(Entity) -> bool) {}

    /// Produce a structured summary for the visualization overlay.
    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text("Operation".to_string())
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()>;
}

pub struct PreRunOperationSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunOperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let map_viz = data.visualization_data.as_deref_mut().map(|v| &mut v.map);

        let mut system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &mut data.room_data,
            room_plan_data: &data.room_plan_data,
            room_plan_queue: &mut data.room_plan_queue,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
            visibility: &mut data.visibility,
            map_viz_data: map_viz,
            economy: &mut data.economy,
            route_cache: &mut data.route_cache,
            threat_data: &data.threat_data,
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData { entity };

            let operation = operation_data.as_operation();

            operation.pre_run_operation(&mut system_data, &mut runtime_data);
        }
    }
}

pub struct RunOperationSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunOperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let map_viz = data.visualization_data.as_deref_mut().map(|v| &mut v.map);

        let mut system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &mut data.room_data,
            room_plan_data: &data.room_plan_data,
            room_plan_queue: &mut data.room_plan_queue,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
            visibility: &mut data.visibility,
            map_viz_data: map_viz,
            economy: &mut data.economy,
            route_cache: &mut data.route_cache,
            threat_data: &data.threat_data,
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData { entity };

            let operation = operation_data.as_operation();

            let cleanup_operation = match operation.run_operation(&mut system_data, &mut runtime_data) {
                Ok(OperationResult::Running) => false,
                Ok(OperationResult::Success) => {
                    info!("Operation complete, cleaning up.");

                    true
                }
                Err(_) => {
                    info!("Operation failed, cleaning up.");

                    true
                }
            };

            if cleanup_operation {
                data.cleanup_queue.delete_operation(OperationCleanup {
                    entity,
                    owner: *operation.get_owner(),
                });
            }
        }
    }
}
