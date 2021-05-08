use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::jobs::data::*;
use crate::jobs::scout::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use super::constants::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use itertools::Itertools;

#[derive(ConvertSaveload)]
pub struct ScoutMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    priority: f32,
    scouts: EntityVec<Entity>,
    next_spawn: Option<u32>,
    spawned_scouts: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity], priority: f32) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ScoutMission::new(owner, room_data, home_room_datas, priority);

        builder
            .with(MissionData::Scout(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity], priority: f32) -> ScoutMission {
        ScoutMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            priority,
            scouts: EntityVec::new(),
            next_spawn: None,
            spawned_scouts: 0,
        }
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    pub fn get_priority(&self) -> f32 {
        self.priority
    }

    pub fn set_priority(&mut self, priority: f32) {
        self.priority = priority
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

                    let next_spawn_time =
                        std::cmp::min(mission_data.spawned_scouts * (CREEP_LIFE_TIME / 4), CREEP_LIFE_TIME * 3) + game::time();

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

        let home_room_names = self.home_room_datas.iter()
            .filter_map(|e| system_data.room_data.get(*e))        
            .map(|d| d.name.to_string())
            .join("/");

        format!(
            "Scout - Priority: {} - Scouts: {} - Next spawn: {} - Home Rooms: {}",
            self.priority,
            self.scouts.len(),
            next_spawn,
            home_room_names
        )
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup scouts that no longer exist.
        //

        self.scouts
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        //
        // Cleanup home rooms that no longer exist.
        //

        self.home_room_datas
            .retain(|entity| {
                system_data.room_data
                    .get(*entity)
                    .map(is_valid_home_room)
                    .unwrap_or(false)
            });

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for scout mission".to_owned());
        }

        let mid_point_coordinate = unsafe{ RoomCoordinate::unchecked_new(ROOM_SIZE / 2) };

        let home_room_positions = self.home_room_datas.iter()
            .filter_map(|e| system_data.room_data.get(*e))        
            .map(|room| Position::new(mid_point_coordinate, mid_point_coordinate, room.name));

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;            

        let target_position = Position::new(mid_point_coordinate, mid_point_coordinate, room_data.name);

        for home_room_position in home_room_positions {
            MapVisual::line(home_room_position, target_position, Some(LineStyle::default().color("Green")));
        }

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
            return Err(format!(
                "Failed scout mission - unable to scout room after {} attempts",
                self.spawned_scouts
            ));
        }

        let should_spawn = can_execute_cpu(CpuBar::LowPriority) && self.next_spawn.map(|t| game::time() >= t).unwrap_or(true);

        let token = system_data.spawn_queue.token();

        for home_room_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            if self.scouts.is_empty() && should_spawn {
                //TODO: Compute best body parts to use.
                let body_definition = crate::creep::SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: None,
                    maximum_repeat: None,
                    pre_body: &[Part::Move],
                    repeat_body: &[],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let priority = if self.priority >= VISIBILITY_PRIORITY_CRITICAL {
                        SPAWN_PRIORITY_HIGH
                    } else if self.priority >= VISIBILITY_PRIORITY_HIGH {
                        SPAWN_PRIORITY_MEDIUM
                    } else if self.priority >= VISIBILITY_PRIORITY_MEDIUM {
                        SPAWN_PRIORITY_LOW
                    } else {
                        SPAWN_PRIORITY_NONE
                    };

                    let spawn_request = SpawnRequest::new(
                        format!("Scout - Target Room: {}", room_data.name),
                        &body,
                        priority,
                        Some(token),
                        Self::create_handle_scout_spawn(mission_entity, room_data.name),
                    );

                    system_data.spawn_queue.request(*home_room_entity, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
