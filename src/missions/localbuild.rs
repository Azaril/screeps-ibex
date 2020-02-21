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
}

impl Mission for LocalBuildMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("LocalBuildMission");

        //
        // Cleanup builders that no longer exist.
        //

        self.builders
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(room) = game::rooms::get(room_data.name) {
                let builder_priority = if self.builders.0.len() < 2 {
                    if !room.find(find::MY_CONSTRUCTION_SITES).is_empty() {
                        if self.builders.0.is_empty() {
                            Some(SPAWN_PRIORITY_HIGH)
                        } else {
                            Some(SPAWN_PRIORITY_MEDIUM)
                        }
                    } else {
                        let repair_targets = RepairUtility::get_prioritized_repair_targets(&room);
                        let repair_priorities = [
                            RepairPriority::Critical,
                            RepairPriority::High,
                            RepairPriority::Medium,
                        ];

                        let mut has_repair_target = false;

                        for priority in repair_priorities.iter() {
                            if let Some(structures) = repair_targets.get(priority) {
                                if !structures.is_empty() {
                                    has_repair_target = true;
                                }
                            }
                        }

                        if has_repair_target {
                            Some(SPAWN_PRIORITY_HIGH)
                        } else {
                            None
                        }
                    }
                } else {
                    None
                };

                if let Some(priority) = builder_priority {
                    let use_energy_max =
                        if self.builders.0.is_empty() && priority >= SPAWN_PRIORITY_HIGH {
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
                        let mission_entity = *runtime_data.entity;
                        let room_name = room_data.name;

                        system_data.spawn_queue.request(SpawnRequest::new(
                            room_data.name,
                            &body,
                            priority,
                            Box::new(move |spawn_system_data, name| {
                                let name = name.to_string();

                                spawn_system_data.updater.exec_mut(move |world| {
                                    let creep_job = JobData::Build(::jobs::build::BuildJob::new(
                                        room_name, room_name,
                                    ));

                                    let creep_entity =
                                        ::creep::Spawning::build(world.create_entity(), &name)
                                            .with(creep_job)
                                            .build();

                                    let mission_data_storage =
                                        &mut world.write_storage::<MissionData>();

                                    if let Some(MissionData::LocalBuild(mission_data)) =
                                        mission_data_storage.get_mut(mission_entity)
                                    {
                                        mission_data.builders.0.push(creep_entity);
                                    }
                                });
                            }),
                        ));
                    }
                }

                MissionResult::Running
            } else {
                MissionResult::Failure
            }
        } else {
            MissionResult::Failure
        }
    }
}
