use super::data::*;
use super::operationsystem::*;
use crate::missions::claim::*;
use crate::missions::data::*;
use crate::missions::remotebuild::*;
use crate::room::gather::*;
use crate::room::roomplansystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::missions::missionsystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use itertools::*;

#[derive(Clone, ConvertSaveload)]
pub struct ClaimOperation {
    owner: EntityOption<Entity>,
    claim_missions: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ClaimOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ClaimOperation::new(owner);

        builder.with(OperationData::Claim(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> ClaimOperation {
        ClaimOperation {
            owner: owner.into(),
            claim_missions: EntityVec::new(),
        }
    }

    const VISIBILITY_TIMEOUT: u32 = 20000;

    fn gather_candidate_room_data(gather_system_data: &GatherSystemData, room_name: RoomName) -> Option<CandidateRoomData> {
        let search_room_entity = gather_system_data.mapping.get_room(&room_name)?;
        let search_room_data = gather_system_data.room_data.get(search_room_entity)?;

        let static_visibility_data = search_room_data.get_static_visibility_data()?;
        let dynamic_visibility_data = search_room_data.get_dynamic_visibility_data()?;

        let has_controller = static_visibility_data.controller().is_some();
        let has_sources = !static_visibility_data.sources().is_empty();

        let visibility_timeout = if has_sources {
            Self::VISIBILITY_TIMEOUT
        } else {
            Self::VISIBILITY_TIMEOUT * 2
        };

        if !dynamic_visibility_data.updated_within(visibility_timeout) {
            return None;
        }

        let can_claim = dynamic_visibility_data.owner().neutral()
            && (dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().neutral()) && !dynamic_visibility_data.source_keeper();
        let hostile = dynamic_visibility_data.owner().hostile();

        let can_plan = gather_system_data
            .room_plan_data
            .get(search_room_entity)
            .map(|plan| plan.valid())
            .unwrap_or(true);

        let viable = has_controller && has_sources && can_claim && can_plan;
        let can_expand = !hostile;

        let candidate_room_data = CandidateRoomData::new(search_room_entity, viable, can_expand);

        Some(candidate_room_data)
    }

    fn source_score(system_data: &mut OperationExecutionSystemData, candidate: &CandidateRoom) -> Option<(f32, f32)> {
        let room_data = system_data.room_data.get(candidate.room_data_entity())?;
        let static_visibility_data = room_data.get_static_visibility_data()?;
        let sources = static_visibility_data.sources().len();

        let score = match sources {
            0 => None,
            1 => Some(0.25),
            x if x >= 2 => Some(1.0),
            _ => None
        }?;

        Some((score, 4.0))
    }

    fn walkability_score(system_data: &mut OperationExecutionSystemData, candidate: &CandidateRoom) -> Option<(f32, f32)> {
        let room_data = system_data.room_data.get(candidate.room_data_entity())?;
        let static_visibility_data = room_data.get_static_visibility_data()?;
        let statistics = static_visibility_data.terrain_statistics();

        let walkable_tiles = statistics.walkable_tiles();

        if walkable_tiles == 0 {
            return None;
        }

        let plains_ratio = statistics.plain_tiles() as f32 / statistics.walkable_tiles() as f32;

        if plains_ratio < 0.75 {
            return None;
        }

        Some((plains_ratio, 1.0))
    }

    fn distance_score(_system_data: &mut OperationExecutionSystemData, candidate: &CandidateRoom) -> Option<(f32, f32)> {
        let score = match candidate.distance() {
            0 => None,
            1 => Some(0.25),
            2 => Some(0.75),
            3 => Some(1.0),
            4 => Some(1.0),
            _ => Some(0.5),
        }?;

        Some((score, 2.0))
    }

    fn score_candidate_room(system_data: &mut OperationExecutionSystemData, candidate: &CandidateRoom) -> Option<f32> {
        let scorers = [Self::source_score, Self::walkability_score, Self::distance_score];

        let mut total_score = 0.0;
        let mut total_weight = 0.0;

        for scorer in scorers.iter() {
            let (score, weight) = scorer(system_data, candidate)?;

            total_score += score * weight;
            total_weight += weight;
        }

        if total_weight > 0.0 {
            let score = total_score / total_weight;

            Some(score)
        } else {
            None
        }
    }

    fn spawn_remote_build(system_data: &mut OperationExecutionSystemData, runtime_data: &mut OperationExecutionRuntimeData) {
        //
        // Ensure remote builders occur.
        //

        let mut needs_remote_build = Vec::new();

        for (entity, room_data) in (&*system_data.entities, &*system_data.room_data).join() {
            //TODO: The construction operation will trigger construction sites - this is brittle to rely on.

            //
            // Spawn remote build for rooms that are owned and have a spawn construction site.
            //

            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    if RemoteBuildMission::can_run(&room_data) {
                        let mission_data = system_data.mission_data;

                        let has_remote_build_mission = room_data
                            .get_missions()
                            .iter()
                            .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<RemoteBuildMission>().is_some());

                        //
                        // Spawn a new mission to fill the remote build role if missing.
                        //

                        if !has_remote_build_mission {
                            needs_remote_build.push(entity);
                        }
                    }
                }
            }
        }

