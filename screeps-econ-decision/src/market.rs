//! The **live market selection kernel** (ADR 0040 §D3, milestone M5a): the per-room per-tick
//! assignment pass, extracted here so ONE implementation is consumed by BOTH the offline sim
//! (`screeps-econ-eval::market::MarketRuntime::market_pass`) and the live bot
//! (`screeps-ibex/src/transfer/transfersystem.rs`'s market adapter). Since the market kernels
//! ([`crate::sink_economics`] bids + floor, [`crate::matching`] greedy) already live here, this
//! module lifts the last piece off the sim — the edge generation + concrete-sink resolution —
//! so the A/B gate is by CONSTRUCTION: equal DTOs in ⇒ equal assignments out (the M5a parity
//! test asserts exactly this against the sim driver).
//!
//! **Genericity.** The pass is over opaque adapter-scoped indices only: carriers, deposits,
//! pickups. The sim supplies `SinkKey`/`SrcKey`-derived indices; the live bot supplies its
//! `TransferTarget` node indices. Neither type reaches this kernel — every input is `u32` +
//! `Position` + amount + bid. The one predicate the kernel needs across the two worlds — "is
//! this pickup the same structure as this deposit (never withdraw from the sink you serve)" —
//! is supplied as a small `same_structure` closure.
//!
//! **The algorithm** mirrors the M4-measured sim `market_pass` verbatim (its module docs are
//! the reference): the engine-fungible spawn lane (`is_refill`) aggregates into ONE demand node
//! (a 50-cap extension can never out-density a 2000-cap container at comparable bids, so the
//! lane — the recovered-state gate's own quantity — would never sustain full); every non-pool
//! sink matches per-structure. Loaded carriers deliver carried cargo; empty carriers get their
//! best pickup per (carrier, target). Edge value density `v = bid·amount/service` (the shipped
//! [`crate::matching::greedy_assign`]); the harvester opportunity gate ([`carrier_gate`])
//! prices ticket surplus against a live harvest alternative. The aggregate refill node resolves
//! to the nearest still-needy lane structure (pass-locally booked); surplus flow stays aboard
//! for the next pass (emergent bulk hauling).

use crate::matching as m;
use crate::sink_economics as econ;
use screeps::Position;
use std::collections::BTreeMap;

/// A haul-capable carrier as the pass sees it (mirrors the sim `CarrierDto`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketCarrier {
    /// Opaque adapter-scoped carrier identity (never interpreted here).
    pub id: u32,
    pub pos: Position,
    /// Free capacity (the budget for an empty carrier's pickup+deliver).
    pub free: u32,
    /// Carried energy (the budget for a loaded carrier's deliver).
    pub held: u32,
    /// The carrier's productive alternative in milli-e/t (a harvester's live-source harvest
    /// rate; 0 for haulers) — the edge gate ([`carrier_gate`]).
    pub opportunity_milli: u32,
}

/// A deposit demand as the pass sees it (mirrors the sim `Deposit` + its per-tick bid).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketDeposit {
    /// Adapter-scoped sink identity (used only to book the resolved flow; the kernel returns it
    /// in the task).
    pub sink: u32,
    pub pos: Position,
    /// The quantized deposit bid, milli (`sink_economics`).
    pub bid_milli: u32,
    /// Unfulfilled amount (requested − booked).
    pub unfulfilled: u32,
    /// Whether this sink is a member of the engine-fungible spawn lane (aggregates into one
    /// refill node). The sim's `SinkKey::is_fungible_pool_member`; the bot's spawn/extension.
    pub is_refill: bool,
}

/// A pickup source as the pass sees it (mirrors the sim `Pickup`, haul-lane only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketPickup {
    /// Adapter-scoped source identity.
    pub src: u32,
    pub pos: Position,
    /// Available amount (requested − booked).
    pub available: u32,
    /// ADR 0044 stage-1 admission — this source's outside option (milli): par for a LOSSLESS
    /// source (storage/terminal — declining an arc truly holds the energy) and ~0 for a SATURATING
    /// buffer (a filling source container / dropped energy — declining means overflow/decay, not a
    /// hold). The adapter classifies (the kernel stays key-agnostic); it becomes the delivery
    /// edge's `source_floor_milli` for the reduced-cost reject test.
    pub source_floor_milli: u32,
}

