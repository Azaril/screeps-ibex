use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::haul::*;
use crate::ownership::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use std::collections::HashMap;

#[derive(Clone, ConvertSaveload)]
pub struct HaulMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
    haulers: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HaulMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = HaulMission::new(owner, room_data);

        builder.with(MissionData::Haul(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity) -> HaulMission {
        HaulMission {
            owner: owner.into(),
            room_data,
            haulers: EntityVec::new(),
        }
    }

    fn create_handle_hauler_spawn(mission_entity: Entity, haul_rooms: &[Entity]) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        let rooms = haul_rooms.to_vec();

        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();
            let rooms = rooms.clone();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Haul(HaulJob::new(&rooms, &rooms));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Haul(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.haulers.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for HaulMission {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: OperationOrMissionEntity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _describe_data: &mut MissionDescribeData) -> String {
        format!("Hauler - Haulers: {}", self.haulers.len())
    }

    fn pre_run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup haulers that no longer exist.
        //

        self.haulers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let controller = room.controller().ok_or("Expected controller")?;

        let unfufilled = system_data
            .transfer_queue
            .try_get_room(room_data.name)
            .map(|r| r.stats().total_unfufilled_resources(TransferType::Haul))
            .unwrap_or_else(HashMap::new);

        let total_unfufilled: u32 = unfufilled.values().sum();

        let base_amount = controller.level() * 500;

        let desired_haulers = (total_unfufilled as f32 / base_amount as f32).ceil().min(3.0) as usize;

        let should_spawn = self.haulers.len() < desired_haulers;

        if should_spawn {
            let energy_to_use = if self.haulers.is_empty() {
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

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let haul_rooms = &[self.room_data];

                let priority = if self.haulers.is_empty() {
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
                    Self::create_handle_hauler_spawn(runtime_data.entity, haul_rooms),
                );

                system_data.spawn_queue.request(room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
