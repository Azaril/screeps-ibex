use super::data::*;
use super::operationsystem::*;
use crate::missions::scout::*;
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

/// Always-running operation that manages scout missions for the visibility queue.
///
/// Reads the visibility queue for entries that need scouting (rooms not in
/// observer range or not observed for a long time) and spawns `ScoutMission`s
/// to service them.
#[derive(Clone, ConvertSaveload)]
pub struct ScoutOperation {
    owner: EntityOption<Entity>,
    scout_missions: EntityVec<Entity>,
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
            scout_missions: EntityVec::new(),
        }
    }

    /// Gather home rooms that can spawn scouts (have spawns, level >= 2).
    fn gather_home_rooms(system_data: &OperationExecutionSystemData) -> Vec<(Entity, RoomName)> {
        let mut result = Vec::new();
        for (entity, room_data) in (system_data.entities, &*system_data.room_data).join() {
            let dvd = match room_data.get_dynamic_visibility_data() {
                Some(d) => d,
                None => continue,
            };
            if !dvd.owner().mine() {
                continue;
            }
            let structures = match room_data.get_structures() {
                Some(s) => s,
                None => continue,
            };
            if structures.spawns().is_empty() {
                continue;
            }
            let max_level = match structures.controllers().iter().map(|c: &StructureController| c.level()).max() {
                Some(l) => l,
                None => continue,
            };
            if max_level < 2 {
                continue;
            }
            result.push((entity, room_data.name));
        }
        result
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

    fn child_complete(&mut self, child: Entity) {
        self.scout_missions.retain(|e| *e != child);
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        self.scout_missions.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead scout mission entity {:?} removed from ScoutOperation", e);
            }
            ok
        });
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text(format!("Scout - Missions: {}", self.scout_missions.len()))
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        // Check if there are scout-eligible entries that need servicing.
        if !system_data.visibility.has_unclaimed_scout_eligible() {
            return Ok(OperationResult::Running);
        }

        // Don't spawn too many scout missions.
        const MAX_SCOUT_MISSIONS: usize = 3;
        if self.scout_missions.len() >= MAX_SCOUT_MISSIONS {
            return Ok(OperationResult::Running);
        }

        let home_rooms = Self::gather_home_rooms(system_data);
        if home_rooms.is_empty() {
            return Ok(OperationResult::Running);
        }

        // Collect rooms that already have an active scout mission to avoid duplicates.
        let mut rooms_with_missions: std::collections::HashSet<RoomName> = std::collections::HashSet::new();
        for mission_entity in self.scout_missions.iter() {
            if let Some(mission) = system_data.mission_data.get(*mission_entity) {
                let room_entity = mission.as_mission().get_room();
                if let Some(room_data) = system_data.room_data.get(room_entity) {
                    rooms_with_missions.insert(room_data.name);
                }
            }
        }

        // Spawn missions for unclaimed entries until we hit the cap.
        let slots = MAX_SCOUT_MISSIONS.saturating_sub(self.scout_missions.len());

        // Gather eligible entries sorted by priority descending.
        // Opportunistic entries (created by idle scouts for proactive
        // exploration) are excluded — they should not trigger new missions.
        let mut eligible_entries: Vec<_> = system_data
            .visibility
            .entries
            .iter()
            .filter(|e| e.allowed_types.contains(VisibilityRequestFlags::SCOUT) && !e.opportunistic)
            .filter(|e| {
                let rt = system_data.visibility.runtime.get(&e.room_name);
                let claimed = rt.map(|r| r.claimed_by.is_some()).unwrap_or(false);
                !claimed && !rooms_with_missions.contains(&e.room_name)
            })
            .cloned()
            .collect();

        eligible_entries.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal).reverse());

        let mut created = 0;
        for target_entry in eligible_entries.iter().take(slots) {
            // Find the room entity for the target.
            let room_entity = match system_data.mapping.get_room(&target_entry.room_name) {
                Some(e) => e,
                None => continue,
            };

            // Select nearby home rooms (within 5 rooms).
            let home_room_entities: Vec<Entity> = home_rooms
                .iter()
                .filter(|(_, home_name)| {
                    let delta = target_entry.room_name - *home_name;
                    let range = delta.0.unsigned_abs() + delta.1.unsigned_abs();
                    range <= 5
                })
                .map(|(entity, _)| *entity)
                .collect();

            if home_room_entities.is_empty() {
                continue;
            }

            debug!("ScoutOperation: spawning scout mission for room {}", target_entry.room_name);

            let mission_entity = ScoutMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(runtime_data.entity),
                room_entity,
                &home_room_entities,
                target_entry.priority,
            )
            .build();

            self.scout_missions.push(mission_entity);
            rooms_with_missions.insert(target_entry.room_name);

            if let Some(room_data) = system_data.room_data.get_mut(room_entity) {
                room_data.add_mission(mission_entity);
            }

            created += 1;
        }

        if created > 0 {
            debug!("ScoutOperation: created {} scout missions this tick", created);
        }

        Ok(OperationResult::Running)
    }
}
