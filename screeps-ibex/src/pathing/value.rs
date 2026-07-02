//! Live w-as-priority: the §D5.4 civilian arms on the rover NUMERIC priority lane (ADR 0033,
//! operator-ratified live adoption 2026-07-01 decision (4)). Civilian movement requests bid a
//! quantized energy-rate estimate instead of the flat `Normal` enum tier, so contested tiles
//! resolve by marginal value (a loaded hauler outranks an empty one; a short-leg delivery
//! outranks a cross-room fetch; a fat builder outranks a scout) while staying STRICTLY inside
//! the (`Low`, `Normal`) anchor band — military and every un-wired job keep their enum tiers and
//! outrank all w-bidders exactly as before (military w needs war-layer objective EV; unblocks
//! after operations/war.rs merges). Wired arms: HAUL legs ([`haul_move_bid`]), worker
//! BUILD/UPGRADE work-travel ([`worker_travel_bid`]), CLAIM/RESERVE travel
//! ([`claim_travel_bid`]), scout travel ([`SCOUT_INTEL_BID`]).
//!
//! This is the live mirror of rover-eval's `value.rs` hauler reduction (`w = r·Δ ≈ Q / T*_rtt`),
//! collapsed to what is cheaply knowable on-tick with NO pathfinding: `T*_rtt ≈ 2 × chebyshev(d)`
//! (round trip at speed 1; roads/fatigue deliberately ignored — a rank, not a physics estimate).
//! Pure integer math end-to-end (quantize-before-order determinism fence: no float ever reaches
//! an ordering).

use screeps::{HasPosition, Position};

/// Floor of the civilian w band: strictly above `MovementPriority::Low.anchor_value()` (0), so
/// even a zero-value bid still outranks a shoveable idle.
pub const W_BID_MIN: i64 = 1;

/// Ceiling of the civilian w band: strictly below `MovementPriority::Normal.anchor_value()`
/// (1_000_000), so no w-bidder ever outranks an enum-tier `Normal` request. At milli-e/t
/// quantization this caps the expressible rate at ~1000 e/t — far above any real hauler leg.
pub const W_BID_MAX: i64 = 999_999;

/// The quantized hauler-arm bid: `w = energy / (2 × chebyshev(from, to))` e/t (round-trip
/// denominator, min 1 so an adjacent leg never divides by zero), quantized ×1000 (milli-e/t,
/// rover-eval `quantize_w`) and clamped to the civilian band. `energy` is the cargo the leg
/// moves — carried when loaded, capacity when heading to a pickup (the caller picks; see
/// [`haul_move_bid`]). `Position` subtraction is world-absolute, so cross-room legs price by
/// true Chebyshev distance. Truncating integer division: exact, allocation-free, deterministic.
pub fn quantized_haul_w(energy: u32, from: Position, to: Position) -> i64 {
    let d = from.get_range_to(to).max(1) as i64;
    (i64::from(energy) * 1000 / (2 * d)).clamp(W_BID_MIN, W_BID_MAX)
}

/// [`quantized_haul_w`] for a live creep: carried-or-capacity energy (carried when loaded — the
/// delivery leg's at-risk cargo; full store capacity when empty — the pickup leg's expected
/// cargo), from the creep's position to its movement destination. Two store reads + integer
/// math; NO pathfinding, no allocation (the CPU-shape contract for a per-move-request helper).
pub fn haul_move_bid(creep: &screeps::Creep, destination: Position) -> i64 {
    let carried = creep.store().get_used_capacity(None);
    let energy = if carried > 0 {
        carried
    } else {
        creep.store().get_capacity(None)
    };
    quantized_haul_w(energy, creep.pos(), destination)
}

/// The SCOUT travel bid: rover-eval `value.rs` decision (3)'s `EPSILON_INTEL` — a 1×MOVE scout's
/// amortized upkeep (`50 / CREEP_LIFE_TIME` = 50/1500 e/t), quantized to the shared milli-e/t
/// lane (truncates to 33). Value-of-information has NO landed kernel; this is the DECLARED
/// policy floor standing in for one (same stance as the eval kernel's `Role::Scout` rail), so a
/// scout yields every contested tile to any real cargo/work bid but still outranks a shoveable
/// idle (`W_BID_MIN` ≤ 33 by construction of the band).
pub const SCOUT_INTEL_BID: i64 = 50 * 1000 / screeps::CREEP_LIFE_TIME as i64;

