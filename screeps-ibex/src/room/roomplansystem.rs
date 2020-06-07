use super::data::*;
use screeps_foreman::layout::*;
use screeps_foreman::planner::*;
use crate::entitymappingsystem::*;
use crate::memorysystem::*;
use crate::ui::*;
use crate::visualize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::prelude::{Entities, ResourceId, System, SystemData, World, Write, WriteStorage};
use specs::*;
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub struct RoomPlanRequest {
    room_name: RoomName,
    priority: f32,
}

impl RoomPlanRequest {
    pub fn new(room_name: RoomName, priority: f32) -> RoomPlanRequest {
        RoomPlanRequest { room_name, priority }
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

#[derive(Clone, Deserialize, Serialize, Component)]
pub struct RoomPlanData {
    plan: Plan,
}

impl RoomPlanData {
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
}

struct RoomDataPlannerDataSource<'a> {
    room_name: RoomName,
    static_visibility: &'a RoomStaticVisibilityData,
    terrain: Option<FastRoomTerrain>,
    controllers: Option<Vec<PlanLocation>>,
    sources: Option<Vec<PlanLocation>>,
    minerals: Option<Vec<PlanLocation>>,
}

impl<'a> RoomDataPlannerDataSource<'a> {
    pub fn new(room_name: RoomName, static_visibility: &RoomStaticVisibilityData) -> RoomDataPlannerDataSource {
        RoomDataPlannerDataSource {
            room_name,
            static_visibility,
            terrain: None,
            controllers: None,
            sources: None,
            minerals: None,
        }
    }
}

impl<'a> PlannerRoomDataSource for RoomDataPlannerDataSource<'a> {
    fn get_terrain(&mut self) -> &FastRoomTerrain {
        if self.terrain.is_none() {
            let room_terrain = game::map::get_room_terrain(self.room_name);
            let terrain_data = room_terrain.get_raw_buffer();

            self.terrain = Some(FastRoomTerrain::new(terrain_data))
        }

        self.terrain.as_ref().unwrap()
    }

    fn get_controllers(&mut self) -> &[PlanLocation] {
        if self.controllers.is_none() {
            let controllers = self
                .static_visibility
                .controller()
                .iter()
                .map(|id| {
                    let pos = id.pos();
                    PlanLocation::new(pos.x() as i8, pos.y() as i8)
                })
                .collect();

            self.controllers = Some(controllers);
        }

        self.controllers.as_ref().unwrap()
    }

    fn get_sources(&mut self) -> &[PlanLocation] {
        if self.sources.is_none() {
            let sources = self
                .static_visibility
                .sources()
                .iter()
                .map(|id| {
                    let pos = id.pos();
                    PlanLocation::new(pos.x() as i8, pos.y() as i8)
                })
                .collect();

            self.sources = Some(sources);
        }

        self.sources.as_ref().unwrap()
    }

