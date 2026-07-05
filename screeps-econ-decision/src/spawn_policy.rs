//! K4 — the spawn-request policy for the localsupply roles: body shapes, sizing, and
//! priority-band selection. MOVED at ADR 0040 M3 from `screeps-ibex/src/spawnsystem.rs` (the
//! `SPAWN_PRIORITY_*` bands), `missions/localsupply/body_helpers.rs` (`harvester_body`),
//! `missions/localsupply/source_mining.rs` (the harvester energy/priority arms),
//! `missions/haul.rs` (the hauler body/desired/priority arms), `missions/upgrade.rs`
//! (`work_parts_for_upkeep`, the upgrader sizing/priority arms) and `missions/localbuild.rs`
//! (the builder tables + the repairer arm + the builder body cap). Lives here now, consumed by
//! the bot missions (which keep their alive-count/ECS bookkeeping and pass plain facts) and by
//! the sim (`screeps-econ-eval::baseline::spawn_requests`, whose transcriptions are deleted).
//!
//! Body EXPANSION stays the already-shared `screeps_combat_decision::spawning::create_body`
//! (one implementation; the bot re-exports it as `crate::creep::spawning::create_body`) — this
//! module owns the body *definitions* and the *policy* numbers.
//!
//! The S6 defect (capacity-sized replacement bodies head-of-line-banking trickle income) is
//! deliberately preserved — extracted faithfully; M4 owns the fix.

use crate::repair::RepairPriority;
use screeps::Part;
use screeps_combat_decision::spawning::SpawnBodyDefinition;

// ── The spawn priority bands (spawnsystem.rs — re-exported by the bot) ───────────────────────

pub const SPAWN_PRIORITY_CRITICAL: f32 = 100.0;
/// A band STRICTLY above the HIGH economy bulk but STRICTLY below the CRITICAL miners, reserved
/// for the slots of a FORMING active offense/defense combat squad (the rally-stall fix — see
/// the spawnsystem head-of-line note).
pub const SPAWN_PRIORITY_COMBAT_FORMING: f32 = 85.0;
pub const SPAWN_PRIORITY_HIGH: f32 = 75.0;
pub const SPAWN_PRIORITY_MEDIUM: f32 = 50.0;
pub const SPAWN_PRIORITY_LOW: f32 = 25.0;
pub const SPAWN_PRIORITY_NONE: f32 = 0.0;

