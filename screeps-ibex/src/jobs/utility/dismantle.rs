use crate::remoteobjectid::*;
use screeps::*;
use std::borrow::*;
use std::collections::HashSet;

pub fn ignore_for_dismantle<T>(structure: T, sources: &[RemoteObjectId<Source>]) -> bool
where
    T: Borrow<StructureObject>,
{
    match structure.borrow() {
        StructureObject::StructureContainer(c) => {
            let pos = c.pos();

            sources.iter().any(|s| s.pos().in_range_to(pos, 1))
        }
        _ => false,
    }
}

pub fn can_dismantle<T>(structure: T) -> bool
where
    T: Borrow<StructureObject>,
{
    let structure = structure.borrow();

    // Engine-dismantlable (constructible) types only: invader cores, keeper
    // lairs, portals and controllers pass an hits>0 check but
    // creep.dismantle() can never damage them — selecting one would wedge
    // target selection and mission completion forever.
    structure.as_dismantleable().is_some() && structure.as_attackable().map(|a| a.hits() > 0 && a.hits_max() > 0).unwrap_or(false)
}

/// Tiles covered by a hostile non-public rampart: the structure beneath can
/// be neither withdrawn from nor dismantled until the rampart falls, so it
/// must stay out of loot/dismantle scope (and out of EV value) while the
/// rampart stands.
pub fn hostile_rampart_positions(structures: &[StructureObject]) -> HashSet<Position> {
    structures
        .iter()
        .filter_map(|s| match s {
            StructureObject::StructureRampart(rampart) if !rampart.my() && !rampart.is_public() => Some(rampart.pos()),
            _ => None,
        })
        .collect()
}

/// True if this structure sits under a hostile rampart (and is not itself a
/// rampart — ramparts are always directly attackable).
pub fn blocked_by_hostile_rampart<T>(structure: T, hostile_ramparts: &HashSet<Position>) -> bool
where
    T: Borrow<StructureObject>,
{
    let structure = structure.borrow();

    if matches!(structure, StructureObject::StructureRampart(_)) {
        return false;
    }

    hostile_ramparts.contains(&structure.pos())
}

pub fn has_empty_storage<T>(structure: T) -> bool
where
    T: Borrow<StructureObject>,
{
    if let Some(store) = structure.borrow().as_has_store() {
        let store_types = store.store().store_types();

        return !store_types.iter().any(|t| store.store().get_used_capacity(Some(*t)) > 0);
    }

    true
}

/// Hit-pool horizon for dismantle work: targets with more hits than
/// `max_hits` are skipped entirely (0 = no limit). Huge walls/ramparts would
/// otherwise pin a dismantle mission ~forever and block any downstream
/// handoff (e.g. salvage → mining outpost). Mission completion checks and
/// job target selection MUST share this filter or the mission never ends
/// (`features.derelict.max_structure_hits`).
pub fn within_dismantle_hits_horizon<T>(structure: T, max_hits: u32) -> bool
where
    T: Borrow<StructureObject>,
{
    if max_hits == 0 {
        return true;
    }

    structure.borrow().as_attackable().map(|a| a.hits() <= max_hits).unwrap_or(false)
}

/// Any dismantle target READY to work on right now: in scope (not a road,
/// not mining infrastructure, engine-dismantlable, within the hit-pool
/// horizon, not under a hostile rampart) AND with an empty store (loot before
/// wreck). `max_structure_hits` must match what the dismantler jobs were
/// spawned with — work detection and job target selection share these
/// filters or the work never ends.
pub fn requires_dismantling(structures: &[StructureObject], sources: &[RemoteObjectId<Source>], max_structure_hits: u32) -> bool {
    let hostile_ramparts = hostile_rampart_positions(structures);

    structures
        .iter()
        .filter(|s| s.structure_type() != StructureType::Road)
        .filter(|s| !ignore_for_dismantle(*s, sources))
        .filter(|s| can_dismantle(*s))
        .filter(|s| within_dismantle_hits_horizon(*s, max_structure_hits))
        .filter(|s| !blocked_by_hostile_rampart(*s, &hostile_ramparts))
        .any(has_empty_storage)
}

/// Structures whose stores may be looted by salvage/raid work: structures
/// owned by another player, or unowned store structures (containers) that are
/// not our mining infrastructure (source-adjacent — same exclusion as
/// [`ignore_for_dismantle`]). Own/ownerless-controller structures are never
/// loot targets, and anything under a hostile rampart is unreachable until
/// the rampart falls.
pub fn is_salvage_loot_target<T>(structure: T, sources: &[RemoteObjectId<Source>], hostile_ramparts: &HashSet<Position>) -> bool
where
    T: Borrow<StructureObject>,
{
    let structure = structure.borrow();

    if has_empty_storage(structure) {
        return false;
    }

    if blocked_by_hostile_rampart(structure, hostile_ramparts) {
        return false;
    }

    match structure.as_owned() {
        Some(owned) => owned.owner().is_some() && !owned.my(),
        None => !ignore_for_dismantle(structure, sources),
    }
}