    fn get_minerals(&mut self) -> &[PlanLocation] {
        if self.minerals.is_none() {
            let minerals = self
                .static_visibility
                .minerals()
                .iter()
                .map(|id| {
                    let pos = id.pos();
                    PlanLocation::new(pos.x() as i8, pos.y() as i8)
                })
                .collect();

            self.minerals = Some(minerals);
        }

        self.minerals.as_ref().unwrap()
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct RoomPlannerRunningData {
    room_name: RoomName,
    planner_state: PlanRunningStateData,
}

impl RoomPlannerRunningData {
    fn seed(room_data: &RoomData) -> Result<PlanSeedResult, String> {
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        let mut data_source = RoomDataPlannerDataSource::new(room_data.name, static_visibility_data);

        let planner = Planner::new(screeps_foreman::scoring::score_state);

        planner.seed(ALL_ROOT_NODES, &mut data_source)
    }

    fn process(&mut self, room_data: &RoomData, budget: f64) -> Result<PlanEvaluationResult, String> {
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;

        let mut data_source = RoomDataPlannerDataSource::new(room_data.name, static_visibility_data);

        let planner = Planner::new(screeps_foreman::scoring::score_state);

        let start_cpu = game::cpu::get_used();

        let should_continue = || (game::cpu::get_used() - start_cpu) < budget;

        planner.evaluate(ALL_ROOT_NODES, &mut data_source, &mut self.planner_state, should_continue)
    }
}

#[derive(Clone, Deserialize, Serialize, Default)]
pub struct RoomPlannerData {
    running_state: Option<RoomPlannerRunningData>,
    last_planned: HashMap<RoomName, u32>,
}

#[derive(SystemData)]
pub struct RoomPlanSystemData<'a> {
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
    entities: Entities<'a>,
    mapping: Read<'a, EntityMappingData>,
    room_data: WriteStorage<'a, RoomData>,
    room_plan_data: WriteStorage<'a, RoomPlanData>,
    room_plan_queue: Write<'a, RoomPlanQueue>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

pub struct RoomPlanSystem;

impl RoomPlanSystem {
    fn seed(data: &mut RoomPlanSystemData, room_name: RoomName) -> Result<PlanSeedResult, String> {
        let room_entity = data.mapping.get_room(&room_name).ok_or("Expected room entity")?;
        let room_data = data.room_data.get(room_entity).ok_or("Expected room data")?;

        RoomPlannerRunningData::seed(&room_data)
    }

    fn process(
        data: &mut RoomPlanSystemData,
        room_name: RoomName,
        planner_state: &mut RoomPlannerRunningData,
    ) -> Result<PlanEvaluationResult, String> {
        let room_entity = data.mapping.get_room(&room_name).ok_or("Expected room entity")?;
        let room_data = data.room_data.get(room_entity).ok_or("Expected room data")?;

        let bucket = game::cpu::bucket();
        let tick_limit = game::cpu::tick_limit();
        let current_cpu = game::cpu::get_used();
        let remaining_cpu = tick_limit - current_cpu;
        let max_cpu = (remaining_cpu * 0.25).min(tick_limit / 2.0);

        if bucket >= tick_limit * 2.0 && max_cpu >= 20.0 {
            planner_state.process(&room_data, max_cpu)
        } else {
            Ok(PlanEvaluationResult::Running())
        }
    }

    fn attach_plan(data: &mut RoomPlanSystemData, room_name: RoomName, plan: Plan) -> Result<(), String> {
        let room_entity = data.mapping.get_room(&room_name).ok_or("Expected room entity")?;

        if let Some(room_plan_data) = data.room_plan_data.get_mut(room_entity) {
            room_plan_data.plan = plan;
        } else {
            data.room_plan_data
                .insert(room_entity, RoomPlanData { plan })
                .map_err(|err| err.to_string())?;
        }

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RoomPlanSystem {
    type SystemData = RoomPlanSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        const MEMORY_SEGMENT: u32 = 60;

        data.memory_arbiter.request(MEMORY_SEGMENT);

        if data.memory_arbiter.is_active(MEMORY_SEGMENT) {
            let planner_data = data.memory_arbiter.get(MEMORY_SEGMENT).unwrap();

            let mut planner_state = if !planner_data.is_empty() {
                match crate::serialize::decode_from_string(&planner_data) {
                    Ok(state) => state,
                    Err(err) => {
                        info!("Failed to decode planner state, resetting. Err: {}", err);
                        RoomPlannerData::default()
                    }
                }
            } else {
                RoomPlannerData::default()
            };

            if planner_state.running_state.is_none() {
                let request = data
                    .room_plan_queue
                    .requests
                    .iter()
                    .filter(|request| {
                        crate::features::construction::force_plan()
                            || planner_state
                                .last_planned
                                .get(&request.room_name)
                                .map(|last_completion| game::time() >= last_completion + 2000)
                                .map(|can_plan| can_plan && crate::features::construction::allow_replan())
                                .unwrap_or(true)
                    })
                    .max_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap())
                    .cloned();

                if let Some(request) = request {
                    planner_state.last_planned.insert(request.room_name, game::time());

                    match Self::seed(&mut data, request.room_name) {
                        Ok(PlanSeedResult::Running(state)) => {
                            info!("Seeding complete for room plan. Room: {}", request.room_name);

                            planner_state.running_state = Some(RoomPlannerRunningData {
                                room_name: request.room_name,
                                planner_state: state,
                            });
                        }
                        Ok(PlanSeedResult::Complete(Some(plan))) => {
                            info!("Seeding complete and viable plan found. Room: {}", request.room_name);

                            if let Err(err) = Self::attach_plan(&mut data, request.room_name, plan) {
                                info!("Failed to attach plan to room! Room: {} - Err: {}", request.room_name, err);
                            }
                        }
                        Ok(PlanSeedResult::Complete(None)) => {
                            info!("Seeding complete but no viable plan found. Room: {}", request.room_name);
                        }
                        Err(err) => {
                            info!("Seeding failure! Room: {} - Error: {}", request.room_name, err);
                        }
                    }
                }
            }

            let is_complete = if let Some(mut running_state) = planner_state.running_state.as_mut() {
                planner_state.last_planned.insert(running_state.room_name, game::time());

                match Self::process(&mut data, running_state.room_name, &mut running_state) {
                    Ok(PlanEvaluationResult::Running()) => false,
                    Ok(PlanEvaluationResult::Complete(Some(plan))) => {
                        info!("Planning complete and viable plan found. Room: {}", running_state.room_name);

                        if let Err(err) = Self::attach_plan(&mut data, running_state.room_name, plan) {
                            info!("Failed to attach plan to room! Room: {} - Error: {}", running_state.room_name, err);
                        }

                        true
                    }
                    Ok(PlanEvaluationResult::Complete(None)) => {
                        info!("Planning complete but no viable plan found. Room: {}", running_state.room_name);

                        true
                    }
                    Err(err) => {
                        info!("Planning failure! Room: {} - Error: {}", running_state.room_name, err);

                        true
                    }
                }
            } else {
                false
            };

            if is_complete {
                planner_state.running_state = None;
            }

            if crate::features::construction::visualize() {
                if let Some(running_state) = &planner_state.running_state {
                    if let Some(visualizer) = &mut data.visualizer {
                        let room_visualizer = visualizer.get_room(running_state.room_name);

                        if crate::features::construction::visualize_planner() {
                            running_state.planner_state.visualize(room_visualizer);
                        }

                        if crate::features::construction::visualize_planner_best() {
                            running_state.planner_state.visualize_best(room_visualizer);
                        }
                    }
                }
            }

            if crate::features::construction::visualize_plan() {
                if let Some(visualizer) = &mut data.visualizer {
                    for (_, room_data, room_plan_data) in (&data.entities, &data.room_data, &data.room_plan_data).join() {
                        let room_visualizer = visualizer.get_room(room_data.name);

                        room_plan_data.plan.visualize(room_visualizer);
                    }
                }
            }

            if let Ok(output_planner_data) = crate::serialize::encode_to_string(&planner_state) {
                data.memory_arbiter.set(MEMORY_SEGMENT, &output_planner_data);
            }
        }

        data.room_plan_queue.clear();
    }
}

impl screeps_foreman::RoomVisualizer for RoomVisualizer {
    fn render(&mut self, location: screeps_foreman::location::Location, structure: StructureType) {
        match structure {
            StructureType::Spawn => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("green").opacity(1.0)),
                );
            }
            StructureType::Extension => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("purple").opacity(1.0)),
                );
            }
            StructureType::Container => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("blue").opacity(1.0)),
                );
            }
            StructureType::Storage => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("red").opacity(1.0)),
                );
            }
            StructureType::Link => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("orange").opacity(1.0)),
                );
            }
            StructureType::Terminal => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("pink").opacity(1.0)),
                );
            }
            StructureType::Nuker => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("black").opacity(1.0)),
                );
            }
             StructureType::Lab => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("aqua").opacity(1.0)),
                );
            }
            StructureType::PowerSpawn => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("Fuschia").opacity(1.0)),
                );
            } 
            StructureType::Observer => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("Lime").opacity(1.0)),
                );
            } 
            StructureType::Factory => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("Brown").opacity(1.0)),
                );
            }
            StructureType::Rampart => {
                RoomVisualizer::rect(
                    self,
                    location.x() as f32 - 0.5,
                    location.y() as f32 - 0.5,
                    1.0,
                    1.0,
                    Some(RectStyle::default().fill("Green").opacity(0.3)),
                );
            }
            _ => {
                RoomVisualizer::circle(
                    self,
                    location.x() as f32,
                    location.y() as f32,
                    Some(CircleStyle::default().fill("yellow").opacity(1.0)),
                );
            }
        }
    }
}