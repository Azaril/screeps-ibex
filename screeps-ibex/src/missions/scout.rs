use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::scout::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct ScoutMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_data: Entity,
    scouts: EntityVec<Entity>,
    next_spawn: Option<u32>,
    spawned_scouts: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ScoutMission::new(owner, room_data, home_room_data);

        builder
            .with(MissionData::Scout(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> ScoutMission {
        ScoutMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            scouts: EntityVec::new(),
            next_spawn: None,
            spawned_scouts: 0,
        }
    }

    fn create_handle_scout_spawn(mission_entity: Entity, scout_room: RoomName) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Scout(ScoutJob::new(scout_room));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<ScoutMission>()
                {
                    mission_data.scouts.push(creep_entity);

                    mission_data.spawned_scouts += 1;

                    let next_spawn_time = std::cmp::min(mission_data.spawned_scouts * (CREEP_LIFE_TIME / 4), CREEP_LIFE_TIME * 3) + game::time();

                    mission_data.next_spawn = Some(next_spawn_time);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ScoutMission {
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

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        let next_spawn = self
            .next_spawn
            .map(|ready_time| {
                let time = game::time();

                if time >= ready_time {
                    0
                } else {
                    ready_time - time
                }
            })
            .unwrap_or(0);

        let home_room_name = system_data.room_data.get(self.home_room_data).map(|d| d.name.to_string()).unwrap_or_else(|| "Unknown".to_owned());

        format!("Scout - Scouts: {} - Home Room: {} - Next spawn: {}", self.scouts.len(), home_room_name, next_spawn)
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup scouts that no longer exist.
        //

        self.scouts
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let data_is_fresh = room_data
            .get_dynamic_visibility_data()
            .as_ref()
            .map(|v| v.updated_within(10))
            .unwrap_or(false);

        if data_is_fresh && self.scouts.is_empty() {
            info!(
                "Completing scout mission - room is visible and no active scouts. Room: {}",
                room_data.name
            );

            return Ok(MissionResult::Success);
        }

        if self.spawned_scouts >= 4 && self.scouts.is_empty() {
            return Err(format!("Failed scout mission - unable to scout room after {} attempts", self.spawned_scouts));
        }

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        let should_spawn = self.next_spawn.map(|t| game::time() >= t).unwrap_or(true) && game::cpu::bucket() > 5000.0;

        if self.scouts.is_empty() && should_spawn {
            //TODO: Compute best body parts to use.
            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: Some(1),
                pre_body: &[],
                repeat_body: &[Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Scout - Target Room: {}", room_data.name),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Self::create_handle_scout_spawn(mission_entity, room_data.name),
                );

                system_data.spawn_queue.request(self.home_room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
