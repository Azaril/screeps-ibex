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
use screeps_econ_engine::{EconWorld, SimResource};
use screeps_rover_eval::base_traffic::{CapturedLayout, PlannedStructure};
use screeps_sim_core::rng::Rng;
use screeps_sim_core::SimTerrain;
use std::collections::BTreeMap;

/// Container general-store capacity — re-exported from the engine crate's citation-pinned
/// definition since M2 (`common/constants.js:341`; build completion needs it engine-side).
pub const CONTAINER_CAPACITY: u32 = screeps_econ_engine::constants::CONTAINER_CAPACITY;
/// Storage general-store capacity — re-exported from the engine crate since M2.
pub const STORAGE_CAPACITY: u32 = screeps_econ_engine::constants::STORAGE_CAPACITY;
/// Source pool in an OWNED room — 3000 (engine-mechanics.md:466; Family C rooms are owned).
pub const SOURCE_CAPACITY: u32 = screeps_econ_engine::constants::SOURCE_CAPACITY_OWNED;

/// The engine's `CONTROLLER_DOWNGRADE` full-clock table, RCL 1..=8 — delegates to the engine
/// crate's citation-pinned table since M2 (the clock now TICKS; scenarios rescale it).
pub fn controller_downgrade_full(rcl: u8) -> u32 {
    screeps_econ_engine::constants::controller_downgrade(rcl)
}

/// A deterministic base-mineral type + starting density for a room (M6): the captured layouts
/// record the mineral TILE but not its type/density, so derive both from the room name — stable
/// per room, no ambient entropy. The type cycles the seven base ores; the density starts MODERATE
/// (tier 2) so the pool is neither trivially small nor maximal. The mineral-economy family cares
/// only that it is a well-typed pool an extractor can mine.
pub fn mineral_type_and_density(room: &str) -> (SimResource, u8) {
    let ores = [
        SimResource::Hydrogen,
        SimResource::Oxygen,
        SimResource::Utrium,
        SimResource::Lemergium,
        SimResource::Keanium,
        SimResource::Zynthium,
        SimResource::Ghodium,
    ];
    let h = room.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    (ores[(h as usize) % ores.len()], screeps_econ_engine::constants::DENSITY_MODERATE)
}

/// A deterministic per-mineral re-roll seed from the room + scenario seed (M6): the density
/// re-roll draws `Rng::seeded(reroll_seed)`, so this must be reproducible (no ambient entropy) but
/// vary per (room, seed) so N-seed runs differ.
pub fn mineral_reroll_seed(room: &str, seed: u32) -> u32 {
    let h = room.bytes().fold(0u32, |a, b| a.wrapping_mul(2166136261).wrapping_add(b as u32));
    h.wrapping_mul(16777619).wrapping_add(seed.wrapping_mul(2654435761))
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

/// One PLANNED structure in the sim's build vocabulary (M2) — the construction pass's schedule:
/// place a site once `required_rcl ≤ level` and nothing stands there (the live
/// `ConstructionMission` per-50-tick pass over the foreman plan, construction.rs:445-479).
#[derive(Clone, Copy, Debug)]
pub struct PlanStructure {
    pub kind: screeps_econ_engine::StructureKind,
    pub x: u8,
    pub y: u8,
    pub required_rcl: u8,
}

/// Static layout facts the policy needs every tick, keyed by tile (containers/roads may DIE, so
/// position — not index — is the stable identity).
#[derive(Clone, Debug)]
pub struct LayoutInfo {
    pub room: RoomName,
    pub controller_pos: Position,
    /// Container tile → role. Since M2 this is derived from the PLAN's container tiles (not the
    /// realized set) so a container BUILT mid-run gets its role the tick it materializes —
    /// identical tiles for realized containers, plus the not-yet-built ones.
    pub container_roles: BTreeMap<(u8, u8), ContainerRole>,
    /// Source index → its planned harvest-container tile.
    pub source_containers: BTreeMap<usize, (u8, u8)>,
    /// The plan's build schedule, in-vocabulary kinds only (spawn/extension/road/container/
    /// storage/tower), capture order (M2 — the construction pass's input). Out-of-vocabulary
    /// kinds (link/lab/terminal/walls/ramparts/…) are realization-only furniture, never placed
    /// as sites (documented reduction).
    pub plan_structures: Vec<PlanStructure>,
    /// Review B4: tiles of REALIZED out-of-vocabulary furniture (labs/links/terminal/ramparts/…
    /// — everything the live anchor scan counts EXCEPT constructedWall, construction.rs:319-324).
    /// They anchor the construction pass's road-adjacency rule exactly as modeled structures do.
    /// Empty for greenfield (nothing realized).
    pub furniture_tiles: Vec<(u8, u8)>,
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

    // Controller: level = R at the captured controller tile; the downgrade clock starts full
    // (the scenario rescales it — and since M2 it TICKS).
    world.set_controller(pos_in(room, layout.controller.0, layout.controller.1), params.rcl);

    for &(x, y) in &layout.sources {
        world.add_source(pos_in(room, x, y), SOURCE_CAPACITY);
    }
    if let Some((x, y)) = layout.mineral {
        // The captured layout carries no mineral TYPE/density (only the tile), so assign a
        // deterministic type + starting density from the room name (stable per room, no ambient
        // entropy). Families C/G/S/D realize the mineral as FURNITURE (no extractor/labs) — the
        // density is inert there; the M6 mineral-economy family adds the extractor + labs
        // ([`realize_mineral_economy`]).
        let (res, density) = mineral_type_and_density(&layout.room);
        world.add_mineral(pos_in(room, x, y), res, density, mineral_reroll_seed(&layout.room, params.seed));
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
            // Towers materialize as engine STUBS (M2 — build vocabulary parity: the construction
            // pass must see the planned tower as EXISTING, or it would re-place a site over the
            // pathing wall). Labs/links/terminal/walls/… stay movement-blocking furniture only.
            "tower" => {
                world.add_tower(p);
            }
            _ => {}
        }
    }

    // Review B4: realized out-of-vocab furniture — anchor candidates for the construction
    // pass's road-adjacency rule (constructedWall excluded like the live scan,
    // construction.rs:321; the captured plans name it "wall").
    let furniture: Vec<(u8, u8)> = included
        .iter()
        .filter(|s| vocab_kind(&s.kind).is_none() && s.kind != "wall" && s.kind != "constructedWall")
        .map(|s| (s.x, s.y))
        .collect();
    let info = layout_info(layout, room, params.rcl, furniture);

    // The returned pathing terrain is the WORLD's movement terrain (walls + the as-of-RCL roads
    // `add_road` registered) — one source of truth for the mover, the oracle, and the engine.
    let terrain = world.movement.terrain.clone();
    Realized { world, terrain, info }
}

