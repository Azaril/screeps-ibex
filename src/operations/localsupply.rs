use specs::*;
use specs::saveload::*;
use screeps::*;
use serde::{Serialize, Deserialize};

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::basicharvest::*;
use crate::missions::complexharvest::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LocalSupplyOperation {
}

impl LocalSupplyOperation
{
    pub fn build<B>(builder: B) -> B where B: Builder + MarkedBuilder {
        let operation = LocalSupplyOperation::new();

        builder.with(OperationData::LocalSupply(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> LocalSupplyOperation {
        LocalSupplyOperation {
        }
    }
}

impl Operation for LocalSupplyOperation
{
    fn run_operation<'a>(&mut self, system_data: &'a OperationExecutionSystemData) -> OperationResult {
        scope_timing!("LocalSupplyOperation");

        for (entity, room_owner, room_data) in (system_data.entities, system_data.room_owner, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_owner.owner) {
                if !room.find(find::MY_SPAWNS).is_empty() {
                    
                    //
                    // Query if any missions running on the room currently fufil the local supply role.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_local_supply_mission = room_data.missions.0.iter().any(|mission_entity| {
                        match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::BasicHarvest(_)) => true,
                            Some(MissionData::ComplexHarvest(_)) => true,
                            _ => false
                        }
                    });

                    //
                    // Spawn a new mission to fill the local supply role if missing.
                    //
        
                    if !has_local_supply_mission {
                        info!("Starting local supply for spawning room. Room: {}", room_owner.owner);

                        let level = if let Some(controller) = room.controller() {
                            controller.level()
                        } else {
                            0
                        };

                        let room_entity = entity;
                        let mission_room = room_owner.owner;

                        system_data.updater.exec_mut(move |world| {
                            //
                            // Spawn the mission entity.
                            //

                            let mission_entity = match level {
                                val if val <= 0 => None,
                                1 => Some(BasicHarvestMission::build(world.create_entity(), &mission_room).build()),
                                _ => Some(ComplexHarvestMission::build(world.create_entity(), &mission_room).build()),
                            };

                            //
                            // Attach the mission to the room.
                            //

                            if let Some(mission_entity) = mission_entity {
                                let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

                                if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                    room_data.missions.0.push(mission_entity);
                                }  
                            }                              
                        });
                    }
                }
            }
        }

        return OperationResult::Running;
    }
}