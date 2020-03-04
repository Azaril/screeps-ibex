use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[cfg_attr(feature = "time", timing)]
pub fn get_new_upgrade_state<F, R>(creep: &Creep, upgrade_room: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<StructureController>) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        let dynamic_visibility_data = upgrade_room.get_dynamic_visibility_data()?;

        if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
            let static_visibility_data = upgrade_room.get_static_visibility_data()?;
            let controller = static_visibility_data.controller()?;

            return Some(state_map(*controller));
        }
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn run_upgrade_state<F, R>(creep: &Creep, controller_id: &RemoteObjectId<StructureController>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep_pos = creep.pos();
    let target_position = controller_id.pos();

    //TODO: Check visibility cache and cancel if controller doesn't exist or isn't owned?

    if creep_pos.room_name() != target_position.room_name() {
        creep.move_to(&target_position);

        return None;
    }

    if let Some(controller) = controller_id.resolve() {
        if !creep_pos.in_range_to(&controller, 3) {
            creep.move_to(&target_position);

            return None;
        }

        match creep.upgrade_controller(&controller) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}
