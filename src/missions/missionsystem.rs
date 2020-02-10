use specs::*;
use specs::prelude::*;

use super::data::*;
use ::room::data::*;
use ::spawnsystem::*;
use ::jobs::data::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    missions: WriteStorage<'a, MissionData>,
    room_owner: WriteStorage<'a, RoomOwnerData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    spawn_queue: Write<'a, SpawnQueue>,
    job_data: WriteStorage<'a, JobData>
}

pub struct MissionExecutionSystemData<'a> {  
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub spawn_queue: &'a Write<'a, SpawnQueue>,
    pub job_data: &'a WriteStorage<'a, JobData>
}

pub struct MissionExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
    pub room_owner: &'a RoomOwnerData,
}

pub enum MissionResult {
    Running,
    Success,
    Failure
}

pub trait Mission {
    fn run_mission(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult;
}

pub struct MissionSystem;

impl<'a> System<'a> for MissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("MissionSystem");

        let system_data = MissionExecutionSystemData{
            updater: &data.updater,
            entities: &data.entities,
            spawn_queue: &data.spawn_queue,
            job_data: &data.job_data
        };

        for (entity, room_owner, mission) in (&data.entities, &data.room_owner, &mut data.missions).join() {
            let runtime_data = MissionExecutionRuntimeData{
                entity: &entity,
                room_owner: &room_owner
            };            

            let cleanup_mission = match mission.as_mission().run_mission(&system_data, &runtime_data) {
                MissionResult::Running => false,
                MissionResult::Success => {
                    info!("Mission complete, cleaning up.");

                    true
                },
                MissionResult::Failure => {
                    info!("Mission failed, cleaning up.");

                    true
                }
            };

            if cleanup_mission {
                data.updater.exec_mut(move |world| {
                    if let Err(err) = world.delete_entity(entity) {
                        warn!("Trying to clean up mission entity that no longer exists. Error: {}", err);
                    }
                });
            }
        }
    }
}