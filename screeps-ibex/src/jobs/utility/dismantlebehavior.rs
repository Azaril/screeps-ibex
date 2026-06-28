use super::dismantle::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::utility::movebehavior::mark_working;
use crate::pathing::pathfinderservice::PathfinderService;
use crate::room::data::*;
use crate::structureidentifier::*;
use screeps_rover::reaches_room_edge;
use screeps::*;
use screeps_foreman::terrain::FastRoomTerrain;
use screeps_rover::*;
use std::collections::{HashMap, HashSet};

/// Structures a creep can stand on / walk through: never breach blockers.
fn structure_is_walkable(structure: &StructureObject) -> bool {
    match structure {
        StructureObject::StructureRoad(_) | StructureObject::StructureContainer(_) | StructureObject::StructureExtractor(_) => true,
        StructureObject::StructureRampart(rampart) => rampart.my() || rampart.is_public(),
        _ => false,
    }
}

/// Movement-blocking structures aggregated per tile for the breach search:
/// stacked blockers (rampart over extension) sum their hits — passing the
/// tile means clearing all of them — and any blocker we will never dismantle
/// (engine-undismantlable, or past the hit-pool horizon) pins the tile
/// impassable so the corridor routes around it.
pub(crate) fn breach_blockers(structures: &[StructureObject], max_structure_hits: u32) -> HashMap<(u8, u8), BreachBlocker> {
    let mut result: HashMap<(u8, u8), BreachBlocker> = HashMap::new();

    for structure in structures {
        if structure_is_walkable(structure) {
            continue;
        }

        let pos = structure.pos();
        let tile = (pos.x().u8(), pos.y().u8());

        let blocker = if can_dismantle(structure) && within_dismantle_hits_horizon(structure, max_structure_hits) {
            BreachBlocker::Dismantlable(structure.as_attackable().map(|a| a.hits()).unwrap_or(0))
        } else {
            BreachBlocker::Impassable
        };

        let merged = match (result.get(&tile), &blocker) {
            (None, _) => blocker,
            (Some(BreachBlocker::Dismantlable(existing)), BreachBlocker::Dismantlable(new)) => {
                BreachBlocker::Dismantlable(existing.saturating_add(*new))
            }
            _ => BreachBlocker::Impassable,
        };

        result.insert(tile, merged);
    }

    result
}

/// Whether the room's controller can be physically reached, and if not,
/// whether dismantling could open a path. Drives the salvage de-claim gate:
/// CLAIM creeps are only worth spawning once a creep can actually walk to the
/// controller — otherwise they die en route while the path is still walled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerAccess {
    /// A creep can walk from a room edge to range 1 of the controller right
    /// now — no dismantling required.
    ReachableNow,
    /// Blocked today, but every blocker sealing it off is dismantlable within
    /// the hit horizon — dismantlers can open a corridor (M10 prioritizes the
    /// controller path), so de-claim is worth keeping alive (just not spawning
    /// yet).
    Breachable,
    /// No path even through dismantlable blockers (sealed by terrain or by
    /// structures past the hit horizon) — de-claim is impossible here.
    Sealed,
}

