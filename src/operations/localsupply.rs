use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::localsupply::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LocalSupplyOperation {}

impl LocalSupplyOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = LocalSupplyOperation::new();

        builder
            .with(OperationData::LocalSupply(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> LocalSupplyOperation {
        LocalSupplyOperation {}
    }
}

impl Operation for LocalSupplyOperation {
    fn run_operation<'a>(
        &mut self,
        system_data: &'a OperationExecutionSystemData,
    ) -> OperationResult {
        scope_timing!("LocalSupplyOperation");

        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                let controller = room.controller();
                let my_room = controller
                    .map(|controller| controller.my())
                    .unwrap_or(false);

                if my_room {
                    //
                    // Query if any missions running on the room currently fufil the local supply role.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_local_supply_mission =
                        room_data.missions.0.iter().any(|mission_entity| {
                            match system_data.mission_data.get(*mission_entity) {
                                Some(MissionData::LocalSupply(_)) => true,
                                _ => false,
                            }
                        });

                    //
                    // Spawn a new mission to fill the local supply role if missing.
                    //

                    if !has_local_supply_mission {
                        info!(
                            "Starting local supply for spawning room. Room: {}",
                            room_data.name
                        );

                        let room_entity = entity;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity =
                                LocalSupplyMission::build(world.create_entity(), room_entity)
                                    .build();

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

        OperationResult::Running
    }
}
