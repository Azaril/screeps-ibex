use screeps::*;
use itertools::*;
use std::collections::HashMap;

#[derive(PartialEq, Eq, Hash, Debug)]
pub enum RepairPriority {
    Critical,
    High,
    Medium,
    Low,
    VeryLow
}

pub static ORDERED_REPAIR_PRIORITIES: &[RepairPriority] = &[
    RepairPriority::Critical, 
    RepairPriority::High, 
    RepairPriority::Medium, 
    RepairPriority::Low, 
    RepairPriority::VeryLow
];

pub struct RepairUtility;

impl RepairUtility {
    fn map_normal_priority(hits: u32, hits_max: u32) -> RepairPriority {
        let health_fraction = (hits as f32) / (hits_max as f32);

        if health_fraction < 0.25 {
            RepairPriority::High
        } else if health_fraction < 0.5 {
            RepairPriority::Medium
        } else if health_fraction < 0.75 {
            RepairPriority::Low
        } else {
            RepairPriority::VeryLow
        }
    }

    fn map_high_value_priority(hits: u32, hits_max: u32) -> RepairPriority {
        let health_fraction = (hits as f32) / (hits_max as f32);

        if health_fraction < 0.5 {
            RepairPriority::Critical
        } else if health_fraction < 0.75 {
            RepairPriority::High
        } else if health_fraction < 0.95 {
            RepairPriority::Low
        } else {
            RepairPriority::VeryLow
        }
    }

    fn map_structure_repair_priority(structure: &Structure, hits: u32, hits_max: u32) -> RepairPriority {
        match structure {
            Structure::Spawn(_) => Self::map_high_value_priority(hits, hits_max),
            Structure::Tower(_) => Self::map_high_value_priority(hits, hits_max),
            Structure::Container(_) => Self::map_high_value_priority(hits, hits_max),
            _ => Self::map_normal_priority(hits, hits_max)
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
                }})
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
        Self::get_repair_targets(room)
            .into_iter()
            .map(|(structure, hits, hits_max)| {
                let priority = Self::map_structure_repair_priority(&structure, hits, hits_max);

                (priority, structure)
                })
            .into_group_map()
    }
}