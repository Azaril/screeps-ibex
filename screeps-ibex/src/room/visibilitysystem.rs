use super::data::*;
use crate::entitymappingsystem::*;
use crate::missions::data::*;
use crate::missions::scout::*;
use crate::serialize::*;
use bitflags::*;
use log::*;
use screeps::*;
use screeps_rover::can_traverse_between_rooms;
use specs::prelude::*;
use specs::saveload::*;
use std::collections::HashMap;

pub const VISIBILITY_PRIORITY_CRITICAL: f32 = 100.0;
pub const VISIBILITY_PRIORITY_HIGH: f32 = 75.0;
pub const VISIBILITY_PRIORITY_MEDIUM: f32 = 50.0;
pub const VISIBILITY_PRIORITY_LOW: f32 = 25.0;
pub const VISIBILITY_PRIORITY_NONE: f32 = 0.0;

bitflags! {
    pub struct VisibilityRequestFlags: u8 {
        const UNSET = 0;

        const OBSERVE = 1u8;
        const SCOUT = 1u8 << 1;

        const ALL = Self::OBSERVE.bits | Self::SCOUT.bits;
    }
}

pub struct VisibilityRequest {
    room_name: RoomName,
    priority: f32,
    allowed_types: VisibilityRequestFlags,
}

impl VisibilityRequest {
    pub fn new(room_name: RoomName, priority: f32, allowed_types: VisibilityRequestFlags) -> VisibilityRequest {
        VisibilityRequest {
            room_name,
            priority,
            allowed_types,
        }
    }

    fn combine_with(&mut self, other: &VisibilityRequest) {
        self.priority = self.priority.max(other.priority);
        self.allowed_types |= other.allowed_types;
    }
}

#[derive(Default)]
pub struct VisibilityQueue {
    pub requests: HashMap<RoomName, VisibilityRequest>,
}

impl VisibilityQueue {
    pub fn request(&mut self, visibility_request: VisibilityRequest) {
        self.requests
            .entry(visibility_request.room_name)
            .and_modify(|e| e.combine_with(&visibility_request))
            .or_insert(visibility_request);
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
    fn process_requests<'a, 'b, T>(system_data: &mut VisibilityQueueSystemData<'a>, requests: T)
    where
        T: Iterator<Item = &'b VisibilityRequest>,
    {
        //
        // Get all unknown rooms and the last time they were visible.
        //

        let mut unknown_rooms = requests
            .map(
                |VisibilityRequest {
                     room_name,
                     priority,
                     allowed_types,
                 }| {
                    let last_visible = system_data
                        .mapping
                        .get_room(room_name)
                        .and_then(|entity| system_data.room_data.get(entity))
                        .and_then(|r| r.get_dynamic_visibility_data())
                        .map(|v| v.last_updated())
                        .unwrap_or(0);

                    (room_name, priority, allowed_types, last_visible)
                },
            )
            .collect::<Vec<_>>();

        if !unknown_rooms.is_empty() {
            unknown_rooms.sort_by(|(_, priority_a, _, last_visible_a), (_, priority_b, _, last_visible_b)| {
                priority_a
                    .partial_cmp(priority_b)
                    .unwrap()
                    .reverse()
                    .then_with(|| last_visible_a.cmp(last_visible_b))
            });

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

                    let max_level = structures.controllers().iter().map(|c| c.level()).max()?;

                    let observers = structures.observers().iter().cloned().collect::<Vec<_>>();

                    Some((entity, room_data.name, max_level, observers))
                })
                .collect::<Vec<_>>();

            //
            // Process requests in priority order.
            //

            for (unknown_room_name, priority, allowed_types, last_visible) in unknown_rooms.iter() {
                if allowed_types.contains(VisibilityRequestFlags::OBSERVE) {
                    let observer = home_room_data
                        .iter_mut()
                        .filter(|(_, _, _, observers)| !observers.is_empty())
                        .map(|(entity, home_room_name, _, observers)| {
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
                            ReturnCode::Ok => {}
                            err => info!("Failed to observe: {:?}", err),
                        }

                        continue;
                    }
                }

                fn get_room_cost(to_room_name: RoomName, from_room_name: RoomName) -> f64 {
                    if !can_traverse_between_rooms(from_room_name, to_room_name) {
                        return f64::INFINITY;
                    }
            
                    1.0
                }

                //
                // Use scout mission after a short period of time.
                //

                if allowed_types.contains(VisibilityRequestFlags::SCOUT) {
                    if game::time() - *last_visible >= 20 {
                        if let Some(room_entity) = system_data.mapping.get_room(&unknown_room_name) {
                            if let Some(room_data) = system_data.room_data.get_mut(room_entity) {
                                const MAX_ROOM_DISTANCE: u32 = 6;

                                let home_room_entities: Vec<_> = home_room_data.iter().map(|(entity, home_room_name, max_level, _)| {
                                    let delta = room_data.name - *home_room_name;
                                    let range = delta.0.abs() as u32 + delta.1.abs() as u32;

                                    (entity, home_room_name, max_level, range)
                                })
                                    .filter(|(_, _, max_level, _)| **max_level >= 2)
                                    .filter(|(_, _, _, range)| *range <= MAX_ROOM_DISTANCE)
                                    .filter(|(_, home_room_name, _, _)| {
                                        let options = map::FindRouteOptions::new()
                                            .room_callback(|to_room_name, from_room_name| {
                                                if !can_traverse_between_rooms(from_room_name, to_room_name) {
                                                    return f64::INFINITY;
                                                }
                                                
                                                //TODO: Need to include hostile rooms.
                                        
                                                1.0
                                            });
                            
                                        if let Ok(room_path) = game::map::find_route(room_data.name, **home_room_name, Some(options)) {
                                            if room_path.len() as u32 <= MAX_ROOM_DISTANCE {
                                                return true;
                                            }
                                        }

                                        false
                                    })
                                    .map(|(entity, _, _, _)| *entity)
                                    .collect();
                                    

                                let mission_data_storage = &system_data.mission_data;

                                let updated_scout_mission = room_data.get_missions().iter().any(|mission_entity| {
                                    if let Some(mut scout_mission) =
                                        mission_data_storage.get(*mission_entity).as_mission_type_mut::<ScoutMission>()
                                    {
                                        let max_priority = scout_mission.get_priority().max(**priority);

                                        scout_mission.set_priority(max_priority);
                                        scout_mission.set_home_rooms(&home_room_entities);

                                        true
                                    } else {
                                        false
                                    }
                                });

                                if !home_room_entities.is_empty() && !updated_scout_mission {                                
                                    //
                                    // Spawn a new mission to fill the scout role if missing.
                                    //

                                    info!("Starting scout mission for room: {}", room_data.name);

                                    let mission_entity = ScoutMission::build(
                                        system_data.updater.create_entity(&system_data.entities),
                                        None,
                                        room_entity,
                                        &home_room_entities,
                                        **priority,
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
        let existing_rooms = (&data.entities, &data.room_data)
            .join()
            .map(|(_, room_data)| room_data.name)
            .collect::<std::collections::HashSet<RoomName>>();

        let requests = std::mem::replace(&mut data.visibility_queue.requests, HashMap::new());

        let missing_rooms = requests.keys().filter(|name| !existing_rooms.contains(name));

        for room_name in missing_rooms {
            info!("Creating room data for room: {}", room_name);

            data.updater
                .create_entity(&data.entities)
                .marked::<SerializeMarker>()
                .with(RoomData::new(*room_name))
                .build();
        }

        Self::process_requests(&mut data, requests.values());
    }
}
