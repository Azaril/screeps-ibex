#![allow(dead_code)] // TRIAGE 2026-08-23 (ws-triage.md): the describe/visualize overlay layer is inert until ADR 0016 dispatches it — file-level by design.
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
        // Live w-as-priority, CLAIM travel (ADR 0033 §D5.4 claim rail): the job carries no
        // pre-priced claim value (ADR 0038's room_net_roi lands at MISSION level; plumbing it
        // here would add a serialized field = WFV bump, deliberately avoided), so bid the HARD
        // FLOOR V/S_REF with V = the claimer's own body cost (`pathing::value::claim_travel_bid`).
        let bid = crate::pathing::value::claim_travel_bid(tick_context.runtime_data.owner);
        tick_move_to_position_with_bid(
            tick_context,
            state_context.claim_target.pos().into(),
            1,
            None,
            Some(bid),
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
    pub fn tick(&mut self, _state_context: &ClaimJobContext, tick_context: &mut JobTickContext) -> Option<ClaimState> {
        mark_idle(tick_context);
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

        crate::machine_tick::run_state_machine(&mut self.state, "ClaimJob", |state| {
            state.tick(&mut self.context, &mut tick_context)
        });
    }
}
