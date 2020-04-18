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
pub struct ReserveJobContext {
    pub reserve_target: RemoteObjectId<StructureController>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum ReserveState {
        MoveToController,
        ReserveController,
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
        
        _ => fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState>;
    }
);

impl MoveToController {
    fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState> {
        tick_move_to_position(
            tick_context,
            state_context.reserve_target.pos(),
            1,
            ReserveState::reserve_controller,
        )
    }
}

impl ReserveController {
    fn tick(&mut self, state_context: &mut ReserveJobContext, tick_context: &mut JobTickContext) -> Option<ReserveState> {
        tick_reserve(tick_context, state_context.reserve_target, || ReserveState::wait(5))
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &ReserveJobContext, _tick_context: &mut JobTickContext) -> Option<ReserveState> {
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
