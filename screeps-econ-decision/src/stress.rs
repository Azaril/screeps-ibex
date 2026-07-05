//! K3 — the S1 repair stress-gate decision kernel (ADR 0040 §D6), MOVED verbatim from
//! `screeps-ibex/src/energy_stress.rs` at M3. Lives here now, consumed by the bot
//! (`energy_stress.rs` keeps the ECS adapter — `EnergyLeakStats`, the snapshot plumbing — and
//! re-exports this kernel) and by the sim (`screeps-econ-eval::baseline`, whose transcribed
//! S1-arm mirror is deleted).
//!
//! **INTERIM**: the allowance gate is stopgap scaffolding, superseded by the unified e/t sink
//! market at M5a (EP-2.10; removal point tied to the market's default-on, EP-10.5). The interim
//! constants below are declared interim — the market replaces them, it does not calibrate them.
//!
//! The kernel is functionally PURE — no `game::*`/world reads, integer math only: it takes plain
//! room facts and returns an allowance.

use crate::repair::RepairPriority;

/// Stored energy (storage + terminal + containers) at or above which repair is
/// unrestricted regardless of refill deficit — a room with a real buffer is
/// not energy-stressed. Precedent: `RENEW_MIN_ROOM_ENERGY` (spawnsystem.rs).
pub const REPAIR_UNRESTRICTED_STORED_ENERGY: u32 = 10_000;

/// Maximum per-mille refill deficit at which repair stays unrestricted
/// (100 = 10%): a near-full spawn/extension network is not stressed.
pub const REPAIR_UNRESTRICTED_MAX_DEFICIT_Q: u32 = 100;

/// What repair work a room's energy posture admits.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RepairAllowance {
    /// No stress: repair at the caller's own minimum priority.
    Unrestricted,
    /// Refill-deficient with no stored buffer: only Critical repair is
    /// admitted (the energy belongs to the refill chain).
    CriticalOnly,
}

/// Per-mille refill deficit. `capacity == 0` (no spawns visible) => 0 — there
/// is no refill demand to protect.
pub fn refill_deficit_q(energy_available: u32, energy_capacity: u32) -> u32 {
    if energy_capacity == 0 {
        return 0;
    }

    let available = energy_available.min(energy_capacity);
    let filled_q = ((available as u64 * 1000) / energy_capacity as u64) as u32;

    1000u32.saturating_sub(filled_q)
}

/// The room's repair allowance from its refill deficit + stored-energy buffer.
pub fn repair_allowance(deficit_q: u32, stored_energy: u32) -> RepairAllowance {
    if stored_energy >= REPAIR_UNRESTRICTED_STORED_ENERGY || deficit_q <= REPAIR_UNRESTRICTED_MAX_DEFICIT_Q {
        RepairAllowance::Unrestricted
    } else {
        RepairAllowance::CriticalOnly
    }
}

