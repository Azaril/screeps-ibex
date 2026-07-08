//! K2-market — the per-room bid-optimizing assignment kernel (ADR 0040 §D3, milestone M4):
//! **deterministic greedy on globally sorted edge value-density with booking** — the shipped v1
//! (`O(E log E)`, a ½-approximation for weighted matching, byte-deterministic: exact-rational
//! value compares + full-key position tie-breaks; no float reaches an ordering).
//!
//! Edge value density (§D3): `v(carrier, ticket) = bid · amount / service_ticks` — milli
//! energy-equivalent per tick of carrier time. The EXACT assignment oracle (min-cost-flow) is
//! SIM-ONLY and lives in `screeps-econ-eval::market` (never a bot dependency path — spec
//! constraint); this kernel is the one the live bot adopts at M5a.
//!
//! Booking semantics: supply/demand availability maps are consumed as flows assign (the §D3
//! "booking via the existing tables" — adapter-side reservation, exactly the live pending-map
//! shape). One ticket per carrier per pass; a later (lower-density) edge whose nodes were
//! drained realizes a smaller flow or none.

use std::collections::BTreeMap;

/// One candidate edge: a carrier serving a (supply?, demand) ticket. `supply == None` = the
/// carrier's own carried cargo (a pure delivery edge). Ids are adapter-scoped indices; the
/// sort tie-break is `(carrier, supply, demand)` ascending, so adapters must mint them
/// deterministically.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AssignEdge {
    pub carrier: u32,
    pub supply: Option<u32>,
    pub demand: u32,
    /// Proposed flow (already clamped to carrier capacity by the adapter); the realized flow
    /// additionally clamps to the REMAINING supply/demand at assignment time.
    pub amount: u32,
    /// The destination sink's quantized bid (milli, `sink_economics`).
    pub bid_milli: u32,
    /// Estimated ticks to serve the ticket (existing distance services — §D3; floored ≥ 1).
    pub service_ticks: u32,
    /// ADR 0044 stage-1 admission — the source's outside option (milli): par for a lossless source
    /// (storage/terminal — declining truly holds the energy), ~0 for a saturating buffer (a filling
    /// source container / dropped energy — declining means overflow/decay). `0` on a pure delivery
    /// edge (`supply == None`): loaded cargo is never re-gated (declining strands it).
    pub source_floor_milli: u32,
    /// ADR 0044 stage-1 admission — the per-energy HAUL cost over the structural source→sink leg
    /// (milli, `sink_economics::haul_milli`). `0` on a pure delivery edge (no admission subtraction).
    pub haul_cost_milli: u32,
}

impl AssignEdge {
    /// The §D3 value density as an exact rational: `num/den = bid·amount / service_ticks`
    /// (stage-2 allocation — distance in the DIVISOR only; never folded with the stage-1
    /// subtraction, which would double-charge distance).
    fn value(&self) -> (u64, u64) {
        (
            self.bid_milli as u64 * self.amount as u64,
            self.service_ticks.max(1) as u64,
        )
    }

    /// ADR 0044 stage-1 admission: keep the arc iff its reduced cost `bid − source_floor −
    /// haul(d)` is strictly positive — i.e. delivering is worth more than the source's outside
    /// option plus the haul. A beyond-break-even arc is DECLINED (the energy stays at the source),
    /// which the value-density divisor alone can never do. This SAME predicate gates both the live
    /// greedy and the sim oracle (critique fix 6-4: otherwise the optimality gap is an artifact).
    pub fn admitted(&self) -> bool {
        crate::sink_economics::delivered_milli(self.bid_milli, self.source_floor_milli, self.haul_cost_milli) > 0
    }
}

/// One realized assignment: the edge (by caller index) + the flow actually booked.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub edge: usize,
    pub amount: u32,
}

