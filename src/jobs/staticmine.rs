use super::jobsystem::*;
use crate::remoteobjectid::*;
use screeps::*;
use serde::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub enum StaticMineTarget {
    Source(RemoteObjectId<Source>),
    Mineral(RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>),
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct StaticMineJob {
    pub mine_target: StaticMineTarget,
    pub container_target: RemoteObjectId<StructureContainer>,
}

impl StaticMineJob {
    pub fn new(mine_target: StaticMineTarget, container_id: RemoteObjectId<StructureContainer>) -> StaticMineJob {
        StaticMineJob {
            mine_target,
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

        //
        // Harvest energy from source.
        //

        //TODO: Validate container still exists? Recyle or reuse miner if it doesn't?

        if creep.pos().is_equal_to(&self.container_target.pos()) {
            match self.mine_target {
                StaticMineTarget::Source(source_id) => {
                    if let Some(source) = source_id.resolve() {
                        creep.harvest(&source);
                    } else {
                        error!("Harvester has no assigned harvesting source! Name: {}", creep.name());
                    }
                }
                StaticMineTarget::Mineral(mineral_id, extractor_id) => {
                    if let Some(extractor) = extractor_id.resolve() {
                        if extractor.cooldown() == 0 {
                            if let Some(mineral) = mineral_id.resolve() {
                                creep.harvest(&mineral);
                            } else {
                                error!("Harvester has no assigned harvesting extractor! Name: {}", creep.name());
                            }
                        }
                    }
                }
            }
        } else {
            creep.move_to(&self.container_target.pos());
        }
    }
}
