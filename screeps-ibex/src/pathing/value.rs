//! Live w-as-priority: the §D5.4 hauler arm on the rover NUMERIC priority lane (ADR 0033,
//! operator-ratified live adoption 2026-07-01 decision (4)). Civilian HAUL-leg movement requests
//! bid a quantized energy-rate estimate instead of the flat `Normal` enum tier, so contested
//! tiles resolve by marginal value (a loaded hauler outranks an empty one; a short-leg delivery
//! outranks a cross-room fetch) while staying STRICTLY inside the (`Low`, `Normal`) anchor band —
//! military and every un-wired job keep their enum tiers and outrank all w-bidders exactly as
//! before (military w needs war-layer objective EV; unblocks after operations/war.rs merges).
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
}
