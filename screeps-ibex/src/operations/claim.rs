use super::data::*;
use super::operationsystem::*;
use crate::missions::claim::*;
use crate::missions::data::*;
use crate::missions::remotebuild::*;
use crate::room::gather::*;
use crate::room::roomplansystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::visualization::{CandidateSubScores, SummaryContent};
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::collections::HashSet;

/// Phase of the claim pipeline state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum ClaimPhase {
    /// Waiting for the next discovery cycle. Serves viz from cache, runs
    /// `spawn_remote_build` on a modulo check.
    #[default]
    Idle,
    /// BFS discovery just completed; waiting for scouts/observers to provide
    /// visibility for the candidate rooms.
    Scouting,
    /// Scouting window elapsed; ready to score candidates and create missions.
    Select,
}

/// Cached candidate room data produced during the Discover phase and
/// incrementally scored during Scouting. Uses `RoomName` rather than `Entity`
/// so the struct is plain serde (no entity references to track across
/// serialization).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedCandidate {
    /// Room name of the candidate.
    room_name: RoomName,
    /// BFS distance from the nearest home room.
    distance: u32,
    /// Home room names that can service this candidate.
    home_rooms: Vec<RoomName>,
    /// `None` = not yet scored (awaiting visibility). `Some` = scored.
    score: Option<(f32, CandidateSubScores)>,
}

