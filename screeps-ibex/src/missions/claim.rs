use super::data::*;
use super::missionsystem::*;
use crate::jobs::claim::*;
use crate::jobs::data::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct ClaimMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    claimers: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ClaimMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ClaimMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::Claim(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> ClaimMission {
        ClaimMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.into(),
            claimers: EntityVec::new(),
        }
    }

    fn create_handle_claimer_spawn(mission_entity: Entity, controller_id: RemoteObjectId<StructureController>) -> SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Claim(ClaimJob::new(controller_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<ClaimMission>()
                {
                    mission_data.claimers.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ClaimMission {
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
        let home_room_names = self
            .home_room_datas
            .iter()
            .filter_map(|e| system_data.room_data.get(*e))
            .map(|d| d.name.to_string())
            .join("/");

        format!("Claim - Claimers: {} - Home rooms: {}", self.claimers.len(), home_room_names)
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Claim - Claimers: {} - Homes: {}",
            self.claimers.len(),
            self.home_room_datas.len()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup claimers that no longer exist.
        //

        self.claimers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
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

        let token = system_data.spawn_queue.token();

        for home_room_data_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_data_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

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
                        SPAWN_PRIORITY_HIGH,
                        Some(token),
                        Self::create_handle_claimer_spawn(mission_entity, *controller),
                    );

                    system_data.spawn_queue.request(*home_room_data_entity, spawn_request);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
