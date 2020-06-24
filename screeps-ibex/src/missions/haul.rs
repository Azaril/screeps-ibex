use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::haul::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::collections::HashMap;

#[derive(Clone, Serialize, Deserialize)]
struct HaulingStats {
    last_updated: u32,
    unfufilled_hauling: u32,
}

#[derive(ConvertSaveload)]
pub struct HaulMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    haulers: EntityVec<Entity>,
    //TODO: Create a room stats component?
    stats: Option<HaulingStats>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HaulMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = HaulMission::new(owner, room_data);

        builder
            .with(MissionData::Haul(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> HaulMission {
        HaulMission {
            owner: owner.into(),
            room_data,
            haulers: EntityVec::new(),
            stats: None,
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

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<HaulMission>()
                {
                    mission_data.haulers.push(creep_entity);
                }
            });
        })
    }

    fn update_stats<'a, 's, RD>(
        transfer_queue: &mut TransferQueue,
        transfer_queue_data: &TransferQueueGeneratorData<'a, 's, RD>,
        room_name: RoomName,
    ) -> HaulingStats
    where
        RD: std::ops::Deref<Target = specs::storage::MaskedStorage<RoomData>>,
    {
        let unfufilled = transfer_queue
            .try_get_room(transfer_queue_data, room_name, TransferType::Haul.into())
            .map(|r| r.stats().total_unfufilled_resources(TransferType::Haul))
            .unwrap_or_else(HashMap::new);

        let total_unfufilled: u32 = unfufilled.values().sum();

        HaulingStats {
            last_updated: game::time(),
            unfufilled_hauling: total_unfufilled,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for HaulMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Hauler - Haulers: {}", self.haulers.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup haulers that no longer exist.
        //

        self.haulers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data_storage = &*system_data.room_data;
        let room_data = room_data_storage.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let controller = room.controller().ok_or("Expected controller")?;

        let transfer_queue = &mut *system_data.transfer_queue;
        let mut transfer_queue_data = TransferQueueGeneratorData {
            cause: "Haul Run Mission",
            room_data: &*room_data_storage,
        };

        let mut stats = self.stats.access(
            |s| game::time() - s.last_updated >= 20,
            || Self::update_stats(transfer_queue, &mut transfer_queue_data, room_data.name),
        );
        let stats = stats.get();

        let base_amount = (controller.level() as f32 * 100.0).powf(1.25);

        let desired_haulers_for_unfufilled = (stats.unfufilled_hauling as f32 / base_amount as f32).ceil() as usize;
        let desired_haulers = desired_haulers_for_unfufilled.min(3) as usize;

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

                let priority = if (self.haulers.len() as f32) < (desired_haulers_for_unfufilled as f32 * 0.75).ceil() {
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
                    Self::create_handle_hauler_spawn(mission_entity, haul_rooms),
                );

                system_data.spawn_queue.request(self.room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
