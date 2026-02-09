use crate::room::data::*;
use screeps::*;

pub fn is_valid_home_room(room_data: &RoomData) -> bool {
    if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
        if dynamic_visibility_data.visible() {
            if dynamic_visibility_data.owner().mine() {
                return true;
            }

            if room_data
                .get_structures()
                .map(|structures| structures.spawns().iter().any(|spawn| spawn.my()))
                .unwrap_or(false)
            {
                return true;
            }
        }
    }

    false
}
