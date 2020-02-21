use screeps::*;

pub trait FindNearest<T: Sized + HasPosition> {
    fn find_nearest_from<F>(self, start_pos: RoomPosition, generator: F) -> Option<T>
    where
        Self: Sized,
        F: Fn(RoomPosition, RoomPosition) -> Path;

    fn find_nearest_path_from<F>(self, start_pos: RoomPosition, generator: F) -> Option<Path>
    where
        Self: Sized,
        F: Fn(RoomPosition, RoomPosition) -> Path;

    fn find_nearest_to<F>(self, end_pos: RoomPosition, generator: F) -> Option<T>
    where
        Self: Sized,
        F: Fn(RoomPosition, RoomPosition) -> Path;

    fn find_nearest_path_to<F>(self, end_pos: RoomPosition, generator: F) -> Option<Path>
    where
        Self: Sized,
        F: Fn(RoomPosition, RoomPosition) -> Path;

    fn find_nearest_linear(self, other_pos: RoomPosition) -> Option<T>
    where
        Self: Sized;
}

pub struct PathFinderHelpers;

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

    pub fn same_room_ignore_creeps_and_structures(
        start_pos: RoomPosition,
        end_pos: RoomPosition,
    ) -> Path {
        let find_options = FindOptions::new()
            .max_rooms(1)
            .ignore_creeps(true)
            .ignore_destructible_structures(true);

        start_pos.find_path_to(&end_pos, find_options)
    }

    pub fn same_room_ignore_creeps_and_structures_range_1(
        start_pos: RoomPosition,
        end_pos: RoomPosition,
    ) -> Path {
        let find_options = FindOptions::new()
            .max_rooms(1)
            .ignore_creeps(true)
            .ignore_destructible_structures(true)
            .range(1);

        start_pos.find_path_to(&end_pos, find_options)
    }
}

impl<I> FindNearest<I::Item> for I
where
    I: Iterator,
    I::Item: HasPosition,
{
    fn find_nearest_from<F>(self, start_pos: RoomPosition, generator: F) -> Option<I::Item>
    where
        F: Fn(RoomPosition, RoomPosition) -> Path,
    {
        self.filter_map(|pos_object| {
            if let Path::Vectorized(path) = generator(start_pos, pos_object.pos()) {
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

    fn find_nearest_path_from<F>(self, start_pos: RoomPosition, generator: F) -> Option<Path>
    where
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

    fn find_nearest_to<F>(self, end_pos: RoomPosition, generator: F) -> Option<I::Item>
    where
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

    fn find_nearest_path_to<F>(self, end_pos: RoomPosition, generator: F) -> Option<Path>
    where
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

    fn find_nearest_linear(self, other_pos: RoomPosition) -> Option<I::Item> {
        self.map(|pos_object| (other_pos.get_range_to(&pos_object), pos_object))
            .min_by_key(|(length, _)| *length)
            .map(|(_, pos_object)| pos_object)
    }
}
