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
            let container_pos: Position = state_context.container_target.pos();

            // If the creep is not standing on the container tile, go back to it.
            if !creep.pos().is_equal_to(container_pos) {
                return Some(StaticMineState::move_to_container());
            }

            // Static miners use High priority so other creeps repath around
            // them, but allow shoving as a last resort. If shoved off the
            // container, the is_equal_to check above will send us back.
            mark_stationed(tick_context);

            // Check container capacity before harvesting.
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
        }

        // Perform the harvest action inline (not via tick_harvest, which would
        // override our immovable movement intent when on a container).
        try_harvest_mine_target(creep, &state_context.mine_target, tick_context)
    }
}

impl Wait {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        // Static miners remain on their container even while waiting.
        mark_stationed(tick_context);

        // If the container is gone, try to rediscover it.
        if state_context.container_target.resolve().is_none() && state_context.try_rediscover_container(tick_context) {
            return Some(StaticMineState::move_to_container());
        }

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
