use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
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

    pub fn can_run(room: &Room) -> bool {
        !Self::get_power_spawns(room).is_empty()
    }

    fn get_power_spawns(room: &Room) -> Vec<StructurePowerSpawn> {
        let structures = room.find(find::STRUCTURES);

        structures.into_iter().filter_map(|structure| match structure {
            Structure::PowerSpawn(power_spawn) => Some(power_spawn),
            _ => None
        }).filter(|power_spawn| power_spawn.my())
        .collect::<Vec<_>>()
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
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let power_spawns = Self::get_power_spawns(&room);

        if power_spawns.is_empty() {
            return Err("No power spawns in room".to_owned());
        }

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |_system, transfer, _room_name| {
                for power_spawn in &power_spawns {
                    let required_energy = power_spawn.store_free_capacity(Some(ResourceType::Energy));
        
                    if required_energy > 0 {
                        let deposit_request = TransferDepositRequest::new(
                            TransferTarget::PowerSpawn(power_spawn.remote_id()), 
                            Some(ResourceType::Energy), 
                            TransferPriority::Low, 
                            required_energy as u32, 
                            TransferType::Haul);
                        
                        transfer.request_deposit(deposit_request);
                    }
        
                    let required_power = power_spawn.store_free_capacity(Some(ResourceType::Power));
        
                    if required_power > 0 {
                        let deposit_request = TransferDepositRequest::new(
                            TransferTarget::PowerSpawn(power_spawn.remote_id()), 
                            Some(ResourceType::Power), 
                            TransferPriority::Low, 
                            required_power as u32, 
                            TransferType::Haul);
                        
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
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let power_spawns = Self::get_power_spawns(&room);

        if power_spawns.is_empty() {
            return Err("No power spawns in room".to_owned());
        }

        for power_spawn in power_spawns {
            let available_energy = power_spawn.store_of(ResourceType::Energy);
            let available_power = power_spawn.store_of(ResourceType::Power);

            if available_energy > POWER_SPAWN_ENERGY_RATIO && available_power > 0 {
                power_spawn.process_power();
            }
        }

        Ok(MissionResult::Running)
    }
}
