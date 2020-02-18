use screeps::*;
use serde::*;

use super::jobsystem::*;
use crate::remoteobjectid::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct ReserveJob {
    pub reserve_target: RemoteObjectId<StructureController>,
}

impl ReserveJob {
    pub fn new(controller_id: RemoteObjectId<StructureController>) -> ReserveJob {
        ReserveJob {
            reserve_target: controller_id,
        }
    }
}

impl Job for ReserveJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Reserve Job - {}", creep.name());

        //
        // Reserver controller.
        //

        //TODO: Validate container still exists? Recyle or reuse miner if it doesn't?

        if creep.pos().is_near_to(&self.reserve_target.pos()) {
            if let Some(controller) = self.reserve_target.resolve() {
                creep.reserve_controller(&controller);
            } else {
                error!(
                    "Reserver has no assigned controller! Name: {}",
                    creep.name()
                );
            }
        } else {
            creep.move_to(&self.reserve_target.pos());
        }
    }
}
