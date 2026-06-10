//! Two-stage spawn-site scoring (P0.P3; stage 1 shared since P0.P7).
//!
//! STAGE 1 — cheap heuristics over the cache: source count (2 strongly
//! preferred), controller presence, mineral presence/type, swamp
//! fraction, wall fraction, exit count/distribution. Each subscore is in
//! `[0, 1]`; the total is a weighted average ([`HeuristicWeights`],
//! operator-configurable). THE SCORING ITSELF LIVES IN
//! [`screeps_foreman::room_scoring`] (P0.P7 — one room-candidate
//! scoring implementation shared with live Ibex claim/expansion and
//! bench calibration); this module is prospector's POLICY SHELL: cache
//! fetch-listing, CLI weight mapping onto the shared
//! [`WeightedPipeline`], disqualification reporting, and stage 2.
//! Rooms missing cached data are FETCH-LISTED ([`NeedsFetch`]) rather
//! than silently skipped — the caller can print the exact `fetch`
//! command to fill the gap.
//!
//! STAGE 2 — offline foreman planning for the top-N finalists: a
//! [`PlannerRoomDataSource`] over the cached terrain+objects (the
//! bench's `RoomDataPlannerDataSource` pattern,
//! `screeps-foreman-bench/src/main.rs:83-106`) drives the full planning
//! pipeline with a generous wall-clock [`CpuBudget`]. The plan's
//! [`PlanScore`] ranks the finalists and the plan's FIRST spawn — the
//! tile the layout itself wants a spawn on — becomes the recommendation's
//! placement position, so the spawn placed at world-bootstrap is already
//! part of the optimal base.
//!
//! FIRST-SPAWN PIN (how the plan exposes spawn positions): planned
//! structures live in `Plan.structures` and, priority-ordered, in
//! `Plan.build_order` (`screeps-foreman/src/plan.rs:260-278`). The build
//! order sorts by priority desc, then `required_rcl` asc, then
//! hub-distance (`screeps-foreman/src/pipeline/finalize.rs:126-135`);
//! spawns are `BuildPriority::Critical` (`planner.rs:159`) and exactly
//! one spawn is placed at RCL 1 — the hub stamp's `sp(Spawn, -1, 0, 1)`
//! (`screeps-foreman/src/stamps/hub.rs:30`; its mirror spawn is RCL 7,
//! and `SpawnLayer` adds the third at RCL 7/8, `layers/spawn.rs:119`).
//! [`first_spawn_tile`] therefore takes the minimum-RCL spawn from the
//! build order (`min_by_key` keeps the first on ties, i.e. build-order
//! priority), falling back to the structures map.
//!
//! Everything here is pure/offline — no network, no Docker. Determinism:
//! given the same cache, stage 1 is exact arithmetic with name
//! tie-breaks, and the foreman search is deterministic (no RNG; FNV maps
//! iterate reproducibly) — pinned by the stage-2 test running a room
//! twice and comparing.

use crate::cache::{CachedRoom, MineralInfo, RoomCache};
use anyhow::{Context, Result};
use screeps_foreman::layer::PlacementLayer;
use screeps_foreman::layers::{
    default_layers, hub_stamp_layer, AnchorLayer, AnchorScoreLayer, ExitSetbackLayer,
    HubQualityScoreLayer,
};
use screeps_foreman::pipeline::CpuBudget;
use screeps_foreman::plan::{Plan, PlanScore};
use screeps_foreman::planner::{tick_planning, PlanResult, PlannerBuilder};
use screeps_foreman::room_data::{PlanLocation, PlannerRoomDataSource};
use screeps_foreman::room_scoring::{
    self, ControllerScorer, ExitScorer, MineralFact, MineralScorer, RoomFacts, RoomScorer,
    ScoringContext, SourceCountScorer, SwampScorer, WallsScorer, WeightedPipeline,
};
use screeps_foreman::terrain::FastRoomTerrain;
use screeps_foreman::StructureType;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Default number of stage-1 finalists carried into stage-2 planning.
pub const DEFAULT_FINALISTS: usize = 8;

/// Default wall-clock budget per finalist plan. Generous: host-side
/// planning of a bench room completes well under this; the budget only
/// exists so a pathological room cannot hang `recommend`.
pub const DEFAULT_PLAN_TIMEOUT_SECS: u64 = 120;

// ---------------------------------------------------------------------------
// Stage 1 — cheap heuristics
// ---------------------------------------------------------------------------

