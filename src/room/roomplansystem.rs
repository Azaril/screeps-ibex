use super::data::*;
use screeps::*;
use specs::*;
use serde::{Deserialize, Serialize};
use specs::prelude::{Entities, ResourceId, System, SystemData, World, Write, WriteStorage};
use specs_derive::*;
use super::planner::*;
use crate::ui::*;
use crate::memorysystem::*;
use crate::visualize::*;

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
}

#[derive(Clone, Deserialize, Serialize, Component)]
pub struct RoomPlanData {
    plan: Plan,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct RoomPlannerRunningData {
    room: RoomName,
    planner_state: PlanRunningStateData,
}

#[derive(Clone, Deserialize, Serialize, Default)]
pub struct RoomPlannerData {
    running_state: Option<RoomPlannerRunningData>,
}

#[derive(SystemData)]
pub struct RoomPlanSystemData<'a> {
    memory_arbiter: Write<'a, MemoryArbiter>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    room_plan_data: WriteStorage<'a, RoomPlanData>,
    room_plan_queue: Write<'a, RoomPlanQueue>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

pub struct RoomPlanSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RoomPlanSystem {
    type SystemData = RoomPlanSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        const MEMORY_SEGMENT: u32 = 60;

        /*
        data.memory_arbiter.request(MEMORY_SEGMENT);

        if data.memory_arbiter.is_active(MEMORY_SEGMENT) {
            let planner_data = data.memory_arbiter.get(MEMORY_SEGMENT).unwrap();

            let planner_state = if !planner_data.is_empty() {
                serde_json::from_str(&planner_data).unwrap_or_default()
            } else {
                RoomPlannerData::default()
            };

            if let Ok(output_planner_data) = serde_json::to_string(&planner_state) {
                data.memory_arbiter.set(MEMORY_SEGMENT, &output_planner_data);
            }
        }
        */
    }
}
