use crate::jobs::actions::*;
use crate::jobs::context::*;
use screeps::*;
use screeps_foreman::constants::*;
use screeps_rover::*;

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
pub fn tick_move_to_room<F, R>(tick_context: &mut JobTickContext, room_name: RoomName, room_options: Option<RoomOptions>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let room_half_width = ROOM_WIDTH as u32 / 2;
    let room_half_height = ROOM_HEIGHT as u32 / 2;
    let range = room_half_width.max(room_half_height) - 2;

    let target_pos = RoomPosition::new(room_half_width, room_half_height, room_name);

    tick_move_to_position(tick_context, target_pos, range, room_options, next_state)
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_move_to_position<F, R>(tick_context: &mut JobTickContext, position: RoomPosition, range: u32, room_options: Option<RoomOptions>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    if creep.pos().in_range_to(&position, range) {
        return Some(next_state());
    }

    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
        //TODO: What to do with failure here?
        let mut builder = tick_context
            .runtime_data
            .movement
            .move_to(tick_context.runtime_data.creep_entity, position);

        builder.range(range);

        if let Some(room_options) = room_options {
            builder.room_options(room_options);
        }
    }

    None
}
