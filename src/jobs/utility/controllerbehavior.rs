use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;

pub struct ControllerBehaviorUtility;

impl ControllerBehaviorUtility {
    pub fn upgrade_controller(creep: &Creep, controller: &StructureController) {
        scope_timing!("upgrade_controller");

        if creep.pos().is_near_to(controller) {
            creep.upgrade_controller(controller);
        } else {
            creep.move_to(controller);
        }
    }

    pub fn upgrade_controller_id(creep: &Creep, controller_id: &RemoteObjectId<StructureController>) {
        let target_position = controller_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        } else if let Some(controller) = controller_id.resolve() {
            //TODO: Handle error code.
            Self::upgrade_controller(creep, &controller)
        } else {
            //TODO: Return error result.
            error!("Failed to resolve controller for upgrading. Name: {}", creep.name());
        }
    }
}

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
        if !creep_pos.is_near_to(&controller) {
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
