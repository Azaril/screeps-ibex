//! Per-tick flow accounting + the exact conservation audit (ADR 0040 §D7 hard gate). All exact
//! integers, per resource:
//!
//! - **Sources (mint):** `harvested` (energy enters the economy at harvest time — source pools are
//!   NOT stock) and `spawn_self_charge`.
//! - **Sinks (burn):** `spawn_bodies` (the atomic debit at spawn-intent time; the body is never
//!   stock), the M1 repair sinks by structure class (`repair_roads` / `repair_containers` /
//!   `repair_other`), and `dropped_decay` (per resource — the ADR §D7 "decay" sink,
//!   [`TickLedger::decay_lost`]).
//! - **Stock:** every store + dropped piles ([`crate::EconWorld::stocks`]).
//!
//! **What decay does and does not ledger (M1):** road/container HIT decay destroys structure hits,
//! not energy — hits are never ledgered as energy. A container dying at 0 hits DROPS its store to
//! the ground (stock relocation, no ledger entry), after which ordinary dropped-pile decay books
//! it under `dropped_decay` — so the only energy decay ever destroys is dropped-pile energy,
//! exactly as in M0.
//!
//! Invariant, checked EVERY tick: `prev_stocks + minted − burned == new_stocks`, exactly. The check
//! is a `debug_assert!` AND a released-mode flag on the tick report — the eval harness must SEE an
//! imbalance and gate on it, never learn of it via a panic (EP-6.12).

use crate::state::SimResource;
use std::collections::{BTreeMap, BTreeSet};

/// One tick's economy flows (exact integers; `u64` so multi-thousand-tick aggregation never
/// saturates).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickLedger {
    /// Energy minted by harvest this tick (including any overflow that went to ground).
    pub harvested: u64,
    /// Energy minted by spawn self-charge this tick (engine-mechanics.md:279).
    pub spawn_self_charge: u64,
    /// Energy burned into spawned bodies this tick (the atomic intent-time debit).
    pub spawn_bodies: u64,
    /// Energy burned repairing ROADS this tick (M1; engine `creeps/repair.js` pricing).
    pub repair_roads: u64,
    /// Energy burned repairing CONTAINERS this tick (M1).
    pub repair_containers: u64,
    /// Energy burned repairing any other structure class (declared for the ADR §D7 sink set;
    /// stays 0 in M1 — roads and containers are the only repairable structures until M2).
    pub repair_other: u64,
    /// Per-resource decay of dropped piles this tick (engine-mechanics.md:431).
    pub dropped_decay: BTreeMap<SimResource, u64>,
}

impl TickLedger {
    /// Total repair energy burned this tick (all classes).
    pub fn repair_total(&self) -> u64 {
        self.repair_roads + self.repair_containers + self.repair_other
    }

    /// The ADR §D7 "decay" sink under its ADR name: energy (or mineral) destroyed by decay this
    /// tick. In M1 this is exactly dropped-pile decay — structure HIT decay destroys hits, not
    /// energy (module docs), and container-death store drops are stock relocation.
    pub fn decay_lost(&self, r: SimResource) -> u64 {
        self.dropped_decay.get(&r).copied().unwrap_or(0)
    }

    fn minted(&self, r: SimResource) -> u64 {
        match r {
            SimResource::Energy => self.harvested + self.spawn_self_charge,
            _ => 0,
        }
    }

    fn burned(&self, r: SimResource) -> u64 {
        let decay = self.dropped_decay.get(&r).copied().unwrap_or(0);
        match r {
            SimResource::Energy => self.spawn_bodies + self.repair_total() + decay,
            _ => decay,
        }
    }
}

/// One resource's failed balance: `prev + minted − burned` should equal `now` and does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConservationViolation {
    pub resource: SimResource,
    pub prev: u64,
    pub minted: u64,
    pub burned: u64,
    /// What the stocks actually total now.
    pub now: u64,
    /// `prev + minted − burned` — what they should total.
    pub expected: i128,
}

/// The exact per-resource balance check. Returns every violated resource (empty = conserved).
pub fn audit_conservation(
    prev: &BTreeMap<SimResource, u64>,
    ledger: &TickLedger,
    now: &BTreeMap<SimResource, u64>,
) -> Vec<ConservationViolation> {
    let mut resources: BTreeSet<SimResource> = BTreeSet::new();
    resources.extend(prev.keys().copied());
    resources.extend(now.keys().copied());
    resources.extend(ledger.dropped_decay.keys().copied());
    resources.insert(SimResource::Energy);

    let mut violations = Vec::new();
    for r in resources {
        let prev_v = prev.get(&r).copied().unwrap_or(0);
        let now_v = now.get(&r).copied().unwrap_or(0);
        let minted = ledger.minted(r);
        let burned = ledger.burned(r);
        let expected = prev_v as i128 + minted as i128 - burned as i128;
        if expected != now_v as i128 {
            violations.push(ConservationViolation {
                resource: r,
                prev: prev_v,
                minted,
                burned,
                now: now_v,
                expected,
            });
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stocks(energy: u64) -> BTreeMap<SimResource, u64> {
        let mut m = BTreeMap::new();
        if energy > 0 {
            m.insert(SimResource::Energy, energy);
        }
        m
    }

    #[test]
    fn balanced_tick_passes() {
        let ledger = TickLedger { harvested: 10, spawn_self_charge: 1, spawn_bodies: 5, ..Default::default() };
        assert!(audit_conservation(&stocks(100), &ledger, &stocks(106)).is_empty());
    }

    #[test]
    fn imbalance_is_reported_not_panicked() {
        let ledger = TickLedger::default();
        let v = audit_conservation(&stocks(100), &ledger, &stocks(99));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].resource, SimResource::Energy);
        assert_eq!(v[0].expected, 100);
        assert_eq!(v[0].now, 99);
    }

    /// M1: repair energy burns by structure class; `repair_total` covers all three; hit decay is
    /// never an energy flow (`decay_lost` names only dropped-pile decay).
    #[test]
    fn repair_sinks_burn_energy_by_class() {
        let ledger = TickLedger {
            harvested: 10,
            repair_roads: 3,
            repair_containers: 2,
            repair_other: 0,
            ..Default::default()
        };
        assert_eq!(ledger.repair_total(), 5);
        assert!(audit_conservation(&stocks(100), &ledger, &stocks(105)).is_empty());
        // A repair burn NOT booked would violate: prev 100 + 10 minted − 5 burned != 110.
        assert_eq!(audit_conservation(&stocks(100), &ledger, &stocks(110)).len(), 1);
        assert_eq!(ledger.decay_lost(SimResource::Energy), 0, "hit decay is not an energy flow");
    }

    #[test]
    fn per_resource_decay_is_audited_for_minerals_too() {
        let mut prev = stocks(0);
        prev.insert(SimResource::Ghodium, 50);
        let mut now = stocks(0);
        now.insert(SimResource::Ghodium, 49);
        let mut ledger = TickLedger::default();
        ledger.dropped_decay.insert(SimResource::Ghodium, 1);
        assert!(audit_conservation(&prev, &ledger, &now).is_empty());
        // ...and a mineral leak with no decay booked is a violation.
        ledger.dropped_decay.clear();
        assert_eq!(audit_conservation(&prev, &ledger, &now).len(), 1);
    }
}
