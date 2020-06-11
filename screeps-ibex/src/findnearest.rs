use screeps::*;
use std::borrow::*;
use std::ops::*;

pub struct PathFinderHelpers;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl PathFinderHelpers {
    pub fn same_room_ignore_creeps(start_pos: RoomPosition, end_pos: RoomPosition) -> Path {
        let find_options = FindOptions::new().max_rooms(1).ignore_creeps(true);

        start_pos.find_path_to(&end_pos, find_options)
    }

    pub fn same_room_ignore_creeps_range_1(start_pos: RoomPosition, end_pos: RoomPosition) -> Path {
        let find_options = FindOptions::new().max_rooms(1).ignore_creeps(true).range(1);

        start_pos.find_path_to(&end_pos, find_options)
    }

    pub fn same_room_ignore_creeps_range_3(start_pos: RoomPosition, end_pos: RoomPosition) -> Path {
        let find_options = FindOptions::new().max_rooms(1).ignore_creeps(true).range(3);

        start_pos.find_path_to(&end_pos, find_options)
    }

    pub fn same_room_ignore_creeps_and_structures(start_pos: RoomPosition, end_pos: RoomPosition) -> Path {
        let find_options = FindOptions::new()
            .max_rooms(1)
            .ignore_creeps(true)
            .ignore_destructible_structures(true);

        start_pos.find_path_to(&end_pos, find_options)
    }

    pub fn same_room_ignore_creeps_and_structures_range_1(start_pos: RoomPosition, end_pos: RoomPosition) -> Path {
        let find_options = FindOptions::new()
            .max_rooms(1)
            .ignore_creeps(true)
            .ignore_destructible_structures(true)
            .range(1);

        start_pos.find_path_to(&end_pos, find_options)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait FindNearestItertools: Iterator {
    fn find_nearest_from<F, V>(self, start_pos: RoomPosition, generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(RoomPosition, RoomPosition) -> Path,
    {
        self.filter_map(|pos_object| {
            if let Path::Vectorized(path) = generator(start_pos, pos_object.borrow().pos()) {
                if !path.is_empty() {
                    //TODO: Check end point is actually target.
                    Some((path.len(), pos_object))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_path_from<F, V>(self, start_pos: RoomPosition, generator: F) -> Option<Path>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(RoomPosition, RoomPosition) -> Path,
    {
        self.filter_map(|pos_object| {
            let path = generator(start_pos, pos_object.pos());

            let len = if let Path::Vectorized(vector_path) = &path {
                if !vector_path.is_empty() {
                    //TODO: Check end point is actually target.
                    Some(vector_path.len())
                } else {
                    None
                }
            } else {
                None
            };

            len.map(|l| (l, path))
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, path)| path)
    }

    fn find_nearest_to<F, V>(self, end_pos: RoomPosition, generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(RoomPosition, RoomPosition) -> Path,
    {
        self.filter_map(|pos_object| {
            if let Path::Vectorized(path) = generator(pos_object.pos(), end_pos) {
                if !path.is_empty() {
                    //TODO: Check end point is actually target.
                    Some((path.len(), pos_object))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_path_to<F, V>(self, end_pos: RoomPosition, generator: F) -> Option<Path>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
        F: Fn(RoomPosition, RoomPosition) -> Path,
    {
        self.filter_map(|pos_object| {
            let path = generator(pos_object.pos(), end_pos);

            let len = if let Path::Vectorized(vector_path) = &path {
                if !vector_path.is_empty() {
                    //TODO: Check end point is actually target.
                    Some(vector_path.len())
                } else {
                    None
                }
            } else {
                None
            };

            len.map(|l| (l, path))
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, path)| path)
    }

    fn find_nearest_linear<V>(self, other_pos: RoomPosition) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
    {
        self.map(|pos_object| (other_pos.get_range_to(&pos_object), pos_object))
            .min_by_key(|(length, _)| *length)
            .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_linear_distance<V>(self, other_pos: RoomPosition) -> Option<u32>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
    {
        self.map(|pos_object| other_pos.get_range_to(&pos_object)).min()
    }

    fn find_nearest_linear_by<F, V>(self, other_pos: RoomPosition, pos_generator: F) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        F: Fn(&V) -> RoomPosition,
    {
        self.map(|pos_object| (other_pos.get_range_to(&pos_generator(&pos_object)), pos_object))
            .min_by_key(|(length, _)| *length)
            .map(|(_, pos_object)| pos_object)
    }

    fn find_nearest_linear_distance_by<F, V>(self, other_pos: RoomPosition, pos_generator: F) -> Option<u32>
    where
        Self: Iterator<Item = V> + Sized,
        F: Fn(&V) -> RoomPosition,
    {
        self.map(|pos_object| other_pos.get_range_to(&pos_generator(&pos_object))).min()
    }
}

impl<T: ?Sized> FindNearestItertools for T where T: Iterator {}
