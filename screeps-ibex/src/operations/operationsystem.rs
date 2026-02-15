use super::data::*;
use crate::entitymappingsystem::EntityMappingData;
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
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData { entity };

            let operation = operation_data.as_operation();

            operation.pre_run_operation(&mut system_data, &mut runtime_data);
        }
    }
}

fn queue_cleanup_operation(updater: &LazyUpdate, operation_entity: Entity, owner: Option<Entity>) {
    updater.exec_mut(move |world| {
        if let Some(owner) = owner {
            if let Some(operation_data) = world.write_storage::<OperationData>().get_mut(owner) {
                operation_data.as_operation().child_complete(operation_entity);
            }

            if let Some(mission_data) = world.write_storage::<MissionData>().get_mut(owner) {
                mission_data.as_mission_mut().child_complete(operation_entity);
            }
        }

        if let Err(err) = world.delete_entity(operation_entity) {
            warn!("Trying to clean up operation entity that no longer exists. Error: {}", err);
        }
    });
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

            //TODO: Copy over cleanup semantics from mission code.

            if cleanup_operation {
                queue_cleanup_operation(&data.updater, entity, *operation.get_owner());
            }
        }
    }
}
