use serde::*;
use screeps::*;
use specs::*;        
use specs_derive::*;                                                                                               

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Component)]
pub struct RoomOwnerData {
    pub owner: RoomName
}

impl RoomOwnerData {
    pub fn new(name: &RoomName) -> RoomOwnerData {
        RoomOwnerData {
            owner: *name
        }                           
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Component)]
pub struct RoomData {
    pub missions: Vec<::missions::data::MissionMarker>
}

impl RoomData {
    pub fn new() -> RoomData {
        RoomData {
            missions: vec!()
        }                           
    }
}