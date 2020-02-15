use serde::*;
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::missionsystem::*;

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub enum MissionData {
    LocalSupply(super::localsupply::LocalSupplyMission),
    Upgrade(super::upgrade::UpgradeMission),
    LocalBuild(super::localbuild::LocalBuildMission),
    Tower(super::tower::TowerMission),
}

impl MissionData {
    pub fn as_mission(&mut self) -> &mut dyn Mission {
        match self {
            MissionData::LocalSupply(ref mut data) => data,
            MissionData::Upgrade(ref mut data) => data,
            MissionData::LocalBuild(ref mut data) => data,
            MissionData::Tower(ref mut data) => data,
        }
    }
}
