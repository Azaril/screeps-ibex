//! Global CPU governor (P1.B3, ADR 0004): ONE tick-consistent view of
//! CPU pressure that every expensive system reads, replacing scattered
//! live `game::cpu::*` reads.
//!
//! ## Model
//!
//! A pure decision kernel ([`compute_tier`]) maps (bucket, bucket
//! trend) to a [`Tier`]:
//!
//! - **Normal** — full behavior.
//! - **Conserve** — the bucket is low or draining: sheddable work slows
//!   (cadences stretch, low-priority spawns pause).
//! - **Critical** — extinction-risk territory: only the never-shed set
//!   runs at full service (defense, spawn, haul, movement,
//!   `serialize_world` — ADR 0004's authoritative shed order; changes
//!   to that set amend the ADR).
//!
//! The thresholds below are INITIAL values pending calibration against
//! the P1.A5 pressure scenario; they are constants (not config) so the
//! kernel stays pure and the calibration lands as a reviewed diff.
//!
//! ## Snapshot
//!
//! [`refresh`] is called once at tick start (`metrics::tick_start`)
//! with the bucket, the metrics window's trend, and `tick_limit`; all
//! readers see that single snapshot for the whole tick. Bucket only
//! changes between ticks, so the snapshot loses nothing vs live reads
//! — and `can_execute_cpu` keeps its exact legacy formula
//! (`bucket >= tick_limit * bar`), making the conversion
//! behavior-preserving (the parity requirement on this task).

use crate::missions::constants::CpuBar;
use std::sync::Mutex;

/// Bucket below this is Critical outright (≈ a few hundred ticks from
/// hard-zero at typical drain).
pub const CRITICAL_BUCKET: i32 = 1_500;
/// Draining faster than this while under [`CONSERVE_BUCKET`] is
/// Critical even though the absolute level looks survivable.
pub const CRITICAL_DRAIN: f64 = -10.0;
/// Bucket below this is at least Conserve.
pub const CONSERVE_BUCKET: i32 = 4_000;
/// A sustained drain steeper than this is Conserve at ANY bucket level
/// (the death-spiral alarm fires on trend, not level — ADR 0004).
pub const CONSERVE_DRAIN: f64 = -5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Normal,
    Conserve,
    Critical,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Normal => "normal",
            Tier::Conserve => "conserve",
            Tier::Critical => "critical",
        }
    }
}

/// Pure decision kernel: (bucket, trend in bucket-units/tick) → tier.
pub fn compute_tier(bucket: i32, trend: f64) -> Tier {
    if bucket < CRITICAL_BUCKET || (bucket < CONSERVE_BUCKET && trend < CRITICAL_DRAIN) {
        Tier::Critical
    } else if bucket < CONSERVE_BUCKET || trend < CONSERVE_DRAIN {
        Tier::Conserve
    } else {
        Tier::Normal
    }
}

#[derive(Debug, Clone, Copy)]
struct Snapshot {
    bucket: i32,
    trend: f64,
    tick_limit: f64,
    tier: Tier,
}

