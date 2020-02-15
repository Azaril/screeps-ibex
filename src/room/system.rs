use specs::*;
use specs::saveload::*;
use screeps::*;

use super::data::*;

pub struct CreateRoomDataSystem;

impl<'a> System<'a> for CreateRoomDataSystem {
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, RoomOwnerData>,
        WriteStorage<'a, RoomData>,
        Read<'a, LazyUpdate>
    );

    fn run(&mut self, (entities, rooms, room_datas, updater): Self::SystemData) {
        scope_timing!("CreateRoomDataSystem");

        let existing_rooms = 
            (&entities, &rooms, &room_datas).join()
            .map(|(_, room, _)| room.owner)
            .collect::<std::collections::HashSet<RoomName>>();

        let missing_rooms = screeps::game::rooms::keys().into_iter()
            .filter(|name| !existing_rooms.contains(name));

        for room in missing_rooms {
            info!("Creating room data for room: {}", room);
            
            updater.create_entity(&entities)
                .marked::<::serialize::SerializeMarker>()
                .with(RoomOwnerData::new(room))
                .with(RoomData::new())
                .build();
        }
    }
}