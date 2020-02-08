use specs::*;
use specs::prelude::*;

use super::data::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    missions: WriteStorage<'a, MissionData>,
    room_owner: WriteStorage<'a, ::room::data::RoomOwnerData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
}

pub struct MissionRuntimeData<'a> {
    pub updater: Read<'a, LazyUpdate>,
    pub entities: Entities<'a>,
}

pub trait Mission {
    fn run_mission(&mut self, data: &MissionRuntimeData, owner: &::room::data::RoomOwnerData);
}

pub struct MissionSystem;

impl<'a> System<'a> for MissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("MissionSystem");

        let runtime_data = MissionRuntimeData{
            updater: data.updater,
            entities: data.entities
        };

        for (owner, mission) in (&data.room_owner, &mut data.missions).join() {
            mission.as_mission().run_mission(&runtime_data, &owner);
        }
    }
}