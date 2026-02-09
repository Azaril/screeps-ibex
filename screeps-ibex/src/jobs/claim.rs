use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use screeps::*;
use screeps_machine::*;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct ClaimJobContext {
    pub claim_target: RemoteObjectId<StructureController>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum ClaimState {
        MoveToController,
        ClaimController,
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut ClaimJobContext, tick_context: &mut JobTickContext) -> Option<ClaimState>;
    }
);

impl MoveToController {
    fn tick(&mut self, state_context: &mut ClaimJobContext, tick_context: &mut JobTickContext) -> Option<ClaimState> {
        tick_move_to_position(
            tick_context,
            state_context.claim_target.pos().into(),
            1,
            None,
            ClaimState::claim_controller,
        )
    }
}

impl ClaimController {
    fn tick(&mut self, state_context: &mut ClaimJobContext, tick_context: &mut JobTickContext) -> Option<ClaimState> {
        tick_claim(tick_context, state_context.claim_target, || ClaimState::wait(5))
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &ClaimJobContext, _tick_context: &mut JobTickContext) -> Option<ClaimState> {
        tick_wait(&mut self.ticks, ClaimState::move_to_controller)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ClaimJob {
    context: ClaimJobContext,
    state: ClaimState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ClaimJob {
    pub fn new(controller_id: RemoteObjectId<StructureController>) -> ClaimJob {
        ClaimJob {
            context: ClaimJobContext {
                claim_target: controller_id,
            },
            state: ClaimState::move_to_controller(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for ClaimJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Claim - {}", self.state.status_description()))
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
