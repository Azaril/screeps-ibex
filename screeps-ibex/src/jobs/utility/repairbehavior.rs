use super::repair::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use screeps::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_repair_state<F, R>(creep: &Creep, build_room: &RoomData, minimum_priority: Option<RepairPriority>, state_map: F) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(room) = game::rooms::get(build_room.name) {
            if let Some(structure) = select_repair_structure(&room, minimum_priority, true) {
                return Some(state_map(RemoteStructureIdentifier::new(&structure)));
            }
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_repair<F, R>(tick_context: &mut JobTickContext, repair_structure_id: RemoteStructureIdentifier, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let target_position = repair_structure_id.pos();

    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let expect_resolve = if creep_pos.room_name() == target_position.room_name() {
        true
    } else {
        let target_room_entity = tick_context.runtime_data.mapping.get_room(&target_position.room_name())?;
        let target_room_data = tick_context.system_data.room_data.get(target_room_entity)?;

        target_room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false)
    };

    let repair_target = repair_structure_id.resolve();

    if let Some(repair_target) = repair_target.as_ref() {
        if let Some(attackable) = repair_target.as_attackable() {
            if attackable.hits() >= attackable.hits_max() {
                return Some(next_state());
            }
        }
    } else if expect_resolve {
        return Some(next_state());
    }

    if !creep_pos.in_range_to(&target_position, 3) {
        if !tick_context.action_flags.contains(SimultaneousActionFlags::MOVE) {
            tick_context.action_flags.insert(SimultaneousActionFlags::MOVE);

            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(3);
        }

        return None;
    }

    if let Some(structure) = repair_target.as_ref() {
        if let Some(attackable) = structure.as_attackable() {
            if attackable.hits() >= attackable.hits_max() {
                return Some(next_state());
            }
        }

        if !tick_context.action_flags.contains(SimultaneousActionFlags::REPAIR) {
            tick_context.action_flags.insert(SimultaneousActionFlags::REPAIR);

            match creep.repair(structure) {
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
