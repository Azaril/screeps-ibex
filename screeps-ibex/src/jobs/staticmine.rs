use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use super::utility::repair::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use screeps::*;
use screeps_machine::*;
use serde::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum StaticMineTarget {
    #[serde(rename = "s")]
    Source(RemoteObjectId<Source>),
    #[serde(rename = "m")]
    Mineral(RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>),
}

impl StaticMineTarget {
    fn pos(&self) -> Position {
        match self {
            StaticMineTarget::Source(id) => id.pos(),
            StaticMineTarget::Mineral(id, _) => id.pos(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StaticMineJobContext {
    pub mine_target: StaticMineTarget,
    pub container_target: RemoteObjectId<StructureContainer>,
}

impl StaticMineJobContext {
    /// Try to find a container adjacent to the mine target using live room
    /// structure data. Returns `true` if the container_target was updated.
    fn try_rediscover_container(&mut self, tick_context: &JobTickContext) -> bool {
        let mine_pos = self.mine_target.pos();
        let room_name = mine_pos.room_name();

        let room_entity = match tick_context.runtime_data.mapping.get_room(&room_name) {
            Some(e) => e,
            None => return false,
        };

        let room_data = match tick_context.system_data.room_data.get(room_entity) {
            Some(d) => d,
            None => return false,
        };

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return false,
        };

        if let Some(container) = structures.containers().iter().find(|c| c.pos().is_near_to(mine_pos)) {
            self.container_target = container.remote_id();
            true
        } else {
            false
        }
    }

    /// Returns `true` when the container exists and the creep is not standing
    /// on it (i.e. it has been shoved off its assigned tile).
    fn is_displaced(&self, creep: &Creep) -> bool {
        self.container_target.resolve().is_some() && !creep.pos().is_equal_to(self.container_target.pos())
    }

    /// Issue a movement request to walk back to the container tile. Call this
    /// when the creep is displaced but still performing its action (harvest)
    /// so it moves back without wasting a tick.
    fn move_to_container(&self, tick_context: &mut JobTickContext) {
        tick_context
            .runtime_data
            .movement
            .move_to(tick_context.runtime_data.creep_entity, self.container_target.pos())
            .range(0);
    }
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum StaticMineState {
        MoveToContainer,
        Harvest,
        Wait { ticks: u32 },
        FindContainer { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState>;
    }
);

impl MoveToContainer {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        // Verify the container still exists before walking to it.
        if state_context.container_target.resolve().is_none() {
            // Container is gone — try to find a replacement near the mine target.
            if !state_context.try_rediscover_container(tick_context) {
                // No container available — harvest at the source anyway so
                // energy isn't wasted (it regenerates on a timer regardless).
                return Some(StaticMineState::harvest());
            }
            // Found a new container — fall through to move to it.
        }

        tick_move_to_position(
            tick_context,
            state_context.container_target.pos().into(),
            0,
            None,
            StaticMineState::harvest,
        )
    }
}

/// Try to harvest the mine target. Returns `None` if the harvest action was
/// issued (or the action flag was already consumed), or `Some` to transition
/// when the resource is depleted or not visible.
fn try_harvest_mine_target(creep: &Creep, mine_target: &StaticMineTarget, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
    match *mine_target {
        StaticMineTarget::Source(source_id) => {
            tick_opportunistic_repair(tick_context, Some(RepairPriority::Low));

            if let Some(source) = source_id.resolve() {
                if source.energy() == 0 {
                    return Some(StaticMineState::wait(1));
                }

                if tick_context.action_flags.consume(SimultaneousActionFlags::HARVEST) {
                    match creep.harvest(&source) {
                        Ok(()) => None,
                        Err(_) => Some(StaticMineState::wait(1)),
                    }
                } else {
                    None
                }
            } else {
                Some(StaticMineState::wait(1))
            }
        }
        StaticMineTarget::Mineral(mineral_id, _) => {
            if let Some(mineral) = mineral_id.resolve() {
                if mineral.mineral_amount() == 0 {
                    return Some(StaticMineState::wait(1));
                }

                if tick_context.action_flags.consume(SimultaneousActionFlags::HARVEST) {
                    match creep.harvest(&mineral) {
                        Ok(()) => None,
                        Err(_) => Some(StaticMineState::wait(1)),
                    }
                } else {
                    None
                }
            } else {
                Some(StaticMineState::wait(1))
            }
        }
    }
}

impl Harvest {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        let creep = tick_context.runtime_data.owner;
        let container_exists = state_context.container_target.resolve().is_some();

        if container_exists {
            let displaced = state_context.is_displaced(creep);

            if displaced {
                // Shoved off the container — issue a move back. The harvest
                // action (HARVEST and MOVE are independent intents) can still
                // fire this tick as long as we're in range of the mine target,
                // so the creep is productive while walking back.
                state_context.move_to_container(tick_context);
            } else {
                // On the container — stay put with high priority so other
                // creeps repath around.
                mark_stationed(tick_context);
            }

            // Check container capacity before harvesting (only when on the
            // container — when displaced, resources drop on the ground and
            // will be picked up, which is better than idling).
            if !displaced {
                let container = state_context.container_target.resolve().unwrap();
                let work_parts = creep.body().iter().filter(|p| p.part() == Part::Work).count() as u32;

                let mining_power = match state_context.mine_target {
                    StaticMineTarget::Source(_) => HARVEST_POWER,
                    StaticMineTarget::Mineral(_, _) => HARVEST_MINERAL_POWER,
                };

                let resources_harvested = work_parts * mining_power;

                if resources_harvested as i32 > container.store().get_free_capacity(None) {
                    return Some(StaticMineState::wait(1));
                }
            }

            // Harvest if in range — works whether on the container or
            // displaced (as long as within range 1 of the mine target).
            if creep.pos().is_near_to(state_context.mine_target.pos()) {
                return try_harvest_mine_target(creep, &state_context.mine_target, tick_context);
            }

            // Displaced and out of harvest range — the move_to_container
            // request above will move us back. Return None to commit the
            // movement intent this tick.
            None
        } else {
            // Container is gone — try to find a replacement.
            if state_context.try_rediscover_container(tick_context) {
                return Some(StaticMineState::move_to_container());
            }

            // No container yet. Harvest at the source anyway — energy
            // regenerates on a timer so it should always be collected.
            let mine_pos = state_context.mine_target.pos();

            if !creep.pos().is_near_to(mine_pos) {
                // Walk toward the source (range 1).
                if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(tick_context.runtime_data.creep_entity, mine_pos)
                        .range(1);
                }
                return None;
            }

            // Near the source without a container — allow shoving within
            // range 1 so we don't block other creeps.
            mark_working(tick_context, mine_pos, 1);

            // Perform the harvest action.
            try_harvest_mine_target(creep, &state_context.mine_target, tick_context)
        }
    }
}

impl Wait {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        let creep = tick_context.runtime_data.owner;

