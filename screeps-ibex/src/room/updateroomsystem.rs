use super::data::*;
use screeps::*;
use specs::prelude::*;

#[derive(SystemData)]
pub struct UpdateRoomDataSystemData<'a> {
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
#[allow(dead_code)] // FOLLOW-UP (ws-triage 2026-08-23): unused fetch/field — remove in the SystemData cleanup pass
    updater: Read<'a, LazyUpdate>,
    identity: Read<'a, crate::identity::BotIdentity>,
}

pub struct UpdateRoomDataSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for UpdateRoomDataSystem {
    type SystemData = UpdateRoomDataSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let rooms = game::rooms();

        for (_entity, room_data) in (&data.entities, &mut data.room_data).join() {
            if let Some(room) = rooms.get(room_data.name) {
                room_data.update(&room, &data.identity.username);
            }
        }
    }
}
