use super::data::*;
use super::missionsystem::*;
use crate::room::planner::*;
use crate::room::data::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::serialize::*;
use log::*;
use crate::room::layout::*;

#[derive(Clone, ConvertSaveload)]
pub struct ConstructionMission {
    room_data: Entity,
    next_update: Option<u32>,
    planner_state: Option<PlanRunningStateData>,
    plan: Option<Plan>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ConstructionMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ConstructionMission::new(room_data);

        builder
            .with(MissionData::Construction(mission))
            .marked::<SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> ConstructionMission {
        ConstructionMission {
            room_data,
            next_update: None,
            planner_state: None,
            plan: None,
        }
    }
}

struct RoomDataPlannerDataSource<'a> {
    room_name: RoomName,
    static_visibility: &'a RoomStaticVisibilityData,
    terrain: Option<FastRoomTerrain>,
    sources: Option<Vec<PlanLocation>>,
    minerals: Option<Vec<PlanLocation>>
}

impl<'a> RoomDataPlannerDataSource<'a> {
    pub fn new(room_name: RoomName, static_visibility: &RoomStaticVisibilityData) -> RoomDataPlannerDataSource {
        RoomDataPlannerDataSource {
            room_name,
            static_visibility,
            terrain: None,
            sources: None,
            minerals: None
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

    fn get_sources(&mut self) -> &[PlanLocation] {
        if self.sources.is_none() {
            let sources = self.static_visibility
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
            let minerals = self.static_visibility.minerals()
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ConstructionMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text("Construction".to_string(), None);
            })
        }
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;

        let should_update = self.next_update.map(|next_time| game::time() >= next_time).unwrap_or(true);

        if should_update && crate::features::construction::plan() {
            if (self.plan.is_none() || crate::features::construction::force_plan()) && self.planner_state.is_none() {
                info!("Starting room planning: {}", room_data.name);

                let mut data_source = RoomDataPlannerDataSource::new(room_data.name, static_visibility_data);

                let planner = Planner::new(crate::room::scoring::score_state);

                match planner.seed(ALL_ROOT_NODES, &mut data_source) {
                    Ok(PlanSeedResult::Complete(plan)) => {
                        if plan.is_some() {
                            info!("Room planning complete - Success - Room: {}", room_data.name);

                            self.plan = plan;
                        } else {
                            info!("Room planning complete - Failure - Room: {}", room_data.name);

                            //TODO: If failure occured, abort?
                            self.next_update = Some(game::time() + 100);
                        }
                    },
                    Ok(PlanSeedResult::Running(state)) => {
                        info!("Seeded room planning: {}", room_data.name);

                        self.planner_state = Some(state);
                    },
                    Err(_) => {
                        info!("Failed to seed room planning: {}", room_data.name);

                        self.next_update = Some(game::time() + 100);
                    }
                }
            }

            if let Some(mut planner_state) = self.planner_state.as_mut() {
                let bucket = game::cpu::bucket();
                let tick_limit = game::cpu::tick_limit();
                let current_cpu = game::cpu::get_used();
                let remaining_cpu = tick_limit - current_cpu;
                let max_cpu = (remaining_cpu * 0.25).min(tick_limit / 2.0);

                if bucket >= tick_limit * 2.0 && max_cpu >= 20.0 {
                    info!("Planning - Budget: {}", max_cpu);

                    let mut data_source = RoomDataPlannerDataSource::new(room_data.name, static_visibility_data);

                    let planner = Planner::new(crate::room::scoring::score_state);

                    match planner.evaluate(ALL_ROOT_NODES, &mut data_source, &mut planner_state, max_cpu) {
                        Ok(PlanEvaluationResult::Complete(plan)) => {
                            if plan.is_some() {
                                info!("Room planning complete - Success - Room: {}", room_data.name);
                                
                                self.plan = plan;
                            } else {
                                info!("Room planning complete - Failure - Room: {}", room_data.name);
    
                                //TODO: If failure occured, abort?
                                self.next_update = Some(game::time() + 20);
                            }

                            self.planner_state = None;
                        },
                        Ok(PlanEvaluationResult::Running()) => {
                        },
                        Err(_) => {
                            info!("Failed room planning: {}", room_data.name);

                            self.planner_state = None;
                        }
                    }
                }
            }
        }
        
        if crate::features::construction::visualize() {
            if  let Some(visualizer) = &mut runtime_data.visualizer {
                let room_visualizer = visualizer.get_room(room_data.name);

                if let Some(planner_state) = &self.planner_state {
                    if crate::features::construction::visualize_planner() {
                        planner_state.visualize(room_visualizer);
                    }
                    
                    if crate::features::construction::visualize_planner_best() {
                        planner_state.visualize_best(room_visualizer);
                    }
                }

                if let Some(plan) = &self.plan {
                    if crate::features::construction::visualize_plan() {
                        plan.visualize(room_visualizer);
                    }
                }
            }
        }

        if should_update && crate::features::construction::execute() {
            if let Some(plan) = &self.plan {
                plan.execute(&room);

                //TODO: Finish when plan is complete?

                self.next_update = Some(game::time() + 50);
            }
        }

        Ok(MissionResult::Running)
    }
}
