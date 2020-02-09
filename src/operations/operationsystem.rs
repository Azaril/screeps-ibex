use specs::*;
use specs::prelude::*;

use super::data::*;

#[derive(SystemData)]
pub struct OperationSystemData<'a> {
    operations: WriteStorage<'a, OperationData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>, 
    room_owner: WriteStorage<'a, ::room::data::RoomOwnerData>,
    room_data: WriteStorage<'a, ::room::data::RoomData>
}

pub struct OperationRuntimeData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub room_owner: &'a WriteStorage<'a, ::room::data::RoomOwnerData>,
    pub room_data: &'a WriteStorage<'a, ::room::data::RoomData>
}

pub trait Operation {
    fn run_operation(&mut self, data: &OperationRuntimeData);
}

pub struct OperationSystem;

impl<'a> System<'a> for OperationSystem {
    type SystemData = OperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("OperationSystem");

        let runtime_data = OperationRuntimeData{
            updater: &data.updater,
            entities: &data.entities,
            room_owner: &data.room_owner,
            room_data: &data.room_data
        };

        for operation in (&mut data.operations).join() {
            operation.as_operation().run_operation(&runtime_data);
        }
    }
}