/// The in-vocabulary kind for a captured plan kind string (None = out of the M2 build vocabulary).
fn vocab_kind(kind: &str) -> Option<screeps_econ_engine::StructureKind> {
    use screeps_econ_engine::StructureKind::*;
    Some(match kind {
        "spawn" => Spawn,
        "extension" => Extension,
        "road" => Road,
        "container" => Container,
        "storage" => Storage,
        "tower" => Tower,
        _ => return None,
    })
}

/// Layout facts from the PLAN (not the realized set — M2: containers/roads built mid-run inherit
/// their roles; the construction pass reads the schedule). `_rcl` reserved for future
/// role-by-realization variants; `furniture_tiles` = the caller's REALIZED out-of-vocab set
/// (review B4 — greenfield passes empty).
pub fn layout_info(layout: &CapturedLayout, room: RoomName, _rcl: u8, furniture_tiles: Vec<(u8, u8)>) -> LayoutInfo {
    // The plan's build schedule, in-vocabulary + realizable (required_rcl ≤ 8; the captured `9`
    // overflow placements never realize — module docs).
    let plan_structures: Vec<PlanStructure> = layout
        .structures
        .iter()
        .filter_map(|s| {
            let kind = vocab_kind(&s.kind)?;
            let required_rcl = s.required_rcl?;
            (required_rcl <= 8).then_some(PlanStructure { kind, x: s.x, y: s.y, required_rcl })
        })
        .collect();

    // Container roles by the Chebyshev-2 nearest rule over PLAN container tiles (module docs).
    let chebyshev = |a: (u8, u8), b: (u8, u8)| (a.0.abs_diff(b.0) as u32).max(a.1.abs_diff(b.1) as u32);
    let mut container_roles: BTreeMap<(u8, u8), ContainerRole> = BTreeMap::new();
    let mut source_containers: BTreeMap<usize, (u8, u8)> = BTreeMap::new();
    let container_tiles: Vec<(u8, u8)> = plan_structures
        .iter()
        .filter(|s| s.kind == screeps_econ_engine::StructureKind::Container)
        .map(|s| (s.x, s.y))
        .collect();
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

    LayoutInfo {
        room,
        controller_pos: pos_in(room, layout.controller.0, layout.controller.1),
        container_roles,
        source_containers,
        plan_structures,
        furniture_tiles,
    }
}

/// **Family G greenfield realization (M2):** virgin room as of RCL 1 — natural terrain, the
/// plan's ANCHOR spawn only (required_rcl ≤ 1), full sources, controller level 1 with a full
/// clock, NO other structures and NO sites (the construction pass builds the plan per
/// `required_rcl` as the rush levels up). The spawn is born FULL (300 — the respawn/new-room
/// convention; the M0 builder default) — documented choice: the closest live analog to "rush
/// from a fresh spawn" seeds the spawn charged, and the T*_RCL oracle uses the same E0.
pub fn realize_greenfield(layout: &CapturedLayout) -> Realized {
    let room: RoomName = layout.room.parse().expect("captured layout room name parses");
    let natural = decode_terrain(&layout.terrain);

    let mut world = EconWorld::default();
    world.movement.terrain = natural.clone();
    world.set_controller(pos_in(room, layout.controller.0, layout.controller.1), 1);

    for &(x, y) in &layout.sources {
        world.add_source(pos_in(room, x, y), SOURCE_CAPACITY);
    }
    if let Some((x, y)) = layout.mineral {
        let (res, density) = mineral_type_and_density(&layout.room);
        world.add_mineral(pos_in(room, x, y), res, density, mineral_reroll_seed(&layout.room, 0));
    }

    let anchor = layout
        .structures
        .iter()
        .find(|s| s.kind == "spawn" && s.required_rcl.is_some_and(|r| r <= 1))
        .expect("every captured plan has an RCL-1 anchor spawn");
    world.add_spawn(pos_in(room, anchor.x, anchor.y));
    world.movement.terrain.walls.insert((anchor.x, anchor.y)); // the spawn blocks pathing

    let info = layout_info(layout, room, 1, Vec::new()); // greenfield: no furniture exists
    let terrain = world.movement.terrain.clone();
    Realized { world, terrain, info }
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
