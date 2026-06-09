//! Per-tick cache for `Game.map.getRoomStatus` so each room is queried at most once per tick.

use screeps::game::map::{get_room_status, RoomStatus};
use screeps::RoomName;
use specs::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// Per-tick cache: room name (string) -> status. Cleared at start of each tick.
/// Uses RefCell so that `get_or_insert` can be called with `&self` (e.g. from rover's get_room_cost).
#[derive(Default)]
pub struct RoomStatusCache {
    map: RefCell<HashMap<String, Option<RoomStatus>>>,
}

impl RoomStatusCache {
    pub fn new() -> Self {
        RoomStatusCache {
            map: RefCell::new(HashMap::new()),
        }
    }

    /// Clear the cache. Call once per tick before any system uses it.
    pub fn clear(&mut self) {
        self.map.get_mut().clear();
    }

    /// Get cached status for `room`, or call `get_room_status` and cache the result.
    /// Safe to call with `&self` (used from movement provider).
    pub fn get_or_insert(&self, room: RoomName) -> Option<RoomStatus> {
        let key = format!("{}", room);
        {
            let map = self.map.borrow();
            if let Some(cached) = map.get(&key) {
                return *cached;
            }
        }
        let status = get_room_status(room).map(|r| r.status());
        self.map.borrow_mut().insert(key, status);
        status
    }
}

#[derive(SystemData)]
pub struct RoomStatusCacheClearSystemData<'a> {
    room_status_cache: WriteExpect<'a, RoomStatusCache>,
}

/// Clears the room status cache at the start of each tick so lookups are fresh.
pub struct RoomStatusCacheClearSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RoomStatusCacheClearSystem {
    type SystemData = RoomStatusCacheClearSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.room_status_cache.clear();
    }
}
