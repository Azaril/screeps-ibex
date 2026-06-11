//! Colony-health score (P1.A4) — the objective function the rewrite
//! optimizes (rewrite-plan §1), computed from a run's captured seg-57
//! metrics blocks plus the console gate counters.
//!
//! Four terms, SURVIVAL-DOMINATING: survival is a multiplicative gate
//! (a run that panicked, lost state, or ended creepless scores ~0
//! regardless of the other terms), the rest are a weighted blend
//! renormalized over the terms that produced a signal (the military
//! term is absent in non-combat scenarios rather than fabricated).
//!
//! v1 calibration honesty: the economic normalization constant below is
//! an initial value — baselines recorded with it are comparable to each
//! other, which is all the differ needs; absolute cross-era comparisons
//! wait for the BASELINE-2 era to settle the constants.

use screeps_ibex_metrics::MetricsBlock;
use serde::{Deserialize, Serialize};

/// Weight of CPU headroom within the non-survival blend.
pub const WEIGHT_CPU: f64 = 0.3;
/// Weight of economic growth within the non-survival blend.
pub const WEIGHT_ECON: f64 = 0.5;
/// Weight of the military term within the non-survival blend (absent
/// in runs with no combat signal; the blend renormalizes).
pub const WEIGHT_MILITARY: f64 = 0.2;
/// Soft-curve knee for economic growth: progress-per-tick at which the
/// econ term reaches 0.5 (x / (x + K)). Initial calibration constant.
pub const ECON_KNEE: f64 = 5.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColonyHealth {
    /// 1.0 or ~0 — the multiplicative gate.
    pub survival: f64,
    /// Mean of (tick_limit − cpu_used)/tick_limit over the run, clamped.
    pub cpu_headroom: f64,
    /// Soft-curved progress-per-tick (gcl + rcl progress + stored energy).
    pub econ_growth: f64,
    /// None when the run carried no combat signal.
    pub military: Option<f64>,
    /// survival × renormalized blend of the present terms.
    pub total: f64,
}

/// Console-derived gate inputs the blocks cannot see (the harness's
/// counters; the bot's own fault counters complement them).
#[derive(Debug, Clone, Copy, Default)]
pub struct GateInputs {
    pub panic_lines: u64,
    pub deser_failure_lines: u64,
}

/// Pure: blocks (chronological) + gate inputs → score.
pub fn colony_health(blocks: &[MetricsBlock], gates: GateInputs) -> ColonyHealth {
    let survived = gates.panic_lines == 0
        && gates.deser_failure_lines == 0
        && blocks.last().map(|b| b.creeps > 0).unwrap_or(false)
        && blocks
            .last()
            .map(|b| b.faults.deser_failures == 0)
            .unwrap_or(true);
    let survival = if survived { 1.0 } else { 0.0 };

    let cpu_headroom = if blocks.is_empty() {
        0.0
    } else {
        blocks
            .iter()
            .map(|b| ((b.cpu.tick_limit - b.cpu.used) / b.cpu.tick_limit).clamp(0.0, 1.0))
            .sum::<f64>()
            / blocks.len() as f64
    };

    let econ_growth = match (blocks.first(), blocks.last()) {
        (Some(first), Some(last)) if last.tick > first.tick => {
            let ticks = (last.tick - first.tick) as f64;
            let gcl = (last.gcl.progress - first.gcl.progress).max(0.0);
            let rcl: f64 = last
                .rooms
                .iter()
                .map(|room| {
                    let before = first
                        .rooms
                        .iter()
                        .find(|r| r.name == room.name)
                        .map(|r| r.rcl_progress)
                        .unwrap_or(0.0);
                    (room.rcl_progress - before).max(0.0)
                })
                .sum();
            let stored: f64 = last.rooms.iter().map(|r| r.stored_energy as f64).sum::<f64>()
                - first.rooms.iter().map(|r| r.stored_energy as f64).sum::<f64>();
            let per_tick = (gcl + rcl + stored.max(0.0)) / ticks;
            per_tick / (per_tick + ECON_KNEE)
        }
        _ => 0.0,
    };

    // v1: no combat scenarios exist yet — the term is absent until the
    // cohesion metrics (Inc 4) land in the block schema.
    let military: Option<f64> = None;

    let mut weight_sum = WEIGHT_CPU + WEIGHT_ECON;
    let mut blend = WEIGHT_CPU * cpu_headroom + WEIGHT_ECON * econ_growth;
    if let Some(m) = military {
        weight_sum += WEIGHT_MILITARY;
        blend += WEIGHT_MILITARY * m;
    }
    let total = survival * (blend / weight_sum);

    ColonyHealth {
        survival,
        cpu_headroom,
        econ_growth,
        military,
        total,
    }
}

