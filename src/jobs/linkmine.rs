use super::jobsystem::*;
use crate::remoteobjectid::*;
use screeps::*;
use serde::*;
use crate::jobs::actions::*;
use super::utility::haulbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::movebehavior::*;
use crate::transfer::transfersystem::*;
use std::collections::HashSet;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum LinkMineState {
    MoveToPosition(RoomPosition),
    Harvest(u8),
    Deposit()
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct LinkMineJob {
    pub mine_target: RemoteObjectId<Source>,
    pub link_target: RemoteObjectId<StructureLink>,
    state: LinkMineState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LinkMineJob {
    pub fn new(mine_target: RemoteObjectId<Source>, link_id: RemoteObjectId<StructureLink>) -> LinkMineJob {
        const ONE_OFFSET_SQUARE: &[(i32, i32)] = &[(-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1)];

        let mine_positions: HashSet<_> = ONE_OFFSET_SQUARE
            .iter()
            .map(|offset| mine_target.pos() + *offset)
            .collect();
            
        let link_positions: HashSet<_> = ONE_OFFSET_SQUARE
            .iter()
            .map(|offset| link_id.pos() + *offset)
            .collect();

        let valid_position = mine_positions
            .intersection(&link_positions)
            .filter(|position| position.pos().room_name() == mine_target.pos().room_name() && position.pos().room_name() == link_id.pos().room_name())
            .filter(|position| {
                let terrain = game::map::get_room_terrain(position.room_name());

                match terrain.get(position.x(), position.y()) {
                    Terrain::Plain => true,
                    Terrain::Wall => false,
                    Terrain::Swamp => true,
                }
            })
            .next();

        let initial_state = if let Some(position) = valid_position {
            LinkMineState::MoveToPosition(*position)
        } else {
            LinkMineState::Harvest(0)
        };

        LinkMineJob {
            mine_target,
            link_target: link_id,
            state: initial_state
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for LinkMineJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                match &self.state {
                    LinkMineState::MoveToPosition(_) => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Move To Position", name), None);
                    }
                    LinkMineState::Harvest(_) => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Harvest", name), None);
                    }
                    LinkMineState::Deposit() => {
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
                LinkMineState::MoveToPosition(position) => run_move_to_position_state(creep, &mut action_flags, *position, || LinkMineState::Harvest(0)), 
                LinkMineState::Harvest(stuck_count) => run_harvest_state(creep, &mut action_flags, &self.mine_target, true, stuck_count, LinkMineState::Deposit),
                LinkMineState::Deposit() => run_deposit_all_resources_state(creep, &mut action_flags, TransferTarget::Link(self.link_target), || LinkMineState::Harvest(0)),
            };

            if let Some(next_state) = state_result {
                self.state = next_state;
            } else {
                break;
            }
        }
    }
}
