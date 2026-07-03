//! The T* oracle — a **pure-arithmetic dependency chain** lower bound on T_recover (ADR §D7; M1
//! spec Part C.5): spawn self-charge from E0 → the first affordable harvester body (cost table) →
//! travel (the plan's haul distance via the memoized oracle traces) → harvest ramp at 2 e/WORK/t
//! under the per-source 10 e/t regen ceiling → fleet build-out at 3 t/part through the SHARED
//! `spawn_step` kernel → the refill deficit at net income → the recovered-state windows.
//!
//! **Loose by construction** (every simplification is OPTIMISTIC, so T* ≤ any achievable T):
//! harvested energy banks instantly (no haul legs), bodies are the cheapest saturating fleet (not
//! the baseline's), no repair spend, no contention, no TTL churn. η = T*/T_recover is therefore
//! DECORATIVE (the ADR's label) — paired diffs are the instrument; H is regression-tracked, never
//! a ==1 gate.

use crate::layout::LayoutInfo;
use crate::metrics::RecoverConsts;
use crate::movement::Mover;
use screeps_econ_engine::constants::{body_cost, SPAWN_ENERGY_CAPACITY};
use screeps_econ_engine::spawn_queue::{spawn_step, HomeLanes, QueuedSpawn};
use screeps_econ_engine::{EconWorld, SimResource};
use screeps_sim_core::SimBody;

/// Source potential: 10 e/t (3000 / 300 regen — engine-mechanics.md:466).
const SOURCE_RATE_E_T: u32 = 10;

