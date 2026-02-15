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
    #[serde(default)]
    room_target: Option<RoomName>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum ScoutState {
        PickTarget,
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

impl PickTarget {
    pub fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        let creep_pos = tick_context.runtime_data.owner.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        if let Some(room_name) = tick_context.runtime_data.visibility_queue.best_unclaimed_for(creep_pos) {
            tick_context.runtime_data.visibility_queue.claim(room_name, creep_entity);
            state_context.room_target = Some(room_name);
            Some(ScoutState::move_to_room())
        } else {
            Some(ScoutState::idle())
        }
    }
}

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        let room_target = match state_context.room_target {
            Some(target) => target,
            None => return Some(ScoutState::pick_target()),
        };

        let room_options = RoomOptions::new(HostileBehavior::HighCost);

        let result = tick_move_to_room(tick_context, room_target, Some(room_options), ScoutState::pick_target);

        if result.is_some() {
            // Arrived at target room — release claim and clear target.
            let creep_entity = tick_context.runtime_data.creep_entity;
            tick_context.runtime_data.visibility_queue.release_entity(creep_entity);
            state_context.room_target = None;

            // Transition to Idle instead of PickTarget to end the state-machine
            // loop this tick. If we returned PickTarget here, it could
            // immediately claim a target the creep is already in range of,
            // creating an infinite PickTarget → MoveToRoom → PickTarget cycle
            // within a single tick. Idle returns None (ending the loop) and
            // will check for new targets next tick.
            return Some(ScoutState::idle());
        }

        result
    }
}

impl Idle {
    pub fn tick(&mut self, _state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        // Always register as idle this tick so the movement resolver knows
        // about us, then end the state-machine loop. Checking for new targets
        // is deferred to pre_run_job / the next tick's PickTarget to avoid an
        // infinite PickTarget → MoveToRoom (instant arrive) → Idle → PickTarget
        // cycle when the creep is already in range of the next target.
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
    /// Create a new scout job. If `room_target` is `None`, the scout will pick
    /// its own target from the visibility queue on the first tick.
    pub fn new(room_target: Option<RoomName>) -> ScoutJob {
        let (state, target) = match room_target {
            Some(room) => (ScoutState::move_to_room(), Some(room)),
            None => (ScoutState::pick_target(), None),
        };

        ScoutJob {
            context: ScoutJobContext { room_target: target },
            state,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for ScoutJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        let target = self
            .context
            .room_target
            .map(|r| r.to_string())
            .unwrap_or_else(|| "none".to_string());
        crate::visualization::SummaryContent::Text(format!("Scout -> {} - {}", target, self.state.status_description()))
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        // Claim maintenance: re-affirm or clear stale claims.
        if let Some(room_target) = self.context.room_target {
            if runtime_data.visibility_queue.has_entry(room_target) {
                // Entry still exists — re-affirm claim.
                runtime_data.visibility_queue.claim(room_target, runtime_data.creep_entity);
            } else {
                // Entry expired and was not re-requested — clear target so we
                // pick a new one in run_job.
                self.context.room_target = None;
                self.state = ScoutState::pick_target();
            }
        }

        // Idle scouts should check for new targets at the start of each tick.
        // The Idle state itself returns None to end the run_job loop (preventing
        // an infinite cycle), so we promote to PickTarget here instead.
        if matches!(self.state, ScoutState::Idle(_)) {
            let creep_pos = runtime_data.owner.pos();
            if runtime_data.visibility_queue.best_unclaimed_for(creep_pos).is_some() {
                self.state = ScoutState::pick_target();
            }
        }
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
