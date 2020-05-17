use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;
use crate::ownership::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct TowerMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TowerMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TowerMission::new(owner, room_data);

        builder
            .with(MissionData::Tower(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity) -> TowerMission {
        TowerMission {
            owner: owner.into(),
            room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for TowerMission {
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

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        "Tower".to_string()
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(|_system, transfer, room_name| {
                let room = game::rooms::get(room_name).ok_or("Expected room")?;

                let towers = room
                    .find(find::MY_STRUCTURES)
                    .into_iter()
                    .map(|owned_structure| owned_structure.as_structure())
                    .filter_map(|structure| {
                        if let Structure::Tower(tower) = structure {
                            Some(tower)
                        } else {
                            None
                        }
                    });

                let hostile_creeps = room.find(find::HOSTILE_CREEPS);
                let are_hostile_creeps = !hostile_creeps.is_empty();

                let priority = if are_hostile_creeps {
                    TransferPriority::High
                } else {
                    TransferPriority::Low
                };

                for tower in towers {
                    let tower_free_capacity = tower.store_free_capacity(Some(ResourceType::Energy));
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
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let towers = room
            .find(find::MY_STRUCTURES)
            .into_iter()
            .map(|owned_structure| owned_structure.as_structure())
            .filter_map(|structure| {
                if let Structure::Tower(tower) = structure {
                    Some(tower)
                } else {
                    None
                }
            });

        let hostile_creeps = room.find(find::HOSTILE_CREEPS);
        let are_hostile_creeps = !hostile_creeps.is_empty();

        let weakest_hostile_creep = hostile_creeps.into_iter().min_by_key(|creep| creep.hits());

        let weakest_friendly_creep = room
            .find(find::MY_CREEPS)
            .into_iter()
            .filter(|creep| creep.hits() < creep.hits_max())
            .min_by_key(|creep| creep.hits());

        let minimum_repair_priority = if are_hostile_creeps {
            Some(RepairPriority::Medium)
        } else {
            Some(RepairPriority::Low)
        };

        let repair_targets = get_prioritized_repair_targets(&room, minimum_repair_priority, false);

        let repair_structure = ORDERED_REPAIR_PRIORITIES
            .iter()
            .filter_map(|priority| repair_targets.get(priority))
            .flat_map(|i| i.iter())
            .filter_map(|structure| {
                let hits = if let Some(attackable) = structure.as_attackable() {
                    Some((attackable.hits(), attackable.hits_max()))
                } else {
                    None
                };

                hits.map(|(hits, hits_max)| (structure, hits, hits_max))
            })
            .min_by_key(|(_, hits, _)| *hits)
            .map(|(structure, _, _)| structure);

        //TODO: Partition targets between towers. (Don't over damage, heal or repair.)

        for tower in towers {
            if let Some(creep) = weakest_hostile_creep.as_ref() {
                tower.attack(creep);
                continue;
            }

            if let Some(creep) = weakest_friendly_creep.as_ref() {
                tower.heal(creep);
                continue;
            }

            if let Some(structure) = repair_structure.as_ref() {
                tower.repair(*structure);
                continue;
            }
        }

        Ok(MissionResult::Running)
    }
}
