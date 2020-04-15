use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::utility::repair::*;
use crate::serialize::*;
use crate::jobs::data::*;
use crate::jobs::build::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use crate::spawnsystem::*;
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::transfer::transfersystem::*;
use crate::remoteobjectid::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct LocalBuildMission {
    room_data: Entity,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalBuildMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalBuildMission::new(room_data);

        builder
            .with(MissionData::LocalBuild(mission))
            .marked::<SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> LocalBuildMission {
        LocalBuildMission {
            room_data,
            builders: EntityVec::new(),
        }
    }

    fn get_builder_priority(&self, room: &Room) -> Option<f32> {
        let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

        if !construction_sites.is_empty() {
            if self.builders.is_empty() {
                Some(SPAWN_PRIORITY_HIGH)
            } else {
                construction_sites.iter().map(|construction_site| {
                    match construction_site.structure_type() {
                        StructureType::Spawn => SPAWN_PRIORITY_HIGH,
                        StructureType::Storage => SPAWN_PRIORITY_HIGH,
                        _ => SPAWN_PRIORITY_MEDIUM
                    }
                }).max_by(|a, b| a.partial_cmp(b).unwrap())
            }
        } else {
            //TODO: Not requiring full hashmap just to check for presence would be cheaper. Lazy iterator would be sufficient.
            let repair_targets = get_prioritized_repair_targets(&room, Some(RepairPriority::Medium), true);

            let has_priority = |priority| repair_targets.get(&priority).map(|s| !s.is_empty()).unwrap_or(false);

            if has_priority(RepairPriority::Critical) || has_priority(RepairPriority::High) {
                Some(SPAWN_PRIORITY_HIGH)
            } else if has_priority(RepairPriority::Medium) {
                Some(SPAWN_PRIORITY_MEDIUM)
            } else {
                None
            }
        }
    }

    fn create_handle_builder_spawn(mission_entity: Entity, room_entity: Entity, allow_harvest: bool) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Build(BuildJob::new(room_entity, room_entity, allow_harvest));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::LocalBuild(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.builders.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for LocalBuildMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui
                    .missions()
                    .add_text(format!("Local Build - Builders: {}", self.builders.len()), None);
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.builders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let controller = room.controller().ok_or("Expected controller")?;

        let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);
        let required_progress: u32 = construction_sites.iter().map(|construction_site| construction_site.progress_total() - construction_site.progress()).sum();
        
        let desired_builders_for_progress = if controller.level() <= 3 {
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

        let has_sufficient_energy = {
            if let Some(room_transfer_data) = runtime_data.transfer_queue.try_get_room(room_data.name) {
                if let Some(storage) = room.storage() {
                    if let Some(storage_node) = room_transfer_data.try_get_node(&TransferTarget::Storage(storage.remote_id())) {
                        if storage_node.get_available_withdrawl_by_resource(TransferType::Haul, ResourceType::Energy) >= 50_000 {
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    let structures = room.find(find::STRUCTURES);                       
                    let containers: Vec<_> = structures
                        .iter()
                        .filter_map(|structure| {
                            if let Structure::Container(container) = structure { 
                                Some(container) 
                            } else { 
                                None 
                            }
                        })
                        .filter_map(|container| room_transfer_data.try_get_node(&TransferTarget::Container(container.remote_id())))
                        .collect();

                    if !containers.is_empty() {
                        containers.iter().any(|container_node| container_node.get_available_withdrawl_by_resource(TransferType::Haul, ResourceType::Energy) as f32 / CONTAINER_CAPACITY as f32 > 0.50)
                    } else {
                        true
                    }
                }
            } else {
                false
            }
        };

        let desired_builders = if has_sufficient_energy { desired_builders_for_progress } else { 1 };

        if self.builders.len() < desired_builders {
            if let Some(priority) = self.get_builder_priority(&room) {
                let use_energy_max = if self.builders.is_empty() && priority >= SPAWN_PRIORITY_HIGH {
                    room.energy_available()
                } else {
                    room.energy_capacity_available()
                };

                let max_body = if priority >= SPAWN_PRIORITY_HIGH { 
                    None
                } else {
                    Some(5)
                };

                let body_definition = SpawnBodyDefinition {
                    maximum_energy: use_energy_max,
                    minimum_repeat: Some(1),
                    maximum_repeat: max_body,
                    pre_body: &[],
                    repeat_body: &[Part::Carry, Part::Work, Part::Move, Part::Move],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let allow_harvest = room.controller().map(|c| c.level() <= 3).unwrap_or(false);

                    let spawn_request = SpawnRequest::new(
                        "Local Builder".to_string(),
                        &body,
                        priority,
                        Self::create_handle_builder_spawn(*runtime_data.entity, self.room_data, allow_harvest),
                    );

                    runtime_data.spawn_queue.request(room_data.name, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
