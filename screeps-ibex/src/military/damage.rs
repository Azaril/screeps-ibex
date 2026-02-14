use screeps::*;

/// Tower attack damage at a given range.
///
/// - Range 0..=5: 600 damage (maximum)
/// - Range 6..=20: linear falloff from 600 to 150
/// - Range 21+: 150 damage (minimum)
pub fn tower_attack_damage_at_range(range: u32) -> f32 {
    if range <= 5 {
        600.0
    } else if range >= 20 {
        150.0
    } else {
        // Linear interpolation between range 5 (600) and range 20 (150).
        let t = (range - 5) as f32 / 15.0;
        600.0 - t * 450.0
    }
}

/// Tower heal power at a given range.
///
/// - Range 0..=5: 400 heal (maximum)
/// - Range 6..=20: linear falloff from 400 to 100
/// - Range 21+: 100 heal (minimum)
pub fn tower_heal_at_range(range: u32) -> f32 {
    if range <= 5 {
        400.0
    } else if range >= 20 {
        100.0
    } else {
        let t = (range - 5) as f32 / 15.0;
        400.0 - t * 300.0
    }
}

/// Tower repair power at a given range.
///
/// - Range 0..=5: 800 repair (maximum)
/// - Range 6..=20: linear falloff from 800 to 200
/// - Range 21+: 200 repair (minimum)
pub fn tower_repair_at_range(range: u32) -> f32 {
    if range <= 5 {
        800.0
    } else if range >= 20 {
        200.0
    } else {
        let t = (range - 5) as f32 / 15.0;
        800.0 - t * 600.0
    }
}

/// Calculate total tower damage from multiple towers against a target at a given position.
pub fn total_tower_damage(tower_positions: &[Position], target_pos: Position) -> f32 {
    tower_positions
        .iter()
        .map(|tp| {
            let range = tp.get_range_to(target_pos);
            tower_attack_damage_at_range(range)
        })
        .sum()
}

/// Calculate net damage (tower damage minus enemy healing) for a target.
/// Returns positive if towers can overcome healing, negative if not.
pub fn net_tower_damage(tower_positions: &[Position], target_pos: Position, enemy_heal_per_tick: f32) -> f32 {
    total_tower_damage(tower_positions, target_pos) - enemy_heal_per_tick
}

/// Determine if towers should fire at a target, considering the enemy's healing capability.
/// Only fire if net damage is positive (we can actually hurt them).
pub fn should_towers_fire(tower_positions: &[Position], target_pos: Position, enemy_heal_per_tick: f32) -> bool {
    net_tower_damage(tower_positions, target_pos, enemy_heal_per_tick) > 0.0
}

/// Check if a hostile creep at the room edge is likely performing a tower drain attack.
/// Tower drain: hostile sits at max range (edge), heals through tower damage to waste energy.
pub fn is_likely_tower_drain(target_pos: Position, target_heal_per_tick: f32, tower_positions: &[Position]) -> bool {
    let x = target_pos.x().u8();
    let y = target_pos.y().u8();

    // Check if near room edge (within 3 tiles of border).
    let near_edge = x <= 3 || x >= 46 || y <= 3 || y >= 46;

    if !near_edge {
        return false;
    }

    // If the target can heal through all tower damage, it's a drain.
    let total_damage = total_tower_damage(tower_positions, target_pos);
    target_heal_per_tick >= total_damage
}

/// Estimate how many ticks it would take for towers to kill a creep,
/// given the creep's total HP, healing, and the tower damage at its position.
/// Returns `None` if towers cannot overcome healing.
pub fn estimated_ticks_to_kill(
    tower_positions: &[Position],
    target_pos: Position,
    target_hits: u32,
    target_heal_per_tick: f32,
) -> Option<u32> {
    let net = net_tower_damage(tower_positions, target_pos, target_heal_per_tick);
    if net <= 0.0 {
        return None;
    }
    Some((target_hits as f32 / net).ceil() as u32)
}

/// Calculate the range between two positions, handling same-room only.
pub fn range_between(a: Position, b: Position) -> u32 {
    a.get_range_to(b)
}
