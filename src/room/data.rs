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
    controller: Option<RemoteObjectId<StructureController>>,
    #[serde(default)]
    sources: Vec<RemoteObjectId<Source>>,
    #[serde(default)]
    minerals: Vec<RemoteObjectId<Mineral>>,
}

impl RoomStaticVisibilityData {
    pub fn controller(&self) -> Option<&RemoteObjectId<StructureController>> {
        self.controller.as_ref()
    }

    pub fn sources(&self) -> &Vec<RemoteObjectId<Source>> {
        &self.sources
    }

    pub fn minerals(&self) -> &Vec<RemoteObjectId<Mineral>> {
        &self.minerals
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum RoomDisposition {
    Neutral,
    Mine,
    Friendly(String),
    Hostile(String),
}

impl RoomDisposition {
    pub fn name(&self) -> Option<&str> {
        match self {
            RoomDisposition::Neutral => None,
            RoomDisposition::Mine => Some(crate::globals::user::name()),
            RoomDisposition::Friendly(name) => Some(name),
            RoomDisposition::Hostile(name) => Some(name),
        }
    }

    pub fn neutral(&self) -> bool {
        if let RoomDisposition::Neutral = self {
            true
        } else {
            false
        }
    }

    pub fn mine(&self) -> bool {
        if let RoomDisposition::Mine = self {
            true
        } else {
            false
        }
    }

    pub fn hostile(&self) -> bool {
        if let RoomDisposition::Hostile(_) = self {
            true
        } else {
            false
        }
    }

    pub fn friendly(&self) -> bool {
        if let RoomDisposition::Friendly(_) = self {
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomDynamicVisibilityData {
    #[serde(default)]
    update_tick: u32,
    owner: RoomDisposition,
    reservation: RoomDisposition,
    #[serde(default)]
    source_keeper: bool,
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

    pub fn owner(&self) -> &RoomDisposition {
        &self.owner
    }

    pub fn reservation(&self) -> &RoomDisposition {
        &self.reservation
    }

    pub fn source_keeper(&self) -> bool {
        self.source_keeper
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
        let controller_id = room.controller().map(|c| c.remote_id());
        let source_ids = room.find(find::SOURCES).into_iter().map(|s| s.remote_id()).collect();
        let mineral_ids = room.find(find::MINERALS).into_iter().map(|s| s.remote_id()).collect();
        
        RoomStaticVisibilityData {
            controller: controller_id,
            sources: source_ids,
            minerals: mineral_ids
        }
    }

    fn name_option_to_disposition(name: Option<String>) -> RoomDisposition {
        let friends: &[String] = &[];

        name.map(|name| {
            if name == crate::globals::user::name() {
                RoomDisposition::Mine
            } else if friends.iter().any(|friend_name| &name == friend_name) {
                RoomDisposition::Friendly(name)
            } else {
                RoomDisposition::Hostile(name)
            }
        })
        .unwrap_or_else(|| RoomDisposition::Neutral)
    }

    fn create_dynamic_visibility_data(room: &Room) -> RoomDynamicVisibilityData {
        let controller = room.controller();

        let controller_owner_name = controller.as_ref().and_then(|c| c.owner_name());
        let controller_owner_disposition = Self::name_option_to_disposition(controller_owner_name);

        let controller_reservation_name = controller.as_ref().and_then(|c| c.reservation()).map(|r| r.username);
        let controller_reservation_disposition = Self::name_option_to_disposition(controller_reservation_name);

        //TODO: This is expensive - can really just be calculated for room number. Not possible to calculate given x/y coord is private.
        let source_keeper = room.find(find::HOSTILE_STRUCTURES).into_iter().any(|s| {
            if let Structure::KeeperLair(_) = s.as_structure() {
                true
            } else {
                false
            }
        });

        RoomDynamicVisibilityData {
            update_tick: game::time(),
            owner: controller_owner_disposition,
            reservation: controller_reservation_disposition,
            source_keeper,
        }
    }

    pub fn get_static_visibility_data(&self) -> Option<&RoomStaticVisibilityData> {
        self.static_visibility_data.as_ref()
    }

    pub fn get_dynamic_visibility_data(&self) -> Option<&RoomDynamicVisibilityData> {
        self.dynamic_visibility_data.as_ref()
    }
}