        if !needs_remote_build.is_empty() {
            let home_room_data = (&*system_data.entities, &*system_data.room_data)
                .join()
                .filter_map(|(entity, room_data)| {
                    let dynamic_visibility_data = room_data.get_dynamic_visibility_data()?;

                    if !dynamic_visibility_data.owner().mine() {
                        return None;
                    }

                    let structures = room_data.get_structures()?;

                    if structures.spawns().is_empty() {
                        return None;
                    }

                    let max_level = structures.controllers().iter().map(|c| c.level()).max()?;

                    Some((entity, room_data.name, max_level))
                })
                .collect::<Vec<_>>();

            for room_entity in needs_remote_build {
                if let Some(room_data) = system_data.room_data.get_mut(room_entity) {
                    //TODO: Use path distance instead of linear distance.
                    let home_room_entities: Vec<_> = home_room_data.iter().map(|(entity, home_room_name, max_level)| {
                        let delta = room_data.name - *home_room_name;
                        let range = delta.0.abs() as u32 + delta.1.abs() as u32;

                        (entity, home_room_name, max_level, range)
                    })
                        .filter(|(_, _, max_level, _)| **max_level >= 2)
                        .filter(|(_, _, _, range)| *range <= 5)
                        .map(|(entity, _, _, _)| *entity)
                        .collect();

                    if !home_room_entities.is_empty() {
                        info!("Starting remote build mission for room: {}", room_data.name);

                        let mission_entity = RemoteBuildMission::build(
                            system_data.updater.create_entity(&system_data.entities),
                            Some(runtime_data.entity),
                            room_entity,
                            &home_room_entities,
                        )
                        .build();

                        room_data.add_mission(mission_entity);
                    }
                }
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ClaimOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn child_complete(&mut self, child: Entity) {
        self.claim_missions.retain(|e| *e != child);
    }

    fn describe(&mut self, system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            let mission_data = &*system_data.mission_data;
            let room_data = &*system_data.room_data;

            if self.claim_missions.is_empty() {
                global_ui.operations().add_text("Claim".to_string(), None);
            } else {
                let rooms = self.claim_missions.iter().filter_map(|claim_mission| {
                        mission_data.get(*claim_mission).as_mission_type_mut::<ClaimMission>().map(|m| m.get_room())
                    }).filter_map(|owning_room| {
                        room_data.get(owning_room)
                    })
                    .map(|room_data| room_data.name.to_string())
                    .join(" / ");

                let style = global_ui.operations().get_default_style().color("Green");

                global_ui.operations().add_text(format!("Claim - Rooms: {}", rooms), Some(style));
            }            
        })
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        Self::spawn_remote_build(system_data, runtime_data);

        //
        // Trigger new claim missions.
        //

        let mut currently_owned_rooms = 0;

        for (_, room_data) in (&*system_data.entities, &*system_data.room_data).join() {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    currently_owned_rooms += 1;
                }
            }
        }

        //
        // Only allow as many missions in progress as would reach GCL/CPU cap.
        //

        let maximum_claim_missions = (currently_owned_rooms as f32).log2().max(1.0) as u32;

        //TODO: Need better dynamic estimation of room cost.
        const ESTIMATED_ROOM_CPU_COST: u32 = 10;
        let cpu_limit = game::cpu::limit();

        let current_gcl = game::gcl::level();
        let maximum_rooms = ((cpu_limit / ESTIMATED_ROOM_CPU_COST) as u32).min(current_gcl);
        let active_rooms = (currently_owned_rooms + self.claim_missions.len()) as u32;        
        let available_rooms = maximum_rooms - active_rooms.min(maximum_rooms);
        let available_rooms = available_rooms.min(maximum_claim_missions);

        if available_rooms == 0 || !crate::features::claim() {
            return Ok(OperationResult::Running);
        }

        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
            room_plan_data: system_data.room_plan_data,
        };

        let home_rooms = gather_home_rooms(&gather_system_data, 1);

        let gathered_data = gather_candidate_rooms(&gather_system_data, &home_rooms, 4, Self::gather_candidate_room_data);

        //
        // Request visibility for all rooms that are going stale or have not had visibility.
        //

        for unknown_room in gathered_data.unknown_rooms().iter() {
            system_data.visibility.request(VisibilityRequest::new(
                unknown_room.room_name(),
                VISIBILITY_PRIORITY_CRITICAL,
                VisibilityRequestFlags::ALL,
            ));
        }

        for candidate_room in gathered_data.candidate_rooms().iter() {
            let room_data = system_data.room_data.get_mut(candidate_room.room_data_entity()).ok_or(())?;
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or(())?;

            if dynamic_visibility_data.age() > Self::VISIBILITY_TIMEOUT / 2 {
                system_data.visibility.request(VisibilityRequest::new(
                    room_data.name,
                    VISIBILITY_PRIORITY_HIGH,
                    VisibilityRequestFlags::ALL,
                ));
            }
        }

        if gathered_data
            .unknown_rooms()
            .iter()
            .any(|unknown_room| unknown_room.distance() <= 4)
        {
            /*
            for room in gathered_data.unknown_rooms().iter().filter(|unknown_room| unknown_room.distance() <= 4) {
                log::info!("Unknown room: {}", room.room_name());
            }
            */

            return Ok(OperationResult::Running);
        }

        //
        // Score rooms for priority.
        //

        let mut scored_candidate_rooms = gathered_data
            .candidate_rooms()
            .iter()
            .filter_map(|candidate| {
                let score = Self::score_candidate_room(system_data, candidate)?;

                Some((candidate, score))
            })
            .collect::<Vec<_>>();

        scored_candidate_rooms.sort_by(|(_, score_a), (_, score_b)| score_a.partial_cmp(&score_b).unwrap().reverse());

        //
        // Get home rooms
        //

        let home_room_data = (&*system_data.entities, &*system_data.room_data)
            .join()
            .filter_map(|(entity, room_data)| {
                let dynamic_visibility_data = room_data.get_dynamic_visibility_data()?;

                if !dynamic_visibility_data.owner().mine() {
                    return None;
                }

                let structures = room_data.get_structures()?;

                if structures.spawns().is_empty() {
                    return None;
                }

                let max_level = structures.controllers().iter().map(|c| c.level()).max()?;

                Some((entity, room_data.name, max_level))
            })
            .collect::<Vec<_>>();

        //
        // Plan or claim best rooms up to the number of available rooms.
        //

        for (candidate_room, _) in scored_candidate_rooms.iter().take(available_rooms as usize) {
            let room_data = system_data.room_data.get_mut(candidate_room.room_data_entity()).ok_or(())?;

            //
            // Ensure a room plan exists for the room.
            //

            if system_data.room_plan_data.get(candidate_room.room_data_entity()).is_none() {
                system_data
                    .room_plan_queue
                    .request(RoomPlanRequest::new(candidate_room.room_data_entity(), 0.5));

                continue;
            }

            let mission_data = system_data.mission_data;

            //TODO: wiarchbe: Use trait instead of match.
            let has_claim_mission = room_data
                .get_missions()
                .iter()
                .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<ClaimMission>().is_some());

            //
            // Spawn a new mission to fill the claim role if missing.
            //

            if !has_claim_mission {
                //TODO: Use path distance instead of linear distance.
                let home_room_entities: Vec<_> = home_room_data.iter().map(|(entity, home_room_name, max_level)| {
                    let delta = room_data.name - *home_room_name;
                    let range = delta.0.abs() as u32 + delta.1.abs() as u32;

                    (entity, home_room_name, max_level, range)
                })
                    .filter(|(_, _, max_level, _)| **max_level >= 2)
                    .filter(|(_, _, _, range)| *range <= 5)
                    .map(|(entity, _, _, _)| *entity)
                    .collect();

                if !home_room_entities.is_empty() {
                    info!("Starting claim for room. Room: {}", room_data.name);

                    let mission_entity = ClaimMission::build(
                        system_data.updater.create_entity(system_data.entities),
                        Some(runtime_data.entity),
                        candidate_room.room_data_entity(),
                        &home_room_entities,
                    )
                    .build();

                    room_data.add_mission(mission_entity);

                    self.claim_missions.push(mission_entity);
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
