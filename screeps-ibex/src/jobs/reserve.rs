use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use crate::constants::*;
use crate::remoteobjectid::*;
use screeps::*;
use screeps_machine::*;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct ReserveJobContext {
    pub reserve_target: RemoteObjectId<StructureController>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum ReserveState {
        MoveToController,
        SignController,
        ReserveController,
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState>;
    }
);

impl MoveToController {
    fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState> {
        tick_move_to_position(
            tick_context,
            state_context.reserve_target.pos().into(),
            1,
            None,
            ReserveState::sign_controller,
        )
    }
}

impl ReserveController {
    fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState> {
        tick_reserve(tick_context, state_context.reserve_target, || ReserveState::wait(5))
    }
}

impl SignController {
    pub fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState> {
        tick_sign(
            tick_context,
            state_context.reserve_target,
            ROOM_SIGN,
            ReserveState::reserve_controller,
        )
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState> {
        mark_idle(tick_context);
        tick_wait(&mut self.ticks, ReserveState::move_to_controller)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct ReserveJob {
    context: ReserveJobContext,
    state: ReserveState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ReserveJob {
    pub fn new(controller_id: RemoteObjectId<StructureController>) -> ReserveJob {
        ReserveJob {
            context: ReserveJobContext {
                reserve_target: controller_id,
            },
            state: ReserveState::move_to_controller(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for ReserveJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Reserve - {}", self.state.status_description()))
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
