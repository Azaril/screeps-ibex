//! K3 (repair-priority half) — the repair priority vocabulary, the health-fraction priority
//! maps, the repair-queue ordering, and the per-tick repair-energy pricing. MOVED at ADR 0040 M3
//! from `screeps-ibex/src/jobs/utility/repair.rs` (`RepairPriority`, `map_normal_priority`,
//! `map_high_value_priority`), `screeps-ibex/src/repairqueue.rs` (the `(priority, lowest
//! hp-fraction)` best-target ordering) and `screeps-ibex/src/jobs/utility/repairbehavior.rs`
//! (`repair_energy_consumed`). Lives here now, consumed by the bot (re-exported from those
//! modules) and by the sim (`screeps-econ-eval::baseline`, whose transcription mirrors are
//! deleted).
//!
//! **Arithmetic note (documented determinism deviation):** the live maps compared `f32`
//! health fractions; this kernel compares exact integer cross-products. For every reachable
//! `hits_max` (≤ 2^24 for all mapped structure classes — roads 5k/25k, containers 250k, spawns
//! 5k, towers 3k) the two are bit-identical: an f32 quotient of exactly-representable u32s can
//! only cross a quarter/percent boundary when the exact ratio does. Same policy, fence-safe
//! arithmetic (the sim baseline's M1 convention).

/// The repair priority ladder (Ord: Critical highest).
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

/// Roads (and every structure without a special arm): <25% → High, <50% → Medium, <75% → Low,
/// else VeryLow. Exact-integer form of the live quarter thresholds:
/// `hits/hits_max < k/4 ⟺ 4·hits < k·hits_max`.
pub fn map_normal_priority(hits: u32, hits_max: u32) -> RepairPriority {
    let (h, m) = (hits as u64 * 4, hits_max as u64);
    if h < m {
        RepairPriority::High
    } else if h < 2 * m {
        RepairPriority::Medium
    } else if h < 3 * m {
        RepairPriority::Low
    } else {
        RepairPriority::VeryLow
    }
}

/// High-value structures (spawns/towers/containers): <50% → Critical, <75% → High, <95% → Low,
/// else VeryLow.
pub fn map_high_value_priority(hits: u32, hits_max: u32) -> RepairPriority {
    let (h, m) = (hits as u64 * 100, hits_max as u64);
    if h < 50 * m {
        RepairPriority::Critical
    } else if h < 75 * m {
        RepairPriority::High
    } else if h < 95 * m {
        RepairPriority::Low
    } else {
        RepairPriority::VeryLow
    }
}

/// The repair-queue best-target ordering: priority first, then the LOWEST hp fraction (more
/// damaged wins). Fractions compare as exact rationals: on equal priority, `a` beats `b` iff
/// `hits_a · max_b < hits_b · max_a`. A `max_hits == 0` entry reads as fraction 1.0 ("not
/// damaged") — the live queue's division-by-zero guard, preserved: it loses to any genuinely
/// damaged structure at equal priority.
///
/// Use with `max_by`: the greater element under this ordering is the better target.
pub fn repair_target_order(
    a: (RepairPriority, u32, u32),
    b: (RepairPriority, u32, u32),
) -> std::cmp::Ordering {
    let (pa, ha, ma) = a;
    let (pb, hb, mb) = b;
    pa.cmp(&pb).then_with(|| {
        // fraction(x) = hits/max, with max == 0 reading as 1.0 (the live guard).
        // Lower fraction ranks GREATER (more damaged wins). Compare
        // fa = ha/ma vs fb = hb/mb as ha·mb vs hb·ma, substituting x/0 → 1.
        match (ma, mb) {
            (0, 0) => std::cmp::Ordering::Equal,
            // a reads 1.0: a is greater only if b's fraction is ALSO ≥ 1 (b damaged ⇒ b wins).
            (0, _) => {
                if hb >= mb {
                    std::cmp::Ordering::Equal
                } else {
                    std::cmp::Ordering::Less
                }
            }
            (_, 0) => {
                if ha >= ma {
                    std::cmp::Ordering::Equal
                } else {
                    std::cmp::Ordering::Greater
                }
            }
            _ => {
                let cross_a = ha as u64 * mb as u64; // hits_a · max_b
                let cross_b = hb as u64 * ma as u64; // hits_b · max_a
                cross_b.cmp(&cross_a) // lower fraction ranks GREATER
            }
        }
    })
}