/// Movement-blocking content of one tile for the breach search
/// ([`breach_path_blockers`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreachBlocker {
    /// Total hits of dismantlable blockers on the tile; the corridor may pass
    /// through at that cost.
    Dismantlable(u32),
    /// At least one blocker we will never clear (engine-undismantlable, or
    /// over the hit-pool horizon): the corridor must route around.
    Impassable,
}

/// Cost of stepping onto any passable tile. Swamps are deliberately not
/// surcharged: the corridor optimizes dismantle work, not travel time.
const BREACH_STEP_COST: u64 = 1;
/// Cost per blocker hit, chosen larger than the maximum possible step count
/// (50×50 = 2_500) so the search strictly minimizes total hits to clear and
/// breaks ties by path length.
const BREACH_HIT_WEIGHT: u64 = 4_096;

/// Fingerprint of the blocker SET for breach-corridor cache invalidation:
/// FNV-1a over the sorted tile coordinates and their passability class. Hits
/// are deliberately excluded — they drift every tick under dismantling and
/// decay, and re-planning on drift would flap the corridor mid-chew (EP-4.4:
/// shed re-decision, never committed work). The corridor re-plans exactly
/// when a blocker appears, disappears, or crosses the dismantlable/impassable
/// line.
pub fn blocker_fingerprint(blockers: &std::collections::HashMap<(u8, u8), BreachBlocker>) -> u64 {
    let mut tiles: Vec<(u8, u8, u8)> = blockers
        .iter()
        .map(|((x, y), blocker)| (*x, *y, matches!(blocker, BreachBlocker::Impassable) as u8))
        .collect();

    tiles.sort_unstable();

    // FNV-1a (inline: 4 lines of arithmetic, not an encoding — EP-9.1 scope
    // is wire formats).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (x, y, impassable) in tiles {
        for byte in [x, y, impassable] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    hash
}

