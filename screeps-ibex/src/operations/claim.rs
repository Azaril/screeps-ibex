use super::data::*;
use super::operationsystem::*;
use crate::missions::claim::*;
use crate::missions::data::*;
use crate::missions::remotebuild::*;
use crate::room::data::*;
use crate::room::gather::*;
use crate::room::roomplansystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

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

    const VISIBILITY_TIMEOUT: u32 = 10000;

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
            && (dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().neutral());
        let hostile = dynamic_visibility_data.owner().hostile() || dynamic_visibility_data.source_keeper();

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

        if sources == 0 {
            return None;
        }

        let score = sources.min(2) as f32 / 2.0;

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
            1 => Some(0.5),
            2 => Some(0.75),
            3 => Some(1.0),
            4 => Some(0.75),
            _ => Some(0.5),
        }?;

        Some((score, 0.5))
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

        for (entity, room_data) in (&*system_data.entities, &*system_data.room_data).join() {
            //TODO: The construction operation will trigger construction sites - this is brittle to rely on.

            //
            // Spawn remote build for rooms that are owned and have a spawn construction site.
            //

            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    if RemoteBuildMission::can_run(&room_data) {
                        if let Some(construction_sites) = room_data.get_construction_sites() {
                            let spawn_construction_site = construction_sites
                                .iter()
                                .find(|construction_site| construction_site.structure_type() == StructureType::Spawn);

                            if let Some(spawn_construction_site) = spawn_construction_site {
                                //TODO: wiarchbe: Use trait instead of match.
                                let mission_data = system_data.mission_data;

                                let has_remote_build_mission = room_data.get_missions().iter().any(|mission_entity| {
                                    mission_data.get(*mission_entity).as_mission_type::<RemoteBuildMission>().is_some()
                                });

                                //
                                // Spawn a new mission to fill the remote build role if missing.
                                //

                                if !has_remote_build_mission {
                                    info!("Starting remote build for room. Room: {}", room_data.name);

                                    let room_entity = entity;

                                    let construction_site_pos = spawn_construction_site.pos();
                                    let mut nearest_spawn = None;

                                    //TODO: Replace this hack that finds the nearest room. (Need state machine to drive operation. Blocked on specs vec serialization.)
                                    for (other_entity, other_room_data) in (&*system_data.entities, &*system_data.room_data).join() {
                                        if let Some(other_dynamic_visibility_data) = other_room_data.get_dynamic_visibility_data() {
                                            if other_dynamic_visibility_data.visible() && other_dynamic_visibility_data.owner().mine() {
                                                if let Some(structures) = other_room_data.get_structures() {
                                                    for spawn in structures.spawns().iter().filter(|s| s.my()) {
                                                        let distance = construction_site_pos.get_range_to(&spawn.pos());
                                                        if let Some((nearest_distance, _)) = nearest_spawn {
                                                            if distance < nearest_distance {
                                                                nearest_spawn = Some((distance, other_entity));
                                                            }
                                                        } else {
                                                            nearest_spawn = Some((distance, other_entity));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if let Some((_, nearest_spawn_room_entity)) = nearest_spawn {
                                        let owner_entity = runtime_data.entity;
                                        let home_room_data_entity = nearest_spawn_room_entity;

                                        system_data.updater.exec_mut(move |world| {
                                            let mission_entity = RemoteBuildMission::build(
                                                world.create_entity(),
                                                Some(owner_entity),
                                                room_entity,
                                                home_room_data_entity,
                                            )
                                            .build();

                                            let room_data_storage = &mut world.write_storage::<RoomData>();

                                            if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                                room_data.add_mission(mission_entity);
                                            }
                                        });
                                    }
                                }
                            }
                        }
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

    fn describe(&mut self, _system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Claim".to_string(), None);
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

        //TODO: Need better dynamic estimation of room cost.
        const ESTIMATED_ROOM_CPU_COST: f64 = 10.0;
        let cpu_limit = game::cpu::limit();

        let current_gcl = game::gcl::level();
        let maximum_rooms = ((cpu_limit / ESTIMATED_ROOM_CPU_COST) as u32).min(current_gcl);
        let active_rooms = (currently_owned_rooms + self.claim_missions.len()) as u32;
        let available_rooms = maximum_rooms - active_rooms.min(maximum_rooms);

        if active_rooms >= maximum_rooms {
            return Ok(OperationResult::Running);
        }

        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
            room_plan_data: system_data.room_plan_data,
        };

        let gathered_data = gather_candidate_rooms(&gather_system_data, 3, 4, Self::gather_candidate_room_data);

        //
        // Request visibility for all rooms that are going stale or have not had visibility.
        //

        for unknown_room in gathered_data.unknown_rooms().iter() {
            system_data.visibility.request(VisibilityRequest::new(
                unknown_room.room_name(),
                VISIBILITY_PRIORITY_MEDIUM,
                VisibilityRequestFlags::ALL,
            ));
        }

        for candidate_room in gathered_data.candidate_rooms().iter() {
            let room_data = system_data.room_data.get_mut(candidate_room.room_data_entity()).ok_or(())?;
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or(())?;

            if dynamic_visibility_data.age() > Self::VISIBILITY_TIMEOUT / 2 {
                system_data.visibility.request(VisibilityRequest::new(
                    room_data.name,
                    VISIBILITY_PRIORITY_MEDIUM,
                    VisibilityRequestFlags::ALL,
                ));
            }
        }

        if gathered_data
            .unknown_rooms()
            .iter()
            .any(|unknown_room| unknown_room.distance() <= 3)
        {
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
        // Plan or claim best rooms up to the number of available rooms.
        //

        for (candidate_room, _) in scored_candidate_rooms.iter().take(available_rooms as usize) {
            let room_data = system_data.room_data.get_mut(candidate_room.room_data_entity()).ok_or(())?;

            //
            // Ensure a room plan exists for the room.
            //

            if system_data.room_plan_data.get(candidate_room.room_data_entity()).is_none() {
                system_data.room_plan_queue.request(RoomPlanRequest::new(room_data.name, 0.5));

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
                info!("Starting claim for room. Room: {}", room_data.name);

                let mission_entity = ClaimMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    candidate_room.room_data_entity(),
                    candidate_room.home_room_data_entity(),
                )
                .build();

                room_data.add_mission(mission_entity);

                self.claim_missions.push(mission_entity);

                if currently_owned_rooms + self.claim_missions.len() >= maximum_rooms as usize {
                    break;
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
