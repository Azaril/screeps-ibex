use screeps::*;
use serde::*;

use super::jobsystem::*;
use crate::remoteobjectid::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct ClaimJob {
    pub claim_target: RemoteObjectId<StructureController>,
}

impl ClaimJob {
    pub fn new(controller_id: RemoteObjectId<StructureController>) -> ClaimJob {
        ClaimJob {
            claim_target: controller_id,
        }
    }
}

impl Job for ClaimJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Claim - {}", name), None);
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("Claim Job - {}", creep.name());

        //
        // Claim controller.
        //

        if creep.pos().is_near_to(&self.claim_target.pos()) {
            if let Some(controller) = self.claim_target.resolve() {
                creep.claim_controller(&controller);
            } else {
                error!("Claim has no assigned controller! Name: {}", creep.name());
            }
        } else {
            creep.move_to(&self.claim_target.pos());
        }
    }
}