/// Breach-corridor PRICING POLICY over the pathfinding system's
/// [`room_grid_dijkstra`](screeps_rover::room_grid_dijkstra)
/// mechanism: plan the cheapest corridor from `start` to within range 1 of
/// `goal` (the controller), where entering a tile costs [`BREACH_STEP_COST`]
/// plus [`BREACH_HIT_WEIGHT`] per hit of dismantlable blocker standing on
/// it. Returns the blocker tiles along that corridor in walk order — empty
/// when the goal is already reachable without dismantling — or `None` when
/// no corridor exists even through dismantlable blockers (sealed by terrain
/// or by structures past the hit-pool horizon).
///
/// Pure (host-tested): terrain arrives as a closure, blockers as plain tile
/// coordinates. The search algorithm itself lives in the pathfinding system
/// (`screeps_rover::room_grid_dijkstra`) — pathfinding algorithms are never
/// implemented in feature modules.
pub fn breach_path_blockers(
    is_wall: &dyn Fn(u8, u8) -> bool,
    blockers: &std::collections::HashMap<(u8, u8), BreachBlocker>,
    start: (u8, u8),
    goal: (u8, u8),
) -> Option<Vec<(u8, u8)>> {
    let enter_cost = |x: u8, y: u8| -> Option<u64> {
        if is_wall(x, y) {
            return None;
        }

        match blockers.get(&(x, y)) {
            Some(BreachBlocker::Impassable) => None,
            Some(BreachBlocker::Dismantlable(hits)) => Some(BREACH_STEP_COST + *hits as u64 * BREACH_HIT_WEIGHT),
            None => Some(BREACH_STEP_COST),
        }
    };

    let path = screeps_rover::room_grid_dijkstra(&enter_cost, start, goal, 1)?;

    Some(
        path.into_iter()
            .filter(|tile| matches!(blockers.get(tile), Some(BreachBlocker::Dismantlable(_))))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const NO_WALLS: fn(u8, u8) -> bool = |_, _| false;

    fn wall_line_with_gaps(x: u8, gaps: &[(u8, u32)]) -> HashMap<(u8, u8), BreachBlocker> {
        // A constructed-wall line across column `x`; `gaps` are (y, hits)
        // tiles that are dismantlable instead of impassable.
        let mut blockers = HashMap::new();
        for y in 0..50u8 {
            blockers.insert((x, y), BreachBlocker::Impassable);
        }
        for (y, hits) in gaps {
            blockers.insert((x, *y), BreachBlocker::Dismantlable(*hits));
        }
        blockers
    }

    #[test]
    fn open_room_needs_no_breach() {
        let result = breach_path_blockers(&NO_WALLS, &HashMap::new(), (5, 25), (45, 25));
        assert_eq!(result, Some(Vec::new()));
    }

    #[test]
    fn start_adjacent_to_goal_needs_no_breach() {
        let blockers = wall_line_with_gaps(25, &[]);
        let result = breach_path_blockers(&NO_WALLS, &blockers, (44, 25), (45, 25));
        assert_eq!(result, Some(Vec::new()));
    }

    #[test]
    fn corridor_picks_the_cheapest_gap_not_the_shortest_path() {
        // Straight-line gap costs 100_000 hits; a distant gap costs 100.
        // Hits dominate steps, so the corridor detours.
        let blockers = wall_line_with_gaps(25, &[(25, 100_000), (5, 100)]);
        let result = breach_path_blockers(&NO_WALLS, &blockers, (5, 25), (45, 25)).expect("corridor should exist");
        assert_eq!(result, vec![(25, 5)]);
    }

    #[test]
    fn sealed_room_returns_none() {
        // No dismantlable gap at all: impassable wall line, no corridor.
        let blockers = wall_line_with_gaps(25, &[]);
        assert_eq!(breach_path_blockers(&NO_WALLS, &blockers, (5, 25), (45, 25)), None);
    }

    #[test]
    fn terrain_walls_seal_like_impassable_structures() {
        // Terrain wall line with a single non-wall tile that carries a
        // dismantlable structure: the corridor must use exactly that tile.
        let is_wall = |x: u8, y: u8| x == 25 && y != 30;
        let mut blockers = HashMap::new();
        blockers.insert((25u8, 30u8), BreachBlocker::Dismantlable(500));

        let result = breach_path_blockers(&is_wall, &blockers, (5, 25), (45, 25)).expect("corridor should exist");
        assert_eq!(result, vec![(25, 30)]);

        // Same terrain, gap blocked by something undismantlable: sealed.
        blockers.insert((25u8, 30u8), BreachBlocker::Impassable);
        assert_eq!(breach_path_blockers(&is_wall, &blockers, (5, 25), (45, 25)), None);
    }

    #[test]
    fn multi_layer_walls_report_every_layer_in_walk_order() {
        let mut blockers = wall_line_with_gaps(20, &[(25, 1_000)]);
        for (tile, blocker) in wall_line_with_gaps(30, &[(25, 1_000)]) {
            blockers.insert(tile, blocker);
        }

        let result = breach_path_blockers(&NO_WALLS, &blockers, (5, 25), (45, 25)).expect("corridor should exist");
        assert_eq!(result, vec![(20, 25), (30, 25)]);
    }

    #[test]
    fn fingerprint_tracks_the_tile_set_not_the_hits() {
        let a = wall_line_with_gaps(25, &[(25, 100_000)]);

        // Same tiles, different hits: hits drift under dismantling/decay and
        // must NOT re-plan the corridor.
        let chewed = wall_line_with_gaps(25, &[(25, 50)]);
        assert_eq!(blocker_fingerprint(&a), blocker_fingerprint(&chewed));

        // A blocker crossing the dismantlable/impassable line re-plans.
        let sealed = wall_line_with_gaps(25, &[]);
        assert_ne!(blocker_fingerprint(&a), blocker_fingerprint(&sealed));

        // A structure death (tile freed) re-plans.
        let mut breached = wall_line_with_gaps(25, &[]);
        breached.remove(&(25, 25));
        assert_ne!(blocker_fingerprint(&sealed), blocker_fingerprint(&breached));
    }

    /// Relation pin: the corridor's total hits never exceed any single
    /// alternative gap — the search minimizes hits first, distance second
    /// (BREACH_HIT_WEIGHT > maximum step count).
    #[test]
    fn corridor_hits_are_minimal_across_gap_choices() {
        for (cheap_hits, far_y) in [(1u32, 0u8), (500, 5), (99_999, 49)] {
            let blockers = wall_line_with_gaps(25, &[(25, 100_000), (far_y, cheap_hits)]);
            let result = breach_path_blockers(&NO_WALLS, &blockers, (5, 25), (45, 25)).expect("corridor should exist");
            assert_eq!(
                result,
                vec![(25, far_y)],
                "cheapest gap ({} hits at y {}) must win",
                cheap_hits,
                far_y
            );
        }
    }
}
