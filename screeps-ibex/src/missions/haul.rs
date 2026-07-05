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

    fn get_room(&self) -> Option<Entity> {
        Some(self.room_data)
    }

    fn remove_creep(&mut self, entity: Entity) {
        self.haulers.retain(|e| *e != entity);
    }

    fn get_creeps(&self) -> Vec<Entity> {
        self.haulers.iter().copied().collect()
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Hauler - Haulers: {}", self.haulers.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Haul - Haulers: {}", self.haulers.len()))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
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
            |s| game::time().saturating_sub(s.last_updated) >= 20 && room_visible,
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

        // K4 policy (ADR 0040 M3): the hauler body shape, demand sizing and priority bands
        // live in `screeps_econ_decision::spawn_policy` (consumed here and by the economy sim).
        let body_definition = screeps_econ_decision::spawn_policy::hauler_body(is_multi_room, energy_to_use);

        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
            let carry_parts = body.iter().filter(|p| **p == Part::Carry).count();

            let (desired_haulers_for_unfufilled, desired_haulers) =
                screeps_econ_decision::spawn_policy::hauler_desired(stats.unfufilled_hauling, carry_parts as u32, max_distance);

            let should_spawn = self.haulers.len() < desired_haulers && self.allow_spawning;

            if should_spawn {
                // Civilian ROI bid (ADR 0040 §D2, M5b — `body_roi_milli`): the hauler's §D5.4 `w`
                // is its logistics rate = throughput unblocked (cargo per round-trip amortized over
                // the round-trip time). `range_multiplier = 1/((max_distance·2)+1)` is exactly the
                // demand-sizing round-trip factor (spawn_policy::hauler_desired), so
                // `carry × CARRY_CAPACITY × multiplier` is the per-tick throughput this body serves.
                let body_cost: u32 = body.iter().map(|p| p.cost()).sum();
                let range_multiplier_milli = (screeps_econ_decision::sink_economics::BID_SCALE) / ((max_distance * 2) + 1);
                let logistics_rate_milli = (carry_parts as u32) * 50 * range_multiplier_milli;
                let priority = screeps_econ_decision::spawn_policy::hauler_bid(
                    self.haulers.len(),
                    desired_haulers_for_unfufilled,
                    max_distance,
                    logistics_rate_milli,
                    body_cost,
                );

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
