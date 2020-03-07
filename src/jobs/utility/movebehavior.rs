use screeps::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[cfg_attr(feature = "time", timing)]
pub fn run_move_to_room_state<F, R>(creep: &Creep, room_name: RoomName, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    if creep.room().map(|r| r.name() == room_name).unwrap_or(false) {
        return Some(next_state());
    }

    let target_pos = RoomPosition::new(25, 25, room_name);

    creep.move_to(&target_pos);

    None
}
