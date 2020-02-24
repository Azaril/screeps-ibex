use crate::remoteobjectid::*;
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
