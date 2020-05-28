use super::data::*;
use crate::serialize::*;
use log::*;
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

        let visible_rooms = screeps::game::rooms::keys();

        let flags = screeps::game::flags::values();
        let flag_rooms = flags.iter().map(|flag| flag.pos().room_name());

        let construction_sites = screeps::game::construction_sites::values();
        let construction_site_rooms = construction_sites
            .iter()
            .map(|construction_site| construction_site.pos().room_name());

        let missing_rooms: HashSet<_> = visible_rooms
            .into_iter()
            .chain(flag_rooms)
            .chain(construction_site_rooms)
            .filter(|name| !existing_rooms.contains(name))
            .collect();

        for room in missing_rooms {
            info!("Creating room data for room: {}", room);

            updater
                .create_entity(&entities)
                .marked::<SerializeMarker>()
                .with(RoomData::new(room))
                .build();
        }
    }
}
