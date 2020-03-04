use screeps::*;
use serde::*;
#[cfg(feature = "time")]
use timing_annotate::*;
use super::jobsystem::*;
use crate::remoteobjectid::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct ReserveJob {
    pub reserve_target: RemoteObjectId<StructureController>,
}

#[cfg_attr(feature = "time", timing)]
impl ReserveJob {
    pub fn new(controller_id: RemoteObjectId<StructureController>) -> ReserveJob {
        ReserveJob {
            reserve_target: controller_id,
        }
    }
}

#[cfg_attr(feature = "time", timing)]
impl Job for ReserveJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Reserve - {}", name), None);
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        //
        // Reserver controller.
        //

        if creep.pos().is_near_to(&self.reserve_target.pos()) {
            if let Some(controller) = self.reserve_target.resolve() {
                creep.reserve_controller(&controller);
            } else {
                error!("Reserver has no assigned controller! Name: {}", creep.name());
            }
        } else {
            creep.move_to(&self.reserve_target.pos());
        }
    }
}
