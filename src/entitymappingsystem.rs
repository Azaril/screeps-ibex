use crate::room::data::*;
use screeps::*;
use specs::prelude::*;
use specs::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct EntityMappingData {
    pub rooms: HashMap<RoomName, Entity>,
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
        let mapping = &mut data.mapping;

        for (entity, room_data) in (&data.entities, &data.room_data).join() {
            mapping.rooms.insert(room_data.name, entity);
        }
    }
}
