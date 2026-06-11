//! Mission-side pathfinding ops pool (P1.B4 / ADR 0004 step 4) — the
//! facade's aggregate bound.
//!
//! The movement system already runs a mature CPU-aware budget (rover:
//! per-tick ops cap, repath budget, CPU cap, `MIN_PATHFIND_OPS` floor).
//! What was unbounded was the MISSION side: `find_nearest_*` searches,
//! spawn-distance precomputation, and `find_route` calls are each
//! individually capped (P1.B1) but their AGGREGATE per tick was not —
//! N call sites × per-search caps is exactly the leak class the
//! governor exists to stop.
//!
//! One pool, reset each tick at the governor's tier:
//! Normal = [`BASE_MISSION_OPS`], Conserve ×½, Critical ×¼. Consumers
//! [`take`] what they want and clamp their search to the grant; an
//! empty pool returns 0 and the caller degrades the way it already
//! degrades for a capped-out search ("no path found" semantics, or
//! serve-stale for the route cache). Movement's own budget is NOT
//! drawn from this pool — movement is never-shed and keeps its
//! independent reserve; the tier scaling there happens at its call
//! site with the floor preserved.

use crate::cpugovernor::Tier;
use std::sync::atomic::{AtomicU32, Ordering};

/// Per-tick mission-side ops at Normal tier (1 op ≈ 0.001 CPU, so this
/// bounds mission pathfinding at ~20 CPU/tick — generous against
/// typical use; the point is the BOUND, not the throttle).
pub const BASE_MISSION_OPS: u32 = 20_000;

/// Nominal charge for one `find_route` call (room-graph search has no
/// ops parameter — this is accounting plus admission control).
pub const FIND_ROUTE_NOMINAL_OPS: u32 = 2_000;

static POOL: AtomicU32 = AtomicU32::new(BASE_MISSION_OPS);
static REMAINING: AtomicU32 = AtomicU32::new(BASE_MISSION_OPS);

/// Pool size for a tier (pure; fixture-tested).
pub fn pool_for_tier(tier: Tier) -> u32 {
    match tier {
        Tier::Normal => BASE_MISSION_OPS,
        Tier::Conserve => BASE_MISSION_OPS / 2,
        Tier::Critical => BASE_MISSION_OPS / 4,
    }
}

/// Tick-start reset (`metrics::tick_start`).
pub fn reset(tier: Tier) {
    let pool = pool_for_tier(tier);
    POOL.store(pool, Ordering::Relaxed);
    REMAINING.store(pool, Ordering::Relaxed);
}

/// Take up to `want` ops from the pool; returns the grant (0 when the
/// pool is exhausted — callers degrade like a capped-out search).
pub fn take(want: u32) -> u32 {
    let mut current = REMAINING.load(Ordering::Relaxed);
    loop {
        let grant = want.min(current);
        if grant == 0 {
            return 0;
        }
        match REMAINING.compare_exchange_weak(
            current,
            current - grant,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return grant,
            Err(actual) => current = actual,
        }
    }
}

/// Telemetry: (pool, consumed) this tick.
pub fn snapshot() -> (u32, u32) {
    let pool = POOL.load(Ordering::Relaxed);
    let remaining = REMAINING.load(Ordering::Relaxed);
    (pool, pool.saturating_sub(remaining))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_scales_with_tier() {
        assert_eq!(pool_for_tier(Tier::Normal), 20_000);
        assert_eq!(pool_for_tier(Tier::Conserve), 10_000);
        assert_eq!(pool_for_tier(Tier::Critical), 5_000);
    }

    /// Grants clamp to what remains; exhaustion returns 0; the
    /// telemetry tracks consumption.
    #[test]
    fn take_drains_and_clamps() {
        reset(Tier::Critical); // 5000
        assert_eq!(take(2_000), 2_000);
        assert_eq!(take(2_000), 2_000);
        // Only 1000 left — partial grant.
        assert_eq!(take(2_000), 1_000);
        assert_eq!(take(500), 0);
        let (pool, consumed) = snapshot();
        assert_eq!(pool, 5_000);
        assert_eq!(consumed, 5_000);
        reset(Tier::Normal);
        assert_eq!(snapshot(), (20_000, 0));
    }
}
