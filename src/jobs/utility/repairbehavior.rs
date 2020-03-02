use super::repair::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use screeps::*;

pub fn get_new_repair_state<F, R>(creep: &Creep, build_room: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(room) = game::rooms::get(build_room.name) {
            if let Some(structure) = select_repair_structure(&room, creep.pos()) {
                return Some(state_map(RemoteStructureIdentifier::new(&structure)));
            }
        }
    }

    None
}

pub fn run_repair_state<F, R>(creep: &Creep, repair_structure_id: &RemoteStructureIdentifier, next_state: F) -> Option<R>
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

        if !creep.pos().is_near_to(&structure) {
            creep.move_to(&structure);

            return None;
        }

        match creep.repair(&structure) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}