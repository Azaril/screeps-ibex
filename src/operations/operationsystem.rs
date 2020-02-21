use specs::prelude::*;

use super::data::*;
use crate::mappingsystem::MappingData;
use crate::room::visibilitysystem::*;

#[derive(SystemData)]
pub struct OperationSystemData<'a> {
    operations: WriteStorage<'a, OperationData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, ::room::data::RoomData>,
    mission_data: ReadStorage<'a, ::missions::data::MissionData>,
    mapping: Read<'a, MappingData>,
    visibility: Read<'a, VisibilityQueue>,
}

pub struct OperationExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub room_data: &'a WriteStorage<'a, ::room::data::RoomData>,
    pub mission_data: &'a ReadStorage<'a, ::missions::data::MissionData>,
    pub mapping: &'a Read<'a, MappingData>,
    pub visibility: &'a Read<'a, VisibilityQueue>,
}

pub struct OperationExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
}

pub enum OperationResult {
    Running,
    Success,
    Failure,
}

pub trait Operation {
    fn run_operation(&mut self, system_data: &OperationExecutionSystemData, runtime_data: &OperationExecutionRuntimeData) -> OperationResult;
}

pub struct OperationSystem;

impl<'a> System<'a> for OperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("OperationSystem");

        let system_data = OperationExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            mission_data: &data.mission_data,
            mapping: &data.mapping,
            visibility: &data.visibility,
        };

        for (entity, operation) in (&data.entities, &mut data.operations).join() {
            let runtime_data = OperationExecutionRuntimeData {
                entity: &entity
            };

            let cleanup_operation = match operation.as_operation().run_operation(&system_data, &runtime_data) {
                OperationResult::Running => false,
                OperationResult::Success => {
                    info!("Operation complete, cleaning up.");

                    true
                }
                OperationResult::Failure => {
                    info!("Operation failed, cleaning up.");

                    true
                }
            };

            if cleanup_operation {
                data.updater.exec_mut(move |world| {
                    if let Err(err) = world.delete_entity(entity) {
                        warn!(
                            "Trying to clean up operation entity that no longer exists. Error: {}",
                            err
                        );
                    }
                });
            }
        }
    }
}
