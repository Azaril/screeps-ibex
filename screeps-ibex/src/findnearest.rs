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

/// Ops cap for the same-room helper (P1.B1 / IBEX-035, ADR 0004 step
/// 1). The engine default is 2000 ops PER SEARCH and the
/// `find_nearest_*` combinator below runs one search PER CANDIDATE —
/// the uncapped worst case is candidates × 2000 on a single decision.
/// A single 50×50 room cannot usefully consume more than ~500 ops; a
/// search that exhausts this cap returns an incomplete/empty path,
/// which every caller already treats as "no path".
///
/// P1.B4: each search additionally draws its ops from the mission-side
/// pool ([`crate::pathbudget`]) — the per-search cap bounds one
/// search, the pool bounds the tick's AGGREGATE.
const SAME_ROOM_MAX_OPS: u32 = 500;

/// Pool-clamped per-search ops grant (0 = pool exhausted, search
/// returns the empty path the callers already handle).
fn same_room_ops() -> u32 {
    crate::pathbudget::take(SAME_ROOM_MAX_OPS)
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl PathFinderHelpers {
    pub fn same_room_ignore_creeps_range_1(start_pos: Position, end_pos: Position) -> Path {
        // PathFinder.search ignores creeps by default. It also ignores structures unless
        // a room_callback providing a CostMatrix is given.
        let ops = same_room_ops();
        if ops == 0 {
            return Path(Vec::new());
        }
        let options = pathfinder::SearchOptions::default().max_rooms(1).max_ops(ops);
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

    fn find_nearest_linear_by<F, V>(self, other_pos: Position, pos_generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        F: Fn(&V) -> Position,
    {
        self.map(|pos_object| (other_pos.get_range_to(pos_generator(&pos_object)), pos_object))
            .min_by_key(|(length, _)| *length)
            .map(|(_, pos_object)| pos_object)
    }
}

impl<T: ?Sized> FindNearestItertools for T where T: Iterator {}
