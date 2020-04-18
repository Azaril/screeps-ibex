use super::jobsystem::*;
use crate::remoteobjectid::*;
use screeps::*;
use serde::*;
use super::actions::*;
use super::context::*;
use super::utility::haulbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use crate::transfer::transfersystem::*;
use std::collections::HashSet;
use screeps_machine::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct LinkMineJobContext {
    mine_target: RemoteObjectId<Source>,
    link_target: RemoteObjectId<StructureLink>,
    container_target: Option<RemoteObjectId<StructureContainer>>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum LinkMineState {
        MoveToPosition { target: RoomPosition },
        Idle,
        Harvest,
        DepositLink,
        DepositContainer,
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
            let room = { describe_data.owner.room() };

            if let Some(room) = room {
                let name = describe_data.owner.name();
                let room_name = room.name();

                describe_data
                    .ui
                    .with_room(room_name, &mut describe_data.visualizer, |room_ui| {
                        let description = self.status_description();

                        room_ui.jobs().add_text(format!("{} - {}", name, description), None);
                    });
            }
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}
        
        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}
        
        _ => fn tick(&mut self, state_context: &mut LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState>;
    }
);

impl MoveToPosition {
    pub fn tick(&mut self, _state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        tick_move_to_position(tick_context, self.target, 0, LinkMineState::idle)
    }
}

impl Idle {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        let creep = tick_context.runtime_data.owner;

        if let Some(state) = get_new_harvest_target_state(creep, &state_context.mine_target, tick_context.action_flags.contains(SimultaneousActionFlags::TRANSFER),  |_| LinkMineState::harvest()) {
            return Some(state);
        }
        
        if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
            if let Some(link) = state_context.link_target.resolve() {
                let capacity = link.store_capacity(Some(ResourceType::Energy));
                let used_capacity = link.store_used_capacity(Some(ResourceType::Energy));

                if used_capacity < capacity {
                    return Some(LinkMineState::deposit_link());
                }
            }

            if let Some(container) = state_context.container_target.and_then(|id| id.resolve()) {
                let capacity = container.store_capacity(None);
                let store_types = container.store_types();
                let used_capacity = store_types.iter().map(|r| container.store_used_capacity(Some(*r))).sum::<u32>();

                if used_capacity < capacity {
                    return Some(LinkMineState::deposit_container());
                }
            }
        }

        Some(LinkMineState::wait(5))
    }
}

impl Harvest {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        tick_harvest(tick_context, state_context.mine_target, true, true, LinkMineState::idle)
    }
}

impl DepositLink {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        tick_deposit_all_resources_state(tick_context, TransferTarget::Link(state_context.link_target), LinkMineState::idle)
    }
}

impl DepositContainer {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        tick_deposit_all_resources_state(tick_context, TransferTarget::Container(state_context.container_target.unwrap()), LinkMineState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &LinkMineJobContext, _tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        tick_wait(&mut self.ticks, LinkMineState::idle)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct LinkMineJob {
    context: LinkMineJobContext,
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
            LinkMineState::move_to_position(position)
        } else {
            LinkMineState::idle()
        };

        LinkMineJob {
            context: LinkMineJobContext {
                mine_target,
                link_target: link_id,
                container_target: container_id,
            },
            state: initial_state
        }
    }

    pub fn get_mine_target(&self) -> &RemoteObjectId<Source> {
        &self.context.mine_target
    }

    pub fn get_link_target(&self) -> &RemoteObjectId<StructureLink> {
        &self.context.link_target
    }

    pub fn get_container_target(&self) -> &Option<RemoteObjectId<StructureContainer>> {
        &self.context.container_target
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for LinkMineJob {
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        self.state.describe(system_data, describe_data);
        self.state.visualize(system_data, describe_data);
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.gather_data(system_data, runtime_data);
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data,
            runtime_data,
            action_flags: SimultaneousActionFlags::UNSET
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }

    /*
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
    */
}