/// Classify how reachable a TILE (a controller, a source, …) is, using the
/// pathfinding system's flood-to-edge primitive twice: once treating ALL
/// non-walkable structures as blocking (reachable now?), once treating only
/// the un-clearable ones (terrain walls + engine-undismantlable + over-horizon)
/// as blocking (breachable?). Needs live terrain; returns `Breachable` (the
/// safe "wait, don't give up" verdict) if the room is not visible.
pub fn position_access(room: RoomName, structures: &[StructureObject], target_pos: Position, max_structure_hits: u32) -> ControllerAccess {
    let Some(room_obj) = game::rooms().get(room) else {
        return ControllerAccess::Breachable;
    };

    let terrain = FastRoomTerrain::new(room_obj.get_terrain().get_raw_buffer().to_vec());

    let mut blocked_now: HashSet<(u8, u8)> = HashSet::new();
    let mut blocked_unclearable: HashSet<(u8, u8)> = HashSet::new();

    for structure in structures {
        if structure_is_walkable(structure) {
            continue;
        }

        let pos = structure.pos();
        let tile = (pos.x().u8(), pos.y().u8());
        blocked_now.insert(tile);

        let clearable = can_dismantle(structure) && within_dismantle_hits_horizon(structure, max_structure_hits);
        if !clearable {
            blocked_unclearable.insert(tile);
        }
    }

    let start = (target_pos.x().u8(), target_pos.y().u8());

    let passable_now = |x: u8, y: u8| !terrain.is_wall(x, y) && !blocked_now.contains(&(x, y));
    if reaches_room_edge(&passable_now, start) {
        return ControllerAccess::ReachableNow;
    }

    let passable_breach = |x: u8, y: u8| !terrain.is_wall(x, y) && !blocked_unclearable.contains(&(x, y));
    if reaches_room_edge(&passable_breach, start) {
        ControllerAccess::Breachable
    } else {
        ControllerAccess::Sealed
    }
}

/// True if ANY of `objectives` (controller, sources, …) is sealed at the
/// normal dismantle horizon but reachable when it is ignored — i.e. blocked
/// only by over-horizon enemy walls/ramparts we could chew to open access.
/// One terrain fetch covers all objectives. Drives the salvage breach
/// decision for both de-claim (controller) and mining (sources).
pub fn objectives_need_breach(room: RoomName, structures: &[StructureObject], objectives: &[Position], max_structure_hits: u32) -> bool {
    if objectives.is_empty() {
        return false;
    }

    let Some(room_obj) = game::rooms().get(room) else {
        return false;
    };

    let terrain = FastRoomTerrain::new(room_obj.get_terrain().get_raw_buffer().to_vec());

    let mut blocked_now: HashSet<(u8, u8)> = HashSet::new();
    let mut blocked_unclearable: HashSet<(u8, u8)> = HashSet::new();

    for structure in structures {
        if structure_is_walkable(structure) {
            continue;
        }
        let pos = structure.pos();
        let tile = (pos.x().u8(), pos.y().u8());
        blocked_now.insert(tile);
        if !(can_dismantle(structure) && within_dismantle_hits_horizon(structure, max_structure_hits)) {
            blocked_unclearable.insert(tile);
        }
    }

    let passable_now = |x: u8, y: u8| !terrain.is_wall(x, y) && !blocked_now.contains(&(x, y));
    let passable_breach = |x: u8, y: u8| !terrain.is_wall(x, y) && !blocked_unclearable.contains(&(x, y));

    objectives.iter().any(|obj| {
        let start = (obj.x().u8(), obj.y().u8());
        // Sealed now (can't walk to it) but breachable (reachable if the
        // over-horizon walls in the way are treated as dismantlable).
        !reaches_room_edge(&passable_now, start) && reaches_room_edge(&passable_breach, start)
    })
}

