use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct PowerSpawnMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl PowerSpawnMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = PowerSpawnMission::new(owner, room_data);

        builder
            .with(MissionData::PowerSpawn(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> PowerSpawnMission {
        PowerSpawnMission {
            owner: owner.into(),
            room_data,
        }
    }

    pub fn can_run(room_data: &RoomData) -> bool {
        room_data
            .get_structures()
            .map(|structures| !structures.power_spawns().is_empty())
            .unwrap_or(false)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for PowerSpawnMission {
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
        "PowerSpawn".to_string()
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let structures = room_data.get_structures().ok_or("Expected structures")?;

        if structures.power_spawns().is_empty() {
            return Err("No power spawns in room".to_owned());
        }

        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room data")?;
                let structures = room_data.get_structures().ok_or("Expected structures")?;
                let power_spawns = structures.power_spawns();

                for power_spawn in power_spawns.iter() {
                    let required_energy = power_spawn.store().get_free_capacity(Some(ResourceType::Energy));

                    let map_priority = |fraction: f32| {
                        if fraction < 0.25 {
                            TransferPriority::High
                        } else if fraction < 0.5 {
                            TransferPriority::Medium
                        } else {
                            TransferPriority::Low
                        }
                    };

                    if required_energy > 0 {
                        let maximum_energy = power_spawn.store().get_capacity(Some(ResourceType::Energy));
                        let energy_fraction = (required_energy as f32) / (maximum_energy as f32);

                        let deposit_request = TransferDepositRequest::new(
                            TransferTarget::PowerSpawn(power_spawn.remote_id()),
                            Some(ResourceType::Energy),
                            map_priority(energy_fraction),
                            required_energy as u32,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(deposit_request);
                    }

                    let required_power = power_spawn.store().get_free_capacity(Some(ResourceType::Power));

                    if required_power > 0 {
                        let maximum_power = power_spawn.store().get_capacity(Some(ResourceType::Power));
                        let power_fraction = (required_power as f32) / (maximum_power as f32);

                        let deposit_request = TransferDepositRequest::new(
                            TransferTarget::PowerSpawn(power_spawn.remote_id()),
                            Some(ResourceType::Power),
                            map_priority(power_fraction),
                            required_power as u32,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(deposit_request);
                    }
                }

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let structures = room_data.get_structures().ok_or("Expected structures")?;
        let power_spawns = structures.power_spawns();

        if power_spawns.is_empty() {
            return Err("No power spawns in room".to_owned());
        }

        for power_spawn in power_spawns.iter() {
            let available_energy = power_spawn.store().get(ResourceType::Energy).unwrap_or(0);
            let available_power = power_spawn.store().get(ResourceType::Power).unwrap_or(0);

            if available_energy > POWER_SPAWN_ENERGY_RATIO && available_power > 0 {
                let _ = power_spawn.process_power();
            }
        }

        Ok(MissionResult::Running)
    }
}
