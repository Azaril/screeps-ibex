use super::dismantle::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::utility::movebehavior::mark_working;
use crate::pathing::pathfinderservice::PathfinderService;
use crate::room::data::*;
use crate::structureidentifier::*;
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
fn breach_blockers(structures: &[StructureObject], max_structure_hits: u32) -> HashMap<(u8, u8), BreachBlocker> {
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

/// Tiles on the cheapest dismantle corridor from the creep to the room's
/// controller. Empty when the controller is already reachable, the room has
/// no controller, terrain is unavailable, or the room is sealed past the
/// hit-pool horizon (callers fall back to nearest-target selection).
fn controller_breach_tiles(
    creep_pos: Position,
    dismantle_room: &RoomData,
    structures: &[StructureObject],
    max_structure_hits: u32,
) -> HashSet<(u8, u8)> {
    let Some(controller_pos) = dismantle_room
        .get_static_visibility_data()
        .and_then(|static_data| static_data.controller().map(|c| c.pos()))
    else {
        return HashSet::new();
    };

    let Some(room) = game::rooms().get(dismantle_room.name) else {
        return HashSet::new();
    };

    let terrain = FastRoomTerrain::new(room.get_terrain().get_raw_buffer().to_vec());

    let blockers = breach_blockers(structures, max_structure_hits);
    let start = (creep_pos.x().u8(), creep_pos.y().u8());
    let goal = (controller_pos.x().u8(), controller_pos.y().u8());

    breach_path_blockers(&|x, y| terrain.is_wall(x, y), &blockers, start, goal)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Nearest workable target from a candidate set.
fn pick_dismantle_target(candidates: &[&StructureObject], creep_pos: Position, pathfinder: &mut PathfinderService) -> Option<StructureObject> {
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

        // Controller-access priority: structures on the cheapest corridor to
        // the controller come first, so a reserver/claimer can reach it as
        // soon as the dead owner's controller decays — instead of waiting for
        // the whole room to be flattened in nearest-first order. Falls back
        // to nearest-target when the corridor is open, unknown, or its
        // structures are not yet workable (e.g. store not emptied by raiders).
        let breach_tiles = controller_breach_tiles(creep_pos, dismantle_room, structures.all(), max_structure_hits);

        let breach_structures = dismantle_structures
            .iter()
            .copied()
            .filter(|s| breach_tiles.contains(&(s.pos().x().u8(), s.pos().y().u8())))
            .collect::<Vec<_>>();

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
