//! ADR 0044 step 2 — the LIVE routed-distance oracle for the haul market.
//!
//! The transfer/market layer prices the pickup→sink haul leg through the narrow
//! [`crate::transfer::market_adapter::HaulDistance`] trait; THIS module is the live implementation
//! it never sees the internals of. It computes the SAME full-tile routed distance the sim's
//! `screeps-econ-eval::RoverMover` uses — so tournament tuning done on the sim transfers to live —
//! but backs it with the live-optimal machinery:
//!
//! * **Pathfinding stays in rover** (the no-one-off rule): rover's engine-backed
//!   [`ScreepsPathfinder`] reads room TERRAIN natively (walls/swamp — no wasm re-parse) and takes a
//!   per-room STRUCTURE overlay from the shared [`CostMatrixCache`] (segment-loaded, already built
//!   by movement this tick). The sim uses rover's pure-Rust `LocalPathfinder` over hand-baked
//!   terrain matrices; both are `PathfindingProvider`s finding the shortest walkable tile path, so
//!   the distance MODEL is identical.
//! * **Static ⇒ cacheable.** The obstacle set is terrain + roads + structure blockers with NO
//!   transient creeps/sites, so a (pickup, sink) distance is a stable structural fact. It's memoized
//!   per pair with a long TTL; steady-state pathfinds ≈ 0 (only a new pair or a road/rampart build
//!   inside the TTL recomputes). The per-tick compute/hit counters are the CPU ship-gate benchmark.

use crate::transfer::market_adapter::HaulDistance;
use screeps::Position;
use screeps_rover::screeps_impl::{ScreepsCostMatrixDataSource, ScreepsPathfinder};
use screeps_rover::{CostMatrixCache, CostMatrixOptions, CostMatrixSystem, PathfindingProvider};
use std::collections::HashMap;

/// TTL (ticks) for a cached routed distance. Structures are static, so this is safe to keep long;
/// the only in-window staleness is a road/rampart build, and a road changes tile COST (fatigue),
/// not the tile COUNT the distance measures — so the value barely moves. Recomputed past this age.
const DIST_TTL: u32 = 1500;

/// Per-search op cap (live CPU budget). Generous for a structure-to-structure route across a handful
/// of rooms, but bounds a pathological long/blocked search: an exhausted search returns `incomplete`
/// ⇒ the pair is treated as unreachable (`None`) and the arc is DECLINED. That is the correct outcome
/// — a haul so long it blows 20k ops is beyond break-even anyway, so declining ≈ what admission does.
const SEARCH_MAX_OPS: u32 = 20_000;

const PLAIN_COST: u8 = 2;
const SWAMP_COST: u8 = 10;

/// The STATIC obstacle set for a haul route: terrain (read natively by the engine pathfinder) +
/// roads + structure blockers, and deliberately NO transient creeps / construction sites / SK-aggro
/// — so the routed distance is a stable structural property we can memoize for [`DIST_TTL`]. Fatigue
/// costs (road 1 / plain 2 / swamp 10) mirror the sim mover so both price haul on the same model.
fn haul_cost_matrix_options() -> CostMatrixOptions {
    CostMatrixOptions {
        structures: true,
        friendly_creeps: false,
        hostile_creeps: false,
        construction_sites: false,
        source_keeper_aggro: false,
        road_cost: 1,
        plains_cost: PLAIN_COST,
        swamp_cost: SWAMP_COST,
        source_keeper_aggro_cost: 50,
        friendly_inactive_construction_site_cost: None,
        friendly_active_construction_site_cost: None,
        hostile_inactive_construction_site_cost: None,
        hostile_active_construction_site_cost: None,
        friendly_creep_proximity: None,
    }
}

/// Pure recompute policy (offline-testable): recompute a MISSING entry, or one older than the TTL.
fn should_recompute(missing: bool, age: u32) -> bool {
    missing || age > DIST_TTL
}

struct CachedDist {
    /// Routed ticks, or `None` when there is NO PATH (unreachable / beyond the op budget). Caching
    /// the `None` matters: it stops us re-running a doomed (or pathologically long) search every tick
    /// for the TTL window — a genuinely unreachable pair only becomes reachable on a layout change.
    dist: Option<u32>,
    cached_at: u32,
}