/// One market task the pass hands a carrier (mirrors the sim `MarketTask`). Adapter-scoped
/// `sink`/`src` indices; the adapter maps them back to real targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketTask {
    /// Empty carrier: pick up `take` at `src`, deliver `give` to `sink`.
    PickupDeliver {
        src: u32,
        src_pos: Position,
        take: u32,
        sink: u32,
        sink_pos: Position,
        give: u32,
    },
    /// Loaded carrier: deliver `amount` of carried energy to `sink`.
    Deliver { sink: u32, sink_pos: Position, amount: u32 },
}

/// One assigned (carrier, task) the pass produced, in scan order (carrier-id order is not
/// implied — the adapter keys by `carrier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketAssignment {
    pub carrier: u32,
    pub task: MarketTask,
}

/// The booked flows the pass reserved (the adapter mirrors these into its pending maps): summed
/// pickup take per `src`, deposit give per `sink`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketBookings {
    pub pickups: BTreeMap<u32, u32>,
    pub deposits: BTreeMap<u32, u32>,
}

/// Diagnostics from one pass (the §D3 CPU-gate instruments — the same the sim `MarketRuntime`
/// accumulates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketPassStats {
    pub edges: u64,
    pub ops: u64,
}

/// The pass output. `edges` + `supply0`/`demand0` are the greedy's exact inputs, surfaced for
/// the SIM-ONLY exact oracle (`screeps-econ-eval::market::oracle_best_fp` — the §D8 #4
/// match-optimality-gap instrument); the live bot ignores them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketPassResult {
    pub assignments: Vec<MarketAssignment>,
    pub bookings: MarketBookings,
    pub stats: MarketPassStats,
    /// The candidate edges the pass generated (density-unsorted, push order = candidate order).
    pub edges: Vec<m::AssignEdge>,
    /// The greedy's initial supply availability (pickup index → available).
    pub supply0: BTreeMap<u32, u32>,
    /// The greedy's initial demand (deposit index / refill aggregate → unmet).
    pub demand0: BTreeMap<u32, u32>,
    /// The realized greedy assignments (edge index + booked flow) aligned with `edges` — the
    /// oracle compares its bound against `assignments_value_fp(&edges, &greedy)`.
    pub greedy: Vec<m::Assignment>,
}

/// The harvester opportunity gate (§D3, verbatim from the sim): a carrier with a live harvest
/// alternative only takes tickets whose SURPLUS value rate — `(bid − par)·amount / service` —
/// strictly beats its harvest rate. Haulers (`opportunity == 0`) take any positive edge.
pub fn carrier_gate(opportunity_milli: u32, bid: u32, amount: u32, service: u32) -> bool {
    if opportunity_milli == 0 {
        return true;
    }
    let surplus = bid.saturating_sub(econ::BID_SCALE) as u64 * amount as u64;
    surplus > opportunity_milli as u64 * service.max(1) as u64
}

