use screeps::*;

pub trait FindNearest<T: Sized + HasPosition> {
    //fn find_nearest<'a, F>(self, start_pos: &RoomPosition, find_options: FindOptions<'a, F>) -> Option<T> where Self: Sized, F: Fn(RoomName, CostMatrix<'_>) -> Option<CostMatrix<'a>>;

    fn find_nearest<F>(self, start_pos: &RoomPosition, generator: F) -> Option<T> where Self: Sized, F: Fn(&RoomPosition, &T) -> Path;
}

pub struct PathFinderHelpers;

impl PathFinderHelpers {
    pub fn same_room<T>(start_pos: &RoomPosition, pos_object: &T) -> Path where T: HasPosition {
        let find_options = FindOptions::new()
            .max_rooms(1)
            .ignore_creeps(true);

        start_pos.find_path_to(&pos_object.pos(), find_options)
    }
}

impl<I> FindNearest<I::Item> for I where I: Iterator, I::Item: HasPosition {
    fn find_nearest<F>(self, start_pos: &RoomPosition, generator: F) -> Option<I::Item> where F: Fn(&RoomPosition, &I::Item) -> Path {
        self.filter_map(|pos_object| {
            if let Path::Vectorized(path) = generator(&start_pos, &pos_object) {
                if !path.is_empty() {
                    //TODO: Check end point is actually target.
                    Some((path.len(), pos_object))   
                } else {
                    None
                }
            } else {
                None
            }
        }).min_by_key(|(length, _)| {
            *length
        }).map(|(_, pos_object)| {
            pos_object
        })
    }

    /*
    fn find_nearest<'a, F>(self, start_pos: &RoomPosition, find_options: FindOptions<'a, F>) -> Option<I::Item> where F: Fn(RoomName, CostMatrix<'_>) -> Option<CostMatrix<'a>> {
        self.filter_map(|pos_object| {
            if let Path::Vectorized(path) = start_pos.find_path_to(&pos_object.pos(), find_options.copy()) {
                if !path.is_empty() {
                    //TODO: Check end point is actually target.
                    Some((path.len(), pos_object))   
                } else {
                    None
                }
            } else {
                None
            }
        }).min_by_key(|(length, _)| {
            *length
        }).map(|(_, pos_object)| {
            pos_object
        })
    }
    */
}