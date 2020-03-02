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

#[derive(Clone, ConvertSaveload)]
pub struct HaulMission {
    room_data: Entity,
    haulers: EntityVec,
}

impl HaulMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = HaulMission::new(room_data);

        builder.with(MissionData::Haul(mission)).marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> HaulMission {
        HaulMission {
            room_data,
            haulers: EntityVec::new(),
        }
    }

    fn create_handle_hauler_spawn(
        mission_entity: Entity,
        haul_rooms: &[Entity],
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync> {
        let rooms = haul_rooms.to_vec();

        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();
            let rooms = rooms.clone();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Haul(::jobs::haul::HaulJob::new(&rooms));

                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Haul(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.haulers.0.push(creep_entity);
                }
            });
        })
    }
}

impl Mission for HaulMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text("Haul".to_string(), None);
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup haulers that no longer exist.
        //

        self.haulers.0.retain(|entity| system_data.entities.is_alive(*entity));

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        scope_timing!("HaulMission");

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let stats = runtime_data
            .transfer_queue
            .try_get_room(room_data.name)
            .map(|r| r.stats());

        let need_hauling = stats
            .map(|s| s.total_withdrawl > 0)
            .unwrap_or(false);

        let should_spawn = need_hauling && self.haulers.0.len() < 2;

        if should_spawn {
            let energy_to_use = if self.haulers.0.is_empty() {
                room.energy_available()
            } else {
                room.energy_capacity_available()
            };

            //TODO: Compute best body parts to use.
            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy: energy_to_use,
                minimum_repeat: Some(1),
                maximum_repeat: Some(8),
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                let haul_rooms = &[self.room_data];

                let priority = if self.haulers.0.is_empty() {
                    SPAWN_PRIORITY_HIGH
                } else {
                    SPAWN_PRIORITY_MEDIUM
                };

                //TODO: Compute priority based on transfer requests.
                //TODO: Make sure there is handling for starvation/bootstrap mode.
                let spawn_request = SpawnRequest::new(
                    format!("Haul - Target Room: {}", room_data.name),
                    &body,
                    priority,
                    Self::create_handle_hauler_spawn(*runtime_data.entity, haul_rooms),
                );

                runtime_data.spawn_queue.request(room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}