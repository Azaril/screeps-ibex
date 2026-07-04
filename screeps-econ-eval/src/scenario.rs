//! The scenario families (ADR 0040 §D7):
//!
//! - **Family C — collapse** (M1): a captured foreman layout realized as of RCL R,
//!   spawns/extensions drained to 0, storage at S0, creeps wiped or a skeleton, roads
//!   pre-decayed (the BAIT axis) — run until recovered or the tick cap. Every bait scenario has
//!   an IDENTICAL no-bait control (roads at 100%, same seed ⇒ same decay phases/skeleton) so the
//!   repro gate's paired diff isolates exactly the repair-bait axis. Since M2 the downgrade
//!   clock TICKS (it starts full here — no downgrade pressure inside a recovery horizon).
//! - **Family G — greenfield rush** (M2): virgin room at RCL 1 with the plan's anchor spawn
//!   only; the construction pass builds the plan per `required_rcl` as the rush levels; scored
//!   by T_RCL(N) against the conservation-bound T*_RCL(N) oracle. Seeds jitter the
//!   construction-pass phase (live rooms sit at arbitrary 50-tick phase; greenfield has no other
//!   natural jitter — documented).
//! - **Family D — downgrade pressure** (M2): the Family C collapse start with the downgrade
//!   clock at 10% — the refill-vs-controller-save triage; scored by T_recover AND levels_lost.
//! - **Family S — steady state, THE GUARD RAIL** (M2): a healthy room (full lane, stocked
//!   stores, full fleet with seed-jittered TTLs, roads at 100%) run a 10k-tick horizon;
//!   refill-latency distribution, spawn idle, road-stock trajectory (must not collapse),
//!   steady-state repair leak, flap rate, and intents/tick are the products. Includes LOW-RCL
//!   healthy rooms (the S1 constants' breadth — the §D8 #2 evidence channel).

use crate::layout::{
    controller_downgrade_full, realize, realize_greenfield, LayoutInfo, RealizeParams,
};
use screeps::{Part, Position};
use screeps_econ_engine::{EconWorld, SimResource};
use screeps_rover_eval::base_traffic::{captured_layouts, CapturedLayout};
use screeps_sim_core::rng::Rng;
use screeps_sim_core::SimTerrain;

/// Initial creep population.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreepInit {
    /// Total wipe: zero creeps — recovery starts from spawn self-charge alone.
    Wiped,
    /// One surviving bootstrap harvester ([M,M,C,W]) with a seed-jittered mid-life TTL, placed
    /// beside the first spawn.
    Skeleton,
}

/// One Family-C scenario (the M1 spec's exact axes).
#[derive(Clone, Debug)]
pub struct EconScenario {
    pub name: String,
    pub layout_room: String,
    pub rcl: u8,
    /// S0 — energy pre-loaded into storage (requires RCL ≥ 4; asserted at instantiation).
    pub storage_energy: u32,
    pub creeps: CreepInit,
    /// Road starting health (% of hitsMax): 30/60 = bait, 100 = control.
    pub road_health_pct: u32,
    /// Downgrade clock as % of the RCL's full clock — STATE ONLY until M2.
    pub downgrade_clock_pct: u32,
    pub tick_cap: u32,
    /// Jitters decay phases + skeleton TTL (identical in a scenario's bait/control pair).
    pub seed: u32,
    /// Whether this is a bait arm (decayed roads) — controls pair to `bait == false`.
    pub bait: bool,
}

/// Default per-run tick cap (~the M1 spec's 15_000; `ECON_TICK_CAP` overrides in the bench).
pub const DEFAULT_TICK_CAP: u32 = 15_000;

impl EconScenario {
    /// The identical NO-BAIT control: roads at 100%, everything else — INCLUDING the seed, so
    /// decay phases and the skeleton match — unchanged.
    pub fn control(&self) -> EconScenario {
        EconScenario {
            name: format!("{}-CTRL", self.name),
            road_health_pct: 100,
            bait: false,
            ..self.clone()
        }
    }

    /// The same scenario under a different seed (the repro gate's N-seed axis).
    pub fn with_seed(&self, seed: u32) -> EconScenario {
        let base = self.name.clone();
        EconScenario { name: format!("{base}#s{seed}"), seed, ..self.clone() }
    }
}

