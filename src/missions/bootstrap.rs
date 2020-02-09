use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};

use super::data::*;
use super::missionsystem::*;
use ::jobs::data::*;
use ::spawnsystem::*;
use crate::serialize::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct BootstrapMission {
    harvesters: EntityVec
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
            harvesters: EntityVec::new()
        }
    }
}

impl Mission for BootstrapMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) {
        scope_timing!("BootstrapMission - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup harvesters that no longer exist.
        //

        self.harvesters.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            let sources = room.find(find::SOURCES);
            let available_sources = sources.iter().filter(|_source| self.harvesters.0.len() <= 4);

            for source in available_sources {
                let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

                let mission_entity = runtime_data.entity.clone();
                let source_id = source.id();

                system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, Box::new(move |spawn_system_data, name| {
                    let name = name.to_string();

                    spawn_system_data.updater.exec_mut(move |world| {
                        let creep_job = JobData::Harvest(::jobs::harvest::HarvestJob::new(source_id));

                        let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                            .with(creep_job)
                            .build();

                        let mission_data_storage = &mut world.write_storage::<MissionData>();

                        if let Some(mission_data) = mission_data_storage.get_mut(mission_entity) {
                            let MissionData::Bootstrap(ref mut bootstrap_data) = mission_data;

                            bootstrap_data.harvesters.0.push(creep_entity);
                        }                            
                    });
                })));
            }
        }
    }
}