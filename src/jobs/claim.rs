use super::jobsystem::*;
use crate::remoteobjectid::*;
use super::context::*;
use super::actions::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use super::utility::controllerbehavior::*;
use screeps::*;
use serde::*;
use screeps_machine::*;

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
        
        _ => fn tick(&mut self, state_context: &mut ClaimJobContext, tick_context: &mut JobTickContext) -> Option<ClaimState>;
    }
);

impl MoveToController {
    fn tick(&mut self, state_context: &mut ClaimJobContext, tick_context: &mut JobTickContext) -> Option<ClaimState> {
        tick_move_to_position(tick_context, state_context.claim_target.pos(), 1, ClaimState::claim_controller)
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
                claim_target: controller_id
            },
            state: ClaimState::move_to_controller()
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for ClaimJob {
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
}
