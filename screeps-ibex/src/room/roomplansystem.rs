use super::data::*;
use crate::entitymappingsystem::*;
use crate::memorysystem::*;
use crate::segments::PLANNER_MEMORY_SEGMENT;
use crate::visualize::RoomVisualizer;
use log::*;
use screeps::*;
use screeps_foreman::pipeline::{CpuBudget, PlanningState};
use screeps_foreman::plan::Plan;
use screeps_foreman::planner::*;
use screeps_foreman::room_data::*;
use serde::{Deserialize, Serialize};
use specs::prelude::{Entities, ResourceId, System, SystemData, World, Write, WriteStorage};
use specs::*;

// ---------------------------------------------------------------------------
// Shared types (used by both planning and visualization)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct RoomPlanRequest {
    room: Entity,
    priority: f32,
}

impl RoomPlanRequest {
    pub fn new(room: Entity, priority: f32) -> RoomPlanRequest {
        // Tripwire (IBEX-046): the queue comparator coalesces NaN to Equal;
        // assert finiteness where the priority is produced instead.
        debug_assert!(priority.is_finite(), "room plan request priority not finite: {priority}");

        RoomPlanRequest { room, priority }
    }
}

#[derive(Default)]
pub struct RoomPlanQueue {
    pub requests: Vec<RoomPlanRequest>,
}

impl RoomPlanQueue {
    pub fn request(&mut self, room_plan_request: RoomPlanRequest) {
        self.requests.push(room_plan_request);
    }