#[derive(Clone, ConvertSaveload)]
pub struct ClaimOperation {
    owner: EntityOption<Entity>,
    claim_missions: EntityVec<Entity>,
    /// Current phase of the claim pipeline.
    phase: ClaimPhase,
    /// Tick when the current phase started (used for timing windows).
    phase_tick: Option<u32>,
    /// Cached candidates from the last Discover pass.
    candidates: Vec<CachedCandidate>,
    /// Home room names from the last Discover pass.
    home_rooms: Vec<RoomName>,
    /// Unknown rooms (no entity/visibility) from the last Discover pass.
    unknown_rooms: Vec<RoomName>,
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
            phase: ClaimPhase::Idle,
            phase_tick: None,
            candidates: Vec::new(),
            home_rooms: Vec::new(),
            unknown_rooms: Vec::new(),
        }
    }

    pub fn claim_missions(&self) -> &EntityVec<Entity> {
        &self.claim_missions
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
            && (dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().neutral())
            && !dynamic_visibility_data.source_keeper();
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

    fn source_score(system_data: &mut OperationExecutionSystemData, room_entity: Entity) -> Option<(f32, f32)> {
        let room_data = system_data.room_data.get(room_entity)?;
        let static_visibility_data = room_data.get_static_visibility_data()?;
        let sources = static_visibility_data.sources().len();

        if sources == 0 {
            return None;
        }

        let score = sources.min(2) as f32 / 2.0;

        Some((score, 4.0))
    }

    fn walkability_score(system_data: &mut OperationExecutionSystemData, room_entity: Entity) -> Option<(f32, f32)> {
        let room_data = system_data.room_data.get(room_entity)?;
        let static_visibility_data = room_data.get_static_visibility_data()?;
        let statistics = static_visibility_data.terrain_statistics();

        let walkable_tiles = statistics.walkable_tiles();

        if walkable_tiles == 0 {
            return None;
        }

        // Use plains ratio directly as a 0.0–1.0 score. Swampy rooms score
        // lower but are no longer hard-rejected — the old 0.75 threshold
        // eliminated most rooms.
        let plains_ratio = statistics.plain_tiles() as f32 / statistics.walkable_tiles() as f32;

        Some((plains_ratio, 1.0))
    }

    fn distance_score(distance: u32) -> Option<(f32, f32)> {
        let score = match distance {
            0 => None,
            1 => Some(0.5),
            2 => Some(0.75),
            3 => Some(1.0),
            4 => Some(1.0),
            _ => Some(0.5),
        }?;

        Some((score, 0.5))
    }

    /// Score a candidate room, returning the weighted total and raw sub-scores.
    /// Returns `None` if the room lacks visibility data or fails any scoring
    /// criterion.
    fn score_candidate(
        system_data: &mut OperationExecutionSystemData,
        room_entity: Entity,
        distance: u32,
    ) -> Option<(f32, CandidateSubScores)> {
        let (source_raw, source_w) = Self::source_score(system_data, room_entity)?;
        let (walk_raw, walk_w) = Self::walkability_score(system_data, room_entity)?;
        let (dist_raw, dist_w) = Self::distance_score(distance)?;

        let total_weight = source_w + walk_w + dist_w;
        if total_weight <= 0.0 {
            return None;
        }

        let total = (source_raw * source_w + walk_raw * walk_w + dist_raw * dist_w) / total_weight;

        Some((
            total,
            CandidateSubScores {
                source: source_raw,
                walkability: walk_raw,
                distance: dist_raw,
            },
        ))
    }

    // ── Phase: Discover ─────────────────────────────────────────────────────

    /// Run BFS room discovery, populate cached candidates and unknown rooms,
    /// request visibility, and transition to Scouting.
    fn run_discover(&mut self, system_data: &mut OperationExecutionSystemData) {
        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
            room_plan_data: system_data.room_plan_data,
        };

        // Use min_rcl=2 so the BFS only seeds from rooms that can spawn scouts.
        let home_rooms = gather_home_rooms(&gather_system_data, 2);

        let gathered_data = gather_candidate_rooms(&gather_system_data, &home_rooms, 4, Self::gather_candidate_room_data);

        // Build cached candidates from BFS results.
        self.candidates = gathered_data
            .candidate_rooms()
            .iter()
            .filter_map(|candidate| {
                let room_data = system_data.room_data.get(candidate.room_data_entity())?;
                let home_names: Vec<RoomName> = candidate
                    .home_room_data_entities()
                    .iter()
                    .filter_map(|e| system_data.room_data.get(*e).map(|d| d.name))
                    .collect();
                Some(CachedCandidate {
                    room_name: room_data.name,
                    distance: candidate.distance(),
                    home_rooms: home_names,
                    score: None,
                })
            })
            .collect();

        // Cache home room names.
        self.home_rooms = home_rooms
            .iter()
            .filter_map(|e| system_data.room_data.get(*e).map(|d| d.name))
            .collect();

        // Cache unknown room names.
        self.unknown_rooms = gathered_data.unknown_rooms().iter().map(|u| u.room_name()).collect();

        // Request visibility for unknown rooms (critical priority).
        for unknown_room in gathered_data.unknown_rooms().iter() {
            system_data.visibility.request(VisibilityRequest::new(
                unknown_room.room_name(),
                VISIBILITY_PRIORITY_CRITICAL,
                VisibilityRequestFlags::ALL,
            ));
        }

        // Request visibility for candidate rooms that are going stale.
        for candidate_room in gathered_data.candidate_rooms().iter() {
            if let Some(room_data) = system_data.room_data.get(candidate_room.room_data_entity()) {
                if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                    if dynamic_visibility_data.age() > Self::VISIBILITY_TIMEOUT / 2 {
                        system_data.visibility.request(VisibilityRequest::new(
                            room_data.name,
                            VISIBILITY_PRIORITY_HIGH,
                            VisibilityRequestFlags::ALL,
                        ));
                    }
                }
            }
        }

        // Record phase start tick and transition.
        self.phase_tick = Some(game::time());
        self.phase = ClaimPhase::Scouting;
    }

    // ── Phase: Scouting ─────────────────────────────────────────────────────

    /// Keep visibility requests alive for rooms that still need scouting.
    /// Called each tick during the Scouting phase so that entries don't expire
    /// before scouts/observers can service them.
    fn refresh_visibility_requests(&self, system_data: &mut OperationExecutionSystemData) {
        // Unknown rooms need critical-priority visibility.
        for room_name in &self.unknown_rooms {
            system_data.visibility.request(VisibilityRequest::new(
                *room_name,
                VISIBILITY_PRIORITY_CRITICAL,
                VisibilityRequestFlags::ALL,
            ));
        }

        // Unscored candidates need high-priority visibility.
        for candidate in &self.candidates {
            if candidate.score.is_none() {
                system_data.visibility.request(VisibilityRequest::new(
                    candidate.room_name,
                    VISIBILITY_PRIORITY_HIGH,
                    VisibilityRequestFlags::ALL,
                ));
            }
        }
    }

    /// Attempt to score any candidates that now have fresh visibility data.
    /// Pure ECS lookups, no JS API calls.
    fn try_score_candidates(&mut self, system_data: &mut OperationExecutionSystemData) {
        for candidate in self.candidates.iter_mut() {
            if candidate.score.is_some() {
                continue;
            }

            let room_entity = match system_data.mapping.get_room(&candidate.room_name) {
                Some(e) => e,
                None => continue,
            };

            // Check if the room is still viable (not hostile/owned).
            if let Some(room_data) = system_data.room_data.get(room_entity) {
                if let Some(dynamic) = room_data.get_dynamic_visibility_data() {
                    if dynamic.owner().hostile() {
                        // Mark as unscoreable by setting a negative score.
                        candidate.score = Some((
                            -1.0,
                            CandidateSubScores {
                                source: 0.0,
                                walkability: 0.0,
                                distance: 0.0,
                            },
                        ));
                        continue;
                    }
                }
            }

            // Attempt scoring.
            if let Some(result) = Self::score_candidate(system_data, room_entity, candidate.distance) {
                candidate.score = Some(result);
            }
        }
    }

    // ── Phase: Select ───────────────────────────────────────────────────────

    /// Score and sort candidates, create a mission for the best one, and
    /// transition back to Idle.
    ///
    /// Only the single highest-scoring candidate is claimed per select cycle
    /// to avoid over-committing resources.
    fn run_select(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
        maximum_rooms: u32,
        currently_owned_rooms: u32,
        features: &crate::features::ClaimFeatures,
    ) {
        // Final scoring pass for any candidates still unscored.
        self.try_score_candidates(system_data);

        let total_before_prune = self.candidates.len();
        let unscored = self.candidates.iter().filter(|c| c.score.is_none()).count();
        let hostile = self
            .candidates
            .iter()
            .filter(|c| c.score.map(|(s, _)| s < 0.0).unwrap_or(false))
            .count();

        // Prune candidates that are unscored (no visibility arrived) or hostile
        // (negative score).
        self.candidates.retain(|c| c.score.map(|(s, _)| s >= 0.0).unwrap_or(false));

        info!(
            "ClaimOp [Select]: {} candidates total, {} unscored (pruned), {} hostile (pruned), {} remaining",
            total_before_prune,
            unscored,
            hostile,
            self.candidates.len()
        );

        // Sort by score descending.
        self.candidates.sort_by(|a, b| {
            let sa = a.score.map(|(s, _)| s).unwrap_or(0.0);
            let sb = b.score.map(|(s, _)| s).unwrap_or(0.0);
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal).reverse()
        });

        // Log the ranked candidates.
        for (i, candidate) in self.candidates.iter().enumerate() {
            if let Some((score, sub)) = candidate.score {
                info!(
                    "ClaimOp [Select]:   #{} {} score={:.3} (source={:.2} walk={:.2} dist={:.2}) dist={} homes=[{}]",
                    i + 1,
                    candidate.room_name,
                    score,
                    sub.source,
                    sub.walkability,
                    sub.distance,
                    candidate.distance,
                    candidate.home_rooms.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","),
                );
            }
        }

        let active_rooms = (currently_owned_rooms as usize + self.claim_missions.len()) as u32;
        let available_rooms = maximum_rooms.saturating_sub(active_rooms);
        let at_capacity = active_rooms >= maximum_rooms || !features.on;

        info!(
            "ClaimOp [Select]: owned={} active_missions={} max_rooms={} available={} at_capacity={} features.on={}",
            currently_owned_rooms,
            self.claim_missions.len(),
            maximum_rooms,
            available_rooms,
            at_capacity,
            features.on,
        );

        // Only claim the single best candidate per cycle to avoid
        // over-committing resources.
        let max_new_missions = if at_capacity {
            info!("ClaimOp [Select]: at capacity, no new missions");
            0
        } else {
            // Cap to 1 per select cycle regardless of room/mission headroom.
            1usize
        };

        if max_new_missions > 0 {
            // Gather home room data for mission creation.
            let home_room_data: Vec<_> = (system_data.entities, &*system_data.room_data)
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
                .collect();

            // Build set of home rooms already committed to active claim missions.
            let mut used_home_rooms: HashSet<Entity> = HashSet::new();
            for mission_entity in self.claim_missions.iter() {
                if let Some(mission) = system_data.mission_data.get(*mission_entity) {
                    if let Some(claim_mission) = mission.as_mission_type::<ClaimMission>() {
                        for home_entity in claim_mission.home_room_datas().iter() {
                            used_home_rooms.insert(*home_entity);
                        }
                    }
                }
            }

            let mut missions_created = 0;

            for candidate in self.candidates.iter().take(1) {
                if missions_created >= max_new_missions {
                    break;
                }

                let candidate_entity = match system_data.mapping.get_room(&candidate.room_name) {
                    Some(e) => e,
                    None => {
                        info!(
                            "ClaimOp [Select]: top candidate {} has no entity mapping, skipping",
                            candidate.room_name
                        );
                        continue;
                    }
                };

                let room_data = match system_data.room_data.get_mut(candidate_entity) {
                    Some(d) => d,
                    None => {
                        info!("ClaimOp [Select]: top candidate {} has no room data, skipping", candidate.room_name);
                        continue;
                    }
                };

                // Ensure a room plan exists for the room.
                if system_data.room_plan_data.get(candidate_entity).is_none() {
                    info!(
                        "ClaimOp [Select]: top candidate {} has no room plan, requesting one",
                        candidate.room_name
                    );
                    system_data.room_plan_queue.request(RoomPlanRequest::new(candidate_entity, 0.5));
                    continue;
                }

                let mission_data = system_data.mission_data;

                let has_claim_mission = room_data
                    .get_missions()
                    .iter()
                    .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<ClaimMission>().is_some());

                if has_claim_mission {
                    info!("ClaimOp [Select]: top candidate {} already has a claim mission", room_data.name);
                } else {
                    let home_room_entities: Vec<_> = home_room_data
                        .iter()
                        .map(|(entity, home_room_name, max_level)| {
                            let delta = room_data.name - *home_room_name;
                            let range = delta.0.unsigned_abs() + delta.1.unsigned_abs();
                            (entity, home_room_name, max_level, range)
                        })
                        .filter(|(_, _, max_level, _)| **max_level >= 2)
                        .filter(|(_, _, _, range)| *range <= 5)
                        .filter(|(entity, _, _, _)| !used_home_rooms.contains(entity))
                        .map(|(entity, _, _, _)| *entity)
                        .collect();

                    if home_room_entities.is_empty() {
                        info!(
                            "ClaimOp [Select]: top candidate {} has no eligible home rooms (all used or out of range)",
                            room_data.name
                        );
                    } else {
                        info!(
                            "ClaimOp [Select]: creating claim mission for {} (score={:.3})",
                            room_data.name,
                            candidate.score.map(|(s, _)| s).unwrap_or(0.0),
                        );

                        let mission_entity = ClaimMission::build(
                            system_data.updater.create_entity(system_data.entities),
                            Some(runtime_data.entity),
                            candidate_entity,
                            &home_room_entities,
                        )
                        .build();

                        room_data.add_mission(mission_entity);

                        self.claim_missions.push(mission_entity);
                        missions_created += 1;

                        for entity in &home_room_entities {
                            used_home_rooms.insert(*entity);
                        }
                    }
                }
            }

            if missions_created == 0 && !self.candidates.is_empty() {
                info!(
                    "ClaimOp [Select]: had {} scored candidates but created no missions",
                    self.candidates.len()
                );
            }
        }

        // Transition back to Idle, recording the current tick for the
        // re-discover interval.
        self.phase_tick = Some(game::time());
        self.phase = ClaimPhase::Idle;
    }

    // ── Visualization from cache ────────────────────────────────────────────

    /// Populate visualization data from cached state. Runs every tick when viz
    /// is enabled. Cost: O(candidates) small-vec clones, no JS calls.
    fn populate_viz_from_cache(&self, system_data: &mut OperationExecutionSystemData, currently_owned_rooms: u32, maximum_rooms: u32) {
        if let Some(map_viz) = system_data.map_viz_data.as_mut() {
            let features = crate::features::features();
            if !features.claim.visualize {
                return;
            }

            map_viz.claim.owned_rooms = currently_owned_rooms;
            map_viz.claim.maximum_rooms = maximum_rooms;

            // Unknown rooms from cache.
            map_viz.claim.unknown_rooms = self.unknown_rooms.clone();

            // Home rooms from cache.
            map_viz.claim.home_rooms = self.home_rooms.clone();

            // Blocked-by-visibility is no longer a hard block, but still useful
            // for the viz panel.
            map_viz.claim.blocked_by_visibility = !self.unknown_rooms.is_empty();

            // Scored candidate rooms from cache.
            map_viz.claim.candidate_rooms = self
                .candidates
                .iter()
                .filter_map(|c| {
                    let (score, sub) = c.score?;
                    if score < 0.0 {
                        return None;
                    }
                    Some((c.room_name, score, sub))
                })
                .collect();

            // Active claim mission info.
            for mission_entity in self.claim_missions.iter() {
                if let Some(mission) = system_data.mission_data.get(*mission_entity) {
                    let target_entity = mission.as_mission().get_room();
                    if let Some(target_room) = system_data.room_data.get(target_entity) {
                        let home_names: Vec<RoomName> = mission
                            .as_mission_type::<ClaimMission>()
                            .map(|cm| {
                                cm.home_room_datas()
                                    .iter()
                                    .filter_map(|e| system_data.room_data.get(*e).map(|d| d.name))
                                    .collect()
                            })
                            .unwrap_or_default();
                        map_viz.claim.active_claims.push((home_names, target_room.name));
                    }
                }
            }
        }
    }

    // ── spawn_remote_build (unchanged logic) ────────────────────────────────

    fn spawn_remote_build(system_data: &mut OperationExecutionSystemData, runtime_data: &mut OperationExecutionRuntimeData) {
        //
        // Ensure remote builders occur.
        //

        let mut needs_remote_build = Vec::new();

        for (entity, room_data) in (system_data.entities, &*system_data.room_data).join() {
            //TODO: The construction operation will trigger construction sites - this is brittle to rely on.

            //
            // Spawn remote build for rooms that are owned and have a spawn construction site.
            //

            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() && RemoteBuildMission::can_run(room_data) {
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

        if !needs_remote_build.is_empty() {
            let home_room_data = (system_data.entities, &*system_data.room_data)
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
                    let home_room_entities: Vec<_> = home_room_data
                        .iter()
                        .map(|(entity, home_room_name, max_level)| {
                            let delta = room_data.name - *home_room_name;
                            let range = delta.0.unsigned_abs() + delta.1.unsigned_abs();

                            (entity, home_room_name, max_level, range)
                        })
                        .filter(|(_, _, max_level, _)| **max_level >= 2)
                        .filter(|(_, _, _, range)| *range <= 5)
                        .map(|(entity, _, _, _)| *entity)
                        .collect();

                    if !home_room_entities.is_empty() {
                        info!("Starting remote build mission for room: {}", room_data.name);

                        let mission_entity = RemoteBuildMission::build(
                            system_data.updater.create_entity(system_data.entities),
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

    fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        let mut items = Vec::new();

        // Active claim missions with home rooms
        for mission_entity in self.claim_missions.iter() {
            if let Some(mission) = ctx.mission_data.get(*mission_entity) {
                let room_entity = mission.as_mission().get_room();
                if let Some(room) = ctx.room_data.get(room_entity) {
                    let home_names: Vec<String> = mission
                        .as_mission_type::<ClaimMission>()
                        .map(|cm| {
                            cm.home_room_datas()
                                .iter()
                                .filter_map(|e| ctx.room_data.get(*e))
                                .map(|d| d.name.to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    if home_names.is_empty() {
                        items.push(format!("-> {}", room.name));
                    } else {
                        items.push(format!("-> {} (from {})", room.name, home_names.join(",")));
                    }
                }
            }
        }

        if items.is_empty() {
            let phase_label = match self.phase {
                ClaimPhase::Idle => "idle",
                ClaimPhase::Scouting => "scouting",
                ClaimPhase::Select => "selecting",
            };
            items.push(format!("({})", phase_label));
        }

        SummaryContent::Lines {
            header: "Claim".to_string(),
            items,
        }
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let features = crate::features::features();

        // ── 1. Count owned rooms, compute capacity, track min RCL ───────

        let mut currently_owned_rooms: u32 = 0;
        let mut min_rcl: u32 = u32::MAX;

        for (_, room_data) in (system_data.entities, &*system_data.room_data).join() {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    currently_owned_rooms += 1;

                    let rcl = room_data
                        .get_structures()
                        .iter()
                        .flat_map(|s| s.controllers())
                        .map(|c| c.level() as u32)
                        .max()
                        .unwrap_or(0);
                    min_rcl = min_rcl.min(rcl);
                }
            }
        }

        // If we have no rooms, min_rcl stays MAX; treat as "ready" so we don't
        // block forever on an empty empire.
        if currently_owned_rooms == 0 {
            min_rcl = u32::MAX;
        }

        const ESTIMATED_ROOM_CPU_COST: u32 = 10;
        let cpu_limit = game::cpu::limit();
        let current_gcl = game::gcl::level();
        let maximum_rooms = (cpu_limit / ESTIMATED_ROOM_CPU_COST).min(current_gcl);

        // ── 2. Populate visualization from cache (cheap, every tick) ────

        self.populate_viz_from_cache(system_data, currently_owned_rooms, maximum_rooms);

        // ── 3. spawn_remote_build on modulo ─────────────────────────────

        if game::time().is_multiple_of(features.claim.remote_build_interval) {
            Self::spawn_remote_build(system_data, runtime_data);
        }

        // ── 4. Phase dispatch ───────────────────────────────────────────

        match self.phase {
            ClaimPhase::Idle => {
                let elapsed = self.phase_tick.map(|t| game::time().saturating_sub(t)).unwrap_or(u32::MAX);

                if elapsed >= features.claim.discover_interval {
                    // Readiness gate: all owned rooms must be RCL >= 2.
                    if min_rcl >= 2 {
                        self.run_discover(system_data);
                    }
                }
            }
            ClaimPhase::Scouting => {
                self.try_score_candidates(system_data);
                self.refresh_visibility_requests(system_data);

                let elapsed = self.phase_tick.map(|t| game::time().saturating_sub(t)).unwrap_or(0);

                if elapsed >= features.claim.scouting_window {
                    self.phase = ClaimPhase::Select;
                }
            }
            ClaimPhase::Select => {
                self.run_select(system_data, runtime_data, maximum_rooms, currently_owned_rooms, &features.claim);
            }
        }

        Ok(OperationResult::Running)
    }
}
