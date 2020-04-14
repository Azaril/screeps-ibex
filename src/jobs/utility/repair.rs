use itertools::*;
use screeps::*;
use std::collections::HashMap;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub enum RepairPriority {
    VeryLow,
    Low,
    Medium,
    High,
    Critical,
}

pub static ORDERED_REPAIR_PRIORITIES: &[RepairPriority] = &[
    RepairPriority::Critical,
    RepairPriority::High,
    RepairPriority::Medium,
    RepairPriority::Low,
    RepairPriority::VeryLow,
];

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

fn map_defense_priority(structure_type: StructureType, hits: u32, hits_max: u32, available_energy: u32, under_attack: bool) -> Option<RepairPriority> {
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
    } else if structure_type == StructureType::Rampart && hits <= RAMPART_DECAY_AMOUNT {
        Some(RepairPriority::Critical)
    } else if structure_type == StructureType::Rampart && hits <= RAMPART_DECAY_AMOUNT * 5 {
        Some(RepairPriority::High)
    } else if health_fraction < 0.0001 {
        Some(RepairPriority::High)
    } else if health_fraction < 0.001 {
        Some(RepairPriority::Medium)
    } else if health_fraction < 0.1 {
        Some(RepairPriority::Low)
    } else if available_energy > 10_000 {
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
        Structure::Spawn(_) => map_high_value_priority(hits, hits_max),
        Structure::Tower(_) => map_high_value_priority(hits, hits_max),
        Structure::Container(_) => map_high_value_priority(hits, hits_max),
        Structure::Wall(_) => map_defense_priority(StructureType::Wall, hits, hits_max, available_energy, under_attack),
        Structure::Rampart(_) => map_defense_priority(StructureType::Rampart, hits, hits_max, available_energy, under_attack),
        _ => map_normal_priority(hits, hits_max),
    }
}

pub fn get_repair_targets(room: &Room, allow_walls: bool) -> Vec<(Structure, u32, u32)> {
    room.find(find::STRUCTURES)
        .into_iter()
        .filter(|structure| {
            match structure {
                Structure::Wall(_) => allow_walls,
                Structure::Rampart(_) => allow_walls,
                _ => true 
            }
        })
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_prioritized_repair_targets(room: &Room, minimum_priority: Option<RepairPriority>, allow_walls: bool) -> HashMap<RepairPriority, Vec<Structure>> {
    let are_hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();

    let available_energy = room
        .storage()
        .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
        .unwrap_or(0);

    get_repair_targets(room, allow_walls)
        .into_iter()
        .filter_map(|(structure, hits, hits_max)| {
            map_structure_repair_priority(&structure, hits, hits_max, available_energy, are_hostile_creeps)
                .filter(|p| minimum_priority.map(|op| *p >= op).unwrap_or(true))
                .map(|p| (p, structure))
        })
        .into_group_map()
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_repair_structure(room: &Room, minimum_priority: Option<RepairPriority>, allow_walls: bool) -> Option<Structure> {
    let mut repair_targets = get_prioritized_repair_targets(room, minimum_priority, allow_walls);

    ORDERED_REPAIR_PRIORITIES
        .iter()
        .filter_map(|priority| {
            repair_targets.remove(priority)
        })
        .filter_map(|targets| targets.into_iter().min_by_key(|structure| structure.as_attackable().unwrap().hits()))
        .next()
}
