use super::constants::*;
use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::dismantle::*;
use crate::jobs::haul::*;
use crate::jobs::utility::dismantle::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::convert::*;

/// Loot volume one raider is assumed to clear per lifetime (≈500 capacity at
/// short range); sizes the raider roster from observed loot.
const RAIDER_LOOT_PER_LIFETIME: u32 = 25_000;
const MAX_RAIDERS: usize = 3;
/// Dismantle hit pool above which a second dismantler is worth spawning.
const SECOND_DISMANTLER_HITS: u32 = 1_000_000;

/// Salvage work observed in a room. `dismantle_hits` is decay-adjusted and
/// horizon-filtered; loot quantities are raw store contents in scope.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SalvageWork {
    pub loot_energy: u32,
    pub loot_other: u32,
    pub dismantle_hits: u32,
}

impl SalvageWork {
    pub fn loot_total(&self) -> u32 {
        self.loot_energy.saturating_add(self.loot_other)
    }
}

/// Survey a visible room for salvageable value. `lead_ticks` (travel + spawn
/// lead) discounts decaying structures — value that will rot away before our
/// creeps arrive is not value (ramparts bleed 3 hits/tick; containers 10/tick
/// in owned rooms, 50/tick in unowned). The admission EV gate passes a real
/// lead; in-mission re-surveys pass 0 (creeps are already on site).
pub fn assess_salvage_work(
    structures: &[StructureObject],
    sources: &[RemoteObjectId<Source>],
    max_structure_hits: u32,
    lead_ticks: u32,
    room_owned: bool,
) -> SalvageWork {
    let mut work = SalvageWork::default();
    let hostile_ramparts = hostile_rampart_positions(structures);

    for structure in structures.iter() {
        if is_salvage_loot_target(structure, sources, &hostile_ramparts) {
            if let Some(store) = structure.as_has_store() {
                for resource in store.store().store_types() {
                    let amount = store.store().get_used_capacity(Some(resource));
                    if resource == ResourceType::Energy {
                        work.loot_energy += amount;
                    } else {
                        work.loot_other += amount;
                    }
                }
            }
        }

        let dismantlable = structure.structure_type() != StructureType::Road
            && !ignore_for_dismantle(structure, sources)
            && can_dismantle(structure)
            && within_dismantle_hits_horizon(structure, max_structure_hits)
            && !blocked_by_hostile_rampart(structure, &hostile_ramparts);

        if dismantlable {
            if let Some(attackable) = structure.as_attackable() {
                let decay_per_tick = match structure {
                    StructureObject::StructureRampart(_) => RAMPART_DECAY_AMOUNT / RAMPART_DECAY_TIME,
                    // Containers decay 5x slower in owned rooms — and a
                    // derelict room is still owned until its controller drops.
                    StructureObject::StructureContainer(_) if room_owned => CONTAINER_DECAY / CONTAINER_DECAY_TIME_OWNED,
                    StructureObject::StructureContainer(_) => CONTAINER_DECAY / CONTAINER_DECAY_TIME,
                    _ => 0,
                };

                work.dismantle_hits += attackable.hits().saturating_sub(decay_per_tick * lead_ticks);
            }
        }
    }

    work
}

