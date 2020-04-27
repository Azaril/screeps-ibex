use screeps::*;
use screeps::game::map::*;

pub fn can_traverse_between_rooms(from: RoomName, to: RoomName) -> bool {
    let from_room_status = game::map::get_room_status(from);
    let to_room_status = game::map::get_room_status(to);

    can_traverse_between_room_status(&from_room_status, &to_room_status)
}

pub fn can_traverse_between_room_status(from: &MapRoomStatus, to: &MapRoomStatus) -> bool {
    match to.status {
        game::map::RoomStatus::Normal => from.status == game::map::RoomStatus::Normal,
        game::map::RoomStatus::Closed => false,
        game::map::RoomStatus::Novice => from.status == game::map::RoomStatus::Novice,
        game::map::RoomStatus::Respawn => from.status == game::map::RoomStatus::Respawn,
    }
}