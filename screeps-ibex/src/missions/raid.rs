use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::haul::*;
use crate::jobs::utility::dismantle::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::convert::*;

#[derive(ConvertSaveload)]
pub struct RaidMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_data: Entity,
    raiders: EntityVec<Entity>,
    allow_spawning: bool,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RaidMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RaidMission::new(owner, room_data, home_room_data);

        builder
            .with(MissionData::Raid(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_data: Entity) -> RaidMission {
        RaidMission {
            owner: owner.into(),
            room_data,
            home_room_data,
            raiders: EntityVec::new(),
            allow_spawning: true,
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow
    }

    fn create_handle_raider_spawn(
        mission_entity: Entity,
        raid_room: Entity,
        delivery_room: Entity,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Haul(HaulJob::new(&[raid_room], &[delivery_room], false));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<RaidMission>()
                {
                    mission_data.raiders.push(creep_entity);
                }
            });
        })
    }

    fn request_transfer_for_structures(transfer: &mut dyn TransferRequestSystem, room_data: &RoomData) -> Result<(), String> {
        //TODO: Fill out remaining types?
        //Structure::Ruin(s) => Ok(s.into()),
        //Structure::Tombstone(s) => Ok(s.into()),
        //Structure::Resource(s) => Ok(s.into()),

        let structures = room_data.get_structures().ok_or("Expected structures")?;

        for structure in structures.all().iter() {
            if let Some(store) = structure.as_has_store() {
                for resource in store.store_types() {
                    let resource_amount = store.store_used_capacity(Some(resource));

                    if resource_amount > 0 {
                        if let Ok(transfer_target) = structure.try_into() {
                            let transfer_request = TransferWithdrawRequest::new(
                                transfer_target,
                                resource,
                                TransferPriority::Low,
                                resource_amount,
                                TransferType::Haul,
                            );

                            transfer.request_withdraw(transfer_request);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for RaidMission {
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
        format!("Raid - Raiders: {}", self.raiders.len())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.raiders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room")?;

                Self::request_transfer_for_structures(transfer, &room_data)?;

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

        if dynamic_visibility_data.updated_within(1000)
            && (dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly())
        {
            return Err("Room is owned by ourselves or a friendly".to_string());
        }

        if let Some(structures) = room_data.get_structures() {
            if structures.all().iter().all(|s| has_empty_storage(s)) {
                return Ok(MissionResult::Success);
            }
        }

        let home_room_data = system_data.room_data.get(self.home_room_data).ok_or("Expected home room data")?;
        let home_room = game::rooms::get(home_room_data.name).ok_or("Expected home room")?;

        //TODO: Add better dynamic cpu adaptation.
        let bucket = game::cpu::bucket();
        let can_spawn = bucket > 9000.0 && crate::features::raid() && self.allow_spawning;

        if !can_spawn {
            return Ok(MissionResult::Running);
        }

        let desired_raiders = 2;

        if self.raiders.len() < desired_raiders {
            let priority = SPAWN_PRIORITY_LOW;

            let body_definition = SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: None,
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Raider - Target Room: {}", room_data.name),
                    &body,
                    priority,
                    Self::create_handle_raider_spawn(mission_entity, self.room_data, self.home_room_data),
                );

                system_data.spawn_queue.request(self.home_room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
