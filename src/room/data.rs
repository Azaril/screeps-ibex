use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};
use crate::serialize::EntityVec;                                                                                       

#[derive(Clone, Copy, Debug, Component, ConvertSaveload)]
pub struct RoomOwnerData {
    pub owner: RoomName
}

impl RoomOwnerData {
    pub fn new(name: RoomName) -> RoomOwnerData {
        RoomOwnerData {
            owner: name
        }                           
    }
}

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub struct RoomData {
    pub missions: EntityVec
}

impl RoomData {
    pub fn new() -> RoomData {
        RoomData {
            missions: EntityVec::new()
        }                           
    }
}