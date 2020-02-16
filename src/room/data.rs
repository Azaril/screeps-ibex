use crate::serialize::EntityVec;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::remoteobjectid::*;

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub struct RoomData {
    pub name: RoomName,
    visible: bool,
    has_been_visible: bool,
    pub missions: EntityVec,
    cached_visibility_data: Option<RoomCachedVisibilityData>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomCachedVisibilityData {
    #[serde(default)]
    sources: Vec<RemoteObjectId<Source>>,
}

impl RoomCachedVisibilityData {
    pub fn new(sources: Vec<RemoteObjectId<Source>>) -> RoomCachedVisibilityData {
        RoomCachedVisibilityData {
            sources
        }
    }

    pub fn sources(&self) -> &Vec<RemoteObjectId<Source>> {
        &self.sources
    }
}

impl RoomData {
    pub fn new(room_name: RoomName) -> RoomData {
        RoomData {
            name: room_name,
            visible: false,
            has_been_visible: false,
            missions: EntityVec::new(),
            cached_visibility_data: None,
        }
    }

    pub fn set_visible(&mut self, visibility: bool) {
        self.visible = visibility;
    }

    pub fn set_visibility_data(&mut self, data: RoomCachedVisibilityData) {
        self.cached_visibility_data = Some(data);
    }

    pub fn get_visibility_data(&self) -> &Option<RoomCachedVisibilityData> {
        &self.cached_visibility_data
    }
}