/// Operator-configurable weights for the stage-1 subscores. The total is
/// `sum(weight_i * subscore_i) / sum(weight_i)`, so it stays in `[0, 1]`
/// for any non-negative weights.
#[derive(Debug, Clone, PartialEq)]
pub struct HeuristicWeights {
    /// Source count (2 strongly preferred — the defaults make this the
    /// dominant term).
    pub sources: f32,
    /// Controller presence (rooms without one cannot be claimed and are
    /// additionally disqualified from stage 2).
    pub controller: f32,
    /// Mineral presence + type preference.
    pub mineral: f32,
    /// 1 - swamp fraction (movement/road cost).
    pub swamp: f32,
    /// 1 - wall fraction (buildable area; extreme cramping is caught by
    /// stage-2 plan failure, not here).
    pub walls: f32,
    /// Exit count + side distribution (defensibility: fewer exit tiles
    /// on fewer sides = cheaper perimeter).
    pub exits: f32,
}

impl HeuristicWeights {
    /// Map these weights onto the shared pipeline, scorers in the
    /// canonical (weighted-average accumulation) order: sources,
    /// controller, mineral, swamp, walls, exits.
    fn to_pipeline(&self) -> WeightedPipeline {
        WeightedPipeline::new()
            .with_weighted(SourceCountScorer, self.sources)
            .with_weighted(ControllerScorer, self.controller)
            .with_weighted(MineralScorer, self.mineral)
            .with_weighted(SwampScorer, self.swamp)
            .with_weighted(WallsScorer, self.walls)
            .with_weighted(ExitScorer, self.exits)
    }
}

impl Default for HeuristicWeights {
    /// One source of truth: the shared scorers' default weights
    /// (`screeps_foreman::room_scoring`).
    fn default() -> Self {
        HeuristicWeights {
            sources: SourceCountScorer.default_weight(),
            controller: ControllerScorer.default_weight(),
            mineral: MineralScorer.default_weight(),
            swamp: SwampScorer.default_weight(),
            walls: WallsScorer.default_weight(),
            exits: ExitScorer.default_weight(),
        }
    }
}

/// Terrain-derived facts for one room — the shared module's
/// [`room_scoring::TerrainStats`] under this crate's historical name.
pub type TerrainMetrics = room_scoring::TerrainStats;

/// Compute [`TerrainMetrics`] from decoded terrain (delegates to
/// [`room_scoring::terrain_stats`]).
pub fn terrain_metrics(terrain: &FastRoomTerrain) -> TerrainMetrics {
    room_scoring::terrain_stats(terrain)
}

/// Source-count subscore ([`room_scoring::source_count_curve`]): 2
/// strongly preferred (the standard claim target), 1 viable but slow,
/// 3+ unusual (SK-adjacent layouts), 0 useless.
pub fn source_count_score(count: usize) -> f32 {
    room_scoring::source_count_curve(count)
}

/// Bridge the cache's mineral record into the shared fact type.
fn mineral_fact(info: &MineralInfo) -> MineralFact {
    MineralFact {
        location: PlanLocation::new(info.x as i8, info.y as i8),
        mineral_type: info.mineral_type.clone(),
    }
}

/// Mineral subscore ([`room_scoring::mineral_curve`]): presence is most
/// of the value; the type bonus prefers X (catalyst — every tier-3
/// boost needs it), then H/O (the base feedstocks).
pub fn mineral_score(mineral: Option<&MineralInfo>) -> f32 {
    room_scoring::mineral_curve(mineral.map(mineral_fact).as_ref())
}

/// Exit subscore ([`room_scoring::exit_curve`]): fewer exit tiles on
/// fewer sides = a cheaper, more defensible perimeter. 0 exits would
/// mean a sealed (unreachable) room — scored 0 as almost certainly bad
/// data.
pub fn exit_score(exit_tiles: usize, exit_sides: usize) -> f32 {
    room_scoring::exit_curve(exit_tiles, exit_sides)
}

/// Stage-1 result for one room: total, every subscore, and the raw
/// facts the subscores came from (for table display).
#[derive(Debug, Clone, PartialEq)]
pub struct HeuristicScore {
    pub room: String,
    /// Weighted average of the subscores, in `[0, 1]`.
    pub total: f32,
    pub sources_score: f32,
    pub controller_score: f32,
    pub mineral_score: f32,
    pub swamp_score: f32,
    pub walls_score: f32,
    pub exits_score: f32,
    pub source_count: usize,
    pub has_controller: bool,
    pub mineral_type: Option<String>,
    pub metrics: TerrainMetrics,
    /// `Some(reason)` when the room scores but cannot proceed to stage 2
    /// (foreman planning requires sources and a controller — see
    /// `SourceInfraLayer`/`ControllerInfraLayer`, "fails on missing").
    pub disqualified: Option<String>,
}

/// A room that could not be scored because its cached data is
/// incomplete — the caller should `fetch` it, not ignore it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsFetch {
    pub room: String,
    pub reason: String,
}

