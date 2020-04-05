use super::data::*;
use super::missionsystem::*;
use crate::room::roomplansystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::serialize::*;

#[derive(Clone, ConvertSaveload)]
pub struct ConstructionMission {
    room_data: Entity
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
            room_data
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

        let request_plan = if let Some(room_plan_data) = system_data.room_plan_data.get(self.room_data) {
            if (game::time() % 50 == 0) && crate::features::construction::execute() {
                room_plan_data.plan().execute(&room);

                //TODO: Finish when plan is complete?
            }

            crate::features::construction::force_plan()
        } else {
            true
        };

        if request_plan {
            runtime_data.room_plan_queue.request(RoomPlanRequest::new(room_data.name, 1.0));
        }

        Ok(MissionResult::Running)
    }
}
