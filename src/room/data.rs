use serde::*;
use screeps::*;
use specs::*;                                                                                                       

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
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

impl Component for RoomOwnerData {
    type Storage = HashMapStorage<Self>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

impl Component for RoomData {
    type Storage = HashMapStorage<Self>;
}