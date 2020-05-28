use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::build::*;
use crate::jobs::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct RemoteBuildMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_data: Entity,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RemoteBuildMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RemoteBuildMission::new(owner, room_data, home_room_data);

        builder
            .with(MissionData::RemoteBuild(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> RemoteBuildMission {
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

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<RemoteBuildMission>()
                {
                    mission_data.builders.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for RemoteBuildMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Remote Build - Builders: {}", self.builders.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.builders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
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
                (SPAWN_PRIORITY_HIGH + SPAWN_PRIORITY_MEDIUM) / 2.0
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
                    Self::create_handle_builder_spawn(mission_entity, self.room_data, true),
                );

                system_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
