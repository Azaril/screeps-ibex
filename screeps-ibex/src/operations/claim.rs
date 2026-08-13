use super::data::*;
use super::operationsystem::*;
use crate::military::objective_queue::{ForceRequirement, ObjectiveKind, ObjectiveOwner, ObjectiveRequest, OBJECTIVE_PRIORITY_MEDIUM};
use crate::military::threatmap::ThreatLevel;
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
use screeps_combat_decision::composition::CompositionParams;
use screeps_combat_decision::doctrine::{
    decide_doctrine, defense_doctrines, plan_engagement, DoctrineObjective, EnemyCoordination, EnemyForce, EngagementContext,
};
use screeps_combat_decision::force_sizing::DefenseProfile;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::collections::HashSet;

/// The heaviest hostile combat DPS an Escort pre-clear will screen against (ADR 0017 escort/pre-clear;
/// combat-overhaul-plan.md §W3). Above this the room is not a MARGINAL claim target but a genuine
/// contest — a claimer has no business there, so the escort is a NO-OP and the safety gate keeps
/// rejecting the candidate (the war/harass lane owns real assaults, not the claim pipeline). A
/// bot-side named constant (EP-4.6): calibration lands as a reviewed diff, not runtime config. Sized at
/// ~a single ATTACK creep's worth of DPS (2×ATTACK = 60), the "light screen" a claim escort should
/// clear ahead of the claimer without widening aggression.
const ESCORT_MAX_SCREEN_DPS: f32 = 60.0;

/// On-site window (ticks) a claim-escort screen has to deliver its clear — a normal-lifetime defender
/// mirroring the war.rs defense window. Feeds the composition optimizer's `deliverable` term.
const ESCORT_ONSITE_WINDOW: u32 = 1400;

/// EV target value handed to the escort composition optimizer. Mirrors the war.rs defense target value:
/// high enough that "EV > commit" ⇔ "winnable" so a winnable light screen is never deferred for low value.
const ESCORT_TARGET_VALUE: f32 = 1_000_000.0;

/// TTL (ticks) for an `Escort` pre-clear objective. UNLIKE the war/defense scans — which re-assert every
/// 1–40 ticks and so run tiny TTLs (`DEFEND_OBJECTIVE_TTL` 60 / `OFFENSE_OBJECTIVE_TTL` 100) — the escort
/// is re-asserted only ONCE per claim discover cycle (`emit_escort_objectives` fires from `run_select`,
/// which runs after the full Idle→Discover→Scouting→Select loop). That re-assert gap is
/// `discover_interval_eff` (500..=1500) + `scouting_window_eff` (≥200) ≈ 700..~4000 ticks, so the queue's
/// default 200-tick TTL would lapse the escort between cycles and it could vanish mid-pre-clear (right
/// while a claimer is spawning/en-route/clearing). Size the TTL to bridge BOTH the claimer's journey the
/// escort screens — spawn (~hundreds) + travel (≤ `max_claim_radius_hops` 11 × `TICKS_PER_HOP` 50 = 550) +
/// the on-site clear window (`ESCORT_ONSITE_WINDOW` 1400) — AND a worst-case re-assert gap (a
/// `max_discover_interval` 1500 cycle). 4000 clears the journey ceiling (~2500) plus a full slow cycle
/// with margin. Still AUTHORITATIVE (`.authoritative()`): a cleared threat / completed claim simply stops
/// re-asserting and the objective lapses within one cycle — the TTL only prevents a spurious mid-journey
/// gap, it does not latch a stale escort (the manager also retires the squad on world-state). A bot-side
/// named constant (EP-4.6): calibration lands as a reviewed diff, not runtime config.
const ESCORT_OBJECTIVE_TTL: u32 = 4000;

/// How long Select may HOLD waiting for the governor to sample back to Normal
/// before completing the cycle anyway (stall report §4: the tier is a per-tick
/// sample; consulting it at one instant let a single Conserve sawtooth tick
/// zero a whole discover cycle). ~One bucket-recovery horizon.
const SELECT_CPU_HOLD_MAX_TICKS: u32 = 300;

/// Maximum home rooms one claim mission may reserve (stall report §4): a claim
/// used to consume EVERY eligible home, starving all other candidates in the
/// same cycle via `used_home_rooms`. Two covers claimer + remote-build support.
const HOME_CONSUMPTION_CAP: usize = 2;

