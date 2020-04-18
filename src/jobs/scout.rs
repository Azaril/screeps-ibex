use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use screeps::*;
use screeps_machine::*;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct ScoutJobContext {
    room_target: RoomName,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum ScoutState {
        MoveToRoom,
        Suicide,
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
        
        _ => fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState>;
    }
);

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        //TODO: Scout multiple rooms instead of just suiciding.
        //TODO: Handle navigation failure.
        tick_move_to_room(tick_context, state_context.room_target, ScoutState::suicide)
    }
}

impl Suicide {
    pub fn tick(&mut self, _state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        tick_context.runtime_data.owner.suicide();

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
