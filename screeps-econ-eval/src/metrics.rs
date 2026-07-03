//! T_recover + diagnostics + the deadlock sentinel + **THE REPRO GATE** (ADR 0040 §D7 metrics;
//! M1 spec Part C.5/C.9). All the statistics ride rover-eval's `stats.rs` verbatim (the lib dep):
//! `H = ΣWη/ΣW`, seeded bootstrap 95% CI, p05–p95.

use screeps_econ_engine::{EconTickReport, EconWorld, SimResource};
use screeps_rover_eval::stats::{bootstrap_weighted_mean_ci, Summary};

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Recovered-state constants (ADR §D7 T_recover; open decision #7 — named + env-overridable so the
// sweep can probe them; the 0.9 cliff is #7's justify-or-replace subject).
// ═════════════════════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug)]
pub struct RecoverConsts {
    /// (i) spawn/extension energy == capacity for this many CONSECUTIVE ticks — RECOVER_FULL_WINDOW.
    pub full_window: u32,
    /// (ii) trailing income ≥ this per-mille of source potential — RECOVER_INCOME_FRAC (900 = 0.9).
    pub income_frac_q: u32,
    /// The trailing-income window (the ADR's 300t).
    pub income_window: u32,
}

impl Default for RecoverConsts {
    fn default() -> Self {
        RecoverConsts { full_window: 50, income_frac_q: 900, income_window: 300 }
    }
}

impl RecoverConsts {
    /// Env overrides: `ECON_RECOVER_FULL_WINDOW`, `ECON_RECOVER_INCOME_FRAC_Q` (per-mille),
    /// `ECON_RECOVER_INCOME_WINDOW`.
    pub fn from_env() -> Self {
        let get = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(d);
        let d = RecoverConsts::default();
        RecoverConsts {
            full_window: get("ECON_RECOVER_FULL_WINDOW", d.full_window),
            income_frac_q: get("ECON_RECOVER_INCOME_FRAC_Q", d.income_frac_q),
            income_window: get("ECON_RECOVER_INCOME_WINDOW", d.income_window),
        }
    }

    /// The income threshold for `n_sources` over one window: `frac × n × 10 e/t × window`
    /// (10 e/t = the 3000/300 source potential — engine-mechanics.md:466), exact integer.
    pub fn income_threshold(&self, n_sources: u32) -> u64 {
        self.income_frac_q as u64 * n_sources as u64 * 10 * self.income_window as u64 / 1000
    }
}

/// Streaming T_recover detection: recovered = the first tick where BOTH (i) the spawn lane has
/// been full for `full_window` consecutive ticks AND (ii) trailing-`income_window` harvested
/// income meets the threshold.
pub struct RecoveryTracker {
    consts: RecoverConsts,
    n_sources: u32,
    full_streak: u32,
    income_ring: Vec<u64>,
    income_sum: u64,
    tick_count: u32,
    pub recovered_at: Option<u32>,
}

impl RecoveryTracker {
    pub fn new(consts: RecoverConsts, n_sources: u32) -> Self {
        let window = consts.income_window.max(1) as usize;
        RecoveryTracker {
            consts,
            n_sources,
            full_streak: 0,
            income_ring: vec![0; window],
            income_sum: 0,
            tick_count: 0,
            recovered_at: None,
        }
    }

    /// Feed one resolved tick (call AFTER `resolve_econ_tick`, with the mutated world).
    pub fn observe(&mut self, world: &EconWorld, report: &EconTickReport) {
        let idx = (self.tick_count as usize) % self.income_ring.len();
        self.income_sum = self.income_sum - self.income_ring[idx] + report.ledger.harvested;
        self.income_ring[idx] = report.ledger.harvested;
        self.tick_count += 1;

        let capacity = crate::baseline::spawn_lane_capacity(world);
        let full = capacity > 0 && world.room_spawn_energy() >= capacity;
        self.full_streak = if full { self.full_streak + 1 } else { 0 };

        if self.recovered_at.is_none()
            && self.full_streak >= self.consts.full_window
            && self.income_sum >= self.consts.income_threshold(self.n_sources)
        {
            self.recovered_at = Some(report.tick);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The deadlock sentinel (ADR §D7 hard gate): 12 zero-progress ticks WITH demand ⇒ η = 0 + gate.
// ═════════════════════════════════════════════════════════════════════════════════════════════

pub const DEADLOCK_TICKS: u32 = 12;

/// Progress = any economic flow or event this tick (harvest/self-charge/repair, births, spawn
/// starts, deaths, a creep moving, or any stock delta). Demand = a refill deficit or a pending
/// spawn queue. 12 consecutive no-progress-with-demand ticks fire the sentinel.
#[derive(Default)]
pub struct DeadlockSentinel {
    stall: u32,
    pub fired_at: Option<u32>,
}

impl DeadlockSentinel {
    #[allow(clippy::too_many_arguments)]
    pub fn observe(&mut self, tick: u32, progressed: bool, demand: bool) {
        if self.fired_at.is_some() {
            return;
        }
        if progressed || !demand {
            self.stall = 0;
        } else {
            self.stall += 1;
            if self.stall >= DEADLOCK_TICKS {
                self.fired_at = Some(tick);
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Per-run diagnostics (ADR §D7 refill diagnostics + the leak).
// ═════════════════════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug, Default)]
pub struct LeakTotals {
    pub roads: u64,
    pub containers: u64,
    pub other: u64,
}

impl LeakTotals {
    pub fn total(&self) -> u64 {
        self.roads + self.containers + self.other
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Diagnostics {
    /// Ticks where an idle spawn + a pending affordable-at-capacity request existed but energy
    /// didn't cover it (the head-of-line banking state — the S6 window made visible).
    pub spawn_energy_blocked_ticks: u32,
    /// Spawn-ticks idle (no creep in production) over spawns × ticks.
    pub spawn_idle_ticks: u64,
    pub spawn_ticks: u64,
    /// Σ over ticks of (spawn-lane capacity − energy) — the refill-latency integral.
    pub extension_deficit_integral: u64,
    /// Σ `repair_leak_e` by class (the engine report's counter).
    pub leak: LeakTotals,
}

impl Diagnostics {
    pub fn spawn_idle_frac(&self) -> f64 {
        if self.spawn_ticks == 0 {
            0.0
        } else {
            self.spawn_idle_ticks as f64 / self.spawn_ticks as f64
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// H over Family C — rover-eval stats verbatim; W = T* with the 25%-per-family single-scenario
// weight cap (ADR §D7: one scenario may not dominate its family's H).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Apply the 25% single-scenario weight-share cap — the exact WATER-FILLING fixed point: after
/// capping, every weight's share of the FINAL total is ≤ 25%. With a binding set B of the k
/// largest weights (k ≤ 3), each binding weight ends at `cap = 0.25 · T_final` where
/// `T_final = Σ_{non-binding} w / (1 − 0.25k)`; B is found by trying k = 0..=3 and taking the
/// first k whose implied cap already covers the (k+1)-th largest weight (the fixed point is
/// unique — the implied cap is monotone in k). INFEASIBLE for n ≤ 3 samples (three shares each
/// ≤ 25% cannot sum to 1) — the cap is SKIPPED there, weights untouched (a ≤ 3-scenario family
/// is too small for a dominance cap to bind meaningfully; the bench prints n).
pub fn cap_family_weights(samples: &mut [(f64, f64)]) {
    let n = samples.len();
    if n <= 3 {
        return; // infeasible (doc above)
    }
    let total: f64 = samples.iter().map(|&(_, w)| w).sum();
    if total <= 0.0 {
        return;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        samples[b].1.partial_cmp(&samples[a].1).unwrap_or(std::cmp::Ordering::Equal)
    });
    for k in 0..=3usize.min(n - 1) {
        let rest: f64 = order[k..].iter().map(|&i| samples[i].1).sum();
        let t_final = rest / (1.0 - 0.25 * k as f64);
        let cap = 0.25 * t_final;
        let k_binding_ok = k == 0 || samples[order[k - 1]].1 > cap;
        let next_free_ok = samples[order[k]].1 <= cap;
        if k_binding_ok && next_free_ok {
            for &i in &order[..k] {
                samples[i].1 = cap;
            }
            return;
        }
    }
    // Unreachable for n ≥ 4 (some k in 0..=3 always satisfies both conditions); if float
    // pathology ever lands here, untouched weights are the safe fallback.
}

/// Family-C H: `Summary::of` over the weight-capped (η, W = T*) samples.
pub fn family_h(samples: &[(f64, f64)], seed: u32) -> Summary {
    let mut capped = samples.to_vec();
    cap_family_weights(&mut capped);
    Summary::of(&capped, seed)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// THE REPRO GATE (blocks M4): with the BASELINE policy over N ≥ 10 seeds —
//   (a) repair_leak_e > 0 on EVERY bait run, AND
//   (b) pooled paired ΔT_recover (bait − control) > 0 with a bootstrap 95% CI excluding zero.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One paired (scenario, seed) observation.
#[derive(Clone, Debug)]
pub struct PairedRun {
    pub scenario: String,
    pub seed: u32,
    pub bait_t: u32,
    pub control_t: u32,
    pub bait_recovered: bool,
    pub control_recovered: bool,
    pub bait_leak: LeakTotals,
    pub control_leak: LeakTotals,
}

impl PairedRun {
    pub fn delta(&self) -> i64 {
        self.bait_t as i64 - self.control_t as i64
    }
}

#[derive(Clone, Debug)]
pub struct GateVerdict {
    pub n_pairs: usize,
    pub all_bait_leaked: bool,
    pub bait_runs_leaked: usize,
    pub mean_delta: f64,
    /// Bootstrap 95% CI of the mean paired delta (rover-eval stats, weight 1 per pair).
    pub ci95: (f64, f64),
    pub ci_excludes_zero: bool,
    pub pass: bool,
}

/// Compute the verdict. `seed` drives the bootstrap (reproducible).
pub fn repro_gate_verdict(pairs: &[PairedRun], seed: u32) -> GateVerdict {
    let bait_runs_leaked = pairs.iter().filter(|p| p.bait_leak.total() > 0).count();
    let all_bait_leaked = bait_runs_leaked == pairs.len() && !pairs.is_empty();
    let samples: Vec<(f64, f64)> = pairs.iter().map(|p| (p.delta() as f64, 1.0)).collect();
    let mean_delta = if samples.is_empty() {
        0.0
    } else {
        samples.iter().map(|&(v, _)| v).sum::<f64>() / samples.len() as f64
    };
    let ci95 = bootstrap_weighted_mean_ci(&samples, 2000, 0.05, seed);
    let ci_excludes_zero = ci95.0 > 0.0;
    GateVerdict {
        n_pairs: pairs.len(),
        all_bait_leaked,
        bait_runs_leaked,
        mean_delta,
        ci95,
        ci_excludes_zero,
        pass: all_bait_leaked && ci_excludes_zero,
    }
}

/// Stock helper for progress detection (exact, cheap): total energy across every store + piles.
pub fn energy_stock(world: &EconWorld) -> u64 {
    world.stocks().get(&SimResource::Energy).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn income_threshold_is_exact() {
        let c = RecoverConsts::default();
        assert_eq!(c.income_threshold(1), 2700, "0.9 × 1 source × 10 e/t × 300t");
        assert_eq!(c.income_threshold(2), 5400);
    }

    #[test]
    fn weight_cap_limits_any_single_scenario_to_a_quarter() {
        // The reviewer's binding case: {90,5,5,5,5,5} → the big weight water-fills to exactly
        // 25/3 (rest = 25, k = 1: T_final = 25/0.75, cap = 25/3) — and EVERY final share ≤ 25%.
        let mut s = vec![(1.0, 90.0), (0.5, 5.0), (0.2, 5.0), (0.3, 5.0), (0.4, 5.0), (0.6, 5.0)];
        cap_family_weights(&mut s);
        assert!((s[0].1 - 25.0 / 3.0).abs() < 1e-9, "the binding weight ends at 25% of the FINAL total");
        let total: f64 = s.iter().map(|&(_, w)| w).sum();
        for (i, &(_, w)) in s.iter().enumerate() {
            assert!(w / total <= 0.25 + 1e-12, "sample {i}: final share {} ≤ 25%", w / total);
        }
        assert_eq!(s[1].1, 5.0, "non-binding weights untouched");

        // Two binding weights: {60, 60, 10, 10, 10, 10} → k = 2: T_final = 40/0.5 = 80, cap = 20.
        let mut s2 = vec![(0.1, 60.0), (0.2, 60.0), (0.3, 10.0), (0.4, 10.0), (0.5, 10.0), (0.6, 10.0)];
        cap_family_weights(&mut s2);
        assert!((s2[0].1 - 20.0).abs() < 1e-9 && (s2[1].1 - 20.0).abs() < 1e-9);
        let total2: f64 = s2.iter().map(|&(_, w)| w).sum();
        assert!((total2 - 80.0).abs() < 1e-9);
        for &(_, w) in &s2 {
            assert!(w / total2 <= 0.25 + 1e-12);
        }

        // Already-feasible weights are untouched; n ≤ 3 is infeasible ⇒ skipped untouched.
        let mut s3 = vec![(0.1, 10.0), (0.2, 10.0), (0.3, 10.0), (0.4, 10.0)];
        cap_family_weights(&mut s3);
        assert!(s3.iter().all(|&(_, w)| w == 10.0), "equal weights already satisfy the cap");
        let mut s4 = vec![(1.0, 90.0), (0.5, 5.0), (0.2, 5.0)];
        cap_family_weights(&mut s4);
        assert_eq!(s4[0].1, 90.0, "n ≤ 3: the cap is infeasible and skipped");
    }

    #[test]
    fn gate_verdict_requires_both_arms() {
        let leak = LeakTotals { roads: 5, ..Default::default() };
        let none = LeakTotals::default();
        let mk = |d: i64, bl: LeakTotals| PairedRun {
            scenario: "x".into(),
            seed: 1,
            bait_t: (1000 + d) as u32,
            control_t: 1000,
            bait_recovered: true,
            control_recovered: true,
            bait_leak: bl,
            control_leak: none,
        };
        // Strong positive deltas + universal leak → pass.
        let pairs: Vec<PairedRun> = (0..12).map(|i| mk(200 + i, leak)).collect();
        let v = repro_gate_verdict(&pairs, 1);
        assert!(v.all_bait_leaked && v.ci_excludes_zero && v.pass);
        assert!(v.ci95.0 > 0.0 && v.mean_delta > 200.0);
        // One leak-free bait run → (a) fails.
        let mut pairs2 = pairs.clone();
        pairs2[3].bait_leak = none;
        assert!(!repro_gate_verdict(&pairs2, 1).pass);
        // Deltas straddling zero → (b) fails.
        let pairs3: Vec<PairedRun> = (0..12).map(|i| mk(if i % 2 == 0 { 50 } else { -50 }, leak)).collect();
        assert!(!repro_gate_verdict(&pairs3, 1).pass);
    }

    #[test]
    fn deadlock_needs_demand_and_persistence() {
        let mut s = DeadlockSentinel::default();
        for t in 0..11 {
            s.observe(t, false, true);
        }
        assert!(s.fired_at.is_none(), "11 ticks is not yet a deadlock");
        s.observe(11, false, true);
        assert_eq!(s.fired_at, Some(11));
        let mut s = DeadlockSentinel::default();
        for t in 0..100 {
            s.observe(t, false, false); // no demand: never fires
        }
        assert!(s.fired_at.is_none());
        let mut s = DeadlockSentinel::default();
        for t in 0..100 {
            s.observe(t, t % 5 == 0, true); // periodic progress resets
        }
        assert!(s.fired_at.is_none());
    }
}
