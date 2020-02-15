use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::tower::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TowerOperation {}

impl TowerOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = TowerOperation::new();

        builder
            .with(OperationData::Tower(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> TowerOperation {
        TowerOperation {}
    }
}

impl Operation for TowerOperation {
    fn run_operation<'a>(
        &mut self,
        system_data: &'a OperationExecutionSystemData,
    ) -> OperationResult {
        scope_timing!("TowerOperation");

        for (entity, room_owner, room_data) in (
            system_data.entities,
            system_data.room_owner,
            system_data.room_data,
        )
            .join()
        {
            if let Some(room) = game::rooms::get(room_owner.owner) {
                //TODO: Factor this out.
                let mut towers = room
                    .find(find::MY_STRUCTURES)
                    .into_iter()
                    .map(|owned_structure| owned_structure.as_structure())
                    .filter_map(|structure| {
                        if let Structure::Tower(tower) = structure {
                            Some(tower)
                        } else {
                            None
                        }
                    });

                if towers.any(|_| true) {
                    //
                    // Query if any missions running on the room currently fufil the tower.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_tower_mission = room_data.missions.0.iter().any(|mission_entity| {
                        match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::Tower(_)) => true,
                            _ => false,
                        }
                    });

                    //
                    // Spawn a new mission to fill the local tower control role if missing.
                    //

                    if !has_tower_mission {
                        info!(
                            "Starting tower mission for room. Room: {}",
                            room_owner.owner
                        );

                        let room_entity = entity;
                        let mission_room = room_owner.owner;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity =
                                TowerMission::build(world.create_entity(), mission_room).build();

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