        // If the container is gone, try to rediscover it.
        if state_context.container_target.resolve().is_none() && state_context.try_rediscover_container(tick_context) {
            return Some(StaticMineState::move_to_container());
        }

        if state_context.is_displaced(creep) {
            // Shoved off the container while waiting — skip the wait and go
            // straight to Harvest so we start walking back while still being
            // productive (Harvest will issue the move and attempt to mine).
            return Some(StaticMineState::harvest());
        }

        // On the container — stay put.
        mark_stationed(tick_context);

        tick_wait(&mut self.ticks, StaticMineState::harvest)
    }
}

impl FindContainer {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        // Container was missing when we entered this state — check again.
        if state_context.try_rediscover_container(tick_context) {
            return Some(StaticMineState::move_to_container());
        }

        // No container yet, but go harvest anyway — don't waste source regen.
        Some(StaticMineState::harvest())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StaticMineJob {
    pub context: StaticMineJobContext,
    pub state: StaticMineState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl StaticMineJob {
    pub fn new(mine_target: StaticMineTarget, container_id: RemoteObjectId<StructureContainer>) -> StaticMineJob {
        StaticMineJob {
            context: StaticMineJobContext {
                mine_target,
                container_target: container_id,
            },
            state: StaticMineState::move_to_container(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for StaticMineJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("StaticMine - {}", self.state.status_description()))
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
