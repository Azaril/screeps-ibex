//! The scout fleet owner (ADR 0046 D4).
//!
//! With assignment centralized in the `ScoutAssignmentSystem` post-pass, a
//! mission whose identity is one room is meaningless (it was the source of
//! the false-unreachable defect F5). `ScoutOperation` now owns the pooled
//! fleet roster; the per-room `ScoutMission` and its `MAX_SCOUT_MISSIONS = 3`
//! cap are deleted (WFV 28). Spawning is EV-driven and bid from the
//! assignment system (design-review resolution #6); the spawn callback
//! attaches the `ScoutJob` and the roster entry here (resolution #9).

use super::data::*;
use super::operationsystem::*;
use crate::jobs::data::*;
use crate::jobs::scout::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Always-running operation that owns the pooled scout fleet.
#[derive(Clone, ConvertSaveload)]
pub struct ScoutOperation {
    owner: EntityOption<Entity>,
    /// The fleet roster: every live (or spawning) scout creep entity. Kept
    /// consistent by the spawn callback (attach), `remove_creep`
    /// (death/cleanup notify), and `repair_entity_refs` (serialize-time
    /// scrub).
    scouts: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ScoutOperation::new(owner);

        builder.with(OperationData::Scout(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> ScoutOperation {
        ScoutOperation {
            owner: owner.into(),
            scouts: EntityVec::new(),
        }
    }

    /// Spawn callback for the EV-driven scout spawn (pushed by the
    /// `ScoutAssignmentSystem`): create the tour-walking `ScoutJob` and attach
    /// the creep entity to this operation's fleet roster (typed attach via the
    /// `operation_type!` `TryFrom`, design-review resolution #9).
    pub fn create_spawn_callback(operation_entity: Entity) -> crate::spawnsystem::SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Scout(ScoutJob::new());

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mut operations = world.write_storage::<OperationData>();
                if let Some(operation_data) = operations.get_mut(operation_entity) {
                    if let Ok(scout_operation) = <&mut ScoutOperation>::try_from(operation_data) {
                        scout_operation.scouts.push(creep_entity);
                    }
                }
            });
        })
    }

    /// Inject visibility requests for rooms that have a "scout" flag placed in
    /// them. An operator flag is an imperative force-visit:
    /// `want_fresh_within = 0` (design-review resolution #5) — only a
    /// same-tick sighting satisfies it, so the fleet keeps eyes on the room
    /// for as long as the flag stands.
    fn inject_flag_scout_requests(visibility: &mut VisibilityQueue) {
        for flag in game::flags().values() {
            if flag.name().to_lowercase().starts_with("scout") {
                let room_name = flag.pos().room_name();
                visibility.request(VisibilityRequest::new(room_name, VISIBILITY_PRIORITY_HIGH, VisibilityRequestFlags::ALL).want_fresh_within(0));
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ScoutOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn remove_creep(&mut self, entity: Entity) {
        self.scouts.retain(|e| *e != entity);
    }

    fn get_creeps(&self) -> Vec<Entity> {
        self.scouts.iter().copied().collect()
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        self.scouts.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead scout creep entity {:?} removed from ScoutOperation", e);
            }
            ok
        });
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text(format!("Scout - Fleet: {}", self.scouts.len()))
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        // Inject visibility requests from "scout" flags so flagged rooms are
        // always queued for scouting. Everything else — demand filtering,
        // tours, unreachable evidence, spawn EV — happens in the
        // `ScoutAssignmentSystem` post-pass after ALL producers have run.
        Self::inject_flag_scout_requests(system_data.visibility);

        Ok(OperationResult::Running)
    }
}
