use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::upgrade::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use lerp::*;
use screeps::*;
use serde::{Deserialize, Serialize};
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

    fn create_handle_upgrader_spawn(
        mission_entity: Entity,
        home_room: Entity,
        allow_harvest: bool,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Upgrade(UpgradeJob::new(home_room, allow_harvest));

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

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Upgrade - Upgraders: {}", self.upgraders.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.upgraders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        //TODO: Limit upgraders to 15 total work parts upgrading across all creeps at RCL 8.

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let structures = room_data.get_structures().ok_or("Expected structure data")?;
        let creeps = room_data.get_creeps().ok_or("Expected creeps")?;
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;

        let controllers = structures.controllers();

        if !Self::can_run(&room_data) {
            return Err("Upgrade room not owned by user".to_string());
        }

        let controller_level = controllers.iter().map(|c| c.level()).max().ok_or("Expected controller level")?;

        let has_excess_energy = {
            if !structures.storages().is_empty() {
                structures
                    .storages()
                    .iter()
                    .any(|container| container.store_of(ResourceType::Energy) >= 100_000)
            } else if !structures.containers().is_empty() {
                structures
                    .containers()
                    .iter()
                    .any(|container| container.store_of(ResourceType::Energy) as f32 / CONTAINER_CAPACITY as f32 > 0.75)
            } else {
                true
            }
        };

        let are_hostile_creeps = !creeps.hostile().is_empty();

        //TODO: Need better calculation for maximum number of upgraders.
        let max_upgraders = if game::cpu::bucket() < game::cpu::tick_limit() * 2 {
            1
        } else if are_hostile_creeps {
            1
        } else if controller_level >= 8 {
            1
        } else if has_excess_energy {
            if controller_level <= 3 {
                5
            } else {
                3
            }
        } else {
            1
        };

        let alive_upgraders = self
            .upgraders
            .iter()
            .filter(|entity| {
                system_data.creep_spawning.get(**entity).is_some()
                    || system_data
                        .creep_owner
                        .get(**entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live().ok())
                        .map(|count| count > 100)
                        .unwrap_or(false)
            })
            .count();

        if alive_upgraders < max_upgraders {
            let work_parts_per_upgrader = if controller_level == 8 {
                let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);

                let work_parts = (work_parts_per_tick / (max_upgraders as f32)).ceil();

                Some(work_parts as usize)
            } else if has_excess_energy {
                Some(10)
            } else {
                let sources = static_visibility_data.sources();

                let energy_per_second = ((SOURCE_ENERGY_CAPACITY * sources.len() as u32) as f32) / (ENERGY_REGEN_TIME as f32);
                let upgrade_per_second = energy_per_second / (UPGRADE_CONTROLLER_POWER as f32);

                let parts_per_upgrader = ((upgrade_per_second / 2.0) / max_upgraders as f32).floor().max(1.0) as usize;

                Some(parts_per_upgrader)
            };

            let downgrade_risk = controllers
                .iter()
                .filter_map(|controller| controller_downgrade(controller.level()).map(|ticks| controller.ticks_to_downgrade() < ticks / 2))
                .any(|risk| risk);

            let maximum_energy = if self.upgraders.is_empty() && downgrade_risk {
                room.energy_available().max(SPAWN_ENERGY_CAPACITY)
            } else {
                room.energy_capacity_available()
            };

            let body_definition = if controller_level <= 3 {
                crate::creep::SpawnBodyDefinition {
                    maximum_energy,
                    minimum_repeat: Some(1),
                    maximum_repeat: work_parts_per_upgrader,
                    pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                    repeat_body: &[Part::Work, Part::Move],
                    post_body: &[],
                }
            } else {
                crate::creep::SpawnBodyDefinition {
                    maximum_energy,
                    minimum_repeat: Some(1),
                    maximum_repeat: work_parts_per_upgrader.map(|p| p - 1),
                    pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                    repeat_body: &[Part::Work],
                    post_body: &[],
                }
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let priority = if self.upgraders.is_empty() && (downgrade_risk || controller_level <= 1) {
                    SPAWN_PRIORITY_HIGH
                } else if self.upgraders.is_empty() {
                    SPAWN_PRIORITY_HIGH
                } else {
                    let interp = (alive_upgraders as f32) / (max_upgraders as f32);

                    SPAWN_PRIORITY_MEDIUM.lerp_bounded(SPAWN_PRIORITY_LOW, interp)
                };

                let allow_harvest = controller_level <= 3;

                let spawn_request = SpawnRequest::new(
                    "Upgrader".to_string(),
                    &body,
                    priority,
                    None,
                    Self::create_handle_upgrader_spawn(mission_entity, self.room_data, allow_harvest),
                );

                system_data.spawn_queue.request(self.room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