/// **The per-tick market assignment pass** — the ONE selection algorithm the bot and the sim
/// share (module docs). `same_structure(src, sink)` reports whether a pickup source and a
/// deposit sink are the SAME structure (never propose withdrawing from a non-refill sink being
/// served); the sim's `same_structure(SrcKey, SinkKey)`, the bot's `TransferTarget` identity.
///
/// Determinism: BTreeMaps/BTreeSets throughout, exact-rational compares in the greedy
/// ([`crate::matching`]), full-key tie-breaks — input (carrier/deposit/pickup vec) order is the
/// candidate order, matching the adapters' deterministic construction.
pub fn market_pass(
    carriers: &[MarketCarrier],
    deposits: &[MarketDeposit],
    pickups: &[MarketPickup],
    // ADR 0044: the per-mille road factor for the stage-1 haul subtraction (`haul_milli(d, q)`);
    // `HAUL_ROAD_Q_PLAINS_PERMILLE` = plains (live default). `0` disables the haul subtraction
    // (`haul_milli(d,0)=0`) — the config-gate the sim's admission-OFF arm uses (with source_floor
    // also 0) to reproduce the pre-reduced-cost market (haul as the `service_ticks` divisor only).
    haul_road_q: u32,
    // ADR 0044 step 2: the pickup→sink DISTANCE oracle (the structural haul leg). The kernel no
    // longer assumes Chebyshev `get_range_to` here — the adapter supplies TRUE routed distance
    // (sim: the multi-room mover; live: the cached `PathfinderService`), so `haul_milli(d)` and the
    // density's sink leg price the real path, not a straight line that underprices cross-room hauls.
    // Only the pickup→sink leg (static structure pair, cacheable) uses this; the dynamic
    // carrier→pickup approach leg stays cheap Chebyshev. `|a,b| a.get_range_to(b)` restores the old
    // behaviour.
    dist: &mut dyn FnMut(Position, Position) -> u32,
    same_structure: impl Fn(u32, u32) -> bool,
) -> MarketPassResult {
    let mut result = MarketPassResult::default();
    if carriers.is_empty() || deposits.is_empty() {
        return result;
    }

    // The refill aggregate (module docs): one demand node per ENGINE-FUNGIBLE POOL (the spawn
    // lane). Every non-pool sink matches per-structure.
    let refill_indices: Vec<usize> = (0..deposits.len()).filter(|&i| deposits[i].is_refill).collect();
    let refill_total: u32 = refill_indices.iter().map(|&i| deposits[i].unfulfilled).sum();
    let refill_bid = refill_indices.first().map(|&i| deposits[i].bid_milli).unwrap_or(0);
    let refill_node = deposits.len() as u32;
    // Nearest still-needy lane structure from a position (ties: lowest deposit index).
    let nearest_refill = |from: Position, local: &BTreeMap<usize, u32>| -> Option<usize> {
        refill_indices
            .iter()
            .copied()
            .filter(|&i| deposits[i].unfulfilled > local.get(&i).copied().unwrap_or(0))
            .min_by_key(|&i| (from.get_range_to(deposits[i].pos), i))
    };
    let no_local: BTreeMap<usize, u32> = BTreeMap::new();

    // ── Edge generation (verbatim from the sim market_pass) ───────────────────────────────────
    let mut edges: Vec<m::AssignEdge> = Vec::new();
    for (ci, c) in carriers.iter().enumerate() {
        let budget = if c.held > 0 { c.held } else { c.free };
        if budget == 0 {
            continue;
        }
        // Candidate demand targets: every non-refill deposit with unmet demand + the refill
        // aggregate (resolved to a concrete lane structure position for the service estimate).
        let mut targets: Vec<(u32, u32, u32, Position)> = Vec::new(); // (node, unmet, bid, pos)
        for (di, d) in deposits.iter().enumerate() {
            if !d.is_refill && d.unfulfilled > 0 {
                targets.push((di as u32, d.unfulfilled, d.bid_milli, d.pos));
            }
        }
        if refill_total > 0 {
            if let Some(i) = nearest_refill(c.pos, &no_local) {
                targets.push((refill_node, refill_total, refill_bid, deposits[i].pos));
            }
        }
        for &(node, unmet, bid, dpos) in &targets {
            if c.held > 0 {
                let amount = c.held.min(unmet);
                let service = c.pos.get_range_to(dpos) + 1;
                if amount == 0 || !carrier_gate(c.opportunity_milli, bid, amount, service) {
                    continue;
                }
                edges.push(m::AssignEdge {
                    carrier: ci as u32,
                    supply: None,
                    demand: node,
                    amount,
                    bid_milli: bid,
                    service_ticks: service,
                    // Loaded cargo is never re-gated (ADR 0044 fix 6-2): declining a delivery of
                    // energy already aboard strands it — admission applies only at pickup-commit.
                    source_floor_milli: 0,
                    haul_cost_milli: 0,
                });
            } else {
                // Best pickup for this (carrier, target): max flow/service (exact rationals),
                // ties to lower service then lower pickup index.
                let mut best: Option<(usize, u32, u32)> = None; // (pi, flow, service)
                for (pi, p) in pickups.iter().enumerate() {
                    if p.available == 0 {
                        continue;
                    }
                    // Never propose withdrawing from a non-refill sink being served.
                    if node != refill_node && same_structure(p.src, deposits[node as usize].sink) {
                        continue;
                    }
                    let flow = c.free.min(p.available).min(unmet);
                    if flow == 0 {
                        continue;
                    }
                    // Carrier→pickup approach = cheap Chebyshev (dynamic); pickup→sink = TRUE routed
                    // distance (static structure pair) via the `dist` oracle.
                    let service = c.pos.get_range_to(p.pos) + dist(p.pos, dpos) + 2;
                    let better = match best {
                        None => true,
                        Some((bpi, bflow, bserv)) => {
                            let lhs = flow as u64 * bserv as u64;
                            let rhs = bflow as u64 * service as u64;
                            lhs > rhs || (lhs == rhs && (service, pi) < (bserv, bpi))
                        }
                    };
                    if better {
                        best = Some((pi, flow, service));
                    }
                }
                if let Some((pi, flow, service)) = best {
                    if !carrier_gate(c.opportunity_milli, bid, flow, service) {
                        continue;
                    }
                    // ADR 0044 stage-1: the reduced-cost reject inputs — the structural source→sink
                    // leg `pickup→deposit` (NOT the carrier-approach leg the divisor also counts)
                    // and this source's outside option. `market_pass`/greedy declines the arc if
                    // `bid − source_floor − haul(d) ≤ 0`, leaving the energy at the source.
                    let haul_d = dist(pickups[pi].pos, dpos);
                    edges.push(m::AssignEdge {
                        carrier: ci as u32,
                        supply: Some(pi as u32),
                        demand: node,
                        amount: flow,
                        bid_milli: bid,
                        service_ticks: service,
                        source_floor_milli: pickups[pi].source_floor_milli,
                        haul_cost_milli: econ::haul_milli(haul_d, haul_road_q),
                    });
                }
            }
        }
    }
    result.stats.edges = edges.len() as u64;
    if edges.is_empty() {
        return result;
    }

    // ── The shipped greedy with booking (the crate's matcher) ─────────────────────────────────
    let mut supply_avail: BTreeMap<u32, u32> =
        pickups.iter().enumerate().map(|(i, p)| (i as u32, p.available)).collect();
    let mut demand_unmet: BTreeMap<u32, u32> = deposits
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.is_refill)
        .map(|(i, d)| (i as u32, d.unfulfilled))
        .collect();
    demand_unmet.insert(refill_node, refill_total);
    let supply0 = supply_avail.clone();
    let demand0 = demand_unmet.clone();
    let (assignments, ops) = m::greedy_assign(&edges, &mut supply_avail, &mut demand_unmet);
    result.stats.ops = ops;

    // Pass-local lane booking: concurrent refill assignments spread across structures.
    let mut local_lane: BTreeMap<usize, u32> = BTreeMap::new();
    for a in &assignments {
        let e = &edges[a.edge];
        let c = &carriers[e.carrier as usize];
        // Resolve the CONCRETE sink: the aggregate refill node delivers to the nearest still-
        // needy lane structure (pass-locally booked); surplus flow stays aboard for the next
        // pass (module docs).
        let (sink, sink_pos, give) = if e.demand == refill_node {
            let anchor = match e.supply {
                Some(pi) => pickups[pi as usize].pos,
                None => c.pos,
            };
            let Some(di) = nearest_refill(anchor, &local_lane) else { continue };
            let d = &deposits[di];
            let already = local_lane.get(&di).copied().unwrap_or(0);
            let give = a.amount.min(d.unfulfilled - already);
            *local_lane.entry(di).or_insert(0) += give;
            (d.sink, d.pos, give)
        } else {
            let d = &deposits[e.demand as usize];
            (d.sink, d.pos, a.amount)
        };
        if give == 0 {
            continue;
        }
        let task = match e.supply {
            Some(pi) => {
                let p = &pickups[pi as usize];
                *result.bookings.pickups.entry(p.src).or_insert(0) += a.amount;
                *result.bookings.deposits.entry(sink).or_insert(0) += give;
                MarketTask::PickupDeliver {
                    src: p.src,
                    src_pos: p.pos,
                    take: a.amount,
                    sink,
                    sink_pos,
                    give,
                }
            }
            None => {
                *result.bookings.deposits.entry(sink).or_insert(0) += give;
                MarketTask::Deliver { sink, sink_pos, amount: give }
            }
        };
        result.assignments.push(MarketAssignment { carrier: c.id, task });
    }

    // Surface the greedy's exact inputs/outputs for the sim-only oracle (module docs).
    result.edges = edges;
    result.supply0 = supply0;
    result.demand0 = demand0;
    result.greedy = assignments;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    /// ADR 0044: the reduced-cost admission DECLINES a long haul that is not worth it, and the
    /// `haul_road_q` config-gate turns it on/off. A mid-value sink (bid 1500) fed from a LOSSLESS
    /// source (`source_floor = par`) 400 tiles away nets `1500 − 1000 − haul`: with the haul
    /// subtraction OFF (`haul_road_q = 0`) it is SERVED (net +500); ON (plains) the haul (~1067)
    /// makes it net-negative → DECLINED (the energy stays banked at the lossless source). This is
    /// the A1→A2 behaviour the sim arms toggle — unambiguous at the kernel, unlike a full-sim run
    /// where a profitable remote is (correctly) served under both arms.
    #[test]
    fn admission_declines_far_par_sink() {
        let sink = pos(25, 25);
        let far = sink.checked_add((400, 0)).expect("400 tiles east is on the map"); // ~8 rooms
        let carriers = [MarketCarrier { id: 0, pos: sink, free: 100, held: 0, opportunity_milli: 0 }];
        // A mid-value NON-refill sink (so it matches per-structure, not the aggregate lane).
        let deposits = [MarketDeposit { sink: 0, pos: sink, bid_milli: 1500, unfulfilled: 100, is_refill: false }];
        // A LOSSLESS far source (storage) — outside option = par.
        let pickups = [MarketPickup { src: 0, pos: far, available: 100, source_floor_milli: econ::STORAGE_BID }];

        let served = market_pass(&carriers, &deposits, &pickups, 0, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false);
        assert_eq!(served.assignments.len(), 1, "haul OFF: net +500 → the long haul is served");

        let declined = market_pass(&carriers, &deposits, &pickups, econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false);
        assert!(declined.assignments.is_empty(), "haul ON: net-negative long haul is DECLINED (energy stays banked)");
    }

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    /// A loaded hauler delivers carried cargo to the highest-density admitted sink; the pass
    /// books the give and returns a `Deliver` task keyed by the carrier id.
    #[test]
    fn loaded_carrier_delivers_to_best_density() {
        let carriers = [MarketCarrier { id: 7, pos: pos(10, 10), free: 0, held: 100, opportunity_milli: 0 }];
        let deposits = [
            // storage dump at par, far.
            MarketDeposit { sink: 0, pos: pos(40, 40), bid_milli: 1000, unfulfilled: 100_000, is_refill: false },
            // a stressed container close, high bid.
            MarketDeposit { sink: 1, pos: pos(11, 10), bid_milli: 5000, unfulfilled: 100, is_refill: false },
        ];
        let res = market_pass(&carriers, &deposits, &[], econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false);
        assert_eq!(res.assignments.len(), 1);
        assert_eq!(res.assignments[0].carrier, 7);
        match res.assignments[0].task {
            MarketTask::Deliver { sink, amount, .. } => {
                assert_eq!(sink, 1, "the dense near sink wins");
                assert_eq!(amount, 100);
            }
            _ => panic!("expected Deliver"),
        }
        assert_eq!(res.bookings.deposits.get(&1), Some(&100));
    }

    /// The spawn lane aggregates: two extensions + a spawn are ONE refill node; an empty hauler
    /// picks up from storage and delivers to the nearest still-needy lane structure.
    #[test]
    fn refill_lane_aggregates_and_resolves_nearest() {
        let carriers = [MarketCarrier { id: 1, pos: pos(20, 20), free: 200, held: 0, opportunity_milli: 0 }];
        let deposits = [
            MarketDeposit { sink: 10, pos: pos(21, 20), bid_milli: 6000, unfulfilled: 50, is_refill: true },
            MarketDeposit { sink: 11, pos: pos(25, 20), bid_milli: 6000, unfulfilled: 50, is_refill: true },
        ];
        let pickups = [MarketPickup { src: 99, pos: pos(20, 21), available: 500, source_floor_milli: 0 }];
        let res = market_pass(&carriers, &deposits, &pickups, econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false);
        assert_eq!(res.assignments.len(), 1);
        match res.assignments[0].task {
            MarketTask::PickupDeliver { src, sink, take, give, .. } => {
                assert_eq!(src, 99);
                assert_eq!(sink, 10, "nearest still-needy lane structure to the pickup");
                assert_eq!(give, 50, "the nearest structure's unmet; surplus stays aboard");
                assert_eq!(take, 100, "min(free 200, avail 500, refill_total 100)");
            }
            _ => panic!("expected PickupDeliver"),
        }
    }

    /// The harvester gate: a par delivery never beats a live source; a stressed refill does.
    #[test]
    fn harvester_gate_blocks_par_but_admits_stress() {
        let harvester = [MarketCarrier { id: 3, pos: pos(10, 10), free: 0, held: 50, opportunity_milli: 2000 }];
        // par storage: surplus 0 — the harvester keeps harvesting (no assignment).
        let par = [MarketDeposit { sink: 0, pos: pos(11, 10), bid_milli: 1000, unfulfilled: 500, is_refill: false }];
        assert!(market_pass(&harvester, &par, &[], econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false).assignments.is_empty());
        // stressed refill at 12×: surplus (11000)·50 ≫ 2000·(range+1) — it delivers.
        let stress = [MarketDeposit { sink: 0, pos: pos(11, 10), bid_milli: 12_000, unfulfilled: 500, is_refill: true }];
        assert_eq!(market_pass(&harvester, &stress, &[], econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false).assignments.len(), 1);
    }

    /// `same_structure` blocks a self-withdraw: a carrier cannot be told to withdraw from the
    /// very container it is supposed to fill.
    #[test]
    fn same_structure_blocks_self_withdraw() {
        let carriers = [MarketCarrier { id: 1, pos: pos(10, 10), free: 100, held: 0, opportunity_milli: 0 }];
        let deposits = [MarketDeposit { sink: 5, pos: pos(11, 10), bid_milli: 3000, unfulfilled: 100, is_refill: false }];
        // The only pickup IS sink 5 (src index 5 maps to sink 5).
        let pickups = [MarketPickup { src: 5, pos: pos(11, 10), available: 100, source_floor_milli: 0 }];
        let res = market_pass(&carriers, &deposits, &pickups, econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |src, sink| src == sink);
        assert!(res.assignments.is_empty(), "no self-withdraw edge is generated");
    }

    /// Empty world / no carriers ⇒ empty result, no panic.
    #[test]
    fn empty_inputs_are_safe() {
        assert!(market_pass(&[], &[], &[], econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false).assignments.is_empty());
        let c = [MarketCarrier { id: 1, pos: pos(1, 1), free: 100, held: 0, opportunity_milli: 0 }];
        assert!(market_pass(&c, &[], &[], econ::HAUL_ROAD_Q_PLAINS_PERMILLE, &mut |a: Position, b: Position| a.get_range_to(b), |_, _| false).assignments.is_empty());
    }
}
