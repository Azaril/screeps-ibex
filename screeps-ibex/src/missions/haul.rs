use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::jobs::data::*;
use crate::jobs::haul::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, Serialize, Deserialize)]
struct HaulingStats {
    last_updated: u32,
    unfufilled_hauling: u32,
}

#[derive(ConvertSaveload)]
pub struct HaulMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    haulers: EntityVec<Entity>,
    //TODO: Create a room stats component?
    stats: Option<HaulingStats>,
    allow_spawning: bool,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HaulMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = HaulMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::Haul(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> HaulMission {
        HaulMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.into(),
            haulers: EntityVec::new(),
            stats: None,
            allow_spawning: true,
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    fn create_handle_hauler_spawn(
        mission_entity: Entity,
        pickup_rooms: &[Entity],
        delivery_rooms: &[Entity],
        allow_repair: bool,
        storage_delivery_only: bool,
    ) -> crate::spawnsystem::SpawnQueueCallback {
        let pickup_rooms = pickup_rooms.to_vec();
        let delivery_rooms = delivery_rooms.to_vec();

        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();
            let pickup_rooms = pickup_rooms.clone();
            let delivery_rooms = delivery_rooms.clone();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Haul(HaulJob::new(&pickup_rooms, &delivery_rooms, allow_repair, storage_delivery_only));

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
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
    ) -> HaulingStats
    where
        RD: std::ops::Deref<Target = specs::storage::MaskedStorage<RoomData>>,
    {
        let unfufilled = transfer_queue.total_unfufilled_resources(transfer_queue_data, pickup_rooms, delivery_rooms, TransferType::Haul);

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

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Haul - Haulers: {}", self.haulers.len()))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup haulers that no longer exist.
        //

        self.haulers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        //
        // Cleanup home rooms that no longer exist.
        //

        self.home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for haul mission".to_owned());
        }

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data_storage = &*system_data.room_data;
        let room_data = room_data_storage.get(self.room_data).ok_or("Expected room data")?;

        let transfer_queue = &mut *system_data.transfer_queue;
        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Haul Run Mission",
            room_data: room_data_storage,
        };

        let room_visible = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

        let home_room_datas: Vec<_> = self
            .home_room_datas
            .iter()
            .filter_map(|e| room_data_storage.get(*e).map(|d| (e, d)))
            .collect();

        if home_room_datas.is_empty() {
            return Err("No home rooms available for hauling".to_owned());
        }

        let home_room_names: Vec<_> = home_room_datas.iter().map(|(_, r)| r.name).collect();

        let pickup_rooms = &[room_data.name];

        let mut stats = self.stats.access(
            |s| game::time() - s.last_updated >= 20 && room_visible,
            || Self::update_stats(transfer_queue, &transfer_queue_data, pickup_rooms, &home_room_names),
        );
        let stats = stats.get();

        //TODO: Use find route plus cache.
        let home_room_spawn_info: Vec<_> = home_room_datas
            .iter()
            .filter_map(|(entity, home_room_data)| {
                let room_offset_distance = home_room_data.name - room_data.name;

                let room_manhattan_distance = room_offset_distance.0.unsigned_abs() + room_offset_distance.1.unsigned_abs();

                //TODO: Use structure cache?
                let room = game::rooms().get(home_room_data.name)?;
                let controller = room.controller()?;

                let current_energy = room.energy_available().max(SPAWN_ENERGY_CAPACITY);
                let max_energy = room.energy_capacity_available();

                Some((
                    entity,
                    room,
                    room_manhattan_distance,
                    controller.level(),
                    current_energy,
                    max_energy,
                ))
            })
            .collect();

        let is_multi_room = home_room_spawn_info.iter().any(|(_, _, distance, _, _, _)| *distance > 0);

        let token = system_data.spawn_queue.token();

        let energy_to_use = if self.haulers.is_empty() {
            home_room_spawn_info
                .iter()
                .map(|(_, _, _, _, current_energy, _)| *current_energy)
                .max()
        } else {
            home_room_spawn_info.iter().map(|(_, _, _, _, _, max_energy)| *max_energy).min()
        }
        .unwrap_or(SPAWN_ENERGY_CAPACITY);

        let max_distance = home_room_spawn_info
            .iter()
            .map(|(_, _, distance, _, _, _)| *distance)
            .max()
            .unwrap_or(0);

        let body_definition = if is_multi_room {
            crate::creep::SpawnBodyDefinition {
                maximum_energy: energy_to_use,
                minimum_repeat: Some(1),
                maximum_repeat: Some(20),
                pre_body: &[Part::Work, Part::Move],
                repeat_body: &[Part::Carry, Part::Move],
                post_body: &[],
            }
        } else {
            crate::creep::SpawnBodyDefinition {
                maximum_energy: energy_to_use,
                minimum_repeat: Some(1),
                maximum_repeat: Some(20),
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Move],
                post_body: &[],
            }
        };

        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
            let carry_parts = body.iter().filter(|p| **p == Part::Carry).count();

            let range_multiplier = 1.0 / ((max_distance as f32 * 2.0) + 1.0);
            let base_amount = carry_parts as f32 * CARRY_CAPACITY as f32 * range_multiplier;

            let max_haulers = 3 + (max_distance * 3);

            let desired_haulers_for_unfufilled = (stats.unfufilled_hauling as f32 / base_amount) as u32;
            let desired_haulers = desired_haulers_for_unfufilled.min(max_haulers) as usize;

            let should_spawn = self.haulers.len() < desired_haulers && self.allow_spawning;

            if should_spawn {
                let priority = if (self.haulers.len() as f32) < (desired_haulers_for_unfufilled as f32 * 0.75).ceil() {
                    if max_distance == 0 {
                        SPAWN_PRIORITY_HIGH
                    } else {
                        SPAWN_PRIORITY_MEDIUM
                    }
                } else {
                    if max_distance == 0 {
                        SPAWN_PRIORITY_MEDIUM
                    } else {
                        SPAWN_PRIORITY_LOW
                    }
                };

                let pickup_rooms = &[self.room_data];

                let allow_repair = max_distance > 0;
                let storage_delivery_only = max_distance > 0;

                for (entity, _, _, _, _, _) in home_room_spawn_info {
                    //TODO: Make sure there is handling for starvation/bootstrap mode.
                    let spawn_request = SpawnRequest::new(
                        format!("Haul - Target Room: {}", room_data.name),
                        &body,
                        priority,
                        Some(token),
                        Self::create_handle_hauler_spawn(
                            mission_entity,
                            pickup_rooms,
                            &self.home_room_datas,
                            allow_repair,
                            storage_delivery_only,
                        ),
                    );

                    system_data.spawn_queue.request(**entity, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
