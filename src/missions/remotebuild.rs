use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
#[cfg(feature = "time")]
use timing_annotate::*;

use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::serialize::*;
use jobs::data::*;
use spawnsystem::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct RemoteBuildMission {
    room_data: Entity,
    home_room_data: Entity,
    builders: EntityVec,
}

#[cfg_attr(feature = "time", timing)]
impl RemoteBuildMission {
    pub fn build<B>(builder: B, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RemoteBuildMission::new(room_data, home_room_data);

        builder
            .with(MissionData::RemoteBuild(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity, home_room_data: Entity) -> RemoteBuildMission {
        RemoteBuildMission {
            room_data,
            home_room_data,
            builders: EntityVec::new(),
        }
    }

    fn create_handle_builder_spawn(
        mission_entity: Entity,
        build_room_entity: Entity,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Build(::jobs::build::BuildJob::new(
                    //TODO: Pass an array of home rooms - allow for hauling energy if harvesting is not possible.
                    build_room_entity,
                    build_room_entity,
                ));

                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::RemoteBuild(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.builders.0.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "time", timing)]
impl Mission for RemoteBuildMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text(
                    format!("Remote Build - Builders: {}", self.builders.0.len()),
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

        self.builders.0.retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        if let Some(room) = game::rooms::get(room_data.name) {
            if room.find(find::MY_CONSTRUCTION_SITES).is_empty() {
                return Ok(MissionResult::Success);
            }
        }

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        if self.builders.0.len() < 2 {
            let priority = if self.builders.0.is_empty() {
                SPAWN_PRIORITY_MEDIUM
            } else {
                SPAWN_PRIORITY_LOW
            };

            let body_definition = SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: None,
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Work, Part::Move, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Remote Builder - Target Room: {}", room_data.name),
                    &body,
                    priority,
                    Self::create_handle_builder_spawn(*runtime_data.entity, self.room_data),
                );

                runtime_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
