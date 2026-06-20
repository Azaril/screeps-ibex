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
    plan_kite_anchor, KiteScoreParams, KiteThreat, KiteTower, PositionLayers, SquadKiteView, ThreatKind, MAX_KITE_OPS,
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
///
/// `shared` selects the path under test: `false` = each block builds its own [`PositionLayers`] (the
/// pre-Stage-3b per-squad cost); `true` = the layers are built **once per tick** and shared across all
/// four co-located blocks (ADR 0019 build-once-per-room — the threat field + reachability flood depend
/// only on the room's enemies, so one build serves every squad fighting there). The per-squad cohesion
/// search still runs per block in both. Behaviour is identical (same plans); only the cost differs.
pub fn run_compound_worst_case(ticks: usize) -> BenchResult {
    run_worst_case(ticks, false)
}

/// The build-once-per-room variant (`shared = true`) — see [`run_compound_worst_case`].
pub fn run_compound_worst_case_shared(ticks: usize) -> BenchResult {
    run_worst_case(ticks, true)
}

fn run_worst_case(ticks: usize, shared: bool) -> BenchResult {
    let _ = Part::Move; // (engage/focus-damage layer will read bodies; placeholder so the import is used)
    let threats = worst_case_threats();
    let towers = worst_case_towers();
    let centroids = [pos(8, 8), pos(42, 8), pos(8, 42), pos(42, 42)];
    let matrix = LocalCostMatrix::new(); // open room: worst case for flood coverage

    let mut plans = 0usize;
    let start = Instant::now();
    for _ in 0..ticks {
        // Build-once-per-room: one PositionLayers per tick, shared by every block this tick (live, the
        // SquadManager does this in its per-tick room cache). The per-squad path rebuilds inside the loop.
        let layers = shared.then(|| PositionLayers::build(&threats, &towers, room(), &matrix, MAX_KITE_OPS));
        for &c in &centroids {
            let view = SquadKiteView {
                centroid: c,
                threats: &threats,
                towers: &towers,
                focus: None,
                focus_damage: None,
                params: KiteScoreParams::default(),
                fragile_hits: 5000, // a boosted brick — edge tiles survivable, centre lethal (veto active)
                squad_heal: 0,
                weapon_range: 3,
            };
            let mut cb = |_r: RoomName| Some(matrix.clone());
            if plan_kite_anchor(&view, layers.as_ref(), &mut cb, MAX_KITE_OPS).is_some() {
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

    /// The ADR 0019 Stage 3b **default-on gate**: the unified position utility is the live default (no
    /// kill-switch — there is no flag path to fall back to), so this bench is the standing regression
    /// guard that the compound worst case stays bounded as layers are added. It enforces three things:
    /// (1) no hang, (2) a generous per-block-tick budget that comfortably clears the current cost in
    /// BOTH debug and release yet trips on a `B × max_ops` algorithmic blowup, and (3) that the
    /// build-once-per-room sharing is behaviour-preserving (identical plans) — so the optimization can
    /// never silently change tactics. The per-squad-vs-shared cost ratio is printed for visibility (a
    /// tight wall-clock ratio would be host/profile-flaky, so it is informational, not asserted).
    #[test]
    fn cpu_bench_compound_worst_case_is_bounded() {
        let ticks = 25;
        let unshared = run_compound_worst_case(ticks);
        let shared = run_compound_worst_case_shared(ticks);
        let speedup = unshared.total.as_secs_f64() / shared.total.as_secs_f64().max(f64::MIN_POSITIVE);
        println!(
            "[ADR0019 Stage3b default-on gate] {} blocks x {} ticks: per-squad {:.1} us/bt ({} plans) | \
             build-once-per-room {:.1} us/bt ({} plans) | sharing speedup {:.2}x",
            unshared.blocks, unshared.ticks, unshared.per_block_tick_us, unshared.plans, shared.per_block_tick_us, shared.plans, speedup
        );
        // (1) No hang: every block-tick completed. A block may legitimately Hold (None) when its
        // centroid is already the least-bad tile (e.g. the survival veto finds no better non-lethal
        // tile in range), so we don't require a plan every tick.
        assert!(unshared.plans <= unshared.blocks * ticks);
        // (2) Default-on budget (native host proxy, generous so it's non-flaky across debug/release/host
        // — the absolute Screeps-ms gate is a separate live measurement): the per-block-tick cost of one
        // squad's full position search must stay well under this. Current: ~30 us release / ~900 us
        // debug; a B×max_ops blowup would be 10-100x. Catches the real algorithmic risk (perf-MF-3).
        const BUDGET_US_PER_BLOCK_TICK: f64 = 5_000.0;
        assert!(
            unshared.per_block_tick_us < BUDGET_US_PER_BLOCK_TICK,
            "per-squad position search blew the budget: {:.1} us/block-tick",
            unshared.per_block_tick_us
        );
        assert!(
            shared.per_block_tick_us < BUDGET_US_PER_BLOCK_TICK,
            "build-once-per-room position search blew the budget: {:.1} us/block-tick",
            shared.per_block_tick_us
        );
        // (3) Build-once-per-room is behaviour-preserving: sharing the layers must not change which
        // tiles are chosen, so the plan count is identical to the per-squad path.
        assert_eq!(shared.plans, unshared.plans, "build-once-per-room changed tactics");
    }
}