/// Bounded lerp between two bands (the live `lerp::Lerp::lerp_bounded` on f32).
pub fn lerp_bounded(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// A role tag + body + priority — one K4 spawn request (the `RequestSpawn` intent payload).
/// The `body` is fully expanded (via the shared `create_body`); adapters attach their own
/// callbacks/tokens.
#[derive(Clone, Debug)]
pub struct SpawnPlan {
    pub body: Vec<Part>,
    pub priority: f32,
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Harvesters (source_mining.rs + body_helpers.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The harvester body definition ([M,M,C,W] × 1..=5 within `energy`) — body_helpers.rs verbatim.
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

/// The harvester body budget: the FIRST harvester (no harvesting creeps anywhere) sizes from
/// available-now energy (floored at the 300 spawn), every replacement from capacity — the S6
/// arm, preserved (source_mining.rs).
pub fn harvester_body_energy(total_harvesting_creeps: usize, energy_available: u32, energy_capacity: u32) -> u32 {
    if total_harvesting_creeps == 0 {
        energy_available.max(300)
    } else {
        energy_capacity
    }
}

/// The per-source desired harvester count (source_mining.rs `desired_harvesters`).
pub const DESIRED_HARVESTERS_PER_SOURCE: usize = 4;

/// The harvester priority: lerped across the (range-start, range-end) band for the home's
/// Manhattan room distance — local (CRITICAL→HIGH), adjacent (MEDIUM→NONE), far (LOW→NONE)
/// (source_mining.rs).
pub fn harvester_priority(current: usize, desired: usize, room_manhattan_distance: u32) -> f32 {
    let priority_range = if room_manhattan_distance == 0 {
        (SPAWN_PRIORITY_CRITICAL, SPAWN_PRIORITY_HIGH)
    } else if room_manhattan_distance <= 1 {
        (SPAWN_PRIORITY_MEDIUM, SPAWN_PRIORITY_NONE)
    } else {
        (SPAWN_PRIORITY_LOW, SPAWN_PRIORITY_NONE)
    };
    let interp = (current as f32) / (desired as f32);
    lerp_bounded(priority_range.0, priority_range.1, interp)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Haulers (missions/haul.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The hauler body definition: [C,M] × 1..=20; multi-room haulers prepend [W,M] (the road-
/// repair work part) — missions/haul.rs verbatim.
pub fn hauler_body(is_multi_room: bool, energy: u32) -> SpawnBodyDefinition<'static> {
    if is_multi_room {
        SpawnBodyDefinition {
            maximum_energy: energy,
            minimum_repeat: Some(1),
            maximum_repeat: Some(20),
            pre_body: &[Part::Work, Part::Move],
            repeat_body: &[Part::Carry, Part::Move],
            post_body: &[],
        }
    } else {
        SpawnBodyDefinition {
            maximum_energy: energy,
            minimum_repeat: Some(1),
            maximum_repeat: Some(20),
            pre_body: &[],
            repeat_body: &[Part::Carry, Part::Move],
            post_body: &[],
        }
    }
}

/// The hauler demand sizing (missions/haul.rs): `range_multiplier = 1/((max_distance·2)+1)`,
/// `base = carry_parts × CARRY_CAPACITY × multiplier`, desired-for-unfulfilled =
/// `unfulfilled / base` (f32 truncation, live verbatim), capped at `3 + max_distance·3`.
/// Returns `(desired_for_unfulfilled, desired_capped)`.
pub fn hauler_desired(unfulfilled_hauling: u32, carry_parts: u32, max_distance: u32) -> (u32, usize) {
    let range_multiplier = 1.0 / ((max_distance as f32 * 2.0) + 1.0);
    let base_amount = carry_parts as f32 * 50.0 * range_multiplier;
    let max_haulers = 3 + (max_distance * 3);
    let desired_for_unfulfilled = (unfulfilled_hauling as f32 / base_amount) as u32;
    let desired = desired_for_unfulfilled.min(max_haulers) as usize;
    (desired_for_unfulfilled, desired)
}

/// The hauler priority bands (missions/haul.rs): below 75% of the unfulfilled-desired count →
/// the urgent band (HIGH local / MEDIUM remote), else the relaxed band (MEDIUM local / LOW
/// remote).
pub fn hauler_priority(current: usize, desired_for_unfulfilled: u32, max_distance: u32) -> f32 {
    if (current as f32) < (desired_for_unfulfilled as f32 * 0.75).ceil() {
        if max_distance == 0 {
            SPAWN_PRIORITY_HIGH
        } else {
            SPAWN_PRIORITY_MEDIUM
        }
    } else if max_distance == 0 {
        SPAWN_PRIORITY_MEDIUM
    } else {
        SPAWN_PRIORITY_LOW
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Upgraders (missions/upgrade.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Engine constants used by the upkeep model (each cited in engine-mechanics.md; the live code
/// read them from screeps-game-api).
const CONTROLLER_DOWNGRADE_RESTORE: u32 = 100;
const CONTROLLER_MAX_UPGRADE_PER_TICK: u32 = 15;
const CREEP_SPAWN_TIME: u32 = 3;
const CREEP_LIFE_TIME: u32 = 1500;
const CARRY_CAPACITY: u32 = 50;
const UPGRADE_CONTROLLER_POWER: u32 = 1;

/// The minimum WORK parts for an upkeep upgrader to restore the downgrade clock from
/// `current_ttd` back to `max_ticks / 2` within one lifetime (missions/upgrade.rs verbatim —
/// the f64 arithmetic kept; the result is a body size, never a per-tick branch).
pub fn work_parts_for_upkeep(current_ttd: u32, max_ticks: u32) -> usize {
    let safe_threshold = max_ticks / 2;
    if current_ttd >= safe_threshold {
        return 1;
    }
    let deficit = (safe_threshold - current_ttd) as f64;
    let net_restore_per_upgrade_tick = (CONTROLLER_DOWNGRADE_RESTORE as f64) - 1.0;

    for w in 1..=CONTROLLER_MAX_UPGRADE_PER_TICK {
        let body_parts = w + 3;
        let spawn_ticks = body_parts * CREEP_SPAWN_TIME;
        let lifetime = CREEP_LIFE_TIME.saturating_sub(spawn_ticks) as f64;

        let carry_cap = CARRY_CAPACITY as f64;
        let upgrade_ticks_per_cycle = (carry_cap / w as f64).floor();
        if upgrade_ticks_per_cycle < 1.0 {
            continue;
        }
        let cycle_ticks = upgrade_ticks_per_cycle;
        let net_per_cycle = upgrade_ticks_per_cycle * net_restore_per_upgrade_tick;
        if net_per_cycle <= 0.0 {
            continue;
        }

        let cycles = (lifetime / cycle_ticks).floor();
        let total_restore = cycles * net_per_cycle;

        if total_restore >= deficit {
            return w as usize;
        }
    }

    CONTROLLER_MAX_UPGRADE_PER_TICK as usize
}

/// The upgrader roster cap (missions/upgrade.rs): governor-unwilling / hostiles / max-level → 1;
/// excess energy → 5 (RCL ≤ 3) or 3; else 1.
pub fn max_upgraders(governor_willing: bool, hostile_creeps: bool, at_max_level: bool, has_excess_energy: bool, rcl: u8) -> usize {
    if !governor_willing {
        return 1;
    }
    if hostile_creeps || at_max_level {
        1
    } else if has_excess_energy {
        if rcl <= 3 {
            5
        } else {
            3
        }
    } else {
        1
    }
}

/// The upgrader WORK sizing (missions/upgrade.rs): downgrade-risk first-body sized to save the
/// clock; otherwise the max-level cap split / 20-with-excess / half the source potential.
pub fn upgrader_work_parts(
    downgrade_upkeep_parts: Option<usize>,
    roster_empty: bool,
    at_max_level: bool,
    has_excess_energy: bool,
    source_count: usize,
    max_upgraders: usize,
) -> Option<usize> {
    if let Some(upkeep_parts) = downgrade_upkeep_parts {
        if roster_empty {
            Some(upkeep_parts)
        } else {
            let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);
            Some((work_parts_per_tick / (max_upgraders as f32)).ceil() as usize)
        }
    } else if at_max_level {
        let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);
        Some((work_parts_per_tick / (max_upgraders as f32)).ceil() as usize)
    } else if has_excess_energy {
        Some(20)
    } else {
        // Half the room's source potential, split across upgraders (3000/300 e/t per source).
        let energy_per_second = ((3000 * source_count as u32) as f32) / 300.0;
        let upgrade_per_second = energy_per_second / (UPGRADE_CONTROLLER_POWER as f32);
        Some(((upgrade_per_second / 2.0) / max_upgraders as f32).floor().max(1.0) as usize)
    }
}

/// The upgrader body definition (missions/upgrade.rs): RCL ≤ 3 → pre `[W,C,M,M]`, repeat
/// `[W,M]` × 0..=work_parts; RCL > 3 → pre `[W,C,M,M]`, repeat `[W]` × 1..=(work_parts − 1).
pub fn upgrader_body(rcl: u8, maximum_energy: u32, work_parts: Option<usize>) -> SpawnBodyDefinition<'static> {
    if rcl <= 3 {
        SpawnBodyDefinition {
            maximum_energy,
            minimum_repeat: Some(0),
            maximum_repeat: work_parts,
            pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
            repeat_body: &[Part::Work, Part::Move],
            post_body: &[],
        }
    } else {
        SpawnBodyDefinition {
            maximum_energy,
            minimum_repeat: Some(1),
            maximum_repeat: work_parts.map(|p| p.saturating_sub(1)),
            pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
            repeat_body: &[Part::Work],
            post_body: &[],
        }
    }
}

/// The upgrader priority bands (missions/upgrade.rs): downgrade-risk-with-empty-roster
/// CRITICAL; empty roster HIGH; excess+storage lerp HIGH→MEDIUM; multi lerp MEDIUM→LOW; else
/// MEDIUM.
pub fn upgrader_priority(
    downgrade_risk: bool,
    roster_empty: bool,
    has_excess_energy: bool,
    has_storage: bool,
    max_upgraders: usize,
    alive_upgraders: usize,
) -> f32 {
    if downgrade_risk && roster_empty {
        SPAWN_PRIORITY_CRITICAL
    } else if roster_empty {
        SPAWN_PRIORITY_HIGH
    } else if has_excess_energy && has_storage && max_upgraders > 1 {
        let interp = (alive_upgraders as f32) / ((max_upgraders - 1) as f32);
        lerp_bounded(SPAWN_PRIORITY_HIGH, SPAWN_PRIORITY_MEDIUM, interp)
    } else if max_upgraders > 1 {
        let interp = (alive_upgraders as f32) / ((max_upgraders - 1) as f32);
        lerp_bounded(SPAWN_PRIORITY_MEDIUM, SPAWN_PRIORITY_LOW, interp)
    } else {
        SPAWN_PRIORITY_MEDIUM
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Builders + the repairer arm (missions/localbuild.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The builder count table by pending construction progress + RCL band
/// (missions/localbuild.rs).
pub fn builder_desired_for_progress(rcl: u8, required_progress: u32) -> u32 {
    if rcl <= 3 {
        match required_progress {
            0 => 0,
            1..=1000 => 1,
            1001..=2000 => 2,
            2001..=3000 => 3,
            3001..=4000 => 4,
            _ => 5,
        }
    } else if rcl <= 6 {
        match required_progress {
            0 => 0,
            1..=2000 => 1,
            2001..=4000 => 2,
            4001..=6000 => 3,
            _ => 4,
        }
    } else {
        match required_progress {
            0 => 0,
            1..=3000 => 1,
            3001..=6000 => 2,
            6001..=9000 => 3,
            _ => 4,
        }
    }
}

/// The first-builder priority: (HIGH + MEDIUM) / 2 (missions/localbuild.rs).
pub const FIRST_BUILDER_PRIORITY: f32 = (SPAWN_PRIORITY_HIGH + SPAWN_PRIORITY_MEDIUM) / 2.0;

/// The with-builders priority: HIGH iff any spawn/storage site is pending, else MEDIUM
/// (missions/localbuild.rs — the per-site max collapses to this).
pub fn builder_priority_with_builders(any_spawn_or_storage_site: bool) -> f32 {
    if any_spawn_or_storage_site {
        SPAWN_PRIORITY_HIGH
    } else {
        SPAWN_PRIORITY_MEDIUM
    }
}

/// The repairer-builder arm (missions/localbuild.rs `get_repairer_priority` tail): the queue's
/// best candidate at the allowance-raised minimum decides — ≥ High → (1, HIGH); ≥ Medium →
/// (1, MEDIUM); else none.
pub fn repairer_spawn_priority(best_candidate: RepairPriority) -> Option<(u32, f32)> {
    if best_candidate >= RepairPriority::High {
        Some((1, SPAWN_PRIORITY_HIGH))
    } else if best_candidate >= RepairPriority::Medium {
        Some((1, SPAWN_PRIORITY_MEDIUM))
    } else {
        None
    }
}

/// The builder body definition (missions/localbuild.rs): repeat `[C,W,M,M]` × 1.., capped at 5
/// repeats below HIGH priority, uncapped at ≥ HIGH.
pub fn builder_body(maximum_energy: u32, priority: f32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy,
        minimum_repeat: Some(1),
        maximum_repeat: if priority >= SPAWN_PRIORITY_HIGH { None } else { Some(5) },
        pre_body: &[],
        repeat_body: &[Part::Carry, Part::Work, Part::Move, Part::Move],
        post_body: &[],
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Energy posture thresholds (missions/upgrade.rs `has_excess_energy` / missions/localbuild.rs
// `has_sufficient_energy`).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The desired storage buffer the excess/sufficient fractions divide
/// (missions/constants.rs `get_desired_storage_amount(Energy)`).
pub const DESIRED_STORAGE_ENERGY: u32 = 200_000;
/// Container capacity (engine constant; the fraction ladders' denominator).
pub const CONTAINER_CAPACITY: u32 = 2000;

/// `has_excess_energy` (missions/upgrade.rs): storage present → Σ storage energy ≥ 100k; else
/// containers present → ANY container > 75% full; else TRUE (a bare room reads "excess").
/// `container_energies` are per-container energy amounts.
pub fn has_excess_energy(storage_present: bool, total_storage_energy: u32, container_energies: &[u32]) -> bool {
    if storage_present {
        total_storage_energy >= DESIRED_STORAGE_ENERGY / 2
    } else if !container_energies.is_empty() {
        container_energies.iter().any(|&e| e as u64 * 100 > CONTAINER_CAPACITY as u64 * 75)
    } else {
        true
    }
}

/// `has_sufficient_energy` (missions/localbuild.rs): storage present → ANY storage ≥ 50k; else
/// ANY container > 50% full (an empty candidate set is false — the greenfield RCL-1 room reads
/// insufficient). `storage_energies` are per-storage energy amounts.
pub fn has_sufficient_energy(storage_present: bool, storage_energies: &[u32], container_energies: &[u32]) -> bool {
    if storage_present {
        storage_energies.iter().any(|&e| e >= DESIRED_STORAGE_ENERGY / 4)
    } else {
        container_energies.iter().any(|&e| e as u64 * 100 > CONTAINER_CAPACITY as u64 * 50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_combat_decision::spawning::create_body;

    /// The harvester arm end-to-end shapes (pre-move fixtures, source_mining.rs +
    /// body_helpers.rs): bootstrap available-sized, replacement capacity-sized (S6),
    /// CRITICAL→HIGH lerp.
    #[test]
    fn harvester_policy_matches_live() {
        assert_eq!(harvester_body_energy(0, 250, 800), 300, "bootstrap floors at the bare spawn");
        assert_eq!(harvester_body_energy(0, 450, 800), 450);
        assert_eq!(harvester_body_energy(1, 450, 800), 800, "replacement is capacity-sized — S6 preserved");
        let body = create_body(&harvester_body(300)).unwrap();
        assert_eq!(body.len(), 4, "1 repeat of [M,M,C,W] at 300");
        let body = create_body(&harvester_body(800)).unwrap();
        assert_eq!(body.len(), 12, "3 repeats at 800");
        assert_eq!(harvester_priority(0, 4, 0), SPAWN_PRIORITY_CRITICAL);
        assert!((harvester_priority(1, 4, 0) - 93.75).abs() < 1e-6, "1/4 lerp toward HIGH");
        assert_eq!(harvester_priority(0, 4, 1), SPAWN_PRIORITY_MEDIUM, "adjacent-room band");
        assert_eq!(harvester_priority(0, 4, 2), SPAWN_PRIORITY_LOW, "far band");
    }

    /// The hauler arm (pre-move fixtures, missions/haul.rs): local body, desired sizing,
    /// priority bands incl. the remote arms.
    #[test]
    fn hauler_policy_matches_live() {
        let body = create_body(&hauler_body(false, 300)).unwrap();
        assert_eq!(body.len(), 6, "3 repeats of [C,M] at 300");
        let body = create_body(&hauler_body(true, 300)).unwrap();
        assert_eq!(body.iter().filter(|p| **p == Part::Work).count(), 1, "multi-room prepends [W,M]");
        // 800 unfulfilled / (3 carry × 50) = 5 → desired for unfulfilled 5, capped at 3 local.
        assert_eq!(hauler_desired(800, 3, 0), (5, 3));
        // Remote distance 1: multiplier 1/3 → base 50, 800/50 = 16, cap 6.
        assert_eq!(hauler_desired(800, 3, 1), (16, 6));
        assert_eq!(hauler_priority(0, 5, 0), SPAWN_PRIORITY_HIGH);
        assert_eq!(hauler_priority(4, 5, 0), SPAWN_PRIORITY_MEDIUM, "≥ ceil(75%) of desired");
        assert_eq!(hauler_priority(0, 5, 1), SPAWN_PRIORITY_MEDIUM, "remote urgent band");
        assert_eq!(hauler_priority(4, 5, 1), SPAWN_PRIORITY_LOW, "remote relaxed band");
    }

    /// The upkeep sizing (pre-move fixture, missions/upgrade.rs): at/above half-max → 1 WORK;
    /// every realizable deficit fits in 1 WORK (the live loop exists for the parameter shape).
    #[test]
    fn work_parts_for_upkeep_matches_live_math() {
        assert_eq!(work_parts_for_upkeep(10_000, 20_000), 1, "at the safe threshold: 1");
        assert_eq!(work_parts_for_upkeep(2_000, 20_000), 1, "RCL-3 at 10%");
        assert_eq!(work_parts_for_upkeep(0, 200_000), 1, "even the RCL-8 full deficit");
    }

    /// The upgrader bodies (pre-move fixtures, missions/upgrade.rs).
    #[test]
    fn upgrader_bodies_match_live_definitions() {
        let b = create_body(&upgrader_body(3, 300, Some(10))).unwrap();
        assert_eq!(b, vec![Part::Work, Part::Carry, Part::Move, Part::Move], "min repeat 0 at the floor");
        let b = create_body(&upgrader_body(3, 800, Some(10))).unwrap();
        assert_eq!(b.iter().filter(|p| **p == Part::Work).count(), 4, "3 repeats of [W,M] within 800");
        let b = create_body(&upgrader_body(4, 800, Some(20))).unwrap();
        assert_eq!(b.iter().filter(|p| **p == Part::Work).count(), 6, "pre W + 5 repeat W within 800");
        assert!(create_body(&upgrader_body(4, 300, Some(20))).is_err(), "RCL>3 needs ≥ 350 (min repeat 1)");
    }

    /// The upgrader roster/sizing/priority arms (pre-move fixtures, missions/upgrade.rs).
    #[test]
    fn upgrader_policy_matches_live() {
        assert_eq!(max_upgraders(false, false, false, true, 2), 1, "governor-unwilling caps at 1");
        assert_eq!(max_upgraders(true, true, false, true, 2), 1);
        assert_eq!(max_upgraders(true, false, true, true, 8), 1);
        assert_eq!(max_upgraders(true, false, false, true, 3), 5);
        assert_eq!(max_upgraders(true, false, false, true, 4), 3);
        assert_eq!(max_upgraders(true, false, false, false, 3), 1);

        assert_eq!(upgrader_work_parts(Some(3), true, false, false, 2, 1), Some(3), "clock-saving first body");
        assert_eq!(upgrader_work_parts(Some(3), false, false, false, 2, 3), Some(5), "replacement: ceil(15/3)");
        assert_eq!(upgrader_work_parts(None, true, true, false, 2, 1), Some(15), "max-level cap");
        assert_eq!(upgrader_work_parts(None, true, false, true, 2, 3), Some(20), "excess");
        // 2 sources: 20 e/t, half = 10, / 1 upgrader = 10.
        assert_eq!(upgrader_work_parts(None, true, false, false, 2, 1), Some(10));

        assert_eq!(upgrader_priority(true, true, false, false, 1, 0), SPAWN_PRIORITY_CRITICAL);
        assert_eq!(upgrader_priority(false, true, true, true, 3, 0), SPAWN_PRIORITY_HIGH, "empty roster");
        assert_eq!(upgrader_priority(false, false, true, true, 3, 2), SPAWN_PRIORITY_MEDIUM, "full lerp");
        assert_eq!(upgrader_priority(false, false, false, false, 1, 1), SPAWN_PRIORITY_MEDIUM);
    }

    /// The builder tables + the repairer arm + the body cap (pre-move fixtures,
    /// missions/localbuild.rs).
    #[test]
    fn builder_policy_matches_live() {
        assert_eq!(builder_desired_for_progress(3, 0), 0);
        assert_eq!(builder_desired_for_progress(3, 3000), 3);
        assert_eq!(builder_desired_for_progress(3, 4001), 5);
        assert_eq!(builder_desired_for_progress(5, 3000), 2);
        assert_eq!(builder_desired_for_progress(8, 3000), 1);
        assert_eq!(FIRST_BUILDER_PRIORITY, 62.5);
        assert_eq!(builder_priority_with_builders(true), SPAWN_PRIORITY_HIGH);
        assert_eq!(builder_priority_with_builders(false), SPAWN_PRIORITY_MEDIUM);

        assert_eq!(repairer_spawn_priority(RepairPriority::Critical), Some((1, SPAWN_PRIORITY_HIGH)));
        assert_eq!(repairer_spawn_priority(RepairPriority::High), Some((1, SPAWN_PRIORITY_HIGH)));
        assert_eq!(repairer_spawn_priority(RepairPriority::Medium), Some((1, SPAWN_PRIORITY_MEDIUM)));
        assert_eq!(repairer_spawn_priority(RepairPriority::Low), None);

        let b = create_body(&builder_body(10_000, SPAWN_PRIORITY_MEDIUM)).unwrap();
        assert_eq!(b.len(), 20, "5 repeats × 4 parts below HIGH");
        let b = create_body(&builder_body(10_000, SPAWN_PRIORITY_HIGH)).unwrap();
        assert!(b.len() > 20, "≥ HIGH: uncapped repeats");
    }

    /// The excess/sufficient thresholds incl. the bare-room split (pre-move fixtures).
    #[test]
    fn excess_and_sufficient_energy_thresholds() {
        assert!(has_excess_energy(false, 0, &[]), "bare room: excess TRUE");
        assert!(!has_sufficient_energy(false, &[], &[]), "bare room: sufficient FALSE");
        assert!(!has_excess_energy(true, 99_999, &[]));
        assert!(has_excess_energy(true, 100_000, &[]));
        assert!(has_sufficient_energy(true, &[50_000], &[]));
        assert!(!has_sufficient_energy(true, &[49_999], &[]));
        assert!(!has_excess_energy(false, 0, &[1500]), "exactly 75% is NOT > 75%");
        assert!(has_excess_energy(false, 0, &[1501]));
        assert!(has_sufficient_energy(false, &[], &[1001]));
        assert!(!has_sufficient_energy(false, &[], &[1000]), "exactly 50% is NOT > 50%");
    }
}
