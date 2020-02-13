use specs::*;
use specs::saveload::*;
use screeps::*;
use serde::{Serialize, Deserialize};

use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TowerMission {
}

impl TowerMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = TowerMission::new();

        builder.with(MissionData::Tower(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> TowerMission {
        TowerMission {
        }
    }
}

impl Mission for TowerMission
{
    fn run_mission<'a>(&mut self, _system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("Tower - Room: {}", runtime_data.room_owner.owner);

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            let towers = room.find(find::MY_STRUCTURES)
                .into_iter()
                .map(|owned_structure| owned_structure.as_structure())
                .filter_map(|structure| {
                    if let Structure::Tower(tower) = structure {
                        Some(tower)
                    } else {
                        None
                    }
                });

            let weakest_hostile_creep = room.find(find::HOSTILE_CREEPS)
                .into_iter()
                .min_by_key(|creep| creep.hits());

            let weakest_friendly_creep = room.find(find::MY_CREEPS)
                .into_iter()
                .filter(|creep| creep.hits() < creep.hits_max())
                .min_by_key(|creep| creep.hits());

            let mut repair_targets = RepairUtility::get_prioritized_repair_targets(&room);
            let mut repair_priorities = [RepairPriority::Critical, RepairPriority::High, RepairPriority::Medium].iter();

            let repair_structure = loop {
                if let Some(priority) = repair_priorities.next() {
                    if let Some(structures) = repair_targets.remove(priority) {
                        let weakest_structure = structures
                            .into_iter()
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

                        if let Some(weakest_structure) = weakest_structure {
                            break Some(weakest_structure)
                        }
                    }
                } else {
                    break None
                }
            };

            //TODO: Partition targets between towers. (Don't over damage, heal or repair.)

            for tower in towers {
                if let Some(creep) = weakest_hostile_creep {
                    tower.attack(&creep);
    
                    break;
                }
    
                if let Some(creep) = weakest_friendly_creep {
                    tower.heal(&creep);
    
                    break;
                }

                if let Some(structure) = repair_structure {
                    tower.repair(&structure);

                    break;
                }
            }

            return MissionResult::Running;
        }

        return MissionResult::Failure;
    }
}