/// Exact-rational density compare: `a > b ⟺ a.num·b.den > b.num·a.den` (u128 — no overflow for
/// any reachable bid·amount ≤ 2^63).
fn density_gt(a: (u64, u64), b: (u64, u64)) -> bool {
    (a.0 as u128) * (b.1 as u128) > (b.0 as u128) * (a.1 as u128)
}

/// The v1 greedy (§D3): sort every edge by value density descending (exact rationals; ties to
/// the lowest `(carrier, supply, demand)`), then scan once — each carrier takes its first edge
/// whose realized flow (clamped to remaining supply/demand) is positive, consuming the booking
/// maps. Returns the assignments (in scan order) and the ops proxy: `E + E·⌈log₂E⌉ + scanned`
/// — the §D3 CPU-gate instrument (sort-dominated, the M5a budget is set from it).
pub fn greedy_assign(
    edges: &[AssignEdge],
    supply_avail: &mut BTreeMap<u32, u32>,
    demand_unmet: &mut BTreeMap<u32, u32>,
) -> (Vec<Assignment>, u64) {
    let e = edges.len() as u64;
    let mut ops: u64 = e; // edge intake
    if edges.is_empty() {
        return (Vec::new(), ops);
    }

    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&a, &b| {
        let (ea, eb) = (&edges[a], &edges[b]);
        let (va, vb) = (ea.value(), eb.value());
        if density_gt(va, vb) {
            std::cmp::Ordering::Less
        } else if density_gt(vb, va) {
            std::cmp::Ordering::Greater
        } else {
            (ea.carrier, ea.supply, ea.demand, a).cmp(&(eb.carrier, eb.supply, eb.demand, b))
        }
    });
    ops += e * (u64::BITS - (e.max(2) - 1).leading_zeros()) as u64; // E·⌈log₂E⌉ sort proxy

    let mut assigned_carriers = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for idx in order {
        ops += 1; // scan
        let edge = &edges[idx];
        if assigned_carriers.contains(&edge.carrier) {
            continue;
        }
        // ADR 0044 stage-1: decline a below-break-even arc outright (the reduced cost ≤ 0). The
        // energy stays at the source rather than being hauled at a net loss — the accept/reject the
        // stage-2 density divisor structurally cannot express.
        if !edge.admitted() {
            continue;
        }
        let mut flow = edge.amount;
        if let Some(s) = edge.supply {
            flow = flow.min(supply_avail.get(&s).copied().unwrap_or(0));
        }
        flow = flow.min(demand_unmet.get(&edge.demand).copied().unwrap_or(0));
        if flow == 0 {
            continue;
        }
        if let Some(s) = edge.supply {
            *supply_avail.get_mut(&s).expect("clamped above") -= flow;
        }
        *demand_unmet.get_mut(&edge.demand).expect("clamped above") -= flow;
        assigned_carriers.insert(edge.carrier);
        out.push(Assignment { edge: idx, amount: flow });
    }
    (out, ops)
}

/// The fixed-point value of a realized flow — the gap instrument's shared unit (the sim's
/// exact oracle totals in the SAME fixed point, so `match_optimality_gap` compares like with
/// like). FP = 1024 per milli-bid·energy/tick.
pub const VALUE_FP: u64 = 1024;

pub fn flow_value_fp(bid_milli: u32, amount: u32, service_ticks: u32) -> u64 {
    bid_milli as u64 * amount as u64 * VALUE_FP / service_ticks.max(1) as u64
}

