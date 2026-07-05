use crate::repairqueue::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use screeps::*;

// `RepairPriority` + the health-fraction priority maps live in
// `screeps_econ_decision::repair` since ADR 0040 M3 (K3) — one implementation, consumed by
// this adapter AND the economy sim. Re-exported so every bot call site keeps its
// `jobs::utility::repair::RepairPriority` path. (The live f32 fraction maps were replaced by
// the kernel's exact-integer forms — bit-identical on every reachable hits_max; see the
// kernel's module docs.)
#[allow(unused_imports)] // ORDERED_REPAIR_PRIORITIES is re-exported API (pre-move pub static)
pub use screeps_econ_decision::repair::{RepairPriority, ORDERED_REPAIR_PRIORITIES};

fn map_normal_priority(hits: u32, hits_max: u32) -> Option<RepairPriority> {
    Some(screeps_econ_decision::repair::map_normal_priority(hits, hits_max))
}

fn map_high_value_priority(hits: u32, hits_max: u32) -> Option<RepairPriority> {
    Some(screeps_econ_decision::repair::map_high_value_priority(hits, hits_max))
}

fn map_defense_priority(
    structure_type: StructureType,
    hits: u32,
    hits_max: u32,
    // The ad-hoc `available_energy > 10_000` VeryLow wall/rampart gate was
    // deleted at ADR 0040 M5a (operator decision 2026-07-05: remove the
    // walls-10k thresholds); the market now prices wall/rampart maintenance
    // repair natively. The parameter is retained (the queue-populating callers
    // still thread it) but no longer consulted in the peaceful branch.
    _available_energy: Option<u32>,
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
    } else if (structure_type == StructureType::Rampart && hits <= RAMPART_DECAY_AMOUNT * 5) || health_fraction < 0.0001 {
        Some(RepairPriority::High)
    } else if health_fraction < 0.001 {
        Some(RepairPriority::Medium)
    } else if health_fraction < 0.1 {
        Some(RepairPriority::Low)
    } else {
        None
    }
}

/// Compute the repair priority for a structure based on its type and health.
/// Public so missions can use this when populating the repair queue.
pub fn map_structure_repair_priority(
    structure: &StructureObject,
    hits: u32,
    hits_max: u32,
    available_energy: Option<u32>,
    under_attack: bool,
) -> Option<RepairPriority> {
    match structure {
        StructureObject::StructureSpawn(_) => map_high_value_priority(hits, hits_max),
        StructureObject::StructureTower(_) => map_high_value_priority(hits, hits_max),
        StructureObject::StructureContainer(_) => map_high_value_priority(hits, hits_max),
        StructureObject::StructureWall(_) => map_defense_priority(StructureType::Wall, hits, hits_max, available_energy, under_attack),
        StructureObject::StructureRampart(_) => {
            map_defense_priority(StructureType::Rampart, hits, hits_max, available_energy, under_attack)
        }
        _ => map_normal_priority(hits, hits_max),
    }
}

pub fn get_repair_targets(structures: &[StructureObject], allow_walls: bool) -> impl Iterator<Item = (&StructureObject, u32, u32)> {
    structures
        .iter()
        .filter(move |structure| match structure {
            StructureObject::StructureWall(_) => allow_walls,
            StructureObject::StructureRampart(_) => allow_walls,
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

/// Get prioritized repair targets from a room scan. This is the low-level
/// fallback used when the repair queue has no entries for a room.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_prioritized_repair_targets(
    structures: &[StructureObject],
    available_energy: Option<u32>,
    are_hostile_creeps: bool,
    allow_walls: bool,
) -> impl Iterator<Item = (RepairPriority, &StructureObject)> {
    get_repair_targets(structures, allow_walls).filter_map(move |(structure, hits, hits_max)| {
        map_structure_repair_priority(structure, hits, hits_max, available_energy, are_hostile_creeps).map(|p| (p, structure))
    })
}

/// Select the best repair target for a room. Checks the repair queue first
/// (mission-requested repairs), then falls back to a room scan.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_repair_structure_and_priority(
    room_data: &RoomData,
    repair_queue: &RepairQueue,
    minimum_priority: Option<RepairPriority>,
    allow_walls: bool,
) -> Option<(RepairPriority, RemoteStructureIdentifier)> {
    // Check the repair queue first -- these are mission-requested repairs
    // (wall repair, nuke defense, etc.) that should take priority.
    if let Some(request) = repair_queue.get_best_target(room_data.name, minimum_priority) {
        return Some((request.priority, request.structure_id));
    }

    // Fall back to room-scan approach for structures not in the queue.
    let structures = room_data.get_structures()?;
    let creeps = room_data.get_creeps()?;

    let are_hostile_creeps = !creeps.hostile().is_empty();

    let available_energy = structures
        .storages()
        .iter()
        .map(|s| s.store().get_used_capacity(Some(ResourceType::Energy)))
        .sum::<u32>();

    get_prioritized_repair_targets(structures.all(), Some(available_energy), are_hostile_creeps, allow_walls)
        .filter(|(priority, _)| minimum_priority.map(|op| *priority >= op).unwrap_or(true))
        .filter_map(|(priority, structure)| structure.as_attackable().map(|a| (priority, structure, a.hits())))
        .max_by(|(priority_a, _, hits_a), (priority_b, _, hits_b)| priority_a.cmp(priority_b).then_with(|| hits_a.cmp(hits_b).reverse()))
        .map(|(priority, structure, _)| (priority, RemoteStructureIdentifier::new(structure)))
}

/// Select the best repair target for a room, returning just the structure identifier.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_repair_structure(
    room_data: &RoomData,
    repair_queue: &RepairQueue,
    minimum_priority: Option<RepairPriority>,
    allow_walls: bool,
) -> Option<RemoteStructureIdentifier> {
    select_repair_structure_and_priority(room_data, repair_queue, minimum_priority, allow_walls).map(|(_, structure)| structure)
}

/// Select the best in-range repair target. Checks the repair queue for
/// nearby mission-requested repairs first, then falls back to a room scan
/// filtered by range. Used for opportunistic repair while moving.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_repair_structure_in_range(
    room_data: &RoomData,
    repair_queue: &RepairQueue,
    pos: Position,
    range: u32,
    minimum_priority: Option<RepairPriority>,
    allow_walls: bool,
) -> Option<(RepairPriority, RemoteStructureIdentifier)> {
    // Check the repair queue for in-range targets first.
    if let Some(request) = repair_queue.get_best_target_in_range(room_data.name, pos, range, minimum_priority) {
        return Some((request.priority, request.structure_id));
    }

    // Fall back to room scan filtered by range.
    let structures = room_data.get_structures()?;
    let creeps = room_data.get_creeps()?;

    let are_hostile_creeps = !creeps.hostile().is_empty();

    get_prioritized_repair_targets(structures.all(), None, are_hostile_creeps, allow_walls)
        .filter(|(priority, _)| minimum_priority.map(|p| *priority >= p).unwrap_or(true))
        .filter(|(_, structure)| structure.pos().in_range_to(pos, range))
        .max_by_key(|(priority, _)| *priority)
        .map(|(priority, structure)| (priority, RemoteStructureIdentifier::new(structure)))
}
