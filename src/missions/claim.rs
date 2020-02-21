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
use crate::room::data::*;

#[derive(Clone, ConvertSaveload)]
pub struct ClaimMission {
    room_data: Entity,
    home_room_data: Entity,
    claimers: EntityVec,
}

impl ClaimMission {
    pub fn build<B>(builder: B, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ClaimMission::new(room_data, home_room_data);

        builder
            .with(MissionData::Claim(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity, home_room_data: Entity) -> ClaimMission {
        ClaimMission {
            room_data,
            home_room_data,
            claimers: EntityVec::new(),
        }
    }
}

impl Mission for ClaimMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("ClaimMission");

        //
        // Cleanup claimers that no longer exist.
        //

        self.claimers
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));

        //
        // NOTE: Room may not be visible if there is no creep or building active in the room.
        //

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.updated_within(1000) {
                    match dynamic_visibility_data.owner() {
                        RoomDisposition::Mine => { 
                            return MissionResult::Success;
                        },
                        RoomDisposition::Friendly(_) | RoomDisposition::Hostile(_) => {
                            return MissionResult::Failure
                        },
                        RoomDisposition::Neutral => {}
                    }

                    match dynamic_visibility_data.reservation() {
                        RoomDisposition::Mine | RoomDisposition::Neutral => {},
                        RoomDisposition::Friendly(_) | RoomDisposition::Hostile(_) => {
                            return MissionResult::Failure
                        }
                    }
                }
            }

            if let Some(static_visibility_data) = room_data.get_static_visibility_data() {
                if let Some(controller) = static_visibility_data.controller() {
                    if let Some(home_room_data) = system_data.room_data.get(self.home_room_data) {
                        if let Some(home_room) = game::rooms::get(home_room_data.name) {
                            if self.claimers.0.is_empty() {
                                let body_definition = crate::creep::SpawnBodyDefinition {
                                    maximum_energy: home_room.energy_capacity_available(),
                                    minimum_repeat: None,
                                    maximum_repeat: None,
                                    pre_body: &[Part::Claim, Part::Move],
                                    repeat_body: &[],
                                    post_body: &[],
                                };

                                if let Ok(body) =
                                    crate::creep::Spawning::create_body(&body_definition)
                                {
                                    let priority = SPAWN_PRIORITY_MEDIUM;

                                    let mission_entity = *runtime_data.entity;
                                    let controller_id = *controller;

                                    system_data.spawn_queue.request(SpawnRequest::new(
                                        home_room_data.name,
                                        &body,
                                        priority,
                                        Box::new(move |spawn_system_data, name| {
                                            let name = name.to_string();

                                            spawn_system_data.updater.exec_mut(move |world| {
                                                let creep_job = JobData::Claim(
                                                    ::jobs::claim::ClaimJob::new(
                                                        controller_id
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

                                                if let Some(MissionData::Claim(mission_data)) =
                                                    mission_data_storage.get_mut(mission_entity)
                                                {
                                                    mission_data.claimers.0.push(creep_entity);
                                                }
                                            });
                                        }),
                                    ));
                                }
                            }

                            return MissionResult::Running;
                        }
                    }
                }
            }
        }

        MissionResult::Failure
    }
}
