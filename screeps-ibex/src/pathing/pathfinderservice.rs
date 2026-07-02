//! THE single mission-side pathfinding instance (statics-review M4;
//! ADR 0004 step 4, contract S3): one specs Resource owning the
//! tier-scaled ops pool, same-room target-selection searches, and the
//! inter-room route cache with `find_route` admission control.
//!
//! Absorbs the former `pathbudget` module statics (the pool is plain
//! fields now — wasm is single-threaded; the CAS bought nothing and
//! cost host-test isolation) and the pathfinding half of
//! `findnearest.rs`. The pool bounds the tick's AGGREGATE mission-side
//! pathfinding; per-search caps bound one search; an exhausted pool
//! degrades exactly like a capped-out search ("no path" semantics, or
//! serve-stale for the route cache).
//!
//! **Movement is deliberately NOT in here** — movement is never-shed
//! with an independent CPU-aware budget (screeps-rover), tier-scaled at
//! its own call site with the `MIN_PATHFIND_OPS` floor. The shared
//! input is [`GovernorSnapshot`]'s tier, not a shared pool.

use crate::cpugovernor::Tier;
use screeps::local::Position;
use screeps::pathfinder;
use screeps::*;
use std::collections::HashMap;

/// Per-tick mission-side ops at Normal tier (1 op ≈ 0.001 CPU, so this
/// bounds mission pathfinding at ~20 CPU/tick — generous against
/// typical use; the point is the BOUND, not the throttle).
pub const BASE_MISSION_OPS: u32 = 20_000;

/// Nominal charge for one `find_route` call (room-graph search has no
/// ops parameter — this is accounting plus admission control).
pub const FIND_ROUTE_NOMINAL_OPS: u32 = 2_000;

/// Ops cap for one same-room search (P1.B1 / IBEX-035, ADR 0004 step
/// 1). The engine default is 2000 ops PER SEARCH and
/// [`PathfinderService::nearest_by_path`] runs one search PER
/// CANDIDATE — the uncapped worst case is candidates × 2000 on a
/// single decision. A single 50×50 room cannot usefully consume more
/// than ~500 ops; a capped-out search returns an incomplete/empty
/// path, which every caller already treats as "no path".
pub const SAME_ROOM_MAX_OPS: u32 = 500;

/// Pool size for a tier (pure; fixture-tested).
pub fn pool_for_tier(tier: Tier) -> u32 {
    match tier {
        Tier::Normal => BASE_MISSION_OPS,
        Tier::Conserve => BASE_MISSION_OPS / 2,
        Tier::Critical => BASE_MISSION_OPS / 4,
    }
}

/// Pure recompute decision for [`PathfinderService::route_distance`]
/// (P1.B1 bucket-guard): missing entries always compute; expired
/// entries recompute except under Critical, where stale is served.
fn should_recompute_route(missing: bool, expired: bool, tier: Tier) -> bool {
    missing || (expired && tier != Tier::Critical)
}

/// Pricing-policy discriminator for the inter-room route cache (REC-024).
/// Routes computed under different room-pricing policies must never share
/// cache entries: the claim oracle DENIES hostile-owned corridor rooms (the
/// mover's `HostileBehavior::Deny`) while the standard policy only up-prices
/// them — one `(from, to)` pair can be reachable under one policy and not the
/// other. The tag IS the pricing contract: every caller passing a given
/// policy must supply the same pricing function, because cached answers are
/// shared across callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutePolicy {
    /// Legacy live-visibility pricing (`game::rooms()`, see
    /// [`standard_room_cost`]): hostile-owned rooms cost 10.0 but stay
    /// traversable. Every pre-REC-024 caller (war travel estimates, salvage,
    /// build feasibility) stays here — military routes must still price a
    /// route TO a hostile-owned target room.
    Standard,
    /// Economy/claim-corridor pricing from cached `RoomData` intel
    /// (`routepricing::economy_route_cost`): hostile rooms are DENIED, exactly
    /// like the claimer's own mover (`HostileBehavior::Deny`), so the reach
    /// oracle can never approve a corridor the creep will refuse to walk.
    ClaimCorridor,
}

