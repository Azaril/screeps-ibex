//! **The M4 market tournament** (ADR 0040 M4 — the decision milestone): BASELINE / S1 / PTRP /
//! MARKET-minus-K4 / MARKET arms over Families C (collapse), G (greenfield rush) and D
//! (downgrade pressure), with Family S (steady state) as the non-regression GUARD RAIL against
//! the stored M2 baselines. The 0033 `tuning.rs` idiom: a pure scorer, a gates-first
//! deterministic ranked key, an env-driven `#[ignore]` coordinate-descent sweep over the named
//! [`MarketConsts`] axes (2-3 values per axis — robust wins only, spec budget), N-seeded PAIRED
//! diffs vs the baseline arm (EP-6.7), and the bench mode (`econ_bench tournament[-full]`).
//!
//! **The gate (ADR M4):** market beats baseline on H_recover AND H_rcl with 95% CI excluding
//! zero on C/G/D; Family S within the non-regression bands; `repair_leak_e` down on C; zero
//! sentinel firings; flap/intents reported and sane; the matching CPU (ops/tick) measured for
//! the M5a budget. A null/partial result is reportable, not failure.

use crate::baseline::PolicyConfig;
use crate::market::MarketArmCfg;
use crate::metrics::{family_h, percentile_u32, RecoverConsts};
use crate::movement::AnalyticMover;
use crate::runner::{run_scenario, run_world, RunGoal, RunOptions, RunOutcome};
use crate::scenario::{
    catalog, contended_catalog, downgrade_catalog, fast_catalog, fast_downgrade_catalog, generate, rush_catalog,
    steady_catalog, ContendedScenario, EconScenario, RushScenario, SteadyScenario,
};
use screeps_econ_decision::sink_economics::MarketConsts;
use screeps_rover_eval::stats::{bootstrap_weighted_mean_ci, Summary};
use screeps_rover_eval::value::quantize_w;
use std::collections::BTreeMap;

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Arms.
// ═════════════════════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug)]
pub struct Arm {
    pub name: &'static str,
    pub cfg: PolicyConfig,
}

