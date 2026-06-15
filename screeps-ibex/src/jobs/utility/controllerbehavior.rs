use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::utility::movebehavior::mark_working;
use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;
use screeps_rover::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_upgrade_state<F, R>(creep: &Creep, upgrade_room: &RoomData, state_map: F, max_rcl: Option<u32>) -> Option<R>
where
    F: Fn(RemoteObjectId<StructureController>) -> R,
{
    if creep.store().get_used_capacity(Some(ResourceType::Energy)) > 0 {
        let dynamic_visibility_data = upgrade_room.get_dynamic_visibility_data()?;

        if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
            let static_visibility_data = upgrade_room.get_static_visibility_data()?;
            let controller_id = static_visibility_data.controller()?;
            let controller = controller_id.resolve()?;

            if max_rcl.map(|max_rcl| (controller.level() as u32) < max_rcl).unwrap_or(true) {
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

/// True when this tick's upgrade will exhaust the creep's energy, so without a
/// parallel refill the creep would spend next tick idle and empty before it
/// could withdraw. Energy/free are the start-of-tick snapshot, unaffected by
/// the upgrade intent already issued this tick.
fn upgrade_about_to_run_dry(creep: &Creep) -> bool {
    let energy = creep.store().get_used_capacity(Some(ResourceType::Energy));
    // Safe on general stores (engine-mechanics folklore row 26).
    let free = creep.store().get_free_capacity(Some(ResourceType::Energy)).max(0) as u32;

    // Empty already takes the Err path below; a full creep has nothing to refill into.
    if energy == 0 || free == 0 {
        return false;
    }

    let work_parts = creep.body().iter().filter(|p| p.part() == Part::Work).count() as u32;
    let per_tick = work_parts.saturating_mul(UPGRADE_CONTROLLER_POWER).max(1);

    energy <= per_tick
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_upgrade<F, R>(
    tick_context: &mut JobTickContext,
    controller_id: RemoteObjectId<StructureController>,
    refill_when_draining: bool,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or isn't owned?

    if !creep_pos.in_range_to(target_position, 3) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(3);
        }

        return None;
    }

    // In range — mark as working so the resolver may rearrange upgraders
    // within range 3 of the controller to resolve clustering deadlocks.
    mark_working(tick_context, target_position, 3);

    if tick_context.action_flags.consume(SimultaneousActionFlags::UPGRADE_CONTROLLER) {
        if let Some(controller) = controller_id.resolve() {
            match creep.upgrade_controller(&controller) {
                // The upgrade intent (pipeline E) succeeded this tick. If the creep
                // is about to run dry, keep the state-machine cascade going so the
                // refill pickup's withdraw (pipeline D) is issued THIS tick rather
                // than next tick — eliminating the idle tick the creep would
                // otherwise spend empty before refilling. When a withdrawable
                // source is adjacent (the common stationary case) the withdraw
                // rides along with no move; otherwise this just starts the refill
                // trip one tick early, exactly as the dry-tick path did before.
                Ok(()) => {
                    if refill_when_draining && upgrade_about_to_run_dry(creep) {
                        Some(next_state())
                    } else {
                        None
                    }
                }
                Err(_) => Some(next_state()),
            }
        } else {
            Some(next_state())
        }
    } else {
        None
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_claim<F, R>(tick_context: &mut JobTickContext, controller_id: RemoteObjectId<StructureController>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or is owned?

    if !creep_pos.is_near_to(target_position) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
        }

        return None;
    }

    // In range — mark as working within range 1 of the controller.
    mark_working(tick_context, target_position, 1);

    if let Some(controller) = controller_id.resolve() {
        // If the controller has a hostile reservation, attack it to burn down
        // the reservation ticks before attempting to claim. claimController()
        // fails on reserved controllers.
        if controller.reservation().is_some() {
            let _ = creep.attack_controller(&controller);
            return None;
        }

        match creep.claim_controller(&controller) {
            Ok(()) => None,
            Err(_) => Some(next_state()),
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

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or is owned?

    if !creep_pos.is_near_to(target_position) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
        }

        return None;
    }

    // In range — mark as working within range 1 of the controller.
    mark_working(tick_context, target_position, 1);

    if let Some(controller) = controller_id.resolve() {
        if let Some(reservation) = controller.reservation() {
            let body = creep.body();
            let claim_parts = body.iter().filter(|b| b.part() == Part::Claim).count();
            let claim_amount = claim_parts as u32 * CONTROLLER_RESERVE;

            if reservation.ticks_to_end() + claim_amount > CONTROLLER_RESERVE_MAX {
                return Some(next_state());
            }
        }

        match creep.reserve_controller(&controller) {
            Ok(()) => None,
            Err(_) => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}

/// De-claim a hostile-owned controller: move to range 1 and `attackController`
/// to knock down its downgrade clock (−300/CLAIM part per strike, one strike
/// per 1000 ticks; engine-mechanics §2.12). Used by salvage de-claimers to
/// neutralize a derelict room's controller so the waiting mining outpost can
/// take it over. Routes through the (confirmed-derelict, hence passable)
/// target room with `HighCost` like the dismantler.
///
/// Yields `next_state` (a short wait) after a strike, when the controller is
/// upgrade-blocked by a recent strike, or once it is no longer owned/reserved
/// (de-claim achieved — the mission then retires the role); returns `None`
/// while still travelling.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_attack_controller<F, R>(
    tick_context: &mut JobTickContext,
    controller_id: RemoteObjectId<StructureController>,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    if !creep_pos.is_near_to(target_position) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1)
                .room_options(RoomOptions::new(HostileBehavior::HighCost));
        }

        return None;
    }

    // In range — mark as working within range 1 of the controller.
    mark_working(tick_context, target_position, 1);

    if let Some(controller) = controller_id.resolve() {
        // Nothing left to do if the controller is already neutral (de-claim
        // achieved), or upgrade-blocked by a strike within the last 1000 ticks
        // (a further attackController would just be rejected — don't spend the
        // intent).
        let owned_or_reserved = controller.owner().is_some() || controller.reservation().is_some();
        let upgrade_blocked = controller.upgrade_blocked().unwrap_or(0) > 0;

        if !owned_or_reserved || upgrade_blocked {
            return Some(next_state());
        }

        if tick_context.action_flags.consume(SimultaneousActionFlags::ATTACK_CONTROLLER) {
            let _ = creep.attack_controller(&controller);
        }

        Some(next_state())
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

    if !creep_pos.is_near_to(target_position) {
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
            let _ = creep.sign_controller(&controller, message);
        }
    }

    Some(next_state())
}
