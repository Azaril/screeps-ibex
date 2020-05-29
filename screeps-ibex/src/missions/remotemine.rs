use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::harvest::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct RemoteMineMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_data: Entity,
    harvesters: EntityVec<Entity>,
    allow_spawning: bool,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RemoteMineMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RemoteMineMission::new(owner, room_data, home_room_data);

        builder
            .with(MissionData::RemoteMine(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> RemoteMineMission {
        RemoteMineMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            harvesters: EntityVec::new(),
            allow_spawning: true,
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow
    }

    fn create_handle_harvester_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        delivery_room: Entity,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Harvest(HarvestJob::new(source_id, delivery_room, false));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<RemoteMineMission>()
                {
                    mission_data.harvesters.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for RemoteMineMission {
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
        format!("Remote Mine - Harvesters: {}", self.harvesters.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.harvesters
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

        if dynamic_visibility_data.updated_within(1000)
            && (!dynamic_visibility_data.owner().neutral()
                || dynamic_visibility_data.reservation().hostile()
                || dynamic_visibility_data.reservation().friendly()
                || dynamic_visibility_data.source_keeper())
        {
            return Err("Room is owned or reserved".to_string());
        }

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        //TODO: Add better dynamic cpu adaptation.
        let bucket = game::cpu::bucket();
        let can_spawn = bucket > 9000.0 && crate::features::remote_mine::harvest() && self.allow_spawning;

        if !can_spawn {
            return Ok(MissionResult::Running);
        }

        //TODO: Store this mapping data as part of the mission. (Blocked on specs collection serialization.)
        let mut sources_to_harvesters = self
            .harvesters
            .iter()
            .filter_map(|harvester_entity| {
                if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                    Some((harvester_data.harvest_target().id(), harvester_entity))
                } else {
                    None
                }
            })
            .into_group_map();

        for source in static_visibility_data.sources().iter() {
            let source_id = source.id();

            let source_harvesters = sources_to_harvesters.remove(&source_id).unwrap_or_else(Vec::new);

            //
            // Spawn harvesters
            //

            //TODO: Compute correct number of harvesters to use for source.
            let current_harvesters = source_harvesters.len();
            let desired_harvesters = 2;

            if current_harvesters < desired_harvesters {
                //TODO: Compute best body parts to use.
                let body_definition = crate::creep::SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: Some(1),
                    maximum_repeat: None,
                    pre_body: &[],
                    repeat_body: &[Part::Move, Part::Move, Part::Carry, Part::Work],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let room_offset_distance = home_room_data.name - source.pos().room_name();
                    let room_manhattan_distance = room_offset_distance.0.abs() + room_offset_distance.1.abs();

                    let priority_range = if room_manhattan_distance <= 1 {
                        (SPAWN_PRIORITY_MEDIUM, SPAWN_PRIORITY_LOW)
                    } else {
                        (SPAWN_PRIORITY_LOW, SPAWN_PRIORITY_NONE)
                    };

                    let interp = (current_harvesters as f32) / (desired_harvesters as f32);
                    let priority = (priority_range.0 + priority_range.1) * interp;

                    let spawn_request = SpawnRequest::new(
                        format!("Remote Mine - Target Room: {}", room_data.name),
                        &body,
                        priority,
                        Self::create_handle_harvester_spawn(mission_entity, *source, self.home_room_data),
                    );

                    system_data.spawn_queue.request(self.home_room_data, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}