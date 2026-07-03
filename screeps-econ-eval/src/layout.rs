//! Realize a captured foreman layout ([`CapturedLayout`]) **as of RCL R** into an
//! [`EconWorld`] + the pathing [`SimTerrain`] (ADR 0040 M1 spec Part C.1).
//!
//! **Inclusion rule:** a planned structure exists at RCL `R` iff its captured `required_rcl ≤ R`.
//! The regenerated cache (capture_layout, 2026-07-03) carries an explicit value for EVERY
//! structure — observed semantics from the data: `0` = pre-RCL furniture (source containers and
//! their access roads — present from the start), `1..=8` = the plan's build schedule (extension
//! counts match the engine allowance 5/10/20/30/40/50/60 exactly; storage at 4; spawns 1/7/8),
//! `9` = beyond-RCL8 overflow placements (never realized). A `None` (only possible from a
//! pre-extension cache) falls back per the M1 spec: roads/containers present, everything else
//! RCL-8-only.
//!
//! **Blocking:** every included structure that blocks movement (not road/container/rampart/
//! extractor — the [`base_traffic`] rule) is a WALL in the pathing terrain, so the analytic
//! movement tier, the engine's birth-tile scan, and the oracle all price the same world.
//! Excluded-at-R structures simply don't exist (no site, no wall).
//!
//! **Roads:** hitsMax from the tile's NATURAL terrain (plain 5000 / swamp 25000 —
//! engine-mechanics.md:430), hits = `road_health_pct`% of max, decay clocks phase-jittered by the
//! scenario seed (so N-seed paired runs genuinely differ) — the SAME phase jitter applies to bait
//! and control (only the health axis differs, the paired-diff contract).

use screeps::{Position, RoomCoordinate, RoomName};
use screeps_combat_eval::harness::terrain_import::decode_terrain;
use screeps_econ_engine::constants::{road_hits_max, CONTAINER_HITS, ROAD_DECAY_TIME};
use screeps_econ_engine::EconWorld;
use screeps_rover_eval::base_traffic::{CapturedLayout, PlannedStructure};
use screeps_sim_core::rng::Rng;
use screeps_sim_core::SimTerrain;
use std::collections::BTreeMap;

/// Container general-store capacity — `CONTAINER_CAPACITY` 2000 (engine `common/constants.js:341`,
/// the sibling row of the engine-mechanics.md:429 container entry).
pub const CONTAINER_CAPACITY: u32 = 2000;
/// Storage general-store capacity — `STORAGE_CAPACITY` 1,000,000 (engine `common/constants.js`).
pub const STORAGE_CAPACITY: u32 = 1_000_000;
/// Source pool in an OWNED room — 3000 (engine-mechanics.md:466; Family C rooms are owned).
pub const SOURCE_CAPACITY: u32 = screeps_econ_engine::constants::SOURCE_CAPACITY_OWNED;

/// The engine's `CONTROLLER_DOWNGRADE` full-clock table, RCL 1..=8
/// (engine-mechanics.md:228, `common/constants.js:232`). Scenario STATE only until M2 — the M1
/// sim sets and reports the clock, it does not tick it (M1 spec Part C.4 note).
pub fn controller_downgrade_full(rcl: u8) -> u32 {
    match rcl {
        1 => 20_000,
        2 => 10_000,
        3 => 20_000,
        4 => 40_000,
        5 => 80_000,
        6 => 120_000,
        7 => 150_000,
        8 => 200_000,
        _ => 0,
    }
}

/// Does a planned structure of `kind` block movement? The [`screeps_rover_eval::base_traffic`]
/// rule, mirrored (roads/containers walkable, own ramparts walkable, extractor sits on the
/// mineral).
fn blocks_movement(kind: &str) -> bool {
    !matches!(kind, "road" | "container" | "rampart" | "extractor")
}

/// The as-of-RCL inclusion rule (module docs): explicit `required_rcl ≤ R`; `None` (legacy cache
/// only) ⇒ roads/containers present, everything else RCL-8-only.
fn included_at(s: &PlannedStructure, rcl: u8) -> bool {
    match s.required_rcl {
        Some(r) => r <= rcl,
        None => matches!(s.kind.as_str(), "road" | "container") || rcl >= 8,
    }
}

