use screeps::local::Position;
use screeps::pathfinder;
use screeps::*;

pub struct PathFinderHelpers;

/// Replacement for the removed `screeps::Path` type.
/// Wraps a vectorized path as a list of positions.
pub struct Path(pub Vec<Position>);

impl Path {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Ops cap for the same-room helpers (P1.B1 / IBEX-035, ADR 0004 step
/// 1). The engine default is 2000 ops PER SEARCH and the
/// `find_nearest_*` combinators below run one search PER CANDIDATE —
/// the uncapped worst case is candidates × 2000 on a single decision.
/// A single 50×50 room cannot usefully consume more than ~500 ops; a
/// search that exhausts this cap returns an incomplete/empty path,
/// which every caller already treats as "no path".
const SAME_ROOM_MAX_OPS: u32 = 500;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl PathFinderHelpers {
    pub fn same_room_ignore_creeps(start_pos: Position, end_pos: Position) -> Path {
        // PathFinder.search ignores creeps by default. It also ignores structures unless
        // a room_callback providing a CostMatrix is given.
        // TODO: Add room_callback with structure costs if structure avoidance is needed.
        let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(SAME_ROOM_MAX_OPS);
        let result = pathfinder::search(start_pos, end_pos, 0, Some(options));
        Path(result.path())
    }

    pub fn same_room_ignore_creeps_range_1(start_pos: Position, end_pos: Position) -> Path {
        let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(SAME_ROOM_MAX_OPS);
        let result = pathfinder::search(start_pos, end_pos, 1, Some(options));
        Path(result.path())
    }

    pub fn same_room_ignore_creeps_range_3(start_pos: Position, end_pos: Position) -> Path {
        let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(SAME_ROOM_MAX_OPS);
        let result = pathfinder::search(start_pos, end_pos, 3, Some(options));
        Path(result.path())
    }

    pub fn same_room_ignore_creeps_and_structures(start_pos: Position, end_pos: Position) -> Path {
        // PathFinder.search ignores both creeps and structures by default,
        // which matches the intent of this method.
        let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(SAME_ROOM_MAX_OPS);
        let result = pathfinder::search(start_pos, end_pos, 0, Some(options));
        Path(result.path())
    }

    pub fn same_room_ignore_creeps_and_structures_range_1(start_pos: Position, end_pos: Position) -> Path {
        let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(SAME_ROOM_MAX_OPS);
        let result = pathfinder::search(start_pos, end_pos, 1, Some(options));
        Path(result.path())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait FindNearestItertools: Iterator {
    fn find_nearest_from<F, V>(self, start_pos: Position, generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(Position, Position) -> Path,
    {
        self.filter_map(|pos_object| {
            let path = generator(start_pos, pos_object.pos());
            if !path.is_empty() {
                Some((path.len(), pos_object))
            } else {
                None
            }
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_path_from<F, V>(self, start_pos: Position, generator: F) -> Option<Path>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(Position, Position) -> Path,
    {
        self.filter_map(|pos_object| {
            let path = generator(start_pos, pos_object.pos());
            if !path.is_empty() {
                let len = path.len();
                Some((len, path))
            } else {
                None
            }
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, path)| path)
    }

    fn find_nearest_to<F, V>(self, end_pos: Position, generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(Position, Position) -> Path,
    {
        self.filter_map(|pos_object| {
            let path = generator(pos_object.pos(), end_pos);
            if !path.is_empty() {
                Some((path.len(), pos_object))
            } else {
                None
            }
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_path_to<F, V>(self, end_pos: Position, generator: F) -> Option<Path>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(Position, Position) -> Path,
    {
        self.filter_map(|pos_object| {
            let path = generator(pos_object.pos(), end_pos);
            if !path.is_empty() {
                let len = path.len();
                Some((len, path))
            } else {
                None
            }
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, path)| path)
    }

    fn find_nearest_linear<V>(self, other_pos: Position) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
    {
        self.map(|pos_object| (other_pos.get_range_to(pos_object.pos()), pos_object))
            .min_by_key(|(length, _)| *length)
            .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_linear_distance<V>(self, other_pos: Position) -> Option<u32>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
    {
        self.map(|pos_object| other_pos.get_range_to(pos_object.pos())).min()
    }

    fn find_nearest_linear_by<F, V>(self, other_pos: Position, pos_generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        F: Fn(&V) -> Position,
    {
        self.map(|pos_object| (other_pos.get_range_to(pos_generator(&pos_object)), pos_object))
            .min_by_key(|(length, _)| *length)
            .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_linear_distance_by<F, V>(self, other_pos: Position, pos_generator: F) -> Option<u32>
    where
        Self: Iterator<Item = V> + Sized,
        F: Fn(&V) -> Position,
    {
        self.map(|pos_object| other_pos.get_range_to(pos_generator(&pos_object))).min()
    }
}

impl<T: ?Sized> FindNearestItertools for T where T: Iterator {}
