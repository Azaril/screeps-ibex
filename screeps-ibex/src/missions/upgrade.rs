use super::constants::*;
use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::upgrade::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct UpgradeMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    upgraders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UpgradeMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = UpgradeMission::new(owner, room_data);

        builder
            .with(MissionData::Upgrade(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> UpgradeMission {
        UpgradeMission {
            owner: owner.into(),
            room_data,
            upgraders: EntityVec::new(),
        }
    }

    pub fn can_run(room_data: &RoomData) -> bool {
        room_data
            .get_structures()
            .map(|s| s.controllers().iter().any(|c| c.my()))
            .unwrap_or(false)
    }

    fn create_handle_upgrader_spawn(mission_entity: Entity, home_room: Entity) -> crate::spawnsystem::SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Upgrade(UpgradeJob::new(home_room));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<UpgradeMission>()
                {
                    mission_data.upgraders.push(creep_entity);
                }
            });
        })
    }

    // `work_parts_for_upkeep` (the clock-saving upgrader sizing model) lives in
    // `screeps_econ_decision::spawn_policy` since ADR 0040 M3 (K4) — one implementation,
    // consumed here and by the economy sim; its model documentation moved with it.
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for UpgradeMission {
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
        self.upgraders.retain(|e| *e != entity);
    }

    fn get_creeps(&self) -> Vec<Entity> {
        self.upgraders.iter().copied().collect()
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Upgrade - Upgraders: {}", self.upgraders.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Upgrade - Upgraders: {}", self.upgraders.len()))
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        //TODO: Limit upgraders to CONTROLLER_MAX_UPGRADE_PER_TICK total work parts at max level.

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let structures = room_data.get_structures().ok_or("Expected structure data")?;
        let creeps = room_data.get_creeps().ok_or("Expected creeps")?;
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;

        let controllers = structures.controllers();
        let storages = structures.storages();

        if !Self::can_run(room_data) {
            return Err("Upgrade room not owned by user".to_string());
        }

        let controller_level = controllers.iter().map(|c| c.level()).max().ok_or("Expected controller level")?;

        // K4 policy (ADR 0040 M3): the excess-energy threshold, roster cap, WORK sizing, body
        // shape and priority bands live in `screeps_econ_decision::spawn_policy` (consumed
        // here and by the economy sim); this mission keeps the ECS/roster bookkeeping.
        let has_excess_energy = {
            let storage_energy: u32 = structures
                .storages()
                .iter()
                .map(|storage| storage.store().get(ResourceType::Energy).unwrap_or(0))
                .sum();
            let container_energies: Vec<u32> = structures
                .containers()
                .iter()
                .map(|container| container.store().get(ResourceType::Energy).unwrap_or(0))
                .collect();
            screeps_econ_decision::spawn_policy::has_excess_energy(!structures.storages().is_empty(), storage_energy, &container_energies)
        };

        let are_hostile_creeps = !creeps.hostile().is_empty();

        // Detect downgrade risk at any RCL. When the downgrade timer falls
        // below half of max we spawn an upkeep upgrader at critical priority,
        // sized so that it can restore the timer back to the safe threshold
        // within a single creep lifetime (assuming a container of energy is
        // adjacent to the controller).
        let downgrade_upkeep_parts: Option<usize> = controllers
            .iter()
            .filter_map(|controller| {
                let max_ticks = controller_downgrade(controller.level())?;
                let ttd = controller.ticks_to_downgrade()?;
                if ttd < max_ticks / 2 {
                    Some(screeps_econ_decision::spawn_policy::work_parts_for_upkeep(ttd, max_ticks))
                } else {
                    None
                }
            })
            .max();

        let downgrade_risk = downgrade_upkeep_parts.is_some();

        let at_max_level = controller_levels(controller_level as u32).is_none();

        //TODO: Need better calculation for maximum number of upgraders.
        let max_upgraders = screeps_econ_decision::spawn_policy::max_upgraders(
            system_data.governor.can_execute_cpu(CpuBar::MediumPriority),
            are_hostile_creeps,
            at_max_level,
            has_excess_energy,
            controller_level,
        );

        let alive_upgraders = self
            .upgraders
            .iter()
            .filter(|entity| {
                system_data.creep_spawning.get(**entity).is_some()
                    || system_data
                        .creep_owner
                        .get(**entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live())
                        .map(|count| count > 100)
                        .unwrap_or(false)
            })
            .count();

        if alive_upgraders < max_upgraders {
            let work_parts_per_upgrader = screeps_econ_decision::spawn_policy::upgrader_work_parts(
                downgrade_upkeep_parts,
                self.upgraders.is_empty(),
                at_max_level,
                has_excess_energy,
                static_visibility_data.sources().len(),
                max_upgraders,
            );

            let maximum_energy = if self.upgraders.is_empty() && downgrade_risk {
                room.energy_available().max(SPAWN_ENERGY_CAPACITY)
            } else {
                room.energy_capacity_available()
            };

            let body_definition =
                screeps_econ_decision::spawn_policy::upgrader_body(controller_level, maximum_energy, work_parts_per_upgrader);

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let priority = screeps_econ_decision::spawn_policy::upgrader_priority(
                    downgrade_risk,
                    self.upgraders.is_empty(),
                    has_excess_energy,
                    !storages.is_empty(),
                    max_upgraders,
                    alive_upgraders,
                );

                let spawn_request = SpawnRequest::new(
                    "Upgrader".to_string(),
                    &body,
                    priority,
                    None,
                    Self::create_handle_upgrader_spawn(mission_entity, self.room_data),
                );

                system_data.spawn_queue.request(self.room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