/// The claim rail's hazard-smoothing reference slack — rover-eval `value.rs` decision (3)'s
/// `S_REF = 100` (the §D5.4 claim rail: `w = min(V, V/max(slack, S_REF))`; a slack-rich claimer
/// bids exactly `V/S_REF`, the HARD FLOOR shape used live below).
pub const CLAIM_HAZARD_S_REF: i64 = 100;

/// The quantized worker (BUILD/UPGRADE) work-travel bid: `w = work_parts × k` e/t where `k` is
/// the per-WORK conversion rate of the leg's job (`BUILD_POWER` = 5, `UPGRADE_CONTROLLER_POWER`
/// = 1 — natively energy/tick, no unit conversion), quantized ×1000 and clamped to the civilian
/// band. The §D5.4 worker rail's rate term with the supply cap deliberately dropped: supply rate
/// is not cheaply knowable at a per-move-request call site, and overbidding a supply-starved
/// worker only costs a tile-contest ordering, not energy. Zero alive WORK floors at `W_BID_MIN`.
pub fn quantized_worker_w(work_parts: u32, k_e_t: u32) -> i64 {
    (i64::from(work_parts) * i64::from(k_e_t) * 1000).clamp(W_BID_MIN, W_BID_MAX)
}

/// [`quantized_worker_w`] for a live creep: WORK counted from the ALIVE body (rover-eval worker
/// rail: a chewed-up worker's bid degrades with its surviving parts). One body iteration +
/// integer math; no pathfinding, no allocation.
pub fn worker_travel_bid(creep: &screeps::Creep, k_e_t: u32) -> i64 {
    let work_parts = creep
        .body()
        .iter()
        .filter(|p| p.hits() > 0 && p.part() == screeps::Part::Work)
        .count() as u32;
    quantized_worker_w(work_parts, k_e_t)
}

/// The quantized CLAIM/RESERVE travel bid, HARD-FLOOR form: `w = V / S_REF` e/t (the rover-eval
/// claim rail's slack-rich limit — see [`CLAIM_HAZARD_S_REF`]), quantized ×1000 and clamped to
/// the civilian band. `V` is the claim value STOCK in energy; the live floor below feeds it the
/// claimer's own body cost — the cheapest correct lower bound (the mission already judged the
/// room worth at least the body it spawned; ADR 0038's `room_net_roi` lands the REAL value at
/// mission level, but no existing serialized job field carries it and adding one is a WFV bump —
/// deliberately avoided, see the module doc).
pub fn quantized_claim_floor_w(body_cost_e: u32) -> i64 {
    (i64::from(body_cost_e) * 1000 / CLAIM_HAZARD_S_REF).clamp(W_BID_MIN, W_BID_MAX)
}

