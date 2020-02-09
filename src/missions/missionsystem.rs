use specs::*;
use specs::prelude::*;

use super::data::*;
use ::room::data::*;
use ::creep::*;
use ::spawnsystem::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    missions: WriteStorage<'a, MissionData>,
    room_owner: WriteStorage<'a, RoomOwnerData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    creep_markers: Read<'a, CreepMarkerAllocator>,
    spawn_queue: Write<'a, SpawnQueue>
}

pub struct MissionExecutionSystemData<'a> {  
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub creep_markers: &'a Read<'a, CreepMarkerAllocator>,
    pub spawn_queue: &'a Write<'a, SpawnQueue>
}

pub struct MissionExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
    pub room_owner: &'a RoomOwnerData,
}

pub trait Mission {
    fn run_mission(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData);
}

pub struct MissionSystem;

impl<'a> System<'a> for MissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("MissionSystem");

        let system_data = MissionExecutionSystemData{
            updater: &data.updater,
            entities: &data.entities,
            creep_markers: &data.creep_markers,
            spawn_queue: &data.spawn_queue
        };

        for (entity, room_owner, mission) in (&data.entities, &data.room_owner, &mut data.missions).join() {
            let runtime_data = MissionExecutionRuntimeData{
                entity: &entity,
                room_owner: &room_owner
            };            

            mission.as_mission().run_mission(&system_data, &runtime_data);
        }
    }
}