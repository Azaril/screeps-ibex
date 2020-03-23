use super::data::*;
use super::missionsystem::*;
use crate::room::planner::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::serialize::*;
use log::*;

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

        let should_update = self.next_update.map(|next_time| game::time() >= next_time).unwrap_or(true);

        if should_update && crate::features::construction::plan() {
            if self.plan.is_none() && self.planner_state.is_none() {
                info!("Starting room planning: {}", room_data.name);

                let planner = Planner::new(&room);

                match planner.seed() {
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
                let planner = Planner::new(&room);

                let bucket = game::cpu::bucket();
                let ticket_limit = game::cpu::tick_limit();
                let current_cpu = game::cpu::get_used();
                let remaining_cpu = ticket_limit - current_cpu;
                let max_cpu = (remaining_cpu * 0.2).min(50.0);

                if bucket >= ticket_limit * 2.0 && max_cpu >= 20.0 {
                    info!("Planning - Budget: {}", max_cpu);

                    match planner.evaluate(&mut planner_state, max_cpu) {
                        Ok(PlanEvaluationResult::Complete(plan)) => {
                            if plan.is_some() {
                                info!("Room planning complete - Success - Room: {}", room_data.name);
                                
                                self.plan = plan;
                            } else {
                                info!("Room planning complete - Failure - Room: {}", room_data.name);
    
                                //TODO: If failure occured, abort?
                                self.next_update = Some(game::time() + 100);
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
