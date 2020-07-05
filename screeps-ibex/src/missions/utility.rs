use crate::room::data::*;

pub fn is_valid_home_room(room_data: &RoomData) -> bool {
    if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
        if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
            return true;
        }
    }

    false
}