/// A cached inter-room route answer (`Copy` — returned by value so the
/// service borrow ends at the call).
#[derive(Debug, Clone, Copy)]
pub struct CachedRoute {
    /// Number of room transitions (u32::MAX = unreachable).
    pub hops: u32,
    /// Estimated travel ticks (hops * 50).
    pub travel_ticks: u32,
    /// Tick the entry was computed.
    pub cached_at: u32,
    /// Whether the route was found (false = no path).
    pub reachable: bool,
}

/// TTL for route-cache entries in ticks. Room exits are static, but
/// room costs change when ownership changes. 1000 ticks (~16 minutes)
/// is a reasonable TTL — ownership changes are infrequent.
const ROUTE_TTL: u32 = 1_000;

/// The single mission-side pathfinding instance. specs Resource;
/// `Default` = full Normal pool (first-tick parity with the old static
/// init: the first VM tick must not shed pathfinding spuriously).
pub struct PathfinderService {
    /// Governor tier cached at tick start — the service's one governor
    /// input (kills the deep `cpugovernor` reads of the static era).
    tier: Tier,
    /// Tick cap (tier-scaled) and what remains of it.
    pool: u32,
    remaining: u32,
    /// Grants refused on an exhausted pool (saturation telemetry,
    /// ADR 0004 step 2; internal until the seg-57 schema gains a
    /// field — additive bump, EP-5.5).
    denied: u32,
    /// Inter-room route cache (ephemeral — survives within a VM
    /// lifecycle, not across resets; entries lazily populated, TTL'd).
    /// Keyed by [`RoutePolicy`] as well as the endpoints so answers computed
    /// under different pricing policies never contaminate each other
    /// (REC-024).
    routes: HashMap<(RoomName, RoomName, RoutePolicy), CachedRoute>,
}

