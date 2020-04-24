use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use crate::findnearest::*;
use super::dismantle::*;
use screeps::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_dismantle_state<F, R>(creep: &Creep, dismantle_room: &RoomData, ignore_storage: bool, state_map: F) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    //TODO: Add bypass for energy check.
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(room) = game::rooms::get(dismantle_room.name) {
            let best_structure = get_dismantle_structures(room, ignore_storage)
                .find_nearest_from(creep.pos(), PathFinderHelpers::same_room_ignore_creeps_range_1);

            if let Some(structure) = best_structure {
                return Some(state_map(RemoteStructureIdentifier::new(&structure)));
            }
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_dismantle<F, R>(tick_context: &mut JobTickContext, dismantle_structure_id: RemoteStructureIdentifier, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let target_position = dismantle_structure_id.pos();

    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let expect_resolve = if creep_pos.room_name() == target_position.room_name() {
        true
    } else {
        let target_room_entity = tick_context.runtime_data.mapping.rooms.get(&target_position.room_name())?;
        let target_room_data = tick_context.system_data.room_data.get(*target_room_entity)?;

        target_room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false)
    };

    let dismantle_target = dismantle_structure_id.resolve();

    if let Some(repair_target) = dismantle_target.as_ref() {
        if let Some(attackable) = repair_target.as_attackable() {
            if attackable.hits() == 0 {
                return Some(next_state());
            }
        }
    } else if expect_resolve {
        return Some(next_state());
    }

    if !creep_pos.in_range_to(&target_position, 1) {
        if !tick_context.action_flags.contains(SimultaneousActionFlags::MOVE) {
            tick_context.action_flags.insert(SimultaneousActionFlags::MOVE);

            tick_context
                .runtime_data
                .movement
                .move_to_range(tick_context.runtime_data.creep_entity, target_position, 1);
        }

        return None;
    }

    if let Some(structure) = dismantle_target.as_ref() {
        if !tick_context.action_flags.contains(SimultaneousActionFlags::DISMANTLE) {
            tick_context.action_flags.insert(SimultaneousActionFlags::DISMANTLE);

            match creep.dismantle(structure) {
                ReturnCode::Ok => None,
                _ => Some(next_state()),
            }
        } else {
            None
        }
    } else {
        Some(next_state())
    }
}
