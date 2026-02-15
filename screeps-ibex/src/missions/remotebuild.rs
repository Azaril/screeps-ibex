use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::creep::*;
use crate::jobs::build::*;
use crate::jobs::data::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use itertools::*;
use lerp::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct RemoteBuildMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RemoteBuildMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RemoteBuildMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::RemoteBuild(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> RemoteBuildMission {
        RemoteBuildMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            builders: EntityVec::new(),
        }
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    fn create_handle_builder_spawn(
        mission_entity: Entity,
        build_room_entity: Entity,
        allow_harvest: bool,
    ) -> crate::spawnsystem::SpawnQueueCallback {
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

    pub fn can_run(room_data: &RoomData) -> bool {
        let has_spawns = room_data
            .get_structures()
            .map(|structures| !structures.spawns().is_empty())
            .unwrap_or(false);

        if !has_spawns {
            if let Some(construction_sites) = room_data.get_construction_sites() {
                let has_pending_spawn = construction_sites
                    .iter()
                    .filter(|s| s.my())
                    .any(|s| s.structure_type() == StructureType::Spawn);

                return has_pending_spawn;
            }
        }

        false
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

    fn remove_creep(&mut self, entity: Entity) {
        self.builders.retain(|e| *e != entity);
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        let home_room_names = self
            .home_room_datas
            .iter()
            .filter_map(|e| system_data.room_data.get(*e))
            .map(|d| d.name.to_string())
            .join("/");

        format!("Remote Build - Builders: {} - Home rooms: {}", self.builders.len(), home_room_names)
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Remote Build - Builders: {}", self.builders.len()))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup home rooms that no longer exist.
        //

        self.home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for remote build mission".to_owned());
        }

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        if !Self::can_run(room_data) {
            return Ok(MissionResult::Success);
        }

        let desired_builders = 4;

        if self.builders.len() < desired_builders {
            let interp = (self.builders.len() as f32) / (desired_builders as f32);

            let priority = SPAWN_PRIORITY_MEDIUM.lerp_bounded(SPAWN_PRIORITY_LOW, interp);

            let token = system_data.spawn_queue.token();

            for home_room_entity in self.home_room_datas.iter() {
                let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

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
                        Some(token),
                        Self::create_handle_builder_spawn(mission_entity, self.room_data, true),
                    );

                    system_data.spawn_queue.request(*home_room_entity, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