/// Score one cached room, or report what data is missing.
///
/// Fetch-listing policy: terrain absent → fetch. Objects empty AND the
/// room was never API-fetched (`fetchedAt` absent — bench seeds carry
/// objects, so an empty array there is genuinely empty) → fetch. Objects
/// empty but fetched → the room really has no planner objects; it scores
/// (low) and is disqualified from stage 2 with a reason.
pub fn heuristic_for_room(
    room: &CachedRoom,
    weights: &HeuristicWeights,
) -> Result<HeuristicScore, NeedsFetch> {
    if !room.has_terrain() {
        return Err(NeedsFetch {
            room: room.room.clone(),
            reason: "terrain not cached".to_owned(),
        });
    }
    if room.objects.is_empty() && room.fetched_at.is_none() {
        return Err(NeedsFetch {
            room: room.room.clone(),
            reason: "objects never fetched".to_owned(),
        });
    }
    let terrain = room.to_fast_terrain().map_err(|err| NeedsFetch {
        room: room.room.clone(),
        reason: format!("cached terrain failed to decode ({err}); refetch"),
    })?;
    let metrics = terrain_metrics(&terrain);
    let summary = room.objects_summary();

    // Bridge the cache record into the shared fact DTO and run the
    // shared pipeline. Stage 1 is context-free here (the cache has no
    // selected/owned rooms); context scorers compose at the call sites
    // that have context.
    let to_plan = |&(x, y): &(i32, i32)| PlanLocation::new(x as i8, y as i8);
    let facts = RoomFacts {
        room: room.room.clone(),
        terrain: metrics,
        sources: summary.sources.iter().map(to_plan).collect(),
        controller: summary.controller.as_ref().map(to_plan),
        mineral: summary.mineral.as_ref().map(mineral_fact),
    };
    let scored = weights
        .to_pipeline()
        .score_room(&facts, &ScoringContext::default());
    let subscore = |name: &str| scored.subscore(name).unwrap_or(0.0);

    Ok(HeuristicScore {
        room: room.room.clone(),
        total: scored.total,
        sources_score: subscore("sources"),
        controller_score: subscore("controller"),
        mineral_score: subscore("mineral"),
        swamp_score: subscore("swamp"),
        walls_score: subscore("walls"),
        exits_score: subscore("exits"),
        source_count: summary.sources.len(),
        has_controller: summary.controller.is_some(),
        mineral_type: summary.mineral.and_then(|m| m.mineral_type),
        metrics,
        disqualified: scored.disqualified,
    })
}

/// Stage-1 output: scored rooms (best first, room-name tie-break for
/// determinism) plus the fetch list.
#[derive(Debug, Default)]
pub struct Stage1Result {
    pub ranked: Vec<HeuristicScore>,
    pub needs_fetch: Vec<NeedsFetch>,
}

/// Run stage 1 over `rooms` (names) against the cache.
pub fn stage1(cache: &RoomCache, rooms: &[String], weights: &HeuristicWeights) -> Stage1Result {
    let mut result = Stage1Result::default();
    for name in rooms {
        match cache.get(name) {
            None => result.needs_fetch.push(NeedsFetch {
                room: name.clone(),
                reason: "not in cache".to_owned(),
            }),
            Some(room) => match heuristic_for_room(room, weights) {
                Ok(score) => result.ranked.push(score),
                Err(needs) => result.needs_fetch.push(needs),
            },
        }
    }
    result.ranked.sort_by(|a, b| {
        b.total
            .partial_cmp(&a.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.room.cmp(&b.room))
    });
    result
}

/// How the room list for `score`/`recommend` was chosen (CLI display).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomSelection {
    /// `--rooms` was passed.
    Explicit,
    /// `--all`: every cached room.
    AllCached,
    /// Default: rooms the cache flags open for spawning (respawn-area
    /// rooms always included; novice-area rooms per the flag).
    OpenRooms { include_novice: bool },
    /// Default, but the cache carries no scan statuses at all (e.g. a
    /// pure bench seed) — fall back to every cached room.
    AllCachedNoStatuses,
}

impl std::fmt::Display for RoomSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoomSelection::Explicit => write!(f, "rooms passed via --rooms"),
            RoomSelection::AllCached => write!(f, "all cached rooms (--all)"),
            RoomSelection::OpenRooms {
                include_novice: false,
            } => write!(
                f,
                "rooms the cache flags open for spawning; novice-area rooms excluded \
                 (--include-novice to widen)"
            ),
            RoomSelection::OpenRooms {
                include_novice: true,
            } => write!(
                f,
                "rooms the cache flags open for spawning, novice-area rooms included"
            ),
            RoomSelection::AllCachedNoStatuses => write!(
                f,
                "all cached rooms (cache has no scan statuses — run `scan` to narrow to open rooms)"
            ),
        }
    }
}

