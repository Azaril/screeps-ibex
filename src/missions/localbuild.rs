use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::utility::repair::*;
use crate::serialize::*;
use jobs::data::*;
use spawnsystem::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct LocalBuildMission {
    room_data: Entity,
    builders: EntityVec,
}

impl LocalBuildMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalBuildMission::new(room_data);

        builder
            .with(MissionData::LocalBuild(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> LocalBuildMission {
        LocalBuildMission {
            room_data,
            builders: EntityVec::new(),
        }
    }

    fn get_builder_priority(&self, room: &Room) -> Option<f32> {
        if !room.find(find::MY_CONSTRUCTION_SITES).is_empty() {
            if self.builders.0.is_empty() {
                Some(SPAWN_PRIORITY_HIGH)
            } else {
                Some(SPAWN_PRIORITY_MEDIUM)
            }
        } else {
            //TODO: Not requiring full hashmap just to check for presence would be cheaper. Lazy iterator would be sufficient.
            let has_repair_target = !get_prioritized_repair_targets(&room, Some(RepairPriority::Medium)).is_empty();

            if has_repair_target {
                Some(SPAWN_PRIORITY_HIGH)
            } else {
                None
            }
        }
    }

    fn create_handle_builder_spawn(
        mission_entity: Entity,
        room_entity: Entity,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Build(::jobs::build::BuildJob::new(room_entity, room_entity));

                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::LocalBuild(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.builders.0.push(creep_entity);
                }
            });
        })
    }
}

impl Mission for LocalBuildMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text(
                    format!("Local Build - Builders: {}", self.builders.0.len()),
                    None,
                );
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

        self.builders.0.retain(|entity| system_data.entities.is_alive(*entity));

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        scope_timing!("LocalBuildMission");

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let max_count = 2;

        if self.builders.0.len() < max_count {
            if let Some(priority) = self.get_builder_priority(&room) {
                let desired_count = if priority >= SPAWN_PRIORITY_MEDIUM { max_count } else { 1 };
                if self.builders.0.len() < desired_count {
                    let use_energy_max = if self.builders.0.is_empty() && priority >= SPAWN_PRIORITY_HIGH {
                        room.energy_available()
                    } else {
                        room.energy_capacity_available()
                    };

                    let body_definition = SpawnBodyDefinition {
                        maximum_energy: use_energy_max,
                        minimum_repeat: Some(1),
                        maximum_repeat: None,
                        pre_body: &[],
                        repeat_body: &[Part::Carry, Part::Work, Part::Move, Part::Move],
                        post_body: &[],
                    };

                    if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                        let spawn_request = SpawnRequest::new(
                            "Local Builder".to_string(),
                            &body,
                            priority,
                            Self::create_handle_builder_spawn(*runtime_data.entity, self.room_data),
                        );

                        runtime_data.spawn_queue.request(room_data.name, spawn_request);
                    }
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