/// The breach corridor's representative TARGET tile + its total dismantle cost,
/// for the v1 `Dismantle{room, pos}` objective the salvage breach producer emits
/// (ADR 0027 v1.1 P1). Plans the cheapest dismantle corridor from EACH objective
/// (controller + every source) OUT to the nearest room edge with
/// [`breach_path_blockers_to_edge`] over the live terrain — the SAME Dijkstra
/// pricing the `DismantleJob` corridor uses, but anchored at the walled-in
/// objective (the producer has no creep position). It returns the OUTERMOST
/// blocker tile (closest to the edge — the first seal an incoming squad reaches)
/// of the CHEAPEST corridor, as a [`Position`], together with the corridor's
/// total hits (the SiegeBreach sizing input). That blocker is the squad's single
/// `Dismantle` target tile; the SquadManager fields a WORK squad that razes it,
/// opening the corridor one blocker at a time as the M10 priority logic did for
/// the solo dismantler. `None` when there is no breachable objective (every
/// objective already reaches an edge, all are sealed past the horizon, no
/// objectives, or no live terrain) — the producer then emits nothing.
///
/// Needs live terrain (mirrors [`objectives_need_breach`] / [`position_access`]);
/// returns `None` if the room is not visible.
pub fn breach_target_tile(
    room: RoomName,
    structures: &[StructureObject],
    objectives: &[Position],
    max_structure_hits: u32,
) -> Option<(Position, u32)> {
    let room_obj = game::rooms().get(room)?;
    let terrain = FastRoomTerrain::new(room_obj.get_terrain().get_raw_buffer().to_vec());
    let is_wall = |x: u8, y: u8| terrain.is_wall(x, y);

    let blockers = breach_blockers(structures, max_structure_hits);

    let mut best: Option<((u8, u8), u32)> = None;
    for obj in objectives {
        let start = (obj.x().u8(), obj.y().u8());
        // Plan from the objective outward to a room edge; the corridor blockers
        // are in walk order from the objective, so the LAST is the outermost
        // seal (closest to the edge) an incoming squad reaches first.
        let Some(corridor) = breach_path_blockers_to_edge(&is_wall, &blockers, start) else {
            continue;
        };
        let Some(&outermost) = corridor.last() else {
            continue; // already reachable without dismantling — no breach needed.
        };
        let total: u32 = corridor.iter().fold(0u32, |acc, tile| {
            let hits = match blockers.get(tile) {
                Some(BreachBlocker::Dismantlable(h)) => *h,
                _ => 0,
            };
            acc.saturating_add(hits)
        });
        // Cheapest corridor wins; deterministic tie-break on the blocker tile.
        let replace = match best {
            None => true,
            Some((best_tile, best_hits)) => total < best_hits || (total == best_hits && outermost < best_tile),
        };
        if replace {
            best = Some((outermost, total));
        }
    }

    best.map(|((x, y), hits)| {
        let pos = Position::new(
            RoomCoordinate::new(x).expect("breach blocker x in-bounds"),
            RoomCoordinate::new(y).expect("breach blocker y in-bounds"),
            room,
        );
        (pos, hits)
    })
}

/// Whether a creep can walk from a room edge to within range 1 of `pos`
/// RIGHT NOW (no dismantling) given current structures — the same "reachable
/// now" test [`position_access`] applies, exposed for any position (e.g. a
/// source). Used for diagnostics and for gating remote mining on whether the
/// source is actually reachable. `false` if the room is not visible.
pub fn position_reachable_now(room: RoomName, structures: &[StructureObject], pos: Position) -> bool {
    let Some(room_obj) = game::rooms().get(room) else {
        return false;
    };

    let terrain = FastRoomTerrain::new(room_obj.get_terrain().get_raw_buffer().to_vec());

    let blocked: HashSet<(u8, u8)> = structures
        .iter()
        .filter(|s| !structure_is_walkable(s))
        .map(|s| {
            let p = s.pos();
            (p.x().u8(), p.y().u8())
        })
        .collect();

    let start = (pos.x().u8(), pos.y().u8());
    let passable = |x: u8, y: u8| !terrain.is_wall(x, y) && !blocked.contains(&(x, y));
    reaches_room_edge(&passable, start)
}

/// Rooms the breach-plan cache retains; least-recently-used entries are
/// evicted beyond this. Generously above `salvage_max_missions` (default 1) —
/// entries are a handful of tiles each.
const MAX_BREACH_PLAN_ROOMS: usize = 8;

struct BreachPlanEntry {
    blocker_fingerprint: u64,
    breach_tiles: HashSet<(u8, u8)>,
    last_used: u32,
}

/// Per-room cache of the controller breach corridor. The corridor only
/// changes when the blocker SET changes — a structure dies, appears, or
/// crosses the hit-pool horizon — never as hits drift under dismantling, so
/// the terrain fetch + Dijkstra run once per structural change instead of
/// once per dismantler retarget ([`blocker_fingerprint`] is the invalidation
/// key; one-off pathfinding per operator directive). Shared across creeps:
/// both dismantlers of a mission work the same corridor.
#[derive(Default)]
pub struct BreachPlanCache {
    entries: HashMap<RoomName, BreachPlanEntry>,
}

