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
    if creep.store_capacity(Some(ResourceType::Energy)) == 0 || creep.store_free_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(room) = game::rooms::get(dismantle_room.name) {
            //TODO: Don't collect here when range check is fixed.
            let structures: Vec<Structure> = get_dismantle_structures(room, ignore_storage).collect();

            let creep_pos = creep.pos();

            //TODO: Fix this hack which is a workaround for range of 1 pathfinding returning empty path.
            let mut best_structure = structures.iter().find(|s| s.pos().get_range_to(&creep_pos) <= 1).cloned();

            if best_structure.is_none() {
                best_structure = structures
                    .into_iter()
                    .find_nearest_from(creep_pos, PathFinderHelpers::same_room_ignore_creeps_range_1);
            }

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
    let creep = tick_context.runtime_data.owner;

    if creep.store_capacity(Some(ResourceType::Energy)) > 0 && creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
        return Some(next_state());
    }

    let creep_pos = creep.pos();
    let target_position = dismantle_structure_id.pos();

    let expect_resolve = if creep_pos.room_name() == target_position.room_name() {
        true
    } else {
        let target_room_entity = tick_context.runtime_data.mapping.get_room(&target_position.room_name())?;
        let target_room_data = tick_context.system_data.room_data.get(target_room_entity)?;

        target_room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false)
    };

    let dismantle_target = dismantle_structure_id.resolve();

    if let Some(dismantle_target) = dismantle_target.as_ref() {
        if let Some(attackable) = dismantle_target.as_attackable() {
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
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
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
