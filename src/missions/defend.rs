use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::serialize::*;
use crate::jobs::data::*;
use crate::jobs::defend::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use crate::spawnsystem::*;
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct DefendMission {
    room_data: Entity,
    defenders: EntityVec,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DefendMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = DefendMission::new(room_data);

        builder
            .with(MissionData::Defend(mission))
            .marked::<SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> DefendMission {
        DefendMission {
            room_data,
            defenders: EntityVec::new(),
        }
    }

    fn create_handle_defender_spawn(mission_entity: Entity, room_entity: Entity) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Defend(DefendJob::new(room_entity));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Defend(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.defenders.0.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for DefendMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui
                    .missions()
                    .add_text(format!("Defend - Archers: {}", self.defenders.0.len()), None);
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

        self.defenders
            .0
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

        let max_count = 2;

        if self.defenders.0.len() < max_count {
            let are_hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();
            let desired_count = if are_hostile_creeps { 2 } else { 0 };

            if self.defenders.0.len() < desired_count {
                let use_energy_max = if self.defenders.0.is_empty() {
                    room.energy_available()
                } else {
                    room.energy_capacity_available()
                };

                let body_definition = SpawnBodyDefinition {
                    maximum_energy: use_energy_max,
                    minimum_repeat: Some(1),
                    maximum_repeat: None,
                    pre_body: &[],
                    repeat_body: &[Part::RangedAttack, Part::Move],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let spawn_request = SpawnRequest::new(
                        "Defender".to_string(),
                        &body,
                        SPAWN_PRIORITY_MEDIUM,
                        Self::create_handle_defender_spawn(*runtime_data.entity, self.room_data),
                    );

                    runtime_data.spawn_queue.request(room_data.name, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