/// The five tournament arms (spec Part C). `measure_gap` turns the sim-only exact oracle on
/// for the FULL MARKET arm only (one arm suffices — the matcher is shared).
pub fn arms(consts: MarketConsts, measure_gap: bool, oracle_period: u32) -> Vec<Arm> {
    vec![
        Arm { name: "baseline", cfg: PolicyConfig::baseline() },
        Arm { name: "s1", cfg: PolicyConfig::s1() },
        Arm { name: "ptrp", cfg: PolicyConfig::ptrp() },
        Arm {
            name: "market-k4off",
            cfg: PolicyConfig::market(MarketArmCfg {
                consts,
                k4_bodies: false,
                measure_gap: false,
                oracle_period,
                deposit_reselect: true,
                a3_live_control: false,
            }),
        },
        // The END-STATE market (ADR 0044): the reduced-cost admission (source_floor + haul
        // subtraction) priced on TRUE routed distance are always on — this arm IS the shipped design;
        // the sweep tunes the constants around it. This is ALSO Arm B of the ADR 0044 A3 validation
        // (EV controller-container deposit + live Use-lane admission — the proposed-live behavior).
        Arm {
            name: "market",
            cfg: PolicyConfig::market(MarketArmCfg {
                consts,
                k4_bodies: true,
                measure_gap,
                oracle_period,
                deposit_reselect: true,
                a3_live_control: false,
            }),
        },
        // ATTRIBUTION control: BOTH 2026-07-10 changes reverted (no deposit-tick reselect, A3
        // defects restored) — the "before today's fixes" reference. Needed because the last stored
        // tournament predates all of ADR 0044, so it cannot attribute a delta to these two changes.
        // Pairs with `market-a3-control` (hauler ON / A3 OFF) to separate the two effects.
        // SIM-VALIDATION-ONLY — never live.
        Arm {
            name: "market-prefix-control",
            cfg: PolicyConfig::market(MarketArmCfg {
                consts,
                k4_bodies: true,
                measure_gap: false,
                oracle_period,
                deposit_reselect: false,
                a3_live_control: true,
            }),
        },
        // ADR 0044 A3 — Arm A ("live-today" control): reverts BOTH A3 defects (flat-tier controller
        // container deposit + bypassed Use-lane admission) to reproduce the shipped-live behavior.
        // The A/B against "market" (Arm B) on Family-C (benefit) + Family-S (regression guard)
        // measures whether shipping A3 live moves the corpus. SIM-VALIDATION-ONLY — never live.
        Arm {
            name: "market-a3-control",
            cfg: PolicyConfig::market(MarketArmCfg {
                consts,
                k4_bodies: true,
                measure_gap: false,
                oracle_period,
                deposit_reselect: true,
                a3_live_control: true,
            }),
        },
    ]
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Corpora + spec.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The tournament run parameters (env-overridable in the bench: `ECON_T_*`).
#[derive(Clone, Copy, Debug)]
pub struct TournamentSpec {
    pub full: bool,
    /// Paired seeds per Family-C scenario (the repro-gate idiom's N).
    pub c_seeds: u32,
    pub d_seeds: u32,
    pub g_seeds: u32,
    pub base_seed: u32,
    pub tick_cap: u32,
    pub g_tick_cap: u32,
    pub include_s: bool,
    /// Run the CONTENDED matching probe (Family M — review #2). Off in the constants sweep
    /// (the many-edge CPU/gap frontier does not move with the pricing constants — a per-point
    /// cost with no per-point signal); on in adjudication.
    pub include_m: bool,
}

impl TournamentSpec {
    pub fn fast_sweep(base_seed: u32) -> Self {
        TournamentSpec {
            full: false,
            c_seeds: 2,
            d_seeds: 1,
            g_seeds: 1,
            base_seed,
            tick_cap: 15_000,
            g_tick_cap: 120_000,
            include_s: true,
            include_m: false,
        }
    }

    pub fn adjudication(full: bool, base_seed: u32) -> Self {
        TournamentSpec {
            full,
            c_seeds: if full { 10 } else { 5 },
            d_seeds: if full { 5 } else { 2 },
            g_seeds: if full { 3 } else { 2 },
            base_seed,
            tick_cap: 15_000,
            g_tick_cap: 120_000,
            include_s: true,
            include_m: true,
        }
    }
}

fn c_corpus(spec: &TournamentSpec) -> Vec<EconScenario> {
    let mut base = if spec.full {
        let mut all = catalog();
        all.extend((100..103).map(generate));
        all
    } else {
        fast_catalog()
    };
    for sc in &mut base {
        sc.tick_cap = spec.tick_cap;
    }
    base
}

fn d_corpus(spec: &TournamentSpec) -> Vec<EconScenario> {
    let mut base = if spec.full { downgrade_catalog() } else { fast_downgrade_catalog() };
    for sc in &mut base {
        sc.tick_cap = spec.tick_cap;
    }
    base
}

fn g_corpus(spec: &TournamentSpec) -> Vec<RushScenario> {
    let mut rushes = if spec.full {
        rush_catalog(4, spec.g_seeds)
    } else {
        let rooms = ["E11N1", "E12S41", "E11N14", "E11N23"];
        rooms
            .iter()
            .flat_map(|r| (1..=spec.g_seeds).map(move |s| RushScenario::new(r, 4, s)))
            .collect()
    };
    for r in &mut rushes {
        r.tick_cap = spec.g_tick_cap;
    }
    rushes
}

fn s_corpus(spec: &TournamentSpec) -> Vec<SteadyScenario> {
    let all = steady_catalog(spec.base_seed);
    if spec.full {
        all
    } else {
        all.into_iter().take(2).collect()
    }
}

/// One Family-G rush under an arm (the bench's `run_rush`, shared here).
pub fn run_rush(rush: &RushScenario, cfg: PolicyConfig, consts: RecoverConsts, tick_cap: u32) -> RunOutcome {
    let (mut world, terrain, info) = rush.instantiate();
    let mut mover = AnalyticMover::new(&terrain);
    let mut opts = RunOptions::new(cfg, consts, tick_cap).with_goal(RunGoal::Rcl { target: rush.target_rcl });
    opts.construction_phase = rush.seed;
    run_world(&rush.shell(), &mut world, &mut mover, &info, &opts)
}

/// One Family-S horizon under an arm (the bench's `run_steady`, shared here).
pub fn run_steady(sc: &SteadyScenario, cfg: PolicyConfig, consts: RecoverConsts) -> RunOutcome {
    let (mut world, terrain, info) = sc.instantiate();
    let mut mover = AnalyticMover::new(&terrain);
    let mut opts = RunOptions::new(cfg, consts, sc.tick_cap).with_goal(RunGoal::Horizon);
    opts.construction_phase = sc.seed;
    run_world(&sc.shell(), &mut world, &mut mover, &info, &opts)
}

/// One Family-M contended-matching window under an arm (review #2). Horizon goal — the run is a
/// matching-frontier probe, scored only for the ops/tick + gap diagnostics.
pub fn run_contended(sc: &ContendedScenario, cfg: PolicyConfig, consts: RecoverConsts) -> RunOutcome {
    let (mut world, terrain, info) = sc.instantiate();
    let mut mover = AnalyticMover::new(&terrain);
    let mut opts = RunOptions::new(cfg, consts, sc.tick_cap).with_goal(RunGoal::Horizon);
    opts.construction_phase = sc.seed;
    run_world(&sc.shell(), &mut world, &mut mover, &info, &opts)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Scores.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// A paired ΔT summary vs the baseline arm over the same (scenario, seed) keys (negative =
/// the candidate is FASTER).
#[derive(Clone, Debug)]
pub struct PairedDelta {
    pub n: usize,
    pub mean: f64,
    pub ci95: (f64, f64),
}

pub fn paired_delta(deltas: &[i64], seed: u32) -> PairedDelta {
    let samples: Vec<(f64, f64)> = deltas.iter().map(|&d| (d as f64, 1.0)).collect();
    let mean = if samples.is_empty() { 0.0 } else { samples.iter().map(|&(v, _)| v).sum::<f64>() / samples.len() as f64 };
    PairedDelta { n: deltas.len(), mean, ci95: bootstrap_weighted_mean_ci(&samples, 2000, 0.05, seed) }
}

/// One arm's score over one family (C/G/D).
#[derive(Clone, Debug)]
pub struct FamilyScore {
    pub h: Summary,
    /// Paired ΔT vs the baseline arm (None for the baseline itself).
    pub delta_t: Option<PairedDelta>,
    /// Mean `repair_leak_e` per run (the C symptom; on G/D it reports the same counter).
    pub leak_mean: f64,
    pub deadlocks: u32,
    pub oracle_violations: u32,
    pub flap_per_kt: f64,
    pub intents_per_tick: f64,
    pub runs: usize,
    pub finished: usize,
}

/// One arm's full tournament result.
#[derive(Clone, Debug)]
pub struct ArmResult {
    pub name: &'static str,
    pub c: FamilyScore,
    pub g: FamilyScore,
    pub d: FamilyScore,
    /// Family-D levels lost across the corpus (the triage guard).
    pub d_levels_lost: u32,
    /// Family-S outcomes (per scenario) — the guard rail's inputs.
    pub s_runs: Vec<RunOutcome>,
    pub s_guard: Vec<SGuardVerdict>,
    /// Matching diagnostics over C/G/D/S (market arms; zero otherwise) — the HOME-ROOM floor.
    pub match_ops_per_tick_mean: f64,
    pub match_ops_per_tick_p95: u32,
    pub match_edges_per_pass: f64,
    pub gap: Option<crate::market::GapStats>,
    /// **Family-M CONTENDED matching diagnostics** (review #2) — the many-edge probe the §D8 #4
    /// verdict and the M5a CPU budget are actually decided on (home-room numbers are a floor).
    pub m_ops_per_tick_mean: f64,
    pub m_ops_per_tick_p95: u32,
    pub m_edges_per_pass: f64,
    pub m_max_edges_per_pass: u64,
    pub m_gap: Option<crate::market::GapStats>,
    /// Σ intents and Σ ticks over C+G+D (ranked-key tie-breaks).
    pub intents_total: u64,
    pub ticks_total: u64,
}

impl ArmResult {
    /// Zero sentinel firings + zero oracle violations + the S guard holding — the hard gates.
    pub fn gates_held(&self) -> bool {
        self.c.deadlocks + self.g.deadlocks + self.d.deadlocks == 0
            && self.c.oracle_violations + self.g.oracle_violations + self.d.oracle_violations == 0
            && self.s_guard.iter().all(|v| v.pass)
    }

    /// The gates-first deterministic ranked key (0033 idiom): gates, then the quantized SUM of
    /// the three family Hs (equal family weighting — cross-family trades stay visible in the
    /// table, never laundered), then fewer intents, then fewer simulated ticks.
    pub fn ranked_key(&self) -> (u8, i64, i64, i64) {
        (
            u8::from(self.gates_held()),
            quantize_w(self.c.h.weighted_mean) + quantize_w(self.g.h.weighted_mean) + quantize_w(self.d.h.weighted_mean),
            -(self.intents_total as i64),
            -(self.ticks_total as i64),
        )
    }
}

fn family_score(
    outcomes: &[RunOutcome],
    baseline_t: Option<&BTreeMap<String, u32>>,
    seed: u32,
) -> (FamilyScore, BTreeMap<String, u32>) {
    let mut h_samples: Vec<(f64, f64)> = Vec::new();
    let mut own_t: BTreeMap<String, u32> = BTreeMap::new();
    let mut deltas: Vec<i64> = Vec::new();
    let mut leak = 0f64;
    let mut deadlocks = 0u32;
    let mut oracle_violations = 0u32;
    let mut assignments = 0u64;
    let mut intents = 0u64;
    let mut ticks = 0u64;
    let mut finished = 0usize;
    for o in outcomes {
        h_samples.push((o.eta, o.t_star as f64));
        own_t.insert(o.scenario.clone(), o.effective_t);
        leak += o.leak().total() as f64;
        deadlocks += u32::from(o.deadlocked);
        if o.eta_raw > 1.01 {
            oracle_violations += 1;
        }
        assignments += o.assignments;
        intents += o.intents_emitted;
        ticks += o.ticks_run as u64;
        finished += usize::from(o.recovered_at.is_some());
        if let Some(base) = baseline_t {
            if let Some(&bt) = base.get(&o.scenario) {
                deltas.push(o.effective_t as i64 - bt as i64);
            }
        }
    }
    let score = FamilyScore {
        h: family_h(&h_samples, seed),
        delta_t: baseline_t.map(|_| paired_delta(&deltas, seed)),
        leak_mean: if outcomes.is_empty() { 0.0 } else { leak / outcomes.len() as f64 },
        deadlocks,
        oracle_violations,
        flap_per_kt: if ticks == 0 { 0.0 } else { assignments as f64 / (ticks as f64 / 1000.0) },
        intents_per_tick: if ticks == 0 { 0.0 } else { intents as f64 / ticks as f64 },
        runs: outcomes.len(),
        finished,
    };
    (score, own_t)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Family S — the guard rail vs the STORED M2 baselines.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The named non-regression bands (spec Part C): additive slack for the ratio metrics,
/// ×1.25/+10 on the refill-deficit EXPOSURE (the integral of lane deficit per tick — the
/// banded "refill latency" quantity), ×2/+10 for flap (assignment churn is policy-shaped),
/// ×1.2+1 for intents (a policy must not win H by emitting more intents).
///
/// **Measured band decision (M4):** the refill-episode p95 LENGTH is REPORTED, not banded —
/// under the market the lane HOVERS a few tens of energy below capacity for long stretches
/// (one nominal "episode") where the baseline oscillates full↔deep-deficit (many short
/// episodes); the per-tick deficit integral — which weighs an episode by its DEPTH — came out
/// flat-to-better for the market on the same runs (E11N1-rcl2: 165.3 vs 170.4 mean deficit
/// e/tick) while episode p95 exploded 192→1347. Episode length treats a 50e residual like a
/// 2300e collapse; the integral is the exposure the room actually experiences.
pub const S_IDLE_SLACK: f64 = 0.05;
pub const S_ROAD_SLACK: f64 = 0.05;
pub const S_DEFICIT_FACTOR: f64 = 1.25;
pub const S_DEFICIT_ABS: f64 = 10.0;
pub const S_FLAP_FACTOR: f64 = 2.0;
pub const S_FLAP_ABS: f64 = 10.0;
pub const S_INTENTS_FACTOR: f64 = 1.2;
pub const S_INTENTS_ABS: f64 = 1.0;

/// One stored (or in-process) Family-S baseline row.
#[derive(Clone, Debug)]
pub struct SBaselineRow {
    pub scenario: String,
    pub spawn_idle_frac: f64,
    pub road_min: f64,
    pub road_end: f64,
    /// Mean lane deficit per tick (the banded exposure).
    pub deficit_per_tick: f64,
    /// Reported only (band decision above).
    pub refill_p95: f64,
    pub flap_per_kt: f64,
    pub intents_per_tick: f64,
}

/// Load the stored M2 Family-S baseline rows (arm == "base") from a bench baseline JSON
/// (`runs/econ/full-be8c8be-dirty.json` by default; `ECON_S_BASELINE` overrides).
pub fn load_stored_s_baseline(path: &std::path::Path) -> Option<Vec<SBaselineRow>> {
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let mut out = Vec::new();
    for rec in doc.get("family_s")?.as_array()? {
        let scenario = rec.get("scenario")?.as_str()?.to_string();
        for arm in rec.get("arms")?.as_array()? {
            if arm.get("arm").and_then(|a| a.as_str()) != Some("base") {
                continue;
            }
            let f = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
            let road = arm.get("road_stock")?;
            let refill = arm.get("refill_episodes")?;
            out.push(SBaselineRow {
                scenario: scenario.clone(),
                spawn_idle_frac: f(arm, "spawn_idle_frac"),
                road_min: f(road, "min"),
                road_end: f(road, "end"),
                // The stored M2 S runs always ran the full 10k-tick horizon (Horizon goal,
                // DEFAULT_S_TICK_CAP) — the per-tick exposure divides by it.
                deficit_per_tick: f(arm, "extension_deficit_integral") / crate::scenario::DEFAULT_S_TICK_CAP as f64,
                refill_p95: f(refill, "p95"),
                flap_per_kt: f(arm, "flap_per_kilotick"),
                intents_per_tick: f(arm, "intents_per_tick"),
            });
        }
    }
    (!out.is_empty()).then_some(out)
}

/// An in-process baseline row from a run outcome (the fallback when a scenario has no stored
/// row; the M3 A/A proved the in-process baseline arm identical to the M2-stored one).
pub fn s_row_from_outcome(o: &RunOutcome) -> SBaselineRow {
    let ratio = |&(_, h, m): &(u32, u64, u64)| h as f64 / m.max(1) as f64;
    SBaselineRow {
        scenario: o.scenario.clone(),
        spawn_idle_frac: o.diagnostics.spawn_idle_frac(),
        road_min: o.road_stock.iter().map(ratio).fold(f64::INFINITY, f64::min).min(1.0),
        road_end: o.road_stock.last().map(ratio).unwrap_or(1.0),
        deficit_per_tick: o.diagnostics.extension_deficit_integral as f64 / o.ticks_run.max(1) as f64,
        refill_p95: percentile_u32(&o.deficit_episodes, 0.95) as f64,
        flap_per_kt: o.assignments as f64 / (o.ticks_run.max(1) as f64 / 1000.0),
        intents_per_tick: o.intents_emitted as f64 / o.ticks_run.max(1) as f64,
    }
}

#[derive(Clone, Debug)]
pub struct SGuardVerdict {
    pub scenario: String,
    /// (metric, baseline, candidate, pass) — every band, individually reportable.
    pub checks: Vec<(&'static str, f64, f64, bool)>,
    pub pass: bool,
}

/// Verdict one candidate S run against its baseline row (bands above) + the absolute guards
/// (zero levels lost, no deadlock).
pub fn s_guard_verdict(base: &SBaselineRow, cand: &RunOutcome) -> SGuardVerdict {
    let c = s_row_from_outcome(cand);
    let mut checks: Vec<(&'static str, f64, f64, bool)> = vec![
        ("spawn_idle_frac", base.spawn_idle_frac, c.spawn_idle_frac, c.spawn_idle_frac <= base.spawn_idle_frac + S_IDLE_SLACK),
        ("road_stock_min", base.road_min, c.road_min, c.road_min >= base.road_min - S_ROAD_SLACK),
        ("road_stock_end", base.road_end, c.road_end, c.road_end >= base.road_end - S_ROAD_SLACK),
        (
            "deficit_per_tick",
            base.deficit_per_tick,
            c.deficit_per_tick,
            c.deficit_per_tick <= (base.deficit_per_tick * S_DEFICIT_FACTOR).max(base.deficit_per_tick + S_DEFICIT_ABS),
        ),
        // REPORTED, never gating (the band decision above): pass = true unconditionally.
        ("refill_p95_reported", base.refill_p95, c.refill_p95, true),
        (
            "flap_per_kt",
            base.flap_per_kt,
            c.flap_per_kt,
            c.flap_per_kt <= (base.flap_per_kt * S_FLAP_FACTOR).max(base.flap_per_kt + S_FLAP_ABS),
        ),
        (
            "intents_per_tick",
            base.intents_per_tick,
            c.intents_per_tick,
            c.intents_per_tick <= (base.intents_per_tick * S_INTENTS_FACTOR).max(base.intents_per_tick + S_INTENTS_ABS),
        ),
        ("levels_lost", 0.0, cand.levels_lost as f64, cand.levels_lost == 0),
        ("deadlocked", 0.0, f64::from(u8::from(cand.deadlocked)), !cand.deadlocked),
    ];
    // An open-ended terminal deficit is a starvation signal the episode list can miss.
    if let Some(open) = cand.deficit_open_at_end {
        checks.push(("deficit_open_at_end", 0.0, open as f64, (open as f64) < cand.ticks_run as f64 * 0.5));
    }
    let pass = checks.iter().all(|&(_, _, _, p)| p);
    SGuardVerdict { scenario: cand.scenario.clone(), checks, pass }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The tournament driver.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The baseline arm's per-family `(scenario#seed → effective_t)` pairing maps.
pub type PairingMaps = (BTreeMap<String, u32>, BTreeMap<String, u32>, BTreeMap<String, u32>);

/// Run one arm across C/G/D/S. `baseline_t` = the baseline arm's pairing maps (None when
/// scoring the baseline itself); `s_baseline` = the guard rows. Returns the result AND this
/// arm's own pairing maps (the baseline pass feeds them to every candidate).
#[allow(clippy::too_many_arguments)]
pub fn run_arm(
    arm: &Arm,
    spec: &TournamentSpec,
    consts: RecoverConsts,
    baseline_t: Option<&PairingMaps>,
    s_baseline: &[SBaselineRow],
) -> (ArmResult, PairingMaps) {
    let opts_c = RunOptions::new(arm.cfg, consts, spec.tick_cap);
    let opts_d = RunOptions::new(arm.cfg, consts, spec.tick_cap).with_goal(RunGoal::RecoverThenClockSafe);

    // Family C: bait scenarios × N seeds.
    let mut c_runs: Vec<RunOutcome> = Vec::new();
    for sc in &c_corpus(spec) {
        for s in 0..spec.c_seeds {
            c_runs.push(run_scenario(&sc.with_seed(spec.base_seed + s), &opts_c));
        }
    }
    // Family G.
    let mut g_runs: Vec<RunOutcome> = Vec::new();
    for rush in &g_corpus(spec) {
        g_runs.push(run_rush(rush, arm.cfg, consts, spec.g_tick_cap));
    }
    // Family D.
    let mut d_runs: Vec<RunOutcome> = Vec::new();
    for sc in &d_corpus(spec) {
        for s in 0..spec.d_seeds {
            d_runs.push(run_scenario(&sc.with_seed(spec.base_seed + s), &opts_d));
        }
    }
    // Family S (the guard rail).
    let s_runs: Vec<RunOutcome> = if spec.include_s {
        s_corpus(spec).iter().map(|sc| run_steady(sc, arm.cfg, consts)).collect()
    } else {
        Vec::new()
    };
    // Family M (the CONTENDED matching probe — review #2): market arms only, adjudication only
    // (its whole purpose is the market's matching-frontier; no pass on baseline/S1/PTRP, and the
    // constants sweep skips it — the frontier is constant-invariant).
    let m_runs: Vec<RunOutcome> = if spec.include_m && arm.cfg.market.is_some() {
        contended_catalog(spec.base_seed, spec.full).iter().map(|sc| run_contended(sc, arm.cfg, consts)).collect()
    } else {
        Vec::new()
    };

    let (c_score, c_t) = family_score(&c_runs, baseline_t.map(|(c, _, _)| c), spec.base_seed);
    let (g_score, g_t) = family_score(&g_runs, baseline_t.map(|(_, g, _)| g), spec.base_seed);
    let (d_score, d_t) = family_score(&d_runs, baseline_t.map(|(_, _, d)| d), spec.base_seed);

    // S guard: stored row when present, else the caller's in-process baseline row.
    let s_guard: Vec<SGuardVerdict> = s_runs
        .iter()
        .filter_map(|o| {
            s_baseline
                .iter()
                .find(|b| b.scenario == o.scenario)
                .map(|b| s_guard_verdict(b, o))
        })
        .collect();

    // Matching diagnostics — HOME-ROOM floor (C/G/D/S) and the CONTENDED probe (Family M).
    let home_runs: Vec<&RunOutcome> = c_runs.iter().chain(&g_runs).chain(&d_runs).chain(&s_runs).collect();
    let (ops_mean, ops_p95, edges_per_pass, _max_home, home_gap) = matching_diag(&home_runs);
    let m_refs: Vec<&RunOutcome> = m_runs.iter().collect();
    let (m_mean, m_p95, m_edges, m_max, m_gap) = matching_diag(&m_refs);

    let d_levels_lost = d_runs.iter().map(|o| o.levels_lost).sum();
    let intents_total = c_runs.iter().chain(&g_runs).chain(&d_runs).map(|o| o.intents_emitted).sum();
    let ticks_total = c_runs.iter().chain(&g_runs).chain(&d_runs).map(|o| o.ticks_run as u64).sum();

    let result = ArmResult {
        name: arm.name,
        c: c_score,
        g: g_score,
        d: d_score,
        d_levels_lost,
        s_runs,
        s_guard,
        match_ops_per_tick_mean: ops_mean,
        match_ops_per_tick_p95: ops_p95,
        match_edges_per_pass: edges_per_pass,
        gap: home_gap,
        m_ops_per_tick_mean: m_mean,
        m_ops_per_tick_p95: m_p95,
        m_edges_per_pass: m_edges,
        m_max_edges_per_pass: m_max,
        m_gap,
        intents_total,
        ticks_total,
    };
    (result, (c_t, g_t, d_t))
}

/// Aggregate the matching diagnostics over a run set: (ops/tick mean, ops/tick p95,
/// edges/pass mean, max edges/pass, pooled gap). Zero/None on a set with no market passes.
fn matching_diag(runs: &[&RunOutcome]) -> (f64, u32, f64, u64, Option<crate::market::GapStats>) {
    let mut ops_per_tick: Vec<u32> = Vec::new();
    let mut gap_total = crate::market::GapStats::default();
    let (mut edges, mut passes, mut ops, mut ticks, mut max_edges) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut any_gap = false;
    for o in runs {
        if o.match_passes > 0 {
            ops += o.match_ops;
            edges += o.match_edges;
            passes += o.match_passes;
            ticks += o.ticks_run as u64;
            max_edges = max_edges.max(o.match_max_edges);
            ops_per_tick.push((o.match_ops / o.ticks_run.max(1) as u64) as u32);
        }
        if let Some(g) = &o.match_gap {
            any_gap = true;
            gap_total.samples += g.samples;
            gap_total.greedy_fp += g.greedy_fp;
            gap_total.oracle_fp += g.oracle_fp;
            gap_total.worst_permille = gap_total.worst_permille.max(g.worst_permille);
            gap_total.skipped += g.skipped;
        }
    }
    (
        if ticks == 0 { 0.0 } else { ops as f64 / ticks as f64 },
        percentile_u32(&ops_per_tick, 0.95),
        if passes == 0 { 0.0 } else { edges as f64 / passes as f64 },
        max_edges,
        any_gap.then_some(gap_total),
    )
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The constants sweep (coordinate descent, the 0033 idiom) — the #[ignore] env-driven test
// lives in `tests/` via the bench; the scorer here is pure.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One sweep point's score: the gates-first ranked key over a MARKET-arm evaluation on the
/// fast corpus, paired against a shared baseline.
pub struct SweepScore {
    pub result: ArmResult,
}

impl SweepScore {
    pub fn ranked_key(&self) -> (u8, i64, i64, i64) {
        self.result.ranked_key()
    }
}

/// Evaluate ONE MarketConsts point (full market arm, no oracle — sweep budget) against a
/// pre-run baseline. Deterministic: same point + spec + seed ⇒ identical score.
pub fn evaluate_point(
    consts_point: MarketConsts,
    spec: &TournamentSpec,
    recover: RecoverConsts,
    baseline_t: &PairingMaps,
    s_baseline: &[SBaselineRow],
) -> SweepScore {
    let arm = Arm {
        name: "market",
        cfg: PolicyConfig::market(MarketArmCfg {
            consts: consts_point,
            k4_bodies: true,
            measure_gap: false,
            oracle_period: 25,
            deposit_reselect: true,
            a3_live_control: false,
        }),
    };
    SweepScore { result: run_arm(&arm, spec, recover, Some(baseline_t), s_baseline).0 }
}

/// The default stored-S-baseline path (`ECON_S_BASELINE` overrides): the M2 full-corpus
/// baseline at `be8c8be` (M3 was A/A byte-identical to it, so it IS the current baseline arm).
pub fn s_baseline_path() -> std::path::PathBuf {
    match std::env::var("ECON_S_BASELINE") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runs/econ/full-be8c8be-dirty.json"),
    }
}

/// Stored rows where available, in-process baseline rows for the rest (module docs).
pub fn s_guard_rows(stored: &[SBaselineRow], baseline_s_runs: &[RunOutcome]) -> Vec<SBaselineRow> {
    let mut rows = stored.to_vec();
    for o in baseline_s_runs {
        if !rows.iter().any(|r| r.scenario == o.scenario) {
            rows.push(s_row_from_outcome(o));
        }
    }
    rows
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Reporting.
// ═════════════════════════════════════════════════════════════════════════════════════════════

pub fn family_line(name: &str, f: &FamilyScore) -> String {
    let delta = match &f.delta_t {
        Some(d) => format!("ΔT {:+9.1} CI[{:+8.1},{:+8.1}] n={}", d.mean, d.ci95.0, d.ci95.1, d.n),
        None => "ΔT (baseline)".to_string(),
    };
    format!(
        "  {name}: H={:.4} CI[{:.4},{:.4}] n={}  {delta}  leak={:8.1}e  fin={}/{} dead={} flap/kt={:.0} i/t={:.2}",
        f.h.weighted_mean, f.h.ci95.0, f.h.ci95.1, f.h.n, f.leak_mean, f.finished, f.runs, f.deadlocks, f.flap_per_kt, f.intents_per_tick,
    )
}

pub fn print_arm(r: &ArmResult) {
    println!("── arm {} (gates {}) ──", r.name, if r.gates_held() { "HELD" } else { "FAILED" });
    println!("{}", family_line("C", &r.c));
    println!("{}", family_line("G", &r.g));
    println!("{}", family_line("D", &r.d));
    println!("  D levels lost: {}", r.d_levels_lost);
    for v in &r.s_guard {
        let fails: Vec<String> = v
            .checks
            .iter()
            .filter(|&&(_, _, _, p)| !p)
            .map(|&(m, b, c, _)| format!("{m} {b:.3}→{c:.3}"))
            .collect();
        // Review #4: SURFACE the refill-episode-p95 delta even when it's benign (reported, not
        // gated) — the tuned refill floor's low-riding cost must be VISIBLE, not silent pass=true.
        // Also surface road_stock_min (which IMPROVES under the market — the §D1 low-riding-roads
        // equilibrium made concrete).
        let reported: Vec<String> = v
            .checks
            .iter()
            .filter(|&&(m, _, _, _)| m == "refill_p95_reported" || m == "road_stock_min")
            .map(|&(m, b, c, _)| {
                let label = if m == "refill_p95_reported" { "refill_p95" } else { "road_min" };
                let x = if b > 0.0 { format!(" ({:.1}x)", c / b) } else { String::new() };
                format!("{label} {b:.2}→{c:.2}{x}")
            })
            .collect();
        println!(
            "  S[{}]: {} [{}]{}",
            v.scenario,
            if v.pass { "PASS" } else { "FAIL " },
            reported.join(", "),
            if fails.is_empty() { String::new() } else { format!(" GATE-FAIL({})", fails.join(", ")) }
        );
    }
    if r.match_ops_per_tick_mean > 0.0 {
        println!(
            "  matching (home rooms, floor): ops/tick mean {:.2} p95 {}  edges/pass {:.2}",
            r.match_ops_per_tick_mean, r.match_ops_per_tick_p95, r.match_edges_per_pass
        );
    }
    if let Some(g) = &r.gap {
        println!(
            "  match_optimality_gap (home): pooled {}‰ worst {}‰ over {} samples ({} skipped)",
            g.pooled_permille(), g.worst_permille, g.samples, g.skipped
        );
    }
    // Family M — the CONTENDED probe (review #2): the numbers §D8 #4 and the M5a budget rest on.
    if r.m_ops_per_tick_mean > 0.0 || r.m_edges_per_pass > 0.0 {
        println!(
            "  matching (Family M, CONTENDED): ops/tick mean {:.1} p95 {}  edges/pass {:.1} (max {})",
            r.m_ops_per_tick_mean, r.m_ops_per_tick_p95, r.m_edges_per_pass, r.m_max_edges_per_pass
        );
    }
    if let Some(g) = &r.m_gap {
        println!(
            "  match_optimality_gap (CONTENDED): pooled {}‰ worst {}‰ over {} samples ({} skipped) ← §D8 #4 decides on THIS",
            g.pooled_permille(), g.worst_permille, g.samples, g.skipped
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{foreman_rcl_sweep, DEFAULT_S_TICK_CAP};

    /// The end-state MARKET arm (structure-aware true routed distance + unreachable-arc exclusion)
    /// at the default constants — what the foreman sweep validates and the constants sweep tunes.
    fn end_state_market() -> PolicyConfig {
        PolicyConfig::market(MarketArmCfg {
            consts: MarketConsts::default(),
            k4_bodies: true,
            measure_gap: false,
            oracle_period: 25,
            deposit_reselect: true,
            a3_live_control: false,
        })
    }

    /// ADR 0044 P2 — the FOREMAN-LAYOUT × RCL validation sweep. Every captured foreman layout at the
    /// requested RCL stages, run to a steady-state horizon under the END-STATE market. Asserts the
    /// economy stays HEALTHY across the whole real-layout corpus (not just the curated guard rail):
    /// it never freezes, roads hold, no permanent refill deficit, and — the structure-reachability
    /// proof this workstream needs — at RCL ≥ 4 the market GENERATES edges (refill/haul sinks are
    /// reachable at every real room, so nothing is wrongly excluded as unreachable).
    fn foreman_sweep_check(scenarios: &[SteadyScenario], horizon: u32, verbose: bool) {
        let cfg = end_state_market();
        let recover = RecoverConsts::default();
        assert!(!scenarios.is_empty(), "the sweep yields scenarios");
        let mut failures = Vec::new();
        for sc in scenarios {
            let mut sc = sc.clone();
            sc.tick_cap = horizon;
            let sc = &sc;
            let out = run_steady(sc, cfg, recover);
            let road_ratio = out
                .road_stock
                .last()
                .map(|&(_, h, m)| if m == 0 { 1.0 } else { h as f64 / m as f64 })
                .unwrap_or(1.0);
            let defp95 = percentile_u32(&out.deficit_episodes, 95.0);
            let mut probs = Vec::new();
            if out.deadlocked {
                probs.push("DEADLOCKED".to_string());
            }
            if sc.rcl >= 4 && out.match_passes > 0 && out.match_edges == 0 {
                probs.push("market ran but generated ZERO edges — a refill/haul sink is unreachable".to_string());
            }
            if road_ratio < 0.30 {
                probs.push(format!("road stock collapsed to {road_ratio:.2}"));
            }
            // A PERMANENT open deficit is only a defect at RCL ≥ 4: stocked storage should always keep
            // the spawn servable, so a stuck deficit there means a refill sink is unreachable / wrongly
            // declined. Below RCL 4 the pre-storage container economy is inherently haul-tight — long
            // deficit episodes are the norm (many healthy RCL-1..3 rooms show the same), so gating this
            // avoids flagging that universal transitional behaviour as a failure.
            if sc.rcl >= 4 && out.deficit_open_at_end.is_some_and(|open| open > horizon / 2) {
                probs.push(format!("permanent refill deficit still open {}t at end", out.deficit_open_at_end.unwrap()));
            }
            if verbose || !probs.is_empty() {
                eprintln!(
                    "[foreman-sweep] {:<26} eff_t={:<6} defp95={:<5} edges={:<8} road={:.2} {}",
                    sc.name,
                    out.effective_t,
                    defp95,
                    out.match_edges,
                    road_ratio,
                    if probs.is_empty() { "ok".to_string() } else { probs.join("; ") }
                );
            }
            if !probs.is_empty() {
                failures.push(format!("{}: {}", sc.name, probs.join("; ")));
            }
        }
        assert!(failures.is_empty(), "unhealthy foreman layout×RCL runs:\n{}", failures.join("\n"));
    }

    /// Fast gated SMOKE — the first few layouts at {4,6} for a short horizon: keeps the sweep harness
    /// (and the structure-reachability health checks) from rotting without the full corpus cost. The
    /// exhaustive corpus × every-RCL sweep is `foreman_rcl_sweep_full` (run on demand).
    #[test]
    fn foreman_layouts_healthy_across_rcls() {
        let smoke: Vec<_> = foreman_rcl_sweep(1, &[4, 6]).into_iter().take(6).collect();
        foreman_sweep_check(&smoke, 1_500, false);
    }

    /// The exhaustive sweep: every RCL 1..=full-build over the whole captured corpus at the guard-rail
    /// horizon, with a per-run health table. This is the ADR 0044 P2 validation deliverable — proves
    /// the structure-aware, unreachable-excluding market is healthy on every real foreman layout.
    /// `cargo test --release -p screeps-econ-eval foreman_rcl_sweep_full -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn foreman_rcl_sweep_full() {
        foreman_sweep_check(&foreman_rcl_sweep(1, &[1, 2, 3, 4, 5, 6, 7, 8]), DEFAULT_S_TICK_CAP, true);
    }

    fn env_u32_list(key: &str, default: &[u32]) -> Vec<u32> {
        std::env::var(key)
            .ok()
            .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
            .filter(|v: &Vec<u32>| !v.is_empty())
            .unwrap_or_else(|| default.to_vec())
    }

    /// **The M4 constants sweep** (spec Part C; the 0033 `tune_rover_parameters` idiom):
    /// coordinate descent over the named [`MarketConsts`] axes, 2-3 COARSE values per axis
    /// (robust wins only), gates-first ranked key, fast corpus, incumbent carried forward.
    /// Run: `cargo test --release -p screeps-econ-eval tune_market_constants -- --ignored --nocapture`
    /// Env (comma lists): `ECON_TUNE_V_UPGRADE`, `ECON_TUNE_BUILD_EXT`, `ECON_TUNE_BUILD_ROAD`,
    /// `ECON_TUNE_IMMINENCE`, `ECON_TUNE_REFILL_CAP`, `ECON_TUNE_K4_WAIT`; `ECON_TUNE_FULL=1`
    /// swaps in the full corpus. (The refill FLOOR is no longer swept — review #1 replaced the
    /// flat constant with the derived `instant_spawnability_premium`.)
    #[test]
    #[ignore]
    fn tune_market_constants() {
        let started = std::time::Instant::now();
        let spec = if std::env::var("ECON_TUNE_FULL").ok().as_deref() == Some("1") {
            TournamentSpec::adjudication(true, 1)
        } else {
            TournamentSpec::fast_sweep(1)
        };
        let recover = RecoverConsts::default();
        let stored = load_stored_s_baseline(&s_baseline_path()).unwrap_or_default();
        eprintln!("[sweep] stored S baseline rows: {}", stored.len());
        let base_arm = Arm { name: "baseline", cfg: PolicyConfig::baseline() };
        let (base_result, maps) = run_arm(&base_arm, &spec, recover, None, &stored);
        let s_rows = s_guard_rows(&stored, &base_result.s_runs);
        eprintln!(
            "[sweep] baseline: C H={:.4} G H={:.4} D H={:.4} ({:.1?})",
            base_result.c.h.weighted_mean, base_result.g.h.weighted_mean, base_result.d.h.weighted_mean,
            started.elapsed()
        );

        type Axis = (&'static str, fn(&mut MarketConsts, u32), Vec<u32>);
        let stages: Vec<Vec<Axis>> = vec![
            vec![(
                "v_upgrade",
                (|c, v| c.v_upgrade_milli = v) as fn(&mut MarketConsts, u32),
                env_u32_list("ECON_TUNE_V_UPGRADE", &[600, 1000, 2000]),
            )],
            vec![
                ("build_ext", |c, v| c.build_bid_extension_milli = v, env_u32_list("ECON_TUNE_BUILD_EXT", &[2000, 4000, 8000])),
                ("build_road", |c, v| c.build_bid_road_milli = v, env_u32_list("ECON_TUNE_BUILD_ROAD", &[250, 500, 1000])),
            ],
            vec![(
                "imminence_horizon",
                |c, v| c.imminence_horizon_ticks = v,
                env_u32_list("ECON_TUNE_IMMINENCE", &[375, 750, 1500]),
            )],
            vec![(
                "refill_roi_cap",
                |c, v| c.refill_roi_cap_milli = v,
                env_u32_list("ECON_TUNE_REFILL_CAP", &[10_000, 20_000, 40_000]),
            )],
            vec![("k4_wait_penalty", |c, v| c.k4_wait_penalty_q = v, env_u32_list("ECON_TUNE_K4_WAIT", &[0, 1000, 3000]))],
        ];
        let n_points: usize = stages.iter().flat_map(|s| s.iter().map(|(_, _, vs)| vs.len())).sum();
        eprintln!("[sweep budget] {} points × fast corpus (~4C+4G+4D+2S runs each)", n_points);

        let mut incumbent = MarketConsts::default();
        for (si, stage) in stages.iter().enumerate() {
            let mut points: Vec<(String, MarketConsts)> = vec![("incumbent".into(), incumbent)];
            for (name, setter, values) in stage {
                for &v in values {
                    let mut c = incumbent;
                    setter(&mut c, v);
                    if c != incumbent {
                        points.push((format!("{name}={v}"), c));
                    }
                }
            }
            let mut ranked: Vec<(String, MarketConsts, SweepScore)> = points
                .into_iter()
                .map(|(name, c)| {
                    let score = evaluate_point(c, &spec, recover, &maps, &s_rows);
                    (name, c, score)
                })
                .collect();
            ranked.sort_by(|a, b| b.2.ranked_key().cmp(&a.2.ranked_key()).then(a.0.cmp(&b.0)));
            eprintln!("── stage {si} ──");
            for (name, _, s) in &ranked {
                let r = &s.result;
                eprintln!(
                    "  {name:<20} gates={} C H={:.4} ΔT{:+8.1} | G H={:.4} ΔT{:+8.1} | D H={:.4} ΔT{:+8.1} | leakC={:.0} intents={} ticks={}",
                    r.gates_held(),
                    r.c.h.weighted_mean, r.c.delta_t.as_ref().map(|d| d.mean).unwrap_or(0.0),
                    r.g.h.weighted_mean, r.g.delta_t.as_ref().map(|d| d.mean).unwrap_or(0.0),
                    r.d.h.weighted_mean, r.d.delta_t.as_ref().map(|d| d.mean).unwrap_or(0.0),
                    r.c.leak_mean, r.intents_total, r.ticks_total,
                );
            }
            incumbent = ranked[0].1;
            eprintln!("  → incumbent := {}", ranked[0].0);
        }
        eprintln!("[sweep final] {incumbent:?} ({:.1?} total)", started.elapsed());
    }
}
