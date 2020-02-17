use crate::serialize::EntityVec;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use crate::remoteobjectid::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomStaticVisibilityData {
    #[serde(default)]
    sources: Vec<RemoteObjectId<Source>>,
}

impl RoomStaticVisibilityData {
    pub fn sources(&self) -> &Vec<RemoteObjectId<Source>> {
        &self.sources
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomDynamicVisibilityData {
    update_tick: u32,
    #[serde(default)]
    hostile: bool
}

impl RoomDynamicVisibilityData {
    pub fn hostile(&self) -> bool {
        self.hostile
    }

    pub fn updated_within(&self, ticks: u32) -> bool {
        (game::time() - self.update_tick) <= ticks
    }
}

#[derive(Clone, Debug, Component, ConvertSaveload)]
pub struct RoomData {
    pub name: RoomName,
    visible: bool,
    has_been_visible: bool,
    pub missions: EntityVec,
    static_visibility_data: Option<RoomStaticVisibilityData>,
    dynamic_visibility_data: Option<RoomDynamicVisibilityData>,
}

impl RoomData {
    pub fn new(room_name: RoomName) -> RoomData {
        RoomData {
            name: room_name,
            visible: false,
            has_been_visible: false,
            missions: EntityVec::new(),
            static_visibility_data: None,
            dynamic_visibility_data: None,
        }
    }

    pub fn clear_visible(&mut self) {
        self.visible = false;
    }

    pub fn update(&mut self, room: &Room) {
        self.visible = true;
        self.has_been_visible = true;

        if self.static_visibility_data.is_none() {
            self.static_visibility_data = Some(Self::create_static_visibility_data(&room));
        }

        self.dynamic_visibility_data = Some(Self::create_dynamic_visibility_data(&room));
    }

    fn create_static_visibility_data(room: &Room) -> RoomStaticVisibilityData {
        let source_ids = room.find(find::SOURCES).into_iter().map(|s| s.remote_id()).collect();

        RoomStaticVisibilityData{
            sources: source_ids
        }
    }

    fn create_dynamic_visibility_data(room: &Room) -> RoomDynamicVisibilityData {
        RoomDynamicVisibilityData{
            update_tick: game::time(),
            hostile: Self::is_room_hostile(room)
        }
    }

    fn is_room_hostile(room: &Room) -> bool {
        if let Some(controller) = room.controller() {
            //TODO: Does reservation need to be checked?
            return controller.has_owner() && !controller.my();
        }

        false
    }

    pub fn get_static_visibility_data(&self) -> &Option<RoomStaticVisibilityData> {
        &self.static_visibility_data
    }

    pub fn get_dynamic_visibility_data(&self) -> &Option<RoomDynamicVisibilityData> {
        &self.dynamic_visibility_data
    }
}
