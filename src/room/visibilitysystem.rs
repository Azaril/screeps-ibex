use super::data::*;
use crate::entitymappingsystem::*;
use crate::missions::data::*;
use crate::missions::scout::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use specs::prelude::*;
use specs::saveload::*;
use std::collections::HashMap;

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
        VisibilityRequest { room_name, priority }
    }
}

#[derive(Default)]
pub struct VisibilityQueue {
    pub requests: Vec<VisibilityRequest>,
}

impl VisibilityQueue {
    pub fn request(&mut self, visibility_request: VisibilityRequest) {
        self.requests.push(visibility_request);
    }
}

#[derive(SystemData)]
pub struct VisibilityQueueSystemData<'a> {
    visibility_queue: Write<'a, VisibilityQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    mission_data: WriteStorage<'a, MissionData>,
    mapping: Read<'a, EntityMappingData>,
}

pub struct VisibilityQueueSystem;

impl VisibilityQueueSystem {
    fn spawn_scout_missions<'a>(system_data: &mut VisibilityQueueSystemData<'a>, rooms: &HashMap<RoomName, f32>) {
        //TODO: Look at priority etc.

        if !rooms.is_empty() {
            let home_rooms = (&system_data.entities, &system_data.room_data)
                .join()
                .filter(|(_, room_data)| {
                    game::rooms::get(room_data.name)
                        .and_then(|r| r.controller())
                        .map(|c| c.my())
                        .unwrap_or(false)
                })
                .map(|(entity, room_data)| (entity, room_data.name))
                .collect::<std::collections::HashSet<_>>();

            for unknown_room_name in rooms.keys() {
                if let Some(room_entity) = system_data.mapping.get_room(&unknown_room_name) {
                    if let Some(room_data) = system_data.room_data.get_mut(room_entity) {
                        let mission_data_storage = &system_data.mission_data;

                        let has_scout_mission =
                            room_data
                                .get_missions()
                                .iter()
                                .any(|mission_entity| match mission_data_storage.get(*mission_entity) {
                                    Some(MissionData::Scout(_)) => true,
                                    _ => false,
                                });

                        //
                        // Spawn a new mission to fill the scout role if missing.
                        //

                        if !has_scout_mission {
                            let nearest_room_entity = home_rooms
                                .iter()
                                .min_by_key(|(_, home_room_name)| {
                                    let delta = room_data.name - *home_room_name;

                                    delta.0.abs().max(delta.1.abs())
                                })
                                .map(|(entity, _)| entity);

                            if let Some(nearest_room_entity) = nearest_room_entity {
                                let mission_entity = ScoutMission::build(
                                    system_data.updater.create_entity(&system_data.entities),
                                    None,
                                    room_entity,
                                    *nearest_room_entity,
                                )
                                .build();

                                room_data.add_mission(mission_entity);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for VisibilityQueueSystem {
    type SystemData = VisibilityQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut room_priorities: HashMap<RoomName, f32> = HashMap::new();

        for request in &data.visibility_queue.requests {
            if let Some(current_priority) = room_priorities.get_mut(&request.room_name) {
                let highest_priority = current_priority.max(request.priority);
                *current_priority = highest_priority;
            } else {
                room_priorities.insert(request.room_name, request.priority);
            }
        }

        let existing_rooms = (&data.entities, &data.room_data)
            .join()
            .map(|(_, room_data)| room_data.name)
            .collect::<std::collections::HashSet<RoomName>>();

        let missing_rooms = room_priorities.keys().filter(|name| !existing_rooms.contains(name));

        for room_name in missing_rooms {
            info!("Creating room data for room: {}", room_name);

            data.updater
                .create_entity(&data.entities)
                .marked::<SerializeMarker>()
                .with(RoomData::new(*room_name))
                .build();
        }

        //TODO: Use observer to look at room.

        Self::spawn_scout_missions(&mut data, &room_priorities);
    }
}