impl BreachPlanCache {
    fn corridor(&mut self, room_name: RoomName, fingerprint: u64, compute: impl FnOnce() -> HashSet<(u8, u8)>) -> &HashSet<(u8, u8)> {
        if self.entries.len() > MAX_BREACH_PLAN_ROOMS {
            if let Some(oldest) = self
                .entries
                .iter()
                .filter(|(name, _)| **name != room_name)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(name, _)| *name)
            {
                self.entries.remove(&oldest);
            }
        }

        use std::collections::hash_map::Entry;

        let entry = match self.entries.entry(room_name) {
            Entry::Occupied(mut occupied) => {
                if occupied.get().blocker_fingerprint != fingerprint {
                    let breach_tiles = compute();
                    let entry = occupied.get_mut();
                    entry.blocker_fingerprint = fingerprint;
                    entry.breach_tiles = breach_tiles;
                }
                occupied.into_mut()
            }
            Entry::Vacant(vacant) => vacant.insert(BreachPlanEntry {
                blocker_fingerprint: fingerprint,
                breach_tiles: compute(),
                last_used: 0,
            }),
        };

        entry.last_used = game::time();

        &entry.breach_tiles
    }
}

/// Tiles on the cheapest dismantle corridors from the creep to the room's
/// OBJECTIVES — the controller AND every source — unioned, served from
/// [`BreachPlanCache`] and re-planned only on blocker-set change. Clearing
/// these opens access for de-claimers/reservers (controller) AND miners
/// (sources): a source walled off by leftover enemy ramparts/walls is
/// otherwise unreachable, since the cost matrix marks those structures
/// impassable. `None`/empty when there are no objectives, all are already
/// reachable, terrain is unavailable, or they are sealed past the hit-pool
/// horizon (callers fall back to nearest-target selection).
fn objective_breach_tiles<'a>(
    cache: &'a mut BreachPlanCache,
    creep_pos: Position,
    dismantle_room: &RoomData,
    structures: &[StructureObject],
    max_structure_hits: u32,
) -> Option<&'a HashSet<(u8, u8)>> {
    let static_data = dismantle_room.get_static_visibility_data()?;

    let mut goals: Vec<(u8, u8)> = Vec::new();
    if let Some(controller) = static_data.controller() {
        let p = controller.pos();
        goals.push((p.x().u8(), p.y().u8()));
    }
    for source in static_data.sources() {
        let p = source.pos();
        goals.push((p.x().u8(), p.y().u8()));
    }

    if goals.is_empty() {
        return None;
    }

    let blockers = breach_blockers(structures, max_structure_hits);
    let fingerprint = blocker_fingerprint(&blockers);

    let room_name = dismantle_room.name;
    let start = (creep_pos.x().u8(), creep_pos.y().u8());

    Some(cache.corridor(room_name, fingerprint, move || {
        let Some(room) = game::rooms().get(room_name) else {
            return HashSet::new();
        };

        let terrain = FastRoomTerrain::new(room.get_terrain().get_raw_buffer().to_vec());
        let is_wall = |x: u8, y: u8| terrain.is_wall(x, y);

        // Union the breach corridor to each objective (controller + sources).
        let mut tiles: HashSet<(u8, u8)> = HashSet::new();
        for goal in &goals {
            if let Some(corridor) = breach_path_blockers(&is_wall, &blockers, start, *goal) {
                tiles.extend(corridor);
            }
        }
        tiles
    }))
}

