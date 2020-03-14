use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;
use crate::remoteobjectid::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(Clone, ConvertSaveload)]
pub struct TowerMission {
    room_data: Entity,
}

#[cfg_attr(feature = "time", timing)]
impl TowerMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TowerMission::new(room_data);

        builder.with(MissionData::Tower(mission)).marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> TowerMission {
        TowerMission { room_data }
    }
}

#[cfg_attr(feature = "time", timing)]
impl Mission for TowerMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text("Tower".to_string(), None);
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
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

        let priority = if are_hostile_creeps {
            TransferPriority::High
        } else {
            TransferPriority::Low
        };

        for tower in towers {
            let tower_free_capacity = tower.store_free_capacity(Some(ResourceType::Energy));
            if tower_free_capacity > 0 {
                let transfer_request =
                    TransferDepositRequest::new(TransferTarget::Tower(tower.remote_id()), None, priority, tower_free_capacity, TransferType::Haul);

                runtime_data.transfer_queue.request_deposit(transfer_request);
            }
        }

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
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
            Some(RepairPriority::High)
        };
        let repair_targets = get_prioritized_repair_targets(&room, minimum_repair_priority);

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