/// Resolve the room list for scoring: explicit `--rooms` wins, then
/// `--all`, then the cache's open rooms (respawn-area always in,
/// novice-area only with `include_novice`) — falling back to all cached
/// rooms when no statuses exist (offline bench-seeded caches).
pub fn select_rooms(
    cache: &RoomCache,
    explicit: Option<Vec<String>>,
    all: bool,
    include_novice: bool,
) -> (Vec<String>, RoomSelection) {
    if let Some(rooms) = explicit {
        return (rooms, RoomSelection::Explicit);
    }
    let all_names = || {
        cache
            .rooms
            .iter()
            .map(|r| r.room.clone())
            .collect::<Vec<_>>()
    };
    if all {
        return (all_names(), RoomSelection::AllCached);
    }
    if cache.rooms.iter().any(|r| r.spawn_status.is_some()) {
        (
            cache
                .open_rooms(include_novice)
                .map(|r| r.room.clone())
                .collect(),
            RoomSelection::OpenRooms { include_novice },
        )
    } else {
        (all_names(), RoomSelection::AllCachedNoStatuses)
    }
}

// ---------------------------------------------------------------------------
// Stage 2 — offline foreman planning
// ---------------------------------------------------------------------------

/// [`PlannerRoomDataSource`] over one cached room — the bench's
/// `RoomDataPlannerDataSource` pattern (bench main.rs:83-106): decoded
/// `FastRoomTerrain` + object positions as `PlanLocation`s.
pub struct CachedRoomDataSource {
    terrain: FastRoomTerrain,
    controllers: Vec<PlanLocation>,
    sources: Vec<PlanLocation>,
    minerals: Vec<PlanLocation>,
}

impl CachedRoomDataSource {
    pub fn from_room(room: &CachedRoom) -> Result<Self> {
        let terrain = room
            .to_fast_terrain()
            .with_context(|| format!("decoding cached terrain for {}", room.room))?;
        let summary = room.objects_summary();
        let to_plan = |&(x, y): &(i32, i32)| PlanLocation::new(x as i8, y as i8);
        Ok(CachedRoomDataSource {
            terrain,
            controllers: summary.controller.iter().map(to_plan).collect(),
            sources: summary.sources.iter().map(to_plan).collect(),
            minerals: summary
                .mineral
                .iter()
                .map(|m| PlanLocation::new(m.x as i8, m.y as i8))
                .collect(),
        })
    }
}

impl PlannerRoomDataSource for CachedRoomDataSource {
    fn get_terrain(&self) -> &FastRoomTerrain {
        &self.terrain
    }

    fn get_controllers(&self) -> &[PlanLocation] {
        &self.controllers
    }

    fn get_sources(&self) -> &[PlanLocation] {
        &self.sources
    }

    fn get_minerals(&self) -> &[PlanLocation] {
        &self.minerals
    }
}

/// Which foreman layer stack to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanProfile {
    /// The full default 23-layer stack (`screeps_foreman::layers::
    /// default_layers`) — what the bot runs in-game; always used for
    /// real recommendations.
    Full,
    /// Anchor + hub stamp + cheap score layers only. MUCH faster; still
    /// places the RCL-1 spawn (the hub stamp carries it) and produces a
    /// comparable (if coarser) score. For tests and quick iteration.
    Reduced,
}

impl PlanProfile {
    pub fn layers(&self) -> Vec<Box<dyn PlacementLayer>> {
        match self {
            PlanProfile::Full => default_layers(),
            PlanProfile::Reduced => vec![
                Box::new(ExitSetbackLayer::new(1)),
                Box::new(AnchorLayer::default()),
                Box::new(hub_stamp_layer()),
                Box::new(AnchorScoreLayer),
                Box::new(HubQualityScoreLayer),
            ],
        }
    }
}

impl std::str::FromStr for PlanProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(PlanProfile::Full),
            "reduced" => Ok(PlanProfile::Reduced),
            other => Err(format!("unknown plan profile '{other}' (full|reduced)")),
        }
    }
}

/// Run the foreman planner to completion under a wall-clock budget.
/// `Err(reason)` covers both planner failure ("No valid plan found",
/// layer failures) and budget exhaustion — the caller turns either into
/// a room rejection with that reason.
pub fn run_planner(
    source: &dyn PlannerRoomDataSource,
    profile: PlanProfile,
    timeout: Duration,
) -> Result<Plan, String> {
    let deadline = Instant::now() + timeout;
    let budget = CpuBudget::new(move || Instant::now() < deadline);
    let mut builder = PlannerBuilder::new();
    for layer in profile.layers() {
        builder = builder.add_layer(layer);
    }
    let mut state = builder.build();
    loop {
        match tick_planning(state, source, &budget) {
            PlanResult::Complete(plan) => return Ok(plan),
            PlanResult::Failed(message) => return Err(message),
            PlanResult::Running(next) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "planning exceeded the {}s budget",
                        timeout.as_secs()
                    ));
                }
                state = next;
            }
        }
    }
}

