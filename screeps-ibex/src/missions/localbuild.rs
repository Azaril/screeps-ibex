use super::data::*;
use super::missionsystem::*;
use super::constants::*;
use crate::creep::*;
use crate::jobs::build::*;
use crate::jobs::data::*;
use crate::jobs::utility::repair::*;
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
pub struct LocalBuildMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalBuildMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalBuildMission::new(owner, room_data);

        builder
            .with(MissionData::LocalBuild(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> LocalBuildMission {
        LocalBuildMission {
            owner: owner.into(),
            room_data,
            builders: EntityVec::new(),
        }
    }

    fn get_builder_priority(&self, room_data: &RoomData, has_sufficient_energy: bool) -> Option<(u32, f32)> {
        let structures = room_data.get_structures()?;
        let controller_level = structures.controllers().iter().map(|c| c.level()).max().unwrap_or(0);
        let construction_sites = room_data.get_construction_sites()?;

        if !construction_sites.is_empty() {
            let required_progress: u32 = construction_sites
                .iter()
                .map(|construction_site| construction_site.progress_total() - construction_site.progress())
                .sum();

            let desired_builders_for_progress: u32 = if controller_level <= 3 {
                match required_progress {
                    0 => 0,
                    1..=1000 => 1,
                    1001..=2000 => 2,
                    2001..=3000 => 3,
                    3001..=4000 => 4,
                    _ => 5,
                }
            } else if controller_level <= 6 {
                match required_progress {
                    0 => 0,
                    1..=2000 => 1,
                    2001..=4000 => 2,
                    4001..=6000 => 3,
                    _ => 4,
                }
            } else {
                match required_progress {
                    0 => 0,
                    1..=3000 => 1,
                    3001..=6000 => 2,
                    6001..=9000 => 3,
                    _ => 4,
                }
            };

            let desired_builders = if has_sufficient_energy { desired_builders_for_progress } else { 1 };

            if desired_builders > 0 {
                let priority = if self.builders.is_empty() {
                    (SPAWN_PRIORITY_HIGH + SPAWN_PRIORITY_MEDIUM) / 2.0
                } else {
                    construction_sites
                        .iter()
                        .map(|construction_site| match construction_site.structure_type() {
                            StructureType::Spawn => SPAWN_PRIORITY_HIGH,
                            StructureType::Storage => SPAWN_PRIORITY_HIGH,
                            _ => SPAWN_PRIORITY_MEDIUM,
                        })
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap_or(SPAWN_PRIORITY_LOW)
                };

                Some((desired_builders, priority))
            } else {
                None
            }
        } else {
            None
        }
    }

    fn get_repairer_priority(&self, room_data: &RoomData) -> Option<(u32, f32)> {
        let (priority, _) = select_repair_structure_and_priority(room_data, None, true)?;

        if priority >= RepairPriority::High {
            Some((1, SPAWN_PRIORITY_HIGH))
        } else if priority >= RepairPriority::Medium {
            Some((1, SPAWN_PRIORITY_MEDIUM))
        } else {
            None
        }
    }

    fn create_handle_builder_spawn(
        mission_entity: Entity,
        room_entity: Entity,
        allow_harvest: bool,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Build(BuildJob::new(room_entity, room_entity, allow_harvest));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<LocalBuildMission>()
                {
                    mission_data.builders.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for LocalBuildMission {
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
        format!("Local Build - Builders: {}", self.builders.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.builders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data_storage = &*system_data.room_data;
        let room_data = room_data_storage.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let structure_data = room_data.get_structures().ok_or("Expected structure data")?;

        let desired_storage_energy = get_desired_storage_amount(ResourceType::Energy) / 4;

        let has_sufficient_energy = {
            if !structure_data.storages().is_empty() {
                structure_data
                    .storages()
                    .iter()
                    .any(|container| container.store().get(ResourceType::Energy).unwrap_or(0) >= desired_storage_energy)
            } else {
                structure_data
                    .containers()
                    .iter()
                    .any(|container| container.store().get(ResourceType::Energy).unwrap_or(0) as f32 / CONTAINER_CAPACITY as f32 > 0.50)
            }
        };

        let mut spawn_count = 0;
        let mut spawn_priority = SPAWN_PRIORITY_NONE;

        if let Some((desired_builders, build_priority)) = self.get_builder_priority(&room_data, has_sufficient_energy) {
            spawn_count = spawn_count.max(desired_builders);
            spawn_priority = spawn_priority.max(build_priority);
        }

        if let Some((desired_repairers, repair_priority)) = self.get_repairer_priority(&room_data) {
            spawn_count = spawn_count.max(desired_repairers);
            spawn_priority = spawn_priority.max(repair_priority);
        }

        if self.builders.len() < spawn_count as usize {
            let use_energy_max = if self.builders.is_empty() && spawn_priority >= SPAWN_PRIORITY_HIGH {
                room.energy_available().max(SPAWN_ENERGY_CAPACITY)
            } else {
                room.energy_capacity_available()
            };

            let max_body = if spawn_priority >= SPAWN_PRIORITY_HIGH { None } else { Some(5) };

            let body_definition = SpawnBodyDefinition {
                maximum_energy: use_energy_max,
                minimum_repeat: Some(1),
                maximum_repeat: max_body,
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Work, Part::Move, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let allow_harvest = room.storage().is_none();

                let spawn_request = SpawnRequest::new(
                    "Local Builder".to_string(),
                    &body,
                    spawn_priority,
                    None,
                    Self::create_handle_builder_spawn(mission_entity, self.room_data, allow_harvest),
                );

                system_data.spawn_queue.request(self.room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
