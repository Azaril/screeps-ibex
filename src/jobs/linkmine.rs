use super::jobsystem::*;
use crate::remoteobjectid::*;
use screeps::*;
use serde::*;
use crate::jobs::actions::*;
use super::utility::haulbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use crate::transfer::transfersystem::*;
use std::collections::HashSet;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum LinkMineState {
    MoveToPosition(RoomPosition),
    Idle(),
    Harvest(u8),
    DepositLink(),
    DepositContainer(),
    Wait(u32)
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct LinkMineJob {
    pub mine_target: RemoteObjectId<Source>,
    pub link_target: RemoteObjectId<StructureLink>,
    pub container_target: Option<RemoteObjectId<StructureContainer>>,
    state: LinkMineState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LinkMineJob {
    pub fn new(mine_target: RemoteObjectId<Source>, link_id: RemoteObjectId<StructureLink>, container_id: Option<RemoteObjectId<StructureContainer>>) -> LinkMineJob {
        const ONE_OFFSET_SQUARE: &[(i32, i32)] = &[(-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1)];

        let start_position = container_id.map(|container| container.pos())
            .or_else(|| {
                let mine_positions: HashSet<_> = ONE_OFFSET_SQUARE
                    .iter()
                    .map(|offset| mine_target.pos() + *offset)
                    .collect();
                    
                let link_positions: HashSet<_> = ONE_OFFSET_SQUARE
                    .iter()
                    .map(|offset| link_id.pos() + *offset)
                    .collect();
        
                mine_positions
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
                    .cloned()
                    .next()
            });

        let initial_state = if let Some(position) = start_position {
            LinkMineState::MoveToPosition(position)
        } else {
            LinkMineState::Idle()
        };

        LinkMineJob {
            mine_target,
            link_target: link_id,
            container_target: container_id,
            state: initial_state
        }
    }

    fn run_idle_state(creep: &Creep, action_flags: &mut SimultaneousActionFlags, harvest_target: &RemoteObjectId<Source>, link_target: &RemoteObjectId<StructureLink>, container_target: &Option<RemoteObjectId<StructureContainer>>) -> Option<LinkMineState> {
        if let Some(state) = get_new_harvest_target_state(creep, harvest_target, action_flags.contains(SimultaneousActionFlags::TRANSFER),  |_| LinkMineState::Harvest(0)) {
            return Some(state);
        }
        
        if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
            if let Some(link) = link_target.resolve() {
                let capacity = link.store_capacity(Some(ResourceType::Energy));
                let used_capacity = link.store_used_capacity(Some(ResourceType::Energy));

                if used_capacity < capacity {
                    return Some(LinkMineState::DepositLink());
                }
            }

            if let Some(container) = container_target.and_then(|id| id.resolve()) {
                let capacity = container.store_capacity(None);
                let store_types = container.store_types();
                let used_capacity = store_types.iter().map(|r| container.store_used_capacity(Some(*r))).sum::<u32>();

                if used_capacity < capacity {
                    return Some(LinkMineState::DepositContainer());
                }
            }
        }

        Some(LinkMineState::Wait(5))
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
                    LinkMineState::Idle() => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Idle", name), None);
                    }
                    LinkMineState::Harvest(_) => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Harvest", name), None);
                    }
                    LinkMineState::DepositLink() => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Deposit Link", name), None);
                    }
                    LinkMineState::DepositContainer() => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Deposit Container", name), None);
                    }
                    LinkMineState::Wait(_) => {
                        room_ui.jobs().add_text(format!("Link Mine - {} - Wait", name), None);
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
                LinkMineState::MoveToPosition(position) => run_move_to_position_state(creep, &mut action_flags, *position, LinkMineState::Idle), 
                LinkMineState::Idle() => Self::run_idle_state(creep, &mut action_flags, &self.mine_target, &self.link_target, &self.container_target),
                LinkMineState::Harvest(stuck_count) => run_harvest_state(creep, &mut action_flags, &self.mine_target, true, stuck_count, LinkMineState::Idle),
                LinkMineState::DepositLink() => run_deposit_all_resources_state(creep, &mut action_flags, TransferTarget::Link(self.link_target), LinkMineState::Idle),
                LinkMineState::DepositContainer() => run_deposit_all_resources_state(creep, &mut action_flags, TransferTarget::Container(self.container_target.unwrap()), LinkMineState::Idle),
                LinkMineState::Wait(time) => run_wait_state(time, LinkMineState::Idle)
            };

            if let Some(next_state) = state_result {
                self.state = next_state;
            } else {
                break;
            }
        }
    }
}
