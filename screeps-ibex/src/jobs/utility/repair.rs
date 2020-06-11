use crate::room::data::*;
use screeps::*;

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

fn map_defense_priority(
    structure_type: StructureType,
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

pub fn get_repair_targets(structures: &[Structure], allow_walls: bool) -> impl Iterator<Item = (&Structure, u32, u32)> {
    structures
        .iter()
        .filter(move |structure| match structure {
            Structure::Wall(_) => allow_walls,
            Structure::Rampart(_) => allow_walls,
            _ => true,
        })
        .filter(|structure| {
            if let Some(owned_structure) = structure.as_owned() {
                owned_structure.my()
            } else {
                true
            }
        })
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
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_prioritized_repair_targets(
    structures: &[Structure],
    available_energy: u32,
    are_hostile_creeps: bool,
    allow_walls: bool,
) -> impl Iterator<Item = (RepairPriority, &Structure)> {
    get_repair_targets(structures, allow_walls).filter_map(move |(structure, hits, hits_max)| {
        map_structure_repair_priority(&structure, hits, hits_max, available_energy, are_hostile_creeps).map(|p| (p, structure))
    })
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_repair_structure_and_priority(
    room_data: &RoomData,
    minimum_priority: Option<RepairPriority>,
    allow_walls: bool,
) -> Option<(RepairPriority, Structure)> {
    let structures = room_data.get_structures()?;
    let creeps = room_data.get_creeps()?;

    let are_hostile_creeps = !creeps.hostile().is_empty();

    let available_energy = structures
        .storages()
        .iter()
        .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
        .sum::<u32>();

    get_prioritized_repair_targets(structures.all(), available_energy, are_hostile_creeps, allow_walls)
        .filter(|(priority, _)| minimum_priority.map(|op| *priority >= op).unwrap_or(true))
        .map(|(priority, structure)| (priority, structure, structure.as_attackable().unwrap().hits()))
        .max_by(|(priority_a, _, hits_a), (priority_b, _, hits_b)| priority_a.cmp(priority_b).then_with(|| hits_a.cmp(hits_b).reverse()))
        .map(|(priority, structure, _)| (priority, structure.clone()))
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_repair_structure(room_data: &RoomData, minimum_priority: Option<RepairPriority>, allow_walls: bool) -> Option<Structure> {
    select_repair_structure_and_priority(room_data, minimum_priority, allow_walls).map(|(_, structure)| structure)
}