/// One run's persisted score artifact (`score.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreArtifact {
    pub scenario: String,
    pub git_sha: String,
    pub health: ColonyHealth,
}

/// Regression threshold for the differ: an absolute total drop beyond
/// this flags the run (single-run comparisons stay advisory — the N=9
/// gate mode is where regressions actually gate, per ADR 0015).
pub const REGRESSION_DROP: f64 = 0.05;

#[derive(Debug, Clone, Serialize)]
pub struct ScoreComparison {
    pub baseline_total: f64,
    pub candidate_total: f64,
    pub delta: f64,
    pub regression: bool,
}

pub fn compare(baseline: &ColonyHealth, candidate: &ColonyHealth) -> ScoreComparison {
    let delta = candidate.total - baseline.total;
    ScoreComparison {
        baseline_total: baseline.total,
        candidate_total: candidate.total,
        delta,
        regression: delta < -REGRESSION_DROP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_ibex_metrics::*;

    fn block(tick: u32, creeps: u32, gcl_progress: f64, used: f64) -> MetricsBlock {
        MetricsBlock {
            v: METRICS_SCHEMA_VERSION,
            tick,
            vm_fresh: false,
            cpu: CpuMetrics {
                used,
                limit: 100.0,
                tick_limit: 500.0,
                bucket: 10_000,
                bucket_trend: 0.0,
            },
            gcl: LevelProgress {
                level: 1,
                progress: gcl_progress,
                progress_total: 1000.0,
            },
            gpl: LevelProgress {
                level: 0,
                progress: 0.0,
                progress_total: 1000.0,
            },
            credits: 0.0,
            creeps,
            missions: 0,
            operations: 0,
            rooms: Vec::new(),
            faults: FaultCounters::default(),
            governor: None,
            pathing: None,
        }
    }

    /// Survival dominates: a panicked run scores ~0 with a booming economy.
    #[test]
    fn survival_gates_everything() {
        let blocks = vec![block(100, 10, 0.0, 10.0), block(700, 10, 6000.0, 10.0)];
        let healthy = colony_health(&blocks, GateInputs::default());
        assert!(healthy.total > 0.4, "{healthy:?}");
        let panicked = colony_health(
            &blocks,
            GateInputs {
                panic_lines: 1,
                ..Default::default()
            },
        );
        assert_eq!(panicked.total, 0.0);
        // A run ending with zero creeps is extinction, not survival.
        let dead = vec![block(100, 10, 0.0, 10.0), block(700, 0, 6000.0, 10.0)];
        assert_eq!(colony_health(&dead, GateInputs::default()).total, 0.0);
    }

    /// More progress per tick scores higher; the curve saturates.
    #[test]
    fn econ_growth_orders_runs() {
        let slow = colony_health(
            &[block(100, 5, 0.0, 10.0), block(700, 5, 600.0, 10.0)],
            GateInputs::default(),
        );
        let fast = colony_health(
            &[block(100, 5, 0.0, 10.0), block(700, 5, 6000.0, 10.0)],
            GateInputs::default(),
        );
        assert!(fast.econ_growth > slow.econ_growth);
        assert!(fast.econ_growth < 1.0);
        assert!(fast.total > slow.total);
    }

    /// Headroom is the mean and burns reduce it.
    #[test]
    fn cpu_headroom_reflects_burn() {
        let idle = colony_health(
            &[block(100, 5, 0.0, 10.0), block(200, 5, 0.0, 10.0)],
            GateInputs::default(),
        );
        let burning = colony_health(
            &[block(100, 5, 0.0, 10.0), block(200, 5, 0.0, 450.0)],
            GateInputs::default(),
        );
        assert!(idle.cpu_headroom > burning.cpu_headroom);
    }

    #[test]
    fn compare_flags_real_regressions_only() {
        let blocks = vec![block(100, 5, 0.0, 10.0), block(700, 5, 3000.0, 10.0)];
        let a = colony_health(&blocks, GateInputs::default());
        let same = compare(&a, &a);
        assert!(!same.regression);
        let mut worse = a.clone();
        worse.total -= 0.2;
        assert!(compare(&a, &worse).regression);
        let mut slightly = a.clone();
        slightly.total -= 0.01;
        assert!(!compare(&a, &slightly).regression, "within threshold");
    }
}
