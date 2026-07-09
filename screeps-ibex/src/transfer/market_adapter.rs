//! ADR 0040 M5a — the **live e/t market selection adapter**. The bot's analog of the sim's
//! `screeps-econ-eval::market::MarketRuntime::{begin_tick, market_pass}`: it prices a room's
//! deposit sinks in the e/t currency ([`screeps_econ_decision::sink_economics`]), computes the
//! room opportunity floor, runs the SHARED market-selection kernel
//! ([`screeps_econ_decision::market::market_pass`]) over the room's idle carriers, gates
//! Use-lane withdraws + repair admission on the floor, and publishes the readout
//! ([`super::transfersystem::MarketBidSummary`]).
//!
//! **Parity by construction (spec Part 2 coverage).** The bot and the sim BOTH build the same
//! [`market::MarketDeposit`]/[`MarketPickup`]/[`MarketCarrier`] DTOs and call the ONE kernel, so
//! equal DTOs ⇒ byte-identical assignments. The M5a parity test
//! (`tests/market_parity.rs`) asserts exactly that against the sim driver: it builds a fixture
//! world's DTOs on this live path and on the sim's `market_pass`, and the two assignment sets
//! are equal — the live-path coverage the "no offline harness for the live path" concern asked
//! for.
//!
//! This module is the pure adapter: it owns the DTO shapes + the floor/admission arithmetic and
//! calls the kernel. The plumbing that gathers a room's real sinks/sources/carriers from the
//! live `game::*` world (structure health, controller state, the spawn preview) is the caller's
//! (the transfer/haul systems); this keeps the adapter JS-free and unit-testable, exactly like
//! the sim's DTO adapters.

use screeps_econ_decision::market::{self, MarketAssignment, MarketCarrier, MarketDeposit, MarketPickup};
use screeps_econ_decision::sink_economics::{self as econ, MarketConsts};

/// One room's market inputs for a tick — the priced deposits, the haul-lane pickups, and the
/// idle carriers. Built by the caller from the live world (or by the parity test from a
/// fixture); consumed by [`run_room_market`].
pub struct RoomMarketInput {
    /// Priced deposit sinks (bids from [`econ`]). `sink` is an adapter-scoped index the caller
    /// maps back to a real `TransferTarget`.
    pub deposits: Vec<MarketDeposit>,
    /// Haul-lane pickup sources.
    pub pickups: Vec<MarketPickup>,
    /// Idle carriers (haulers + gated harvesters).
    pub carriers: Vec<MarketCarrier>,
}

/// One room's market result — the per-carrier assignments + the published floor + the top unmet
/// bids for the readout.
pub struct RoomMarketResult {
    pub assignments: Vec<MarketAssignment>,
    /// The opportunity floor (highest materially-unmet deposit bid, milli-e/t).
    pub opportunity_floor: u32,
    /// The top-3 unmet deposit bids (descending) — the readout payload.
    pub top_unmet_bids: Vec<u32>,
    /// The pass CPU diagnostics (the §D3 budget instrument).
    pub stats: market::MarketPassStats,
}

/// The opportunity floor over a room's priced deposits (mirrors the sim `begin_tick`): the
/// highest MATERIALLY-unmet deposit bid ([`econ::opportunity_floor`]).
pub fn room_opportunity_floor(consts: &MarketConsts, deposits: &[MarketDeposit]) -> u32 {
    econ::opportunity_floor(consts, deposits.iter().map(|d| (d.bid_milli, d.unfulfilled)))
}

/// The top-N unmet deposit bids (descending) for the readout — deposits with a materially-unmet
/// amount, highest bid first.
pub fn top_unmet_bids(consts: &MarketConsts, deposits: &[MarketDeposit], n: usize) -> Vec<u32> {
    let mut bids: Vec<u32> = deposits
        .iter()
        .filter(|d| d.unfulfilled >= consts.floor_material_min_e)
        .map(|d| d.bid_milli)
        .collect();
    bids.sort_unstable_by(|a, b| b.cmp(a));
    bids.truncate(n);
    bids
}

/// Run one room's market pass (spec Part 2, mirroring the sim `begin_tick` + `market_pass`
/// sequence): compute the floor, run the shared kernel, and package the result + readout.
/// `same_structure(src, sink)` reports whether a pickup source and a deposit sink are the same
/// structure (the caller's `TransferTarget` identity; the sim's `SrcKey`/`SinkKey`).
pub fn run_room_market(
    consts: &MarketConsts,
    input: &RoomMarketInput,
    same_structure: impl Fn(u32, u32) -> bool,
) -> RoomMarketResult {
    let opportunity_floor = room_opportunity_floor(consts, &input.deposits);
    let top = top_unmet_bids(consts, &input.deposits, 3);
    // ADR 0044: live runs the reduced-cost admission ON — plains road factor for the haul
    // subtraction (the shipped behavior; the sim toggles this per arm, live never does).
    let out = market::market_pass(
        &input.carriers,
        &input.deposits,
        &input.pickups,
        screeps_econ_decision::sink_economics::HAUL_ROAD_Q_PLAINS_PERMILLE,
        same_structure,
    );
    RoomMarketResult {
        assignments: out.assignments,
        opportunity_floor,
        top_unmet_bids: top,
        stats: out.stats,
    }
}

