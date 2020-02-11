use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};
use itertools::*;

use super::data::*;
use super::missionsystem::*;
use ::jobs::data::*;
use ::spawnsystem::*;
use crate::serialize::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct LocalSupplyMission {
    harvesters: EntityVec
}

impl LocalSupplyMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = LocalSupplyMission::new();

        builder.with(MissionData::LocalSupply(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> LocalSupplyMission {
        LocalSupplyMission {
            harvesters: EntityVec::new()
        }
    }
}

impl Mission for LocalSupplyMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("LocalSupply - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup harvesters that no longer exist.
        //

        self.harvesters.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            //
            // Spawn harvesters to fufill basic room needs.
            //

            let sources_to_harvesters = self.harvesters.0.iter()
                .filter_map(|harvester_entity| {
                    if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                        Some((harvester_data.harvest_target, harvester_entity))
                    } else {
                        None
                    }
                })
                .into_group_map();

            let sources = room.find(find::SOURCES);
            let available_sources = sources.iter()
                .map(|source| {
                    if let Some(harvesters) = sources_to_harvesters.get(&source.id()) {
                        (harvesters.len(), source)
                    } else {
                        (0, source)
                    }
                })
                .filter(|(harvester_count, _)| {
                    //TODO: Compute correct number of harvesters need for source.
                    return *harvester_count < 4;
                })
                .map(|(harvester_count, source)| {
                    let priority = if harvester_count == 0 { SPAWN_PRIORITY_CRITICAL } else { SPAWN_PRIORITY_HIGH };

                    (priority, source)
                });

            for available_source in available_sources {
                let (priority, source) = available_source;
                let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

                let mission_entity = runtime_data.entity.clone();
                let source_id = source.id();

                system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                    let name = name.to_string();

                    spawn_system_data.updater.exec_mut(move |world| {
                        let creep_job = JobData::Harvest(::jobs::harvest::HarvestJob::new(source_id));

                        let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                            .with(creep_job)
                            .build();

                        let mission_data_storage = &mut world.write_storage::<MissionData>();

                        if let Some(MissionData::LocalSupply(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                            mission_data.harvesters.0.push(creep_entity);
                        }       
                    });
                })));
            }

            return MissionResult::Running;
        } else {
            return MissionResult::Failure;
        }
    }
}