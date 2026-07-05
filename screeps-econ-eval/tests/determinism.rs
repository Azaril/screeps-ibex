//! The econ-eval determinism fence (ADR 0040 §D7 SHIP-BLOCKER; M1 spec Part C.8, the
//! tournament.rs:1163-1198 pattern): identical seeded corpus-slice runs must be BIT-identical
//! (spread 0), and intent insertion order must be non-semantic. Both green before the repro gate
//! is trusted.
//!
//! - `econ_smoke_fence` (cheap, always on): one bait scenario, 2 runs, bit-equal digests.
//! - `econ_is_deterministic_over_rounds` (#[ignore], the expensive lane): 5 rounds over a corpus
//!   slice (2 bait scenarios × both policy arms × bait+control), summed digests spread 0.
//!   Run: `cargo test --release -p screeps-econ-eval --test determinism
//!         econ_is_deterministic_over_rounds -- --ignored --nocapture`.
//! - `det_reorder` (#[ignore]): the same slice with every tick's intent list REVERSED before
//!   resolution — bit-identical (the resolver re-orders by creep id / spawn index; the fence
//!   proves the driver leans on that, never on emission order).

use screeps_econ_eval::baseline::PolicyConfig;
use screeps_econ_eval::market::MarketArmCfg;
use screeps_econ_eval::metrics::RecoverConsts;
use screeps_econ_eval::movement::AnalyticMover;
use screeps_econ_eval::runner::{run_scenario, run_world, RunGoal, RunOptions};
use screeps_econ_eval::scenario::{fast_catalog, RushScenario, SteadyScenario};

/// A short cap keeps the fence lanes fast while still covering spawn/harvest/repair/decay flow
/// (and, since M2, upgrade/build/construction-pass flow; since M4, the market pass + oracle).
const FENCE_TICK_CAP: u32 = 1_500;

fn opts(s1: bool, permute: bool) -> RunOptions {
    let mut o = RunOptions::new(
        PolicyConfig { s1_allowance: s1, ..Default::default() },
        RecoverConsts::default(),
        FENCE_TICK_CAP,
    );
    o.permute_intents = permute;
    o
}

/// The M4 market arm with the gap oracle ON — the fence covers bids, floor, the greedy pass,
/// AND the sim-only exact oracle (its fixed-point totals fold into the round digest).
fn market_opts(permute: bool) -> RunOptions {
    let mut o = RunOptions::new(
        PolicyConfig::market(MarketArmCfg { measure_gap: true, ..Default::default() }),
        RecoverConsts::default(),
        FENCE_TICK_CAP,
    );
    o.permute_intents = permute;
    o
}

/// One corpus-slice round: (Σ state digests, Σ report digests) over 2 Family-C scenarios ×
/// bait/control × both policy arms, PLUS (M2 — the new mechanics entered the state/digest) one
/// Family-G greenfield slice (upgrade + build + the construction pass live within 1500 ticks)
/// and one Family-S healthy slice (TTL churn + upgraders + road wear under real traffic).
fn round(permute: bool) -> (u64, u64) {
    let slice: Vec<_> = fast_catalog().into_iter().take(2).collect();
    let (mut sd, mut rd) = (0u64, 0u64);
    for sc in &slice {
        let mut sc = sc.clone();
        sc.tick_cap = FENCE_TICK_CAP;
        for arm in [sc.clone(), sc.control()] {
            for s1 in [false, true] {
                let out = run_scenario(&arm, &opts(s1, permute));
                sd = sd.wrapping_add(out.state_digest);
                rd = rd.wrapping_add(out.report_digest);
            }
        }
    }
    // The M2 G-slice: RCL-8 target never reached inside the cap — pure determinism coverage of
    // the greenfield upgrade/build/construction lanes.
    {
        let mut rush = RushScenario::new("E11N1", 8, 1);
        rush.tick_cap = FENCE_TICK_CAP;
        let (mut world, terrain, info) = rush.instantiate();
        let mut mover = AnalyticMover::new(&terrain);
        let mut o = opts(false, permute).with_goal(RunGoal::Rcl { target: 8 });
        o.construction_phase = rush.seed;
        let out = run_world(&rush.shell(), &mut world, &mut mover, &info, &o);
        sd = sd.wrapping_add(out.state_digest);
        rd = rd.wrapping_add(out.report_digest);
    }
    // The M2 S-slice: the healthy fleet + horizon goal.
    {
        let mut steady = SteadyScenario::new("E12S41", 4, 1);
        steady.tick_cap = FENCE_TICK_CAP;
        let (mut world, terrain, info) = steady.instantiate();
        let mut mover = AnalyticMover::new(&terrain);
        let mut o = opts(false, permute).with_goal(RunGoal::Horizon);
        o.construction_phase = steady.seed;
        let out = run_world(&steady.shell(), &mut world, &mut mover, &info, &o);
        sd = sd.wrapping_add(out.state_digest);
        rd = rd.wrapping_add(out.report_digest);
    }
    // The M4 MARKET slice: bait + control under the full market arm with the exact oracle
    // sampling — the pass, the bids, the floor, K4 bodies AND the oracle's fixed-point totals
    // all enter the round digest (a nondeterministic oracle fails the fence, not just a
    // nondeterministic decision).
    for arm in {
        let mut sc = fast_catalog().remove(0);
        sc.tick_cap = FENCE_TICK_CAP;
        [sc.clone(), sc.control()]
    } {
        let out = run_scenario(&arm, &market_opts(permute));
        sd = sd.wrapping_add(out.state_digest);
        rd = rd.wrapping_add(out.report_digest);
        rd = rd.wrapping_add(out.match_ops).wrapping_add(out.match_edges);
        if let Some(g) = out.match_gap {
            rd = rd.wrapping_add(g.greedy_fp).wrapping_add(g.oracle_fp).wrapping_add(g.samples as u64);
        }
    }
    (sd, rd)
}