/// Engine constant: hits restored per energy spent on repair (`REPAIR_POWER`).
pub const REPAIR_HITS_PER_ENERGY: u32 = 100;

/// The exact repair energy a creep will spend this tick:
/// `min(work_parts, carried, ceil(missing / REPAIR_POWER))` — matches the engine's per-intent
/// pricing bit-for-bit, so a same-tick Transfer+Repair pair can split the cargo exactly (the
/// `consume_resource_from_deposits` mechanic).
pub fn repair_energy_consumed(work_parts: u32, carried: u32, hits: u32, hits_max: u32) -> u32 {
    let missing = hits_max.saturating_sub(hits);
    work_parts.min(carried).min(missing.div_ceil(REPAIR_HITS_PER_ENERGY))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The road priority quarters, exact at the boundaries (moved from the sim baseline pin).
    #[test]
    fn road_priority_map_matches_live_thresholds() {
        assert_eq!(map_normal_priority(1249, 5000), RepairPriority::High);
        assert_eq!(map_normal_priority(1250, 5000), RepairPriority::Medium, "exactly 25% is NOT <25%");
        assert_eq!(map_normal_priority(2499, 5000), RepairPriority::Medium);
        assert_eq!(map_normal_priority(2500, 5000), RepairPriority::Low);
        assert_eq!(map_normal_priority(3750, 5000), RepairPriority::VeryLow);
    }

    /// The container (high-value) map: a half-dead container is CRITICAL (passes even the S1
    /// gate — the refuted-siege-suppression shape).
    #[test]
    fn container_priority_map_matches_live_thresholds() {
        assert_eq!(map_high_value_priority(124_999, 250_000), RepairPriority::Critical);
        assert_eq!(map_high_value_priority(125_000, 250_000), RepairPriority::High);
        assert_eq!(map_high_value_priority(187_500, 250_000), RepairPriority::Low);
        assert_eq!(map_high_value_priority(237_500, 250_000), RepairPriority::VeryLow);
    }

    /// The queue ordering: priority dominates; equal priority resolves to the LOWEST hp
    /// fraction; `max_hits == 0` reads as undamaged (the live NaN-guard pin).
    #[test]
    fn repair_target_order_matches_live_queue() {
        use std::cmp::Ordering::*;
        // Priority dominates fraction.
        assert_eq!(
            repair_target_order((RepairPriority::Critical, 99, 100), (RepairPriority::Low, 1, 100)),
            Greater
        );
        // Equal priority: lower fraction (more damaged) is GREATER.
        assert_eq!(
            repair_target_order((RepairPriority::Medium, 1500, 5000), (RepairPriority::Medium, 2000, 5000)),
            Greater
        );
        // Exact-fraction tie.
        assert_eq!(
            repair_target_order((RepairPriority::Medium, 1, 2), (RepairPriority::Medium, 2500, 5000)),
            Equal
        );
        // max_hits == 0 reads as fraction 1.0 and loses to a damaged structure.
        assert_eq!(
            repair_target_order((RepairPriority::Medium, 0, 0), (RepairPriority::Medium, 50, 100)),
            Less
        );
        assert_eq!(
            repair_target_order((RepairPriority::Medium, 50, 100), (RepairPriority::Medium, 0, 0)),
            Greater
        );
        // Both zero-max: equal.
        assert_eq!(
            repair_target_order((RepairPriority::Medium, 0, 0), (RepairPriority::Medium, 0, 0)),
            Equal
        );
    }

    /// The exact-split contract (moved: repairbehavior.rs + baseline.rs pins).
    #[test]
    fn repair_energy_consumed_matches_resolver_pricing() {
        assert_eq!(repair_energy_consumed(3, 10, 0, 1000), 3, "work-limited");
        assert_eq!(repair_energy_consumed(10, 2, 0, 1000), 2, "carry-limited");
        assert_eq!(repair_energy_consumed(10, 10, 899, 1000), 2, "ceil(101/100)");
        assert_eq!(repair_energy_consumed(10, 10, 900, 1000), 1);
        assert_eq!(repair_energy_consumed(10, 10, 999, 1000), 1);
        assert_eq!(repair_energy_consumed(10, 10, 1000, 1000), 0, "full target");
        assert_eq!(repair_energy_consumed(10, 10, 0, 0), 0);
    }
}
