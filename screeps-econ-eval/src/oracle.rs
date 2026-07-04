//! The T* oracle — a **pure-arithmetic dependency chain** lower bound on T_recover (ADR §D7; M1
//! spec Part C.5): spawn self-charge from E0 → the first affordable harvester body (cost table) →
//! travel (the plan's haul distance via the memoized oracle traces) → harvest ramp at 2 e/WORK/t
//! under the per-source 10 e/t regen ceiling → fleet build-out at 3 t/part through the SHARED
//! `spawn_step` kernel → the refill deficit at net income → the **LANE-ONLY recovered window**
//! (the #7 decision, `metrics::RecoverConsts` docs — the income condition is a demoted
//! diagnostic and no longer part of the official T_recover, so it is no longer part of T*).
//!
//! **Loose by intent, policed empirically (review B6):** the headline simplifications are
//! optimistic (instant banking, no haul legs, no repair spend, no contention, no TTL churn), but
//! two are NOT provably so — post-first-body spawn self-charge is omitted from income (a
//! pessimistic slack of ≤ 1 e/t per spawn while the lane sits under 300), and the shuttle fleet
//! is a heuristic, not a proven-cheapest schedule. The bound is therefore not a theorem; the
//! LIVE `eta_raw > 1 + ε` oracle-sanity gate (`metrics::etas`, review A1) is what actually
//! polices T* ≤ T on every run. η = T*/T_recover stays DECORATIVE — paired diffs are the
//! instrument; H is regression-tracked, never a ==1 gate.

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
    let mut refill_done_at: Option<u32> = None;
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

        // THE recovered state (#7, lane-only): the refill completes when the bank covers the
        // full spawn lane, plus the sustained-full window. (The former income condition is a
        // demoted diagnostic — module docs — and no longer delays T*.)
        if refill_done_at.is_none() && bank >= capacity as u64 {
            refill_done_at = Some(t + consts.full_window);
        }
        if let Some(b) = refill_done_at {
            return b.max(t);
        }
        t += 1;
    }
    hard_cap
}