    fn clear(&mut self) {
        self.requests.clear();
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub enum RoomPlanState {
    Valid(Plan),
    Failed {
        time: u32,
        /// Consecutive failures — drives the exponential replan backoff
        /// (P1.D3 / IBEX-037: a room that cannot plan must not thrash
        /// the multi-tick planner every 2000 ticks forever).
        #[serde(default)]
        attempts: u32,
    },
}

/// Replan backoff (P1.D3 / IBEX-037): base 2000 ticks, doubling per
/// consecutive failure, capped at 16× (32k ticks ≈ 9 real-time hours
/// at 1 s/tick) — terrain rarely changes, but ownership/structures do.
pub fn replan_backoff_ticks(attempts: u32) -> u32 {
    2000u32.saturating_mul(1 << attempts.min(4))
}

impl RoomPlanState {
    pub fn valid(&self) -> bool {
        match self {
            RoomPlanState::Valid(_) => true,
            RoomPlanState::Failed { .. } => false,
        }
    }

    pub fn plan(&self) -> Option<&Plan> {
        match self {
            RoomPlanState::Valid(plan) => Some(plan),
            RoomPlanState::Failed { .. } => None,
        }
    }
}

#[derive(Clone, Deserialize, Serialize, Component)]
pub struct RoomPlanData {
    state: RoomPlanState,
}

impl RoomPlanData {
    pub fn valid(&self) -> bool {
        self.state.valid()
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.state.plan()
    }
}

// ---------------------------------------------------------------------------
// Planner internals
// ---------------------------------------------------------------------------

struct RoomDataPlannerDataSource {
    room_name: RoomName,
    terrain: FastRoomTerrain,
    controllers: Vec<PlanLocation>,
    sources: Vec<PlanLocation>,
    minerals: Vec<PlanLocation>,
}

impl RoomDataPlannerDataSource {
    pub fn new(room_name: RoomName, static_visibility: &RoomStaticVisibilityData) -> RoomDataPlannerDataSource {
        let terrain = if let Some(room_terrain) = game::map::get_room_terrain(room_name) {
            let terrain_data = room_terrain.get_raw_buffer().to_vec();
            FastRoomTerrain::new(terrain_data)
        } else {
            const ROOM_SIZE: usize = 50 * 50;
            FastRoomTerrain::new(vec![0u8; ROOM_SIZE])
        };

        let mut controllers: Vec<_> = static_visibility
            .controller()
            .iter()
            .map(|id| {
                let pos = id.pos();
                PlanLocation::new(pos.x().u8() as i8, pos.y().u8() as i8)
            })
            .collect();
        controllers.sort_by(|a, b| a.x().cmp(&b.x()).then_with(|| a.y().cmp(&b.y())));

        let mut sources: Vec<_> = static_visibility
            .sources()
            .iter()
            .map(|id| {
                let pos = id.pos();
                PlanLocation::new(pos.x().u8() as i8, pos.y().u8() as i8)
            })
            .collect();
        sources.sort_by(|a, b| a.x().cmp(&b.x()).then_with(|| a.y().cmp(&b.y())));

        let mut minerals: Vec<_> = static_visibility
            .minerals()
            .iter()
            .map(|id| {
                let pos = id.pos();
                PlanLocation::new(pos.x().u8() as i8, pos.y().u8() as i8)
            })
            .collect();
        minerals.sort_by(|a, b| a.x().cmp(&b.x()).then_with(|| a.y().cmp(&b.y())));

        RoomDataPlannerDataSource {
            room_name,
            terrain,
            controllers,
            sources,
            minerals,
        }
    }
}

impl PlannerRoomDataSource for RoomDataPlannerDataSource {
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

#[derive(Deserialize, Serialize)]
pub struct RoomPlannerRunningData {
    room_name: RoomName,
    planner_state: PlanningState,
}

impl RoomPlannerRunningData {
    fn start(room_data: &RoomData) -> Result<Self, String> {
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        let _data_source = RoomDataPlannerDataSource::new(room_data.name, static_visibility_data);

        let state = PlannerBuilder::default().build();

        Ok(RoomPlannerRunningData {
            room_name: room_data.name,
            planner_state: state,
        })
    }

    fn process(&mut self, room_data: &RoomData, budget_cpu: f64, tick_limit: f64) -> Result<PlanTickResult, String> {
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        let data_source = RoomDataPlannerDataSource::new(room_data.name, static_visibility_data);

        let start_cpu = game::cpu::get_used();
        // Use 0.75 of allocated budget so we stop early and avoid overshooting the tick.
        let budget = CpuBudget::new(move || {
            let used = game::cpu::get_used();
            (used - start_cpu) < (budget_cpu * 0.75) && used < tick_limit - 0.5
        });

        let old_state = std::mem::replace(&mut self.planner_state, PlanningState::Failed("replaced".to_string()));

        // Re-inject layers via PlannerBuilder::resume() each tick.
        // If fingerprint mismatches (layer config changed), restart planning.
        let builder = PlannerBuilder::default();
        let resumed_state = match builder.resume(old_state) {
            Ok(state) => state,
            Err(_) => {
                info!("Layer fingerprint mismatch, restarting planning for {}", room_data.name);
                PlannerBuilder::default().build()
            }
        };

        let new_state = screeps_foreman::pipeline::tick_pipeline(resumed_state, &data_source, &budget);

        match new_state {
            PlanningState::Complete(plan) => {
                self.planner_state = PlanningState::Complete(plan.clone());
                Ok(PlanTickResult::Complete(Some(plan)))
            }
            PlanningState::Failed(msg) => {
                self.planner_state = PlanningState::Failed(msg.clone());
                Ok(PlanTickResult::Failed(msg))
            }
            other => {
                self.planner_state = other;
                Ok(PlanTickResult::Running)
            }
        }
    }
}

enum PlanTickResult {
    Running,
    Complete(Option<Plan>),
    Failed(String),
}

/// Persistent planner running state, serialized to `PLANNER_MEMORY_SEGMENT`.
#[derive(Deserialize, Serialize, Default)]
pub struct RoomPlannerData {
    running_state: Option<RoomPlannerRunningData>,
}

// ---------------------------------------------------------------------------
// RoomPlanSystem — planning only, no visualization
// ---------------------------------------------------------------------------

#[derive(SystemData)]
pub struct RoomPlanSystemData<'a> {
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
    entities: Entities<'a>,
    mapping: Read<'a, EntityMappingData>,
    room_data: WriteStorage<'a, RoomData>,
    room_plan_data: WriteStorage<'a, RoomPlanData>,
    room_plan_queue: Write<'a, RoomPlanQueue>,
    governor: Read<'a, crate::cpugovernor::GovernorSnapshot>,
    features: Read<'a, crate::features::Features>,
}

pub struct RoomPlanSystem;

impl RoomPlanSystem {
    /// Returns (budget_cpu, tick_limit) when room planning may run. Budget is derived from
    /// remaining CPU and a reserve (like movement), never exceeds the tick limit, and
    /// ensures at least some work when there are pending requests to avoid deadlock.
    /// Planning runs only when bucket is at or above bucket_threshold (0 = always allow).
    /// `bucket` comes from the governor snapshot (M1: the one CPU-pressure truth).
    fn get_cpu_budget(has_pending_request: bool, bucket: i32, construction: &crate::features::ConstructionFeatures) -> Option<(f64, f64)> {
        if construction.bucket_threshold > 0 && bucket < construction.bucket_threshold {
            return None;
        }

        let tick_limit = game::cpu::tick_limit();
        let cpu_limit = game::cpu::limit() as f64;
        let used = game::cpu::get_used();
        let remaining = (tick_limit - used).max(0.0);

        // Under normal conditions use GCL limit; when bucket is at/above threshold allow burst.
        let budget_ceiling = if construction.bucket_threshold == 0 || bucket >= construction.bucket_threshold {
            tick_limit
        } else {
            cpu_limit
        };

        const RESERVE_FRACTION: f64 = 0.2;
        const RESERVE_FLOOR: f64 = 2.0;
        let reserve = (budget_ceiling * RESERVE_FRACTION).max(RESERVE_FLOOR);
        let planning_cpu_available = (remaining - reserve).max(0.0);

        let configured = construction.room_plan_cpu_budget;
        let mut budget_cpu = configured.min(planning_cpu_available);

        if budget_cpu <= 0.0 && has_pending_request && remaining > 5.0 {
            budget_cpu = 5.0;
        }
        if budget_cpu < 1.0 {
            return None;
        }

        Some((budget_cpu, tick_limit))
    }

    /// Consecutive-failure count for the NEXT `Failed` state: prior
    /// failures + 1, or 0 for a first failure (P1.D3 backoff input).
    fn next_failure_attempts(room_plan_data_storage: &WriteStorage<RoomPlanData>, room: Entity) -> u32 {
        match room_plan_data_storage.get(room).map(|d| &d.state) {
            Some(RoomPlanState::Failed { attempts, .. }) => attempts.saturating_add(1),
            _ => 0,
        }
    }

    fn attach_plan_state(
        room_plan_data_storage: &mut WriteStorage<RoomPlanData>,
        room: Entity,
        state: RoomPlanState,
    ) -> Result<(), String> {
        if let Some(room_plan_data) = room_plan_data_storage.get_mut(room) {
            room_plan_data.state = state;
        } else {
            room_plan_data_storage
                .insert(room, RoomPlanData { state })
                .map_err(|err| err.to_string())?;
        }

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RoomPlanSystem {
    type SystemData = RoomPlanSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.memory_arbiter.request(PLANNER_MEMORY_SEGMENT);

        let construction = data.features.construction;
        if construction.plan {
            let has_pending = !data.room_plan_queue.requests.is_empty()
                || data
                    .memory_arbiter
                    .get(PLANNER_MEMORY_SEGMENT)
                    .map(|d| !d.is_empty())
                    .unwrap_or(false);
            if let Some((max_cpu, tick_limit)) = Self::get_cpu_budget(has_pending, data.governor.bucket, &construction) {
                if data.memory_arbiter.is_active(PLANNER_MEMORY_SEGMENT) {
                    let Some(planner_data) = data.memory_arbiter.get(PLANNER_MEMORY_SEGMENT) else {
                        return;
                    };

                    let mut planner_state = if !planner_data.is_empty() {
                        match crate::serialize::decode_from_string(&planner_data) {
                            Ok(state) => state,
                            Err(err) => {
                                log::warn!(
                                    "Planner resume-state decode failed (seg 60), restarting planning from scratch: {}",
                                    err
                                );
                                RoomPlannerData::default()
                            }
                        }
                    } else {
                        RoomPlannerData::default()
                    };

                    if planner_state.running_state.is_none() {
                        let can_plan = |room: Entity| -> bool {
                            if let Some(plan_data) = data.room_plan_data.get(room) {
                                match plan_data.state {
                                    RoomPlanState::Valid(_) => construction.force_plan,
                                    RoomPlanState::Failed { time, attempts } => {
                                        game::time() >= time.saturating_add(replan_backoff_ticks(attempts)) && construction.allow_replan
                                    }
                                }
                            } else {
                                true
                            }
                        };

                        let request = data
                            .room_plan_queue
                            .requests
                            .iter()
                            .filter(|request| can_plan(request.room))
                            .filter(|request| data.room_data.get(request.room).is_some())
                            .max_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal))
                            .cloned();

                        if let Some(request) = request {
                            if let Some(room_data) = data.room_data.get(request.room) {
                                match RoomPlannerRunningData::start(room_data) {
                                    Ok(running_data) => {
                                        info!("Started planning for room: {}", room_data.name);
                                        planner_state.running_state = Some(running_data);
                                    }
                                    Err(err) => {
                                        info!("Failed to start planning! Room: {} - Error: {}", room_data.name, err);
                                    }
                                }
                            }
                        }
                    }

                    let is_complete = if let Some(running_state) = planner_state.running_state.as_mut() {
                        if let Some(room_entity) = data.mapping.get_room(&running_state.room_name) {
                            if let Some(room_data) = data.room_data.get(room_entity) {
                                info!("Planning for room: {}", room_data.name);

                                match running_state.process(room_data, max_cpu, tick_limit) {
                                    Ok(PlanTickResult::Running) => false,
                                    Ok(PlanTickResult::Complete(Some(plan))) => {
                                        info!("Planning complete and viable plan found. Room: {}", room_data.name);

                                        if let Err(err) =
                                            Self::attach_plan_state(&mut data.room_plan_data, room_entity, RoomPlanState::Valid(plan))
                                        {
                                            info!("Failed to attach plan to room! Room: {} - Error: {}", room_data.name, err);
                                        }

                                        true
                                    }
                                    Ok(PlanTickResult::Complete(None)) => {
                                        info!("Planning complete but no viable plan found. Room: {}", room_data.name);

                                        let attempts = Self::next_failure_attempts(&data.room_plan_data, room_entity);
                                        if let Err(err) = Self::attach_plan_state(
                                            &mut data.room_plan_data,
                                            room_entity,
                                            RoomPlanState::Failed {
                                                time: game::time(),
                                                attempts,
                                            },
                                        ) {
                                            info!("Failed to attach plan to room! Room: {} - Error: {}", room_data.name, err);
                                        }

                                        true
                                    }
                                    Ok(PlanTickResult::Failed(msg)) => {
                                        info!("Planning failed! Room: {} - Error: {}", room_data.name, msg);

                                        let attempts = Self::next_failure_attempts(&data.room_plan_data, room_entity);
                                        if let Err(err) = Self::attach_plan_state(
                                            &mut data.room_plan_data,
                                            room_entity,
                                            RoomPlanState::Failed {
                                                time: game::time(),
                                                attempts,
                                            },
                                        ) {
                                            info!("Failed to attach plan to room! Room: {} - Error: {}", room_data.name, err);
                                        }

                                        true
                                    }
                                    Err(err) => {
                                        info!("Planning error! Room: {} - Error: {}", room_data.name, err);
                                        true
                                    }
                                }
                            } else {
                                true
                            }
                        } else {
                            true
                        }
                    } else {
                        true
                    };

                    if is_complete {
                        planner_state.running_state = None;
                    }

                    match crate::serialize::encode_to_string(&planner_state) {
                        Ok(output_planner_data) => {
                            data.memory_arbiter.set(PLANNER_MEMORY_SEGMENT, &output_planner_data);
                        }
                        // Loud, not silent (P1.D3 / ADR 0009 step 1): a
                        // dropped write here silently restarts multi-tick
                        // planning from scratch next tick.
                        Err(err) => {
                            log::error!(
                                "Planner resume-state encode failed (seg 60); in-progress planning will restart: {}",
                                err
                            );
                        }
                    }
                }
            }
        }

        data.room_plan_queue.clear();
    }
}

// ---------------------------------------------------------------------------
// RoomVisualizer trait bridge (used by screeps-foreman's Plan::visualize)
// ---------------------------------------------------------------------------

impl screeps_foreman::RoomVisualizer for RoomVisualizer {
    fn render(&mut self, location: screeps_common::Location, structure: StructureType) {
        screeps_visual::render::render_structure(self, location.x() as f32, location.y() as f32, structure, 1.0);
    }
}

#[cfg(test)]
mod plan_backoff_tests {
    use super::replan_backoff_ticks;

    /// P1.D3 / IBEX-037: exponential, capped — a room that can't plan
    /// retries at 2k, 4k, 8k, 16k, then every 32k ticks, never faster.
    #[test]
    fn replan_backoff_doubles_then_caps() {
        assert_eq!(replan_backoff_ticks(0), 2_000);
        assert_eq!(replan_backoff_ticks(1), 4_000);
        assert_eq!(replan_backoff_ticks(2), 8_000);
        assert_eq!(replan_backoff_ticks(3), 16_000);
        assert_eq!(replan_backoff_ticks(4), 32_000);
        assert_eq!(replan_backoff_ticks(5), 32_000);
        assert_eq!(replan_backoff_ticks(u32::MAX), 32_000);
    }
}
