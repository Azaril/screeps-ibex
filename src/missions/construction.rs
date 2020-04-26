use super::data::*;
use super::missionsystem::*;
use crate::ownership::*;
use crate::room::roomplansystem::*;
use crate::serialize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct ConstructionMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ConstructionMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ConstructionMission::new(owner, room_data);

        builder.with(MissionData::Construction(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity) -> ConstructionMission {
        ConstructionMission {
            owner: owner.into(),
            room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ConstructionMission {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: OperationOrMissionEntity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _describe_data: &mut MissionDescribeData) -> String {
        "Construction".to_string()
    }

    fn run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
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
            system_data.room_plan_queue.request(RoomPlanRequest::new(room_data.name, 1.0));
        }

        Ok(MissionResult::Running)
    }
}
