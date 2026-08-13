//! The scout job (ADR 0046 D3): walk the assigned tour.
//!
//! All target selection lives in the `ScoutAssignmentSystem` post-pass — the
//! job just moves toward the first tour stop that is not its current room.
//! There is no Idle state, no per-creep claim, and no job-side opportunistic
//! request path (all deleted with WFV 28); a scout with an empty tour was
//! given a fallback leg by the assigner, so an empty tour here only happens
//! on the first tick after spawn (or with truly zero demand and no frontier),
//! where the creep registers as shoveable idle for exactly one pass.

use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use crate::jobs::actions::*;
use screeps::*;
use screeps_rover::*;
use serde::*;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ScoutJob {
    /// Cache of the current tour head (visualization only — the tour itself
    /// lives in the ephemeral `ScoutAssignments` resource).
    #[serde(default)]
    room_target: Option<RoomName>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutJob {
    pub fn new() -> ScoutJob {
        ScoutJob::default()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for ScoutJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        let target = self
            .room_target
            .map(|r| r.to_string())
            .unwrap_or_else(|| "none".to_string());
        crate::visualization::SummaryContent::Text(format!("Scout -> {}", target))
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data,
            runtime_data,
            action_flags: SimultaneousActionFlags::UNSET,
        };

        let creep_entity = tick_context.runtime_data.creep_entity;
        let current_room = tick_context.runtime_data.owner.pos().room_name();

        // The first tour stop that is not the room we are standing in. (The
        // assigner pops satisfied stops when it runs; skipping the current
        // room locally keeps a scout moving on assignment-pass-shed ticks.)
        let target = tick_context
            .runtime_data
            .scout_assignments
            .tours
            .get(&creep_entity)
            .and_then(|tour| tour.iter().copied().find(|room| *room != current_room));

        self.room_target = target;

        match target {
            Some(room_name) => {
                let room_options = RoomOptions::new(HostileBehavior::HighCost);

                // Live w-as-priority, SCOUT leg (ADR 0033 §D5.4 role table):
                // the scout bids the DECLARED intel floor (`SCOUT_INTEL_BID`)
                // — it yields every contested tile to real cargo/work bids but
                // still outranks shoveable idles. Arrival is not the goal —
                // fresh intel is; the assigner advances the tour when the
                // room's intel freshens (by this scout or anyone else).
                let _: Option<()> = tick_move_to_room_with_bid(
                    &mut tick_context,
                    room_name,
                    Some(room_options),
                    Some(crate::pathing::value::SCOUT_INTEL_BID),
                    || (),
                );
            }
            None => {
                // No tour yet (first tick after spawn / zero demand): register
                // as shoveable idle so the resolver can push us around.
                mark_idle(&mut tick_context);
            }
        }
    }
}