/// **T*_RCL(N) — the Family-G conservation-bound oracle (M2).** A pure-arithmetic lower bound on
/// the greenfield time-to-RCL(N): total upgrade progress needed = Σ CONTROLLER_LEVELS[1..N−1]
/// (1 energy per progress), paid from a piecewise income schedule (spawn self-charge → harvest
/// ramp as each oracle harvester arrives → source-saturated 10 e/t per source), with fleet
/// bodies scheduled against the 3 t/part spawn throughput and their costs debited.
///
/// **Loose by construction — every simplification is OPTIMISTIC, so T* ≤ any achievable T:**
/// harvested energy banks instantly (no haul legs); harvester bodies are the cheapest saturating
/// 300-budget shuttles ([M,M,C,W], 5/source) walking the real fatigue-optimal trace once;
/// upgraders are bare [W] parts (100e, 3 ticks each, magically fed at range) up to the room's
/// source potential; conversion is energy-bounded (progress/t ≤ min(Σ upgrader WORK, bank));
/// no TTL churn, no travel for upgraders, no downgrade-clock level-up gate (49 ticks ≪ T), and
/// **no build costs** — *deviation from the M2 spec sketch ("+ build costs"), by necessity:
/// construction is OPTIONAL for reaching RCL N (an optimal policy skips it), so counting the
/// baseline's build spend would make the "bound" exceed an achievable T and break the η ≤ 1+ε
/// gate on small N (e.g. RCL 2 = 200 progress vs 15,000e of extension sites). Build energy is
/// exactly the slack the paired diffs measure instead.*
pub fn t_star_rcl(world0: &EconWorld, mover: &mut dyn Mover, info: &LayoutInfo, target: u8) -> u32 {
    let _ = info;
    let k = world0.spawns.len() as u32;
    let n_sources = world0.sources.len() as u32;
    if k == 0 || n_sources == 0 || target < 2 {
        return 0;
    }
    let needed_progress: u64 = (1..target)
        .map(|l| screeps_econ_engine::constants::controller_levels(l).unwrap_or(0) as u64)
        .sum();

    // The saturating shuttle: [M,M,C,W] (250e, 4 parts, 1 WORK → 2 e/t on site; 5 saturate one
    // source's 10 e/t). Travel = the real fatigue-optimal empty walk, spawn → nearest source.
    let shuttle = crate::baseline::harvester_body(SPAWN_ENERGY_CAPACITY).expect("the 300-floor body builds");
    let shuttle_cost = body_cost(&shuttle) as u64;
    let shuttle_parts = shuttle.len() as u32;
    let sim_body = SimBody::unboosted(&shuttle);
    let spawn_pos = world0.spawns[0].pos;
    let travel = world0
        .sources
        .iter()
        .filter_map(|s| mover.travel_ticks(spawn_pos, s.pos, 1, &sim_body, 0))
        .min()
        .unwrap_or(0);

    // Bare-WORK upgraders: 100e, 1 part, 3 ticks; cap total WORK at the source potential
    // (energy-bounded conversion makes more WORK pointless).
    let upgrader_cost = 100u64;
    let work_cap = (10 * n_sources) as u64;

    let mut bank: u64 = world0.room_spawn_energy() as u64;
    let mut per_source_work: Vec<u32> = vec![0; n_sources as usize];
    let mut pending_work: Vec<u32> = vec![0; n_sources as usize]; // spawned, not yet on site
    let mut arrivals: Vec<(u32, usize, u32)> = Vec::new(); // harvesters: (tick, source, work)
    let mut upgrader_work: u64 = 0;
    let mut upgraders_arriving: Vec<u32> = Vec::new(); // bare-W arrival ticks
    let mut progress: u64 = 0;
    let mut spawn_busy_until: Vec<u32> = vec![0; k as usize];
    let mut first_spawned = false;
    let hard_cap = 400_000u32;

    let mut t = 0u32;
    while t < hard_cap {
        for &(at, src, work) in &arrivals {
            if at == t {
                per_source_work[src] += work;
                pending_work[src] -= work;
            }
        }
        arrivals.retain(|&(at, _, _)| at > t);
        upgraders_arriving.retain(|&at| {
            if at == t {
                upgrader_work += 1;
                false
            } else {
                true
            }
        });

        let income: u32 = per_source_work.iter().map(|&w| (2 * w).min(10)).sum();
        let charging = if bank < shuttle_cost && !first_spawned { k } else { 0 };
        bank += charging as u64 + income as u64;

        // Conversion this tick: energy- and WORK-bounded, 1 e per progress.
        let conv = upgrader_work.min(bank);
        progress += conv;
        bank -= conv;
        if progress >= needed_progress {
            return t;
        }

        // Spawn the next needed body (harvesters to saturation first — income compounds — then
        // upgraders to the WORK cap), head-of-line banking on the shared queue kernel. Pending
        // (spawned, in transit) bodies count toward saturation targeting.
        let need_harvester = per_source_work
            .iter()
            .zip(&pending_work)
            .any(|(&w, &p)| 2 * (w + p) < 10);
        let need_upgrader = upgrader_work + upgraders_arriving.len() as u64 <= work_cap;
        let idle_spawns = spawn_busy_until.iter().filter(|&&b| b <= t).count() as u32;
        if idle_spawns > 0 && (need_harvester || need_upgrader) {
            let (cost, parts) = if need_harvester { (shuttle_cost, shuttle_parts) } else { (upgrader_cost, 1) };
            let mut lanes = HomeLanes {
                idle_spawns,
                available_energy: bank.min(u32::MAX as u64) as u32,
                energy_capacity: (cost as u32).max(SPAWN_ENERGY_CAPACITY),
            };
            let queue = [QueuedSpawn { priority: 100.0, body_cost: cost as u32, part_count: parts, id: 1 }];
            for s in spawn_step(&mut lanes, &queue) {
                bank -= cost;
                if need_harvester {
                    let src = (0..per_source_work.len())
                        .min_by_key(|&i| (per_source_work[i] + pending_work[i], i))
                        .expect("sources non-empty");
                    arrivals.push((t + s.completes_in + travel, src, 1));
                    pending_work[src] += 1;
                } else {
                    upgraders_arriving.push(t + s.completes_in);
                }
                if let Some(b) = spawn_busy_until.iter_mut().find(|b| **b <= t) {
                    *b = t + s.completes_in;
                }
                first_spawned = true;
            }
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

    /// The oracle is finite, positive, and floored on real catalog scenarios. Under the #7
    /// lane-only recovered state, a stocked-storage scenario's floor is `full_window` alone
    /// (the S0 bank optimistically covers the lane at t=0 — no bodies needed); only the
    /// S0 = 0 scenarios keep the self-charge floor (≥ ~250/k ticks to the first body).
    #[test]
    fn oracle_is_finite_and_sane_on_the_catalog() {
        let consts = RecoverConsts::default();
        for sc in catalog().iter().take(4) {
            let (world, terrain, info) = instantiate(sc);
            let mut mover = AnalyticMover::new(&terrain);
            let t = t_star(&world, &mut mover, &info, &consts);
            assert!(t > 0, "{}: T* positive", sc.name);
            assert!(t < 100_000, "{}: T* finite ({t})", sc.name);
            assert!(t >= consts.full_window, "{}: T* ≥ the sustained-full window", sc.name);
            if sc.storage_energy == 0 {
                // Self-charge to the 250 first body needs ≥ ~250/k ticks — the chain's floor.
                assert!(t >= 250 / world.spawns.len() as u32, "{}: T* ≥ the charge floor", sc.name);
            }
        }
    }
}
