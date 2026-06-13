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
pub struct DeclaimJobContext {
    pub declaim_target: RemoteObjectId<StructureController>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum DeclaimState {
        AttackController,
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut DeclaimJobContext, tick_context: &mut JobTickContext) -> Option<DeclaimState>;
    }
);

impl AttackController {
    fn tick(&mut self, state_context: &mut DeclaimJobContext, tick_context: &mut JobTickContext) -> Option<DeclaimState> {
        // After a strike the controller is upgrade-blocked for 1000 ticks and
        // rejects the next attackController (engine-mechanics §2.12). A CLAIM
        // body lives only 600 ticks, so each body lands ~one strike then idles
        // out its life; `Wait` re-checks periodically so the moment the block
        // clears (or the controller goes neutral) it acts.
        tick_attack_controller(tick_context, state_context.declaim_target, || DeclaimState::wait(25))
    }
}

impl Wait {
    fn tick(&mut self, _state_context: &mut DeclaimJobContext, tick_context: &mut JobTickContext) -> Option<DeclaimState> {
        mark_idle(tick_context);
        tick_wait(&mut self.ticks, DeclaimState::attack_controller)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DeclaimJob {
    context: DeclaimJobContext,
    state: DeclaimState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DeclaimJob {
    pub fn new(controller_id: RemoteObjectId<StructureController>) -> DeclaimJob {
        DeclaimJob {
            context: DeclaimJobContext {
                declaim_target: controller_id,
            },
            state: DeclaimState::attack_controller(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for DeclaimJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Declaim - {}", self.state.status_description()))
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

        crate::machine_tick::run_state_machine(&mut self.state, "DeclaimJob", |state| {
            state.tick(&mut self.context, &mut tick_context)
        });
    }
}
