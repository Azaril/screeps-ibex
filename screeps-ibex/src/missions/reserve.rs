use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::reserve::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct ReserveMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_data: Entity,
    reservers: EntityVec<Entity>,
    allow_spawning: bool,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ReserveMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ReserveMission::new(owner, room_data, home_room_data);

        builder
            .with(MissionData::Reserve(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> ReserveMission {
        ReserveMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            reservers: EntityVec::new(),
            allow_spawning: true,
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow
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

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<ReserveMission>()
                {
                    mission_data.reservers.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ReserveMission {
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
        format!("Reserve - Reservers: {}", self.reservers.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup reservers that no longer exist.
        //

        self.reservers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
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

        //TODO: Add better dynamic cpu adaptation.
        let bucket = game::cpu::bucket();
        let can_spawn = bucket > 9000.0 && crate::features::remote_mine::reserve() && self.allow_spawning;

        if !can_spawn {
            return Ok(MissionResult::Running);
        }

        let alive_reservers = self
            .reservers
            .iter()
            .filter(|entity| {
                system_data.creep_spawning.get(**entity).is_some()
                    || system_data
                        .creep_owner
                        .get(**entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live().ok())
                        .map(|count| count > 100)
                        .unwrap_or(false)
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
                    Self::create_handle_reserver_spawn(mission_entity, *controller_id),
                );

                system_data.spawn_queue.request(self.home_room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}