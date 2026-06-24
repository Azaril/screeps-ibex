use screeps::*;
// The tower attack/heal/repair falloff curve is engine MECHANICS (the ground truth); reached through
// the decision crate (single source — no duplicated f32 copy). The engine returns u32; cast at use.
use screeps_combat_decision::damage::tower_attack_damage_at_range;

// The threat-picture → part-count helpers (`drain_heal_parts_for_dps`, `attack_parts_to_kill`,
// `KILL_WINDOW_TICKS`, `MAX_OFFENSE_PARTS`, the sized template bodies) moved to
// `screeps_combat_decision::bodies` (the shared force-sizing body layer). This module keeps the
// game-coupled tower-over-`Position` damage math + the defender spawn-readiness decision.

/// Tower DPS at a typical drain position (room edge, north side).
/// Drains sit at the edge to maximize range from towers; this approximates that.
pub fn tower_dps_at_room_edge(room_name: RoomName, tower_positions: &[Position]) -> f32 {
    let edge_pos = Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(0).unwrap(), room_name);
    total_tower_damage(tower_positions, edge_pos)
}

/// Calculate total tower damage from multiple towers against a target at a given position.
pub fn total_tower_damage(tower_positions: &[Position], target_pos: Position) -> f32 {
    tower_positions
        .iter()
        .map(|tp| {
            let range = tp.get_range_to(target_pos);
            tower_attack_damage_at_range(range) as f32
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

// ── Defender spawn-readiness model ───────────────────────────────────────────
//
// The spawn-now-vs-wait decision for an emergency defender, given the room's
// energy state. The part-sizing it pairs with (`attack_parts_to_kill` /
// `sized_defender_body`) lives in `screeps_combat_decision::bodies`.

/// Fraction of a room's MAX spawn energy that must currently be AVAILABLE before
/// we size a defender to full capacity (rather than holding for refill). Keeps a
/// capable room on a momentary energy dip from emitting an under-strength creep.
/// Overridden by the urgent branch when nothing is holding the line.
pub const WAIT_REFILL_FRACTION: f32 = 0.85;

/// Outcome of the spawn-now-vs-wait decision. `SpawnNow(budget)` carries the
/// energy budget to size the body against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnReadiness {
    SpawnNow(u32),
    Wait,
}

/// Decide whether to spawn a defender NOW and at what energy budget. Pure — the
/// caller passes `has_friendly_tower` and `defender_alive` so this stays
/// game-call-free and host-testable.
///
/// - **Urgent** (under attack, nothing holding the line, no tower buying time):
///   spawn immediately from CURRENT energy — a smaller defender now beats a
///   perfect one too late.
/// - **Refilled enough** (`available ≥ WAIT_REFILL_FRACTION × capacity`): spawn
///   a full-strength body sized to capacity.
/// - **Otherwise** (a capable room on a momentary dip, or a tower is covering):
///   wait for refill rather than emit a runt.
pub fn defender_spawn_readiness(
    available: u32,
    capacity: u32,
    incoming_dps: f32,
    has_friendly_tower: bool,
    defender_alive: bool,
) -> SpawnReadiness {
    let urgent = incoming_dps > 0.0 && !defender_alive && !has_friendly_tower;
    if urgent {
        SpawnReadiness::SpawnNow(available)
    } else if available as f32 >= WAIT_REFILL_FRACTION * capacity.max(1) as f32 {
        SpawnReadiness::SpawnNow(capacity)
    } else {
        SpawnReadiness::Wait
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    // (The part-sizing tests — `attack_parts_to_kill`, `defender_heal_parts_for_dps` — moved with the
    // code to `screeps_combat_decision::bodies`.)

    #[test]
    fn readiness_urgent_uses_available() {
        // Towerless, nothing holding the line, under attack ⇒ spawn now from the bank.
        assert_eq!(
            defender_spawn_readiness(250, 5600, 120.0, false, false),
            SpawnReadiness::SpawnNow(250)
        );
    }

    #[test]
    fn readiness_capable_room_on_a_dip_waits() {
        // RCL7, a defender already holding, 900/5600 (<85%) ⇒ wait, don't emit a runt.
        assert_eq!(defender_spawn_readiness(900, 5600, 120.0, false, true), SpawnReadiness::Wait);
        // A tower buying time also means we wait even with no defender yet.
        assert_eq!(defender_spawn_readiness(900, 5600, 120.0, true, false), SpawnReadiness::Wait);
    }

    #[test]
    fn readiness_refilled_uses_capacity() {
        // ≥85% available ⇒ full-strength body sized to capacity.
        assert_eq!(
            defender_spawn_readiness(5040, 5600, 120.0, false, true),
            SpawnReadiness::SpawnNow(5600)
        );
    }
}