/// Extract the plan's FIRST spawn — `(x, y, required_rcl)`. See the
/// module docs for the pin (build-order sorting + the hub stamp's RCL-1
/// spawn make this the tile the layout wants its initial spawn on).
pub fn first_spawn_tile(plan: &Plan) -> Option<(u8, u8, u8)> {
    if let Some(step) = plan
        .build_order
        .iter()
        .filter(|step| step.structure_type == StructureType::Spawn)
        .min_by_key(|step| step.required_rcl)
    {
        return Some((step.location.x(), step.location.y(), step.required_rcl));
    }
    // Fallback: straight off the structures map (covers hypothetical
    // plans with an empty build order). Deterministic tie-break by
    // (rcl, x, y).
    plan.structures
        .iter()
        .flat_map(|(location, items)| {
            items
                .iter()
                .filter(|item| item.structure_type == StructureType::Spawn)
                .map(move |item| (*location, item.required_rcl()))
        })
        .min_by_key(|(location, rcl)| (*rcl, location.x(), location.y()))
        .map(|(location, rcl)| (location.x(), location.y(), rcl))
}

/// One ranked recommendation out of stage 2.
#[derive(Debug, Clone)]
pub struct Recommendation {
    pub room: String,
    /// The stage-1 score that made this room a finalist.
    pub heuristic: HeuristicScore,
    /// The foreman plan's score (total + sub-scores).
    pub plan_score: PlanScore,
    /// Proposed spawn tile — the plan's first (RCL-1) spawn.
    pub spawn: (u8, u8),
    pub spawn_rcl: u8,
    /// Wall-clock planning time (diagnostics).
    pub plan_seconds: f32,
}

/// A room dropped from the pipeline, with the reason (stage-1
/// disqualification or stage-2 plan failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedRoom {
    pub room: String,
    pub reason: String,
}

/// Options for the full `recommend` pipeline.
#[derive(Debug, Clone)]
pub struct RecommendOptions {
    pub weights: HeuristicWeights,
    /// Stage-1 finalist count (top-N carried into planning).
    pub finalists: usize,
    pub profile: PlanProfile,
    /// Wall-clock budget per finalist plan.
    pub plan_timeout: Duration,
}

impl Default for RecommendOptions {
    fn default() -> Self {
        RecommendOptions {
            weights: HeuristicWeights::default(),
            finalists: DEFAULT_FINALISTS,
            profile: PlanProfile::Full,
            plan_timeout: Duration::from_secs(DEFAULT_PLAN_TIMEOUT_SECS),
        }
    }
}

/// Full pipeline output.
#[derive(Debug, Default)]
pub struct RecommendResult {
    /// The complete stage-1 table (finalists and non-finalists alike).
    pub stage1: Stage1Result,
    /// Stage-2 ranked recommendations, best plan score first.
    pub recommendations: Vec<Recommendation>,
    /// Rooms dropped, with reasons.
    pub rejected: Vec<RejectedRoom>,
}

