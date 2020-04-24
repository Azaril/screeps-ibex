use super::data::*;
use super::missionsystem::*;
use crate::jobs::dismantle::*;
use crate::jobs::utility::dismantle::*;
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
pub struct DismantleMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
    home_room_data: Entity,
    ignore_storage: bool,
    dismantlers: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DismantleMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity, ignore_storage: bool) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = DismantleMission::new(owner, room_data, home_room_data, ignore_storage);

        builder.with(MissionData::Dismantle(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity, ignore_storage: bool) -> DismantleMission {
        DismantleMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            ignore_storage,
            dismantlers: EntityVec::new(),
        }
    }

    fn create_handle_dismantler_spawn(
        mission_entity: Entity,
        dismantle_room: Entity,
        delivery_room:Entity,
        ignore_storage: bool
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Dismantle(DismantleJob::new(dismantle_room, delivery_room, ignore_storage));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Dismantle(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.dismantlers.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for DismantleMission {
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
        format!("Dismantle - Dismantlers: {}", self.dismantlers.len())
    }

    fn pre_run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup dismantlers that no longer exist.
        //

        self.dismantlers
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

        if dynamic_visibility_data.updated_within(1000)
            && (dynamic_visibility_data.owner().mine() 
                || dynamic_visibility_data.owner().friendly())
        {
            return Err("Room is owned by ourselves or a friendly".to_string());
        }

        if self.dismantlers.is_empty() {
            if let Some(room) = game::rooms::get(room_data.name) {
                let requires_dismantling = get_dismantle_structures(room, false).next().is_some();

                if !requires_dismantling {
                    return Ok(MissionResult::Success);
                }
            }
        }

        //TODO: Add better dynamic cpu adaptation.
        let bucket = game::cpu::bucket();
        let can_spawn = bucket > 9500.0 && crate::features::dismantle();

        if !can_spawn {
            return Ok(MissionResult::Running);
        }

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        let desired_dismantlers = 1;

        if self.dismantlers.len() < desired_dismantlers {
            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: None,
                maximum_repeat: None,
                pre_body: &[],
                repeat_body: &[Part::Work, Part::Work, Part::Work, Part::Work, Part::Move, Part::Move, Part::Move, Part::Move, Part::Carry, Part::Carry, Part::Move, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    "Dismantler".to_string(),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Self::create_handle_dismantler_spawn(runtime_data.entity, self.room_data, self.home_room_data, self.ignore_storage),
                );

                system_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}