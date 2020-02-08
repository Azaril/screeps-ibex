use serde::*;
use specs::*;
use specs::saveload::*;
use screeps::*;

use super::data::*;
use super::missionsystem::*;
use ::room::data::*;
use ::jobs::data::*;
use ::creep::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct BootstrapMission {
}

impl BootstrapMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = BootstrapMission::new();

        builder.with(MissionData::Bootstrap(mission))
            .marked::<::serialize::SerializeMarker>()
            .marked::<MissionMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> BootstrapMission {
        BootstrapMission {
        }
    }
}

impl Mission for BootstrapMission
{
    fn run_mission<'a>(&mut self, data: &MissionRuntimeData, room_owner: &RoomOwnerData) {
        scope_timing!("BootstrapMission - Room: {}", room_owner.owner);
        
        if let Some(room) = game::rooms::get(room_owner.owner) {
            let sources = room.find(find::SOURCES);
            if let Some(source) = sources.first() {
                for spawn in room.find(find::MY_SPAWNS) {
                    let body = [Part::Move, Part::Move, Part::Carry, Part::Work];
    
                    if spawn.energy() >= body.iter().map(|p| p.cost()).sum() {
                        let time = screeps::game::time();
                        let mut additional = 0;
                        let (res, name) = loop {
                            let name = format!("{}-{}", time, additional);
                            let res = spawn.spawn_creep(&body, &name);
        
                            if res == ReturnCode::NameExists {
                                additional += 1;
                            } else {
                                break (res, name);
                            }
                        };
        
                        if res != ReturnCode::Ok {
                            warn!("Failed to spawn creep: {:?}", res);
                        } else {
                            let job = JobData::Harvest(::jobs::harvest::HarvestJob::new(&source));
        
                            data.updater.exec_mut(move |world| {
                                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name, &job).build();
    
                                let creep_marker_storage = world.read_storage::<CreepMarker>();
                                let _creep_marker = creep_marker_storage.get(creep_entity);
    
                                /*
                                let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();
    
                                if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                    room_data.missions.push(*mission_marker.unwrap());
                                }
                                */                               
                            });      
                        }
                    }
                }
            }
        }
    }
}