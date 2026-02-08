use crate::room::data::*;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct EntityMappingData {
    rooms: HashMap<RoomName, Entity>,
}

impl EntityMappingData {
    pub fn get_room(&self, room_name: &RoomName) -> Option<Entity> {
        self.rooms.get(room_name).cloned()
    }
}

#[derive(SystemData)]
pub struct EntityMappingSystemData<'a> {
    mapping: Write<'a, EntityMappingData>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
}

pub struct EntityMappingSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for EntityMappingSystem {
    type SystemData = EntityMappingSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.mapping.rooms = (&data.entities, &data.room_data)
            .join()
            .map(|(entity, room_data)| (room_data.name, entity))
            .collect::<HashMap<RoomName, Entity>>();
    }
}