impl Default for PathfinderService {
    fn default() -> Self {
        PathfinderService {
            tier: Tier::Normal,
            pool: BASE_MISSION_OPS,
            remaining: BASE_MISSION_OPS,
            denied: 0,
            routes: HashMap::new(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl PathfinderService {
    /// Tick-start reset (`metrics::tick_start`): re-arm the pool at the
    /// tick's tier. The route cache persists across ticks.
    pub fn begin_tick(&mut self, tier: Tier) {
        self.tier = tier;
        let pool = pool_for_tier(tier);
        self.pool = pool;
        self.remaining = pool;
        self.denied = 0;
    }

    /// Take up to `want` ops from the pool; returns the grant (0 when
    /// the pool is exhausted — callers degrade like a capped-out
    /// search).
    pub fn take_ops(&mut self, want: u32) -> u32 {
        let grant = want.min(self.remaining);
        if grant == 0 {
            self.denied += 1;
            return 0;
        }
        self.remaining -= grant;
        grant
    }

    /// Telemetry: (pool, consumed) this tick (seg-57 `pathing` block).
    pub fn snapshot(&self) -> (u32, u32) {
        (self.pool, self.pool.saturating_sub(self.remaining))
    }

    /// Nearest candidate by REAL path length: one pool-clamped,
    /// per-search-capped, same-room search per candidate (absorbs
    /// `find_nearest_from` + `PathFinderHelpers`; sole production
    /// client today: dismantle target selection). An exhausted pool or
    /// capped-out search skips the candidate — `None` when nothing is
    /// reachable, the "no path" degradation callers already handle.
    pub fn nearest_by_path<T>(&mut self, from: Position, candidates: impl IntoIterator<Item = T>, range: u32) -> Option<T>
    where
        T: HasPosition,
    {
        candidates
            .into_iter()
            .filter_map(|candidate| {
                let ops = self.take_ops(SAME_ROOM_MAX_OPS);
                if ops == 0 {
                    return None;
                }
                // PathFinder.search ignores creeps by default; structures
                // are ignored without a cost-matrix callback (the legacy
                // helper's documented semantics, preserved).
                let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(ops);
                let result = pathfinder::search(from, candidate.pos(), range, Some(options));
                let path = result.path();
                if path.is_empty() {
                    None
                } else {
                    Some((path.len(), candidate))
                }
            })
            .min_by_key(|(length, _)| *length)
            .map(|(_, candidate)| candidate)
    }

    /// Cached inter-room route distance under the [`RoutePolicy::Standard`]
    /// legacy pricing, computing on miss. See [`Self::route_distance_via`]
    /// for the cache/budget semantics.
    pub fn route_distance(&mut self, from: RoomName, to: RoomName, current_tick: u32) -> CachedRoute {
        self.route_distance_via(from, to, current_tick, RoutePolicy::Standard, &standard_room_cost)
    }

    /// Convenience: estimated travel ticks, or None if unreachable.
    pub fn travel_ticks(&mut self, from: RoomName, to: RoomName, current_tick: u32) -> Option<u32> {
        self.travel_ticks_via(from, to, current_tick, RoutePolicy::Standard, &standard_room_cost)
    }

    /// Cached inter-room route distance under a caller-supplied pricing
    /// policy, computing on miss. The pricing POLICY belongs to the caller
    /// (`room_cost`: per-room cost, `None` = deny/impassable); the route
    /// algorithm, budget, and cache stay here (the no-one-off-pathfinding
    /// rule). `policy` keys the cache — see [`RoutePolicy`] for the
    /// same-policy-same-pricer contract.
    ///
    /// Bucket-guarded (P1.B1 / ADR 0004 step 1): under a Critical tier,
    /// TTL-expired entries are served STALE instead of recomputed —
    /// `find_route` storms are exactly the leak the governor exists to
    /// stop, and a stale route (ownership changes are slow) is strictly
    /// better than a fabricated answer. Missing entries still compute
    /// at any tier: callers need SOME answer, and a single `find_route`
    /// is not the storm. Each compute charges the pool
    /// [`FIND_ROUTE_NOMINAL_OPS`] (admission control — only TTL
    /// refreshes yield to an exhausted pool).
    pub fn route_distance_via(
        &mut self,
        from: RoomName,
        to: RoomName,
        current_tick: u32,
        policy: RoutePolicy,
        room_cost: &dyn Fn(RoomName) -> Option<f64>,
    ) -> CachedRoute {
        let entry = self.routes.get(&(from, to, policy));
        let missing = entry.is_none();
        let expired = entry
            .map(|entry| current_tick.saturating_sub(entry.cached_at) > ROUTE_TTL)
            .unwrap_or(false);

        if should_recompute_route(missing, expired, self.tier) {
            let granted = self.take_ops(FIND_ROUTE_NOMINAL_OPS);
            if missing || granted > 0 {
                let route = Self::compute_route(from, to, current_tick, room_cost);
                self.routes.insert((from, to, policy), route);
            }
        }

        *self.routes.get(&(from, to, policy)).expect("route entry just ensured")
    }

    /// Convenience: estimated travel ticks under a caller-supplied pricing
    /// policy, or None if unreachable.
    pub fn travel_ticks_via(
        &mut self,
        from: RoomName,
        to: RoomName,
        current_tick: u32,
        policy: RoutePolicy,
        room_cost: &dyn Fn(RoomName) -> Option<f64>,
    ) -> Option<u32> {
        let entry = self.route_distance_via(from, to, current_tick, policy, room_cost);
        if entry.reachable {
            Some(entry.travel_ticks)
        } else {
            None
        }
    }

    fn compute_route(from: RoomName, to: RoomName, tick: u32, room_cost: &dyn Fn(RoomName) -> Option<f64>) -> CachedRoute {
        if from == to {
            return CachedRoute {
                hops: 0,
                travel_ticks: 0,
                cached_at: tick,
                reachable: true,
            };
        }

        // find_route with the caller's room pricing; `None` maps to an
        // infinite cost, which the engine treats as impassable. Closed rooms
        // are handled internally by find_route.
        let options =
            game::map::FindRouteOptions::new().room_callback(|room_name, _from_room| room_cost(room_name).unwrap_or(f64::INFINITY));

        match game::map::find_route(from, to, Some(options)) {
            Ok(steps) => {
                let hops = steps.len() as u32;
                CachedRoute {
                    hops,
                    travel_ticks: hops * 50,
                    cached_at: tick,
                    reachable: true,
                }
            }
            Err(_) => CachedRoute {
                hops: u32::MAX,
                travel_ticks: u32::MAX,
                cached_at: tick,
                reachable: false,
            },
        }
    }
}

/// The [`RoutePolicy::Standard`] legacy pricing (the pre-REC-024 inline
/// callback, verbatim): live `game::rooms()` visibility only — an INVISIBLE
/// room (most remote corridors) always prices at the 2.0 default, and
/// hostile-owned rooms are up-priced but never denied. Kept for the callers
/// whose routes must remain traversable to (or through) hostile territory
/// (war travel estimates); economy reach oracles should price from cached intel
/// instead (`routepricing::economy_route_cost` under
/// [`RoutePolicy::ClaimCorridor`]).
fn standard_room_cost(room_name: RoomName) -> Option<f64> {
    if let Some(room) = game::rooms().get(room_name) {
        if let Some(controller) = room.controller() {
            if controller.my() {
                return Some(1.0);
            }
            if controller.owner().is_some() {
                // Owned by someone else -- high cost to avoid.
                return Some(10.0);
            }
            if controller.reservation().is_some() {
                return Some(2.0);
            }
        }
    }
    // Default cost for unknown/neutral rooms.
    Some(2.0)
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
    /// telemetry tracks consumption. Per-instance now — two services
    /// in one test cannot cross-talk (the M4 test-isolation payoff;
    /// the old static-pool twin of this test could be drained by any
    /// parallel test that pathfound).
    #[test]
    fn take_drains_and_clamps() {
        let mut service = PathfinderService::default();
        service.begin_tick(Tier::Critical); // 5000
        assert_eq!(service.take_ops(2_000), 2_000);
        assert_eq!(service.take_ops(2_000), 2_000);
        // Only 1000 left — partial grant.
        assert_eq!(service.take_ops(2_000), 1_000);
        assert_eq!(service.take_ops(500), 0);
        let (pool, consumed) = service.snapshot();
        assert_eq!(pool, 5_000);
        assert_eq!(consumed, 5_000);
        service.begin_tick(Tier::Normal);
        assert_eq!(service.snapshot(), (20_000, 0));

        // An untouched second instance is unaffected.
        let other = PathfinderService::default();
        assert_eq!(other.snapshot(), (20_000, 0));
    }

    /// First-tick parity pin: `Default` must equal the full Normal
    /// pool (the old statics initialized this way — a smaller default
    /// would shed pathfinding spuriously on the first VM tick).
    #[test]
    fn default_is_full_normal_pool() {
        let service = PathfinderService::default();
        assert_eq!(service.tier, Tier::Normal);
        assert_eq!(service.snapshot(), (BASE_MISSION_OPS, 0));
    }

    #[test]
    fn stale_routes_are_served_under_critical_only() {
        for tier in [Tier::Normal, Tier::Conserve, Tier::Critical] {
            // Missing entries always compute — a single find_route is
            // not the storm the guard exists to stop.
            assert!(should_recompute_route(true, false, tier), "{tier:?}");
            // Fresh entries never recompute.
            assert!(!should_recompute_route(false, false, tier), "{tier:?}");
        }
        // Expired: recompute normally, serve stale under Critical.
        assert!(should_recompute_route(false, true, Tier::Normal));
        assert!(should_recompute_route(false, true, Tier::Conserve));
        assert!(!should_recompute_route(false, true, Tier::Critical));
    }
}
