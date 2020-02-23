use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::spawnsystem::*;

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeMission {
    room_data: Entity,
    upgraders: EntityVec,
}

impl UpgradeMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = UpgradeMission::new(room_data);

        builder
            .with(MissionData::Upgrade(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> UpgradeMission {
        UpgradeMission {
            room_data,
            upgraders: EntityVec::new(),
        }
    }
}

impl Mission for UpgradeMission {
    fn describe(
        &mut self,
        system_data: &MissionExecutionSystemData,
        describe_data: &mut MissionDescribeData,
    ) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text("Upgrade".to_string(), None);
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) {
        //
        // Cleanup scouts that no longer exist.
        //

        self.upgraders
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("UpgradeMission");

        //TODO: Limit upgraders to 15 total work parts upgrading across all creeps.

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(room) = game::rooms::get(room_data.name) {
                if let Some(controller) = room.controller() {
                    if controller.my() {
                        let max_upgraders = 3;

                        if self.upgraders.0.len() < max_upgraders {
                            let work_parts_per_upgrader = if controller.level() == 8 {
                                let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32)
                                    / (UPGRADE_CONTROLLER_POWER as f32);

                                let work_parts =
                                    (work_parts_per_tick / (max_upgraders as f32)).ceil();

                                Some(work_parts as usize)
                            } else {
                                None
                            };

                            let body_definition = crate::creep::SpawnBodyDefinition {
                                maximum_energy: if self.upgraders.0.is_empty() {
                                    room.energy_available()
                                } else {
                                    room.energy_capacity_available()
                                },
                                minimum_repeat: Some(1),
                                maximum_repeat: work_parts_per_upgrader,
                                pre_body: &[],
                                repeat_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                                post_body: &[],
                            };

                            if let Ok(body) = crate::creep::Spawning::create_body(&body_definition)
                            {
                                let mission_entity = *runtime_data.entity;
                                let controller_id = controller.remote_id();
                                let home_room = room_data.name;

                                let priority = if self.upgraders.0.is_empty() {
                                    SPAWN_PRIORITY_CRITICAL
                                } else {
                                    SPAWN_PRIORITY_LOW
                                };

                                runtime_data.spawn_queue.request(
                                    room_data.name,
                                    SpawnRequest::new(
                                        "Upgrader".to_string(),
                                        &body,
                                        priority,
                                        Box::new(move |spawn_system_data, name| {
                                            let name = name.to_string();

                                            spawn_system_data.updater.exec_mut(move |world| {
                                                let creep_job = JobData::Upgrade(
                                                    ::jobs::upgrade::UpgradeJob::new(
                                                        &controller_id,
                                                        home_room,
                                                    ),
                                                );

                                                let creep_entity = ::creep::Spawning::build(
                                                    world.create_entity(),
                                                    &name,
                                                )
                                                .with(creep_job)
                                                .build();

                                                let mission_data_storage =
                                                    &mut world.write_storage::<MissionData>();

                                                if let Some(MissionData::Upgrade(mission_data)) =
                                                    mission_data_storage.get_mut(mission_entity)
                                                {
                                                    mission_data.upgraders.0.push(creep_entity);
                                                }
                                            });
                                        }),
                                    ),
                                );
                            }
                        }

                        return MissionResult::Running;
                    }
                }
            }
        }

        MissionResult::Failure
    }
}