/// The cheap always-on fence: two identical runs, bit-equal (state AND report digests).
#[test]
fn econ_smoke_fence() {
    let mut sc = fast_catalog().remove(0);
    sc.tick_cap = FENCE_TICK_CAP;
    let a = run_scenario(&sc, &opts(false, false));
    let b = run_scenario(&sc, &opts(false, false));
    assert_eq!(a.state_digest, b.state_digest, "state digests diverged");
    assert_eq!(a.report_digest, b.report_digest, "report digests diverged");
    assert_eq!(a.recovered_at, b.recovered_at);
}

/// The always-on MARKET smoke fence (M4): two identical market-arm runs bit-equal, including
/// the matching diagnostics and the oracle's fixed-point totals.
#[test]
fn econ_market_smoke_fence() {
    let mut sc = fast_catalog().remove(0);
    sc.tick_cap = FENCE_TICK_CAP;
    let a = run_scenario(&sc, &market_opts(false));
    let b = run_scenario(&sc, &market_opts(false));
    assert_eq!(a.state_digest, b.state_digest, "market state digests diverged");
    assert_eq!(a.report_digest, b.report_digest, "market report digests diverged");
    assert_eq!((a.match_ops, a.match_edges, a.match_passes), (b.match_ops, b.match_edges, b.match_passes));
    let (ga, gb) = (a.match_gap.unwrap(), b.match_gap.unwrap());
    assert_eq!(
        (ga.samples, ga.greedy_fp, ga.oracle_fp, ga.worst_permille, ga.skipped),
        (gb.samples, gb.greedy_fp, gb.oracle_fp, gb.worst_permille, gb.skipped),
        "oracle totals diverged"
    );
    assert!(a.match_passes > 0, "anti-vacuity: the market pass actually ran");
}

/// The expensive fence lane: 5 rounds, digest spread 0.
#[test]
#[ignore]
fn econ_is_deterministic_over_rounds() {
    let baseline = round(false);
    for r in 1..5 {
        let again = round(false);
        assert_eq!(again, baseline, "round {r} diverged from round 0 (spread != 0)");
    }
    println!("[econ determinism] 5 rounds bit-identical: state Σ {:#x}, report Σ {:#x}", baseline.0, baseline.1);
}

/// Permutation invariance: reversed intent insertion each tick, identical history.
#[test]
#[ignore]
fn det_reorder() {
    assert_eq!(round(false), round(true), "intent insertion order leaked into the outcome");
}

// ── The M6 mineral-family fence (labs + minerals): the mineral-economy metrics are reproducible ──

/// A fold of every M6 metric over the mineral corpus into one digest — the fence's instrument.
/// Covers the seeded mineral re-roll (via the world's density path), the reaction pipeline
/// (compound time-to-X), the boost e/t diagnostic, the recovery-lever delta, and the boosted
/// T_RCL probe. Two folds must be bit-identical (spread 0).
fn m6_fold(seed: u32) -> u64 {
    use screeps_econ_eval::mineral::{
        boost_e_t_equivalent, boosted_upgrader_probe, compound_time_to, mineral_catalog,
        recovery_lever_delta, recovery_lever_world,
    };
    use screeps_econ_engine::SimResource;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    for w in [5u32, 10, 20] {
        eat(boost_e_t_equivalent(w) as u64);
    }
    for sc in mineral_catalog(seed, false) {
        let (mut world, _, _, cluster) = sc.instantiate();
        // The instantiated world's state digest folds the seeded mineral density + reroll seed.
        eat(world.state_digest());
        eat(compound_time_to(&mut world, &cluster, 100, 20_000).unwrap_or(u32::MAX) as u64);
        let (world2, _, _, cluster2) = sc.instantiate();
        let p = boosted_upgrader_probe(&world2, &cluster2, 40_000, 20_000);
        eat(p.unboosted_ticks.unwrap_or(u32::MAX) as u64);
        eat(p.boosted_ticks.unwrap_or(u32::MAX) as u64);
        let rw = recovery_lever_world(&sc.layout_room, SimResource::Ghodium, 50_000, seed);
        let r = recovery_lever_delta(&rw, SimResource::Ghodium, 100, 20_000);
        eat(r.with_lever.unwrap_or(u32::MAX) as u64);
        eat(r.without_lever.unwrap_or(u32::MAX) as u64);
    }
    h
}

/// The always-on M6 fence: two folds of the mineral family, bit-identical (the seeded re-roll +
/// the reaction/boost/recovery drivers are all reproducible — no ambient entropy).
#[test]
fn econ_m6_mineral_family_fence() {
    let a = m6_fold(1);
    let b = m6_fold(1);
    assert_eq!(a, b, "M6 mineral-family metrics diverged across runs (spread != 0)");
    // A different seed genuinely varies the world (the mineral re-roll seed + probe seeding).
    assert_ne!(m6_fold(1), m6_fold(2), "the seed must vary the M6 corpus");
}
