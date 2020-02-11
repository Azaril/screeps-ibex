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
pub struct ComplexHarvestMission {
    harvesters: EntityVec
}

impl ComplexHarvestMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = ComplexHarvestMission::new();

        builder.with(MissionData::ComplexHarvest(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> ComplexHarvestMission {
        ComplexHarvestMission {
            harvesters: EntityVec::new()
        }
    }
}

impl Mission for ComplexHarvestMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("ComplexHarvest - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup harvesters that no longer exist.
        //

        self.harvesters.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {

            //
            // Terminate the mission if not at least room level 2.
            //

            let level = if let Some(controller) = room.controller() {
                controller.level()
            } else {
                0
            };

            if level < 2 {
                return MissionResult::Failure;
            }

            //TODO: This should use miners and hauler roles instead of harvester.

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
            let available_sources = sources.iter().filter(|source| {
                let current_count = if let Some(harvesters) = sources_to_harvesters.get(&source.id()) {
                    harvesters.len()
                } else {
                    0
                };

                return current_count < 4;
            });

            for source in available_sources {
                let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

                let mission_entity = runtime_data.entity.clone();
                let source_id = source.id();
                let priority = if self.harvesters.0.is_empty() { SPAWN_PRIORITY_CRITICAL } else { SPAWN_PRIORITY_HIGH };

                system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                    let name = name.to_string();

                    spawn_system_data.updater.exec_mut(move |world| {
                        let creep_job = JobData::Harvest(::jobs::harvest::HarvestJob::new(source_id));

                        let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                            .with(creep_job)
                            .build();

                        let mission_data_storage = &mut world.write_storage::<MissionData>();

                        if let Some(MissionData::ComplexHarvest(mission_data)) = mission_data_storage.get_mut(mission_entity) {
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