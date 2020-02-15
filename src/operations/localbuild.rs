use specs::*;
use specs::saveload::*;
use screeps::*;
use serde::{Serialize, Deserialize};

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::localbuild::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LocalBuildOperation {
}

impl LocalBuildOperation
{
    pub fn build<B>(builder: B) -> B where B: Builder + MarkedBuilder {
        let operation = LocalBuildOperation::new();

        builder.with(OperationData::LocalBuild(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> LocalBuildOperation {
        LocalBuildOperation {
        }
    }
}

impl Operation for LocalBuildOperation
{
    fn run_operation<'a>(&mut self, system_data: &'a OperationExecutionSystemData) -> OperationResult {
        scope_timing!("LocalBuildOperation");

        for (entity, room_owner, room_data) in (system_data.entities, system_data.room_owner, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_owner.owner) {
                if !room.find(find::MY_SPAWNS).is_empty() {
                    
                    //
                    // Query if any missions running on the room currently fufil the local supply role.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_local_build_mission = room_data.missions.0.iter().any(|mission_entity| {
                        match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::LocalBuild(_)) => true,
                            _ => false
                        }
                    });

                    //
                    // Spawn a new mission to fill the local build role if missing.
                    //
        
                    if !has_local_build_mission {
                        info!("Starting local build for spawning room. Room: {}", room_owner.owner);

                        let room_entity = entity;
                        let mission_room = room_owner.owner;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity = LocalBuildMission::build(world.create_entity(), mission_room).build();

                            let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

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