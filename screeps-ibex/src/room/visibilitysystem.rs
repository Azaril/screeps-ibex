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

    fn clear(&mut self) {
        self.requests.clear();
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
    fn process_requests<'a>(system_data: &mut VisibilityQueueSystemData<'a>, rooms: &HashMap<RoomName, f32>) {
        if !rooms.is_empty() {
            //
            // Get all rooms and observers can that service requests.
            //

            let mut home_room_data = (&system_data.entities, &system_data.room_data)
                .join()
                .filter_map(|(entity, room_data)| {
                    let dynamic_visibility_data = room_data.get_dynamic_visibility_data()?;
                    
                    if !dynamic_visibility_data.owner().mine() {
                        return None;
                    }

                    let structures = room_data.get_structures()?;
                    
                    if structures.spawns().is_empty() {
                        return None;
                    }

                    let observers = structures.observers().iter().cloned().collect::<Vec<_>>();

                    Some((entity, room_data.name, observers))
                })
                .collect::<Vec<_>>();

            //
            // Get all unknown rooms and the last time they were visible.
            //

            let mut unknown_rooms = rooms
                .iter()
                .map(|(room_name, priority)| {
                    let last_visible =  system_data
                        .mapping
                        .get_room(room_name)
                        .and_then(|entity| system_data.room_data.get(entity))
                        .and_then(|r| r.get_dynamic_visibility_data())
                        .map(|v| v.last_updated())
                        .unwrap_or(0);

                    (room_name, priority, last_visible)                            
                })
                .collect::<Vec<_>>();

            unknown_rooms
                .sort_by(|(_, priority_a, last_visible_a), (_, priority_b, last_visible_b)| {
                    priority_a
                        .partial_cmp(priority_b)
                        .unwrap()
                        .reverse()
                        .then_with(|| last_visible_a.cmp(last_visible_b))
                });

            //
            // Process requests in priority order.
            //
            
            for (unknown_room_name, _, last_visible) in unknown_rooms.iter() {
                let observer = home_room_data
                    .iter_mut()
                    .filter(|(_, _, observers)| !observers.is_empty())
                    .map(|(entity, home_room_name, observers)| {
                        let delta = **unknown_room_name - *home_room_name;
                        let range = delta.0.abs().max(delta.1.abs()) as u32;

                        (entity, home_room_name, observers, range)
                    })
                    //TODO: Handle observer infinite range boost
                    .filter(|(_, _, _, range)| *range <= OBSERVER_RANGE)
                    .min_by_key(|(_, _, _, range)| *range)
                    .and_then(|(_, _, observers, _)| observers.pop());

                //
                // Use observer on room if available.
                //

                if let Some(observer) = observer {
                    match observer.observe_room(**unknown_room_name) {
                        ReturnCode::Ok => info!("Observering: {}", **unknown_room_name),
                        err => info!("Failed to observe: {:?}", err)
                    }

                    continue;
                }

                //
                // Use scout mission after a short period of time.
                //

                if *last_visible >= 20 {
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
                                //TODO: Use path distance instead of linear distance.
                                let nearest_room_entity = home_room_data
                                    .iter()
                                    .min_by_key(|(_, home_room_name, _)| {
                                        let delta = room_data.name - *home_room_name;

                                        delta.0.abs().max(delta.1.abs())
                                    })
                                    .map(|(entity, _, _)| entity);

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
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for VisibilityQueueSystem {
    type SystemData = VisibilityQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut room_priorities: HashMap<RoomName, f32> = HashMap::new();

        for request in &data.visibility_queue.requests {
            room_priorities
                .entry(request.room_name)
                .and_modify(|e| *e = e.max(request.priority))
                .or_insert(request.priority);
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

        Self::process_requests(&mut data, &room_priorities);

        data.visibility_queue.clear();
    }
}
