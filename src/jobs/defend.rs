use super::jobsystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::*;
use crate::findnearest::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct DefendJob {
    //defend_target: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DefendJob {
    pub fn new(_target_room_data: Entity) -> DefendJob {
        DefendJob {
            //defend_target: target_room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for DefendJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Defend - {}", name), None);
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        if let Some(room) = creep.room() {
            let hostile_creeps = room.find(find::HOSTILE_CREEPS);

            let mut nearest_creep = hostile_creeps.iter().find(|target_creep| creep.pos().get_range_to(&target_creep.pos()) <= 3).cloned();
            if nearest_creep.is_none() {
                nearest_creep = hostile_creeps.into_iter().find_nearest_from(creep.pos(), PathFinderHelpers::same_room_ignore_creeps_range_3);
            }

            if let Some(target_creep) = nearest_creep {
                let range = creep.pos().get_range_to(&target_creep.pos());

                if range <= 3 {
                    creep.ranged_attack(&target_creep);
                } 
                
                if range > 2 {
                    creep.move_to(&target_creep);
                }
            }
        }
    }
}
