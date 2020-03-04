use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
#[cfg(feature = "time")]
use timing_annotate::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::localbuild::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LocalBuildOperation {}

#[cfg_attr(feature = "time", timing)]
impl LocalBuildOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = LocalBuildOperation::new();

        builder
            .with(OperationData::LocalBuild(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> LocalBuildOperation {
        LocalBuildOperation {}
    }
}

#[cfg_attr(feature = "time", timing)]
impl Operation for LocalBuildOperation {
    fn describe(&mut self, _system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Local Build".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                let controller = room.controller();
                let my_room = controller.map(|controller| controller.my()).unwrap_or(false);

                if my_room {
                    //
                    // Query if any missions running on the room currently fufil the local supply role.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_local_build_mission =
                        room_data
                            .missions
                            .0
                            .iter()
                            .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                                Some(MissionData::LocalBuild(_)) => true,
                                _ => false,
                            });

                    //
                    // Spawn a new mission to fill the local build role if missing.
                    //

                    if !has_local_build_mission {
                        info!("Starting local build for spawning room. Room: {}", room_data.name);

                        let room_entity = entity;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity = LocalBuildMission::build(world.create_entity(), room_entity).build();

                            let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

                            if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                room_data.missions.0.push(mission_entity);
                            }
                        });
                    }
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
