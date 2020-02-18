#[allow(unused_imports)]
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
#[allow(unused_imports)]
use crate::room::planner::*;

#[derive(Clone, ConvertSaveload)]
pub struct ConstructionMission {
    room_data: Entity,
    last_update: Option<u32>,
    plan: Option<Plan>,
}

impl ConstructionMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ConstructionMission::new(room_data);

        builder
            .with(MissionData::Construction(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> ConstructionMission {
        ConstructionMission {
            room_data,
            last_update: None,
            plan: None,
        }
    }
}

impl Mission for ConstructionMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("ConstructionMission");

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(room) = game::rooms::get(room_data.name) {
                if self.plan.is_none() && crate::features::construction::plan() {
                    let planner = Planner::new(&room);

                    self.plan = Some(planner.plan());
                }

                if let Some(plan) = &self.plan {
                    if crate::features::construction::visualize() {
                        plan.visualize(&room);
                    }

                    let should_execute = crate::features::construction::execute()
                        && self
                            .last_update
                            .map(|last_time| game::time() - last_time > 500)
                            .unwrap_or(true);

                    if should_execute {
                        plan.execute(&room);

                        self.last_update = Some(game::time());
                    }
                }

                return MissionResult::Running;
            }
        }

        MissionResult::Failure
    }
}
