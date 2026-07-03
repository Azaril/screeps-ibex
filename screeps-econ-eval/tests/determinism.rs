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
use screeps_econ_eval::metrics::RecoverConsts;
use screeps_econ_eval::runner::{run_scenario, RunOptions};
use screeps_econ_eval::scenario::fast_catalog;

/// A short cap keeps the fence lanes fast while still covering spawn/harvest/repair/decay flow.
const FENCE_TICK_CAP: u32 = 1_500;

fn opts(s1: bool, permute: bool) -> RunOptions {
    let mut o = RunOptions::new(
        PolicyConfig { s1_allowance: s1 },
        RecoverConsts::default(),
        FENCE_TICK_CAP,
    );
    o.permute_intents = permute;
    o
}

/// One corpus-slice round: (Σ state digests, Σ report digests) over 2 scenarios × bait/control ×
/// both policy arms.
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
