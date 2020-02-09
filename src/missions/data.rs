use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use specs_derive::*;
use serde::*;

use super::missionsystem::*;

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub enum MissionData {
    Bootstrap(super::bootstrap::BootstrapMission)
}

impl MissionData
{
    pub fn as_mission(&mut self) -> &mut dyn Mission
    {
        match self {
            MissionData::Bootstrap(ref mut data) => data
        }
    }
}

pub struct MissionMarkerTag;

pub type MissionMarker = SimpleMarker<MissionMarkerTag>;

pub type MissionMarkerAllocator = SimpleMarkerAllocator<MissionMarkerTag>;