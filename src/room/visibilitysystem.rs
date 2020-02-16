use crossbeam_queue::SegQueue;
use screeps::*;
use specs::prelude::*;
use specs::saveload::*;
use std::collections::HashMap;
use super::data::*;

pub const VISIBILITY_PRIORITY_CRITICAL: f32 = 100.0;
pub const VISIBILITY_PRIORITY_HIGH: f32 = 75.0;
pub const VISIBILITY_PRIORITY_MEDIUM: f32 = 50.0;
pub const VISIBILITY_PRIORITY_LOW: f32 = 25.0;
pub const VISIBILITY_PRIORITY_NONE: f32 = 0.0;

pub struct VisibilityRequest {
    room_name: RoomName,
    priority: f32,
}

impl VisibilityRequest {
    pub fn new(room_name: RoomName, priority: f32) -> VisibilityRequest {
        VisibilityRequest {
            room_name,
            priority,
        }
    }
}

#[derive(Default)]
pub struct VisibilityQueue {
    pub requests: SegQueue<VisibilityRequest>,
}

impl VisibilityQueue {
    pub fn request(&self, visibility_request: VisibilityRequest) {
        self.requests.push(visibility_request);
    }
}

impl Drop for VisibilityQueue {
    fn drop(&mut self) {
        // TODO: remove as soon as leak is fixed in crossbeam
        while self.requests.pop().is_ok() {}
    }
}

#[derive(SystemData)]
pub struct VisibilityQueueSystemData<'a> {
    visibility_queue: Write<'a, VisibilityQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_datas: WriteStorage<'a, ::room::data::RoomData>,
}

pub struct VisibilityQueueSystem;

impl<'a> System<'a> for VisibilityQueueSystem {
    type SystemData = VisibilityQueueSystemData<'a>;

    fn run(&mut self, data: Self::SystemData) {
        scope_timing!("VisibilityQueueSystem");

        let mut room_priorities: HashMap<RoomName, f32> = HashMap::new();

        while let Ok(request) = data.visibility_queue.requests.pop() {
            if let Some(current_priority) = room_priorities.get_mut(&request.room_name) {
                let highest_priority = current_priority.max(request.priority);
                *current_priority = highest_priority;
            } else {
                room_priorities.insert(request.room_name, request.priority);
            }
        }

        let existing_rooms = (&data.entities, &data.room_datas)
            .join()
            .map(|(_, room_data)| room_data.name)
            .collect::<std::collections::HashSet<RoomName>>();

        let missing_rooms = room_priorities.keys()
            .filter(|name| !existing_rooms.contains(name));

        for room_name in missing_rooms {
            info!("Creating room data for room: {}", room_name);

            data.updater
                .create_entity(&data.entities)
                .marked::<::serialize::SerializeMarker>()
                .with(RoomData::new(*room_name))
                .build();
        }

        //TODO: Use observer to look at room.
    }
}
