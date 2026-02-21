use super::data::*;
use super::operationsystem::*;
use crate::military::threatmap::ThreatLevel;
use crate::missions::claim::ClaimMission;
use crate::missions::colony::*;
use crate::missions::data::*;
use crate::serialize::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct ColonyOperation {
    owner: EntityOption<Entity>,
    last_run: Option<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ColonyOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ColonyOperation::new(owner);

        builder.with(OperationData::Colony(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> ColonyOperation {
        ColonyOperation {
            owner: owner.into(),
            last_run: None,
        }
    }

    /// Detect rooms that have spawns but an unclaimed controller and attempt
    /// to reclaim them by creating a `ClaimMission` from a nearby home room.
    ///
    /// A room is eligible for reclaim when:
    /// - It is currently visible.
    /// - It has at least one spawn (even unowned — spawns persist after unclaim).
    /// - The controller exists but is neutral (unclaimed).
    /// - The room has no significant hostile threat (at most invader-level).
    /// - No `ClaimMission` already targets this room.
    fn run_reclaim(system_data: &mut OperationExecutionSystemData, runtime_data: &mut OperationExecutionRuntimeData) {
        // Collect rooms that need reclaiming. We gather into a vec first to
        // avoid borrowing conflicts with room_data.
        let mut needs_reclaim: Vec<(Entity, RoomName)> = Vec::new();

        for (entity, room_data) in (system_data.entities, &*system_data.room_data).join() {
            // Must be visible this tick so we have fresh structure data.
            let dynamic = match room_data.get_dynamic_visibility_data() {
                Some(d) if d.visible() => d,
                _ => continue,
            };

            // Controller must be neutral (unclaimed).
            if !dynamic.owner().neutral() {
                continue;
            }

            // Must have a controller to claim.
            let static_vis = match room_data.get_static_visibility_data() {
                Some(s) => s,
                None => continue,
            };
            if static_vis.controller().is_none() {
                continue;
            }

            // Must have at least one spawn in the room.
            let has_spawns = room_data.get_structures().map(|s| !s.spawns().is_empty()).unwrap_or(false);
            if !has_spawns {
                continue;
            }

            // Check threat level — only reclaim if threat is manageable.
            let threat_ok = system_data
                .threat_data
                .get(entity)
                .map(|td| td.threat_level <= ThreatLevel::Invader)
                .unwrap_or(true);
            if !threat_ok {
                info!("Colony reclaim: skipping {} due to hostile threat", room_data.name);
                continue;
            }

            // Skip if a ClaimMission already targets this room.
            let mission_data = system_data.mission_data;
            let has_claim_mission = room_data
                .get_missions()
                .iter()
                .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<ClaimMission>().is_some());
            if has_claim_mission {
                continue;
            }

            needs_reclaim.push((entity, room_data.name));
        }

        if needs_reclaim.is_empty() {
            return;
        }

        // Gather home rooms that can spawn a claimer (owned, have spawns, RCL >= 2).
        let home_rooms: Vec<(Entity, RoomName)> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter_map(|(entity, room_data)| {
                let dynamic = room_data.get_dynamic_visibility_data()?;
                if !dynamic.owner().mine() {
                    return None;
                }
                let structures = room_data.get_structures()?;
                if structures.spawns().is_empty() {
                    return None;
                }
                let max_level = structures.controllers().iter().map(|c| c.level()).max()?;
                if max_level < 2 {
                    return None;
                }
                Some((entity, room_data.name))
            })
            .collect();

        if home_rooms.is_empty() {
            return;
        }

        for (target_entity, target_name) in needs_reclaim {
            // Find home rooms within range (linear distance).
            let home_room_entities: Vec<Entity> = home_rooms
                .iter()
                .map(|(entity, home_name)| {
                    let delta = target_name - *home_name;
                    let range = delta.0.unsigned_abs() + delta.1.unsigned_abs();
                    (*entity, range)
                })
                .filter(|(_, range)| *range <= 10)
                .map(|(entity, _)| entity)
                .collect();

            if home_room_entities.is_empty() {
                info!("Colony reclaim: no eligible home rooms for {}", target_name);
                continue;
            }

            info!("Colony reclaim: creating claim mission to reclaim room {}", target_name);

            let room_data = match system_data.room_data.get_mut(target_entity) {
                Some(rd) => rd,
                None => continue,
            };

            let mission_entity = ClaimMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(runtime_data.entity),
                target_entity,
                &home_room_entities,
            )
            .build();

            room_data.add_mission(mission_entity);
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ColonyOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text("Colony".to_string())
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let should_run = self.last_run.map(|t| game::time() - t >= 50).unwrap_or(true);

        if !should_run {
            return Ok(OperationResult::Running);
        }

        self.last_run = Some(game::time());

        for (entity, room_data) in (system_data.entities, &mut *system_data.room_data).join() {
            let needs_colony = ColonyMission::can_run(room_data);

            if needs_colony {
                //
                // Query if any missions running on the room currently fufill the colony role.
                //

                let mission_data = system_data.mission_data;

                //TODO: wiarchbe: Use trait instead of match.
                let has_colony_mission = room_data
                    .get_missions()
                    .iter()
                    .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<ColonyMission>().is_some());

                //
                // Spawn a new mission to fill the colony role if missing.
                //

                if !has_colony_mission {
                    info!("Starting colony mission for spawning room. Room: {}", room_data.name);

                    let mission_entity = ColonyMission::build(
                        system_data.updater.create_entity(system_data.entities),
                        Some(runtime_data.entity),
                        entity,
                    )
                    .build();

                    room_data.add_mission(mission_entity);
                }
            }
        }

        // Detect rooms with spawns but an unclaimed controller and attempt
        // to reclaim them by sending a claimer from a nearby home room.
        Self::run_reclaim(system_data, runtime_data);

        Ok(OperationResult::Running)
    }
}
