use super::data::*;
use screeps::*;
use specs::prelude::*;

pub struct UpdateRoomDataSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for UpdateRoomDataSystem {
    //TODO: Move this to derived system data.
    type SystemData = (Entities<'a>, WriteStorage<'a, RoomData>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, mut room_datas, _updater): Self::SystemData) {
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
