use super::missionsystem::*;
use serde::*;
use specs::saveload::*;
use specs::*;

#[derive(Component, ConvertSaveload)]
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
    Haul(super::haul::HaulMission),
    Terminal(super::terminal::TerminalMission),
    MiningOutpost(super::miningoutpost::MiningOutpostMission),
    Raid(super::raid::RaidMission),
    Dismantle(super::dismantle::DismantleMission),
    Colony(super::colony::ColonyMission),
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
            MissionData::Haul(ref mut data) => data,
            MissionData::Terminal(ref mut data) => data,
            MissionData::MiningOutpost(ref mut data) => data,
            MissionData::Raid(ref mut data) => data,
            MissionData::Dismantle(ref mut data) => data,
            MissionData::Colony(ref mut data) => data,
        }
    }
}
