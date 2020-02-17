use screeps::*;
use specs::prelude::*;
use super::data::*;

pub struct UpdateRoomDataSystem;

impl<'a> System<'a> for UpdateRoomDataSystem {
    //TODO: Move this to derived system data.
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, RoomData>,
        Read<'a, LazyUpdate>,
    );

    fn run(&mut self, (entities, mut room_datas, _updater): Self::SystemData) {
        scope_timing!("UpdateRoomDataSystem");

        let rooms = game::rooms::hashmap();

        for (_entity, room_data) in (&entities, &mut room_datas).join() {
            if let Some(room) = rooms.get(&room_data.name) {
                room_data.update(&room);
            } else {
                room_data.clear_visible();
            }
        }
    }
}
