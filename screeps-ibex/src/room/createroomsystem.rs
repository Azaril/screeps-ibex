use super::data::*;
use crate::serialize::*;
use screeps::*;
use specs::saveload::*;
use specs::*;
use std::collections::HashSet;

pub struct CreateRoomDataSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CreateRoomDataSystem {
    type SystemData = (Entities<'a>, WriteStorage<'a, RoomData>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, room_datas, updater): Self::SystemData) {
        let existing_rooms = (&entities, &room_datas)
            .join()
            .map(|(_, room_data)| room_data.name)
            .collect::<std::collections::HashSet<RoomName>>();

        let flag_rooms = screeps::game::flags()
            .values()
            .map(|flag| flag.pos().room_name());

        let construction_site_rooms = screeps::game::construction_sites()
            .values()
            .map(|construction_site| construction_site.pos().room_name());

        let missing_rooms: HashSet<_> = game::rooms()
            .keys()
            .chain(flag_rooms)
            .chain(construction_site_rooms)
            .filter(|name| !existing_rooms.contains(name))
            .collect();

        for room in missing_rooms {
            updater
                .create_entity(&entities)
                .marked::<SerializeMarker>()
                .with(RoomData::new(room))
                .build();
        }
    }
}
