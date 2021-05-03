use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct TowerMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TowerMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TowerMission::new(owner, room_data);

        builder
            .with(MissionData::Tower(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> TowerMission {
        TowerMission {
            owner: owner.into(),
            room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for TowerMission {
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
        "Tower".to_string()
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room data")?;
                let structures = room_data.get_structures().ok_or("Expected structures")?;
                let creeps = room_data.get_creeps().ok_or("Expected creeps")?;

                let towers = structures.towers();

                let hostile_creeps = creeps.hostile();
                let are_hostile_creeps = !hostile_creeps.is_empty();

                let priority = if are_hostile_creeps {
                    TransferPriority::High
                } else {
                    TransferPriority::Low
                };

                for tower in towers {
                    let tower_store = tower.store();
                    let tower_free_capacity = tower_store.get_free_capacity(Some(ResourceType::Energy));
                    if tower_free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Tower(tower.remote_id()),
                            Some(ResourceType::Energy),
                            priority,
                            tower_free_capacity as u32,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(transfer_request);
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
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;
        let creeps = room_data.get_creeps().ok_or("Expected creeps")?;

        let towers = structures.towers();
       
        //TODO: Include power creeps?        
        let weakest_dangerous_hostile_creep = creeps.hostile()
            .iter()
            .filter(|c| c.body().iter().any(|p| match p.part() {
                Part::Attack | Part::RangedAttack | Part::Work => true,
                _ => false,
            }))
            .min_by_key(|creep| creep.hits());

        let weakest_hostile_creep = creeps.hostile()
            .iter()
            .min_by_key(|creep| creep.hits());            

        let weakest_friendly_creep = creeps
            .friendly()
            .iter()
            .filter(|creep| creep.hits() < creep.hits_max())
            .min_by_key(|creep| creep.hits());

        let minimum_repair_priority = if dynamic_visibility_data.hostile_creeps() {
            Some(RepairPriority::Medium)
        } else {
            Some(RepairPriority::Low)
        };

        let repair_structure = select_repair_structure(&room_data, minimum_repair_priority, false);

        //TODO: Partition targets between towers. (Don't over damage, heal or repair.)

        for tower in towers {
            if let Some(creep) = weakest_dangerous_hostile_creep.or(weakest_hostile_creep) {
                tower.attack(creep);
                continue;
            }

            if let Some(creep) = weakest_friendly_creep {
                tower.heal(creep);
                continue;
            }

            if let Some(structure) = repair_structure.as_ref() {
                tower.repair(structure.as_structure());
                continue;
            }
        }

        Ok(MissionResult::Running)
    }
}
