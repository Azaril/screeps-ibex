use screeps::*;
use specs::saveload::*;
use specs::*;

use super::data::*;

pub struct CreateRoomDataSystem;

impl<'a> System<'a> for CreateRoomDataSystem {
    type SystemData = (Entities<'a>, WriteStorage<'a, RoomData>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, room_datas, updater): Self::SystemData) {
        scope_timing!("CreateRoomDataSystem");

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

        let missing_rooms = visible_rooms
            .into_iter()
            .chain(flag_rooms)
            .chain(construction_site_rooms)
            .filter(|name| !existing_rooms.contains(name));

        for room in missing_rooms {
            info!("Creating room data for room: {}", room);

            updater
                .create_entity(&entities)
                .marked::<::serialize::SerializeMarker>()
                .with(RoomData::new(room))
                .build();
        }
    }
}