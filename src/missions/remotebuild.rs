use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::build::*;
use crate::jobs::data::*;
use crate::ownership::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, ConvertSaveload)]
pub struct RemoteBuildMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
    home_room_data: Entity,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RemoteBuildMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RemoteBuildMission::new(owner, room_data, home_room_data);

        builder.with(MissionData::RemoteBuild(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity) -> RemoteBuildMission {
        RemoteBuildMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            builders: EntityVec::new(),
        }
    }

    fn create_handle_builder_spawn(
        mission_entity: Entity,
        build_room_entity: Entity,
        allow_harvest: bool,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Build(BuildJob::new(
                    //TODO: Pass an array of home rooms - allow for hauling energy if harvesting is not possible.
                    build_room_entity,
                    build_room_entity,
                    allow_harvest,
                ));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::RemoteBuild(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.builders.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for RemoteBuildMission {
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

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _describe_data: &mut MissionDescribeData) -> String {
        format!("Remote Build - Builders: {}", self.builders.len())
    }

    fn pre_run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
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
        system_data: &mut MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        if let Some(room) = game::rooms::get(room_data.name) {
            let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

            if construction_sites.is_empty() {
                return Ok(MissionResult::Success);
            }

            if !construction_sites.iter().any(|c| c.structure_type() == StructureType::Spawn) {
                return Ok(MissionResult::Success);
            }
        }

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;
        let home_room_controller = home_room.controller().ok_or("Expected controller")?;

        let desired_builders = if home_room_controller.level() <= 3 { 4 } else { 2 };

        if self.builders.len() < desired_builders {
            let priority = if self.builders.is_empty() {
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

            //TODO: Pass in home room if in close proximity (i.e. adjacent) to allow hauling from storage.
            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Remote Builder - Target Room: {}", room_data.name),
                    &body,
                    priority,
                    Self::create_handle_builder_spawn(runtime_data.entity, self.room_data, true),
                );

                system_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
