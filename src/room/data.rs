use crate::remoteobjectid::*;
use crate::serialize::EntityVec;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomStaticVisibilityData {
    #[serde(default)]
    sources: Vec<RemoteObjectId<Source>>,
}

impl RoomStaticVisibilityData {
    pub fn sources(&self) -> &Vec<RemoteObjectId<Source>> {
        &self.sources
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomDynamicVisibilityData {
    #[serde(default)]
    update_tick: u32,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    reseration_owner: Option<String>,
    #[serde(default)]
    my: bool,
    #[serde(default)]
    friendly: bool,
    #[serde(default)]
    hostile: bool,
}

impl RoomDynamicVisibilityData {
    pub fn age(&self) -> u32 {
        game::time() - self.update_tick
    }

    pub fn visible(&self) -> bool {
        self.age() == 0
    }

    pub fn updated_within(&self, ticks: u32) -> bool {
        self.age() <= ticks
    }

    pub fn owner(&self) -> &Option<String> {
        &self.owner
    }

    pub fn reseration_owner(&self) -> &Option<String> {
        &self.reseration_owner
    }

    pub fn my(&self) -> bool {
        self.my
    }

    pub fn friendly(&self) -> bool {
        self.friendly
    }

    pub fn hostile(&self) -> bool {
        self.hostile
    }
}

#[derive(Clone, Component, ConvertSaveload)]
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
        let source_ids = room
            .find(find::SOURCES)
            .into_iter()
            .map(|s| s.remote_id())
            .collect();

        RoomStaticVisibilityData {
            sources: source_ids,
        }
    }

    fn create_dynamic_visibility_data(room: &Room) -> RoomDynamicVisibilityData {
        let controller = room.controller();

        let controller_owner_name = controller.as_ref().and_then(|c| c.owner_name());
        let controller_reservation_owner_name = controller
            .as_ref()
            .and_then(|c| c.reservation())
            .map(|r| r.username);
        let room_owner = controller_owner_name
            .clone()
            .or_else(|| controller_reservation_owner_name.clone());
        let my = room_owner
            .as_ref()
            .map(|name| name == crate::globals::user::name())
            .unwrap_or(false);

        //TODO: Friendly/hostile for now only include current user - in future could be other users.
        let friends = [crate::globals::user::name()];
        let friendly = room_owner
            .as_ref()
            .map(|name| friends.iter().any(|friend_name| name == friend_name))
            .unwrap_or(false);
        let hostile = room_owner
            .as_ref()
            .map(|name| !friends.iter().any(|friend_name| name == friend_name))
            .unwrap_or(false);

        RoomDynamicVisibilityData {
            update_tick: game::time(),
            owner: controller_owner_name,
            reseration_owner: controller_reservation_owner_name,
            my,
            friendly,
            hostile,
        }
    }

    pub fn get_static_visibility_data(&self) -> &Option<RoomStaticVisibilityData> {
        &self.static_visibility_data
    }

    pub fn get_dynamic_visibility_data(&self) -> &Option<RoomDynamicVisibilityData> {
        &self.dynamic_visibility_data
    }
}