/// Which role a container plays in the layout — decides its demand registration (K1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerRole {
    /// A harvest container (nearest container within Chebyshev 2 of a source —
    /// `energy_traffic_fleet`'s rule): withdraw-side provider.
    Source,
    /// The controller's supply container (nearest within Chebyshev 2 of the controller): the
    /// Low-priority deposit — Family C's diversion-bait sink (room_transfer.rs:342-367).
    Controller,
    /// Anything else (e.g. the mineral container at RCL ≥ 6): the generic accepts-all arm
    /// (room_transfer.rs:394-421).
    Other,
}

/// Static layout facts the policy needs every tick, keyed by tile (containers/roads may DIE, so
/// position — not index — is the stable identity).
#[derive(Clone, Debug)]
pub struct LayoutInfo {
    pub room: RoomName,
    pub controller_pos: Position,
    /// Container tile → role (positions of containers that EXIST at realization).
    pub container_roles: BTreeMap<(u8, u8), ContainerRole>,
    /// Source index → its harvest-container tile (if one is realized).
    pub source_containers: BTreeMap<usize, (u8, u8)>,
}

/// Realization parameters (the scenario axes that shape the world itself).
#[derive(Clone, Copy, Debug)]
pub struct RealizeParams {
    pub rcl: u8,
    /// Road starting health, percent of hitsMax (bait: 30/60; control: 100).
    pub road_health_pct: u32,
    /// Seed for the decay-clock phase jitter (per-structure, deterministic).
    pub seed: u32,
}

/// The realized world + the pathing terrain + the layout facts.
pub struct Realized {
    pub world: EconWorld,
    /// The pathing/movement terrain (also installed as `world.movement.terrain`).
    pub terrain: SimTerrain,
    pub info: LayoutInfo,
}

fn pos_in(room: RoomName, x: u8, y: u8) -> Position {
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
}

