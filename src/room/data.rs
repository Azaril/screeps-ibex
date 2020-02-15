use crate::serialize::EntityVec;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, Copy, Debug, Component, ConvertSaveload)]
pub struct RoomOwnerData {
    pub owner: RoomName,
}

impl RoomOwnerData {
    pub fn new(name: RoomName) -> RoomOwnerData {
        RoomOwnerData { owner: name }
    }
}

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub struct RoomData {
    pub missions: EntityVec,
}

impl RoomData {
    pub fn new() -> RoomData {
        RoomData {
            missions: EntityVec::new(),
        }
    }
}
