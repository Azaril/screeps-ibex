use screeps::*;
// The tower attack/heal/repair falloff curve is engine MECHANICS (the ground truth); reached through
// the decision crate (single source — no duplicated f32 copy). The engine returns u32; cast at use.
use screeps_combat_decision::damage::tower_attack_damage_at_range;

// Force sizing (threat-picture → parts/bodies) lives in the decision crate, with
// `screeps_combat_decision::bodies` as the body primitives: `CombatBodySpec` (the `force_sizing`
// solver's output) built into an ordered `Vec<Part>` by `build_combat_body` under a `MoveProfile`,
// `defender_heal_parts_for_dps` (the heal-sizing inverse, used by `force_sizing` + `doctrine`),
// and the `boosts` T3 compound table. This module keeps the game-coupled
// tower-over-`Position` damage math + the defender spawn-readiness decision.

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

// (WvC-1 triage: the once-planned `net_tower_damage`/`should_towers_fire`/`estimated_ticks_to_kill`
// single-target helpers were DELETED as superseded — the U-TOWER `decide_towers` kernel
// (`screeps_combat_decision::tower_fire`) already makes the heal-aware fire decision, and better:
// it sizes the tower commit against `heal_reaching` per target and refuses out-healed dogpiles,
// where the helpers only compared one target's flat heal total.)

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

// ── Defender spawn-readiness model ───────────────────────────────────────────
//
// The spawn-now-vs-wait decision for an emergency defender, given the room's
// energy state. The part-sizing it pairs with lives in the decision crate: the
// doctrine/`force_sizing` path sizes a `bodies::CombatBodySpec` (carried as
// `composition::BodyType::Sized`), which `bodies::build_combat_body` turns into
// the spawned body.

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

/// Map a readiness verdict onto the slot spawner's energy budget (WvC-1 wiring).
///
/// `base_energy` is the spawner's normal sizing budget (strongest in-range home's
/// CAPACITY, capped at `PREFERRED_MEMBER_ENERGY`). The mapping:
///
/// - `SpawnNow(budget)` → `budget.min(base_energy)`: the URGENT branch carries the
///   room's AVAILABLE energy, so the body downsizes to what can spawn THIS tick
///   (a smaller defender now beats a perfect one too late); the refilled branch
///   carries capacity, which the min collapses back to `base_energy` — unchanged.
/// - `Wait` / `None` (not a defense slot) → `base_energy`: queueing the full-size
///   body IS the wait — the spawn system banks energy until the body's cost is
///   affordable, so a capable room on a dip refills rather than emitting a runt.
pub fn slot_build_energy(base_energy: u32, readiness: Option<SpawnReadiness>) -> u32 {
    match readiness {
        Some(SpawnReadiness::SpawnNow(budget)) => budget.min(base_energy),
        Some(SpawnReadiness::Wait) | None => base_energy,
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    // (Part-sizing tests live with the code in the decision crate: the `bodies` tests cover
    // `build_combat_body`/`MoveProfile` + `defender_heal_parts_for_dps`, and the doctrine +
    // `force_sizing`/`composition` tests cover the sizing itself.)

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
    fn slot_energy_urgent_downsizes_wait_and_offense_keep_base() {
        // URGENT carries available (250) ⇒ the body downsizes below the 5600 base.
        assert_eq!(slot_build_energy(5600, Some(SpawnReadiness::SpawnNow(250))), 250);
        // Refilled carries capacity ⇒ min collapses to base (unchanged sizing).
        assert_eq!(slot_build_energy(5600, Some(SpawnReadiness::SpawnNow(5600))), 5600);
        // Wait and non-defense (None) both keep the base bank-to-capacity queue.
        assert_eq!(slot_build_energy(5600, Some(SpawnReadiness::Wait)), 5600);
        assert_eq!(slot_build_energy(5600, None), 5600);
        // A budget above base (capacity > PREFERRED cap) never inflates past base.
        assert_eq!(slot_build_energy(2400, Some(SpawnReadiness::SpawnNow(5600))), 2400);
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
