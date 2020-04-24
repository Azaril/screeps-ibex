use super::data::*;
use super::missionsystem::*;
use crate::jobs::claim::*;
use crate::jobs::data::*;
use crate::ownership::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, ConvertSaveload)]
pub struct ClaimMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
    home_room_data: Entity,
    claimers: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ClaimMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ClaimMission::new(owner, room_data, home_room_data);

        builder.with(MissionData::Claim(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity) -> ClaimMission {
        ClaimMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            claimers: EntityVec::new(),
        }
    }

    fn create_handle_claimer_spawn(
        mission_entity: Entity,
        controller_id: RemoteObjectId<StructureController>,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Claim(ClaimJob::new(controller_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Claim(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.claimers.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ClaimMission {
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
        format!("Claim - Claimers: {}", self.claimers.len())
    }

    fn pre_run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup claimers that no longer exist.
        //

        self.claimers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

        if dynamic_visibility_data.updated_within(1000) {
            match dynamic_visibility_data.owner() {
                RoomDisposition::Mine => {
                    return Ok(MissionResult::Success);
                }
                RoomDisposition::Friendly(_) | RoomDisposition::Hostile(_) => {
                    return Err("Room already owned".to_string());
                }
                RoomDisposition::Neutral => {}
            }

            match dynamic_visibility_data.reservation() {
                RoomDisposition::Mine | RoomDisposition::Neutral => {}
                RoomDisposition::Friendly(_) | RoomDisposition::Hostile(_) => {
                    return Err("Room already owned".to_string());
                }
            }
        }

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;
        let controller = static_visibility_data.controller().ok_or("Expected target controller")?;
        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        if self.claimers.is_empty() {
            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: None,
                maximum_repeat: None,
                pre_body: &[Part::Claim, Part::Move],
                repeat_body: &[],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    "Claimer".to_string(),
                    &body,
                    SPAWN_PRIORITY_MEDIUM,
                    Self::create_handle_claimer_spawn(runtime_data.entity, *controller),
                );

                system_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