/// Plan-prefetch breadth per Scouting tick (stall report §4, M3): how many of
/// the best viable plan-less candidates get a `RoomPlanRequest` pushed while
/// scouting is still running, so plans exist by commit time.
const PLAN_PREFETCH_TOP_N: usize = 6;

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

    /// Re-discovery cadence scaled by the tracked search-area size (ADR 0038 D3): the more reachable rooms the
    /// last discover surfaced, the longer before we re-BFS + re-prioritise scouts — so a large frontier is
    /// re-scanned proportionally less often and scouting can complete instead of thrashing. Small/dense
    /// empires stay near the base interval.
    fn discover_interval_eff(&self, features: &crate::features::ClaimFeatures) -> u32 {
        let tracked = (self.candidates.len() + self.unknown_rooms.len()) as u32;
        features
            .discover_interval
            .saturating_add(features.rediscover_ticks_per_room.saturating_mul(tracked))
            .min(features.max_discover_interval.max(features.discover_interval))
    }

    /// Scouting window scaled so scouts can physically reach the frontier ring (~`TICKS_PER_HOP` per hop) plus
    /// a per-unknown term (ADR 0038 D3). Bounded by `max_scouting_window`; the coverage-early-exit still fires
    /// Select sooner when the reachable ring is covered, so this only raises the ceiling.
    fn scouting_window_eff(&self, features: &crate::features::ClaimFeatures) -> u32 {
        let radius = crate::missions::utility::max_claim_radius_hops();
        let travel = crate::missions::utility::TICKS_PER_HOP.saturating_mul(radius);
        let unknown = self.unknown_rooms.len() as u32;
        features
            .scouting_window
            .saturating_add(travel)
            .saturating_add(features.scout_ticks_per_room.saturating_mul(unknown))
            .min(features.max_scouting_window.max(features.scouting_window.saturating_add(travel)))
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

        // BFS/route-oracle alignment (stall report §4): a LIVE hostile-PLAYER
        // reservation denies movement (`routepricing::is_hostile_for_movement`),
        // so the expansion BFS must not search THROUGH such a room either —
        // otherwise Discover produces candidates whose corridors the route
        // oracle then refuses (phantom candidates; live example W11N56, kept
        // alive behind a reserved wall for weeks). Shares the routepricing
        // derivation: NPC "Invader" reservations and decayed (aged-out)
        // reservations do NOT wall — only a live player reservation does.
        let intel = crate::pathing::routepricing::RouteRoomIntel::from_dynamic(dynamic_visibility_data);
        let live_hostile_player_reservation =
            intel.reservation_hostile_player && intel.intel_age <= crate::pathing::routepricing::RESERVATION_MAX_AGE;

        let viable = has_controller && has_sources && can_claim && can_plan;
        let can_expand = (!hostile || confirmed_derelict) && !live_hostile_player_reservation;

        let candidate_room_data = CandidateRoomData::new(search_room_entity, viable, can_expand);

        Some(candidate_room_data)
    }

    /// Return a plan quality score (0–1) for a room that has a valid plan.
    /// Returns `None` if the room has no plan data or the plan failed.
    fn plan_score(system_data: &mut OperationExecutionSystemData, room_entity: Entity) -> Option<f32> {
        let plan_data = system_data.room_plan_data.get(room_entity)?;
        let plan = plan_data.plan()?;
        // PlanScore.total is already a 0–1 weighted average from screeps-foreman.
        Some(plan.score.total)
    }

    /// Score a candidate room via the unified economic value (ADR 0038 §2 Part B):
    /// `intrinsic owned-colony net-ROI × unlock_fraction(distance) × support_decay(distance) × plan_quality`.
    /// The intrinsic ROI is distance-INDEPENDENT (a claimed room self-hauls internally); distance enters only
    /// through `unlock_fraction` (the sprawl / anti-cannibalization term) and `support_decay`. Returns `None`
    /// only if the room has no visibility or no sources (no exploitable economy — also excluded by the viable
    /// gate). A not-yet-planned room scores with a neutral plan factor so it stays pursued; the HARD
    /// "no valid plan ⇒ no claim" gate is enforced at commit (ADR 0038 D7).
    fn score_candidate(
        system_data: &mut OperationExecutionSystemData,
        room_entity: Entity,
        distance: u32,
        features: &crate::features::ClaimFeatures,
    ) -> Option<(f32, CandidateSubScores)> {
        let source_count = {
            let room_data = system_data.room_data.get(room_entity)?;
            let static_visibility_data = room_data.get_static_visibility_data()?;
            static_visibility_data.sources().len() as u32
        };
        if source_count == 0 {
            return None;
        }

        // Optional plan-quality (a valid plan's 0–1 total); `None` while the room is not yet planned.
        let plan_total = Self::plan_score(system_data, room_entity);

        let params = crate::claim_economics::ClaimValueParams {
            ring_separation_hops: features.ring_separation_hops,
            unlock_floor: features.unlock_floor as f64,
            support_decay_k: features.support_decay_k as f64,
            internal_haul_tiles: features.internal_haul_tiles,
            roi_reference: features.roi_reference as f64,
        };

        let cv = crate::claim_economics::claim_value(source_count, distance, plan_total, &params);

        Some((
            cv.value,
            CandidateSubScores {
                roi: cv.roi,
                unlock: cv.unlock,
                decay: cv.decay,
                plan: plan_total,
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

        // Search the full claimer-viable range every cycle — the only real limit on what we may claim is
        // claimer reach (the `claim_route_feasible` gate + ClaimCorridor route pricing at commit, below), so
        // the BFS explores exactly that far. No adaptive ratchet: a far viable room is found on the first
        // discover, not after N widening cycles (ADR 0038 D1/D2). Each new colony re-seeds the BFS, so the
        // frontier crawls outward toward the world edge.
        let radius = crate::missions::utility::max_claim_radius_hops().max(1);

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

        // Request visibility for unknown rooms (critical priority). ADR 0046
        // D6: an unknown room is serviced by ANY sighting within the "known"
        // horizon — declare `want_fresh_within = VISIBILITY_TIMEOUT` so the
        // assigner stops touring it the moment it has been seen at all.
        for unknown_room in gathered_data.unknown_rooms().iter() {
            system_data.visibility.request(
                VisibilityRequest::new(unknown_room.room_name(), VISIBILITY_PRIORITY_CRITICAL, VisibilityRequestFlags::ALL)
                    .want_fresh_within(Self::VISIBILITY_TIMEOUT),
            );
        }

        // Request visibility for candidate rooms that are going stale. ADR 0046
        // D6: candidates declare the commit gate's real freshness need
        // (`intel_freshness_ticks`) so scouts arrive BECAUSE the commit gate
        // needs them.
        let candidate_freshness = system_data.features.claim.intel_freshness_ticks;
        for candidate_room in gathered_data.candidate_rooms().iter() {
            if let Some(room_data) = system_data.room_data.get(candidate_room.room_data_entity()) {
                if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                    if dynamic_visibility_data.age() > Self::VISIBILITY_TIMEOUT / 2 {
                        system_data.visibility.request(
                            VisibilityRequest::new(room_data.name, VISIBILITY_PRIORITY_HIGH, VisibilityRequestFlags::ALL)
                                .want_fresh_within(candidate_freshness),
                        );
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
        // Unknown rooms need critical-priority visibility. ADR 0046 D6: any
        // sighting within the "known" horizon services an unknown room, so the
        // every-tick re-assert is harmless — the assigner's freshness filter
        // (not the entry's existence) decides servicing.
        for room_name in &self.unknown_rooms {
            system_data.visibility.request(
                VisibilityRequest::new(*room_name, VISIBILITY_PRIORITY_CRITICAL, VisibilityRequestFlags::ALL)
                    .want_fresh_within(Self::VISIBILITY_TIMEOUT),
            );
        }

        // Candidates need high-priority visibility while they are unscored OR
        // while their dynamic intel is too stale to pass the commit-time safety
        // re-check (`intel_freshness_ticks`). Without the staleness clause a
        // candidate scored from never-stale STATIC data (sources/terrain/
        // distance/plan) is treated as "done" and dropped from the scout queue,
        // so its DYNAMIC intel never refreshes — it then fails the commit-time
        // freshness check every cycle and is never claimed (the "scouts never
        // refresh the claim frontier in time" bug). The refresh must key off
        // "is my safety intel fresh", not "do I have a score".
        let freshness = system_data.features.claim.intel_freshness_ticks;
        for candidate in &self.candidates {
            let stale = system_data
                .mapping
                .get_room(&candidate.room_name)
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|d| !d.updated_within(freshness))
                .unwrap_or(true);
            if candidate.score.is_none() || stale {
                // ADR 0046 D6: the candidate declares the commit gate's real
                // freshness need — the assigner keeps it serviced within
                // `intel_freshness_ticks`, so the commit-time re-check passes.
                system_data.visibility.request(
                    VisibilityRequest::new(candidate.room_name, VISIBILITY_PRIORITY_HIGH, VisibilityRequestFlags::ALL)
                        .want_fresh_within(freshness),
                );
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

            // Viability + pre-claim safety gate (ADR 0017). A rejected room is
            // marked with a negative score so it is pruned in run_select and a
            // claimer is never dispatched into a contested room or a room we
            // recently abandoned.
            let mut reject = false;
            if let Some(room_data) = system_data.room_data.get(room_entity) {
                if let Some(dynamic) = room_data.get_dynamic_visibility_data() {
                    // Always reject a room owned by another player (claim impossible).
                    if dynamic.owner().hostile() {
                        reject = true;
                    } else if features.safety_gate {
                        let now = game::time();
                        let avoided = system_data.expansion_avoidance.is_avoided(candidate.room_name, now);
                        let threat = system_data.threat_data.get(room_entity);
                        // Reject only on an ACTIVE threat (or avoid-cooldown)
                        // here — NOT on staleness (u32::MAX skips the freshness
                        // check). A stale-but-clean candidate must stay scoreable
                        // so it isn't permanently rejected before re-scouting; the
                        // freshness requirement is enforced live at commit time
                        // in run_select.
                        if avoided || !crate::missions::utility::is_claim_target_safe(threat, dynamic, u32::MAX) {
                            reject = true;
                        }
                    }
                }
            }

            // A room whose requested plan FAILED is unclaimable NOW (REC-025):
            // score it negative like the hostile reject — otherwise
            // `plan_score` → None maps to `plan_quality`'s NEUTRAL 1.0 and an
            // unbuildable room outranks planned-but-mediocre rooms while the
            // commit gate skips it every cycle (a permanent phantom top
            // candidate). Deliberately the SAME kernel as the commit-time
            // gate (`plan_commit_gate`) so the two sites cannot drift.
            // Discover's `can_plan` already excludes rooms whose plans failed
            // BEFORE the cycle; this closes the mid-cycle window (a plan
            // requested at commit that fails during scouting). The next
            // discover re-evaluates fresh, so a later successful replan makes
            // the room claimable again.
            if plan_commit_gate(system_data.room_plan_data.get(room_entity).map(|plan| plan.valid())) == PlanCommitGate::SkipInvalid {
                reject = true;
            }

            if reject {
                // Mark as unscoreable by setting a negative score.
                candidate.score = Some((
                    -1.0,
                    CandidateSubScores {
                        roi: 0.0,
                        unlock: 0.0,
                        decay: 0.0,
                        plan: None,
                    },
                ));
                continue;
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
        let bucket_healthy = governor.tier == crate::cpugovernor::Tier::Normal && governor.bucket >= features.healthy_bucket_floor;

        let structural = if bucket_healthy {
            estimate_cap.max(currently_owned_rooms + 1)
        } else {
            estimate_cap
        };

        // Safety caps bound the CPU-derived number; GCL is the hard ceiling.
        structural.max(features.min_room_cap).min(features.max_room_cap).min(current_gcl)
    }

    /// Whether the reachable ring at the current radius is fully covered:
    /// every viable candidate scored, and every unknown frontier room either
    /// resolved (now has visibility) or given up on (scout-unreachable
    /// backoff). Lets Select fire as soon as coverage lands instead of always
    /// waiting out the full scouting window — and prevents a hostile-walled,
    /// never-scoutable room from blocking selection forever.
    ///
    /// Deliberately does NOT require every viable candidate's dynamic intel to
    /// be simultaneously fresh (stall report §4, M2-near): with N candidates
    /// and one scout, freshness windows cannot all overlap, so a simultaneous-
    /// freshness clause held coverage open for whole cycles while each room's
    /// intel expired as the next was visited. The staleness that clause guarded
    /// against is handled where it belongs — the per-candidate commit-time
    /// safety re-check — and the ROLLING commit (`try_commit_candidates` from
    /// the Scouting phase) claims each far candidate in the tick its OWN intel
    /// is fresh, so no candidate needs to be fresh at any shared instant.
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

    /// Plan prefetch (stall report §4 — kills M3): push a `RoomPlanRequest` for the top-N viable,
    /// plan-LESS candidates every Scouting tick. Plans are produced asynchronously by
    /// `roomplansystem` (budgeted, one room at a time), so requesting them only at commit time
    /// meant the first cycle a candidate became otherwise-committable was always burned waiting
    /// for a plan ("the plan mystery": the gate ordering meant the request was often never even
    /// made). Priority rides UNDER construction missions (they use 1.0): `0.5 + score.clamp(0,
    /// 0.45)` plans better candidates first without ever pre-empting a live colony's replan. The
    /// queue is per-tick (cleared after each planner step), so re-pushing every tick is the
    /// intended keep-alive, not a leak.
    fn prefetch_candidate_plans(&self, system_data: &mut OperationExecutionSystemData) {
        let mut viable: Vec<(f32, RoomName)> = self
            .candidates
            .iter()
            .filter_map(|c| c.score.map(|(s, _)| (s, c.room_name)))
            .filter(|(s, _)| *s >= 0.0)
            .collect();
        // Best-first, name tie-break (determinism — [[sim-determinism-fence]]).
        viable.sort_by(|a, b| {
            let qa = crate::claim_economics::claim_rank_quantize(a.0);
            let qb = crate::claim_economics::claim_rank_quantize(b.0);
            qb.cmp(&qa).then(a.1.cmp(&b.1))
        });
        let mut requested = 0usize;
        for (score, room_name) in viable {
            if requested >= PLAN_PREFETCH_TOP_N {
                break;
            }
            let Some(entity) = system_data.mapping.get_room(&room_name) else {
                continue;
            };
            // Only plan-LESS candidates: an existing plan (valid or failed) is the commit gate's
            // + roomplansystem's business (failed plans re-request there under the replan backoff).
            if system_data.room_plan_data.get(entity).is_some() {
                continue;
            }
            let priority = 0.5 + score.clamp(0.0, 0.45);
            system_data.room_plan_queue.request(RoomPlanRequest::new(entity, priority));
            requested += 1;
        }
    }

    // ── Phase: Select ───────────────────────────────────────────────────────

    /// Final selection at the scouting window's end: score any stragglers, emit
    /// escort screens, prune dead candidates, then run the SAME commit-gate
    /// chain the rolling pass uses ([`Self::try_commit_candidates`]) one last
    /// time — this is where below-ring candidates get their coverage-gated
    /// last-resort chance (ADR 0038 D9; far candidates normally commit earlier,
    /// from the rolling Scouting pass) — and transition back to Idle.
    fn run_select(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
        maximum_rooms: u32,
        currently_owned_rooms: u32,
        features: &crate::features::ClaimFeatures,
    ) {
        // Drop elapsed avoid-cooldown entries so abandoned rooms become
        // re-claimable once their cooldown passes (ADR 0017).
        system_data.expansion_avoidance.prune(game::time());

        // Final scoring pass for any candidates still unscored.
        self.try_score_candidates(system_data, features);

        // Coverage snapshot BEFORE pruning (prune drops unscored, which would trivially "complete" coverage).
        // Gates the cannibalization-patience check in the commit chain: a below-ring (cannibalizing) room is
        // only claimed once the reachable far frontier is fully scouted-or-given-up (ADR 0038 D9), so a closer
        // room never pre-empts a farther one that is merely still being scouted.
        let covered = self.scouting_coverage_complete(system_data);

        // CPU one-tick-sampling guard (stall report §4): the governor tier is a
        // per-tick sample, and Select previously consulted it at exactly ONE
        // instant — a single Conserve-tick of bucket sawtooth there zeroed the
        // whole discover cycle. Hold the phase and retry next tick instead,
        // bounded so a genuinely stressed empire still completes the cycle
        // (capacity-gated to zero missions) rather than parking in Select.
        if system_data.governor.tier != crate::cpugovernor::Tier::Normal {
            let held = self.phase_tick.map(|t| game::time().saturating_sub(t)).unwrap_or(u32::MAX);
            if held < SELECT_CPU_HOLD_MAX_TICKS {
                return;
            }
        }

        // Escort pre-clear (ADR 0017; combat-overhaul-plan.md §W3): emit an Escort screen for any MARGINAL
        // threatened candidate BEFORE the prune below drops the threat-rejected ones. A NO-OP for clean
        // claims and for any threat too heavy to be a light screen (see `escort_screen_decision`).
        self.emit_escort_objectives(system_data, features);

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

        self.try_commit_candidates(system_data, runtime_data, maximum_rooms, currently_owned_rooms, features, covered, false, true);

        // No adaptive-radius ratchet (ADR 0038 D1): the BFS searches the full claimer-viable range every
        // discover cycle, so there is no radius to widen/re-tighten. Expansion reach grows only by claiming
        // (each new colony re-seeds the BFS outward).

        // Transition back to Idle, recording the current tick for the
        // re-discover interval.
        self.phase_tick = Some(game::time());
        self.phase = ClaimPhase::Idle;
    }

    /// The ONE commit-gate chain (stall report §4 "rolling commit" — kills M2-far): capacity/CPU
    /// gates, then per ranked candidate [score-delta, below-ring patience, plan gate, commit-time
    /// safety re-check, existing-mission check, eligible-home feasibility] — creating a claim
    /// mission for every candidate that passes EVERY gate in THIS tick. Returns missions created.
    ///
    /// Called from two places:
    /// - **Scouting, every tick** (`far_only = true`, quiet): a far (>= ring) candidate commits the
    ///   moment its OWN intel is fresh + safe + planned + home-reachable, instead of at one sampled
    ///   Select instant — with one scout rotating N rooms, per-candidate freshness windows rarely
    ///   overlap the window's end, so the single sampled instant burned whole cycles (M2-far).
    /// - **Select, once at window end** (`far_only = false`, `verbose`): the below-ring stragglers'
    ///   coverage-gated last-resort chance (ADR 0038 D9) + the cycle's ranked summary.
    ///
    /// Up to `max_concurrent_missions` claim missions may be active at once (0 = unlimited, capped
    /// by GCL/CPU). Additional candidates beyond the first are only selected if their score is
    /// within `max_score_delta` of the best, preventing vastly inferior rooms from being claimed.
    ///
    /// Gate ORDER matters (stall report §4, M3): the plan gate runs BEFORE the freshness/safety
    /// skip, so a plan-less candidate gets its plan REQUESTED even while its intel happens to be
    /// stale — previously the staleness skip came first, the request line was never reached, and a
    /// perfectly good candidate could never become committable.
    #[allow(clippy::too_many_arguments)]
    fn try_commit_candidates(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
        maximum_rooms: u32,
        currently_owned_rooms: u32,
        features: &crate::features::ClaimFeatures,
        covered: bool,
        far_only: bool,
        verbose: bool,
    ) -> usize {
        // Ranked snapshot (cloned so the commit loop below can mutate `self`): scored-viable
        // candidates, rolling-pass-filtered to far (>= ring), in the deterministic total order —
        // quantized score DESC then room name ASC (ADR 0038 D8: the quantization stops f64 rounding
        // from splitting a genuine tie; the name tie-break removes the seed-flaky HashMap iteration
        // order the BFS would otherwise leak — [[sim-determinism-fence]]).
        let mut ranked: Vec<CachedCandidate> = self
            .candidates
            .iter()
            .filter(|c| c.score.map(|(s, _)| s >= 0.0).unwrap_or(false))
            .filter(|c| !far_only || c.distance >= features.ring_separation_hops)
            .cloned()
            .collect();
        ranked.sort_by(|a, b| {
            let qa = crate::claim_economics::claim_rank_quantize(a.score.map(|(s, _)| s).unwrap_or(0.0));
            let qb = crate::claim_economics::claim_rank_quantize(b.score.map(|(s, _)| s).unwrap_or(0.0));
            qb.cmp(&qa).then(a.room_name.cmp(&b.room_name))
        });
        if ranked.is_empty() {
            return 0;
        }

        if verbose {
            // Log the ranked candidates.
            for (i, candidate) in ranked.iter().enumerate() {
                if let Some((score, sub)) = candidate.score {
                    let plan_label = sub.plan.map(|p| format!(" plan={:.2}", p)).unwrap_or_default();
                    info!(
                        "ClaimOp [Select]:   #{} {} score={:.3} (roi={:.2} unlock={:.2} decay={:.2}{}) dist={} homes=[{}]",
                        i + 1,
                        candidate.room_name,
                        score,
                        sub.roi,
                        sub.unlock,
                        sub.decay,
                        plan_label,
                        candidate.distance,
                        candidate.home_rooms.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","),
                    );
                }
            }
        }

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

        if verbose {
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
        }

        let max_new_missions = if at_capacity {
            if verbose {
                info!(
                    "ClaimOp [Select]: no new missions (active={} max_rooms={} cpu_healthy={} features.on={})",
                    active_rooms, maximum_rooms, cpu_healthy, features.on
                );
            }
            0
        } else {
            // Cap by both room headroom and mission concurrency limit.
            (available_rooms as usize).min(mission_headroom)
        };

        let mut missions_created = 0;

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

            let best_score = ranked.first().and_then(|c| c.score.map(|(s, _)| s)).unwrap_or(0.0);

            for candidate in ranked.iter() {
                if missions_created >= max_new_missions {
                    break;
                }

                // Enforce score delta: additional candidates beyond the first
                // must be within max_score_delta of the best.
                let candidate_score = candidate.score.map(|(s, _)| s).unwrap_or(0.0);
                if missions_created > 0 && (best_score - candidate_score) > features.max_score_delta {
                    if verbose {
                        info!(
                            "ClaimOp [Select]: candidate {} score={:.3} exceeds delta {:.3} from best {:.3}, stopping",
                            candidate.room_name, candidate_score, features.max_score_delta, best_score,
                        );
                    }
                    break;
                }

                // Cannibalization patience (ADR 0038 D9): a below-ring room overlaps an existing colony's
                // radius-1 remote-mining ring, so it is claimed only as a LAST RESORT — once scouting coverage
                // is complete (the reachable far frontier is fully scouted-or-given-up and offers nothing
                // better). While farther rooms may still be scouted, a closer room must NOT pre-empt them. This
                // is NOT the old hard floor: it is gated on coverage (converges as far unknowns resolve), never
                // on a radius ratchet, so a genuinely boxed-in empire still expands once the frontier is
                // exhausted. Far (>= ring) candidates are never gated here; they claim as soon as scored.
                if !crate::claim_economics::may_claim_below_ring(candidate.distance, features.ring_separation_hops, covered) {
                    info!(
                        "ClaimOp [Select]: candidate {} at distance {} < ring {} deferred (waiting for farther rooms; frontier not yet fully scouted)",
                        candidate.room_name, candidate.distance, features.ring_separation_hops
                    );
                    continue;
                }

                let candidate_entity = match system_data.mapping.get_room(&candidate.room_name) {
                    Some(e) => e,
                    None => {
                        if verbose {
                            info!(
                                "ClaimOp [Select]: top candidate {} has no entity mapping, skipping",
                                candidate.room_name
                            );
                        }
                        continue;
                    }
                };

                // Plan gate (ADR 0038 D7 / REC-025): checked on VALIDITY, not
                // just presence. A plan that FAILED during the scouting window
                // previously passed the old `is_none()` presence check and the
                // room could be claimed despite being unbuildable — an
                // irreversible GCL commit (`should_abandon_claim` never fires
                // without hostiles), violating claim_economics' "no valid plan
                // ⇒ no claim" contract.
                //
                // Runs BEFORE the safety/freshness skip (stall report §4, M3):
                // a plan-less candidate must get its plan REQUESTED even while
                // its intel happens to be stale — with the old order, staleness
                // `continue`d first and the request line was never reached, so
                // "no plan" and "stale intel" starved each other forever.
                match plan_commit_gate(system_data.room_plan_data.get(candidate_entity).map(|p| p.valid())) {
                    PlanCommitGate::RequestPlan => {
                        if verbose {
                            info!(
                                "ClaimOp [Select]: top candidate {} has no room plan, requesting one",
                                candidate.room_name
                            );
                        }
                        system_data.room_plan_queue.request(RoomPlanRequest::new(candidate_entity, 0.5));
                        continue;
                    }
                    PlanCommitGate::SkipInvalid => {
                        if verbose {
                            warn!(
                                "ClaimOp [Select]: top candidate {} has a FAILED room plan — hard skip (no valid plan ⇒ no claim); re-requesting a plan (roomplansystem's replan backoff still applies)",
                                candidate.room_name
                            );
                        }
                        system_data.room_plan_queue.request(RoomPlanRequest::new(candidate_entity, 0.5));
                        continue;
                    }
                    PlanCommitGate::Proceed => {}
                }

                // Commit-time safety re-validation (ADR 0017): intel can change
                // during the scouting window, and "absence of fresh intel is not
                // safety". Skip (do not claim) a candidate that is now contested,
                // in avoid-cooldown, or whose intel is stale — keep scouting it.
                // (The rolling pass retries every tick, so a candidate skipped
                // here commits the tick its own intel comes fresh — M2-far.)
                if features.safety_gate {
                    let now = game::time();
                    let safe = match system_data.mapping.get_room(&candidate.room_name) {
                        Some(e) if !system_data.expansion_avoidance.is_avoided(candidate.room_name, now) => system_data
                            .room_data
                            .get(e)
                            .and_then(|rd| rd.get_dynamic_visibility_data())
                            .map(|dynamic| {
                                crate::missions::utility::is_claim_target_safe(
                                    system_data.threat_data.get(e),
                                    dynamic,
                                    features.intel_freshness_ticks,
                                )
                            })
                            .unwrap_or(false),
                        _ => false,
                    };
                    if !safe {
                        if verbose {
                            info!(
                                "ClaimOp [Select]: candidate {} failed commit-time safety re-check, skipping",
                                candidate.room_name
                            );
                        }
                        continue;
                    }
                }

                let mission_data = system_data.mission_data;

                let has_claim_mission = match system_data.room_data.get(candidate_entity) {
                    Some(room_data) => room_data
                        .get_missions()
                        .iter()
                        .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<ClaimMission>().is_some()),
                    None => {
                        if verbose {
                            info!("ClaimOp [Select]: top candidate {} has no room data, skipping", candidate.room_name);
                        }
                        continue;
                    }
                };

                if has_claim_mission {
                    if verbose {
                        info!("ClaimOp [Select]: top candidate {} already has a claim mission", candidate.room_name);
                    }
                } else {
                    // Eligible homes: restricted to `candidate.home_rooms` — the
                    // Discover BFS reached this candidate from exactly those homes
                    // through hostile-free corridors (`can_expand` prunes
                    // hostile-owned rooms), so intersecting transfers the BFS's
                    // reachability guarantee to the chosen home instead of
                    // re-deriving it over ALL owned rooms (REC-024). Each
                    // surviving home must additionally be uncommitted, able to
                    // AFFORD a claimer ([Claim, Move] = 650 energy ⇒ ~RCL 3
                    // capacity — an RCL 2 home would silently fail create_body),
                    // and within CLAIM-creep reach (below; claim feasibility
                    // implies the colony is also build-feasible).
                    let candidate_name = candidate.room_name;
                    let claimer_cost = Part::Claim.cost() + Part::Move.cost();

                    // Reach-oracle route pricing (REC-024): cached-intel pricing
                    // that DENIES hostile rooms — the same predicate the
                    // claimer's own mover applies (`HostileBehavior::Deny`) —
                    // instead of the legacy `game::rooms()` callback, which
                    // cannot see invisible corridor rooms and priced
                    // hostile-owned ones at a traversable default. The pricing
                    // POLICY lives here with the caller; the route algorithm and
                    // cache stay in `PathfinderService` (no-one-off-pathfinding).
                    let derelict_pathing_on = system_data.features.derelict.on;
                    let mapping = system_data.mapping;
                    let room_datas = &*system_data.room_data;
                    let route_cost = |room: RoomName| -> Option<f64> {
                        let intel = mapping
                            .get_room(&room)
                            .and_then(|e| room_datas.get(e))
                            .and_then(|rd| rd.get_dynamic_visibility_data())
                            .map(crate::pathing::routepricing::RouteRoomIntel::from_dynamic);
                        crate::pathing::routepricing::economy_route_cost(intel, derelict_pathing_on)
                    };

                    // A home is eligible if it is uncommitted, can afford a claimer, and is
                    // CLAIM-reachable through hostile-free corridors (`claim_route_feasible`
                    // over the ClaimCorridor route pricing). `restrict_to_bfs_homes` scopes
                    // the pass to `candidate.home_rooms` (the BFS's minimal-distance homes).
                    let mut collect_eligible_homes = |restrict_to_bfs_homes: bool| -> Vec<Entity> {
                        let mut out: Vec<(RoomName, Entity)> = Vec::new();
                        for (entity, home_room_name, _max_level) in home_room_data.iter() {
                            if used_home_rooms.contains(entity) {
                                continue;
                            }
                            if restrict_to_bfs_homes && !candidate.home_rooms.contains(home_room_name) {
                                continue;
                            }
                            let energy_capacity = game::rooms()
                                .get(*home_room_name)
                                .map(|r| r.energy_capacity_available())
                                .unwrap_or(0);
                            if energy_capacity < claimer_cost {
                                continue;
                            }
                            let route = system_data.pathfinder.route_distance_via(
                                *home_room_name,
                                candidate_name,
                                game::time(),
                                crate::pathing::pathfinderservice::RoutePolicy::ClaimCorridor,
                                &route_cost,
                            );
                            if claim_route_feasible(route) {
                                out.push((*home_room_name, *entity));
                            }
                        }
                        // Home consumption cap (stall report §4): one claim used to
                        // reserve EVERY eligible home, so a single mission consumed the
                        // whole empire's spawn capacity and `used_home_rooms` starved
                        // every other candidate this cycle. Two homes are ample for one
                        // claimer + remote-build support; deterministic order (room name)
                        // keeps the pick stable across ticks ([[sim-determinism-fence]]).
                        out.sort_by_key(|(name, _)| *name);
                        out.truncate(HOME_CONSUMPTION_CAP);
                        out.into_iter().map(|(_, entity)| entity).collect()
                    };

                    // Prefer the BFS's minimal-distance homes. REC-069: those are recorded
                    // only at first-visit distance, so a farther-but-eligible home is
                    // silently excluded (sticky when a corridor near the nearest home stays
                    // hostile-reserved). If the restricted set empties, fall back to the FULL
                    // owned-home set — the `claim_route_feasible` + ClaimCorridor route check
                    // re-derives the hostile-free reachability guarantee the BFS provided, so
                    // the fallback never sends a claimer through a hostile corridor.
                    let mut home_room_entities = collect_eligible_homes(true);
                    if home_room_entities.is_empty() {
                        home_room_entities = collect_eligible_homes(false);
                        if !home_room_entities.is_empty() {
                            info!(
                                "ClaimOp [Select]: candidate {} had no BFS-recorded eligible home; a farther owned home is claim-reachable — using it (REC-069)",
                                candidate.room_name
                            );
                        }
                    }

                    if home_room_entities.is_empty() {
                        if verbose {
                            info!(
                                "ClaimOp [Select]: top candidate {} has no eligible home rooms (all used, can't afford a claimer, or not claim-reachable through hostile-free corridors)",
                                candidate.room_name
                            );
                        }
                    } else {
                        let Some(room_data) = system_data.room_data.get_mut(candidate_entity) else {
                            if verbose {
                                info!("ClaimOp [Select]: top candidate {} has no room data, skipping", candidate.room_name);
                            }
                            continue;
                        };

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

            if verbose && missions_created == 0 {
                info!(
                    "ClaimOp [Select]: had {} scored candidates but created no missions",
                    ranked.len()
                );
            }
        }

        missions_created
    }

    // ── Escort pre-clear producer (ADR 0017; combat-overhaul-plan.md §W3) ────

    /// For each scored candidate that is a MARGINAL claim target — one the claim pipeline is actually
    /// pursuing (viable: controller + sources + neutral owner + plannable) but whose target carries a
    /// detected-but-modest creep threat the pre-claim safety gate rejects — emit an `Escort{room}`
    /// objective so a small defensive screen clears/screens the room ahead of the claimer, instead of
    /// losing the claimer into it. Called from `run_select` BEFORE the hostile/unscored prune, so the
    /// rejected-by-threat candidates (marked with a negative score) are still present.
    ///
    /// Conservative by construction — the EXACT NO-OP conditions (this loop `continue`s or
    /// [`escort_screen_decision`] returns `false`, so no escort is emitted):
    /// - **Feature off** — `!features.on || !features.safety_gate` (the whole pre-claim safety lifecycle
    ///   rides the `safety_gate` master kill-switch; this producer disables with it).
    /// - **No intel** — the candidate has no room entity, no `RoomData`, or no cached dynamic-visibility
    ///   read; or the read is stale (`intel_fresh` — ADR 0017: absence of fresh intel is NOT safety).
    /// - **Not in the pursued set** — the room is EXPANSION-AVOIDED (deliberately abandoned) or UNPLANNABLE
    ///   (`plan_commit_gate == SkipInvalid`); both mirror `try_score_candidates` so the escort never reaches
    ///   into a room the claim pipeline itself would not pursue.
    /// - **Not claimable at all** — a hostile/friendly OWNER or a blocked RESERVATION (claiming is
    ///   impossible regardless of threat — the war/harass lane's business, not the claim escort's).
    /// - **Clean claim** — `!warrants_attention` (`ThreatLevel::None`): a threat-free candidate never
    ///   produces an escort (no wasted screens).
    /// - **Threat arm did not reject** — `!threat_present`: the room is claimable as-is (e.g. a lone 0-DPS
    ///   `PlayerScout` that `warrants_attention` but `is_claim_target_safe` treats as CLEAN); a screen here
    ///   would be redundant AND widen aggression toward a room outside the contested set.
    /// - **Too heavy for a light screen** — a hostile TOWER at the edge, an inbound NUKE, a `PlayerSiege`,
    ///   or `attack_dps` over the [`ESCORT_MAX_SCREEN_DPS`] ceiling: a full assault the claim pipeline must
    ///   never provoke (no aggression widening).
    /// - **Unfieldable this tick** — no in-range home can build even the minimal screen composition.
    ///
    /// The escort keys on the pre-claim safety gate's THREAT arm ONLY. `threat_present` mirrors that arm
    /// verbatim (`threat_level >= PlayerRaid` OR `estimated_attack_dps > 0.0`), NOT the full set of signals
    /// `is_claim_target_safe` consumes (the owner/reservation guards above are producer-side claimability
    /// filters, not the safety gate's non-threat rejection reasons). It reads only the always-cached dynamic
    /// visibility plus the existing `RoomThreatData`, inventing no new scouting. It sizes the screen through
    /// the SAME doctrine driver (`decide_doctrine` then `plan_engagement`, `ClearCreeps`) that war.rs uses
    /// for defense, and upserts onto the SAME `CombatObjectiveQueue` the `SquadManager` pulls.
    fn emit_escort_objectives(&self, system_data: &mut OperationExecutionSystemData, features: &crate::features::ClaimFeatures) {
        // The escort producer is part of the pre-claim safety lifecycle (ADR 0017); when that whole
        // machinery is switched off, produce nothing.
        if !features.on || !features.safety_gate {
            return;
        }

        let now = game::time();
        let defense_docs = defense_doctrines();

        for candidate in &self.candidates {
            let Some(room_entity) = system_data.mapping.get_room(&candidate.room_name) else {
                continue;
            };
            let Some(room_data) = system_data.room_data.get(room_entity) else {
                continue;
            };
            let Some(dynamic) = room_data.get_dynamic_visibility_data() else {
                continue;
            };

            // Restrict to candidates the claim pipeline is ACTUALLY pursuing — a room viable-but-for-threat,
            // NOT one rejected for a non-threat reason (ESCORT-W3 §W3 contract; ADR 0017). The escort fires
            // only when the room's SOLE blocker is a marginal creep presence; a room rejected for avoidance
            // or an unbuildable plan is scored -1.0 in `try_score_candidates` too, so gating on "negative
            // score" alone would leak. Mirror the SAME viability signals `try_score_candidates` checks so the
            // two sites cannot drift:
            //  (1) EXPANSION-AVOIDANCE — a room the bot DELIBERATELY abandoned; no claimer will ever follow.
            //  (2) UNPLANNABLE — `plan_commit_gate == SkipInvalid`; a room the bot can NEVER claim.
            // Either would otherwise spawn+dispatch a real screen squad into a room outside the pursued set
            // (wasted energy/CPU/squad-slot + aggression widened into non-claim rooms). Both guards are the
            // pure `escort_candidate_viable` kernel (pin-tested).
            let avoided = system_data.expansion_avoidance.is_avoided(candidate.room_name, now);
            let plan_gate = plan_commit_gate(system_data.room_plan_data.get(room_entity).map(|plan| plan.valid()));
            if !escort_candidate_viable(avoided, plan_gate) {
                continue;
            }

            let threat = system_data.threat_data.get(room_entity);

            // Gather the cheap threat/intel signals — exactly the inputs the pre-claim safety gate reads.
            // `threat_present` mirrors the THREAT arm of `is_claim_target_safe` verbatim (`threat_level >=
            // PlayerRaid` OR `estimated_attack_dps > 0.0`) so the escort fires exactly for the rooms that gate
            // rejects on threat — not for a harmless 0-DPS presence (a lone scout) the gate would claim.
            let (attack_dps, heal, count, warrants_attention, threat_present, siege, nukes) = match threat {
                Some(t) => (
                    t.estimated_attack_dps,
                    t.estimated_heal,
                    t.hostile_creeps.len() as u32,
                    t.warrants_attention(),
                    t.threat_level >= ThreatLevel::PlayerRaid || t.estimated_attack_dps > 0.0,
                    t.threat_level >= ThreatLevel::PlayerSiege,
                    !t.incoming_nukes.is_empty(),
                ),
                None => (0.0, 0.0, 0, false, false, false, false),
            };

            let inputs = EscortScreenInputs {
                intel_fresh: dynamic.updated_within(features.intel_freshness_ticks),
                owner_hostile: dynamic.owner().hostile(),
                owner_friendly: dynamic.owner().friendly(),
                reservation_blocked: dynamic.reservation().hostile() || dynamic.reservation().friendly(),
                tower_present: dynamic.tower_dps_at_edge().is_some(),
                nukes_incoming: nukes,
                siege,
                warrants_attention,
                threat_present,
                attack_dps,
            };

            // Kernel: a marginal-threatened candidate warrants a screen; everything else is a NO-OP. On a
            // `true` verdict the screen is sized directly off the observed `attack_dps` (certified within
            // the light-screen ceiling by the kernel).
            if !escort_screen_decision(inputs, ESCORT_MAX_SCREEN_DPS) {
                continue;
            }

            // Size a MINIMAL screen through the same doctrine driver war.rs uses for defense: a
            // `ClearCreeps` clear against the modest observed force. Size the members to the strongest
            // BFS-home's spawn capacity (the screen spawns from a real colony, not the unowned target).
            let member_energy = candidate
                .home_rooms
                .iter()
                .filter_map(|r| game::rooms().get(*r))
                .map(|r| r.energy_capacity_available())
                .max()
                .unwrap_or(0);

            let ctx = EngagementContext {
                objective: DoctrineObjective::ClearCreeps,
                coordination: EnemyCoordination::Coordinated,
                defense: DefenseProfile::default(),
                enemy_force: Some(EnemyForce {
                    dps: attack_dps,
                    heal,
                    hits: 0,
                    count: count.max(1),
                    boosted: false,
                }),
                importance: 0.0,
                member_energy,
                target_value: ESCORT_TARGET_VALUE,
                onsite_window: ESCORT_ONSITE_WINDOW,
                params: CompositionParams {
                    member_energy,
                    ..Default::default()
                },
                // A PRESENT (if modest) creep threat drives the screen — never confirmed-undefended.
                defense_intel_reliable: false,
            };

            let Some(composition) = decide_doctrine(&ctx, &defense_docs).and_then(|d| plan_engagement(d, &ctx, None).composition) else {
                // No in-range home can build even the minimal screen — skip (can't field it this tick).
                continue;
            };

            info!(
                "ClaimOp [Escort]: marginal claim target {} carries a light threat (dps={:.0}, heal={:.0}, count={}) — emitting Escort screen",
                candidate.room_name, attack_dps, heal, count,
            );

            // Upsert the Escort objective. `authoritative` so a de-escalating threat's priority decays
            // each scan (REC-041) — the escort follows the current threat, never latches a stale one.
            system_data.combat_objective_queue.request(
                ObjectiveRequest::new(
                    ObjectiveKind::Escort { room: candidate.room_name },
                    OBJECTIVE_PRIORITY_MEDIUM,
                    ForceRequirement::single(composition),
                )
                .owner(ObjectiveOwner::Claim)
                // Sized to bridge the claimer's spawn+travel+clear journey plus a full slow discover cycle
                // (see `ESCORT_OBJECTIVE_TTL`): re-asserted only once per claim Select cycle, so the queue's
                // default 200-tick TTL would lapse it mid-pre-clear.
                .ttl(ESCORT_OBJECTIVE_TTL)
                .authoritative(),
                now,
            );
        }
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
        let mut max_rcl: u32 = 0;

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
                    max_rcl = max_rcl.max(rcl);
                }
            }
        }

        // If we have no rooms, treat as "ready" so we don't block forever on an
        // empty empire (the discovery readiness gate below checks max_rcl).
        if currently_owned_rooms == 0 {
            max_rcl = u32::MAX;
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

                if elapsed >= self.discover_interval_eff(&features.claim) {
                    // Readiness gate (stall report §4, RCL2-freeze fix): at least ONE
                    // owned room must be RCL >= 2 — i.e. the empire has a colony
                    // mature enough to fund a claimer. The old `min_rcl >= 2`
                    // (EVERY room) froze all discovery whenever any single new
                    // colony was bootstrapping at RCL 1 — which is precisely when
                    // the empire is growing — and the capacity math already
                    // throttles simultaneous bootstraps. `max_rcl` is MAX when no
                    // rooms are owned (don't block an empty empire's re-seed).
                    if max_rcl >= 2 {
                        self.run_discover(system_data);
                    }
                }
            }
            ClaimPhase::Scouting => {
                self.try_score_candidates(system_data, &features.claim);
                self.refresh_visibility_requests(system_data);

                // Plan prefetch (stall report §4, M3): request plans for the top
                // viable plan-less candidates WHILE scouting runs, so the async
                // planner has them ready by commit time.
                self.prefetch_candidate_plans(system_data);

                // Rolling commit (stall report §4, M2-far): a far (>= ring)
                // candidate commits the tick it passes EVERY gate — fresh+safe
                // intel, valid plan, reachable affordable home, capacity —
                // instead of at one sampled instant at window end. Below-ring
                // candidates keep their coverage-gated patience in Select
                // (ADR 0038 D9); escorts also stay Select-side.
                self.try_commit_candidates(
                    system_data,
                    runtime_data,
                    maximum_rooms,
                    currently_owned_rooms,
                    &features.claim,
                    false,
                    true,
                    false,
                );

                let elapsed = self.phase_tick.map(|t| game::time().saturating_sub(t)).unwrap_or(0);

                // Select as soon as the reachable ring is covered (every
                // candidate scored, every unknown resolved or given up), or when
                // the scouting window caps out — whichever comes first.
                let covered = self.scouting_coverage_complete(system_data);

                if covered || elapsed >= self.scouting_window_eff(&features.claim) {
                    if covered {
                        info!("ClaimOp [Scouting]: reachable ring covered after {} ticks, selecting", elapsed);
                    }
                    self.phase = ClaimPhase::Select;
                    // Select-entry tick: run_select's CPU-hold retry window is
                    // measured from here (Idle's re-discover interval is re-set
                    // when Select completes).
                    self.phase_tick = Some(game::time());
                }
            }
            ClaimPhase::Select => {
                self.run_select(system_data, runtime_data, maximum_rooms, currently_owned_rooms, &features.claim);
            }
        }

        Ok(OperationResult::Running)
    }
}

/// Commit-time plan-gate decision (ADR 0038 D7 / REC-025). Input: whether
/// plan data exists for the candidate and, if so, whether the plan is VALID
/// (`RoomPlanData::valid()` — `Failed` plans carry data but no plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanCommitGate {
    /// No plan data at all — request one and defer the candidate this cycle.
    RequestPlan,
    /// Plan data present but the plan FAILED: presence is not validity — a
    /// hard, loud skip (claim_economics' "no valid plan ⇒ no claim"
    /// contract). Claiming anyway wedges a GCL slot on an unbuildable room.
    SkipInvalid,
    /// A valid plan exists — the candidate may be committed.
    Proceed,
}

fn plan_commit_gate(plan_valid: Option<bool>) -> PlanCommitGate {
    match plan_valid {
        None => PlanCommitGate::RequestPlan,
        Some(false) => PlanCommitGate::SkipInvalid,
        Some(true) => PlanCommitGate::Proceed,
    }
}

/// Claim-corridor reach (REC-024) — the SOLE live claim reach gate (ADR 0038;
/// the vestigial `missions::utility::is_claim_feasible` was deleted per REC-071).
/// The CLAIM lifetime arithmetic — travel plus the 50-tick arrival margin must
/// fit the 600-tick CLAIM lifetime — is expressed over the route directly. With
/// `travel_ticks = hops × 50` the bound is exactly
/// `hops ≤ max_claim_radius_hops()` (= 11, pinned in `missions::utility`), which
/// avoids duplicating the private lifetime constants. The margin is thin against
/// the hop model's terrain blindness (a `[CLAIM, MOVE]` claimer pays ~5 ticks per
/// swamp tile); the compensating conservatism is in the route PRICING (unscouted
/// rooms dispreferred — see `routepricing::UNSCOUTED_ROUTE_COST`), not here.
fn claim_route_feasible(route: crate::pathing::pathfinderservice::CachedRoute) -> bool {
    route.reachable && route.hops <= crate::missions::utility::max_claim_radius_hops()
}

/// The cheap threat/intel signals for an Escort screen decision on one claim candidate. All primitives
/// (EP-6.2 pure-by-design) so the decision kernel stays `game::*`-free and pin-testable. Sourced from the
/// candidate's dynamic-visibility read + optional `RoomThreatData` — the SAME intel the pre-claim safety
/// gate consumes; the escort producer invents no new scouting.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EscortScreenInputs {
    /// Fresh clean intel required (ADR 0017: absence of fresh intel is NOT safety) — a stale read is no
    /// basis to dispatch a screen. `false` ⇒ never emit (re-scout instead).
    intel_fresh: bool,
    /// The room's owner is a foreign player. A hostile-OWNED room is not a marginal claim target at all
    /// (claimController is impossible there); it belongs to the war/harass lane, never the claim escort.
    owner_hostile: bool,
    /// The room's owner is an ally. Not claimable — no escort.
    owner_friendly: bool,
    /// A foreign player holds/contests the reservation (`claimController` ⇒ ERR_INVALID_TARGET). Not a
    /// marginal claim target.
    reservation_blocked: bool,
    /// A hostile tower can hit the room edge. A towered room is a full assault, not a light screen — a
    /// claim escort must never be sized to beat towers (that would widen aggression). NO-OP.
    tower_present: bool,
    /// A nuke is inbound. Never screen a claimer into it. NO-OP.
    nukes_incoming: bool,
    /// The canonical threat kind is at least `PlayerSiege` — a sustained heavy force, not marginal. NO-OP.
    siege: bool,
    /// The room has SOME threat worth attention (`ThreatLevel != None`). When `false` the claim is CLEAN
    /// and the escort is a strict NO-OP (don't spawn escorts for clean claims).
    warrants_attention: bool,
    /// The room is rejected by the THREAT arm of the pre-claim safety gate specifically — i.e.
    /// `threat_level >= PlayerRaid` OR `estimated_attack_dps > 0.0` (the exact predicate
    /// `is_claim_target_safe` rejects on). When `false` the safety gate treats the room as CLAIMABLE
    /// (e.g. a lone enemy `PlayerScout` with 0 attack DPS: it `warrants_attention` yet is claimed
    /// normally), so a screen here would be redundant AND widen aggression toward a room outside the
    /// contested set → NO-OP. This is the signal that makes the escort's firing set exactly "the room the
    /// claim pipeline is pursuing but the safety gate's threat arm rejected", not the broader
    /// `warrants_attention` set (which includes harmless 0-DPS presences).
    threat_present: bool,
    /// Summed hostile combat DPS (melee + ranged). Must be within [`ESCORT_MAX_SCREEN_DPS`] — a genuinely
    /// light presence a small screen can clear. Above the ceiling the room is a real contest → NO-OP.
    attack_dps: f32,
}

/// Producer-level viability guards for the Escort producer (the two filters `emit_escort_objectives`
/// applies BEFORE the threat/screen kernel), extracted pure (EP-6.2) so the producer's full filtering —
/// not just the screen kernel — is pin-testable without game fixtures. Returns `true` only when the room
/// is one the claim pipeline is ACTUALLY pursuing: NOT expansion-avoided AND its plan is not commit-invalid
/// (`PlanCommitGate::SkipInvalid`). Mirrors the SAME viability signals `try_score_candidates` checks so the
/// two sites cannot drift; a `false` here means a screen would be fielded into a room outside the pursued
/// set (wasted energy/CPU/squad-slot + aggression widened into non-claim rooms). Note `RequestPlan` (no
/// plan yet) is NOT a rejection: only `SkipInvalid` (a FAILED plan — the room can never be built/claimed)
/// blocks the escort, matching the commit gate's "presence is not validity" contract.
fn escort_candidate_viable(avoided: bool, plan_gate: PlanCommitGate) -> bool {
    !avoided && plan_gate != PlanCommitGate::SkipInvalid
}

/// A pure decision: should a claim candidate get an `Escort{room}` pre-clear screen (ADR 0017 expansion
/// pre-clear; combat-overhaul-plan.md §W3)? Returns `true` only for a MARGINAL threatened claim candidate:
/// one the bot would otherwise pursue but whose target carries a detected-but-modest creep presence.
/// Returns `false` (NO-OP) for a clean claim (no escorts on clean rooms), for a room that is unclaimable
/// for a NON-threat reason (hostile/friendly owner, blocked reservation — the war lane's business, not the
/// claim escort's), and for a threat too heavy to be a light defensive screen (towers, nukes, siege, or DPS
/// over the screen ceiling — never widen aggression). Deliberately conservative: the escort is a
/// Secure-adjacent screen, never a full assault. The caller sizes the screen off `inputs.attack_dps` — a
/// `true` verdict certifies that DPS is within [`ESCORT_MAX_SCREEN_DPS`], i.e. a light presence a small
/// screen can clear; the kernel screens (yes/no), it does not transform the force.
fn escort_screen_decision(inputs: EscortScreenInputs, max_screen_dps: f32) -> bool {
    // Stale intel is not a basis to commit a screen (ADR 0017) — re-scout instead.
    if !inputs.intel_fresh {
        return false;
    }
    // Not a marginal CLAIM target: a hostile/friendly owner or a contested reservation makes claiming
    // impossible regardless of threat. Those rooms are the war/harass lane's, not the claim escort's —
    // emitting here would widen aggression into rooms we cannot claim.
    if inputs.owner_hostile || inputs.owner_friendly || inputs.reservation_blocked {
        return false;
    }
    // A clean claim: strict NO-OP (no escorts on clean rooms).
    if !inputs.warrants_attention {
        return false;
    }
    // The safety gate's THREAT arm did not reject this room — it is claimable as-is (e.g. a lone 0-DPS
    // `PlayerScout`, which `warrants_attention` but `is_claim_target_safe` treats as CLEAN). A screen here
    // would be redundant and widen aggression toward a room outside the contested set → NO-OP.
    if !inputs.threat_present {
        return false;
    }
    // Too heavy to be a light defensive screen — a full assault the claim pipeline must never provoke.
    if inputs.tower_present || inputs.nukes_incoming || inputs.siege {
        return false;
    }
    // Only a genuinely MODEST creep presence gets a screen; above the ceiling it is a real contest.
    if inputs.attack_dps > max_screen_dps {
        return false;
    }
    true
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

    // ── Commit-time plan gate (REC-025) ─────────────────────────────────────

    /// Pin (REC-025): plan PRESENCE is not plan VALIDITY. The old commit gate
    /// checked `room_plan_data.is_none()` only, so a plan that FAILED during
    /// the scouting window passed and the room could be claimed despite being
    /// unbuildable — an irreversible GCL commit (`should_abandon_claim` never
    /// fires without hostiles). A failed plan must be a hard skip, distinct
    /// from the missing-plan defer that requests planning.
    ///
    /// This one kernel drives BOTH halves of the fix: `SkipInvalid` is the
    /// commit-time hard skip AND the score-time negative marking in
    /// `try_score_candidates` (a failed plan scores as UNCLAIMABLE rather
    /// than letting `plan_score` → None map to `plan_quality`'s neutral 1.0,
    /// which ranked an unbuildable room above planned-but-mediocre ones).
    /// `RequestPlan` (missing ≠ failed) must stay neutral-scoreable so a
    /// not-yet-planned room keeps being pursued.
    #[test]
    fn plan_commit_gate_distinguishes_missing_from_failed() {
        assert_eq!(plan_commit_gate(None), PlanCommitGate::RequestPlan);
        assert_eq!(plan_commit_gate(Some(false)), PlanCommitGate::SkipInvalid);
        assert_eq!(plan_commit_gate(Some(true)), PlanCommitGate::Proceed);
    }

    // ── Claim-corridor reach (REC-024) ──────────────────────────────────────

    /// Pin (REC-024): the commit-time reach check over a priced route is the SOLE
    /// claim reach gate (REC-071 deleted the vestigial `is_claim_feasible`). Its
    /// lifetime arithmetic — travel (hops × 50) + 50-tick arrival margin within the
    /// 600-tick CLAIM lifetime ⇔ hops ≤ max_claim_radius_hops() = 11 — must hold, and
    /// an unreachable route (every corridor denied by the mover-aligned pricing) must
    /// be infeasible, never defaulted.
    #[test]
    fn claim_route_feasibility_matches_the_claim_lifetime_bound() {
        let route = |hops: u32, reachable: bool| crate::pathing::pathfinderservice::CachedRoute {
            hops,
            travel_ticks: hops.saturating_mul(50),
            cached_at: 0,
            reachable,
        };
        // Boundary: 11 hops (550 + 50 = 600) is the last feasible distance.
        assert!(claim_route_feasible(route(11, true)));
        assert!(!claim_route_feasible(route(12, true)));
        // Same room / short hops are trivially feasible.
        assert!(claim_route_feasible(route(0, true)));
        assert!(claim_route_feasible(route(1, true)));
        // A denied corridor means NOT claimable from that home, regardless of
        // the nominal hop count.
        assert!(!claim_route_feasible(route(2, false)));
        assert!(!claim_route_feasible(route(u32::MAX, false)));
    }

    // ── Escort pre-clear producer (ADR 0017; combat-overhaul-plan.md §W3) ────

    /// A CLEAN, viable claim candidate (no threat) that a MARGINAL-threatened one shares every field
    /// with except the threat signals. Fresh intel, neutral owner/reservation, no towers/nukes/siege.
    fn clean_inputs() -> EscortScreenInputs {
        EscortScreenInputs {
            intel_fresh: true,
            owner_hostile: false,
            owner_friendly: false,
            reservation_blocked: false,
            tower_present: false,
            nukes_incoming: false,
            siege: false,
            warrants_attention: false,
            threat_present: false,
            attack_dps: 0.0,
        }
    }

    /// A marginal threatened candidate: same clean room, but a modest creep presence (within the screen
    /// ceiling) worth attention AND rejected by the safety gate's threat arm (`threat_present`).
    fn marginal_inputs() -> EscortScreenInputs {
        EscortScreenInputs {
            warrants_attention: true,
            threat_present: true,
            attack_dps: 30.0,
            ..clean_inputs()
        }
    }

    /// Pin (§W3): the escort is EMITTED for a marginal threatened claim target — a viable room the bot is
    /// pursuing whose target carries a detected-but-modest creep threat within the light-screen ceiling.
    #[test]
    fn escort_emitted_for_marginal_threatened_claim() {
        assert!(
            escort_screen_decision(marginal_inputs(), ESCORT_MAX_SCREEN_DPS),
            "a modest creep threat on a viable claim target gets a screen"
        );
    }

    /// Pin (§W3): the escort is a strict NO-OP for a CLEAN claim — no threat, no screen (don't spawn
    /// escorts for clean claims). This is the "must not fire on clean rooms" half of the requirement.
    #[test]
    fn escort_not_emitted_for_clean_claim() {
        assert!(
            !escort_screen_decision(clean_inputs(), ESCORT_MAX_SCREEN_DPS),
            "a clean claim target never produces an escort"
        );
    }

    /// Pin (no aggression widening): a threat too HEAVY to be a light defensive screen is a NO-OP — a
    /// towered room, an inbound nuke, a siege-level force, or DPS over the ceiling. The claim pipeline
    /// must never provoke a full assault; those rooms stay the war/harass lane's business.
    #[test]
    fn escort_not_emitted_for_heavy_threat_no_aggression_widening() {
        // Over the light-screen DPS ceiling → not marginal.
        let over_dps = EscortScreenInputs {
            attack_dps: ESCORT_MAX_SCREEN_DPS + 1.0,
            ..marginal_inputs()
        };
        assert!(!escort_screen_decision(over_dps, ESCORT_MAX_SCREEN_DPS), "DPS over the ceiling is a real contest");

        // A towered room is a full assault, not a screen.
        assert!(
            !escort_screen_decision(
                EscortScreenInputs {
                    tower_present: true,
                    ..marginal_inputs()
                },
                ESCORT_MAX_SCREEN_DPS
            ),
            "a towered room is never a claim escort"
        );

        // An inbound nuke.
        assert!(
            !escort_screen_decision(
                EscortScreenInputs {
                    nukes_incoming: true,
                    ..marginal_inputs()
                },
                ESCORT_MAX_SCREEN_DPS
            ),
            "never screen a claimer into an inbound nuke"
        );

        // A siege-level force.
        assert!(
            !escort_screen_decision(
                EscortScreenInputs {
                    siege: true,
                    ..marginal_inputs()
                },
                ESCORT_MAX_SCREEN_DPS
            ),
            "a siege is not a marginal claim target"
        );
    }

    /// Pin (no aggression widening): a room unclaimable for a NON-threat reason — a hostile or friendly
    /// owner, or a blocked reservation — is a NO-OP even with a modest threat. Those belong to the
    /// war/harass lane; the claim escort must not reach into rooms it could never claim anyway.
    #[test]
    fn escort_not_emitted_for_unclaimable_owner_or_reservation() {
        for bad in [
            EscortScreenInputs {
                owner_hostile: true,
                ..marginal_inputs()
            },
            EscortScreenInputs {
                owner_friendly: true,
                ..marginal_inputs()
            },
            EscortScreenInputs {
                reservation_blocked: true,
                ..marginal_inputs()
            },
        ] {
            assert!(
                !escort_screen_decision(bad, ESCORT_MAX_SCREEN_DPS),
                "a non-claimable room is never a claim escort"
            );
        }
    }

    /// Pin (ADR 0017: absence of fresh intel is NOT safety): a marginal threat on STALE intel is a NO-OP
    /// — re-scout rather than dispatch a screen on an old read.
    #[test]
    fn escort_not_emitted_on_stale_intel() {
        assert!(
            !escort_screen_decision(
                EscortScreenInputs {
                    intel_fresh: false,
                    ..marginal_inputs()
                },
                ESCORT_MAX_SCREEN_DPS
            ),
            "stale intel is no basis to commit a screen"
        );
    }

    /// Pin (ESCORT-W3 Finding 2 — no aggression widening + no redundant escort): a room that
    /// `warrants_attention` but is NOT rejected by the safety gate's threat arm (a lone enemy
    /// `PlayerScout`: `estimated_attack_dps == 0`, `threat_level < PlayerRaid`, so
    /// `is_claim_target_safe` treats it as CLEAN and claims it normally) is a strict NO-OP — no
    /// redundant screen, no combat squad fielded toward a room outside the contested set.
    #[test]
    fn escort_not_emitted_for_harmless_lone_scout() {
        let lone_scout = EscortScreenInputs {
            warrants_attention: true,
            threat_present: false,
            attack_dps: 0.0,
            ..clean_inputs()
        };
        assert!(
            !escort_screen_decision(lone_scout, ESCORT_MAX_SCREEN_DPS),
            "a lone 0-DPS scout the safety gate treats as claimable never gets an escort"
        );
    }

    /// Pin (boundary): the light-screen DPS ceiling is inclusive — a threat exactly AT the ceiling is
    /// still a marginal screen; one tick over is a contest.
    #[test]
    fn escort_dps_ceiling_is_inclusive() {
        assert!(
            escort_screen_decision(
                EscortScreenInputs {
                    attack_dps: ESCORT_MAX_SCREEN_DPS,
                    ..marginal_inputs()
                },
                ESCORT_MAX_SCREEN_DPS
            ),
            "DPS exactly at the ceiling is still a light screen"
        );
    }

    // ── Producer-level filtering (emit_escort_objectives, kernel-composed) ────
    //
    // `emit_escort_objectives` needs a live specs `World` + JS-bound `RoomData` dynamic visibility to run
    // end-to-end (heavy fixtures — EP-6.2). Its filtering beyond the screen kernel is entirely the two pure
    // viability guards; these tests compose them EXACTLY as the producer does (viability guard →
    // `escort_screen_decision`) so the producer's filter set is pinned directly.

    /// Model of one candidate the producer walks: the viability inputs (avoidance + plan gate) plus the
    /// screen inputs. Mirrors the producer's per-candidate signal gather.
    struct ProducerCandidate {
        avoided: bool,
        plan_gate: PlanCommitGate,
        screen: EscortScreenInputs,
    }

    /// Whether the producer would emit an Escort for this candidate — the EXACT filtering chain of
    /// `emit_escort_objectives` (the pure viability guard, then the screen kernel).
    fn producer_emits(c: &ProducerCandidate) -> bool {
        escort_candidate_viable(c.avoided, c.plan_gate) && escort_screen_decision(c.screen, ESCORT_MAX_SCREEN_DPS)
    }

    /// Pin (§W3 producer filtering): across a mixed candidate set, the producer emits EXACTLY one Escort —
    /// only for the genuine threat-rejected VIABLE candidate. A plan-invalid candidate with a modest
    /// threat, an avoidance-cooldown candidate with a modest threat, and a clean positively-scored viable
    /// candidate all yield NO escort. This is the producer-level counterpart to the kernel pins (which the
    /// review noted were the only coverage).
    #[test]
    fn producer_emits_only_for_viable_threat_rejected_candidate() {
        let candidates = [
            // (1) Plan-invalid (FAILED plan) + a modest threat → NO escort: unbuildable, never pursued.
            ProducerCandidate {
                avoided: false,
                plan_gate: PlanCommitGate::SkipInvalid,
                screen: marginal_inputs(),
            },
            // (2) Avoidance-cooldown + a modest threat → NO escort: deliberately abandoned, never pursued.
            ProducerCandidate {
                avoided: true,
                plan_gate: PlanCommitGate::Proceed,
                screen: marginal_inputs(),
            },
            // (3) Clean, positively-scored viable candidate → NO escort: no threat, no screen.
            ProducerCandidate {
                avoided: false,
                plan_gate: PlanCommitGate::Proceed,
                screen: clean_inputs(),
            },
            // (4) Genuine threat-rejected VIABLE candidate → the SOLE escort.
            ProducerCandidate {
                avoided: false,
                plan_gate: PlanCommitGate::Proceed,
                screen: marginal_inputs(),
            },
        ];

        assert!(!producer_emits(&candidates[0]), "a plan-invalid candidate is never escorted");
        assert!(!producer_emits(&candidates[1]), "an avoidance-cooldown candidate is never escorted");
        assert!(!producer_emits(&candidates[2]), "a clean viable candidate is never escorted");
        assert!(producer_emits(&candidates[3]), "the viable threat-rejected candidate is escorted");

        let emitted = candidates.iter().filter(|c| producer_emits(c)).count();
        assert_eq!(emitted, 1, "exactly one Escort across the mixed candidate set");
    }

    /// Pin: the viability guard is orthogonal to the threat kernel — a marginal threat that WOULD screen is
    /// still suppressed by EITHER producer-level guard (avoidance OR plan-invalid). `RequestPlan` (no plan
    /// yet) is NOT a rejection (only a FAILED plan blocks), matching the commit gate's presence-≠-validity
    /// contract, so a viable-but-unplanned marginal candidate still passes the viability guard.
    #[test]
    fn producer_viability_guard_suppresses_independently_of_threat() {
        assert!(escort_candidate_viable(false, PlanCommitGate::Proceed), "viable: not avoided, plan proceeds");
        assert!(
            escort_candidate_viable(false, PlanCommitGate::RequestPlan),
            "no plan yet is not a rejection — only a FAILED plan blocks the escort"
        );
        assert!(!escort_candidate_viable(true, PlanCommitGate::Proceed), "avoided rooms are never viable");
        assert!(!escort_candidate_viable(false, PlanCommitGate::SkipInvalid), "FAILED plans are never viable");
        assert!(!escort_candidate_viable(true, PlanCommitGate::SkipInvalid), "both guards failing is not viable");
    }
}

/// Offline decoder for a LIVE serialized world payload (component segments 50–53 fetched read-only via
/// the REST API and concatenated). Diagnostic tooling: introspects the live ClaimOperation /
/// mission / visibility state on the host without touching the server. Ignored by default — run with
///   IBEX_WORLD_PAYLOAD=<path-to-concatenated-base64> [IBEX_NOW=<game tick>] \
///     cargo test -p screeps-ibex decode_live_world -- --ignored --nocapture
/// Lives in this module (not game_loop) so it can read ClaimOperation's private phase/candidate fields.
#[cfg(test)]
mod live_world_decode {
    use super::*;
    use crate::creep::*;
    use crate::jobs::data::JobData;
    use crate::military::objective_queue::CombatObjectiveData;
    use crate::military::squad::SquadContext;
    use crate::military::threatmap::RoomThreatData;
    use crate::missions::data::MissionData;
    use crate::pathing::movementsystem::CreepRoverData;
    use crate::room::data::RoomData;
    use crate::room::roomplansystem::RoomPlanData;
    use crate::room::visibilitysystem::VisibilityQueueData;
    use crate::serialize::{decode_buffer_from_string, SerializeMarker, SerializeMarkerAllocator};
    use bincode::DefaultOptions;
    use specs::prelude::*;
    use specs::saveload::DeserializeComponents;

    /// Mirrors game_loop::WORLD_FORMAT_VERSION (private there). The assert below fails loudly on drift.
    const EXPECTED_WORLD_FORMAT_VERSION: u32 = 28;

    struct DecodeAndDump<'p> {
        payload: &'p [u8],
        now: Option<u32>,
    }

    #[derive(SystemData)]
    struct DecodeSystemData<'a> {
        entities: Entities<'a>,
        marker_alloc: Write<'a, SerializeMarkerAllocator>,
        markers: WriteStorage<'a, SerializeMarker>,
        creep_spawnings: WriteStorage<'a, CreepSpawning>,
        creep_owners: WriteStorage<'a, CreepOwner>,
        creep_movement_data: WriteStorage<'a, CreepRoverData>,
        room_data: WriteStorage<'a, RoomData>,
        room_plan_data: WriteStorage<'a, RoomPlanData>,
        job_data: WriteStorage<'a, JobData>,
        operation_data: WriteStorage<'a, OperationData>,
        mission_data: WriteStorage<'a, MissionData>,
        squad_context: WriteStorage<'a, SquadContext>,
        visibility_queue_data: WriteStorage<'a, VisibilityQueueData>,
        combat_objective_data: WriteStorage<'a, CombatObjectiveData>,
        room_threat_data: WriteStorage<'a, RoomThreatData>,
    }

    impl<'a, 'p> System<'a> for DecodeAndDump<'p> {
        type SystemData = DecodeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            let mut deserializer = bincode::Deserializer::from_slice(self.payload, DefaultOptions::new());
            DeserializeComponents::<std::convert::Infallible, SerializeMarker>::deserialize(
                &mut (
                    &mut data.creep_spawnings,
                    &mut data.creep_owners,
                    &mut data.creep_movement_data,
                    &mut data.room_data,
                    &mut data.room_plan_data,
                    &mut data.job_data,
                    &mut data.operation_data,
                    &mut data.mission_data,
                    &mut data.squad_context,
                    &mut data.visibility_queue_data,
                    &mut data.combat_objective_data,
                    &mut data.room_threat_data,
                ),
                &data.entities,
                &mut data.markers,
                &mut data.marker_alloc,
                &mut deserializer,
            )
            .map(|_| ())
            .expect("component stream deserialize failed");

            let now = self.now;
            let age = |tick: Option<u32>| -> String {
                match (now, tick) {
                    (Some(n), Some(t)) => format!("{} (age {})", t, n.saturating_sub(t)),
                    (_, Some(t)) => format!("{}", t),
                    _ => "?".to_owned(),
                }
            };

            println!("=== entity counts ===");
            println!("rooms: {}", (&data.room_data).join().count());
            println!("room plans: {}", (&data.room_plan_data).join().count());
            println!("operations: {}", (&data.operation_data).join().count());
            println!("missions: {}", (&data.mission_data).join().count());
            println!("jobs: {}", (&data.job_data).join().count());

            // Room-name → entity map for candidate lookups.
            let mut rooms_by_name = std::collections::HashMap::new();
            for (entity, rd) in (&data.entities, &data.room_data).join() {
                rooms_by_name.insert(rd.name, entity);
            }

            println!("\n=== ClaimOperation ===");
            for op in (&data.operation_data).join() {
                let OperationData::Claim(claim) = op else { continue };
                println!("phase: {:?}, phase_tick: {}", claim.phase, age(claim.phase_tick));
                println!("home_rooms: {:?}", claim.home_rooms.iter().map(|r| r.to_string()).collect::<Vec<_>>());
                println!("claim_missions: {}", claim.claim_missions.len());
                println!("unknown_rooms ({}):", claim.unknown_rooms.len());
                for r in &claim.unknown_rooms {
                    println!("  {}", r);
                }
                println!("candidates ({}):", claim.candidates.len());
                for c in &claim.candidates {
                    let plan_state = rooms_by_name
                        .get(&c.room_name)
                        .map(|e| match data.room_plan_data.get(*e).map(|p| p.valid()) {
                            None => "no-plan-data".to_owned(),
                            Some(true) => "plan-VALID".to_owned(),
                            Some(false) => "plan-FAILED".to_owned(),
                        })
                        .unwrap_or_else(|| "no-room-entity".to_owned());
                    let threat = rooms_by_name
                        .get(&c.room_name)
                        .and_then(|e| data.room_threat_data.get(*e))
                        .map(|t| {
                            format!(
                                "threat={:?} dps={:.0} hostiles={}",
                                t.threat_level,
                                t.estimated_attack_dps,
                                t.hostile_creeps.len()
                            )
                        })
                        .unwrap_or_else(|| "no-threat-data".to_owned());
                    println!(
                        "  {} dist={} homes={:?} score={:?} [{}] [{}]",
                        c.room_name,
                        c.distance,
                        c.home_rooms.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                        c.score,
                        plan_state,
                        threat,
                    );
                }
            }

            println!("\n=== claim/colony/remote-build missions ===");
            for (entity, mission) in (&data.entities, &data.mission_data).join() {
                let room_name = mission
                    .as_mission()
                    .get_room()
                    .and_then(|e| data.room_data.get(e))
                    .map(|rd| rd.name.to_string())
                    .unwrap_or_else(|| "?".to_owned());
                if let Some(cm) = mission.as_mission_type::<ClaimMission>() {
                    let homes: Vec<String> = cm
                        .home_room_datas()
                        .iter()
                        .filter_map(|e| data.room_data.get(*e).map(|rd| rd.name.to_string()))
                        .collect();
                    println!("  {:?} ClaimMission -> {} (homes {:?})", entity, room_name, homes);
                } else if mission.as_mission_type::<RemoteBuildMission>().is_some() {
                    println!("  {:?} RemoteBuildMission -> {}", entity, room_name);
                }
            }

            println!("\n=== visibility queue ===");
            for vq in (&data.visibility_queue_data).join() {
                println!("entries ({}):", vq.entries.len());
                for e in &vq.entries {
                    println!(
                        "  {} prio={} expires_at={} opportunistic={}",
                        e.room_name,
                        e.priority,
                        age(Some(e.expires_at)),
                        e.opportunistic
                    );
                }
                println!("unreachable ({}):", vq.unreachable.len());
                for u in &vq.unreachable {
                    println!("  {} retry_after={} attempts={}", u.room_name, age(Some(u.retry_after)), u.attempts);
                }
            }
        }
    }

    #[test]
    #[ignore]
    fn decode_live_world() {
        let Ok(path) = std::env::var("IBEX_WORLD_PAYLOAD") else {
            eprintln!("IBEX_WORLD_PAYLOAD not set; skipping");
            return;
        };
        let now: Option<u32> = std::env::var("IBEX_NOW").ok().and_then(|v| v.parse().ok());
        let encoded = std::fs::read_to_string(&path).expect("read payload file");
        let decoded = decode_buffer_from_string(encoded.trim()).expect("base64+gzip decode");
        assert!(decoded.len() >= 4, "payload too short");
        let version = u32::from_le_bytes(decoded[..4].try_into().unwrap());
        assert_eq!(
            version, EXPECTED_WORLD_FORMAT_VERSION,
            "payload world-format version mismatch (bincode would misalign)"
        );

        let mut world = World::new();
        world.register::<SerializeMarker>();
        world.register::<CreepSpawning>();
        world.register::<CreepOwner>();
        world.register::<CreepRoverData>();
        world.register::<RoomData>();
        world.register::<RoomPlanData>();
        world.register::<JobData>();
        world.register::<OperationData>();
        world.register::<MissionData>();
        world.register::<SquadContext>();
        world.register::<VisibilityQueueData>();
        world.register::<CombatObjectiveData>();
        world.register::<RoomThreatData>();
        world.insert(SerializeMarkerAllocator::new());

        let mut sys = DecodeAndDump {
            payload: &decoded[4..],
            now,
        };
        sys.run_now(&world);
    }
}
