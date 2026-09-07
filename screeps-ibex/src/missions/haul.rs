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

/// The energy a home room's spawn lane can be refilled to with NO new income — the K4 starvation-
/// sizing input (`spawn_policy::replacement_body_energy`, RULING-11 root B): the lane itself plus
/// the room's HAULABLE stock — storage, the source-side links and the source-side containers. A
/// container OR link within range 3 of the controller is the upgrade buffer (a Use-lane sink, never
/// hauled back to the lane — counting a full controller link would keep a capacity body "reachable"
/// on a lane that can only reach the 300 regen, re-forming the bank) and the terminal sits under
/// the 10k transfer reserve, so none of those count. Reads the cached
/// `RoomStructureData` (no `find`); a room without cached structures reads as its lane alone.
pub(crate) fn spawn_lane_reachable_energy(room_data: &RoomData, energy_available: u32) -> u32 {
    let Some(structures) = room_data.get_structures() else {
        return energy_available;
    };
    let controller_pos = structures.controllers().first().map(|c| c.pos());
    let energy_in = |store: Store| store.get_used_capacity(Some(ResourceType::Energy));
    let storage: u32 = structures.storages().iter().map(|s| energy_in(s.store())).sum();
    let links: u32 = structures
        .links()
        .iter()
        .filter(|l| controller_pos.map(|p| l.pos().get_range_to(p) > 3).unwrap_or(true))
        .map(|l| energy_in(l.store()))
        .sum();
    let containers: u32 = structures
        .containers()
        .iter()
        .filter(|c| controller_pos.map(|p| c.pos().get_range_to(p) > 3).unwrap_or(true))
        .map(|c| energy_in(c.store()))
        .sum();
    energy_available
        .saturating_add(storage)
        .saturating_add(links)
        .saturating_add(containers)
}

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

                let energy_available = room.energy_available();
                let max_energy = room.energy_capacity_available();
                let reachable_energy = spawn_lane_reachable_energy(home_room_data, energy_available);

                Some((
                    entity,
                    room,
                    room_manhattan_distance,
                    controller.level(),
                    energy_available,
                    max_energy,
                    reachable_energy,
                ))
            })
            .collect();

        let is_multi_room = home_room_spawn_info.iter().any(|(_, _, distance, _, _, _, _)| *distance > 0);

        let token = system_data.spawn_queue.token();

        // K4 policy (ADR 0040 M3): the hauler body shape, demand sizing and priority bands
        // live in `screeps_econ_decision::spawn_policy` (consumed here and by the economy sim).
        let energy_to_use = if self.haulers.is_empty() {
            // The bootstrap carrier: sized from available-now energy (floored at the 300 regen) —
            // always fieldable, bid at the bootstrap floor (`hauler_bid`).
            home_room_spawn_info
                .iter()
                .map(|(_, _, _, _, energy_available, _, _)| (*energy_available).max(SPAWN_ENERGY_CAPACITY))
                .max()
        } else {
            // K4 starvation sizing (RULING-11 root B): the capacity body only when the lane can
            // reach its cost from what the room holds; else the affordable-now body. Over the
            // shared-token homes the conservative (min) facts decide, matching the min capacity.
            let energy_capacity = home_room_spawn_info.iter().map(|(_, _, _, _, _, max_energy, _)| *max_energy).min();
            energy_capacity.map(|energy_capacity| {
                let capacity_body_cost =
                    crate::creep::spawning::create_body(&screeps_econ_decision::spawn_policy::hauler_body(is_multi_room, energy_capacity))
                        .map(|body| body.iter().map(|p| p.cost()).sum())
                        .unwrap_or(energy_capacity);
                let energy_available = home_room_spawn_info
                    .iter()
                    .map(|(_, _, _, _, energy_available, _, _)| *energy_available)
                    .min()
                    .unwrap_or(0);
                let reachable_energy = home_room_spawn_info
                    .iter()
                    .map(|(_, _, _, _, _, _, reachable)| *reachable)
                    .min()
                    .unwrap_or(0);
                screeps_econ_decision::spawn_policy::replacement_body_energy(
                    energy_available,
                    energy_capacity,
                    reachable_energy,
                    capacity_body_cost,
                )
            })
        }
        .unwrap_or(SPAWN_ENERGY_CAPACITY);

        let max_distance = home_room_spawn_info
            .iter()
            .map(|(_, _, distance, _, _, _, _)| *distance)
            .max()
            .unwrap_or(0);

        let body_definition = screeps_econ_decision::spawn_policy::hauler_body(is_multi_room, energy_to_use);

        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
            let carry_parts = body.iter().filter(|p| **p == Part::Carry).count();

            let (desired_haulers_for_unfufilled, desired_haulers) =
                screeps_econ_decision::spawn_policy::hauler_desired(stats.unfufilled_hauling, carry_parts as u32, max_distance);

            let should_spawn = self.haulers.len() < desired_haulers && self.allow_spawning;

            if should_spawn {
                // Civilian ROI bid (ADR 0040 §D2, M5b — `body_roi_milli`; ADR 0043 A10 marginal
                // form): the hauler's §D5.4 `w` is the marginal throughput this body unblocks —
                // `min(body throughput, unfulfilled hauling not served by the alive roster)` in
                // the demand-sizing currency (`hauler_desired`), so a lane whose demand is met
                // prices at its band and an unserved one bids up, never reaching the miner band.
                // The first local hauler is the bootstrap carrier (the floor above miners).
                let body_cost: u32 = body.iter().map(|p| p.cost()).sum();
                let priority = screeps_econ_decision::spawn_policy::hauler_bid(
                    self.haulers.len(),
                    desired_haulers_for_unfufilled,
                    max_distance,
                    stats.unfufilled_hauling,
                    carry_parts as u32,
                    body_cost,
                );

                let pickup_rooms = &[self.room_data];

                let allow_repair = max_distance > 0;
                let storage_delivery_only = max_distance > 0;

                for (entity, _, _, _, _, _, _) in home_room_spawn_info {
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

#[cfg(test)]
mod tests {
    use super::super::localsupply::body_helpers::source_miner_body;
    use crate::spawnsystem::{SpawnQueue, SpawnRequest};
    use screeps::Part;
    use screeps_econ_decision::spawn_policy::*;
    use specs::prelude::*;

    fn cost(body: &[Part]) -> u32 {
        body.iter().map(|p| p.cost()).sum()
    }

    fn request(description: &str, body: &[Part], bid: u32) -> SpawnRequest {
        SpawnRequest::new(description.to_owned(), body, bid, None, Box::new(|_, _| {}))
    }

    /// Queue heads (description, cost) after the caller-side bids run through the real
    /// descending head-of-line-banking `SpawnQueue`.
    fn queue_order(requests: Vec<SpawnRequest>) -> Vec<(String, u32)> {
        let mut world = World::new();
        let room = world.create_entity().build();
        let mut queue = SpawnQueue::default();
        for r in requests {
            queue.request(room, r);
        }
        queue
            .room_requests(room)
            .iter()
            .map(|r| (r.description().to_owned(), r.cost()))
            .collect()
    }

    /// RULING-11 root B (2026-09-07), ibex-side pin — INCOME OUTRANKS LOGISTICS at the queue the
    /// callers feed. The live W5N49 deadlock shape (lane 300; containers 2000/2000 → 4000e unmet;
    /// 2 small haulers alive; 0 miners): the capacity-sized 1800e hauler used to head the queue at
    /// 99_999 over the 550e miners at HIGH and bank the lane forever. Now the miners head it, the
    /// restart harvester heads the miners, and the first carrier heads the miners too.
    #[test]
    fn income_outranks_logistics_in_the_spawn_queue() {
        let hauler_1800 = crate::creep::spawning::create_body(&hauler_body(false, 1_800)).unwrap();
        let carry = hauler_1800.iter().filter(|p| **p == Part::Carry).count() as u32;
        let miner_550 = crate::creep::spawning::create_body(&source_miner_body(true, 1_300, 5, false)).unwrap();
        assert_eq!(cost(&miner_550), 550, "the live 5W container miner");

        // W5N49: 2 haulers alive, 4000e unmet; hauler_desired(4000, 18 carry, d=0) yields 4 (4000/900).
        let (desired_for_unfulfilled, _) = hauler_desired(4_000, carry, 0);
        let hauler_bid_w5n49 = hauler_bid(2, desired_for_unfulfilled, 0, 4_000, carry, cost(&hauler_1800));
        let order = queue_order(vec![
            request("Haul", &hauler_1800, hauler_bid_w5n49),
            request("Container Miner", &miner_550, SPAWN_BID_MINER),
            request("Container Miner", &miner_550, SPAWN_BID_MINER),
        ]);
        assert_eq!(
            order[0],
            ("Container Miner".to_owned(), 550),
            "a miner heads the queue, not the 1800e hauler ({order:?})"
        );
        assert_eq!(order[1].0, "Container Miner");
        assert_eq!(order[2], ("Haul".to_owned(), 1_800), "the hauler banks BEHIND income");

        // W16N51: the only link miner expired; its 600e replacement vs an 1800e hauler (1 hauler alive).
        let link_miner = crate::creep::spawning::create_body(&source_miner_body(true, 1_800, 5, true)).unwrap();
        let order = queue_order(vec![
            request("Haul", &hauler_1800, hauler_bid(1, 26, 0, 4_000, carry, 1_800)),
            request("Link Miner", &link_miner, SPAWN_BID_MINER),
        ]);
        assert_eq!(order[0].0, "Link Miner", "the income body heads the queue ({order:?})");

        // Bootstrap ladder: restart harvester > first carrier > miner > replacement harvester.
        let harvester = crate::creep::spawning::create_body(&harvester_body(300)).unwrap();
        let carrier = crate::creep::spawning::create_body(&hauler_body(false, 300)).unwrap();
        let order = queue_order(vec![
            request("Container Miner", &miner_550, SPAWN_BID_MINER),
            request("Harvester (replacement)", &harvester, harvester_bid(1, 1, 4, 0)),
            request("Haul (first)", &carrier, hauler_bid(0, 26, 0, 4_000, 3, 300)),
            request("Harvester (restart)", &harvester, harvester_bid(0, 0, 4, 0)),
        ]);
        let names: Vec<&str> = order.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["Harvester (restart)", "Haul (first)", "Container Miner", "Harvester (replacement)"],
            "the bootstrap ladder orders restart > carrier > income > replacement"
        );
    }

    /// RULING-11 root B, ibex-side pin — STARVATION SIZING yields always-fieldable bodies through
    /// the live body definitions: with the lane at the 300 regen and nothing haulable, every
    /// replacement (hauler / harvester / container miner / link miner) costs ≤ 300; with stock
    /// reachable, the capacity bodies come back unchanged (steady state).
    #[test]
    fn starvation_sized_replacement_bodies_are_always_fieldable() {
        let starved = |capacity: u32, capacity_body_cost: u32| replacement_body_energy(300, capacity, 300, capacity_body_cost);
        let hauler = crate::creep::spawning::create_body(&hauler_body(false, starved(1_800, 1_800))).unwrap();
        assert_eq!(cost(&hauler), 300, "3C3M hauler");
        let harvester = crate::creep::spawning::create_body(&harvester_body(starved(2_300, 1_250))).unwrap();
        assert_eq!(cost(&harvester), 250, "[M,M,C,W] harvester");
        let miner = crate::creep::spawning::create_body(&source_miner_body(true, starved(1_300, 550), 5, false)).unwrap();
        assert_eq!(cost(&miner), 250, "[M,W,W] container miner");
        let link_miner = crate::creep::spawning::create_body(&source_miner_body(true, starved(1_800, 600), 5, true)).unwrap();
        assert_eq!(cost(&link_miner), 300, "[M,C,W,W] link miner");

        // Reachable stock (full containers) → the capacity bodies, exactly as before.
        let fed = |capacity: u32, capacity_body_cost: u32| replacement_body_energy(300, capacity, 4_300, capacity_body_cost);
        assert_eq!(
            cost(&crate::creep::spawning::create_body(&hauler_body(false, fed(1_800, 1_800))).unwrap()),
            1_800
        );
        assert_eq!(
            cost(&crate::creep::spawning::create_body(&source_miner_body(true, fed(1_300, 550), 5, false)).unwrap()),
            550
        );
    }
}
