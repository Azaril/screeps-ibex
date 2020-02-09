use serde::*;
use specs::*;
use specs::saveload::*;
use screeps::*;

use super::data::*;
use super::operationsystem::*;
use ::missions::data::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct BootstrapOperation {
}

impl BootstrapOperation
{
    pub fn build<B>(builder: B) -> B where B: Builder + MarkedBuilder {
        let operation = BootstrapOperation::new();

        builder.with(OperationData::Bootstrap(operation))
            .marked::<::serialize::SerializeMarker>()
            .marked::<OperationMarker>()
    }

    pub fn new() -> BootstrapOperation {
        BootstrapOperation {
        }
    }    
}

impl Operation for BootstrapOperation
{
    fn run_operation<'a>(&mut self, data: &OperationRuntimeData) {
        scope_timing!("BootstrapOperation");

        for (entity, room_owner, room_data) in (data.entities, data.room_owner, data.room_data).join() {
            if room_data.missions.is_empty() {
                if let Some(room) = game::rooms::get(room_owner.owner) {
                    if !room.find(find::MY_SPAWNS).is_empty() {
                        info!("Starting bootstrap room for spawning room with no missions.");

                        let room_entity = entity;
                        let mission_room = room_owner.owner;

                        data.updater.exec_mut(move |world| {
                            let mission_entity = ::missions::bootstrap::BootstrapMission::build(world.create_entity(), &mission_room).build();

                            let mission_marker_storage = world.read_storage::<MissionMarker>();
                            let mission_marker = mission_marker_storage.get(mission_entity);

                            let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

                            if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                room_data.missions.push(*mission_marker.unwrap());
                            }                                
                        });
                    }
                }
            }
        }
    }
}