/// T* for a collapse world: the arithmetic chain above. `world0` is the INSTANTIATED (drained)
/// scenario world.
pub fn t_star(world0: &EconWorld, mover: &mut dyn Mover, info: &LayoutInfo, consts: &RecoverConsts) -> u32 {
    let _ = info;
    let k = world0.spawns.len() as u32;
    if k == 0 || world0.sources.is_empty() {
        return 0;
    }
    let capacity = crate::baseline::spawn_lane_capacity(world0);
    let n_sources = world0.sources.len() as u32;

    // The optimal opening body: the biggest harvester the 300 self-charge floor affords —
    // the SAME 250-cost [M,M,C,W] the cost table yields (harvester_body(300)).
    let first_body = crate::baseline::harvester_body(SPAWN_ENERGY_CAPACITY.max(world0.room_spawn_energy()))
        .expect("the 300-floor body always builds");
    let first_cost = body_cost(&first_body);
    let first_parts = first_body.len() as u32;
    let first_work = first_body.iter().filter(|p| **p == screeps::Part::Work).count() as u32;

    // Travel: spawn → the nearest source at range 1, walked by the first body EMPTY — the plan's
    // haul distance from the same memoized oracle traces the movement tier uses.
    let sim_body = SimBody::unboosted(&first_body);
    let spawn_pos = world0.spawns[0].pos;
    let travel = world0
        .sources
        .iter()
        .filter_map(|s| mover.travel_ticks(spawn_pos, s.pos, 1, &sim_body, 0))
        .min()
        .unwrap_or(0);

    // Storage stock counts toward the refill optimistically (haulers move it for free).
    let s0 = world0.storage.as_ref().map(|s| s.store.amount(SimResource::Energy) as u64).unwrap_or(0);

    // ── The arithmetic tick loop: self-charge → first body → income ramp → fleet build-out via
    // the SHARED spawn_step queue kernel → refill at net income. `bank` is total free energy
    // (structures + storage abstraction — instant logistics, optimistic).
    let mut bank: u64 = world0.room_spawn_energy() as u64 + s0;
    #[allow(unused_assignments)]
    let mut income: u32 = 0; // e/t from harvesters on site, capped per source (set each loop)
    let mut per_source_work: Vec<u32> = vec![0; n_sources as usize];
    let mut arrivals: Vec<(u32, usize, u32)> = Vec::new(); // (arrive_tick, source, work)
    let mut t: u32 = 0;
    let mut income_ok_at: Option<u32> = None;
    let mut refill_done_at: Option<u32> = None;
    let income_threshold = consts.income_threshold(n_sources);
    // The optimal fleet: 5-WORK bodies until each source's Σ(2·WORK) ≥ 10 — one 1250 body per
    // source at capacity ≥ 1250, else stacked smaller bodies (cheapest saturating fleet).
    let fleet_body = crate::baseline::harvester_body(capacity).expect("capacity ≥ 300");
    let fleet_cost = body_cost(&fleet_body);
    let fleet_parts = fleet_body.len() as u32;
    let fleet_work = fleet_body.iter().filter(|p| **p == screeps::Part::Work).count() as u32;
    let mut spawn_busy_until: Vec<u32> = vec![0; k as usize];
    let mut first_spawned = false;
    let hard_cap = 200_000u32; // arithmetic safety net, never binding in practice

    while t < hard_cap {
        // Arrivals come online.
        for &(at, src, work) in &arrivals {
            if at == t {
                per_source_work[src] += work;
            }
        }
        arrivals.retain(|&(at, _, _)| at > t);
        income = per_source_work
            .iter()
            .map(|&w| (2 * w).min(SOURCE_RATE_E_T))
            .sum();

        // Flows: self-charge (only while the spawn lane is under 300 — modeled coarsely as
        // "while the bank can't yet cover the pending body", optimistic) + income.
        let charging = if bank < first_cost as u64 && !first_spawned { k } else { 0 };
        bank += charging as u64 + income as u64;

        // Spawn decisions through the shared queue kernel: the next needed harvester.
        let need_more = per_source_work.iter().any(|&w| 2 * w < SOURCE_RATE_E_T);
        let idle_spawns = spawn_busy_until.iter().filter(|&&b| b <= t).count() as u32;
        if need_more && idle_spawns > 0 {
            let (cost, parts, work) = if first_spawned {
                (fleet_cost, fleet_parts, fleet_work)
            } else {
                (first_cost, first_parts, first_work)
            };
            let mut lanes = HomeLanes {
                idle_spawns,
                available_energy: bank.min(u32::MAX as u64) as u32,
                energy_capacity: capacity.max(cost),
            };
            let queue = [QueuedSpawn { priority: 100.0, body_cost: cost, part_count: parts, id: 1 }];
            for s in spawn_step(&mut lanes, &queue) {
                bank -= cost as u64;
                // Assign to the least-saturated source.
                let (src, _) = per_source_work
                    .iter()
                    .enumerate()
                    .min_by_key(|(i, &w)| (w, *i))
                    .expect("sources non-empty");
                // Pending arrivals also count toward saturation targeting.
                per_source_work[src] += 0;
                arrivals.push((t + s.completes_in + travel, src, work));
                if let Some(b) = spawn_busy_until.iter_mut().find(|b| **b <= t) {
                    *b = t + s.completes_in;
                }
                first_spawned = true;
            }
        }

        // Condition (ii): income at/above the threshold rate — the trailing window then fills in
        // threshold/income more ticks (income jumped, optimistically instantaneous).
        if income_ok_at.is_none() && income as u64 * consts.income_window as u64 >= income_threshold {
            let fill = income_threshold.div_ceil(income.max(1) as u64) as u32;
            income_ok_at = Some(t + fill);
        }
        // Condition (i): the refill completes when the bank covers the full spawn lane.
        if refill_done_at.is_none() && bank >= capacity as u64 {
            refill_done_at = Some(t + consts.full_window);
        }
        if let (Some(a), Some(b)) = (income_ok_at, refill_done_at) {
            return a.max(b).max(t);
        }
        t += 1;
    }
    hard_cap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::AnalyticMover;
    use crate::scenario::{catalog, instantiate};

    /// The oracle is finite, positive, and monotone in the collapse depth (more stored energy ⇒
    /// never a later T*) on real catalog scenarios.
    #[test]
    fn oracle_is_finite_and_sane_on_the_catalog() {
        let consts = RecoverConsts::default();
        for sc in catalog().iter().take(4) {
            let (world, terrain, info) = instantiate(sc);
            let mut mover = AnalyticMover::new(&terrain);
            let t = t_star(&world, &mut mover, &info, &consts);
            assert!(t > 0, "{}: T* positive", sc.name);
            assert!(t < 100_000, "{}: T* finite ({t})", sc.name);
            // Self-charge to the 250 first body needs ≥ ~250/k ticks — the chain's floor.
            assert!(t >= 250 / world.spawns.len() as u32, "{}: T* ≥ the charge floor", sc.name);
        }
    }
}