/// Realize `layout` as of `params.rcl`. Spawns are born FULL (the M0 builder contract) and
/// extensions empty — the COLLAPSE drain (spawns/extensions to 0, storage S0) is the scenario's
/// job ([`crate::scenario::instantiate`]), not realization's.
pub fn realize(layout: &CapturedLayout, params: &RealizeParams) -> Realized {
    let room: RoomName = layout.room.parse().expect("captured layout room name parses");
    let natural = decode_terrain(&layout.terrain);
    let mut terrain = natural.clone();

    let included: Vec<&PlannedStructure> =
        layout.structures.iter().filter(|s| included_at(s, params.rcl)).collect();

    // Blocking structures become pathing walls (module docs).
    for s in &included {
        if blocks_movement(&s.kind) {
            terrain.walls.insert((s.x, s.y));
        }
    }

    let mut world = EconWorld::default();
    world.movement.terrain = terrain.clone();

    // Controller: level = R; the downgrade clock is set by the scenario (state-only until M2).
    world.controller = Some(screeps_econ_engine::SimController {
        level: params.rcl,
        progress: 0,
        downgrade_ticks: controller_downgrade_full(params.rcl),
    });

    for &(x, y) in &layout.sources {
        world.add_source(pos_in(room, x, y), SOURCE_CAPACITY);
    }
    if let Some((x, y)) = layout.mineral {
        world.add_mineral(pos_in(room, x, y), 0, 0); // furniture until M6
    }

    let mut rng = Rng::seeded(params.seed);
    // Structures in the captured (sorted) order — construction order is part of world identity,
    // and the per-structure jitter stream is consumed in this same deterministic order.
    for s in &included {
        let p = pos_in(room, s.x, s.y);
        match s.kind.as_str() {
            "spawn" => {
                world.add_spawn(p);
            }
            "extension" => {
                world.add_extension(p, params.rcl);
            }
            "storage" => {
                // Belt-and-braces on top of the plan's schedule: storage exists at RCL ≥ 4
                // (engine `CONTROLLER_STRUCTURES`; the captured schedule already says 4).
                if params.rcl >= 4 {
                    world.set_storage(p, STORAGE_CAPACITY);
                }
            }
            "container" => {
                let i = world.add_container(p, CONTAINER_CAPACITY, CONTAINER_HITS);
                // Phase jitter: the decay clock starts anywhere in its window (both arms).
                let window = world.container_decay_window();
                world.containers[i].next_decay_at = rng.range(1, window);
            }
            "road" => {
                let swamp = natural.swamps.contains(&(s.x, s.y));
                let max = road_hits_max(swamp);
                let hits = ((max as u64 * params.road_health_pct as u64) / 100).max(1) as u32;
                let i = world.add_road(p, hits.min(max), max);
                world.roads[i].next_decay_at = rng.range(1, ROAD_DECAY_TIME);
            }
            // Towers/labs/links/walls/… are movement-blocking furniture only in M1 (their
            // mechanics arrive M2/M6); the wall insertion above is their whole existence.
            _ => {}
        }
    }

    // Layout facts: container roles by the Chebyshev-2 nearest rule (module docs).
    let chebyshev = |a: (u8, u8), b: (u8, u8)| (a.0.abs_diff(b.0) as u32).max(a.1.abs_diff(b.1) as u32);
    let mut container_roles: BTreeMap<(u8, u8), ContainerRole> = BTreeMap::new();
    let mut source_containers: BTreeMap<usize, (u8, u8)> = BTreeMap::new();
    let container_tiles: Vec<(u8, u8)> =
        world.containers.iter().map(|c| (c.pos.x().u8(), c.pos.y().u8())).collect();
    for t in &container_tiles {
        container_roles.insert(*t, ContainerRole::Other);
    }
    for (i, &(sx, sy)) in layout.sources.iter().enumerate() {
        if let Some(&c) = container_tiles
            .iter()
            .filter(|&&c| chebyshev(c, (sx, sy)) <= 2)
            .min_by_key(|&&c| (chebyshev(c, (sx, sy)), c.1, c.0))
        {
            container_roles.insert(c, ContainerRole::Source);
            source_containers.insert(i, c);
        }
    }
    if let Some(&c) = container_tiles
        .iter()
        .filter(|&&c| chebyshev(c, layout.controller) <= 2)
        .filter(|&&c| container_roles.get(&c) != Some(&ContainerRole::Source))
        .min_by_key(|&&c| (chebyshev(c, layout.controller), c.1, c.0))
    {
        container_roles.insert(c, ContainerRole::Controller);
    }

    // The returned pathing terrain is the WORLD's movement terrain (walls + the as-of-RCL roads
    // `add_road` registered) — one source of truth for the mover, the oracle, and the engine.
    let terrain = world.movement.terrain.clone();
    Realized {
        world,
        terrain,
        info: LayoutInfo {
            room,
            controller_pos: pos_in(room, layout.controller.0, layout.controller.1),
            container_roles,
            source_containers,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_rover_eval::base_traffic::captured_layouts;

    /// The regenerated cache carries an explicit `required_rcl` on EVERY structure, and the
    /// extension schedule matches the engine allowance table (5/10/20/30/40/50/60 for RCL 2..8 —
    /// `CONTROLLER_STRUCTURES`) on every captured room — the data the as-of-RCL realization
    /// trusts, pinned.
    #[test]
    fn captured_schedule_is_explicit_and_engine_shaped() {
        let layouts = captured_layouts();
        assert_eq!(layouts.len(), 13, "the 13-room corpus");
        let allowance = [0, 0, 5, 10, 20, 30, 40, 50, 60];
        for l in &layouts {
            for s in &l.structures {
                assert!(s.required_rcl.is_some(), "{}: {} at ({},{}) has explicit rcl", l.room, s.kind, s.x, s.y);
            }
            for rcl in 2..=8u8 {
                let n = l
                    .structures
                    .iter()
                    .filter(|s| s.kind == "extension" && included_at(s, rcl))
                    .count();
                assert_eq!(
                    n, allowance[rcl as usize],
                    "{}: extension count at RCL {rcl} matches the engine allowance",
                    l.room
                );
            }
            let storage_rcl = l
                .structures
                .iter()
                .find(|s| s.kind == "storage")
                .and_then(|s| s.required_rcl)
                .expect("every captured plan has storage");
            assert_eq!(storage_rcl, 4, "{}: storage schedules at RCL 4", l.room);
        }
    }

    /// Realization respects the schedule: RCL 3 has 10 extensions + no storage; RCL 4 has 20 +
    /// storage; blocking structures become pathing walls; roads carry health + terrain-scaled
    /// hitsMax; source/controller containers get their roles.
    #[test]
    fn realize_as_of_rcl() {
        let layouts = captured_layouts();
        let l = &layouts[0]; // E11N1
        let at = |rcl: u8, pct: u32| realize(l, &RealizeParams { rcl, road_health_pct: pct, seed: 7 });

        let r3 = at(3, 30);
        assert_eq!(r3.world.extensions.len(), 10, "RCL 3 → 10 extensions");
        assert!(r3.world.storage.is_none(), "no storage before RCL 4");
        assert_eq!(r3.world.controller.as_ref().unwrap().level, 3);
        assert!(!r3.world.spawns.is_empty(), "the RCL-1 spawn exists");
        assert_eq!(r3.world.sources.len(), l.sources.len());

        let r4 = at(4, 30);
        assert_eq!(r4.world.extensions.len(), 20, "RCL 4 → 20 extensions");
        assert!(r4.world.storage.is_some(), "storage at RCL 4");
        assert!(
            r4.world.roads.len() >= r3.world.roads.len(),
            "the road network grows with the schedule"
        );

        // Blocking structures price as walls; roads and containers never do.
        let spawn_tile = (r4.world.spawns[0].pos.x().u8(), r4.world.spawns[0].pos.y().u8());
        assert!(r4.terrain.walls.contains(&spawn_tile), "spawns block movement");
        for road in &r4.world.roads {
            let t = (road.pos.x().u8(), road.pos.y().u8());
            assert!(!r4.terrain.walls.contains(&t), "roads stay walkable");
            assert!(r4.terrain.roads.contains(&t), "roads price fatigue 1");
            assert_eq!(road.hits, road.hits_max * 30 / 100, "30% health");
        }

        // An RCL-8 world includes at most the engine allowance of extensions (rcl-9 overflow
        // placements stay unrealized).
        let r8 = at(8, 100);
        assert_eq!(r8.world.extensions.len(), 60, "RCL 8 → 60 extensions (rcl-9 extras excluded)");
        for road in &r8.world.roads {
            assert_eq!(road.hits, road.hits_max, "control arm: full-health roads");
        }

        // Roles: every source with a planned harvest container is mapped; the controller
        // container is distinct from source containers.
        assert!(!r4.info.source_containers.is_empty(), "source containers mapped");
        let controller_containers = r4
            .info
            .container_roles
            .values()
            .filter(|&&r| r == ContainerRole::Controller)
            .count();
        assert!(controller_containers <= 1, "at most one controller container");
    }

    /// The seed jitters ONLY decay phases (bait axis unchanged): same seed ⇒ identical worlds;
    /// different seeds ⇒ different decay clocks but identical structure sets + hits.
    #[test]
    fn seed_jitters_phases_only() {
        let layouts = captured_layouts();
        let l = &layouts[1];
        let a = realize(l, &RealizeParams { rcl: 4, road_health_pct: 30, seed: 1 });
        let b = realize(l, &RealizeParams { rcl: 4, road_health_pct: 30, seed: 1 });
        assert_eq!(a.world.state_digest(), b.world.state_digest(), "same seed ⇒ same world");
        let c = realize(l, &RealizeParams { rcl: 4, road_health_pct: 30, seed: 2 });
        assert_ne!(a.world.state_digest(), c.world.state_digest(), "seed moves the decay phases");
        assert_eq!(a.world.roads.len(), c.world.roads.len());
        for (ra, rc) in a.world.roads.iter().zip(&c.world.roads) {
            assert_eq!((ra.pos, ra.hits, ra.hits_max), (rc.pos, rc.hits, rc.hits_max), "only phases differ");
        }
    }
}
