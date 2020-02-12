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
pub struct LocalBuildMission {
    builders: EntityVec
}

impl LocalBuildMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = LocalBuildMission::new();

        builder.with(MissionData::LocalBuild(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> LocalBuildMission {
        LocalBuildMission {
            builders: EntityVec::new()
        }
    }
}

impl Mission for LocalBuildMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("LocalBuildMission - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup builders that no longer exist.
        //

        self.builders.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            if self.builders.0.len() < 1 {
                let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

                if construction_sites.len() > 0 {
                    let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

                    let mission_entity = runtime_data.entity.clone();
                    let room_name = room.name();

                    let priority = SPAWN_PRIORITY_HIGH;

                    system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                        let name = name.to_string();

                        spawn_system_data.updater.exec_mut(move |world| {
                            let creep_job = JobData::Build(::jobs::build::BuildJob::new(&room_name));

                            let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                                .with(creep_job)
                                .build();

                            let mission_data_storage = &mut world.write_storage::<MissionData>();

                            if let Some(MissionData::LocalBuild(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                                mission_data.builders.0.push(creep_entity);
                            }       
                        });
                    })));
                }
            }

            return MissionResult::Running;
        } else {
            return MissionResult::Failure;
        }
    }
}