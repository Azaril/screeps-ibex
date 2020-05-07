use super::data::*;
use super::missionsystem::*;
use crate::ownership::*;
use crate::serialize::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct DefendMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
    home_room_data: Entity
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DefendMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = DefendMission::new(owner, room_data, home_room_data);

        builder.with(MissionData::Defend(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity, home_room_data: Entity) -> DefendMission {
        DefendMission {
            owner: owner.into(),
            room_data,
            home_room_data
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for DefendMission {
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
        format!("Defend")
    }

    fn pre_run_mission(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        Ok(())
    }

    fn run_mission(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        Ok(MissionResult::Running)
    }
}
