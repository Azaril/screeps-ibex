use serde::*;
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::missionsystem::*;

#[derive(Clone, Component, ConvertSaveload)]
pub enum MissionData {
    LocalSupply(super::localsupply::LocalSupplyMission),
    Upgrade(super::upgrade::UpgradeMission),
    LocalBuild(super::localbuild::LocalBuildMission),
    Tower(super::tower::TowerMission),
    RemoteMine(super::remotemine::RemoteMineMission),
    Scout(super::scout::ScoutMission),
    Construction(super::construction::ConstructionMission),
    Reserve(super::reserve::ReserveMission),
    Claim(super::claim::ClaimMission),
    RemoteBuild(super::remotebuild::RemoteBuildMission),
}

impl MissionData {
    pub fn as_mission(&mut self) -> &mut dyn Mission {
        match self {
            MissionData::LocalSupply(ref mut data) => data,
            MissionData::Upgrade(ref mut data) => data,
            MissionData::LocalBuild(ref mut data) => data,
            MissionData::Tower(ref mut data) => data,
            MissionData::RemoteMine(ref mut data) => data,
            MissionData::Scout(ref mut data) => data,
            MissionData::Construction(ref mut data) => data,
            MissionData::Reserve(ref mut data) => data,
            MissionData::Claim(ref mut data) => data,
            MissionData::RemoteBuild(ref mut data) => data,
        }
    }
}
