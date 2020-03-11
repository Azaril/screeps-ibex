use super::repair::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use screeps::*;
#[cfg(feature = "time")]
use timing_annotate::*;
use crate::jobs::actions::*;

#[cfg_attr(feature = "time", timing)]
pub fn get_new_repair_state<F, R>(creep: &Creep, build_room: &RoomData, minimum_priority: Option<RepairPriority>, state_map: F) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(room) = game::rooms::get(build_room.name) {
            if let Some(structure) = select_repair_structure(&room, creep.pos(), minimum_priority) {
                return Some(state_map(RemoteStructureIdentifier::new(&structure)));
            }
        }
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn run_repair_state<F, R>(creep: &Creep, action_flags: &mut SimultaneousActionFlags, repair_structure_id: &RemoteStructureIdentifier, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    //TODO: Check visibility cache.

    if let Some(structure) = repair_structure_id.resolve() {
        if let Some(attackable) = structure.as_attackable() {
            if attackable.hits() >= attackable.hits_max() {
                return Some(next_state());
            }
        }

        if !creep.pos().in_range_to(&structure, 3) {
            if !action_flags.contains(SimultaneousActionFlags::MOVE) {
                action_flags.insert(SimultaneousActionFlags::MOVE);

                creep.move_to(&structure);
            }

            return None;
        }

        if !action_flags.contains(SimultaneousActionFlags::REPAIR) {
            action_flags.insert(SimultaneousActionFlags::REPAIR);
            match creep.repair(&structure) {
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
