use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct UpgradeMission {
    upgraders: EntityVec,
}

impl UpgradeMission {
    pub fn build<B>(builder: B, room_name: RoomName) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = UpgradeMission::new();

        builder
            .with(MissionData::Upgrade(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> UpgradeMission {
        UpgradeMission {
            upgraders: EntityVec::new(),
        }
    }
}

impl Mission for UpgradeMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("Upgrade - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup upgraders that no longer exist.
        //

        self.upgraders
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));

        //TODO: Limit upgraders to 15 total work parts upgrading across all creeps.

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            if let Some(controller) = room.controller() {
                if controller.my() {
                    let max_upgraders = 3;

                    if self.upgraders.0.len() < max_upgraders {
                        let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32)
                            / (UPGRADE_CONTROLLER_POWER as f32);
                        let work_parts_per_upgrader =
                            (work_parts_per_tick / (max_upgraders as f32)).ceil() as usize;

                        let body_definition = crate::creep::SpawnBodyDefinition {
                            maximum_energy: room.energy_capacity_available(),
                            minimum_repeat: Some(1),
                            maximum_repeat: Some(work_parts_per_upgrader),
                            pre_body: &[],
                            repeat_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                            post_body: &[],
                        };

                        if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                            let mission_entity = *runtime_data.entity;
                            let controller_id = controller.id();

                            let priority = if self.upgraders.0.is_empty() {
                                SPAWN_PRIORITY_CRITICAL
                            } else {
                                SPAWN_PRIORITY_LOW
                            };

                            system_data.spawn_queue.request(SpawnRequest::new(
                                runtime_data.room_owner.owner,
                                &body,
                                priority,
                                Box::new(move |spawn_system_data, name| {
                                    let name = name.to_string();

                                    spawn_system_data.updater.exec_mut(move |world| {
                                        let creep_job = JobData::Upgrade(
                                            ::jobs::upgrade::UpgradeJob::new(&controller_id),
                                        );

                                        let creep_entity =
                                            ::creep::Spawning::build(world.create_entity(), &name)
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
                            ));
                        }
                    }

                    return MissionResult::Running;
                }
            }
        }

        MissionResult::Failure
    }
}
