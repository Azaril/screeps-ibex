use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::jobs::reserve::*;

#[derive(Clone, ConvertSaveload)]
pub struct ReserveMission {
    room_data: Entity,
    home_room_data: Entity,
    reservers: EntityVec,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ReserveMission {
    pub fn build<B>(builder: B, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ReserveMission::new(room_data, home_room_data);

        builder.with(MissionData::Reserve(mission)).marked::<SerializeMarker>()
    }

    pub fn new(room_data: Entity, home_room_data: Entity) -> ReserveMission {
        ReserveMission {
            room_data,
            home_room_data,
            reservers: EntityVec::new(),
        }
    }

    fn create_handle_reserver_spawn(
        mission_entity: Entity,
        controller_id: RemoteObjectId<StructureController>,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Reserve(ReserveJob::new(controller_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::Reserve(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.reservers.0.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ReserveMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui
                    .missions()
                    .add_text(format!("Reserve - Reservers: {}", self.reservers.0.len()), None);
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup reservers that no longer exist.
        //

        self.reservers
            .0
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

        if dynamic_visibility_data.updated_within(1000) {
            if dynamic_visibility_data.owner().mine() {
                return Ok(MissionResult::Success);
            }

            if !dynamic_visibility_data.owner().neutral()
                || dynamic_visibility_data.reservation().hostile()
                || dynamic_visibility_data.reservation().friendly()
            {
                return Err("Room is owned or reserved".to_string());
            }
        }

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;
        let controller_id = static_visibility_data.controller().ok_or("Expected a controller")?;

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        let alive_reservers = self
            .reservers
            .0
            .iter()
            .filter(|reserver_entity| {
                if let Some(creep_owner) = system_data.creep_owner.get(**reserver_entity) {
                    creep_owner
                        .owner
                        .resolve()
                        .and_then(|creep| creep.ticks_to_live().ok())
                        .unwrap_or(0)
                        > 100
                } else {
                    false
                }
            })
            .count();

        //TODO: Use visibility data to estimate amount thas has ticked down.
        let controller_has_sufficient_reservation = game::rooms::get(room_data.name)
            .and_then(|r| r.controller())
            .and_then(|c| c.reservation())
            .map(|r| r.ticks_to_end > 1000)
            .unwrap_or(false);

        //TODO: Compute number of reservers actually needed.
        if alive_reservers < 1 && !controller_has_sufficient_reservation {
            let body_definition = crate::creep::SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: Some(2),
                pre_body: &[],
                repeat_body: &[Part::Claim, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Reserver - Target Room: {}", room_data.name),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Self::create_handle_reserver_spawn(*runtime_data.entity, *controller_id),
                );

                runtime_data.spawn_queue.request(home_room_data.name, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
