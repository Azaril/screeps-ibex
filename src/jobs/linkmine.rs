use super::jobsystem::*;
use crate::remoteobjectid::*;
use screeps::*;
use serde::*;
#[cfg(feature = "time")]
use timing_annotate::*;
use crate::jobs::actions::*;
use super::utility::haulbehavior::*;
use super::utility::harvestbehavior::*;
use crate::transfer::transfersystem::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum LinkMineState {
    Harvest(u8),
    Deposit
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct LinkMineJob {
    pub mine_target: RemoteObjectId<Source>,
    pub link_target: RemoteObjectId<StructureLink>,
    state: LinkMineState,
}

#[cfg_attr(feature = "time", timing)]
impl LinkMineJob {
    pub fn new(mine_target: RemoteObjectId<Source>, link_id: RemoteObjectId<StructureLink>) -> LinkMineJob {
        LinkMineJob {
            mine_target,
            link_target: link_id,
            state: LinkMineState::Harvest(0)
        }
    }
}

#[cfg_attr(feature = "time", timing)]
impl Job for LinkMineJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                match &self.state {
                    LinkMineState::Harvest(_) => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Harvest", name), None);
                    }
                    LinkMineState::Deposit => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Deposit", name), None);
                    }
                };
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        let mut action_flags = SimultaneousActionFlags::UNSET;

        loop {
            let state_result = match &mut self.state {
                LinkMineState::Harvest(stuck_count) => run_harvest_state(creep, &mut action_flags, &self.mine_target, true, stuck_count, || LinkMineState::Deposit),
                LinkMineState::Deposit => run_deposit_all_resources_state(creep, &mut action_flags, TransferTarget::Link(self.link_target), || LinkMineState::Harvest(0)),
            };

            if let Some(next_state) = state_result {
                self.state = next_state;
            } else {
                break;
            }
        }
    }
}