/// The full two-stage pipeline: stage-1 ranking, top-N finalist
/// selection (disqualified rooms are rejected with their reason, never
/// planned), per-finalist foreman planning, and a final ranking by plan
/// score. Pure/offline — operates on the cache only.
pub fn recommend(
    cache: &RoomCache,
    rooms: &[String],
    options: &RecommendOptions,
) -> RecommendResult {
    let stage1_result = stage1(cache, rooms, &options.weights);
    let mut rejected = Vec::new();
    let mut finalists = Vec::new();
    for score in &stage1_result.ranked {
        if finalists.len() >= options.finalists {
            break;
        }
        match &score.disqualified {
            Some(reason) => rejected.push(RejectedRoom {
                room: score.room.clone(),
                reason: reason.clone(),
            }),
            None => finalists.push(score.clone()),
        }
    }
    info!(
        scored = stage1_result.ranked.len(),
        finalists = finalists.len(),
        needs_fetch = stage1_result.needs_fetch.len(),
        profile = ?options.profile,
        "stage 1 complete; planning finalists"
    );

    let mut recommendations = Vec::new();
    for finalist in finalists {
        let room = cache
            .get(&finalist.room)
            .expect("finalists come from the cache");
        let source = match CachedRoomDataSource::from_room(room) {
            Ok(source) => source,
            Err(err) => {
                rejected.push(RejectedRoom {
                    room: finalist.room.clone(),
                    reason: format!("{err:#}"),
                });
                continue;
            }
        };
        debug!(room = %finalist.room, "planning");
        let started = Instant::now();
        match run_planner(&source, options.profile, options.plan_timeout) {
            Ok(plan) => match first_spawn_tile(&plan) {
                Some((x, y, rcl)) => {
                    let elapsed = started.elapsed().as_secs_f32();
                    info!(
                        room = %finalist.room,
                        score = plan.score.total,
                        spawn = ?(x, y),
                        seconds = elapsed,
                        "plan complete"
                    );
                    recommendations.push(Recommendation {
                        room: finalist.room.clone(),
                        heuristic: finalist,
                        plan_score: plan.score.clone(),
                        spawn: (x, y),
                        spawn_rcl: rcl,
                        plan_seconds: elapsed,
                    });
                }
                None => rejected.push(RejectedRoom {
                    room: finalist.room.clone(),
                    reason: "plan contains no spawn".to_owned(),
                }),
            },
            Err(reason) => {
                warn!(room = %finalist.room, reason, "plan failed");
                rejected.push(RejectedRoom {
                    room: finalist.room.clone(),
                    reason: format!("planning failed: {reason}"),
                });
            }
        }
    }
    recommendations.sort_by(|a, b| {
        b.plan_score
            .total
            .partial_cmp(&a.plan_score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.room.cmp(&b.room))
    });
    RecommendResult {
        stage1: stage1_result,
        recommendations,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{CachedRoom, RoomStatus, TERRAIN_LEN};
    use std::path::{Path, PathBuf};

    /// Bench seed fixture — READ-ONLY; stage-2 tests operate on a COPY.
    const BENCH_PRIVATE_SERVER: &str =
        "../screeps-foreman-bench/resources/default-private-server.json";
    /// One 2-source + controller + mineral room from the bench default
    /// private-server map (verified via the bench's own `list-rooms`).
    const BENCH_ROOM: &str = "W9N9";

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "screeps-prospector-score-tests-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Literal terrain fixture: walled border with a 10-tile exit on the
    /// top edge, interior filled with `interior` (a terrain hex digit).
    fn fixture_terrain(interior: char) -> String {
        let mut out = String::with_capacity(TERRAIN_LEN);
        for y in 0..50 {
            for x in 0..50 {
                let border = x == 0 || y == 0 || x == 49 || y == 49;
                let c = if border {
                    if y == 0 && (20..30).contains(&x) {
                        '0'
                    } else {
                        '1'
                    }
                } else {
                    interior
                };
                out.push(c);
            }
        }
        out
    }

    fn objects(sources: usize, controller: bool, mineral: Option<&str>) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let source_positions = [(14, 9), (38, 41), (10, 40)];
        for &(x, y) in source_positions.iter().take(sources) {
            out.push(serde_json::json!({"type": "source", "x": x, "y": y}));
        }
        if controller {
            out.push(serde_json::json!({"type": "controller", "x": 21, "y": 32}));
        }
        if let Some(kind) = mineral {
            out.push(serde_json::json!({"type": "mineral", "x": 40, "y": 40, "mineralType": kind}));
        }
        out
    }

    fn fixture_room(
        name: &str,
        interior: char,
        sources: usize,
        controller: bool,
        mineral: Option<&str>,
    ) -> CachedRoom {
        CachedRoom {
            terrain: fixture_terrain(interior),
            objects: objects(sources, controller, mineral),
            fetched_at: Some(1_780_000_000),
            ..CachedRoom::new(name)
        }
    }

    fn fixture_cache() -> RoomCache {
        RoomCache {
            description: "test:stage1".to_owned(),
            rooms: vec![
                fixture_room("W1N1", '0', 2, true, Some("X")),  // ideal
                fixture_room("W2N1", '2', 2, true, Some("X")),  // swampy
                fixture_room("W3N1", '0', 1, true, Some("X")),  // one source
                fixture_room("W4N1", '0', 2, false, Some("X")), // no controller
                CachedRoom::new("W5N1"),                        // never fetched
            ],
        }
    }

    fn room_names(cache: &RoomCache) -> Vec<String> {
        cache.rooms.iter().map(|r| r.room.clone()).collect()
    }

    // ---- stage 1: pure subscores ----

    #[test]
    fn source_count_strongly_prefers_two() {
        assert_eq!(source_count_score(2), 1.0);
        assert!(source_count_score(2) > source_count_score(3));
        assert!(source_count_score(3) > source_count_score(1));
        assert!(source_count_score(1) > source_count_score(0));
        assert_eq!(source_count_score(0), 0.0);
    }

    #[test]
    fn mineral_type_preference_orders_x_first() {
        let info = |t: &str| MineralInfo {
            x: 0,
            y: 0,
            mineral_type: Some(t.to_owned()),
        };
        assert_eq!(mineral_score(None), 0.0);
        let x = mineral_score(Some(&info("X")));
        let h = mineral_score(Some(&info("H")));
        let z = mineral_score(Some(&info("Z")));
        assert!(x > h && h > z && z > 0.0);
    }

    #[test]
    fn exit_score_prefers_few_tiles_on_few_sides() {
        let one_side_small = exit_score(10, 1);
        let one_side_large = exit_score(150, 1);
        let four_sides = exit_score(150, 4);
        assert!(one_side_small > one_side_large);
        assert!(one_side_large > four_sides);
        assert_eq!(exit_score(0, 0), 0.0, "sealed room = bad data");
    }

    #[test]
    fn terrain_metrics_on_literal_fixture() {
        let terrain =
            FastRoomTerrain::new(crate::cache::terrain_hex_to_vec(&fixture_terrain('2')).unwrap());
        let metrics = terrain_metrics(&terrain);
        // Border = 196 tiles, 10 opened as exits on the top edge.
        assert_eq!(metrics.exit_tiles, 10);
        assert_eq!(metrics.exit_sides, 1);
        let walls = 196 - 10;
        assert!((metrics.wall_fraction - walls as f32 / 2500.0).abs() < 1e-6);
        // Interior (48*48) is all swamp; the 10 exit tiles are plain.
        let non_wall = 2500 - walls;
        assert!((metrics.swamp_fraction - (48.0 * 48.0) / non_wall as f32).abs() < 1e-6);
    }

    // ---- stage 1: ranking on literal fixtures ----

    #[test]
    fn stage1_ranks_fixture_rooms_deterministically() {
        let cache = fixture_cache();
        let result = stage1(&cache, &room_names(&cache), &HeuristicWeights::default());

        let order: Vec<&str> = result.ranked.iter().map(|s| s.room.as_str()).collect();
        // ideal > swampy (swamp penalty ~1.0 weighted) > one-source
        // (source penalty ~1.65 weighted) > no-controller (controller
        // penalty 2.0 weighted).
        assert_eq!(order, vec!["W1N1", "W2N1", "W3N1", "W4N1"]);

        let ideal = &result.ranked[0];
        assert!(
            ideal.total > 0.95,
            "ideal room scores high: {}",
            ideal.total
        );
        assert!(ideal.disqualified.is_none());
        assert_eq!(ideal.source_count, 2);
        assert_eq!(ideal.mineral_type.as_deref(), Some("X"));

        let no_controller = &result.ranked[3];
        assert!(
            no_controller
                .disqualified
                .as_deref()
                .unwrap()
                .contains("no controller"),
            "missing controller carries a reason"
        );

        // The unfetched room is FETCH-LISTED, not silently skipped.
        assert_eq!(result.needs_fetch.len(), 1);
        assert_eq!(result.needs_fetch[0].room, "W5N1");
        assert!(result.needs_fetch[0].reason.contains("terrain"));
    }

    #[test]
    fn stage1_fetch_lists_unknown_rooms_and_runs_are_repeatable() {
        let cache = fixture_cache();
        let mut rooms = room_names(&cache);
        rooms.push("W99N99".to_owned()); // not in the cache at all
        let weights = HeuristicWeights::default();
        let first = stage1(&cache, &rooms, &weights);
        let second = stage1(&cache, &rooms, &weights);
        assert_eq!(first.ranked, second.ranked, "stage 1 is deterministic");
        assert!(first
            .needs_fetch
            .iter()
            .any(|n| n.room == "W99N99" && n.reason.contains("not in cache")));
    }

    #[test]
    fn weights_are_configurable() {
        let cache = fixture_cache();
        let rooms = room_names(&cache);
        // Zero out everything except swamp: the swampy room must drop
        // below the one-source room.
        let swamp_only = HeuristicWeights {
            sources: 0.0,
            controller: 0.0,
            mineral: 0.0,
            swamp: 1.0,
            walls: 0.0,
            exits: 0.0,
        };
        let result = stage1(&cache, &rooms, &swamp_only);
        let swampy_rank = result.ranked.iter().position(|s| s.room == "W2N1").unwrap();
        let one_source_rank = result.ranked.iter().position(|s| s.room == "W3N1").unwrap();
        assert!(one_source_rank < swampy_rank);
    }

    #[test]
    fn select_rooms_policy() {
        let mut cache = fixture_cache();
        // No statuses at all -> default falls back to every cached room.
        let (rooms, selection) = select_rooms(&cache, None, false, false);
        assert_eq!(selection, RoomSelection::AllCachedNoStatuses);
        assert_eq!(rooms.len(), cache.rooms.len());

        // With statuses, the default narrows to open rooms — respawn-
        // area rooms included, novice-area rooms only on request.
        cache.rooms[0].spawn_status = Some(RoomStatus {
            open: true,
            novice: false,
            respawn: true,
        });
        cache.rooms[1].spawn_status = Some(RoomStatus {
            open: true,
            novice: true,
            respawn: false,
        });
        cache.rooms[2].spawn_status = Some(RoomStatus {
            open: false,
            novice: false,
            respawn: false,
        });
        let novice_room = cache.rooms[1].room.clone();
        let (rooms, selection) = select_rooms(&cache, None, false, false);
        assert_eq!(
            selection,
            RoomSelection::OpenRooms {
                include_novice: false
            }
        );
        assert_eq!(rooms, vec!["W1N1".to_owned()], "respawn in, novice out");
        let (rooms, selection) = select_rooms(&cache, None, false, true);
        assert_eq!(
            selection,
            RoomSelection::OpenRooms {
                include_novice: true
            }
        );
        assert_eq!(rooms, vec!["W1N1".to_owned(), novice_room]);

        // --all and --rooms override (no protection filtering).
        let (rooms, selection) = select_rooms(&cache, None, true, false);
        assert_eq!(selection, RoomSelection::AllCached);
        assert_eq!(rooms.len(), cache.rooms.len());
        let (rooms, selection) = select_rooms(&cache, Some(vec!["W7N7".to_owned()]), true, false);
        assert_eq!(selection, RoomSelection::Explicit);
        assert_eq!(rooms, vec!["W7N7".to_owned()]);
    }

    // ---- stage 2: planning over a COPY of the bench fixture ----

    fn bench_cache_copy(name: &str) -> RoomCache {
        let source = Path::new(BENCH_PRIVATE_SERVER);
        let dest = temp_path(name);
        std::fs::copy(source, &dest).expect(
            "bench fixture missing — run tests from the screeps-prospector crate directory",
        );
        RoomCache::load(&dest).unwrap()
    }

    /// Always-on stage-2 test: the REDUCED profile plans one real bench
    /// room fast, deterministically, and yields the hub stamp's RCL-1
    /// spawn as the proposed tile.
    #[test]
    fn reduced_profile_plans_a_bench_room_deterministically() {
        let cache = bench_cache_copy("stage2-reduced.json");
        let room = cache.get(BENCH_ROOM).expect("bench room exists");
        let source = CachedRoomDataSource::from_room(room).unwrap();

        let first = run_planner(&source, PlanProfile::Reduced, Duration::from_secs(30))
            .expect("reduced-profile planning succeeds");
        let second = run_planner(&source, PlanProfile::Reduced, Duration::from_secs(30))
            .expect("reduced-profile planning succeeds twice");

        let spawn_a = first_spawn_tile(&first).expect("plan carries a spawn");
        let spawn_b = first_spawn_tile(&second).unwrap();
        assert_eq!(spawn_a, spawn_b, "planning is deterministic");
        assert_eq!(
            first.score.total, second.score.total,
            "scores are deterministic"
        );

        let (x, y, rcl) = spawn_a;
        assert_eq!(rcl, 1, "the first spawn is the hub stamp's RCL-1 spawn");
        assert!((2..=47).contains(&x) && (2..=47).contains(&y), "in bounds");
        assert!(first.score.total > 0.0, "score layers contributed");
    }

    /// Stage-2 end-to-end through `recommend` on ONE bench room with the
    /// FULL default layer stack — a real foreman run (seconds, not
    /// minutes; kept to one room on purpose).
    #[test]
    fn full_profile_recommends_one_bench_room_end_to_end() {
        let cache = bench_cache_copy("stage2-full.json");
        let options = RecommendOptions {
            finalists: 1,
            profile: PlanProfile::Full,
            ..RecommendOptions::default()
        };
        let result = recommend(&cache, &[BENCH_ROOM.to_owned()], &options);

        assert!(
            result.rejected.is_empty(),
            "rejected: {:?}",
            result.rejected
        );
        assert_eq!(result.recommendations.len(), 1);
        let rec = &result.recommendations[0];
        assert_eq!(rec.room, BENCH_ROOM);
        assert_eq!(rec.spawn_rcl, 1);
        assert!(rec.plan_score.total > 0.0);
        let (x, y) = rec.spawn;
        assert!((2..=47).contains(&x) && (2..=47).contains(&y));
        // The heuristic facts came along for the table.
        assert_eq!(rec.heuristic.source_count, 2);
        assert!(rec.heuristic.has_controller);
    }

    #[test]
    fn zero_budget_times_out_with_reason() {
        let cache = bench_cache_copy("stage2-timeout.json");
        let room = cache.get(BENCH_ROOM).unwrap();
        let source = CachedRoomDataSource::from_room(room).unwrap();
        let err = run_planner(&source, PlanProfile::Reduced, Duration::ZERO).unwrap_err();
        assert!(err.contains("budget"), "reason mentions the budget: {err}");
    }

    #[test]
    fn disqualified_rooms_are_rejected_not_planned() {
        let cache = RoomCache {
            description: "test:disqualified".to_owned(),
            rooms: vec![fixture_room("W4N1", '0', 2, false, None)],
        };
        let started = Instant::now();
        let result = recommend(
            &cache,
            &["W4N1".to_owned()],
            &RecommendOptions::default(), // FULL profile: would take seconds if planned
        );
        assert!(result.recommendations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0].reason.contains("no controller"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "disqualified rooms must not reach the planner"
        );
    }

    #[test]
    fn recommend_propagates_fetch_list() {
        let mut cache = RoomCache {
            description: "test:needs-fetch".to_owned(),
            rooms: vec![CachedRoom::new("W8N8")], // scanned-only: no terrain
        };
        cache.rooms[0].spawn_status = Some(RoomStatus {
            open: true,
            novice: false,
            respawn: false,
        });
        let result = recommend(&cache, &["W8N8".to_owned()], &RecommendOptions::default());
        assert!(result.recommendations.is_empty());
        assert_eq!(result.stage1.needs_fetch.len(), 1);
        assert_eq!(result.stage1.needs_fetch[0].room, "W8N8");
    }
}
