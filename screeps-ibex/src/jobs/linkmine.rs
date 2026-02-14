use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::{self, *};
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_machine::*;
use serde::*;
use std::collections::HashSet;

#[derive(Clone, Serialize, Deserialize)]
pub struct LinkMineJobContext {
    mine_target: RemoteObjectId<Source>,
    link_target: RemoteObjectId<StructureLink>,
    container_target: Option<RemoteObjectId<StructureContainer>>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum LinkMineState {
        MoveToPosition { target: Position },
        Idle,
        Harvest,
        DepositLink,
        DepositContainer,
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

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
        tick_move_to_position(tick_context, self.target.into(), 0, None, LinkMineState::idle)
    }
}

impl Idle {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        // Link miner is at its mining position — use High priority so other
        // creeps repath around, but allow shoving as a last resort.
        movebehavior::mark_stationed(tick_context);

        let creep = tick_context.runtime_data.owner;

        if let Some(state) = get_new_harvest_target_state(
            creep,
            &state_context.mine_target,
            tick_context.action_flags.contains(SimultaneousActionFlags::TRANSFER),
            |_| LinkMineState::harvest(),
        ) {
            return Some(state);
        }

        if creep.store().get_used_capacity(Some(ResourceType::Energy)) > 0 {
            if let Some(link) = state_context.link_target.resolve() {
                let capacity = link.store().get_capacity(Some(ResourceType::Energy));
                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

                if used_capacity < capacity {
                    return Some(LinkMineState::deposit_link());
                }
            }

            if let Some(container) = state_context.container_target.and_then(|id| id.resolve()) {
                let capacity = container.store().get_capacity(None);
                let store_types = container.store().store_types();
                let used_capacity = store_types
                    .iter()
                    .map(|r| container.store().get_used_capacity(Some(*r)))
                    .sum::<u32>();

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
        let creep = tick_context.runtime_data.owner;
        let near_source = creep.pos().is_near_to(state_context.mine_target.pos());

        let result = tick_harvest(tick_context, state_context.mine_target, true, true, LinkMineState::idle);

        // Only override the movement request when the creep is actually at its
        // mining position. When out of range, tick_harvest issues a move_to
        // toward the source that must not be overwritten — otherwise the miner
        // will never walk back after being shoved away.
        if near_source {
            movebehavior::mark_stationed(tick_context);
        }

        result
    }
}

impl DepositLink {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        let creep = tick_context.runtime_data.owner;
        let near_link = creep.pos().is_near_to(state_context.link_target.pos());

        let result = tick_deposit_all_resources_state(tick_context, TransferTarget::Link(state_context.link_target), LinkMineState::idle);

        // Only mark stationed when in range; when out of range the deposit
        // function issues a move_to that must not be overwritten.
        if near_link {
            movebehavior::mark_stationed(tick_context);
        }

        result
    }
}

impl DepositContainer {
    pub fn tick(&mut self, state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        let Some(container_id) = state_context.container_target else {
            return Some(LinkMineState::idle());
        };

        let creep = tick_context.runtime_data.owner;
        let near_container = creep.pos().is_near_to(container_id.pos());

        let result = tick_deposit_all_resources_state(tick_context, TransferTarget::Container(container_id), LinkMineState::idle);

        // Only mark stationed when in range; when out of range the deposit
        // function issues a move_to that must not be overwritten.
        if near_container {
            movebehavior::mark_stationed(tick_context);
        }

        result
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &LinkMineJobContext, tick_context: &mut JobTickContext) -> Option<LinkMineState> {
        movebehavior::mark_stationed(tick_context);

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
    pub fn new(
        mine_target: RemoteObjectId<Source>,
        link_id: RemoteObjectId<StructureLink>,
        container_id: Option<RemoteObjectId<StructureContainer>>,
    ) -> LinkMineJob {
        const ONE_OFFSET_SQUARE: &[(i32, i32)] = &[(-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1)];

        let start_position = container_id.map(|container| container.pos()).or_else(|| {
            let mine_positions: HashSet<_> = ONE_OFFSET_SQUARE.iter().map(|offset| mine_target.pos() + *offset).collect();

            let link_positions: HashSet<_> = ONE_OFFSET_SQUARE.iter().map(|offset| link_id.pos() + *offset).collect();

            mine_positions
                .intersection(&link_positions)
                .filter(|position| {
                    position.pos().room_name() == mine_target.pos().room_name() && position.pos().room_name() == link_id.pos().room_name()
                })
                .filter(|position| {
                    if let Some(terrain) = game::map::get_room_terrain(position.room_name()) {
                        match terrain.get(position.x().u8(), position.y().u8()) {
                            Terrain::Plain => true,
                            Terrain::Wall => false,
                            Terrain::Swamp => true,
                        }
                    } else {
                        false
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
            state: initial_state,
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
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("LinkMine - {}", self.state.status_description()))
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.gather_data(system_data, runtime_data);
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data,
            runtime_data,
            action_flags: SimultaneousActionFlags::UNSET,
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }
}
