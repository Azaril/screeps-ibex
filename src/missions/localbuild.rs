use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};

use super::data::*;
use super::missionsystem::*;
use ::jobs::data::*;
use ::spawnsystem::*;
use crate::serialize::*;
use crate::jobs::utility::repair::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct LocalBuildMission {
    builders: EntityVec
}

impl LocalBuildMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = LocalBuildMission::new();

        builder.with(MissionData::LocalBuild(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> LocalBuildMission {
        LocalBuildMission {
            builders: EntityVec::new()
        }
    }
}

impl Mission for LocalBuildMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("LocalBuildMission - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup builders that no longer exist.
        //

        self.builders.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            let builder_priority = if self.builders.0.len() < 2 {
                if room.find(find::MY_CONSTRUCTION_SITES).len() > 0 {
                    if self.builders.0.len() == 0 { 
                        Some(SPAWN_PRIORITY_HIGH)
                    } else { 
                        Some(SPAWN_PRIORITY_MEDIUM) 
                    }
                } else {
                    let repair_targets = RepairUtility::get_prioritized_repair_targets(&room);
                    let repair_priorities = [RepairPriority::Critical, RepairPriority::High, RepairPriority::Medium];

                    let mut has_repair_target = false;

                    for priority in repair_priorities.iter() {
                        if let Some(structures) = repair_targets.get(priority) {
                            if structures.len() > 0 {
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
                let base_body = &[Part::Carry, Part::Work, Part::Move, Part::Move];
                let base_body_cost: u32 = base_body.iter().map(|p| p.cost()).sum();

                let repeat_body = &[Part::Carry, Part::Work, Part::Move, Part::Move];
                let repeat_body_cost: u32 = repeat_body.iter().map(|p| p.cost()).sum();

                let builder_count = self.builders.0.len();
                let use_energy_max = if builder_count == 0 && priority >= SPAWN_PRIORITY_HIGH { 
                    room.energy_available() 
                } else { 
                    room.energy_capacity_available() 
                };
                
                if base_body_cost <= use_energy_max {
                    let remaining_available_energy: u32 = use_energy_max - base_body_cost;
                    let max_repeat_parts = (remaining_available_energy as f32) / (repeat_body_cost as f32);

                    let spawn_work_parts = max_repeat_parts.floor() as usize;
                    
                    let body = repeat_body
                        .iter()
                        .cycle()
                        .take(spawn_work_parts * repeat_body.len())
                        .chain(base_body.iter())
                        .cloned()
                        .collect::<Vec<Part>>();

                    let mission_entity = runtime_data.entity.clone();
                    let room_name = room.name();

                    system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                        let name = name.to_string();

                        spawn_system_data.updater.exec_mut(move |world| {
                            let creep_job = JobData::Build(::jobs::build::BuildJob::new(&room_name));

                            let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                                .with(creep_job)
                                .build();

                            let mission_data_storage = &mut world.write_storage::<MissionData>();

                            if let Some(MissionData::LocalBuild(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                                mission_data.builders.0.push(creep_entity);
                            }       
                        });
                    })));
                }
            }

            return MissionResult::Running;
        } else {
            return MissionResult::Failure;
        }
    }
}