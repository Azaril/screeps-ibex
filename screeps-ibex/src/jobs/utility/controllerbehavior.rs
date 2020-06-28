use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_upgrade_state<F, R>(creep: &Creep, upgrade_room: &RoomData, state_map: F, max_rcl: Option<u32>) -> Option<R>
where
    F: Fn(RemoteObjectId<StructureController>) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        let dynamic_visibility_data = upgrade_room.get_dynamic_visibility_data()?;

        if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
            let static_visibility_data = upgrade_room.get_static_visibility_data()?;
            let controller_id = static_visibility_data.controller()?;
            let controller = controller_id.resolve()?;

            if max_rcl.map(|max_rcl| controller.level() < max_rcl).unwrap_or(true) {
                return Some(state_map(*controller_id));
            }
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_sign_state<F, R>(sign_room: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<StructureController>) -> R,
{
    let dynamic_visibility_data = sign_room.get_dynamic_visibility_data()?;

    if dynamic_visibility_data.updated_within(1000) && dynamic_visibility_data.sign().as_ref().map(|s| !s.user().mine()).unwrap_or(true) {
        let static_visibility_data = sign_room.get_static_visibility_data()?;
        let controller = static_visibility_data.controller()?;

        return Some(state_map(*controller));
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_upgrade<F, R>(tick_context: &mut JobTickContext, controller_id: RemoteObjectId<StructureController>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;
    let action_flags = &mut tick_context.action_flags;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or isn't owned?

    if !creep_pos.in_range_to(&target_position, 3) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(3);
        }

        return None;
    }

    if let Some(controller) = controller_id.resolve() {
        match creep.upgrade_controller(&controller) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_claim<F, R>(tick_context: &mut JobTickContext, controller_id: RemoteObjectId<StructureController>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;
    let action_flags = &mut tick_context.action_flags;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or is owned?

    if !creep_pos.is_near_to(&target_position) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
        }

        return None;
    }

    if let Some(controller) = controller_id.resolve() {
        match creep.claim_controller(&controller) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_reserve<F, R>(tick_context: &mut JobTickContext, controller_id: RemoteObjectId<StructureController>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;
    let action_flags = &mut tick_context.action_flags;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or is owned?

    if !creep_pos.is_near_to(&target_position) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
        }

        return None;
    }

    if let Some(controller) = controller_id.resolve() {
        if let Some(reservation) = controller.reservation() {
            let body = creep.body();
            let claim_parts = body.iter().filter(|b| b.part == Part::Claim).count();
            let claim_amount = claim_parts as u32 * CONTROLLER_RESERVE;

            if reservation.ticks_to_end + claim_amount > CONTROLLER_RESERVE_MAX {
                return Some(next_state());
            }
        }

        match creep.reserve_controller(&controller) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_sign<F, R>(
    tick_context: &mut JobTickContext,
    controller_id: RemoteObjectId<StructureController>,
    message: &str,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;
    let action_flags = &mut tick_context.action_flags;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or is owned?

    if !creep_pos.is_near_to(&target_position) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
        }

        return None;
    }

    if let Some(controller) = controller_id.resolve() {
        if action_flags.consume(SimultaneousActionFlags::SIGN) {
            creep.sign_controller(&controller, message);
        }
    }

    Some(next_state())
}
