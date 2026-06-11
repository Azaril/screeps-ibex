//! Linear (Chebyshev) nearest-selection iterator helpers — pure math,
//! no pathfinding, no budget. The budgeted by-real-path selection that
//! used to live here is [`crate::pathing::pathfinderservice`]'s
//! `nearest_by_path` (statics-review M4).

use screeps::local::Position;

pub trait FindNearestItertools: Iterator {
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
