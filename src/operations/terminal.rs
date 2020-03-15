use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::terminal::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use crate::room::data::*;
use crate::serialize::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct TerminalOperation {}

impl TerminalOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = TerminalOperation::new();

        builder
            .with(OperationData::Terminal(operation))
            .marked::<SerializeMarker>()
    }

    pub fn new() -> TerminalOperation {
        TerminalOperation {}
    }
}

impl Operation for TerminalOperation {
    fn describe(&mut self, _system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Terminal".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                if let Some(controller) = room.controller() {
                    if controller.my() && room.terminal().is_some() {
                        //
                        // Query if any missions running on the room currently fufil the Terminal role.
                        //

                        //TODO: wiarchbe: Use trait instead of match.
                        let has_terminal_mission =
                            room_data
                                .missions
                                .0
                                .iter()
                                .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                                    Some(MissionData::Terminal(_)) => true,
                                    _ => false,
                                });

                        //
                        // Spawn a new mission to fill the terminal role if missing.
                        //

                        if !has_terminal_mission {
                            info!("Starting terminal mission for spawning room. Room: {}", room_data.name);

                            let room_entity = entity;

                            system_data.updater.exec_mut(move |world| {
                                let mission_entity = TerminalMission::build(world.create_entity(), room_entity).build();

                                //
                                // Attach the mission to the room.
                                //

                                let room_data_storage = &mut world.write_storage::<RoomData>();

                                if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                    room_data.missions.0.push(mission_entity);
                                }
                            });
                        }
                    }
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