impl Default for Snapshot {
    fn default() -> Self {
        // Pre-refresh default (first tick of a fresh VM before
        // tick_start runs): a healthy posture so nothing is shed
        // before evidence exists.
        Snapshot {
            bucket: 10_000,
            trend: 0.0,
            tick_limit: 500.0,
            tier: Tier::Normal,
        }
    }
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

fn snapshot() -> Snapshot {
    SNAPSHOT.lock().ok().and_then(|guard| *guard).unwrap_or_default()
}

/// Tick-start refresh (called from `metrics::tick_start`).
pub fn refresh(bucket: i32, trend: f64, tick_limit: f64) {
    let snap = Snapshot {
        bucket,
        trend,
        tick_limit,
        tier: compute_tier(bucket, trend),
    };
    if let Ok(mut guard) = SNAPSHOT.lock() {
        *guard = Some(snap);
    }
}

/// The tick's governor tier.
pub fn tier() -> Tier {
    snapshot().tier
}

/// The legacy CPU bar check, now reading the tick-start snapshot:
/// `bucket >= tick_limit * bar`. Exactly the formula the scattered
/// `game::cpu` sites used — behavior-preserving by construction.
pub fn can_execute_cpu(bar: CpuBar) -> bool {
    let snap = snapshot();
    snap.bucket as f64 >= snap.tick_limit * bar as u32 as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bucket/trend profiles from the ADR 0004 narrative: crash,
    /// slow drain, recovery, sustained pressure.
    #[test]
    fn tier_kernel_profiles() {
        // Healthy: full bucket, flat.
        assert_eq!(compute_tier(10_000, 0.0), Tier::Normal);
        // Crash: low bucket regardless of trend.
        assert_eq!(compute_tier(1_000, 0.0), Tier::Critical);
        assert_eq!(compute_tier(1_499, 5.0), Tier::Critical);
        // Fast drain while merely low: critical before the floor.
        assert_eq!(compute_tier(3_000, -12.0), Tier::Critical);
        // Slow drain while low: conserve.
        assert_eq!(compute_tier(3_000, -2.0), Tier::Conserve);
        // Low but refilling: still conserve (level rules until clear).
        assert_eq!(compute_tier(3_000, 8.0), Tier::Conserve);
        // High bucket but steep sustained drain: conserve early —
        // the death-spiral alarm acts on trend, not level.
        assert_eq!(compute_tier(9_000, -7.5), Tier::Conserve);
        // Recovery: back above thresholds, flat.
        assert_eq!(compute_tier(4_001, 0.0), Tier::Normal);
    }

    /// Boundary exactness — the constants are load-bearing.
    #[test]
    fn tier_kernel_boundaries() {
        assert_eq!(compute_tier(CRITICAL_BUCKET, 0.0), Tier::Conserve);
        assert_eq!(compute_tier(CRITICAL_BUCKET - 1, 0.0), Tier::Critical);
        assert_eq!(compute_tier(CONSERVE_BUCKET, 0.0), Tier::Normal);
        assert_eq!(compute_tier(CONSERVE_BUCKET - 1, 0.0), Tier::Conserve);
        assert_eq!(compute_tier(CONSERVE_BUCKET, CONSERVE_DRAIN), Tier::Normal);
        assert_eq!(
            compute_tier(CONSERVE_BUCKET, CONSERVE_DRAIN - 0.1),
            Tier::Conserve
        );
        assert_eq!(
            compute_tier(CONSERVE_BUCKET - 1, CRITICAL_DRAIN - 0.1),
            Tier::Critical
        );
    }

    /// The snapshot-backed bar check reproduces the legacy formula.
    #[test]
    fn can_execute_cpu_matches_legacy_formula() {
        refresh(2_500, 0.0, 500.0);
        // bar thresholds: Critical=2 -> 1000, High=3 -> 1500,
        // Medium=4 -> 2000, Low=5 -> 2500, Idle=7 -> 3500.
        assert!(can_execute_cpu(CpuBar::CriticalPriority));
        assert!(can_execute_cpu(CpuBar::HighPriority));
        assert!(can_execute_cpu(CpuBar::MediumPriority));
        assert!(can_execute_cpu(CpuBar::LowPriority));
        assert!(!can_execute_cpu(CpuBar::IdlePriority));
        refresh(999, 0.0, 500.0);
        assert!(!can_execute_cpu(CpuBar::CriticalPriority));
    }

    /// Pre-refresh default is the healthy posture (nothing shed before
    /// evidence exists).
    #[test]
    fn default_snapshot_is_healthy() {
        // Note: relies on test ordering being irrelevant — we only
        // assert the DEFAULT constructor here, not the global.
        let snap = Snapshot::default();
        assert_eq!(snap.tier, Tier::Normal);
        assert!(snap.bucket as f64 >= snap.tick_limit * 7.0);
    }
}
