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

/// The highest RCL at which a captured layout adds a structure — its full-build stage. Structures
/// with no `required_rcl` (out-of-vocab furniture) are ignored; a plan always has an RCL-1 spawn.
pub fn layout_max_rcl(layout: &CapturedLayout) -> u8 {
    layout.structures.iter().filter_map(|s| s.required_rcl).max().unwrap_or(1).min(8)
}

/// ADR 0044 P2 — the FOREMAN-LAYOUT × RCL validation sweep. Every captured foreman layout realized at
/// each requested RCL stage it supports (a real room's growth curve), as a healthy `SteadyScenario`.
/// Proves the economy + transfer market — now structure-aware with true routed distance + the
/// unreachable-arc exclusion — behaves across the whole corpus, not just the curated guard rail.
/// `rcls` is the stage set to probe (capped per layout at its full-build RCL; stages above a layout's
/// max are skipped, so a small room isn't given a phantom high-RCL controller with no structures).
pub fn foreman_rcl_sweep(seed: u32, rcls: &[u8]) -> Vec<SteadyScenario> {
    let mut out = Vec::new();
    for layout in captured_layouts() {
        let max_rcl = layout_max_rcl(&layout);
        for &rcl in rcls {
            if rcl >= 1 && rcl <= max_rcl {
                out.push(SteadyScenario::new(&layout.room, rcl, seed));
            }
        }
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Family R — REMOTE MINING (ADR 0044 P2). A healthy home plus synthetic remote sources at
// controlled TRUE path-distances (open corridor rooms east of home), so the reduced-cost admission
// can be measured serving/declining remote hauls by distance. The runner drives creeps with the
// multi-room `RoverMover` (selected when `remote_distances` is non-empty).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One remote-mining scenario: a healthy home + remote sources at the given true path distances.
#[derive(Clone, Debug)]
pub struct FamilyRScenario {
    pub name: String,
    pub layout_room: String,
    pub rcl: u8,
    /// True path-distances (tiles east of the home spawn) of the remote sources, e.g.
    /// `[10,40,90,150,210,260]` — spanning same-room (10) to ~5 rooms out (260). In the REALISTIC
    /// variant these are re-interpreted as one remote per corridor ROOM (ordinal), and the true
    /// routed distance (≫ Chebyshev through the generated walls) is what the mover measures.
    pub remote_distances: Vec<u32>,
    pub tick_cap: u32,
    pub seed: u32,
    /// ADR 0044: use PROCEDURAL realistic terrain for the corridor rooms (cave walls + swamps +
    /// mid-edge exits) instead of the synthetic open channel — so `routed ≫ Chebyshev` (the regime
    /// the true-distance migration must handle). One remote per corridor room at its centre.
    pub realistic: bool,
    /// ADR 0044 step-2 economic-decline validation: SATURATE the home so refill demand doesn't make
    /// every remote a bargain (the hungry-refill corridor "correctly serves all" — ADR success-gate
    /// note). Seeds a long-lived fleet (no churn ⇒ no respawn ⇒ extensions stay topped ⇒ no refill
    /// hunger) so the marginal sink is storage-at-PAR — the regime where the routed haul subtraction
    /// can push a FAR remote past break-even and the admission DECLINES it in-sim.
    pub saturated: bool,
}

impl FamilyRScenario {
    pub fn new(room: &str, rcl: u8, remote_distances: Vec<u32>, seed: u32) -> Self {
        FamilyRScenario {
            name: format!("R-{room}-rcl{rcl}#s{seed}"),
            layout_room: room.to_string(),
            rcl,
            remote_distances,
            tick_cap: DEFAULT_S_TICK_CAP,
            seed,
            realistic: false,
            saturated: false,
        }
    }

    /// Use procedural realistic terrain for the corridor (ADR 0044). One remote per corridor room.
    pub fn realistic(mut self) -> Self {
        self.realistic = true;
        self.name = format!("{}-realistic", self.name);
        self
    }

    /// Saturate the home so remotes price at PAR (see [`Self::saturated`]) — the regime that exposes
    /// the admission's beyond-break-even DECLINE in-sim.
    pub fn saturated(mut self) -> Self {
        self.saturated = true;
        self.name = format!("{}-sat", self.name);
        self
    }

    /// Instantiate the multi-room world: a healthy home (mirrors [`SteadyScenario::instantiate`]) +
    /// synthetic remotes. Each remote is a `SimSource` (+ drop container one tile toward home) placed
    /// `d` tiles east of the home spawn over OPEN corridor rooms, so the true routed distance ≈ `d`.
    /// The home room keeps its realized terrain (structure walls); only the corridor rooms are open.
    pub fn instantiate(&self) -> (EconWorld, SimTerrain, LayoutInfo) {
        let layouts = captured_layouts();
        let layout: &CapturedLayout = layouts
            .iter()
            .find(|l| l.room == self.layout_room)
            .unwrap_or_else(|| panic!("family-R layout `{}` not in the captured cache", self.layout_room));
        let realized = realize(layout, &RealizeParams { rcl: self.rcl, road_health_pct: 100, seed: self.seed });
        let mut world = realized.world;
        let info = realized.info;

        // Healthy home stores: full spawn lane + stocked storage (RCL ≥ 4 by construction).
        for i in 0..world.extensions.len() {
            world.extensions[i].store_energy = world.extensions[i].capacity;
        }
        if let Some(storage) = world.storage.as_mut() {
            storage.store.add(SimResource::Energy, 200_000);
        }

        // Synthetic remotes over an OPEN corridor east of home.
        let home_spawn = world.spawns[0].pos;
        let home_room = home_spawn.room_name();
        // Carve a guaranteed east exit channel along the spawn's row: captured rooms need not have
        // an east exit (E11N1 has none), so clear the home terrain's walls on that row from the
        // spawn to the east border, connecting to the open corridor. A synthetic "remote highway"
        // — a deterministic modeling stopgap until realistic terrain generation lands (ADR deferred
        // note); the structures still exist as world objects, only their pathing-wall tiles on this
        // one row are cleared.
        let (sx, sy) = (home_spawn.x().u8(), home_spawn.y().u8());
        // The home→corridor seam band. In the REALISTIC variant the first corridor room is GENERATED
        // and its west exit is the SHARED seam range `seam_range_between(home, room1)` — the home
        // (a captured room) carves EXACTLY that, so the seam matches by construction (the engine's
        // aligned-exit invariant; the kernel relocates across exits without a wall check,
        // tick.rs:53-76, so a mismatch would drop a creep onto a wall). Synthetic → the channel's `sy±1`.
        let room1 = home_spawn.checked_add((50, 0)).ok().map(|p| p.room_name());
        let (blo, bhi) = match (self.realistic, room1) {
            (true, Some(r1)) => screeps_sim_core::terrain_gen::seam_range_between(home_room, r1),
            _ => (sy.saturating_sub(1), (sy + 1).min(49)),
        };
        // Interior highway: spawn row out to x=48, then column 48 spanning the band rows (interior);
        // the edge column 49 is opened only at the aligned seam band.
        for x in sx..49 {
            world.movement.terrain.walls.remove(&(x, sy));
        }
        for y in sy.min(blo)..=sy.max(bhi) {
            world.movement.terrain.walls.remove(&(48, y));
        }
        for y in blo..=bhi {
            world.movement.terrain.walls.remove(&(49, y));
        }
        // Register the HOME room as a known world room (with its carved terrain) so the multi-room
        // mover treats every OTHER room as impassable and never wanders off the corridor.
        world.movement.rooms.insert(home_room, world.movement.terrain.clone());

        if self.realistic {
            // PROCEDURAL corridor: one GENERATED room per remote, chained east. Each room's exits are
            // SEAM-DERIVED (shared with neighbours) via `generate_terrain_for_room`, so every seam
            // aligns by construction — no carving; the cave walls make routed ≫ Chebyshev. A remote
            // source at each room's centre (seeded-open + connected).
            use screeps_sim_core::terrain_gen::{generate_terrain_for_room, Exits, TerrainGenParams, EXIT_MID};
            let params = TerrainGenParams::default();
            for (k, _) in self.remote_distances.iter().enumerate() {
                let Ok(anchor) = home_spawn.checked_add(((k as i32 + 1) * 50, 0)) else {
                    continue;
                };
                let room = anchor.room_name();
                world.movement.rooms.insert(room, generate_terrain_for_room(room, self.seed, Exits::horizontal(), &params));
                let src_pos = Position::new(
                    screeps::RoomCoordinate::new(EXIT_MID).unwrap(),
                    screeps::RoomCoordinate::new(EXIT_MID).unwrap(),
                    room,
                );
                world.add_source(src_pos, crate::layout::SOURCE_CAPACITY);
                if let Ok(cont_pos) = src_pos.checked_add((-1, 0)) {
                    world.add_container(cont_pos, crate::layout::CONTAINER_CAPACITY, screeps_econ_engine::constants::CONTAINER_HITS);
                }
            }
        } else {
            // Synthetic 3-wide horizontal CHANNEL corridor at the spawn row (walled elsewhere): forces
            // the search onto a straight highway (no diagonal room-CORNER crossings the walk can't
            // follow) with room for the harvester/container. Remotes at the requested tile distances.
            let mut channel = SimTerrain::default();
            let (lo, hi) = (sy.saturating_sub(1), (sy + 1).min(49));
            for x in 0..50u8 {
                for y in 0..50u8 {
                    if y < lo || y > hi {
                        channel.walls.insert((x, y));
                    }
                }
            }
            let max_d = self.remote_distances.iter().copied().max().unwrap_or(0);
            for step in 1..=max_d {
                if let Ok(p) = home_spawn.checked_add((step as i32, 0)) {
                    if p.room_name() != home_room {
                        world.movement.rooms.entry(p.room_name()).or_insert_with(|| channel.clone());
                    }
                }
            }
            for &d in &self.remote_distances {
                let Ok(src_pos) = home_spawn.checked_add((d as i32, 0)) else {
                    continue;
                };
                world.add_source(src_pos, crate::layout::SOURCE_CAPACITY);
                if let Ok(cont_pos) = src_pos.checked_add((-1, 0)) {
                    world.add_container(cont_pos, crate::layout::CONTAINER_CAPACITY, screeps_econ_engine::constants::CONTAINER_HITS);
                }
            }
        }

        // Seed the fleet AFTER the remotes so every source (home + remote) gets shuttle harvesters;
        // extra haulers for the remote lanes.
        let mut rng = Rng::seeded(self.seed.wrapping_mul(6151).wrapping_add(29));
        let capacity = crate::baseline::spawn_lane_capacity(&world);
        let harvester = crate::baseline::harvester_body(capacity).expect("capacity ≥ 300");
        let hauler = crate::baseline::hauler_body(capacity).expect("capacity ≥ 300");
        // Saturated: long-lived creeps (outlast the run) so nothing dies → nothing respawns → the
        // spawn lane stays topped → refill demand vanishes → remotes compete at storage PAR.
        let ttl = |rng: &mut Rng| {
            if self.saturated {
                self.tick_cap + 500
            } else {
                rng.range(200, 1400)
            }
        };
        let n_sources = world.sources.len();
        let mut placed = 0u8;
        for _ in 0..n_sources {
            for _ in 0..2 {
                let tile = fleet_tile(&world, home_spawn, placed);
                placed = placed.wrapping_add(1);
                let t = ttl(&mut rng);
                world.add_creep(tile, &harvester, t);
            }
        }
        // Saturated runs seed GENEROUS haulers (3× per remote) so haul CAPACITY never bottlenecks —
        // a remote left un-hauled is then a genuine admission DECLINE (delivered ≤ 0), not carriers
        // simply being busy elsewhere.
        let n_haulers = if self.saturated { 2 + 3 * self.remote_distances.len() } else { 2 + self.remote_distances.len() };
        for _ in 0..n_haulers {
            let tile = fleet_tile(&world, home_spawn, placed);
            placed = placed.wrapping_add(1);
            let t = ttl(&mut rng);
            world.add_creep(tile, &hauler, t);
        }

        (world, realized.terrain, info)
    }

    /// A minimal `EconScenario` shell carrying the identity fields `run_world` reports (name/seed);
    /// the run parameters come from `RunOptions`. Mirrors [`SteadyScenario::shell`].
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

/// The R corpus: one healthy home + the ADR 0044 remote distance ladder (straddling the
/// break-even).
pub fn remote_catalog(seed: u32) -> Vec<FamilyRScenario> {
    vec![FamilyRScenario::new("E11N1", 6, vec![10, 40, 90, 150, 210, 260], seed)]
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Family M — CONTENDED matching (M4 review #2). The §D3 matching-frontier / §D8 #4 escalation
// question and the M5a CPU budget cannot be ratified on home-room floors (~1.1 edges/pass, a
// FEW carriers): the greedy's myopia only bites when a pass generates MANY candidate edges into
// MANY non-aggregated sinks. Family M builds exactly that pressure — a high-RCL room (many
// extensions, containers, storage) with (a) a large IDLE hauler crowd and (b) many concurrently
// drained NON-aggregated sinks (every container empty, storage below capacity, dropped piles),
// so each pass's edge set is O(carriers × sinks). Run the market arm with the exact oracle ON;
// the pooled gap it measures there is the one §D8 #4 is decided on, and its ops/tick is the real
// M5a budget input (the home-room number is a floor).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One contended-matching scenario: a high-RCL healthy room stripped to a maximal-edge state
/// (drained sinks + a big idle hauler fleet) run a short window under the matching pass.
#[derive(Clone, Debug)]
pub struct ContendedScenario {
    pub name: String,
    pub layout_room: String,
    pub rcl: u8,
    /// The idle hauler crowd size (the "6-12+ carriers" the review asks for).
    pub haulers: u8,
    pub tick_cap: u32,
    pub seed: u32,
}

/// The contended window: short (the matching pass runs every tick; a few hundred ticks samples
/// the oracle many times without a full recovery arc muddying the edge-count signal).
pub const DEFAULT_M_TICK_CAP: u32 = 600;

impl ContendedScenario {
    pub fn new(room: &str, rcl: u8, haulers: u8, seed: u32) -> Self {
        ContendedScenario {
            name: format!("M-{room}-rcl{rcl}-h{haulers}#s{seed}"),
            layout_room: room.to_string(),
            rcl,
            haulers,
            tick_cap: DEFAULT_M_TICK_CAP,
            seed,
        }
    }

    /// Instantiate the maximal-edge world: full realization at `rcl` (so the room has its full
    /// extension/container/storage count), then DRAIN every sink (extensions to 0, containers to
    /// 0, storage to a large-but-below-capacity stock so it is both a big withdraw source AND a
    /// deposit sink), stock the source containers + drop several piles (many withdraw sources),
    /// and seed a big idle hauler crowd + one loaded shuttle per source (the deposit pressure).
    pub fn instantiate(&self) -> (EconWorld, SimTerrain, LayoutInfo) {
        let layouts = captured_layouts();
        let layout: &CapturedLayout = layouts
            .iter()
            .find(|l| l.room == self.layout_room)
            .unwrap_or_else(|| panic!("contended layout `{}` not in the captured cache", self.layout_room));
        let realized = realize(
            layout,
            &RealizeParams { rcl: self.rcl, road_health_pct: 100, seed: self.seed },
        );
        let mut world = realized.world;
        let info = realized.info;

        // Drain the deposit sinks (many concurrent High/Low deposit nodes).
        for s in &mut world.spawns {
            s.store_energy = 0;
        }
        for e in &mut world.extensions {
            e.store_energy = 0;
        }
        // Provider/source containers STOCKED (withdraw sources); every other container drained.
        let roles = info.container_roles.clone();
        for c in world.containers.iter_mut() {
            let tile = (c.pos.x().u8(), c.pos.y().u8());
            // Provider/source containers stocked (withdraw sources); controller + other
            // containers left empty (deposit demand).
            if roles.get(&tile) == Some(&crate::layout::ContainerRole::Source) {
                c.store.add(SimResource::Energy, c.store.capacity);
            }
        }
        // Storage: a big stock, below capacity — a large withdraw source AND a deposit sink.
        if let Some(storage) = world.storage.as_mut() {
            storage.store.add(SimResource::Energy, 300_000);
        }
        // Dropped piles near the spawn: extra withdraw sources (more edges), deterministic tiles.
        let spawn_pos = world.spawns[0].pos;
        let mut rng = Rng::seeded(self.seed.wrapping_mul(7723).wrapping_add(29));
        let mut placed = 0u8;
        for _ in 0..4 {
            let tile = fleet_tile(&world, spawn_pos, placed);
            placed += 1;
            world.drop_resource(tile, SimResource::Energy, rng.range(200, 900));
        }
        // The idle hauler crowd (the carriers): balanced [C,M] bodies, seed-jittered TTLs.
        let hauler = crate::baseline::hauler_body(600).expect("600 ≥ 300");
        for _ in 0..self.haulers {
            let tile = fleet_tile(&world, spawn_pos, placed);
            placed += 1;
            let ttl = rng.range(400, 1400);
            world.add_creep(tile, &hauler, ttl);
        }
        // One LOADED shuttle harvester per source (a full store ⇒ a delivery-edge carrier).
        let capacity = crate::baseline::spawn_lane_capacity(&world);
        let harvester = crate::baseline::harvester_body(capacity.min(800)).expect("≥ 300");
        for _ in 0..world.sources.len() {
            let tile = fleet_tile(&world, spawn_pos, placed);
            placed += 1;
            let id = world.add_creep(tile, &harvester, rng.range(400, 1400));
            let cap = world.creep_stores.get(&id).map(|s| s.capacity).unwrap_or(0);
            if let Some(store) = world.creep_stores.get_mut(&id) {
                store.add(SimResource::Energy, cap);
            }
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

/// The M corpus: the highest-RCL captured rooms (most extensions/containers = most sinks) with a
/// large hauler crowd. `full` widens the room/crowd spread; `fast` takes the first pair.
pub fn contended_catalog(seed: u32, full: bool) -> Vec<ContendedScenario> {
    // The captured foreman rooms realize to RCL 6 max in the corpus; a crowd of 10-16 haulers
    // against a fully-drained RCL-6 sink set generates the many-edge passes.
    let mut out = vec![
        ContendedScenario::new("E11N14", 6, 12, seed),
        ContendedScenario::new("E11N23", 6, 16, seed),
    ];
    if full {
        out.push(ContendedScenario::new("E11N13", 5, 10, seed));
        out.push(ContendedScenario::new("E11N11", 6, 12, seed));
        out.push(ContendedScenario::new("E11N14", 6, 12, seed.wrapping_add(1)));
        out.push(ContendedScenario::new("E11N23", 6, 16, seed.wrapping_add(1)));
    }
    out
}

/// A free walkable tile near the spawn for fleet seeding: ring-scan outward from the spawn,
/// skipping occupied tiles, offset by `n` (deterministic). Radius 8 accommodates the Family-M
/// crowd (16+ haulers); home-room fleets exhaust radius 4 at most.
fn fleet_tile(world: &EconWorld, spawn_pos: Position, n: u8) -> Position {
    let mut count = 0u8;
    for radius in 1..=8i32 {
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

    /// ADR 0044 P2: Family R instantiates a MULTI-ROOM world — a healthy home plus remote sources
    /// at controlled distances over open corridor rooms — and the multi-room `RoverMover` reaches
    /// the farthest remote at ≈ the requested TRUE routed distance (the axis the admission prices).
    #[test]
    fn family_r_instantiates_multiroom_and_remotes_reachable() {
        let sc = FamilyRScenario::new("E11N1", 6, vec![40, 150], 7);
        let (world, _terrain, _info) = sc.instantiate();
        // Corridor rooms populated (home stays default; E12/E13/E14 open) and remotes added.
        assert!(!world.movement.rooms.is_empty(), "corridor rooms present (multi-room)");
        assert!(world.sources.len() >= 2, "home + remote sources");
        assert!(world.storage.as_ref().unwrap().store.amount(SimResource::Energy) >= 200_000, "healthy home");
        // The home room is a known world room but keeps its REAL terrain (walls), not open corridor.
        let home_room = world.spawns[0].pos.room_name();
        assert!(
            !world.movement.rooms.get(&home_room).unwrap().walls.is_empty(),
            "home keeps its realized (walled) terrain, not open corridor"
        );

        // The mover routes home → the farthest remote at the true routed distance (~150 on open).
        let home_spawn = world.spawns[0].pos;
        let far = home_spawn.checked_add((150, 0)).unwrap();
        use crate::movement::Mover;
        let mut m = crate::movement::RoverMover::new(&world.movement);
        let body = screeps_sim_core::SimBody::unboosted(&[Part::Carry, Part::Move]);
        // Every remote is reachable at ≈ its requested true routed distance.
        for &probe in &[40u32, 150] {
            let g = home_spawn.checked_add((probe as i32, 0)).unwrap();
            let d = m.travel_ticks(home_spawn, g, 1, &body, 0).unwrap_or_else(|| panic!("remote +{probe} unreachable"));
            assert!((probe..=probe + 20).contains(&d), "remote +{probe}: true routed distance {d} ≈ {probe}");
        }
        let d = m.travel_ticks(home_spawn, far, 1, &body, 0).expect("remote reachable");
        assert!((150..=170).contains(&d), "true routed distance ≈ 150: {d}");
    }

    /// ADR 0044: the REALISTIC Family R variant uses procedural cave terrain for the corridor, so
    /// the true routed distance to a remote is materially GREATER than the straight-line Chebyshev
    /// — the `routed ≫ Chebyshev` regime the true-distance haul migration exists to price. Also
    /// proves the generated corridor is traversable (remotes reachable through the caves).
    #[test]
    fn family_r_realistic_routed_exceeds_chebyshev() {
        let sc = FamilyRScenario::new("E11N1", 6, vec![0, 0, 0], 11).realistic(); // 3 corridor rooms
        let (world, _t, _info) = sc.instantiate();
        let home_spawn = world.spawns[0].pos;
        let mid = screeps_sim_core::terrain_gen::EXIT_MID;
        let room1 = home_spawn.checked_add((50, 0)).unwrap().room_name();
        let remote1 = Position::new(
            screeps::RoomCoordinate::new(mid).unwrap(),
            screeps::RoomCoordinate::new(mid).unwrap(),
            room1,
        );
        assert!(world.sources.iter().any(|s| s.pos == remote1), "a remote source sits at room-1 centre");

        let _ = remote1;
        // Measure to the FARTHEST remote (3 caves): detours accumulate room-over-room, so the true
        // routed distance materially exceeds the straight-line Chebyshev — the regime step 2 prices.
        let room3 = home_spawn.checked_add((150, 0)).unwrap().room_name();
        let remote3 = Position::new(screeps::RoomCoordinate::new(mid).unwrap(), screeps::RoomCoordinate::new(mid).unwrap(), room3);
        use crate::movement::Mover;
        let mut m = crate::movement::RoverMover::new(&world.movement);
        let body = screeps_sim_core::SimBody::unboosted(&[Part::Carry, Part::Move]);
        let routed = m.travel_ticks(home_spawn, remote3, 1, &body, 0).expect("far remote reachable through the caves");
        let chebyshev = home_spawn.get_range_to(remote3);
        assert!(routed > chebyshev, "realistic terrain: routed {routed} > Chebyshev {chebyshev} (the regime step 2 prices)");
    }

    /// ADR 0044 step 2 (structure-SINK fix): `realize()` folds a refill sink (spawn/extension) into
    /// `terrain.walls`, so its tile is IMPASSABLE. A range-0 distance query to it is UNREACHABLE — the
    /// sim's `market_pass` would then silently fall back to Chebyshev, leaving the DOMINANT refill haul
    /// priced straight-line and defeating the migration on realistic terrain. Range 1 (deliver-adjacent,
    /// exactly what the market uses + where the hauler stands) routes to it. This pins that the sim
    /// prices refill haul on the REAL routed distance.
    #[test]
    fn structure_sink_reachable_at_range_one_not_zero() {
        use crate::movement::Mover;
        let layouts = captured_layouts();
        let layout = layouts.iter().find(|l| l.room == "E11N1").expect("E11N1 in the captured cache");
        let world = realize(layout, &RealizeParams { rcl: 6, road_health_pct: 100, seed: 3 }).world;
        let spawn = world.spawns[0].pos;
        let (sx, sy) = (spawn.x().u8(), spawn.y().u8());
        assert!(
            world.movement.terrain.walls.contains(&(sx, sy)),
            "the refill sink (spawn) tile is an impassable structure wall"
        );
        // Any walkable home tile a few steps away — the pickup end of a refill haul.
        let (ox, oy) = (0..50u8)
            .flat_map(|x| (0..50u8).map(move |y| (x, y)))
            .find(|&(x, y)| {
                !world.movement.terrain.walls.contains(&(x, y))
                    && (x as i32 - sx as i32).abs().max((y as i32 - sy as i32).abs()) >= 5
            })
            .expect("a walkable home tile ≥5 from the spawn");
        let origin = Position::new(
            screeps::RoomCoordinate::new(ox).unwrap(),
            screeps::RoomCoordinate::new(oy).unwrap(),
            spawn.room_name(),
        );
        let mut m = crate::movement::RoverMover::new(&world.movement);
        let body = screeps_sim_core::SimBody::unboosted(&[Part::Carry, Part::Move]);
        assert_eq!(
            m.travel_ticks(origin, spawn, 0, &body, 0),
            None,
            "range 0 onto the wall-tile sink is unreachable — the bug that fell back to Chebyshev"
        );
        assert!(
            m.travel_ticks(origin, spawn, 1, &body, 0).is_some(),
            "range 1 (deliver-adjacent) routes to the structure sink — the fix"
        );
    }

    /// ADR 0044 (operator correctness check): every OPEN room-edge tile in the realistic Family R
    /// world has a WALKABLE mirror in its neighbouring room — the engine's exit-alignment invariant
    /// the kernel's wall-blind edge relocation (`tick.rs:53-76`) relies on. A mismatched seam would
    /// drop a creep onto a wall. (Neighbours outside the known world are skipped — no crossing there.)
    #[test]
    fn family_r_realistic_exit_seams_are_walkable_both_sides() {
        let sc = FamilyRScenario::new("E11N1", 6, vec![0, 0, 0], 11).realistic();
        let (world, _t, _info) = sc.instantiate();
        let rooms = &world.movement.rooms;
        let mut violations: Vec<String> = Vec::new();
        for (&room, terrain) in rooms {
            for i in 0..50u8 {
                for &(x, y) in &[(0u8, i), (49, i), (i, 0), (i, 49)] {
                    if terrain.walls.contains(&(x, y)) {
                        continue; // walled edge — the kernel attempts no crossing here
                    }
                    // The kernel's relocation offset (its priority order: x==0, y==0, x==49, y==49).
                    let off = if x == 0 { (-1, 0) } else if y == 0 { (0, -1) } else if x == 49 { (1, 0) } else { (0, 1) };
                    let here = Position::new(screeps::RoomCoordinate::new(x).unwrap(), screeps::RoomCoordinate::new(y).unwrap(), room);
                    if let Ok(mirror) = here.checked_add(off) {
                        if let Some(nbr) = rooms.get(&mirror.room_name()) {
                            if nbr.walls.contains(&(mirror.x().u8(), mirror.y().u8())) {
                                violations.push(format!("{}({x},{y})→wall", room));
                            }
                        }
                    }
                }
            }
        }
        assert!(violations.is_empty(), "open exit seams with a wall on the far side: {violations:?}");
    }

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
