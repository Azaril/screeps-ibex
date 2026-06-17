//! The sim's movement-planning bridge (P2.M-bridge): turns a tactical movement **goal**
//! ([`CombatIntent::MoveTo`] / [`CombatIntent::Flee`]) into the next-step [`Direction`] by routing
//! through **rover** — a [`CombatWorld`]-backed [`CostMatrixDataSource`] feeds rover's cost-matrix
//! builder, and rover's headless [`LocalPathfinder`] does the multi-step, room-aware search. The
//! caller hands the resulting `Direction` to the engine's `resolve_moves` (the authoritative
//! "server"), so live and sim plan paths through the same system and the engine validates the move
//! (ADR 0006 §B.2). Real pathfinding, not a greedy stepper: a kiter routes *around* obstacles.

use screeps::{Direction, Position, RoomName};
use screeps_combat_decision::CombatIntent;
use screeps_combat_engine::CombatWorld;
use screeps_rover::{
    ConstructionSiteCostMatrixCache, CostMatrixCache, CostMatrixDataSource, CostMatrixOptions, CostMatrixSystem,
    CostMatrixWrite, CreepCostMatrixCache, LinearCostMatrix, LocalPathfinder, PathfindingProvider,
    StuctureCostMatrixCache,
};

/// Search budget — the room is 2500 tiles; this comfortably covers a single-room plan.
const MAX_OPS: u32 = 2000;
/// Swamp tile cost baked into the matrix (matches rover's `CostMatrixOptions::default().swamp_cost`).
const SWAMP_COST: u8 = 10;

/// A [`CostMatrixDataSource`] over a `CombatWorld` snapshot. It owns its data (no borrow of the
/// world), satisfying the `'static` bound `CostMatrixSystem` places on its boxed data source. Every
/// obstacle — walls, structures, towers, and every living creep — is impassable (255); swamps cost
/// [`SWAMP_COST`]. The pathing creep's own tile being blocked is harmless: the search starts there
/// and never re-enters it.
struct CombatCostSource {
    room: RoomName,
    walls: Vec<(u8, u8)>,
    swamps: Vec<(u8, u8)>,
    blockers: Vec<(u8, u8)>,
    creeps: Vec<(u8, u8)>,
}

impl CombatCostSource {
    fn from_world(world: &CombatWorld, room: RoomName) -> Self {
        let mut blockers = Vec::new();
        for s in world.structures.iter().filter(|s| s.is_alive()) {
            blockers.push((s.pos.x().u8(), s.pos.y().u8()));
        }
        for t in world.towers.iter().filter(|t| t.is_alive()) {
            blockers.push((t.pos.x().u8(), t.pos.y().u8()));
        }
        Self {
            room,
            walls: world.terrain.walls.iter().copied().collect(),
            swamps: world.terrain.swamps.iter().copied().collect(),
            blockers,
            creeps: world
                .creeps
                .iter()
                .filter(|c| c.is_alive())
                .map(|c| (c.pos.x().u8(), c.pos.y().u8()))
                .collect(),
        }
    }
}

impl CostMatrixDataSource for CombatCostSource {
    fn get_structure_costs(&self, room_name: RoomName) -> Option<StuctureCostMatrixCache> {
        if room_name != self.room {
            return None;
        }
        let mut other = LinearCostMatrix::new();
        // Swamps first, then impassables — later `set`s win on a tile (apply order = push order).
        for &(x, y) in &self.swamps {
            other.set(x, y, SWAMP_COST);
        }
        for &(x, y) in self.walls.iter().chain(&self.blockers) {
            other.set(x, y, u8::MAX);
        }
        Some(StuctureCostMatrixCache { roads: LinearCostMatrix::new(), other })
    }

    fn get_construction_site_costs(&self, _room: RoomName) -> Option<ConstructionSiteCostMatrixCache> {
        None
    }

    fn get_creep_costs(&self, room_name: RoomName) -> Option<CreepCostMatrixCache> {
        if room_name != self.room {
            return None;
        }
        let mut hostile_creeps = LinearCostMatrix::new();
        for &(x, y) in &self.creeps {
            hostile_creeps.set(x, y, u8::MAX);
        }
        Some(CreepCostMatrixCache {
            friendly_creeps: LinearCostMatrix::new(),
            hostile_creeps,
            source_keeper_agro: LinearCostMatrix::new(),
        })
    }
}

