use crate::entitymappingsystem::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use screeps::*;
use screeps_rover::*;
use specs::*;
use std::collections::HashMap;

pub struct CandidateRoomData {
    room_data_entity: Entity,
    viable: bool,
    can_expand: bool,
}

impl CandidateRoomData {
    pub fn new(room_data_entity: Entity, viable: bool, can_expand: bool) -> CandidateRoomData {
        CandidateRoomData {
            room_data_entity,
            viable,
            can_expand,
        }
    }
}

pub struct CandidateRoom {
    room_data_entity: Entity,
    home_room_data_entity: Entity,
    distance: u32,
}

impl CandidateRoom {
    pub fn room_data_entity(&self) -> Entity {
        self.room_data_entity
    }

    pub fn home_room_data_entity(&self) -> Entity {
        self.home_room_data_entity
    }

    pub fn distance(&self) -> u32 {
        self.distance
    }
}

pub struct UnknownRoom {
    room_name: RoomName,
    home_room_data_entity: Entity,
    distance: u32,
}

impl UnknownRoom {
    pub fn room_name(&self) -> RoomName {
        self.room_name
    }

    pub fn home_room_data_entity(&self) -> Entity {
        self.home_room_data_entity
    }

    pub fn distance(&self) -> u32 {
        self.distance
    }
}

pub struct GatherRoomData {
    candidate_rooms: Vec<CandidateRoom>,
    unknown_rooms: Vec<UnknownRoom>,
}

impl GatherRoomData {
    pub fn candidate_rooms(&self) -> &Vec<CandidateRoom> {
        &self.candidate_rooms
    }

    pub fn unknown_rooms(&self) -> &Vec<UnknownRoom> {
        &self.unknown_rooms
    }
}

struct VisitedRoomData {
    room_data_entity: Entity,
    home_room_data_entity: Entity,
    distance: u32,
    viable: bool,
    can_expand: bool,
}

pub struct GatherSystemData<'a, 'b> {
    pub entities: &'b Entities<'a>,
    pub mapping: &'b Read<'a, EntityMappingData>,
    pub room_data: &'b mut WriteStorage<'a, RoomData>,
    pub room_plan_data: &'b ReadStorage<'a, RoomPlanData>,
}

pub fn gather_candidate_rooms<F>(system_data: &GatherSystemData, min_rcl: u32, max_distance: u32, candidate_generator: F) -> GatherRoomData
where
    F: Fn(&GatherSystemData, RoomName) -> Option<CandidateRoomData>,
{
    let mut unknown_rooms = HashMap::new();

    let mut visited_rooms: HashMap<RoomName, VisitedRoomData> = HashMap::new();
    let mut expansion_rooms: HashMap<RoomName, Entity> = HashMap::new();

    for (entity, room_data) in (&*system_data.entities, &*system_data.room_data).join() {
        if let Some(room) = game::rooms::get(room_data.name) {
            let seed_room = room
                .controller()
                .map(|controller| controller.my() && controller.level() >= min_rcl)
                .unwrap_or(false);

            if seed_room {
                let visited_room = VisitedRoomData {
                    room_data_entity: entity,
                    home_room_data_entity: entity,
                    distance: 0,
                    viable: false,
                    can_expand: true,
                };

                if visited_room.can_expand {
                    let room_exits = game::map::describe_exits(room_data.name);

                    let source_room_status = game::map::get_room_status(room_data.name);

                    for expansion_room in room_exits.values() {
                        let expansion_room_status = game::map::get_room_status(*expansion_room);

                        if can_traverse_between_room_status(&source_room_status, &expansion_room_status) {
                            expansion_rooms.insert(*expansion_room, entity);
                        }
                    }
                }

                visited_rooms.insert(room_data.name, visited_room);
            }
        }
    }

    let mut distance = 1;

    while !expansion_rooms.is_empty() && distance <= max_distance {
        let next_rooms: HashMap<RoomName, Entity> = std::mem::replace(&mut expansion_rooms, HashMap::new());

        for (source_room_name, home_room_entity) in next_rooms.iter() {
            if !visited_rooms.contains_key(source_room_name) {
                let candiate_room_data = (candidate_generator)(system_data, *source_room_name);

                if let Some(candidate_room_data) = candiate_room_data {
                    let visited_room = VisitedRoomData {
                        room_data_entity: candidate_room_data.room_data_entity,
                        home_room_data_entity: *home_room_entity,
                        distance,
                        viable: candidate_room_data.viable,
                        can_expand: candidate_room_data.can_expand,
                    };

                    if visited_room.can_expand {
                        let room_exits = game::map::describe_exits(*source_room_name);

                        let source_room_status = game::map::get_room_status(*source_room_name);

                        for expansion_room in room_exits.values() {
                            let expansion_room_status = game::map::get_room_status(*expansion_room);

                            if can_traverse_between_room_status(&source_room_status, &expansion_room_status) {
                                expansion_rooms.insert(*expansion_room, *home_room_entity);
                            }
                        }
                    }

                    visited_rooms.insert(*source_room_name, visited_room);
                } else {
                    unknown_rooms.insert(*source_room_name, (*home_room_entity, distance));
                }
            }
        }

        distance += 1;
    }

    let candidate_rooms = visited_rooms
        .values()
        .filter(|v| v.viable)
        .map(|v| CandidateRoom {
            room_data_entity: v.room_data_entity,
            home_room_data_entity: v.home_room_data_entity,
            distance: v.distance,
        })
        .collect();

    let returned_unknown_rooms = unknown_rooms
        .into_iter()
        .map(|(room_name, (home_room_data_entity, distance))| UnknownRoom {
            room_name,
            home_room_data_entity,
            distance,
        })
        .collect();

    GatherRoomData {
        candidate_rooms,
        unknown_rooms: returned_unknown_rooms,
    }
}
