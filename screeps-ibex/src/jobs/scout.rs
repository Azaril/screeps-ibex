use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use crate::room::data::RoomDynamicVisibilityData;
use crate::room::visibilitysystem::*;
use log::*;
use screeps::*;
use screeps_machine::*;
use screeps_rover::*;
use serde::*;
use specs::Join;

/// Number of ticks an idle scout waits before proactively exploring an adjacent room.
const IDLE_EXPLORE_THRESHOLD: u32 = 10;

#[derive(Clone, Serialize, Deserialize)]
pub struct ScoutJobContext {
    #[serde(default)]
    room_target: Option<RoomName>,
    /// Game tick at which the scout entered the Idle state. Used to detect
    /// prolonged idleness and trigger proactive exploration.
    #[serde(default)]
    idle_since: Option<u32>,
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
            state_context.idle_since = None;
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
    pub fn tick(&mut self, state_context: &mut ScoutJobContext, tick_context: &mut JobTickContext) -> Option<ScoutState> {
        // Record when we first entered idle so pre_run_job can detect prolonged idleness.
        if state_context.idle_since.is_none() {
            state_context.idle_since = Some(game::time());
        }

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
            context: ScoutJobContext {
                room_target: target,
                idle_since: None,
            },
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

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
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
            } else if let Some(idle_since) = self.context.idle_since {
                // Scout has been idle with nothing in the visibility queue.
                // After a threshold, proactively explore the adjacent room with
                // the oldest (or absent) visibility data.
                let idle_ticks = game::time().saturating_sub(idle_since);
                if idle_ticks >= IDLE_EXPLORE_THRESHOLD {
                    if let Some(target) = pick_adjacent_explore_target(creep_pos, system_data, runtime_data) {
                        info!("Idle scout proactively exploring adjacent room {}", target);

                        runtime_data.visibility_queue.request(VisibilityRequest::new_opportunistic(
                            target,
                            VISIBILITY_PRIORITY_LOW,
                            VisibilityRequestFlags::SCOUT,
                        ));

                        // Immediately claim the request and set the target so
                        // no other scout can snatch it before we act on it.
                        runtime_data.visibility_queue.claim(target, runtime_data.creep_entity);
                        self.context.room_target = Some(target);
                        self.context.idle_since = None;
                        self.state = ScoutState::move_to_room();
                    }
                }
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

/// Maximum linear (Chebyshev) distance from the scout's current room to
/// consider for proactive exploration.
const EXPLORE_SEARCH_RADIUS: u32 = 5;

/// Pick the best nearby room for an idle scout to explore proactively.
///
/// Searches in two passes:
/// 1. **Known rooms** — iterates all `RoomData` entities within
///    [`EXPLORE_SEARCH_RADIUS`] of the scout and scores them by visibility age
///    (oldest first), with a tiebreaker preferring closer rooms.
/// 2. **Unknown adjacent rooms** — checks immediate exits for rooms that have
///    no `RoomData` entity at all (never seen). These are highest priority.
///
/// Rooms that already have a pending visibility queue entry are skipped.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn pick_adjacent_explore_target(
    creep_pos: Position,
    system_data: &JobExecutionSystemData,
    runtime_data: &JobExecutionRuntimeData,
) -> Option<RoomName> {
    let current_room = creep_pos.room_name();

    // ── Pass 1: score all known rooms within range ───────────────────────
    let best_known: Option<(RoomName, u32, u32)> = (system_data.entities, system_data.room_data)
        .join()
        .filter_map(|(_, rd)| {
            let name = rd.name;
            if name == current_room {
                return None;
            }

            // Skip rooms already in the visibility queue.
            if runtime_data.visibility_queue.has_entry(name) {
                return None;
            }

            let dist = game::map::get_room_linear_distance(current_room, name, false);
            if dist > EXPLORE_SEARCH_RADIUS {
                return None;
            }

            let age = rd
                .get_dynamic_visibility_data()
                .map(|dvd: &RoomDynamicVisibilityData| dvd.age())
                .unwrap_or(u32::MAX);

            Some((name, age, dist))
        })
        // Prefer oldest data first, then closest on ties.
        .max_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| b.2.cmp(&a.2)) // smaller dist is better
        });

    // ── Pass 2: check immediate exits for truly unknown rooms ────────────
    let exits = game::map::describe_exits(current_room);
    let unknown_neighbor = exits.values().find(|neighbor| {
        !runtime_data.visibility_queue.has_entry(*neighbor)
            && runtime_data.mapping.get_room(neighbor).is_none()
    });

    // Unknown neighbors are highest priority (age = u32::MAX, dist = 1).
    // Compare against the best known room.
    match (unknown_neighbor, best_known) {
        (Some(unknown), Some(_)) => {
            // Unknown adjacent neighbor always wins — it's never-seen and
            // immediately reachable.
            Some(unknown)
        }
        (Some(unknown), None) => Some(unknown),
        (None, Some((known_name, _, _))) => Some(known_name),
        (None, None) => None,
    }
}