/// Resolve a movement goal to the next-step [`Direction`] from `from`, via rover's pathfinder over
/// the `CombatWorld`. Returns `None` for non-movement intents, when already satisfied (empty path),
/// or when no route exists. Combat intents (`Attack`/`Heal`/…) and `Idle` yield `None` here.
pub fn resolve_move_direction(world: &CombatWorld, from: Position, intent: &CombatIntent) -> Option<Direction> {
    let room = from.room_name();
    let mut cache = CostMatrixCache::default();
    let source = Box::new(CombatCostSource::from_world(world, room));
    let mut system = CostMatrixSystem::new(&mut cache, source);
    let opts = CostMatrixOptions::default();
    let mut room_cb = |r: RoomName| system.build_local_cost_matrix(r, &opts).ok();
    let mut pf = LocalPathfinder;

    let result = match intent {
        CombatIntent::MoveTo { target, range } => {
            pf.search(from, *target, *range as u32, &mut room_cb, MAX_OPS, opts.plains_cost, opts.swamp_cost)
        }
        CombatIntent::Flee { from: threats, range } => {
            let goals: Vec<(Position, u32)> = threats.iter().map(|p| (*p, *range as u32)).collect();
            pf.search_many(from, &goals, true, &mut room_cb, MAX_OPS, opts.plains_cost, opts.swamp_cost)
        }
        _ => return None,
    };

    result.path.first().and_then(|next| from.get_direction_to(*next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Part, RoomCoordinate};
    use screeps_combat_engine::{CombatWorld, SimBody, SimCreep};

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }
    fn creep(id: u32, x: u8, y: u8) -> SimCreep {
        SimCreep {
            id,
            owner: 0,
            pos: pos(x, y),
            body: SimBody::unboosted(&[Part::Move, Part::Attack]),
            fatigue: 0,
        }
    }

    #[test]
    fn moves_toward_an_open_goal() {
        let world = CombatWorld { creeps: vec![creep(1, 5, 25)], ..Default::default() };
        let dir = resolve_move_direction(&world, pos(5, 25), &CombatIntent::MoveTo { target: pos(15, 25), range: 0 });
        // 8-directional + uniform cost ⇒ Right / TopRight / BottomRight are all equally-optimal
        // first steps toward an eastern goal; assert we head east, not the exact diagonal.
        assert!(
            matches!(dir, Some(Direction::Right | Direction::TopRight | Direction::BottomRight)),
            "open room → step east toward the goal, got {:?}",
            dir
        );
    }

    #[test]
    fn detours_around_a_wall() {
        // Wall column at x=6, y=23..=27, goal directly east behind it. The first step must not be
        // straight into the wall at (6,25) — it routes around (a diagonal toward a gap).
        let mut world = CombatWorld { creeps: vec![creep(1, 5, 25)], ..Default::default() };
        for y in 23..=27 {
            world.terrain.walls.insert((6, y));
        }
        let dir = resolve_move_direction(&world, pos(5, 25), &CombatIntent::MoveTo { target: pos(10, 25), range: 0 })
            .expect("a route around exists");
        // Stepping Right would enter the wall at (6,25); the planner must pick a detour.
        assert_ne!(dir, Direction::Right, "does not walk into the wall");
        assert!(
            matches!(dir, Direction::TopRight | Direction::BottomRight | Direction::Top | Direction::Bottom),
            "heads around the wall, got {:?}",
            dir
        );
    }

    #[test]
    fn already_in_range_yields_no_move() {
        let world = CombatWorld { creeps: vec![creep(1, 5, 25)], ..Default::default() };
        let dir = resolve_move_direction(&world, pos(5, 25), &CombatIntent::MoveTo { target: pos(7, 25), range: 3 });
        assert_eq!(dir, None, "distance 2 already within range 3 → hold");
    }

    #[test]
    fn flees_away_from_a_threat() {
        let world = CombatWorld { creeps: vec![creep(1, 30, 25)], ..Default::default() };
        let dir = resolve_move_direction(&world, pos(30, 25), &CombatIntent::Flee { from: vec![pos(25, 25)], range: 5 })
            .expect("can flee in an open room");
        // Threat is to the west (x=25); fleeing should move east (away), increasing x.
        assert!(
            matches!(dir, Direction::Right | Direction::TopRight | Direction::BottomRight),
            "flees away from the threat (eastward), got {:?}",
            dir
        );
    }

    #[test]
    fn non_movement_intent_is_none() {
        let world = CombatWorld { creeps: vec![creep(1, 5, 25)], ..Default::default() };
        assert_eq!(resolve_move_direction(&world, pos(5, 25), &CombatIntent::Idle), None);
    }
}