fn scenario(
    room: &str,
    rcl: u8,
    storage_energy: u32,
    creeps: CreepInit,
    road_health_pct: u32,
) -> EconScenario {
    let c = match creeps {
        CreepInit::Wiped => "wiped",
        CreepInit::Skeleton => "skel",
    };
    EconScenario {
        name: format!("{room}-rcl{rcl}-s{}k-r{road_health_pct}-{c}", storage_energy / 1000),
        layout_room: room.to_string(),
        rcl,
        storage_energy,
        creeps,
        road_health_pct,
        downgrade_clock_pct: 100,
        tick_cap: DEFAULT_TICK_CAP,
        seed: 1,
        bait: true,
    }
}

/// The curated BAIT catalog: 10 scenarios spanning the axes (13 layouts × RCL {3,4,5,6} ×
/// S0 {0, 5k, 50k} × road {30, 45} × wiped/skeleton — a representative spread, not the product;
/// RCL 3 has no storage so its S0 is 0 by construction). Pair each with [`EconScenario::control`]
/// for the gate.
///
/// **Bait-axis deviation from the M1 spec's {30, 60} sketch (measured, documented):** roads at
/// ≥ 50% health map to `RepairPriority::Low` (repair.rs:23-37), which is BELOW every local-room
/// repair admission the transcribed policy has (harvesters ≥ Medium — harvest.rs:225; the hauler
/// ≥ Low arm is gated to REMOTE missions by `allow_repair = max_distance > 0`,
/// missions/haul.rs:295) — a 60%-health room produces ZERO admissible repair within the recovery
/// horizon, i.e. it is not bait under the current bot at all (verified in the first bench run:
/// leak = 0, ΔT_recover = 0). The high end of the bait axis is therefore 45% (still < 50% ⇒
/// Medium), keeping every catalog bait scenario genuinely diseased for the repro gate's
/// every-run-leaks arm.
pub fn catalog() -> Vec<EconScenario> {
    vec![
        scenario("E11N1", 3, 0, CreepInit::Wiped, 30),
        scenario("E11N37", 4, 0, CreepInit::Wiped, 30),
        scenario("E12S41", 4, 5_000, CreepInit::Wiped, 30),
        scenario("E13S29", 5, 5_000, CreepInit::Wiped, 45),
        scenario("E11N13", 5, 50_000, CreepInit::Wiped, 30),
        scenario("E11N14", 6, 50_000, CreepInit::Wiped, 45),
        scenario("E11N23", 6, 0, CreepInit::Skeleton, 30),
        scenario("E11N31", 4, 5_000, CreepInit::Skeleton, 45),
        scenario("E11N32", 5, 0, CreepInit::Wiped, 30),
        scenario("E11N11", 6, 5_000, CreepInit::Wiped, 45),
    ]
}

/// The fast-mode subset (4 bait scenarios spanning RCL 3/4/6, S0 0/5k/50k, road 30/60,
/// wiped+skeleton) — `econ_bench fast` runs these + controls through the gate.
pub fn fast_catalog() -> Vec<EconScenario> {
    let all = catalog();
    [0usize, 2, 5, 6].iter().map(|&i| all[i].clone()).collect()
}

/// A procedural bait variant (M1 spec: `generate(seed)`): layout/RCL/S0/road drawn from the
/// seeded kernel RNG over the same axes — extra corpus breadth for the full mode + the fence.
pub fn generate(seed: u32) -> EconScenario {
    let mut rng = Rng::seeded(seed);
    let layouts = captured_layouts();
    let room = layouts[(rng.next_u64() % layouts.len() as u64) as usize].room.clone();
    let rcl = rng.range(3, 6) as u8;
    let storage_energy = if rcl >= 4 { [0u32, 5_000, 50_000][(rng.next_u64() % 3) as usize] } else { 0 };
    let creeps = if rng.chance(30) { CreepInit::Skeleton } else { CreepInit::Wiped };
    // Bait range 25..=48: strictly below the 50% Medium threshold (the catalog's bait-axis note).
    let road = rng.range(25, 48);
    let mut sc = scenario(&room, rcl, storage_energy, creeps, road);
    sc.name = format!("gen{seed}-{}", sc.name);
    sc.seed = seed;
    sc
}