/// [`quantized_claim_floor_w`] for a live claimer/reserver: `V` = the creep's full body cost
/// (sunk capital — hit-point damage does not un-spend it, so ALL parts count, unlike the worker
/// rate above). One body iteration + integer math.
pub fn claim_travel_bid(creep: &screeps::Creep) -> i64 {
    let body_cost: u32 = creep.body().iter().map(|p| p.part().cost()).sum();
    quantized_claim_floor_w(body_cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            "W5N5".parse::<RoomName>().unwrap(),
        )
    }

    /// The hauler reduction, exactly: 500 energy over a 10-tile leg = 500/(2·10) = 25 e/t
    /// = 25_000 milli-e/t. Inside the band, untouched by the clamps.
    #[test]
    fn bid_is_energy_over_round_trip_in_milli_e_t() {
        assert_eq!(quantized_haul_w(500, pos(10, 25), pos(20, 25)), 25_000);
        // Chebyshev, not Manhattan: the diagonal 10-tile leg prices identically.
        assert_eq!(quantized_haul_w(500, pos(10, 15), pos(20, 25)), 25_000);
    }

    /// More cargo or a shorter leg outranks — the ordering the lane exists for.
    #[test]
    fn loaded_and_near_outrank_empty_and_far() {
        let far = quantized_haul_w(500, pos(1, 25), pos(45, 25));
        let near = quantized_haul_w(500, pos(40, 25), pos(45, 25));
        assert!(near > far);
        let light = quantized_haul_w(100, pos(10, 25), pos(20, 25));
        let heavy = quantized_haul_w(800, pos(10, 25), pos(20, 25));
        assert!(heavy > light);
    }

    /// Band clamps: zero energy floors at `W_BID_MIN` (still above the Low anchor 0); an
    /// adjacent max-cargo leg ceilings at `W_BID_MAX` (still below the Normal anchor 1M).
    /// The band invariant vs the enum anchors is pinned in rover (`anchor_value`).
    #[test]
    fn clamps_stay_strictly_inside_the_low_normal_band() {
        assert_eq!(quantized_haul_w(0, pos(10, 25), pos(20, 25)), W_BID_MIN);
        // 2500 energy at range 1: 2500·1000/2 = 1_250_000 → clamped under Normal's anchor.
        assert_eq!(quantized_haul_w(2500, pos(10, 25), pos(11, 25)), W_BID_MAX);
        assert!(W_BID_MIN > screeps_rover::MovementPriority::Low.anchor_value());
        assert!(W_BID_MAX < screeps_rover::MovementPriority::Normal.anchor_value());
    }

    /// Same-tile degenerate leg (already at destination but a request was still issued):
    /// distance floors at 1, no divide-by-zero, bid stays in band.
    #[test]
    fn zero_distance_floors_at_one() {
        assert_eq!(quantized_haul_w(100, pos(10, 25), pos(10, 25)), 50_000);
    }

    /// The scout floor IS rover-eval decision (3)'s EPSILON_INTEL (50/1500 e/t), quantized:
    /// 50·1000/1500 = 33 milli-e/t — in band, and below any real 1-e/t work bid.
    #[test]
    fn scout_bid_is_the_declared_intel_floor() {
        assert_eq!(SCOUT_INTEL_BID, 33);
        assert!(SCOUT_INTEL_BID >= W_BID_MIN && SCOUT_INTEL_BID <= W_BID_MAX);
        assert!(SCOUT_INTEL_BID < quantized_worker_w(1, screeps::UPGRADE_CONTROLLER_POWER));
    }

    /// The worker rail: `WORK × k`, natively e/t — a 2×WORK builder bids 10 e/t (k=5), the SAME
    /// body upgrading bids 2 e/t (k=1); the ordering the k split exists for. Zero WORK floors at
    /// the band minimum (still outranks a shoveable idle, never a real bid).
    #[test]
    fn worker_bids_scale_with_work_and_job_rate() {
        assert_eq!(quantized_worker_w(2, screeps::BUILD_POWER), 10_000);
        assert_eq!(quantized_worker_w(2, screeps::UPGRADE_CONTROLLER_POWER), 2_000);
        assert!(quantized_worker_w(2, screeps::BUILD_POWER) > quantized_worker_w(2, screeps::UPGRADE_CONTROLLER_POWER));
        assert_eq!(quantized_worker_w(0, screeps::BUILD_POWER), W_BID_MIN);
        // A GCL-farm 15×WORK upgrader stays in band.
        assert!(quantized_worker_w(15, screeps::UPGRADE_CONTROLLER_POWER) <= W_BID_MAX);
    }

    /// The claim hard floor: `V/S_REF` with V = body cost. A minimal CLAIM+MOVE claimer
    /// (600+50 = 650 e) bids 6.5 e/t = 6_500 milli — above the scout floor, below a loaded
    /// hauler's short leg; a fatter reserver scales linearly and stays in band.
    #[test]
    fn claim_floor_is_body_cost_over_s_ref() {
        assert_eq!(quantized_claim_floor_w(650), 6_500);
        assert!(quantized_claim_floor_w(650) > SCOUT_INTEL_BID);
        assert_eq!(quantized_claim_floor_w(2 * 650), 13_000);
        assert_eq!(quantized_claim_floor_w(0), W_BID_MIN);
        assert!(quantized_claim_floor_w(50 * 650) <= W_BID_MAX);
    }
}
