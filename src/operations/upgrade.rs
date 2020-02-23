use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::upgrade::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct UpgradeOperation {}

impl UpgradeOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = UpgradeOperation::new();

        builder
            .with(OperationData::Upgrade(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> UpgradeOperation {
        UpgradeOperation {}
    }
}

impl Operation for UpgradeOperation {
    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) -> OperationResult {
        scope_timing!("UpgradeOperation");

        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                if let Some(controller) = room.controller() {
                    if controller.my() {
                        //
                        // Query if any missions running on the room currently fufil the upgrade role.
                        //

                        //TODO: wiarchbe: Use trait instead of match.
                        let has_upgrade_mission =
                            room_data.missions.0.iter().any(|mission_entity| {
                                match system_data.mission_data.get(*mission_entity) {
                                    Some(MissionData::Upgrade(_)) => true,
                                    _ => false,
                                }
                            });

                        //
                        // Spawn a new mission to fill the upgrade role if missing.
                        //

                        if !has_upgrade_mission {
                            info!(
                                "Starting upgrade mission for spawning room. Room: {}",
                                room_data.name
                            );

                            let room_entity = entity;

                            system_data.updater.exec_mut(move |world| {
                                let mission_entity =
                                    UpgradeMission::build(world.create_entity(), room_entity)
                                        .build();

                                //
                                // Attach the mission to the room.
                                //

                                let room_data_storage =
                                    &mut world.write_storage::<::room::data::RoomData>();

                                if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                    room_data.missions.0.push(mission_entity);
                                }
                            });
                        }
                    }
                }
            }
        }

        OperationResult::Running
    }
}
