use screeps::*;
use serde::*;

use super::jobsystem::*;
use crate::remoteobjectid::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct StaticMineJob {
    pub mine_target: RemoteObjectId<Source>,
    pub container_target: RemoteObjectId<StructureContainer>,
}

impl StaticMineJob {
    pub fn new(source_id: RemoteObjectId<Source>, container_id: RemoteObjectId<StructureContainer>) -> StaticMineJob {
        StaticMineJob {
            mine_target: source_id,
            container_target: container_id,
        }
    }
}

impl Job for StaticMineJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Static Mine - {}", name), None);
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("StaticMine Job - {}", creep.name());

        //
        // Harvest energy from source.
        //

        //TODO: Validate container still exists? Recyle or reuse miner if it doesn't?

        if creep.pos().is_equal_to(&self.container_target.pos()) {
            if let Some(source) = self.mine_target.resolve() {
                creep.harvest(&source);
            } else {
                error!("Harvester has no assigned harvesting source! Name: {}", creep.name());
            }
        } else {
            creep.move_to(&self.container_target.pos());
        }
    }
}