/// Total fixed-point value of a greedy result over its edge list.
pub fn assignments_value_fp(edges: &[AssignEdge], assignments: &[Assignment]) -> u64 {
    assignments
        .iter()
        .map(|a| flow_value_fp(edges[a.edge].bid_milli, a.amount, edges[a.edge].service_ticks))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps(supply: &[(u32, u32)], demand: &[(u32, u32)]) -> (BTreeMap<u32, u32>, BTreeMap<u32, u32>) {
        (supply.iter().copied().collect(), demand.iter().copied().collect())
    }

    /// Value density decides, not listing order: the high-bid short-service edge wins the
    /// contested demand; the losing carrier falls to its next-best live edge.
    #[test]
    fn density_orders_and_bookings_consume() {
        let edges = [
            // carrier 0: par storage dump, 100 over 10 ticks → v = 10k/t
            AssignEdge { carrier: 0, supply: Some(0), demand: 9, amount: 100, bid_milli: 1000, service_ticks: 10, source_floor_milli: 0, haul_cost_milli: 0 },
            // carrier 0: refill 300 @12000 over 20 ticks → v = 180k/t (wins for carrier 0)
            AssignEdge { carrier: 0, supply: Some(0), demand: 1, amount: 300, bid_milli: 12_000, service_ticks: 20, source_floor_milli: 0, haul_cost_milli: 0 },
            // carrier 1: the SAME refill demand, slightly farther → v = 150k/t
            AssignEdge { carrier: 1, supply: Some(0), demand: 1, amount: 300, bid_milli: 12_000, service_ticks: 24, source_floor_milli: 0, haul_cost_milli: 0 },
            // carrier 1 fallback: storage dump
            AssignEdge { carrier: 1, supply: Some(0), demand: 9, amount: 100, bid_milli: 1000, service_ticks: 8, source_floor_milli: 0, haul_cost_milli: 0 },
        ];
        let (mut s, mut d) = maps(&[(0, 350)], &[(1, 300), (9, 100_000)]);
        let (got, _ops) = greedy_assign(&edges, &mut s, &mut d);
        assert_eq!(got.len(), 2, "both carriers assigned");
        assert_eq!(got[0], Assignment { edge: 1, amount: 300 }, "carrier 0 takes the refill at full flow");
        // Carrier 1's refill edge realizes 0 (demand drained) — it falls to the dump, clamped
        // by the remaining supply (350 − 300 = 50).
        assert_eq!(got[1], Assignment { edge: 3, amount: 50 });
        assert_eq!(s[&0], 0, "supply fully booked");
        assert_eq!(d[&1], 0);
    }

    /// One ticket per carrier per pass; a carrier with no live edge gets nothing.
    #[test]
    fn one_ticket_per_carrier() {
        let edges = [
            AssignEdge { carrier: 0, supply: None, demand: 0, amount: 50, bid_milli: 2000, service_ticks: 5, source_floor_milli: 0, haul_cost_milli: 0 },
            AssignEdge { carrier: 0, supply: None, demand: 1, amount: 50, bid_milli: 2000, service_ticks: 5, source_floor_milli: 0, haul_cost_milli: 0 },
        ];
        let (mut s, mut d) = maps(&[], &[(0, 50), (1, 50)]);
        let (got, _) = greedy_assign(&edges, &mut s, &mut d);
        assert_eq!(got.len(), 1, "one assignment per carrier");
        assert_eq!(got[0].edge, 0, "exact tie breaks to the lowest (carrier, supply, demand)");
    }

    /// Byte-determinism: permuting the edge LIST yields the same realized assignment set (the
    /// full-key tie-break is input-order-free for distinct keys).
    #[test]
    fn assignment_is_input_order_free() {
        let edges = vec![
            AssignEdge { carrier: 0, supply: Some(0), demand: 0, amount: 100, bid_milli: 1500, service_ticks: 7, source_floor_milli: 0, haul_cost_milli: 0 },
            AssignEdge { carrier: 1, supply: Some(0), demand: 0, amount: 100, bid_milli: 1500, service_ticks: 7, source_floor_milli: 0, haul_cost_milli: 0 },
            AssignEdge { carrier: 1, supply: Some(1), demand: 1, amount: 60, bid_milli: 9000, service_ticks: 30, source_floor_milli: 0, haul_cost_milli: 0 },
            AssignEdge { carrier: 0, supply: Some(1), demand: 1, amount: 60, bid_milli: 9000, service_ticks: 31, source_floor_milli: 0, haul_cost_milli: 0 },
        ];
        let run = |es: &[AssignEdge]| {
            let (mut s, mut d) = maps(&[(0, 100), (1, 60)], &[(0, 150), (1, 60)]);
            let (got, _) = greedy_assign(es, &mut s, &mut d);
            let mut flows: Vec<(u32, Option<u32>, u32, u32)> =
                got.iter().map(|a| (es[a.edge].carrier, es[a.edge].supply, es[a.edge].demand, a.amount)).collect();
            flows.sort();
            flows
        };
        let mut rev = edges.clone();
        rev.reverse();
        assert_eq!(run(&edges), run(&rev), "edge insertion order is non-semantic");
    }

    /// The ops proxy is monotone in E and the fixed-point totals are exact for whole flows.
    #[test]
    fn ops_proxy_and_value_fp() {
        let mk = |n: u32| -> Vec<AssignEdge> {
            (0..n)
                .map(|i| AssignEdge { carrier: i, supply: None, demand: i, amount: 10, bid_milli: 1000, service_ticks: 1, source_floor_milli: 0, haul_cost_milli: 0 })
                .collect()
        };
        let (mut s1, mut d1) = maps(&[], &(0..4).map(|i| (i, 10)).collect::<Vec<_>>());
        let (a4, ops4) = greedy_assign(&mk(4), &mut s1, &mut d1);
        let (mut s2, mut d2) = maps(&[], &(0..16).map(|i| (i, 10)).collect::<Vec<_>>());
        let (a16, ops16) = greedy_assign(&mk(16), &mut s2, &mut d2);
        assert!(ops16 > ops4 * 3, "ops grows superlinearly-ish with E ({ops4} → {ops16})");
        assert_eq!(assignments_value_fp(&mk(4), &a4), 4 * 1000 * 10 * VALUE_FP);
        assert_eq!(assignments_value_fp(&mk(16), &a16), 16 * 1000 * 10 * VALUE_FP);
        assert_eq!(flow_value_fp(1000, 10, 0), 1000 * 10 * VALUE_FP, "service floored at 1");
    }

    /// ADR 0044 stage-1: a below-break-even arc is DECLINED — its carrier stays unassigned even
    /// though a positive-density fallback does not exist. Here a par storage→storage dump from a
    /// lossless source (floor par) over any haul nets ≤ 0 and is refused; a high-ROI refill from
    /// the same source clears break-even and is served. This is the reject the density divisor
    /// alone cannot express (a divisor shrinks a bid but never zeroes an arc out).
    #[test]
    fn admission_declines_below_break_even() {
        let par = crate::sink_economics::STORAGE_BID;
        let haul = crate::sink_economics::haul_milli(90, 1000); // 240
        let edges = [
            // carrier 0: par→par storage rebalance from lossless storage, real haul → delivered
            // = 1000 − 1000 − 240 < 0 → DECLINED (not merely low-density).
            AssignEdge { carrier: 0, supply: Some(0), demand: 9, amount: 100, bid_milli: par, service_ticks: 90, source_floor_milli: par, haul_cost_milli: haul },
            // carrier 1: high-ROI refill from the same lossless source → 8000 − 1000 − 240 > 0 → served.
            AssignEdge { carrier: 1, supply: Some(0), demand: 1, amount: 300, bid_milli: 8000, service_ticks: 92, source_floor_milli: par, haul_cost_milli: haul },
        ];
        let (mut s, mut d) = maps(&[(0, 1000)], &[(1, 300), (9, 100_000)]);
        let (got, _) = greedy_assign(&edges, &mut s, &mut d);
        assert_eq!(got.len(), 1, "only the admitted refill assigns; the par rebalance is declined");
        assert_eq!(got[0].edge, 1);
        assert_eq!(d[&9], 100_000, "the storage rebalance demand is untouched — energy stayed put");
    }
}
