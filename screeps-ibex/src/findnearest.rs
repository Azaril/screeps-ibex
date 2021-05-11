use screeps::*;
use screeps_rover::{CostMatrixOptions, CostMatrixSystem};
use std::ops::*;

pub struct PathFinderHelpers;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl PathFinderHelpers {
    /*
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
    */
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait FindNearestItertools: Iterator {
    fn find_nearest_from<V>(
        self,
        start_pos: Position,
        range: u32,
        cost_matrix_system: &mut CostMatrixSystem,
        cost_matrix_options: &CostMatrixOptions,
    ) -> Option<V>
    where
        Self: Iterator<Item = V> + Sized,
        V: HasPosition,
    {
        //TODO: Add max rooms!
        //TODO: Add max ops!

        //TODO: Add search options customization?
        let search_options = SearchOptions::new()
            //.max_rooms(1)
            //.max_ops(max_ops)
            .plain_cost(cost_matrix_options.plains_cost)
            .swamp_cost(cost_matrix_options.swamp_cost)
            .room_callback(|room_name: RoomName| -> MultiRoomCostResult {
                let mut cost_matrix = CostMatrix::new();

                match cost_matrix_system.apply_cost_matrix(room_name, &mut cost_matrix, &cost_matrix_options) {
                    Ok(()) => MultiRoomCostResult::CostMatrix(cost_matrix),
                    Err(_err) => MultiRoomCostResult::Impassable,
                }
            });

        let positions: Vec<_> = self
            .map(|item| {
                let pos = item.pos();

                (item, pos)
            })
            .collect();

        let search_goals = positions.iter().map(|(_, pos)| SearchGoal::new(*pos, range));

        let search_result = pathfinder::search_many(start_pos.into(), search_goals, Some(search_options));

        if !search_result.incomplete() {
            let path = search_result.path();

            if let Some(goal_pos) = path.last() {
                return positions
                    .into_iter()
                    .filter_map(|(item, pos)| if pos == *goal_pos { Some(item) } else { None })
                    .next();
            }
        }

        None
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
