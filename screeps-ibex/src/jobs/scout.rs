use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use screeps::*;
use screeps_machine::*;
use screeps_rover::*;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct ScoutJobContext {
    room_target: RoomName,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum ScoutState {
        MoveToRoom,
        Idle,
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState>;
    }
);

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        //TODO: Scout multiple rooms instead of just suiciding.
        //TODO: Handle navigation failure.
        let room_options = RoomOptions::new(HostileBehavior::HighCost);

        tick_move_to_room(tick_context, state_context.room_target, Some(room_options), ScoutState::idle)
    }
}

impl Idle {
    pub fn tick(&mut self, _state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        mark_idle(tick_context);
        None
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ScoutJob {
    pub context: ScoutJobContext,
    pub state: ScoutState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutJob {
    pub fn new(room_target: RoomName) -> ScoutJob {
        ScoutJob {
            context: ScoutJobContext { room_target },
            state: ScoutState::move_to_room(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for ScoutJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Scout - {}", self.state.status_description()))
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