/// Raise the caller's minimum repair priority under [`RepairAllowance::CriticalOnly`].
/// `Unrestricted` leaves the minimum unchanged; `CriticalOnly` raises it to
/// `Critical` (the enum maximum, so this only ever tightens).
pub fn effective_min_repair_priority(min: Option<RepairPriority>, allowance: RepairAllowance) -> Option<RepairPriority> {
    match allowance {
        RepairAllowance::Unrestricted => min,
        RepairAllowance::CriticalOnly => Some(RepairPriority::Critical),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Boundaries (moved with the kernel from energy_stress.rs) ────────────

    #[test]
    fn stored_exactly_at_threshold_is_unrestricted() {
        // 10k stored overrides any deficit (post-wipe deficit 1000 included).
        assert_eq!(repair_allowance(1000, REPAIR_UNRESTRICTED_STORED_ENERGY), RepairAllowance::Unrestricted);
    }

    #[test]
    fn below_stored_threshold_with_deficit_above_max_is_critical_only() {
        assert_eq!(
            repair_allowance(REPAIR_UNRESTRICTED_MAX_DEFICIT_Q + 1, REPAIR_UNRESTRICTED_STORED_ENERGY - 1),
            RepairAllowance::CriticalOnly
        );
    }

    #[test]
    fn deficit_exactly_at_max_is_unrestricted() {
        assert_eq!(repair_allowance(REPAIR_UNRESTRICTED_MAX_DEFICIT_Q, 0), RepairAllowance::Unrestricted);
    }

    #[test]
    fn zero_capacity_means_zero_deficit() {
        // No spawns visible => no refill demand to protect.
        assert_eq!(refill_deficit_q(0, 0), 0);
        assert_eq!(refill_deficit_q(500, 0), 0);
    }

    #[test]
    fn post_wipe_room_has_full_deficit() {
        assert_eq!(refill_deficit_q(0, 300), 1000);
    }

    #[test]
    fn full_room_has_zero_deficit() {
        assert_eq!(refill_deficit_q(300, 300), 0);
        assert_eq!(refill_deficit_q(12_900, 12_900), 0);
        // Clamped: available above capacity still reads as full.
        assert_eq!(refill_deficit_q(400, 300), 0);
    }

    #[test]
    fn deficit_is_per_mille_of_capacity() {
        // 90% full => 100 per-mille deficit (the unrestricted boundary).
        assert_eq!(refill_deficit_q(900, 1000), 100);
        assert_eq!(refill_deficit_q(899, 1000), 101);
        // Large capacity: u64 intermediate avoids overflow (2e9 * 1000 > u32::MAX).
        assert_eq!(refill_deficit_q(2_000_000_000, 4_000_000_000), 500);
    }

    // ── Monotonicity ────────────────────────────────────────────────────────

    #[test]
    fn more_stored_energy_never_tightens_allowance() {
        for deficit_q in (0..=1000).step_by(50) {
            let mut prev = repair_allowance(deficit_q, 0);
            for stored in (0..=20_000).step_by(500) {
                let cur = repair_allowance(deficit_q, stored);
                // Unrestricted must never regress to CriticalOnly as stored grows.
                assert!(
                    !(prev == RepairAllowance::Unrestricted && cur == RepairAllowance::CriticalOnly),
                    "tightened at deficit_q={deficit_q} stored={stored}"
                );
                prev = cur;
            }
        }
    }

    #[test]
    fn larger_deficit_never_loosens_allowance() {
        for stored in (0..=20_000).step_by(500) {
            let mut prev = repair_allowance(0, stored);
            for deficit_q in 0..=1000 {
                let cur = repair_allowance(deficit_q, stored);
                // CriticalOnly must never relax to Unrestricted as the deficit grows.
                assert!(
                    !(prev == RepairAllowance::CriticalOnly && cur == RepairAllowance::Unrestricted),
                    "loosened at deficit_q={deficit_q} stored={stored}"
                );
                prev = cur;
            }
        }
    }

    #[test]
    fn deficit_is_monotone_in_available_energy() {
        let capacity = 1300;
        let mut prev = refill_deficit_q(0, capacity);
        for available in 1..=capacity {
            let cur = refill_deficit_q(available, capacity);
            assert!(cur <= prev, "deficit grew at available={available}");
            prev = cur;
        }
    }

    // ── effective_min table ─────────────────────────────────────────────────

    #[test]
    fn effective_min_table() {
        let minimums = [
            None,
            Some(RepairPriority::VeryLow),
            Some(RepairPriority::Low),
            Some(RepairPriority::Medium),
            Some(RepairPriority::High),
            Some(RepairPriority::Critical),
        ];

        for min in minimums {
            // Unrestricted: caller's minimum passes through unchanged.
            assert_eq!(effective_min_repair_priority(min, RepairAllowance::Unrestricted), min);
            // CriticalOnly: always raised to Critical (the enum max).
            assert_eq!(
                effective_min_repair_priority(min, RepairAllowance::CriticalOnly),
                Some(RepairPriority::Critical)
            );
        }
    }
}