/// Instantiate: realize the layout as-of-RCL, then apply the COLLAPSE state — spawns/extensions
/// drained to 0 (the ADR's collapse definition), storage loaded with S0, the downgrade clock set
/// (state-only), and the initial creeps placed.
pub fn instantiate(sc: &EconScenario) -> (EconWorld, SimTerrain, LayoutInfo) {
    let layouts = captured_layouts();
    let layout: &CapturedLayout = layouts
        .iter()
        .find(|l| l.room == sc.layout_room)
        .unwrap_or_else(|| panic!("scenario layout `{}` not in the captured cache", sc.layout_room));

    let realized = realize(
        layout,
        &RealizeParams { rcl: sc.rcl, road_health_pct: sc.road_health_pct, seed: sc.seed },
    );
    let mut world = realized.world;

    // The collapse drain: spawn/extension energy to 0 (extensions are born empty already).
    for s in &mut world.spawns {
        s.store_energy = 0;
    }
    // S0 into storage (RCL ≥ 4 by scenario construction — asserted).
    if sc.storage_energy > 0 {
        let storage = world
            .storage
            .as_mut()
            .unwrap_or_else(|| panic!("{}: S0 > 0 needs storage (RCL ≥ 4)", sc.name));
        storage.store.add(SimResource::Energy, sc.storage_energy);
    }
    // Downgrade clock: scenario STATE, reported not ticked (module docs).
    if let Some(c) = world.controller.as_mut() {
        c.downgrade_ticks =
            (controller_downgrade_full(sc.rcl) as u64 * sc.downgrade_clock_pct as u64 / 100) as u32;
    }
    // Initial creeps.
    if sc.creeps == CreepInit::Skeleton {
        let mut rng = Rng::seeded(sc.seed.wrapping_mul(7919).wrapping_add(13));
        let spawn_pos = world.spawns[0].pos;
        let tile = skeleton_tile(&world, spawn_pos);
        let ttl = rng.range(300, 900); // a mid-life survivor
        world.add_creep(tile, &[Part::Move, Part::Move, Part::Carry, Part::Work], ttl);
    }

    (world, realized.terrain, realized.info)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Family G — greenfield rush (M2).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One greenfield rush scenario: layout + target RCL. The SEED jitters the construction-pass
/// phase only (module docs).
#[derive(Clone, Debug)]
pub struct RushScenario {
    pub name: String,
    pub layout_room: String,
    pub target_rcl: u8,
    pub tick_cap: u32,
    pub seed: u32,
}

/// Default Family-G tick cap (N ≤ 4 rushes finish well inside; env `ECON_G_TICK_CAP` overrides
/// in the bench). RCL 4 needs 180,200 progress ≈ 9k ticks at full 2-source potential — real
/// baselines run 3-6× the oracle, hence the headroom.
pub const DEFAULT_G_TICK_CAP: u32 = 60_000;

impl RushScenario {
    pub fn new(room: &str, target_rcl: u8, seed: u32) -> Self {
        RushScenario {
            name: format!("G-{room}-rcl{target_rcl}#s{seed}"),
            layout_room: room.to_string(),
            target_rcl,
            tick_cap: DEFAULT_G_TICK_CAP,
            seed,
        }
    }

    /// Instantiate the greenfield world (+ its Family-C-shaped shell for the runner's
    /// `EconScenario` fields the outcome line reads).
    pub fn instantiate(&self) -> (EconWorld, SimTerrain, LayoutInfo) {
        let layouts = captured_layouts();
        let layout: &CapturedLayout = layouts
            .iter()
            .find(|l| l.room == self.layout_room)
            .unwrap_or_else(|| panic!("rush layout `{}` not in the captured cache", self.layout_room));
        let realized = realize_greenfield(layout);
        (realized.world, realized.terrain, realized.info)
    }

    /// The runner's scenario shell (name/seed/cap — the RunOutcome header fields).
    pub fn shell(&self) -> EconScenario {
        EconScenario {
            name: self.name.clone(),
            layout_room: self.layout_room.clone(),
            rcl: 1,
            storage_energy: 0,
            creeps: CreepInit::Wiped,
            road_health_pct: 100,
            downgrade_clock_pct: 100,
            tick_cap: self.tick_cap,
            seed: self.seed,
            bait: false,
        }
    }
}

/// The G corpus: every captured layout at `target_rcl` × `seeds` phase-jitter seeds.
pub fn rush_catalog(target_rcl: u8, seeds: u32) -> Vec<RushScenario> {
    captured_layouts()
        .iter()
        .flat_map(|l| (1..=seeds).map(move |s| RushScenario::new(&l.room, target_rcl, s)))
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Family D — downgrade pressure (M2).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Family D = the Family C catalog with the downgrade clock at 10% (the refill-vs-controller
/// triage; scored by T_recover AND levels_lost).
pub fn downgrade_catalog() -> Vec<EconScenario> {
    catalog()
        .into_iter()
        .map(|mut sc| {
            sc.downgrade_clock_pct = 10;
            sc.name = format!("D-{}", sc.name);
            sc
        })
        .collect()
}

/// The fast-mode Family-D subset (mirrors `fast_catalog`'s axes).
pub fn fast_downgrade_catalog() -> Vec<EconScenario> {
    fast_catalog()
        .into_iter()
        .map(|mut sc| {
            sc.downgrade_clock_pct = 10;
            sc.name = format!("D-{}", sc.name);
            sc
        })
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Family S — steady-state guard rail (M2).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One steady-state scenario: a HEALTHY room at `rcl` run for a 10k-tick horizon.
#[derive(Clone, Debug)]
pub struct SteadyScenario {
    pub name: String,
    pub layout_room: String,
    pub rcl: u8,
    pub tick_cap: u32,
    pub seed: u32,
}

pub const DEFAULT_S_TICK_CAP: u32 = 10_000;

impl SteadyScenario {
    pub fn new(room: &str, rcl: u8, seed: u32) -> Self {
        SteadyScenario {
            name: format!("S-{room}-rcl{rcl}#s{seed}"),
            layout_room: room.to_string(),
            rcl,
            tick_cap: DEFAULT_S_TICK_CAP,
            seed,
        }
    }

    /// Instantiate the healthy world: full realization at `rcl` (roads 100%, decay phases
    /// seed-jittered), spawn lane FULL, storage stocked to 200k (RCL ≥ 4 — `has_excess`
    /// exercised) / source+controller containers stocked (RCL < 4), and the steady fleet
    /// pre-seeded with seed-jittered TTLs so the TTL churn spreads: 2 capacity-sized shuttle
    /// harvesters per source + 2 haulers (upgraders/builders spawn organically within the first
    /// K4 passes — their bodies depend on live state the policy itself computes).
    pub fn instantiate(&self) -> (EconWorld, SimTerrain, LayoutInfo) {
        let layouts = captured_layouts();
        let layout: &CapturedLayout = layouts
            .iter()
            .find(|l| l.room == self.layout_room)
            .unwrap_or_else(|| panic!("steady layout `{}` not in the captured cache", self.layout_room));
        let realized = realize(
            layout,
            &RealizeParams { rcl: self.rcl, road_health_pct: 100, seed: self.seed },
        );
        let mut world = realized.world;
        let info = realized.info;

        // Healthy stores: full spawn lane (spawns are born full; extensions topped)…
        for i in 0..world.extensions.len() {
            world.extensions[i].store_energy = world.extensions[i].capacity;
        }
        // …stocked storage at RCL ≥ 4 (200k = the desired amount; has_excess true) or stocked
        // containers below (source containers half, the controller container near-full).
        if let Some(storage) = world.storage.as_mut() {
            storage.store.add(SimResource::Energy, 200_000);
        } else {
            let roles = info.container_roles.clone();
            for c in world.containers.iter_mut() {
                let tile = (c.pos.x().u8(), c.pos.y().u8());
                match roles.get(&tile) {
                    Some(crate::layout::ContainerRole::Controller) => {
                        c.store.add(SimResource::Energy, 1_600);
                    }
                    _ => {
                        c.store.add(SimResource::Energy, 1_000);
                    }
                }
            }
        }

        // The steady fleet with jittered TTLs (uniform 200..1400 — churn spreads immediately).
        let mut rng = Rng::seeded(self.seed.wrapping_mul(6151).wrapping_add(17));
        let capacity = crate::baseline::spawn_lane_capacity(&world);
        let harvester = crate::baseline::harvester_body(capacity).expect("capacity ≥ 300");
        let hauler = crate::baseline::hauler_body(capacity).expect("capacity ≥ 300");
        let spawn_pos = world.spawns[0].pos;
        let n_sources = world.sources.len();
        let mut placed = 0u8;
        for _src in 0..n_sources {
            for _ in 0..2 {
                let tile = fleet_tile(&world, spawn_pos, placed);
                placed += 1;
                let ttl = rng.range(200, 1400);
                world.add_creep(tile, &harvester, ttl);
            }
        }
        for _ in 0..2 {
            let tile = fleet_tile(&world, spawn_pos, placed);
            placed += 1;
            let ttl = rng.range(200, 1400);
            world.add_creep(tile, &hauler, ttl);
        }

        (world, realized.terrain, info)
    }

    pub fn shell(&self) -> EconScenario {
        EconScenario {
            name: self.name.clone(),
            layout_room: self.layout_room.clone(),
            rcl: self.rcl,
            storage_energy: 0,
            creeps: CreepInit::Wiped,
            road_health_pct: 100,
            downgrade_clock_pct: 100,
            tick_cap: self.tick_cap,
            seed: self.seed,
            bait: false,
        }
    }
}

/// The S corpus: RCL {2 (low-RCL healthy — the §D8 #2 evidence channel), 4, 6} over a 4-layout
/// spread (full mode; `fast` takes the first pair).
pub fn steady_catalog(seed: u32) -> Vec<SteadyScenario> {
    let rooms = ["E11N1", "E12S41", "E11N14", "E13S29"];
    let mut out = Vec::new();
    for (i, room) in rooms.iter().enumerate() {
        let rcl = [2u8, 4, 6, 4][i];
        out.push(SteadyScenario::new(room, rcl, seed));
    }
    // The low-RCL breadth arm: a second RCL-2 and an RCL-3 healthy room.
    out.push(SteadyScenario::new("E11N37", 3, seed));
    out.push(SteadyScenario::new("E11N23", 2, seed));
    out
}

/// A free walkable tile near the spawn for fleet seeding: ring-scan outward from the spawn,
/// skipping occupied tiles, offset by `n` (deterministic).
fn fleet_tile(world: &EconWorld, spawn_pos: Position, n: u8) -> Position {
    let mut count = 0u8;
    for radius in 1..=4i32 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                if let Ok(p) = spawn_pos.checked_add((dx, dy)) {
                    if world.is_walkable(p) {
                        if count == n {
                            return p;
                        }
                        count += 1;
                    }
                }
            }
        }
    }
    panic!("no free fleet tile within radius 4 of the spawn");
}

/// The first walkable tile adjacent to the spawn (row-major, the birth-tile order) for the
/// skeleton survivor.
fn skeleton_tile(world: &EconWorld, spawn_pos: Position) -> Position {
    let mut candidates: Vec<Position> = Vec::new();
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            if let Ok(p) = spawn_pos.checked_add((dx, dy)) {
                candidates.push(p);
            }
        }
    }
    candidates.sort_by_key(|p| (p.y().u8(), p.x().u8()));
    candidates
        .into_iter()
        .find(|&p| world.is_walkable(p))
        .expect("a foreman spawn always has a walkable neighbor")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catalog well-posedness: every scenario instantiates; the collapse drain holds; controls
    /// differ ONLY on the road axis; skeletons exist; RCL-3 scenarios carry no S0.
    #[test]
    fn catalog_instantiates_and_controls_pair() {
        for sc in catalog() {
            assert!(sc.bait);
            if sc.rcl < 4 {
                assert_eq!(sc.storage_energy, 0, "{}: no storage below RCL 4", sc.name);
            }
            let (world, _, info) = instantiate(&sc);
            assert!(world.room_spawn_energy() == 0, "{}: collapse drains the spawn lane", sc.name);
            assert!(!world.sources.is_empty());
            assert!(!world.spawns.is_empty());
            assert!(!info.source_containers.is_empty(), "{}: harvest containers mapped", sc.name);
            if sc.storage_energy > 0 {
                assert_eq!(
                    world.storage.as_ref().unwrap().store.amount(SimResource::Energy),
                    sc.storage_energy
                );
            }
            match sc.creeps {
                CreepInit::Wiped => assert!(world.movement.creeps.is_empty()),
                CreepInit::Skeleton => assert_eq!(world.movement.creeps.len(), 1),
            }
            for r in &world.roads {
                assert_eq!(r.hits, r.hits_max * sc.road_health_pct / 100, "{}: bait roads", sc.name);
            }

            let ctrl = sc.control();
            let (cw, _, _) = instantiate(&ctrl);
            for r in &cw.roads {
                assert_eq!(r.hits, r.hits_max, "{}: control roads at 100%", ctrl.name);
            }
            assert_eq!(
                cw.roads.len(),
                world.roads.len(),
                "control differs from bait ONLY on the health axis"
            );
        }
    }

    /// Family G well-posedness (M2): greenfield = anchor spawn + controller level 1 (full
    /// 20k clock) + virgin sources, NOTHING else; the plan schedule is available for the
    /// construction pass; the D catalog only moves the clock axis; Family S rooms are healthy
    /// (full lane, stocked stores, seeded fleet with jittered TTLs).
    #[test]
    fn m2_families_instantiate_well_posed() {
        let rush = RushScenario::new("E11N1", 4, 1);
        let (w, _, info) = rush.instantiate();
        assert_eq!(w.spawns.len(), 1, "the anchor spawn only");
        assert_eq!(w.spawns[0].store_energy, 300, "born full (the documented greenfield E0)");
        assert!(w.extensions.is_empty() && w.roads.is_empty() && w.containers.is_empty());
        assert!(w.storage.is_none() && w.sites.is_empty() && w.towers.is_empty());
        let c = w.controller.as_ref().unwrap();
        assert_eq!((c.level, c.progress, c.downgrade_ticks), (1, 0, 20_000));
        assert!(w.sources.iter().all(|s| s.energy == 3000), "virgin sources");
        assert!(!info.plan_structures.is_empty(), "the build schedule rides along");
        assert!(
            info.plan_structures.iter().any(|s| s.kind == screeps_econ_engine::StructureKind::Extension && s.required_rcl == 2),
            "RCL-2 extensions are in the schedule"
        );

        for (d, c) in downgrade_catalog().iter().zip(catalog().iter()) {
            assert_eq!(d.downgrade_clock_pct, 10);
            assert_eq!(d.layout_room, c.layout_room, "D = C with only the clock axis moved");
            assert_eq!(d.road_health_pct, c.road_health_pct);
        }
        let (w, _, _) = instantiate(&downgrade_catalog()[0]);
        let c = w.controller.as_ref().unwrap();
        assert_eq!(c.downgrade_ticks, controller_downgrade_full(c.level) / 10, "clock at 10%");

        let steady = SteadyScenario::new("E12S41", 4, 7);
        let (w, _, _) = steady.instantiate();
        let capacity = crate::baseline::spawn_lane_capacity(&w);
        assert_eq!(w.room_spawn_energy(), capacity, "healthy: the spawn lane starts FULL");
        assert_eq!(
            w.storage.as_ref().unwrap().store.amount(SimResource::Energy),
            200_000,
            "healthy: storage stocked to the desired amount"
        );
        assert!(w.movement.creeps.len() >= 4, "the steady fleet is seeded");
        let ttls: Vec<u32> = w.creep_ttl.values().copied().collect();
        assert!(
            ttls.iter().max() > ttls.iter().min(),
            "TTLs jittered — churn spreads ({ttls:?})"
        );
        // A low-RCL healthy room stocks its containers instead.
        let steady2 = SteadyScenario::new("E11N1", 2, 7);
        let (w2, _, _) = steady2.instantiate();
        assert!(w2.storage.is_none());
        assert!(w2.containers.iter().any(|c| c.store.amount(SimResource::Energy) > 0), "containers stocked");
    }

    /// Seeds jitter phases/skeletons but never the axes; generate() stays in the axes' ranges.
    #[test]
    fn seeds_and_generation_stay_on_axis() {
        let sc = &catalog()[6]; // the skeleton scenario
        let (a, _, _) = instantiate(&sc.with_seed(11));
        let (b, _, _) = instantiate(&sc.with_seed(12));
        assert_ne!(a.state_digest(), b.state_digest(), "seeds genuinely vary the paired runs");
        assert_eq!(a.movement.creeps.len(), 1);
        for seed in 0..6u32 {
            let g = generate(seed);
            assert!((3..=6).contains(&g.rcl));
            assert!((25..=48).contains(&g.road_health_pct), "generated bait stays below the 50% Medium threshold");
            let (w, _, _) = instantiate(&g);
            assert!(!w.spawns.is_empty());
        }
    }
}
