use crate::jobs::actions::*;
use crate::jobs::context::*;
use screeps::*;
use screeps_foreman::constants::*;
use screeps_rover::*;

/// Threshold (in ticks) beyond which stuck detection is reported to the caller
/// as a movement failure. Below this threshold, the movement system handles
/// recovery internally (repathing, avoiding creeps, etc.).
pub const STUCK_REPORT_THRESHOLD: u16 = 10;

/// Check the movement results from the previous tick for the current creep.
/// Returns `Some(())` if movement failed in a way that the job should handle
/// (e.g. path not found, stuck timeout). Returns `None` if movement is
/// proceeding normally or stuck recovery is still in progress.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn check_movement_failure(tick_context: &JobTickContext) -> Option<MovementFailure> {
    let entity = tick_context.runtime_data.creep_entity;
    let results = tick_context.runtime_data.movement_results;

    match results.get(&entity) {
        Some(MovementResult::Failed(failure)) => Some(failure.clone()),
        Some(MovementResult::Stuck { ticks }) if *ticks >= STUCK_REPORT_THRESHOLD => {
            Some(MovementFailure::StuckTimeout { ticks: *ticks })
        }
        _ => None,
    }
}

/// Register the creep as idle and freely shovable. Call this when a creep has
/// nothing to do (Wait/Idle states) and is just occupying a tile. The resolver
/// can push it anywhere to clear the way for other creeps.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_idle(tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder
        .range(0)
        .priority(MovementPriority::Low)
        .allow_shove(true)
        .allow_swap(true);
}

/// Register the creep as immovable at its current position. Call this when a
/// creep must stay on its exact tile (e.g. static miners on a container).
/// The movement resolver will never shove or swap this creep.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_immovable(tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder
        .range(0)
        .priority(MovementPriority::Immovable)
        .allow_shove(false)
        .allow_swap(false);
}

/// Register the creep as a stationary worker that prefers to stay put but may
/// be shoved or swapped to an adjacent tile as long as it remains within
/// `range` of `target_pos`. Use this for range-based workers (upgraders,
/// builders, repairers, etc.) so clustered creeps can rearrange without
/// deadlocking. The creep's work action (harvest, upgrade, build, etc.) is
/// not interrupted because MOVE and work intents use separate action slots.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_working(tick_context: &mut JobTickContext, target_pos: Position, range: u32) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder
        .range(0)
        .priority(MovementPriority::Low)
        .allow_shove(true)
        .allow_swap(true)
        .anchor(AnchorConstraint {
            position: target_pos,
            range,
        });
}

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
pub fn tick_move_to_room<F, R>(
    tick_context: &mut JobTickContext,
    room_name: RoomName,
    room_options: Option<RoomOptions>,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let room_half_width = ROOM_WIDTH as u32 / 2;
    let room_half_height = ROOM_HEIGHT as u32 / 2;
    let range = room_half_width.max(room_half_height) - 2;

    let target_pos = RoomPosition::new(room_half_width as u8, room_half_height as u8, room_name);

    tick_move_to_position(tick_context, target_pos, range, room_options, next_state)
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_move_to_position<F, R>(
    tick_context: &mut JobTickContext,
    position: RoomPosition,
    range: u32,
    room_options: Option<RoomOptions>,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    if creep.pos().in_range_to(position.clone().into(), range) {
        return Some(next_state());
    }

    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
        let mut builder = tick_context
            .runtime_data
            .movement
            .move_to(tick_context.runtime_data.creep_entity, position.into());

        builder.range(range);

        if let Some(room_options) = room_options {
            builder.room_options(room_options);
        }
    }

    None
}
