use screeps::*;
use crate::jobs::actions::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_move_to_room_state<F, R>(creep: &Creep, room_name: RoomName, state_map: F) -> Option<R>
where
    F: Fn(RoomName) -> R,
{
    if creep.pos().room_name() != room_name {
        return Some(state_map(room_name));
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn run_move_to_position_state<F, R>(creep: &Creep, action_flags: &mut SimultaneousActionFlags, position: RoomPosition, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    if creep.pos() == position {
        return Some(next_state());
    }

    if !action_flags.contains(SimultaneousActionFlags::MOVE) {
        action_flags.insert(SimultaneousActionFlags::MOVE);

        //TODO: What to do with failure here?
        creep.move_to(&position);
    }

    None
}
