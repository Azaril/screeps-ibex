//! **Family C — collapse** (ADR 0040 §D7; M1 spec Part C.4): a captured foreman layout realized
//! as of RCL R, spawns/extensions drained to 0, storage at S0, creeps wiped or a skeleton, roads
//! pre-decayed (the BAIT axis) — run until recovered or the tick cap. Every bait scenario has an
//! IDENTICAL no-bait control (roads at 100%, same seed ⇒ same decay phases/skeleton) so the
//! repro gate's paired diff isolates exactly the repair-bait axis.
//!
//! The downgrade clock is scenario STATE only until M2 (set + reported, never ticked — M2 owns
//! controller mechanics; documented per the spec).

use crate::layout::{controller_downgrade_full, realize, LayoutInfo, RealizeParams};
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