/// Per-room salvage mission: strips a militarily dead room of its resources
/// with two creep roles — raiders (pure Carry/Move running [`HaulJob`], loot
/// stores home) and dismantlers (Work-heavy running [`DismantleJob`], wreck
/// structures and recover the energy refund). Rosters are sized from the
/// observed work volume; loot-before-wreck falls out of the shared scope
/// filters (dismantle targets must have empty stores).
///
/// Created exclusively by `SalvageOperation` after the EV/strategic gate.
/// The mission owns the in-room lifecycle: it keeps intel fresh, aborts
/// loudly if the room re-arms, is serviced by its owner, or enters safe
/// mode, and completes once no enabled role has work left. Controller
/// takeover is deliberately NOT this mission's job — once the owner's
/// controller decays to neutral, the mining-outpost pipeline takes the room
/// over through its normal candidate flow.
#[derive(ConvertSaveload)]
pub struct SalvageMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    raiders: EntityVec<Entity>,
    dismantlers: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SalvageMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SalvageMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::Salvage(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> SalvageMission {
        SalvageMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            raiders: EntityVec::new(),
            dismantlers: EntityVec::new(),
        }
    }

    fn create_handle_raider_spawn(
        mission_entity: Entity,
        raid_room: Entity,
        delivery_rooms: &[Entity],
    ) -> crate::spawnsystem::SpawnQueueCallback {
        let delivery_rooms = delivery_rooms.to_owned();

        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();
            let delivery_rooms = delivery_rooms.clone();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Haul(HaulJob::new(&[raid_room], &delivery_rooms, false, false));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SalvageMission>()
                {
                    mission_data.raiders.push(creep_entity);
                }
            });
        })
    }

    fn create_handle_dismantler_spawn(
        mission_entity: Entity,
        dismantle_room: Entity,
        delivery_room: Entity,
        max_structure_hits: u32,
    ) -> crate::spawnsystem::SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Dismantle(DismantleJob::new(dismantle_room, delivery_room, false, max_structure_hits));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SalvageMission>()
                {
                    mission_data.dismantlers.push(creep_entity);
                }
            });
        })
    }

    /// Advertise in-scope loot to the transfer system so raider HaulJobs (and
    /// any other haulers passing through) pick it up.
    fn request_transfer_for_loot(transfer: &mut dyn TransferRequestSystem, room_data: &RoomData) -> Result<(), String> {
        //TODO: Fill out remaining types?
        //Structure::Ruin(s) => Ok(s.into()),
        //Structure::Tombstone(s) => Ok(s.into()),
        //Structure::Resource(s) => Ok(s.into()),

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return Ok(()),
        };

        let sources = room_data
            .get_static_visibility_data()
            .map(|s| s.sources().as_slice())
            .unwrap_or(&[]);

        let hostile_ramparts = hostile_rampart_positions(structures.all());

        for structure in structures
            .all()
            .iter()
            .filter(|s| is_salvage_loot_target(*s, sources, &hostile_ramparts))
        {
            if let Some(store) = structure.as_has_store() {
                for resource in store.store().store_types() {
                    let resource_amount = store.store().get_used_capacity(Some(resource));

                    if resource_amount > 0 {
                        if let Ok(transfer_target) = structure.try_into() {
                            let transfer_request = TransferWithdrawRequest::new(
                                transfer_target,
                                resource,
                                TransferPriority::Low,
                                resource_amount,
                                TransferType::Haul,
                            );

                            transfer.request_withdraw(transfer_request);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn spawn_raiders(
        &self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        room_name: RoomName,
    ) -> Result<(), String> {
        let token = system_data.spawn_queue.token();

        for home_room_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            let body_definition = SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: None,
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Raider - Target Room: {}", room_name),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Some(token),
                    Self::create_handle_raider_spawn(mission_entity, self.room_data, &self.home_room_datas),
                );

                system_data.spawn_queue.request(*home_room_entity, spawn_request);
            }
        }

        Ok(())
    }

    fn spawn_dismantlers(
        &self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        max_structure_hits: u32,
    ) -> Result<(), String> {
        let token = system_data.spawn_queue.token();

        for home_room_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            let body_definition = if home_room_data.get_structures().map(|s| !s.storages().is_empty()).unwrap_or(false) {
                SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: None,
                    maximum_repeat: None,
                    pre_body: &[Part::Move, Part::Move, Part::Work, Part::Work],
                    repeat_body: &[
                        Part::Work,
                        Part::Work,
                        Part::Move,
                        Part::Move,
                        Part::Carry,
                        Part::Carry,
                        Part::Move,
                        Part::Move,
                        Part::Carry,
                        Part::Carry,
                        Part::Move,
                        Part::Move,
                    ],
                    post_body: &[],
                }
            } else {
                SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: Some(1),
                    maximum_repeat: None,
                    pre_body: &[],
                    repeat_body: &[Part::Move, Part::Work],
                    post_body: &[],
                }
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    "Dismantler".to_string(),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Some(token),
                    Self::create_handle_dismantler_spawn(mission_entity, self.room_data, *home_room_entity, max_structure_hits),
                );

                system_data.spawn_queue.request(*home_room_entity, spawn_request);
            }
        }

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SalvageMission {
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
        self.raiders.retain(|e| *e != entity);
        self.dismantlers.retain(|e| *e != entity);
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Salvage - Raiders: {} Dismantlers: {}", self.raiders.len(), self.dismantlers.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Salvage - Raiders: {} Dismantlers: {}",
            self.raiders.len(),
            self.dismantlers.len()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        self.home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for salvage mission".to_owned());
        }

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room")?;

                Self::request_transfer_for_loot(transfer, room_data)?;

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let features = system_data.features;
        let derelict_features = features.derelict;

        if !derelict_features.on {
            return Err("Derelict-room handling disabled - aborting salvage".to_string());
        }

        // Phase 1: gates and work survey against an immutable room borrow.
        let (room_name, work, dismantle_ready) = {
            let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

            if dynamic_visibility_data.updated_within(1000) {
                if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
                    // Claimed - colony/outpost machinery owns the room now.
                    return Ok(MissionResult::Success);
                }

                if dynamic_visibility_data.militarily_active() {
                    return Err("Salvage target re-armed (spawn/tower/combat creeps) - aborting".to_string());
                }

                // For hostile-owned targets, any threat-capable creep sighting
                // (haulers refilling towers, claimers, healers) breaks the
                // derelict classification even though it is not "militarised".
                if dynamic_visibility_data.owner().hostile() && !dynamic_visibility_data.derelict() {
                    return Err("Salvage target no longer derelict (hostile activity sighted) - aborting".to_string());
                }
            } else if !dynamic_visibility_data.updated_within(derelict_features.action_max_age) {
                return Err("Salvage intel too stale - aborting".to_string());
            }

            if dynamic_visibility_data.safe_mode_active() {
                // Safe mode blocks withdraw/dismantle for us; abort and let the
                // operation re-admit once it has expired.
                return Err("Salvage target under safe mode - aborting".to_string());
            }

            // Work survey needs live structure data; keep eyes on the room.
            let survey = room_data.get_structures().map(|structures| {
                let sources = room_data
                    .get_static_visibility_data()
                    .map(|s| s.sources().as_slice())
                    .unwrap_or(&[]);

                let work = assess_salvage_work(
                    structures.all(),
                    sources,
                    derelict_features.max_structure_hits,
                    0,
                    dynamic_visibility_data.owner().hostile(),
                );

                // "Ready" = a target with an empty store exists right now;
                // store-full structures become ready as raiders drain them.
                let dismantle_ready = requires_dismantling(structures.all(), sources, derelict_features.max_structure_hits);

                (work, dismantle_ready)
            });

            match survey {
                Some((work, dismantle_ready)) => (room_data.name, work, dismantle_ready),
                None => {
                    system_data.visibility.request(VisibilityRequest::new(
                        room_data.name,
                        VISIBILITY_PRIORITY_MEDIUM,
                        VisibilityRequestFlags::ALL,
                    ));

                    return Ok(MissionResult::Running);
                }
            }
        };

        // Per-role desired rosters from observed work. Disabled feature flags
        // zero the role; live creeps finish their jobs and expire naturally.
        let desired_raiders = if features.raid && work.loot_total() > 0 {
            (work.loot_total().div_ceil(RAIDER_LOOT_PER_LIFETIME) as usize).clamp(1, MAX_RAIDERS)
        } else {
            0
        };

        let desired_dismantlers = if features.dismantle && dismantle_ready {
            if work.dismantle_hits > SECOND_DISMANTLER_HITS {
                2
            } else {
                1
            }
        } else {
            0
        };

        if desired_raiders == 0 && desired_dismantlers == 0 {
            info!("Salvage of room {} complete - no enabled work remains", room_name);

            return Ok(MissionResult::Success);
        }

        if !system_data.governor.can_execute_cpu(CpuBar::LowPriority) {
            return Ok(MissionResult::Running);
        }

        if self.raiders.len() < desired_raiders {
            self.spawn_raiders(system_data, mission_entity, room_name)?;
        }

        if self.dismantlers.len() < desired_dismantlers {
            self.spawn_dismantlers(system_data, mission_entity, derelict_features.max_structure_hits)?;
        }

        Ok(MissionResult::Running)
    }
}
