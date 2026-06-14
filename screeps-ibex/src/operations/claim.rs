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
    /// Adaptive BFS search radius (room-hops). Starts tight at
    /// `min_search_radius`, widens only when the ring is fully scouted, there
    /// is capacity, more reachable map exists, and nothing good was found —
    /// then re-tightens (hugs the nearest good candidate). The upper bound is
    /// dynamic (claim-creep reach), so there is no static max config.
    current_search_radius: u32,
    /// Whether the last Discover BFS stopped at the radius cap with a live
    /// expandable frontier (more reachable map exists) vs. exhausting it
    /// (boxed in by hostiles/closed rooms). Gates widening.
    frontier_truncated: bool,
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
            // Default to the policy floor; clamped to configured min on first
            // discover and adjusted by the ratchet thereafter.
            current_search_radius: 4,
            frontier_truncated: false,
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

        // A confirmed-derelict room is not claimable (the controller is still
        // owned) but it is traversable, so expansion may search through it —
        // otherwise a single dead claimed room can wall off an entire frontier.
        let derelict_features = gather_system_data.derelict_features;
        let confirmed_derelict = derelict_features.on
            && dynamic_visibility_data.confirmed_derelict(derelict_features.confirm_ticks, derelict_features.path_max_age);

        let viable = has_controller && has_sources && can_claim && can_plan;
        let can_expand = !hostile || confirmed_derelict;

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

    /// Raw distance preference (0–1), peaking at the own-remote-ring band
    /// (~4–5) and disfavouring closer rooms (which would share/cannibalize an
    /// existing room's remotes). Never zero ≥ distance 1, so a far room stays
    /// selectable as a last resort — tight but not stuck. `None` for distance 0
    /// (the home room itself).
    fn distance_score(distance: u32) -> Option<f32> {
        match distance {
            0 => None,
            1 => Some(0.05),
            2 => Some(0.2),
            3 => Some(0.45),
            4 => Some(1.0),
            5 => Some(0.9),
            6 => Some(0.7),
            7 => Some(0.5),
            _ => Some(0.35),
        }
    }

    /// Return a plan quality score (0–1) for a room that has a valid plan.
    /// Returns `None` if the room has no plan data or the plan failed.
    fn plan_score(system_data: &mut OperationExecutionSystemData, room_entity: Entity) -> Option<f32> {
        let plan_data = system_data.room_plan_data.get(room_entity)?;
        let plan = plan_data.plan()?;
        // PlanScore.total is already a 0–1 weighted average from screeps-foreman.
        Some(plan.score.total)
    }

    /// Score a candidate room, returning the weighted total and raw sub-scores.
    /// Returns `None` if the room lacks visibility data or fails any scoring
    /// criterion.
    fn score_candidate(
        system_data: &mut OperationExecutionSystemData,
        room_entity: Entity,
        distance: u32,
        features: &crate::features::ClaimFeatures,
    ) -> Option<(f32, CandidateSubScores)> {
        let (source_raw, source_w) = Self::source_score(system_data, room_entity)?;
        let (walk_raw, walk_w) = Self::walkability_score(system_data, room_entity)?;
        let dist_raw = Self::distance_score(distance)?;
        let dist_w = features.distance_score_weight;

        // Plan score is optional — rooms without a plan yet are scored without
        // this component (weight excluded from total).
        let plan_raw = Self::plan_score(system_data, room_entity);
        let (plan_contribution, plan_w) = match plan_raw {
            Some(raw) => (raw * features.plan_score_weight, features.plan_score_weight),
            None => (0.0, 0.0),
        };

        let total_weight = source_w + walk_w + dist_w + plan_w;
        if total_weight <= 0.0 {
            return None;
        }

        let mut total = (source_raw * source_w + walk_raw * walk_w + dist_raw * dist_w + plan_contribution) / total_weight;

        // Heavy cannibalization penalty: a sourced room one hop from an owned
        // room is one that room could remote-mine, so claiming it splits the
        // franchise. Multiplicative so it dominates without distorting the
        // moderate 4-vs-5-vs-6 preference.
        if distance == 1 {
            total *= features.adjacent_claim_penalty;
        }

        Some((
            total,
            CandidateSubScores {
                source: source_raw,
                walkability: walk_raw,
                distance: dist_raw,
                plan: plan_raw,
            },
        ))
    }

    // ── Phase: Discover ─────────────────────────────────────────────────────

    /// Run BFS room discovery, populate cached candidates and unknown rooms,
    /// request visibility, and transition to Scouting.
    fn run_discover(&mut self, system_data: &mut OperationExecutionSystemData) {
        // Expansion is in the shed-first class (ADR 0004's authoritative
        // order): under Critical, skip discovery this cadence — the
        // phase machine stays in Discover and retries when pressure
        // clears (P1.D3, the governor's first expansion consumer).
        if system_data.governor.tier == crate::cpugovernor::Tier::Critical {
            log::debug!("expansion discovery shed (governor Critical)");
            return;
        }

        // Adaptive search radius, clamped to [min_search_radius, dynamic claim
        // reach]. The ratchet in run_select moves it between cycles.
        let min_radius = system_data.features.claim.min_search_radius;
        let max_radius = crate::missions::utility::max_claim_radius_hops().max(min_radius);
        let radius = self.current_search_radius.clamp(min_radius, max_radius);
        self.current_search_radius = radius;

        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
            room_plan_data: system_data.room_plan_data,
            room_status_cache: system_data.room_status_cache,
            derelict_features: system_data.features.derelict,
        };

        // Use min_rcl=2 so the BFS only seeds from rooms that can spawn scouts.
        let home_rooms = gather_home_rooms(&gather_system_data, 2);

        let gathered_data = gather_candidate_rooms(&gather_system_data, &home_rooms, radius, Self::gather_candidate_room_data);

        // Cache whether there is more reachable map beyond the current radius
        // (feeds the widen decision in run_select).
        self.frontier_truncated = gathered_data.frontier_truncated();

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
    fn try_score_candidates(&mut self, system_data: &mut OperationExecutionSystemData, features: &crate::features::ClaimFeatures) {
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
                                plan: None,
                            },
                        ));
                        continue;
                    }
                }
            }

            // Attempt scoring.
            if let Some(result) = Self::score_candidate(system_data, room_entity, candidate.distance, features) {
                candidate.score = Some(result);
                // We have fresh visibility for this room — it is reachable, so
                // drop any stale scout give-up backoff.
                system_data.visibility.clear_unreachable(candidate.room_name);
            }
        }
    }

    // ── Capacity: dynamic CPU room cap ──────────────────────────────────────

    /// Dynamic expansion room cap, replacing the old `cpu_limit / 10` guess.
    /// Leads with the measured per-room CPU cost (config fallback while the
    /// model is cold), lets a CPU-healthy empire probe one room beyond the
    /// static estimate, and clamps to GCL (hard game limit) and the safety
    /// caps.
    fn compute_maximum_rooms(
        features: &crate::features::ClaimFeatures,
        cpu_budget: crate::metrics::CpuBudget,
        governor: crate::cpugovernor::GovernorSnapshot,
        currently_owned_rooms: u32,
        current_gcl: u32,
    ) -> u32 {
        let cpu_limit = if cpu_budget.cpu_limit > 0.0 {
            cpu_budget.cpu_limit
        } else {
            game::cpu::limit() as f64
        };

        // Per-room cost: measured (used / rooms) once the model is warm and the
        // empire is large enough for the average to mean something; else the
        // configured fallback. Average over-estimates marginal cost (overhead
        // is folded in) — conservative, which is the headroom we want.
        let est_room_cpu = match cpu_budget.cpu_used_estimate {
            Some(used) if currently_owned_rooms >= 2 => (used / currently_owned_rooms as f64).max(1.0),
            _ => (features.fallback_room_cpu_cost as f64).max(1.0),
        };

        let estimate_cap = ((cpu_limit * features.cpu_headroom_factor as f64) / est_room_cpu).floor().max(0.0) as u32;

        // Probe one more room when the bucket is comfortably healthy: try
        // growth, then back off (next claim vetoed, cap shrinks) if the new
        // room actually pushes us over budget. Gated on tier + a high bucket,
        // not a raw `trend >= 0` (a near-full bucket sawtooths slightly
        // negative and would otherwise never probe).
        let bucket_healthy =
            governor.tier == crate::cpugovernor::Tier::Normal && governor.bucket >= features.healthy_bucket_floor;

        let structural = if bucket_healthy {
            estimate_cap.max(currently_owned_rooms + 1)
        } else {
            estimate_cap
        };

        // Safety caps bound the CPU-derived number; GCL is the hard ceiling.
        structural
            .max(features.min_room_cap)
            .min(features.max_room_cap)
            .min(current_gcl)
    }

    /// Whether the reachable ring at the current radius is fully covered:
    /// every viable candidate scored, and every unknown frontier room either
    /// resolved (now has visibility) or given up on (scout-unreachable
    /// backoff). Lets Select fire as soon as coverage lands instead of always
    /// waiting out the full scouting window — and prevents a hostile-walled,
    /// never-scoutable room from blocking selection forever.
    fn scouting_coverage_complete(&self, system_data: &OperationExecutionSystemData) -> bool {
        let now = game::time();

        if self.candidates.iter().any(|c| c.score.is_none()) {
            return false;
        }

        for room_name in &self.unknown_rooms {
            if system_data.visibility.is_unreachable_now(*room_name, now) {
                continue;
            }

            let has_visibility = system_data
                .mapping
                .get_room(room_name)
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data().map(|d| d.updated_within(Self::VISIBILITY_TIMEOUT)))
                .unwrap_or(false);

            if !has_visibility {
                return false;
            }
        }

        true
    }

    // ── Phase: Select ───────────────────────────────────────────────────────

    /// Score and sort candidates, create missions for the best candidates, and
    /// transition back to Idle.
    ///
    /// Up to `max_concurrent_missions` claim missions may be active at once
    /// (0 = unlimited, capped by GCL/CPU). Additional candidates beyond the
    /// first are only selected if their score is within `max_score_delta` of
    /// the best candidate, preventing vastly inferior rooms from being claimed.
    fn run_select(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
        maximum_rooms: u32,
        currently_owned_rooms: u32,
        features: &crate::features::ClaimFeatures,
    ) {
        // Final scoring pass for any candidates still unscored.
        self.try_score_candidates(system_data, features);

        // Coverage snapshot BEFORE pruning (prune drops unscored, which would
        // otherwise make coverage trivially "complete"). Gates the widen step.
        let covered = self.scouting_coverage_complete(system_data);

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
                let plan_label = sub.plan.map(|p| format!(" plan={:.2}", p)).unwrap_or_default();
                info!(
                    "ClaimOp [Select]:   #{} {} score={:.3} (source={:.2} walk={:.2} dist={:.2}{}) dist={} homes=[{}]",
                    i + 1,
                    candidate.room_name,
                    score,
                    sub.source,
                    sub.walkability,
                    sub.distance,
                    plan_label,
                    candidate.distance,
                    candidate.home_rooms.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","),
                );
            }
        }

        // Adaptive-radius inputs (used by the distance gate below and the
        // ratchet at the end). A "good" candidate sits in the own-remote-ring
        // band (distance >= the policy floor); closer rooms cannibalize an
        // existing room's remotes.
        let min_claim_distance = features.min_search_radius;
        let max_radius = crate::missions::utility::max_claim_radius_hops().max(min_claim_distance);
        let nearest_good = self
            .candidates
            .iter()
            .filter(|c| c.distance >= min_claim_distance)
            .map(|c| c.distance)
            .min();
        // Last resort: only claim a below-floor (cannibalizing) room when boxed
        // in at the max radius with no good candidate anywhere.
        let allow_penalized = self.current_search_radius >= max_radius && nearest_good.is_none();

        // Live affordability veto: don't START a new claim while CPU is
        // genuinely stressed (Conserve/Critical). Use the governor tier — which
        // already protects against a death-spiral drain (trend < -5) — rather
        // than a raw `trend >= 0`: a healthy empire at a near-full bucket has a
        // slightly-negative sawtooth trend most ticks, and gating on it would
        // veto claims for whole discovery cycles.
        let cpu_healthy = system_data.governor.tier == crate::cpugovernor::Tier::Normal;

        let active_rooms = (currently_owned_rooms as usize + self.claim_missions.len()) as u32;
        let available_rooms = maximum_rooms.saturating_sub(active_rooms);
        let at_capacity = active_rooms >= maximum_rooms || !features.on || !cpu_healthy;

        // Determine how many missions we can still create this cycle.
        // max_concurrent_missions caps total active missions (0 = unlimited).
        let mission_headroom = if features.max_concurrent_missions == 0 {
            usize::MAX
        } else {
            (features.max_concurrent_missions as usize).saturating_sub(self.claim_missions.len())
        };

        info!(
            "ClaimOp [Select]: owned={} active_missions={} max_rooms={} available={} mission_cap={} at_capacity={} features.on={} cpu_healthy={} est_room_cpu={:.1}",
            currently_owned_rooms,
            self.claim_missions.len(),
            maximum_rooms,
            available_rooms,
            features.max_concurrent_missions,
            at_capacity,
            features.on,
            cpu_healthy,
            system_data
                .cpu_budget
                .cpu_used_estimate
                .map(|u| if currently_owned_rooms >= 2 { u / currently_owned_rooms as f64 } else { features.fallback_room_cpu_cost as f64 })
                .unwrap_or(features.fallback_room_cpu_cost as f64),
        );

        let max_new_missions = if at_capacity {
            info!(
                "ClaimOp [Select]: no new missions (active={} max_rooms={} cpu_healthy={} features.on={})",
                active_rooms, maximum_rooms, cpu_healthy, features.on
            );
            0
        } else {
            // Cap by both room headroom and mission concurrency limit.
            (available_rooms as usize).min(mission_headroom)
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
            let best_score = self.candidates.first().and_then(|c| c.score.map(|(s, _)| s)).unwrap_or(0.0);

            for candidate in self.candidates.iter() {
                if missions_created >= max_new_missions {
                    break;
                }

                // Enforce score delta: additional candidates beyond the first
                // must be within max_score_delta of the best.
                let candidate_score = candidate.score.map(|(s, _)| s).unwrap_or(0.0);
                if missions_created > 0 && (best_score - candidate_score) > features.max_score_delta {
                    info!(
                        "ClaimOp [Select]: candidate {} score={:.3} exceeds delta {:.3} from best {:.3}, stopping",
                        candidate.room_name, candidate_score, features.max_score_delta, best_score,
                    );
                    break;
                }

                // Own-remote-ring floor: skip a too-close room (it would share
                // an existing room's remote franchise) unless we are boxed in at
                // the max radius with nothing better.
                if candidate.distance < min_claim_distance && !allow_penalized {
                    info!(
                        "ClaimOp [Select]: candidate {} at distance {} below min claim distance {}, skipping (would cannibalize remotes)",
                        candidate.room_name, candidate.distance, min_claim_distance
                    );
                    continue;
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
                    // Eligible homes: not already committed, able to AFFORD a
                    // claimer ([Claim, Move] = 650 energy ⇒ ~RCL 3 capacity —
                    // an RCL 2 home would silently fail create_body), and within
                    // CLAIM-creep reach (travel-time feasibility; claim
                    // feasibility implies the colony is also build-feasible).
                    let candidate_name = candidate.room_name;
                    let claimer_cost = Part::Claim.cost() + Part::Move.cost();
                    let mut home_room_entities: Vec<Entity> = Vec::new();
                    for (entity, home_room_name, _max_level) in home_room_data.iter() {
                        if used_home_rooms.contains(entity) {
                            continue;
                        }
                        let energy_capacity = game::rooms()
                            .get(*home_room_name)
                            .map(|r| r.energy_capacity_available())
                            .unwrap_or(0);
                        if energy_capacity < claimer_cost {
                            continue;
                        }
                        if crate::missions::utility::is_claim_feasible(system_data.pathfinder, *home_room_name, candidate_name) {
                            home_room_entities.push(*entity);
                        }
                    }

                    if home_room_entities.is_empty() {
                        info!(
                            "ClaimOp [Select]: top candidate {} has no eligible home rooms (all used, can't afford a claimer, or not claim-reachable)",
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

        // ── Adaptive radius ratchet ──────────────────────────────────────
        // Hug just out to the nearest good candidate (re-tighten); if none and
        // the ring is fully covered with capacity and more reachable map beyond
        // it, widen by one; otherwise hold (boxed in / no capacity / still
        // scouting).
        let new_radius = match nearest_good {
            Some(d) => d.clamp(min_claim_distance, max_radius),
            None => {
                if available_rooms > 0 && covered && self.frontier_truncated {
                    (self.current_search_radius + 1).min(max_radius)
                } else {
                    self.current_search_radius.clamp(min_claim_distance, max_radius)
                }
            }
        };

        if new_radius != self.current_search_radius {
            info!(
                "ClaimOp [Select]: search radius {} -> {} (nearest_good={:?} covered={} truncated={} avail={})",
                self.current_search_radius, new_radius, nearest_good, covered, self.frontier_truncated, available_rooms,
            );
        }
        self.current_search_radius = new_radius.clamp(min_claim_distance, max_radius);

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
            if !system_data.features.claim.visualize {
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
                    if let Some(target_room) = target_entity.and_then(|e| system_data.room_data.get(e)) {
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

    // ── spawn_remote_build ──────────────────────────────────────────────────

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
                    // Eligible build homes: RCL >= 2 and within build-feasible
                    // travel reach (a builder must arrive with enough life to
                    // harvest + build) — dynamic, replaces the old linear ≤5.
                    let target_name = room_data.name;
                    let mut home_room_entities: Vec<Entity> = Vec::new();
                    for (entity, home_room_name, max_level) in home_room_data.iter() {
                        if *max_level < 2 {
                            continue;
                        }
                        if crate::missions::utility::is_build_feasible(system_data.pathfinder, *home_room_name, target_name) {
                            home_room_entities.push(*entity);
                        }
                    }

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

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        self.claim_missions.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead claim mission entity {:?} removed from ClaimOperation", e);
            }
            ok
        });
    }

    fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        let mut children = Vec::new();

        // Active claim missions with home rooms.
        for mission_entity in self.claim_missions.iter() {
            if let Some(mission) = ctx.mission_data.get(*mission_entity) {
                let room_entity = mission.as_mission().get_room();
                if let Some(room) = room_entity.and_then(|e| ctx.room_data.get(e)) {
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
                        children.push(SummaryContent::Text(format!("-> {}", room.name)));
                    } else {
                        children.push(SummaryContent::Text(format!("-> {} (from {})", room.name, home_names.join(", "))));
                    }
                }
            }
        }

        // When idle/scouting/selecting with no active missions, show phase in header.
        if children.is_empty() {
            let phase_label = match self.phase {
                ClaimPhase::Idle => "Idle",
                ClaimPhase::Scouting => "Scouting",
                ClaimPhase::Select => "Selecting",
            };
            return SummaryContent::Text(format!("Claim ({})", phase_label));
        }

        SummaryContent::Tree {
            label: "Claim".to_string(),
            children,
        }
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let features = system_data.features;

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

        let current_gcl = game::gcl::level();
        let maximum_rooms = Self::compute_maximum_rooms(
            &features.claim,
            system_data.cpu_budget,
            system_data.governor,
            currently_owned_rooms,
            current_gcl,
        );

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
                self.try_score_candidates(system_data, &features.claim);
                self.refresh_visibility_requests(system_data);

                let elapsed = self.phase_tick.map(|t| game::time().saturating_sub(t)).unwrap_or(0);

                // Select as soon as the reachable ring is covered (every
                // candidate scored, every unknown resolved or given up), or when
                // the scouting window caps out — whichever comes first.
                let covered = self.scouting_coverage_complete(system_data);

                if covered || elapsed >= features.claim.scouting_window {
                    if covered {
                        info!("ClaimOp [Scouting]: reachable ring covered after {} ticks, selecting", elapsed);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpugovernor::GovernorSnapshot;
    use crate::features::ClaimFeatures;
    use crate::metrics::CpuBudget;

    fn healthy_governor() -> GovernorSnapshot {
        // Full bucket, flat trend → Normal tier, comfortably above the
        // healthy-bucket floor.
        GovernorSnapshot::compute(10_000, 0.0, 500.0)
    }

    // ── distance_score: own-remote-ring band, never-zero falloff ────────────

    #[test]
    fn distance_score_peaks_at_four_and_disfavours_close() {
        assert_eq!(ClaimOperation::distance_score(0), None);
        // Peak at 4.
        assert_eq!(ClaimOperation::distance_score(4), Some(1.0));
        // Strictly increasing up to the peak (close rooms disfavoured).
        let d1 = ClaimOperation::distance_score(1).unwrap();
        let d2 = ClaimOperation::distance_score(2).unwrap();
        let d3 = ClaimOperation::distance_score(3).unwrap();
        let d4 = ClaimOperation::distance_score(4).unwrap();
        assert!(d1 < d2 && d2 < d3 && d3 < d4, "{d1} {d2} {d3} {d4}");
        // Distance 1 is heavily disfavoured.
        assert!(d1 <= 0.1);
        // Declining but never zero beyond the peak (selectable as last resort).
        let d5 = ClaimOperation::distance_score(5).unwrap();
        let d6 = ClaimOperation::distance_score(6).unwrap();
        let d7 = ClaimOperation::distance_score(7).unwrap();
        let d20 = ClaimOperation::distance_score(20).unwrap();
        assert!(d4 > d5 && d5 > d6 && d6 > d7, "{d4} {d5} {d6} {d7}");
        assert!(d20 > 0.0);
    }

    // ── compute_maximum_rooms: dynamic, self-tuning cap ─────────────────────

    #[test]
    fn max_rooms_cold_model_uses_fallback_and_probes_when_healthy() {
        let f = ClaimFeatures::default(); // headroom 0.85, fallback 10, caps [1,50]
        let budget = CpuBudget {
            cpu_used_estimate: None,
            cpu_limit: 100.0,
        };
        // est_room_cpu = fallback 10 → estimate_cap = floor(100*0.85/10) = 8.
        // owned 3, healthy → structural = max(8, 4) = 8, min gcl 10 = 8.
        let cap = ClaimOperation::compute_maximum_rooms(&f, budget, healthy_governor(), 3, 10);
        assert_eq!(cap, 8);
    }

    #[test]
    fn max_rooms_warm_model_divides_by_owned_rooms() {
        let f = ClaimFeatures::default();
        let budget = CpuBudget {
            cpu_used_estimate: Some(60.0),
            cpu_limit: 100.0,
        };
        // est_room_cpu = 60/3 = 20 → estimate_cap = floor(100*0.85/20) = 4.
        let cap = ClaimOperation::compute_maximum_rooms(&f, budget, healthy_governor(), 3, 10);
        assert_eq!(cap, 4);
    }

    #[test]
    fn max_rooms_probe_only_when_healthy() {
        let f = ClaimFeatures::default();
        let budget = CpuBudget {
            cpu_used_estimate: Some(90.0),
            cpu_limit: 100.0,
        };
        // est_room_cpu = 90/9 = 10 → estimate_cap = floor(85/10) = 8.
        // Draining/low bucket → Conserve tier → no probe. owned 9 → cap stays 8
        // (so owned >= cap blocks growth; the live veto also fires).
        let draining = GovernorSnapshot::compute(2_000, -8.0, 500.0);
        let cap = ClaimOperation::compute_maximum_rooms(&f, budget, draining, 9, 20);
        assert_eq!(cap, 8);

        // Same numbers but healthy → probe one more: max(8, 10) = 10.
        let cap_healthy = ClaimOperation::compute_maximum_rooms(&f, budget, healthy_governor(), 9, 20);
        assert_eq!(cap_healthy, 10);
    }

    #[test]
    fn max_rooms_probes_with_mildly_negative_trend_at_full_bucket() {
        // A healthy empire at a near-full bucket sawtooths slightly negative;
        // the probe must still fire (regression: the old `trend >= 0` gate
        // would have blocked it, silently capping expansion).
        let f = ClaimFeatures::default();
        let budget = CpuBudget {
            cpu_used_estimate: Some(90.0),
            cpu_limit: 100.0,
        };
        // tier Normal (bucket 9000 ≥ 4000, trend −1 ≥ −5), bucket ≥ 8000 floor.
        let mildly_draining_but_full = GovernorSnapshot::compute(9_000, -1.0, 500.0);
        // est_room_cpu = 90/9 = 10 → estimate_cap = 8; probe → max(8, 10) = 10.
        let cap = ClaimOperation::compute_maximum_rooms(&f, budget, mildly_draining_but_full, 9, 20);
        assert_eq!(cap, 10);
    }

    #[test]
    fn max_rooms_clamped_by_gcl_and_safety_cap() {
        let f = ClaimFeatures::default();
        let budget = CpuBudget {
            cpu_used_estimate: None,
            cpu_limit: 10_000.0, // estimate_cap would be huge
        };
        // GCL is the hard ceiling.
        assert_eq!(ClaimOperation::compute_maximum_rooms(&f, budget, healthy_governor(), 2, 5), 5);
        // With abundant GCL, the max_room_cap safety bound (50) applies.
        assert_eq!(ClaimOperation::compute_maximum_rooms(&f, budget, healthy_governor(), 2, 100), 50);
    }
}
