use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use specs_derive::*;
use serde::*;

use super::missionsystem::*;

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub enum MissionData {
    BasicHarvest(super::basicharvest::BasicHarvestMission),
    ComplexHarvest(super::complexharvest::ComplexHarvestMission),
    Upgrade(super::upgrade::UpgradeMission)
}

impl MissionData
{
    pub fn as_mission(&mut self) -> &mut dyn Mission
    {
        match self {
            MissionData::BasicHarvest(ref mut data) => data,
            MissionData::ComplexHarvest(ref mut data) => data,
            MissionData::Upgrade(ref mut data) => data
        }
    }
}