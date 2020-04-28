use crate::creep::*;
use screeps::*;
use serde::*;
use specs::*;
use specs::prelude::*;
use std::collections::HashMap;
use crate::room::data::*;
use crate::entitymappingsystem::*;
use std::collections::HashSet;
use std::str::FromStr;
use crate::room::utility::*;
use screeps::pathfinder::*;

struct RoomOptions {
    allow_hostile: bool,
}

struct MovementRequest {
    destination: RoomPosition,
    range: u32,
    room_options: RoomOptions,
}

impl Default for RoomOptions {
    fn default() -> Self {
        RoomOptions {
            allow_hostile: false
        }
    }
}

impl MovementRequest {
    pub fn move_to(destination: RoomPosition) -> MovementRequest {
        MovementRequest {
            destination,
            range: 0,
            room_options: RoomOptions::default()
        }
    }

    pub fn move_to_with_options(destination: RoomPosition, room_options: RoomOptions) -> MovementRequest {
        MovementRequest {
            destination,
            range: 0,
            room_options
        }
    }

    pub fn move_to_range(destination: RoomPosition, range: u32) -> MovementRequest {
        MovementRequest { 
            destination, 
            range,
            room_options: RoomOptions::default()
        }
    }

    pub fn move_to_range_with_options(destination: RoomPosition, range: u32, room_options: RoomOptions) -> MovementRequest {
        MovementRequest {
            destination,
            range,
            room_options
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct CachedMovementData {
    destination: RoomPosition,
    range: u32,
}

#[derive(Component, Serialize, Deserialize)]
pub struct CreepMovementData {
    data: Option<CachedMovementData>,
}

#[derive(Default)]
pub struct MovementData {
    requests: HashMap<Entity, MovementRequest>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MovementData {
    fn request(&mut self, creep_entity: Entity, request: MovementRequest) {
        self.requests.insert(creep_entity, request);
    }

    pub fn move_to(&mut self, creep_entity: Entity, destination: RoomPosition) {
        self.request(creep_entity, MovementRequest::move_to(destination));
    }

    pub fn move_to_range(&mut self, creep_entity: Entity, destination: RoomPosition, range: u32) {
        self.request(creep_entity, MovementRequest::move_to_range(destination, range));
    }
}

#[derive(SystemData)]
pub struct MovementSystemData<'a> {
    movement: Write<'a, MovementData>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
}

pub struct MovementSystem;

impl MovementSystem {
    fn process_request_inbuilt(system_data: &MovementSystemData, creep_entity: Entity, request: &MovementRequest) -> Result<(), String> {
        let creep_owner = system_data.creep_owner.get(creep_entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        const REUSE_PATH_LENGTH: u32 = 10;

        let move_options = MoveToOptions::new()
            .range(request.range)
            .reuse_path(REUSE_PATH_LENGTH);

        match creep.move_to_with_options(&request.destination, move_options) {
            ReturnCode::Ok => return Ok(()),
            err => return Err(format!("Move error: {:?}", err)),
        }
    }

    fn process_request_custom(system_data: &MovementSystemData, creep_entity: Entity, request: &MovementRequest) -> Result<(), String> {
        let creep_owner = system_data.creep_owner.get(creep_entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        const REUSE_PATH_LENGTH: u32 = 10;

        let move_options = MoveToOptions::new()
            .range(request.range)
            .reuse_path(REUSE_PATH_LENGTH)
            .no_path_finding(true);

        match creep.move_to_with_options(&request.destination, move_options) {
            ReturnCode::Ok => return Ok(()),
            ReturnCode::NotFound => {},
            err => return Err(format!("Move error: {:?}", err)),
        }

        let creep_pos = creep.pos();
        let creep_room_name = creep_pos.room_name();

        let room_path = game::map::find_route_with_callback(
            creep_room_name, 
            request.destination.room_name(),
            |to_room_name, from_room_name| Self::get_room_weight(system_data, from_room_name, to_room_name, creep_room_name, &request.room_options).unwrap_or(f64::INFINITY)
        ).map_err(|e| format!("Could not find path between rooms: {:?}", e))?;

        let room_names: HashSet<_> = room_path
            .iter()
            .map(|step| RoomName::from_str(&step.room).unwrap())
            .collect();

        let cost_callback = |room_name: RoomName, _cost_matrix: CostMatrix| -> MultiRoomCostResult {
            if room_names.contains(&room_name) {
                //TODO: Get or generate cost matrix!
                MultiRoomCostResult::Default
            } else {
                MultiRoomCostResult::Impassable
            }
        };

        let move_options = MoveToOptions::new()
            .range(request.range)
            .reuse_path(REUSE_PATH_LENGTH)
            .cost_callback(cost_callback);

        match creep.move_to_with_options(&request.destination, move_options) {
            ReturnCode::Ok => Ok(()),
            //TODO: Replace with own pathfinding.
            ReturnCode::NoPath => Ok(()),
            //TODO: Don't run move to if tired?
            ReturnCode::Tired => Ok(()),
            err => Err(format!("Move error: {:?}", err)),
        }
    }

    fn get_room_weight(system_data: &MovementSystemData, from_room_name: RoomName, to_room_name: RoomName, current_room_name: RoomName, room_options: &RoomOptions) -> Option<f64> {
        if !can_traverse_between_rooms(from_room_name, to_room_name) {
            return Some(f64::INFINITY);
        }

        let target_room_entity = system_data.mapping.get_room(&to_room_name)?;
        let target_room_data = system_data.room_data.get(target_room_entity)?;

        // let from_room_entity = system_data.mapping.get_room(&from_room_name)?;
        // let from_room_data = system_data.room_data.get(from_room_entity)?;

        let is_current_room = to_room_name == current_room_name;

        if let Some(dynamic_visibility_data) = target_room_data.get_dynamic_visibility_data() {
            if !is_current_room {
                if !room_options.allow_hostile {
                    if dynamic_visibility_data.source_keeper() || dynamic_visibility_data.owner().hostile() {
                        return Some(f64::INFINITY);
                    }
                } 
            }
            
            if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
                Some(3.0)
            } else if dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().friendly() {
                Some(2.0)
            } else {
                Some(1.0)
            }
        } else {
            Some(2.0)
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MovementSystem {
    type SystemData = MovementSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if crate::features::pathing::custom() {
            for (entity, request) in data.movement.requests.iter() {
                match Self::process_request_inbuilt(&data, *entity, &request) {
                    Ok(()) => {}
                    Err(_err) => {}/* debug!("Failed move: {}", err) */,
                }
            }
        } else {
            for (entity, request) in data.movement.requests.iter() {
                match Self::process_request_inbuilt(&data, *entity, &request) {
                    Ok(()) => {}
                    Err(_err) => {}/* debug!("Failed move: {}", err) */,
                }
            }
        }

        data.movement.requests.clear();
    }
}