/// Nearest workable target from a candidate set.
fn pick_dismantle_target(
    candidates: &[&StructureObject],
    creep_pos: Position,
    pathfinder: &mut PathfinderService,
) -> Option<StructureObject> {
    //TODO: Fix this hack which is a workaround for range of 1 pathfinding returning empty path.
    candidates
        .iter()
        .find(|s| s.pos().get_range_to(creep_pos) <= 1)
        .map(|&s| s.clone())
        .or_else(|| pathfinder.nearest_by_path(creep_pos, candidates.iter().map(|&s| s.clone()), 1))
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_dismantle_state<F, R>(
    creep: &Creep,
    dismantle_room: &RoomData,
    ignore_storage: bool,
    max_structure_hits: u32,
    pathfinder: &mut PathfinderService,
    breach_cache: &mut BreachPlanCache,
    state_map: F,
) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    //TODO: Add bypass for energy check.
    if creep.store().get_capacity(Some(ResourceType::Energy)) == 0 || creep.store().get_free_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        let structures = dismantle_room.get_structures()?;
        let static_visibility_data = dismantle_room.get_static_visibility_data()?;
        let sources = static_visibility_data.sources();

        let hostile_ramparts = hostile_rampart_positions(structures.all());

        //TODO: Don't collect here when range check is fixed.
        let dismantle_structures = structures
            .all()
            .iter()
            .filter(|s| !ignore_for_dismantle(*s, sources))
            .filter(|s| can_dismantle(*s))
            .filter(|s| within_dismantle_hits_horizon(*s, max_structure_hits))
            .filter(|s| !blocked_by_hostile_rampart(*s, &hostile_ramparts))
            .filter(|s| ignore_storage || has_empty_storage(*s))
            .collect::<Vec<_>>();

        let creep_pos = creep.pos();

        // Objective-access priority: structures on the cheapest corridors to
        // the controller AND the sources come first, so reservers/de-claimers
        // (controller) and miners (sources) can reach their targets instead of
        // waiting for the whole room to be flattened in nearest-first order.
        // Falls back to nearest-target when the corridors are open, unknown, or
        // their structures are not yet workable (e.g. store not emptied).
        let breach_structures = match objective_breach_tiles(breach_cache, creep_pos, dismantle_room, structures.all(), max_structure_hits)
        {
            Some(breach_tiles) if !breach_tiles.is_empty() => dismantle_structures
                .iter()
                .copied()
                .filter(|s| breach_tiles.contains(&(s.pos().x().u8(), s.pos().y().u8())))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };

        let best_structure = pick_dismantle_target(&breach_structures, creep_pos, pathfinder)
            .or_else(|| pick_dismantle_target(&dismantle_structures, creep_pos, pathfinder));

        if let Some(structure) = best_structure {
            return Some(state_map(RemoteStructureIdentifier::new(&structure)));
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_dismantle<F, R>(
    tick_context: &mut JobTickContext,
    dismantle_structure_id: RemoteStructureIdentifier,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    if creep.store().get_capacity(Some(ResourceType::Energy)) > 0 && creep.store().get_free_capacity(Some(ResourceType::Energy)) == 0 {
        return Some(next_state());
    }

    let creep_pos = creep.pos();
    let target_position = dismantle_structure_id.pos();

    let expect_resolve = if creep_pos.room_name() == target_position.room_name() {
        true
    } else {
        let target_room_entity = tick_context.runtime_data.mapping.get_room(&target_position.room_name())?;
        let target_room_data = tick_context.system_data.room_data.get(target_room_entity)?;

        target_room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false)
    };

    let dismantle_target = dismantle_structure_id.resolve();

    if let Some(dismantle_target) = dismantle_target.as_ref() {
        if let Some(attackable) = dismantle_target.as_attackable() {
            if attackable.hits() == 0 {
                return Some(next_state());
            }
        }
    } else if expect_resolve {
        return Some(next_state());
    }

    if !creep_pos.in_range_to(target_position, 1) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1)
                .room_options(RoomOptions::new(HostileBehavior::HighCost));
        }

        return None;
    }

    // In range — mark as working within range 1 of the dismantle target.
    mark_working(tick_context, target_position, 1);

    if let Some(structure) = dismantle_target.as_ref() {
        if tick_context.action_flags.consume(SimultaneousActionFlags::DISMANTLE) {
            if let Some(dismantleable) = structure.as_dismantleable() {
                match creep.dismantle(dismantleable) {
                    Ok(()) => None,
                    Err(_) => Some(next_state()),
                }
            } else {
                Some(next_state())
            }
        } else {
            None
        }
    } else {
        Some(next_state())
    }
}
