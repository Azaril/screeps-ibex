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

#[derive(Clone, ConvertSaveload)]
pub struct ScoutMission {
    room_data: Entity,
    home_room_data: Entity,
    scouts: EntityVec,
    next_spawn: Option<u32>,
    spawned_scouts: u32,
}

impl ScoutMission {
    pub fn build<B>(builder: B, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ScoutMission::new(room_data, home_room_data);

        builder.with(MissionData::Scout(mission)).marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity, home_room_data: Entity) -> ScoutMission {
        ScoutMission {
            room_data,
            home_room_data,
            scouts: EntityVec::new(),
            next_spawn: None,
            spawned_scouts: 0,
        }
    }

    fn create_handle_scout_spawn(
        mission_entity: Entity,
        scout_room: RoomName,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Scout(::jobs::scout::ScoutJob::new(scout_room));

                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Scout(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.scouts.0.push(creep_entity);

                    mission_data.spawned_scouts += 1;
                    mission_data.next_spawn = Some(std::cmp::min(mission_data.spawned_scouts * 2000, 10000));
                }
            });
        })
    }
}

impl Mission for ScoutMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text(
                    format!("Scout - Scouts: {}", self.scouts.0.len()),
                    None,
                );
            });
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup scouts that no longer exist.
        //

        self.scouts.0.retain(|entity| system_data.entities.is_alive(*entity));

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        scope_timing!("ScoutMission");

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let data_is_fresh = room_data
            .get_dynamic_visibility_data()
            .as_ref()
            .map(|v| v.updated_within(10))
            .unwrap_or(false);

        if data_is_fresh && self.scouts.0.is_empty() {
            info!(
                "Completing scout mission - room is visible and no active scouts. Room: {}",
                room_data.name
            );

            return Ok(MissionResult::Success);
        }

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        let should_spawn = self.next_spawn.map(|t| t >= game::time()).unwrap_or(true);

        if self.scouts.0.is_empty() && should_spawn {
            //TODO: Compute best body parts to use.
            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: Some(1),
                pre_body: &[],
                repeat_body: &[Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Scout - Target Room: {}", room_data.name),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Self::create_handle_scout_spawn(*runtime_data.entity, room_data.name),
                );

                runtime_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
