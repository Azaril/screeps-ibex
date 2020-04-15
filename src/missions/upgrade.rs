use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::jobs::upgrade::*;
use crate::transfer::transfersystem::*;
use crate::remoteobjectid::*;

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeMission {
    room_data: Entity,
    upgraders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UpgradeMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = UpgradeMission::new(room_data);

        builder.with(MissionData::Upgrade(mission)).marked::<SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> UpgradeMission {
        UpgradeMission {
            room_data,
            upgraders: EntityVec::new(),
        }
    }

    fn create_handle_upgrader_spawn(mission_entity: Entity, home_room: Entity, allow_harvest: bool) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Upgrade(UpgradeJob::new(home_room, allow_harvest));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Upgrade(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.upgraders.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for UpgradeMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui
                    .missions()
                    .add_text(format!("Upgrade - Upgraders: {}", self.upgraders.len()), None);
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

        self.upgraders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        //TODO: Limit upgraders to 15 total work parts upgrading across all creeps.

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let controller = room.controller().ok_or("Expected controller")?;

        if !controller.my() {
            return Err("Room not owned by user".to_string());
        }

        let has_excess_energy = {
            if let Some(room_transfer_data) = runtime_data.transfer_queue.try_get_room(room_data.name) {
                if let Some(storage) = room.storage() {
                    if let Some(storage_node) = room_transfer_data.try_get_node(&TransferTarget::Storage(storage.remote_id())) {
                        if storage_node.get_available_withdrawl_by_resource(TransferType::Haul, ResourceType::Energy) >= 100_000 {
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
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
                        .filter_map(|container| room_transfer_data.try_get_node(&TransferTarget::Container(container.remote_id())))
                        .any(|container_node| container_node.get_available_withdrawl_by_resource(TransferType::Haul, ResourceType::Energy) as f32 / CONTAINER_CAPACITY as f32 > 0.75)
                }
            } else {
                false
            }
        };

        let are_hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();

        //TODO: Need better calculation for maximum number of upgraders.
        let max_upgraders = if game::cpu::bucket() < game::cpu::tick_limit() * 10.0 {
            1
        } else if are_hostile_creeps {
            1
        } else if controller.level() >= 8 {
            1
        } else if has_excess_energy {
            if controller.level() <= 3 {
                5
            } else {
                3
            }
        } else {
            1
        };

        if self.upgraders.len() < max_upgraders {
            let storage_sufficient = room.storage().map(|s| s.store_used_capacity(Some(ResourceType::Energy)) > 50_000).unwrap_or(true);

            let work_parts_per_upgrader = if !storage_sufficient {
                Some(1)
            } else if controller.level() == 8 {
                let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);

                let work_parts = (work_parts_per_tick / (max_upgraders as f32)).ceil();

                Some(work_parts as usize)
            } else {
                None
            };

            let downgrade_risk = controller_downgrade(controller.level())
                .map(|ticks| controller.ticks_to_downgrade() < ticks / 2)
                .unwrap_or(false);

            let maximum_energy = if self.upgraders.is_empty() && downgrade_risk {
                room.energy_available()
            } else {
                room.energy_capacity_available()
            };

            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy,
                minimum_repeat: Some(1),
                maximum_repeat: work_parts_per_upgrader,
                pre_body: &[],
                repeat_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let priority = if self.upgraders.is_empty() && downgrade_risk {
                    SPAWN_PRIORITY_HIGH
                } else {
                    SPAWN_PRIORITY_LOW
                };

                let allow_harvest = controller.level() <= 3;

                let spawn_request = SpawnRequest::new(
                    "Upgrader".to_string(),
                    &body,
                    priority,
                    Self::create_handle_upgrader_spawn(*runtime_data.entity, self.room_data, allow_harvest),
                );

                runtime_data.spawn_queue.request(room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