/// Use-lane withdraw admission (spec Part 2 / Part 3): a `Use`-lane withdraw (an upgrader/
/// builder drawing supply) is admitted iff the DESTINATION sink's bid meets the floor
/// ([`econ::admit_use_withdraw`]) — so under a deep refill deficit the upgrade sink is priced
/// out and stops draining the container the refill hauler needs.
pub fn admit_use_withdraw(sink_bid: u32, floor: u32) -> bool {
    econ::admit_use_withdraw(sink_bid, floor)
}

/// Repair admission (spec Part 3 — the S1-replacing gate): a repair is admitted iff its
/// `repair_bid` meets the floor ([`econ::admit_repair`]). Survival overrides (near-dead
/// containers, hostile towers) bypass this entirely — they are vetoes, not bids.
pub fn admit_repair(repair_bid: u32, floor: u32) -> bool {
    econ::admit_repair(repair_bid, floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Position, RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    fn consts() -> MarketConsts {
        MarketConsts::default()
    }

    /// A healthy room floors at the numeraire (storage par is the only materially-unmet
    /// deposit); the readout lists it.
    #[test]
    fn healthy_room_floors_at_par() {
        let deposits = vec![MarketDeposit {
            sink: 0,
            pos: pos(25, 25),
            bid_milli: econ::STORAGE_BID,
            unfulfilled: 500_000,
            is_refill: false,
        }];
        let floor = room_opportunity_floor(&consts(), &deposits);
        assert_eq!(floor, econ::STORAGE_BID);
        assert_eq!(top_unmet_bids(&consts(), &deposits, 3), vec![econ::STORAGE_BID]);
    }

    /// Under a deep refill deficit the floor is the stressed refill bid, and Use-lane upgrade
    /// withdraws + quiet-road repairs are priced OUT (spec Part 3, the S1-replacing behavior).
    #[test]
    fn deep_deficit_prices_out_use_and_repair() {
        let refill = econ::refill_bid(&consts(), Some(12_000), 550, 250); // post-wipe: capped 10×
        let deposits = vec![
            MarketDeposit { sink: 0, pos: pos(25, 25), bid_milli: refill, unfulfilled: 550, is_refill: true },
            MarketDeposit { sink: 1, pos: pos(30, 30), bid_milli: econ::STORAGE_BID, unfulfilled: 500_000, is_refill: false },
        ];
        let floor = room_opportunity_floor(&consts(), &deposits);
        assert_eq!(floor, refill, "the floor is the stressed refill bid");
        // The upgrade sink at V_UPGRADE (2000) is priced OUT.
        assert!(!admit_use_withdraw(econ::upgrade_bid(&consts(), false), floor));
        // A quiet 40% road (prices ~0.37 at the seed horizon) is priced OUT.
        let quiet_road = 370;
        assert!(!admit_repair(quiet_road, floor));
        // A hostile-tower refill (survival) is above everything — it would bypass admission.
        assert!(admit_use_withdraw(econ::SURVIVAL_BID, floor));
    }

    /// The end-to-end room pass: a loaded hauler delivers to the highest-density admitted sink;
    /// the floor + readout are published.
    #[test]
    fn room_pass_assigns_and_publishes_floor() {
        let deposits = vec![
            MarketDeposit { sink: 0, pos: pos(40, 40), bid_milli: econ::STORAGE_BID, unfulfilled: 500_000, is_refill: false },
            MarketDeposit { sink: 1, pos: pos(11, 10), bid_milli: 6000, unfulfilled: 100, is_refill: true },
        ];
        let carriers = vec![MarketCarrier { id: 5, pos: pos(10, 10), free: 0, held: 100, opportunity_milli: 0 }];
        let input = RoomMarketInput { deposits, pickups: vec![], carriers };
        let res = run_room_market(&consts(), &input, |_, _| false);
        assert_eq!(res.assignments.len(), 1);
        assert_eq!(res.assignments[0].carrier, 5);
        // The stressed refill sink (6000) is the floor; the readout leads with it.
        assert_eq!(res.opportunity_floor, 6000);
        assert_eq!(res.top_unmet_bids.first(), Some(&6000));
    }
}
