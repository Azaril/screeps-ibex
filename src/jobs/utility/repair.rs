use crate::findnearest::*;
use crate::structureidentifier::*;
use itertools::*;
use screeps::*;
use std::collections::HashMap;

#[derive(PartialEq, Eq, Hash, Debug)]
pub enum RepairPriority {
    Critical,
    High,
    Medium,
    Low,
    VeryLow,
}

pub static ORDERED_REPAIR_PRIORITIES: &[RepairPriority] = &[
    RepairPriority::Critical,
    RepairPriority::High,
    RepairPriority::Medium,
    RepairPriority::Low,
    RepairPriority::VeryLow,
];

pub struct RepairUtility;

impl RepairUtility {
    fn map_normal_priority(hits: u32, hits_max: u32) -> Option<RepairPriority> {
        let health_fraction = (hits as f32) / (hits_max as f32);

        let priority = if health_fraction < 0.25 {
            RepairPriority::High
        } else if health_fraction < 0.5 {
            RepairPriority::Medium
        } else if health_fraction < 0.75 {
            RepairPriority::Low
        } else {
            RepairPriority::VeryLow
        };

        Some(priority)
    }

    fn map_high_value_priority(hits: u32, hits_max: u32) -> Option<RepairPriority> {
        let health_fraction = (hits as f32) / (hits_max as f32);

        let priority = if health_fraction < 0.5 {
            RepairPriority::Critical
        } else if health_fraction < 0.75 {
            RepairPriority::High
        } else if health_fraction < 0.95 {
            RepairPriority::Low
        } else {
            RepairPriority::VeryLow
        };

        Some(priority)
    }

    fn map_defense_priority(
        hits: u32,
        hits_max: u32,
        available_energy: u32,
        under_attack: bool,
    ) -> Option<RepairPriority> {
        let health_fraction = (hits as f32) / (hits_max as f32);

        if under_attack {
            if health_fraction < 0.01 {
                Some(RepairPriority::Critical)
            } else if health_fraction < 0.25 {
                Some(RepairPriority::High)
            } else if health_fraction < 0.5 {
                Some(RepairPriority::Medium)
            } else if health_fraction < 0.95 {
                Some(RepairPriority::Low)
            } else {
                Some(RepairPriority::VeryLow)
            }
        } else if health_fraction < 0.001 {
            Some(RepairPriority::Medium)
        } else if health_fraction < 0.1 {
            Some(RepairPriority::Low)
        } else if available_energy > 100_000 {
            Some(RepairPriority::VeryLow)
        } else {
            None
        }
    }

    fn map_structure_repair_priority(
        structure: &Structure,
        hits: u32,
        hits_max: u32,
        available_energy: u32,
        under_attack: bool,
    ) -> Option<RepairPriority> {
        match structure {
            Structure::Spawn(_) => Self::map_high_value_priority(hits, hits_max),
            Structure::Tower(_) => Self::map_high_value_priority(hits, hits_max),
            Structure::Container(_) => Self::map_high_value_priority(hits, hits_max),
            Structure::Wall(_) => {
                Self::map_defense_priority(hits, hits_max, available_energy, under_attack)
            }
            Structure::Rampart(_) => {
                Self::map_defense_priority(hits, hits_max, available_energy, under_attack)
            }
            _ => Self::map_normal_priority(hits, hits_max),
        }
    }

    pub fn get_repair_targets(room: &Room) -> Vec<(Structure, u32, u32)> {
        room.find(find::STRUCTURES)
            .into_iter()
            .filter(|structure| {
                if let Some(owned_structure) = structure.as_owned() {
                    owned_structure.my()
                } else {
                    true
                }
            })
            .map(|owned_structure| owned_structure.as_structure())
            .filter_map(|structure| {
                let hits = if let Some(attackable) = structure.as_attackable() {
                    let hits = attackable.hits();
                    let hits_max = attackable.hits_max();
                    if hits > 0 && hits_max > 0 {
                        Some((hits, hits_max))
                    } else {
                        None
                    }
                } else {
                    None
                };

                hits.map(|(hits, hits_max)| (structure, hits, hits_max))
            })
            .filter(|(_, hits, hits_max)| hits < hits_max)
            .collect()
    }

    pub fn get_prioritized_repair_targets(room: &Room) -> HashMap<RepairPriority, Vec<Structure>> {
        let are_hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();

        let available_energy = room
            .storage()
            .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
            .unwrap_or(0);

        Self::get_repair_targets(room)
            .into_iter()
            .filter_map(|(structure, hits, hits_max)| {
                let priority = Self::map_structure_repair_priority(
                    &structure,
                    hits,
                    hits_max,
                    available_energy,
                    are_hostile_creeps,
                );

                priority.map(|p| (p, structure))
            })
            .into_group_map()
    }

    pub fn select_repair_structure(room: &Room, start_pos: RoomPosition) -> Option<Structure> {
        let mut repair_targets = Self::get_prioritized_repair_targets(room);

        for priority in ORDERED_REPAIR_PRIORITIES.iter() {
            if let Some(structures) = repair_targets.remove(priority) {
                //TODO: Make find_nearest cheap - find_nearest linear is a bad approximation.
                if let Some(structure) = structures.into_iter().find_nearest_linear(start_pos) {
                    return Some(structure);
                }
            }
        }

        None
    }
}

pub trait ValidateRepairTarget {
    fn is_valid_repair_target(&self) -> Option<bool>;
}

impl ValidateRepairTarget for Structure {
    fn is_valid_repair_target(&self) -> Option<bool> {
        if let Some(attackable) = self.as_attackable() {
            Some(attackable.hits() < attackable.hits_max())
        } else {
            Some(false)
        }
    }
}

impl ValidateRepairTarget for RemoteStructureIdentifier {
    fn is_valid_repair_target(&self) -> Option<bool> {
        if game::rooms::get(self.pos().room_name()).is_some() {
            self.resolve()
                .and_then(|s| s.is_valid_repair_target())
                .or(Some(false))
        } else {
            None
        }
    }
}
