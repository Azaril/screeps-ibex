use crate::creep::SpawnBodyDefinition;
use screeps::*;

use super::structure_data::StructureData;

/// Minimum TTL threshold used when we cannot compute a more accurate lead
/// time (e.g. no spawn positions visible). This is deliberately conservative
/// — a small overlap is cheaper than losing mining ticks.
pub const MIN_REPLACEMENT_LEAD_TICKS: u32 = 30;

/// Build a `SpawnBodyDefinition` for a source miner (link or container).
///
/// - `is_local`: true when the source is in the same room as the home room
/// - `energy_capacity`: the home room's `energy_capacity_available()`
/// - `work_parts`: max WORK parts needed to fully harvest the source
/// - `has_link`: true when the miner will deposit into a link (gets a CARRY part)
pub fn source_miner_body(is_local: bool, energy_capacity: u32, work_parts: usize, has_link: bool) -> SpawnBodyDefinition<'static> {
    if is_local {
        if has_link {
            SpawnBodyDefinition {
                maximum_energy: energy_capacity,
                minimum_repeat: Some(1),
                maximum_repeat: Some(work_parts),
                pre_body: &[Part::Move, Part::Carry],
                repeat_body: &[Part::Work],
                post_body: &[],
            }
        } else {
            SpawnBodyDefinition {
                maximum_energy: energy_capacity,
                minimum_repeat: Some(1),
                maximum_repeat: Some(work_parts),
                pre_body: &[Part::Move],
                repeat_body: &[Part::Work],
                post_body: &[],
            }
        }
    } else if has_link {
        SpawnBodyDefinition {
            maximum_energy: energy_capacity,
            minimum_repeat: Some(1),
            maximum_repeat: Some(work_parts),
            pre_body: &[Part::Carry],
            repeat_body: &[Part::Move, Part::Work],
            post_body: &[],
        }
    } else {
        SpawnBodyDefinition {
            maximum_energy: energy_capacity,
            minimum_repeat: Some(1),
            maximum_repeat: Some(work_parts + 1),
            pre_body: &[Part::Carry],
            repeat_body: &[Part::Move, Part::Work],
            post_body: &[],
        }
    }
}

/// Build a `SpawnBodyDefinition` for a mineral miner.
///
/// - `is_local`: true when the mineral is in the same room as the home room
/// - `energy_capacity`: the home room's `energy_capacity_available()`
pub fn mineral_miner_body(is_local: bool, energy_capacity: u32) -> SpawnBodyDefinition<'static> {
    if is_local {
        SpawnBodyDefinition {
            maximum_energy: energy_capacity,
            minimum_repeat: Some(1),
            maximum_repeat: None,
            pre_body: &[],
            repeat_body: &[Part::Work, Part::Work, Part::Move],
            post_body: &[],
        }
    } else {
        SpawnBodyDefinition {
            maximum_energy: energy_capacity,
            minimum_repeat: Some(1),
            maximum_repeat: None,
            pre_body: &[],
            repeat_body: &[Part::Move, Part::Work],
            post_body: &[],
        }
    }
}

/// Build a `SpawnBodyDefinition` for a harvester.
///
/// - `energy`: the energy to use for the body (may be `energy_available` or `energy_capacity_available`)
pub fn harvester_body(energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: energy,
        minimum_repeat: Some(1),
        maximum_repeat: Some(5),
        pre_body: &[],
        repeat_body: &[Part::Move, Part::Move, Part::Carry, Part::Work],
        post_body: &[],
    }
}

/// Estimate how many ticks it takes for a creep with the given body to
/// traverse `distance` tiles, assuming roads are present. Returns the
/// travel time in ticks.
///
/// Screeps movement on roads:
///   fatigue_per_tile = MOVE_COST_ROAD (1) * non_move_parts
///   fatigue_removed_per_tick = MOVE_POWER (2) * move_parts
///   ticks_per_tile = ceil(fatigue_per_tile / fatigue_removed_per_tick)
///
/// If the creep has no MOVE parts it cannot move; returns u32::MAX.
pub fn estimate_travel_ticks(body: &[Part], distance: u32) -> u32 {
    let move_parts = body.iter().filter(|p| **p == Part::Move).count() as u32;
    if move_parts == 0 {
        return u32::MAX;
    }

    let non_move_parts = body.len() as u32 - move_parts;
    let fatigue_per_tile = MOVE_COST_ROAD * non_move_parts;
    let fatigue_removed_per_tick = MOVE_POWER * move_parts;

    // Ceiling division: ticks needed to clear fatigue from one road tile.
    let ticks_per_tile = fatigue_per_tile.div_ceil(fatigue_removed_per_tick);
    // At minimum 1 tick per tile (even with excess MOVE parts).
    let ticks_per_tile = ticks_per_tile.max(1);

    distance * ticks_per_tile
}

/// Compute the total lead time (in ticks) needed to spawn a replacement
/// creep and have it walk to `target_pos`. Uses the precomputed pathfinding
/// distance from the nearest spawn in `structure_data`.
pub fn miner_lead_ticks(body: &[Part], target_pos: screeps::Position, structure_data: &StructureData) -> u32 {
    let spawn_ticks = body.len() as u32 * CREEP_SPAWN_TIME;

    let distance = structure_data.nearest_spawn_distances.get(&target_pos).copied().unwrap_or(0);

    let travel_ticks = estimate_travel_ticks(body, distance);

    (spawn_ticks + travel_ticks).max(MIN_REPLACEMENT_LEAD_TICKS)
}

/// Compute the number of WORK parts needed to fully harvest a source each
/// regeneration cycle.
pub fn source_work_parts(likely_owned_room: bool) -> usize {
    let energy_capacity = if likely_owned_room {
        SOURCE_ENERGY_CAPACITY
    } else {
        SOURCE_ENERGY_NEUTRAL_CAPACITY
    };
    let energy_per_tick = (energy_capacity as f32) / (ENERGY_REGEN_TIME as f32);
    (energy_per_tick / (HARVEST_POWER as f32)).ceil() as usize
}
