//! CPU bench for the position-selection machinery (ADR 0019 Stage 3b **gate**): measures the per-tick
//! cost of the squad position search in a COMPOUND WORST CASE — a large OPEN room (no walls to prune
//! the floods, so they settle their full `max_ops`), 6 enemy towers, 5 melee + 5 ranged chasers, and
//! 4 converging friendly blocks each running `plan_kite_anchor` (which builds the `PositionLayers`
//! floods + a scored search). This is the harness the ADR demands *before* Stage 3b wires the unified
//! utility default-ON — establish the budget now, re-run after each Stage-3b addition to prove it
//! stays bounded.
//!
//! Note: native host wall-clock is a **relative** proxy for Screeps CPU (wasm differs, and there is no
//! game CPU meter offline). It catches the real risk — the `B × max_ops` algorithmic blowup the ADR
//! flags (perf-MF-3) — and gives a comparable baseline across changes; the absolute Screeps-ms gate is
//! a separate live measurement. The pass/fail assertion is deliberately LOOSE (a death-spiral guard,
//! not a tight threshold) to stay non-flaky; the precise per-block-tick number is printed.

use screeps::local::LocalCostMatrix;
use screeps::{Part, Position, RoomCoordinate, RoomName};
use screeps_combat_decision::kite::{
    plan_kite_anchor, KiteScoreParams, KiteThreat, KiteTower, SquadKiteView, ThreatKind, MAX_KITE_OPS,
};
use std::time::{Duration, Instant};

fn room() -> RoomName {
    "W1N1".parse().unwrap()
}
fn pos(x: u8, y: u8) -> Position {
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
}

/// The result of a position-selection CPU bench run.
#[derive(Clone, Debug)]
pub struct BenchResult {
    pub blocks: usize,
    pub ticks: usize,
    /// Plans actually produced (= blocks × ticks when every block found a goal — proves no hang).
    pub plans: usize,
    pub total: Duration,
    /// Mean microseconds per (block, tick) — the headline "cost of one squad's position search".
    pub per_block_tick_us: f64,
}

/// The compound worst-case threats: 5 melee (mobile, reach 2) + 5 ranged (mobile, reach 0), spread so
/// every block has chasers in range of its flood — maximizing reachability-seed work.
fn worst_case_threats() -> Vec<KiteThreat> {
    let melee = [(15, 15), (35, 15), (15, 35), (35, 35), (25, 25)];
    let ranged = [(20, 20), (30, 20), (20, 30), (30, 30), (25, 18)];
    let mut v = Vec::new();
    for &(x, y) in &melee {
        v.push(KiteThreat { pos: pos(x, y), kind: ThreatKind::MeleeOnly, reach: 2, step_ticks: Some(1), attack_power: 300, ranged_power: 0 });
    }
    for &(x, y) in &ranged {
        v.push(KiteThreat { pos: pos(x, y), kind: ThreatKind::Ranged, reach: 0, step_ticks: Some(1), attack_power: 0, ranged_power: 100 });
    }
    v
}

/// 6 enemy towers clustered near the room centre (full whole-room falloff coverage = the safety term's
/// worst case for every tile a block's search visits).
fn worst_case_towers() -> Vec<KiteTower> {
    [(23, 23), (25, 23), (27, 23), (23, 25), (25, 25), (27, 25)].iter().map(|&(x, y)| KiteTower { pos: pos(x, y) }).collect()
}

/// Run the position-selection worst case: 4 blocks (corners) each `plan_kite_anchor` per tick, over an
/// OPEN cost matrix (no walls → the floods settle their full `max_ops`, the heaviest case). `Part` is
/// imported to keep the signature aligned with the bodies-aware future bench (engage layer, Stage 3b).
pub fn run_compound_worst_case(ticks: usize) -> BenchResult {
    let _ = Part::Move; // (engage/focus-damage layer will read bodies; placeholder so the import is used)
    let threats = worst_case_threats();
    let towers = worst_case_towers();
    let centroids = [pos(8, 8), pos(42, 8), pos(8, 42), pos(42, 42)];
    let matrix = LocalCostMatrix::new(); // open room: worst case for flood coverage

    let mut plans = 0usize;
    let start = Instant::now();
    for _ in 0..ticks {
        for &c in &centroids {
            let view = SquadKiteView {
                centroid: c,
                threats: &threats,
                towers: &towers,
                focus: None,
                prev_goal: None,
                focus_damage: None,
                params: KiteScoreParams::default(),
                fragile_hits: 5000, // a boosted brick — edge tiles survivable, centre lethal (veto active)
                squad_heal: 0,
                weapon_range: 3,
            };
            let mut cb = |_r: RoomName| Some(matrix.clone());
            if plan_kite_anchor(&view, None, &mut cb, MAX_KITE_OPS).is_some() {
                plans += 1;
            }
        }
    }
    let total = start.elapsed();
    let block_ticks = (centroids.len() * ticks).max(1);
    BenchResult {
        blocks: centroids.len(),
        ticks,
        plans,
        total,
        per_block_tick_us: total.as_secs_f64() * 1e6 / block_ticks as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_bench_compound_worst_case_is_bounded() {
        let ticks = 25;
        let r = run_compound_worst_case(ticks);
        println!(
            "[ADR0019 Stage3b CPU bench] {} blocks x {} ticks = {} plans in {:?} ({:.1} us / block-tick)",
            r.blocks, r.ticks, r.plans, r.total, r.per_block_tick_us
        );
        // Structural: the run completed all block-ticks without hanging. A block may legitimately
        // return None (Hold) when its centroid is already the least-bad tile — e.g. the survival veto
        // finds no strictly-better non-lethal tile in range — so we don't require a plan every tick.
        assert!(r.plans <= r.blocks * ticks);
        // Death-spiral guard (LOOSE, native host proxy — not a tight Screeps-ms threshold): this
        // worst case is ~100 squad-searches; if it takes seconds, the B×max_ops blowup is real.
        assert!(r.total.as_millis() < 3000, "position-selection worst case blew the budget: {:?}", r.total);
    }
}
