use super::data::*;
use crate::entitymappingsystem::EntityMappingData;
use crate::missions::data::*;
use crate::ownership::*;
use crate::room::data::*;
use crate::room::visibilitysystem::*;
use crate::room::roomplansystem::*;
use crate::ui::*;
use crate::visualize::*;
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
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
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
}

pub struct OperationExecutionRuntimeData {
    pub entity: Entity,
}

pub struct OperationDescribeData<'a> {
    pub entity: Entity,
    pub visualizer: &'a mut Visualizer,
    pub ui: &'a mut UISystem,
}

pub enum OperationResult {
    Running,
    Success,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Operation {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity>;

    fn owner_complete(&mut self, owner: OperationOrMissionEntity);

    fn child_complete(&mut self, _child: Entity) {}

    fn describe(&mut self, system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData);

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
        let mut system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &mut data.room_data,
            room_plan_data: &data.room_plan_data,
            room_plan_queue: &mut data.room_plan_queue,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
            visibility: &mut data.visibility,
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData {
                entity: entity,
            };

            let operation = operation_data.as_operation();

            operation.pre_run_operation(&mut system_data, &mut runtime_data);
        }

        //TODO: Is this the right phase for visualization? Potentially better at the end of tick?
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
                    let mut describe_data = OperationDescribeData {
                        entity: entity,
                        visualizer,
                        ui,
                    };

                    let operation = operation_data.as_operation();

                    operation.describe(&mut system_data, &mut describe_data);
                }
            }
        }
    }
}

fn queue_cleanup_operation(updater: &LazyUpdate, entity: Entity, owner: Option<OperationOrMissionEntity>) {
    updater.exec_mut(move |world| {
        match owner {
            Some(OperationOrMissionEntity::Operation(owner_operation_entity)) => {
                if let Some(operation_data) = world.write_storage::<OperationData>().get_mut(owner_operation_entity) {
                    operation_data.as_operation().child_complete(entity);
                }
            }
            Some(OperationOrMissionEntity::Mission(owner_mission_entity)) => {
                if let Some(mission_data) = world.write_storage::<MissionData>().get_mut(owner_mission_entity) {
                    mission_data.as_mission().child_complete(entity);
                }
            }
            None => {}
        }

        if let Err(err) = world.delete_entity(entity) {
            warn!("Trying to clean up operation entity that no longer exists. Error: {}", err);
        }
    });
}

pub struct RunOperationSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunOperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &mut data.room_data,
            room_plan_data: &data.room_plan_data,
            room_plan_queue: &mut data.room_plan_queue,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
            visibility: &mut data.visibility,
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData {
                entity: entity,
            };

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
                queue_cleanup_operation(&data.updater, entity, operation.get_owner().clone());
            }
        }
    }
}
