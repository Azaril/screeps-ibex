use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::harvestbehavior::*;
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

#[derive(Clone, Serialize, Deserialize)]
pub struct StaticMineJobContext {
    pub mine_target: StaticMineTarget,
    pub container_target: RemoteObjectId<StructureContainer>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum StaticMineState {
        MoveToContainer,
        Harvest,
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

        _ => fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState>;
    }
);

impl MoveToContainer {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        tick_move_to_position(
            tick_context,
            state_context.container_target.pos(),
            0,
            None,
            StaticMineState::harvest,
        )
    }
}

impl Harvest {
    fn tick(&mut self, state_context: &mut StaticMineJobContext, tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        if let Some(container) = state_context.container_target.resolve() {
            let creep = tick_context.runtime_data.owner;
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

        match state_context.mine_target {
            StaticMineTarget::Source(source_id) => {
                tick_opportunistic_repair(tick_context, Some(RepairPriority::Low));

                tick_harvest(tick_context, source_id, true, false, || StaticMineState::wait(1))
            }
            StaticMineTarget::Mineral(mineral_id, _) => tick_harvest(tick_context, mineral_id, true, false, || StaticMineState::wait(1)),
        }
    }
}

impl Wait {
    fn tick(&mut self, _state_context: &mut StaticMineJobContext, _tick_context: &mut JobTickContext) -> Option<StaticMineState> {
        tick_wait(&mut self.ticks, StaticMineState::harvest)
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
            action_flags: SimultaneousActionFlags::UNSET,
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }
}
