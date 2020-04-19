use crate::creep::*;
use log::*;
use screeps::*;
use serde::*;
use specs::prelude::*;
use specs_derive::*;
use std::collections::HashMap;

struct MovementRequest {
    destination: RoomPosition,
    range: u32,
}

impl MovementRequest {
    pub fn move_to(destination: RoomPosition) -> MovementRequest {
        Self::move_to_range(destination, 0)
    }

    pub fn move_to_range(destination: RoomPosition, range: u32) -> MovementRequest {
        MovementRequest { destination, range }
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
}

pub struct MovementSystem;

impl MovementSystem {
    fn process_request(system_data: &MovementSystemData, creep_entity: Entity, request: &MovementRequest) -> Result<(), String> {
        let creep_owner = system_data.creep_owner.get(creep_entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        let move_options = MoveToOptions::new()
            .range(request.range)
            .reuse_path(10);

        match creep.move_to_with_options(&request.destination, move_options) {
            ReturnCode::Ok => Ok(()),
            //TODO: Replace with own pathfinding.
            ReturnCode::NoPath => Ok(()),
            //TODO: Don't run move to if tired?
            ReturnCode::Tired => Ok(()),
            err => Err(format!("Move error: {:?}", err)),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MovementSystem {
    type SystemData = MovementSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        for (entity, request) in data.movement.requests.iter() {
            match Self::process_request(&data, *entity, &request) {
                Ok(()) => {}
                Err(err) => info!("Failed move: {}", err),
            }
        }

        data.movement.requests.clear();
    }
}
