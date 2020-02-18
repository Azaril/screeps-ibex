use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct RemoteMineMission {
    room_data: Entity,
    home_room_data: Entity,
    harvesters: EntityVec,
}

impl RemoteMineMission {
    pub fn build<B>(builder: B, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RemoteMineMission::new(room_data, home_room_data);

        builder
            .with(MissionData::RemoteMine(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity, home_room_data: Entity) -> RemoteMineMission {
        RemoteMineMission {
            room_data,
            home_room_data,
            harvesters: EntityVec::new(),
        }
    }
}

impl Mission for RemoteMineMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("RemoteMineMission");

        //
        // Cleanup harvesters that no longer exist.
        //

        self.harvesters
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));

        //
        // NOTE: Room may not be visible if there is no creep or building active in the room.
        //

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.updated_within(1000) && dynamic_visibility_data.hostile()
                {
                    return MissionResult::Failure;
                }
            }

            if let Some(static_visibility_data) = room_data.get_static_visibility_data() {
                if let Some(home_room_data) = system_data.room_data.get(self.home_room_data) {
                    if let Some(home_room) = game::rooms::get(home_room_data.name) {
                        //TODO: Store this mapping data as part of the mission. (Blocked on specs collection serialization.)
                        let mut sources_to_harvesters = self
                            .harvesters
                            .0
                            .iter()
                            .filter_map(|harvester_entity| {
                                if let Some(JobData::Harvest(harvester_data)) =
                                    system_data.job_data.get(*harvester_entity)
                                {
                                    Some((harvester_data.harvest_target.id(), harvester_entity))
                                } else {
                                    None
                                }
                            })
                            .into_group_map();

                        for source in static_visibility_data.sources().iter() {
                            let source_id = source.id();

                            let source_harvesters = sources_to_harvesters
                                .remove(&source_id)
                                .unwrap_or_else(Vec::new);

                            //
                            // Spawn harvesters
                            //

                            //TODO: Compute correct number of harvesters to use for source.
                            if source_harvesters.len() < 2 {
                                //TODO: Compute best body parts to use.
                                let body_definition = crate::creep::SpawnBodyDefinition {
                                    maximum_energy: home_room.energy_capacity_available(),
                                    minimum_repeat: Some(1),
                                    maximum_repeat: Some(6),
                                    pre_body: &[],
                                    repeat_body: &[Part::Move, Part::Move, Part::Carry, Part::Work],
                                    post_body: &[],
                                };

                                if let Ok(body) =
                                    crate::creep::Spawning::create_body(&body_definition)
                                {
                                    let priority = SPAWN_PRIORITY_LOW;

                                    let mission_entity = *runtime_data.entity;
                                    let delivery_room = home_room_data.name;
                                    let source_id = *source;

                                    system_data.spawn_queue.request(SpawnRequest::new(
                                        home_room_data.name,
                                        &body,
                                        priority,
                                        Box::new(move |spawn_system_data, name| {
                                            let name = name.to_string();

                                            spawn_system_data.updater.exec_mut(move |world| {
                                                let creep_job = JobData::Harvest(
                                                    ::jobs::harvest::HarvestJob::new(
                                                        source_id,
                                                        delivery_room,
                                                    ),
                                                );

                                                let creep_entity = ::creep::Spawning::build(
                                                    world.create_entity(),
                                                    &name,
                                                )
                                                .with(creep_job)
                                                .build();

                                                let mission_data_storage =
                                                    &mut world.write_storage::<MissionData>();

                                                if let Some(MissionData::RemoteMine(mission_data)) =
                                                    mission_data_storage.get_mut(mission_entity)
                                                {
                                                    mission_data.harvesters.0.push(creep_entity);
                                                }
                                            });
                                        }),
                                    ));
                                }
                            }
                        }

                        return MissionResult::Running;
                    }
                }
            }
        }

        MissionResult::Failure
    }
}
