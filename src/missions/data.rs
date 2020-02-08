use specs::*;
use specs::saveload::*;
use serde::*;

use super::missionsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum MissionData {
    Bootstrap(super::bootstrap::BootstrapMission)
}

impl Component for MissionData {
    type Storage = HashMapStorage<Self>;
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