/// The `(pickup, sink)` routed-distance memo + the per-tick CPU-benchmark counters. A specs
/// `Resource` (one per world), shared into the job loop via [`RoverDistanceOracle`].
#[derive(Default)]
pub struct HaulDistanceService {
    cache: HashMap<(Position, Position), CachedDist>,
    computes_this_tick: u32,
    hits_this_tick: u32,
}

impl HaulDistanceService {
    /// Reset the per-tick benchmark counters (called once at the top of the job pass).
    pub fn reset_tick_counters(&mut self) {
        self.computes_this_tick = 0;
        self.hits_this_tick = 0;
    }

    /// The ship-gate benchmark readout: `(pathfinds this tick, cache hits this tick, cached pairs)`.
    pub fn snapshot(&self) -> (u32, u32, usize) {
        (self.computes_this_tick, self.hits_this_tick, self.cache.len())
    }

    /// Routed tile-distance from `from` to `to` (the pickup→sink leg), memoized per static pair, or
    /// `None` when there is NO PATH — a creep cannot make that delivery, so the market drops the arc
    /// (never a fabricated straight-line price for an unservable haul). On a miss it pathfinds via
    /// rover's engine-backed provider (terrain native; structures from the shared cache).
    pub fn haul_distance(&mut self, from: Position, to: Position, tick: u32, cost_matrix_cache: &mut CostMatrixCache) -> Option<u32> {
        if from == to {
            return Some(0);
        }
        let key = (from, to);
        if let Some(c) = self.cache.get(&key) {
            if !should_recompute(false, tick.saturating_sub(c.cached_at)) {
                self.hits_this_tick += 1;
                return c.dist;
            }
        }
        self.computes_this_tick += 1;
        let dist = compute_routed(from, to, cost_matrix_cache);
        self.cache.insert(key, CachedDist { dist, cached_at: tick });
        dist
    }
}

/// One rover search (engine-backed) → path tile count, or `None` when the sink is unreachable (or
/// beyond the op budget) — the market then declines the arc. Range 1: a hauler delivers ADJACENT to
/// the sink (whose own tile the structure blocks); this is within ~1 tile of the sim's
/// center-to-center distance (negligible in `haul_milli`). Terrain is read natively by the engine;
/// the structure overlay comes from `CostMatrixCache`.
fn compute_routed(from: Position, to: Position, cost_matrix_cache: &mut CostMatrixCache) -> Option<u32> {
    let mut cms = CostMatrixSystem::new(cost_matrix_cache, Box::new(ScreepsCostMatrixDataSource));
    let opts = haul_cost_matrix_options();
    let mut pathfinder = ScreepsPathfinder;
    let mut cb = |room| cms.build_local_cost_matrix(room, &opts).ok();
    let res = pathfinder.search(from, to, 1, &mut cb, SEARCH_MAX_OPS, PLAIN_COST, SWAMP_COST);
    if res.incomplete {
        None
    } else {
        Some(res.path.len() as u32)
    }
}

/// The live [`HaulDistance`] oracle: binds the memo service + the shared cost-matrix cache + the
/// current tick for one job pass. Built per creep in the job loop; the transfer layer sees only the
/// `HaulDistance` trait (no rover / cache types cross the seam).
pub struct RoverDistanceOracle<'a> {
    service: &'a mut HaulDistanceService,
    cost_matrix_cache: &'a mut CostMatrixCache,
    tick: u32,
}

impl<'a> RoverDistanceOracle<'a> {
    pub fn new(service: &'a mut HaulDistanceService, cost_matrix_cache: &'a mut CostMatrixCache, tick: u32) -> Self {
        RoverDistanceOracle {
            service,
            cost_matrix_cache,
            tick,
        }
    }
}

impl HaulDistance for RoverDistanceOracle<'_> {
    fn haul_distance(&mut self, from: Position, to: Position) -> Option<u32> {
        self.service.haul_distance(from, to, self.tick, self.cost_matrix_cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recompute_policy_missing_or_expired() {
        assert!(should_recompute(true, 0), "missing always recomputes");
        assert!(!should_recompute(false, DIST_TTL), "fresh within TTL is a hit");
        assert!(should_recompute(false, DIST_TTL + 1), "expired recomputes");
    }
}
