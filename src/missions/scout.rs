use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use crate::jobs::data::*;
use crate::spawnsystem::*;
use super::data::*;
use super::missionsystem::*;
use crate::serialize::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct ScoutMission {
    room_data: Entity,
    home_room_data: Entity,
    scouts: EntityVec,
}

impl ScoutMission {
    pub fn build<B>(builder: B, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ScoutMission::new(room_data, home_room_data);

        builder
            .with(MissionData::Scout(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity, home_room_data: Entity) -> ScoutMission {
        ScoutMission {
            room_data,
            home_room_data,
            scouts: EntityVec::new(),
        }
    }
}

impl Mission for ScoutMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("ScoutMission");

        //
        // Cleanup harvesters that no longer exist.
        //

        self.scouts
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));

        //
        // NOTE: Room may not be visible if there is no creep or building active in the room.
        //

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if room_data.get_dynamic_visibility_data().is_some() && self.scouts.0.is_empty() {
                info!("Completing scout mission - room is visible and no active scouts. Room: {}", room_data.name);

                //TODO: Current data is all immutable - this needs to change when there's a freshness value on the data.
                return MissionResult::Success;
            }

            if let Some(home_room_data) = system_data.room_data.get(self.home_room_data) {
                if let Some(home_room) = game::rooms::get(home_room_data.name) {
                    if self.scouts.0.is_empty() {
                        //TODO: Compute best body parts to use.
                        let body_definition = crate::creep::SpawnBodyDefinition {
                            maximum_energy: home_room.energy_capacity_available(),
                            minimum_repeat: Some(1),
                            maximum_repeat: Some(1),
                            pre_body: &[],
                            repeat_body: &[Part::Move],
                            post_body: &[],
                        };

                        if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                            let priority = SPAWN_PRIORITY_LOW;

                            let mission_entity = *runtime_data.entity;
                            let scout_room = room_data.name;

                            system_data.spawn_queue.request(SpawnRequest::new(
                                home_room_data.name,
                                &body,
                                priority,
                                Box::new(move |spawn_system_data, name| {
                                    let name = name.to_string();

                                    spawn_system_data.updater.exec_mut(move |world| {
                                        let creep_job =
                                            JobData::Scout(::jobs::scout::ScoutJob::new(
                                                scout_room
                                            ));

                                        let creep_entity =
                                            ::creep::Spawning::build(world.create_entity(), &name)
                                                .with(creep_job)
                                                .build();

                                        let mission_data_storage =
                                            &mut world.write_storage::<MissionData>();

                                        if let Some(MissionData::Scout(mission_data)) =
                                            mission_data_storage.get_mut(mission_entity)
                                        {
                                            mission_data.scouts.0.push(creep_entity);
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
            
        MissionResult::Failure
    }
}
