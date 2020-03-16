use super::data::*;
use crate::mappingsystem::MappingData;
use crate::room::visibilitysystem::*;
use crate::ui::*;
use crate::visualize::*;
use specs::prelude::*;
use crate::missions::data::*;
use crate::room::data::*;
use log::*;

#[derive(SystemData)]
pub struct OperationSystemData<'a> {
    operations: WriteStorage<'a, OperationData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    mission_data: ReadStorage<'a, MissionData>,
    mapping: Read<'a, MappingData>,
    visibility: Write<'a, VisibilityQueue>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

pub struct OperationExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub room_data: &'a WriteStorage<'a, RoomData>,
    pub mission_data: &'a ReadStorage<'a, MissionData>,
    pub mapping: &'a Read<'a, MappingData>,
}

pub struct OperationExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
    pub visibility: &'a mut VisibilityQueue,
}

pub struct OperationDescribeData<'a> {
    pub entity: &'a Entity,
    pub visualizer: &'a mut Visualizer,
    pub ui: &'a mut UISystem,
}

pub enum OperationResult {
    Running,
    Success,
}

pub trait Operation {
    fn describe(&mut self, system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData);

    fn pre_run_operation(&mut self, _system_data: &OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()>;
}

pub struct PreRunOperationSystem;

impl<'a> System<'a> for PreRunOperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData {
                entity: &entity,
                visibility: &mut data.visibility,
            };

            let operation = operation_data.as_operation();

            operation.pre_run_operation(&system_data, &mut runtime_data);
        }

        //TODO: Is this the right phase for visualization? Potentially better at the end of tick?
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
                    let mut describe_data = OperationDescribeData {
                        entity: &entity,
                        visualizer,
                        ui,
                    };

                    let operation = operation_data.as_operation();

                    operation.describe(&system_data, &mut describe_data);
                }
            }
        }
    }
}

pub struct RunOperationSystem;

impl<'a> System<'a> for RunOperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
        };

        for (entity, operation_data) in (&data.entities, &mut data.operations).join() {
            let mut runtime_data = OperationExecutionRuntimeData {
                entity: &entity,
                visibility: &mut data.visibility,
            };

            let operation = operation_data.as_operation();

            let cleanup_operation = match operation.run_operation(&system_data, &mut runtime_data) {
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
                data.updater.exec_mut(move |world| {
                    if let Err(err) = world.delete_entity(entity) {
                        warn!("Trying to clean up operation entity that no longer exists. Error: {}", err);
                    }
                });
            }
        }
    }
}
