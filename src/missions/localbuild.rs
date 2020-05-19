use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::build::*;
use crate::jobs::data::*;
use crate::jobs::utility::repair::*;
use crate::ownership::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct LocalBuildMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalBuildMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalBuildMission::new(owner, room_data);

        builder
            .with(MissionData::LocalBuild(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity) -> LocalBuildMission {
        LocalBuildMission {
            owner: owner.into(),
            room_data,
            builders: EntityVec::new(),
        }
    }

    fn get_builder_priority(&self, room: &Room, has_sufficient_energy: bool) -> Option<(u32, f32)> {
        let controller = room.controller()?;
        let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

        if !construction_sites.is_empty() {
            let required_progress: u32 = construction_sites
                .iter()
                .map(|construction_site| construction_site.progress_total() - construction_site.progress())
                .sum();

            let desired_builders_for_progress: u32 = if controller.level() <= 3 {
                match required_progress {
                    0 => 0,
                    1..=1000 => 1,
                    1001..=2000 => 2,
                    2001..=3000 => 3,
                    3001..=4000 => 4,
                    _ => 5,
                }
            } else if controller.level() <= 6 {
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
                    SPAWN_PRIORITY_HIGH
                } else {
                    construction_sites
                        .iter()
                        .map(|construction_site| match construction_site.structure_type() {
                            StructureType::Spawn => SPAWN_PRIORITY_HIGH,
                            StructureType::Storage => SPAWN_PRIORITY_HIGH,
                            _ => SPAWN_PRIORITY_MEDIUM,
                        })
                        .max_by(|a, b| a.partial_cmp(b).unwrap())
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

    fn get_repairer_priority(&self, room: &Room) -> Option<(u32, f32)> {
        //TODO: Not requiring full hashmap just to check for presence would be cheaper. Lazy iterator would be sufficient.
        let repair_targets = get_prioritized_repair_targets(&room, Some(RepairPriority::Medium), true);

        let has_priority = |priority| repair_targets.get(&priority).map(|s| !s.is_empty()).unwrap_or(false);

        if has_priority(RepairPriority::Critical) || has_priority(RepairPriority::High) {
            Some((1, SPAWN_PRIORITY_HIGH))
        } else if has_priority(RepairPriority::Medium) {
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
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let has_sufficient_energy = {
            if let Some(storage) = room.storage() {
                storage.store_of(ResourceType::Energy) >= 50_000
            } else {
                let structures = room.find(find::STRUCTURES);
                structures
                    .iter()
                    .filter_map(|structure| {
                        if let Structure::Container(container) = structure {
                            Some(container)
                        } else {
                            None
                        }
                    })
                    .any(|container| container.store_of(ResourceType::Energy) as f32 / CONTAINER_CAPACITY as f32 > 0.50)
            }
        };

        let mut spawn_count = 0;
        let mut spawn_priority = SPAWN_PRIORITY_NONE;

        if let Some((desired_builders, build_priority)) = self.get_builder_priority(&room, has_sufficient_energy) {
            spawn_count = spawn_count.max(desired_builders);
            spawn_priority = spawn_priority.max(build_priority);
        }

        if let Some((desired_repairers, repair_priority)) = self.get_repairer_priority(&room) {
            spawn_count = spawn_count.max(desired_repairers);
            spawn_priority = spawn_priority.max(repair_priority);
        }

        if self.builders.len() < spawn_count as usize {
            let use_energy_max = if self.builders.is_empty() && spawn_priority >= SPAWN_PRIORITY_HIGH {
                room.energy_available()
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
                    Self::create_handle_builder_spawn(mission_entity, self.room_data, allow_harvest),
                );

                system_data.spawn_queue.request(room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
