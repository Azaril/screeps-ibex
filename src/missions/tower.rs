use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct TowerMission {
    room_data: Entity,
}

impl TowerMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TowerMission::new(room_data);

        builder
            .with(MissionData::Tower(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> TowerMission {
        TowerMission { room_data }
    }
}

impl Mission for TowerMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("TowerMission");

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(room) = game::rooms::get(room_data.name) {
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

                let weakest_hostile_creep =
                    hostile_creeps.into_iter().min_by_key(|creep| creep.hits());

                let weakest_friendly_creep = room
                    .find(find::MY_CREEPS)
                    .into_iter()
                    .filter(|creep| creep.hits() < creep.hits_max())
                    .min_by_key(|creep| creep.hits());

                let mut repair_targets = RepairUtility::get_prioritized_repair_targets(&room);
                let mut repair_priorities = if are_hostile_creeps {
                    [
                        RepairPriority::Critical,
                        RepairPriority::High,
                        RepairPriority::Medium,
                    ]
                    .iter()
                } else {
                    [RepairPriority::Critical, RepairPriority::High].iter()
                };

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
                                break Some(weakest_structure);
                            }
                        }
                    } else {
                        break None;
                    }
                };

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
                        tower.repair(structure);
                        continue;
                    }
                }

                return MissionResult::Running;
            }
        }

        MissionResult::Failure
    }
}
