use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct MappingData {
    pub rooms: HashMap<RoomName, Entity>,
}

#[derive(SystemData)]
pub struct MappingSystemData<'a> {
    mapping: Write<'a, MappingData>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, ::room::data::RoomData>,
}

pub struct MappingSystem;

impl<'a> System<'a> for MappingSystem {
    type SystemData = MappingSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("MappingSystem");

        let mapping = &mut data.mapping;

        for (entity, room_data) in (&data.entities, &data.room_data).join() {
            mapping.rooms.insert(room_data.name, entity);
        }
    }
}
