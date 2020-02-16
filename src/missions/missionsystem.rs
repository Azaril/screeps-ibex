use specs::prelude::*;

use super::data::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::room::data::*;
use crate::spawnsystem::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    updater: Read<'a, LazyUpdate>,
    missions: WriteStorage<'a, MissionData>,
    room_data: WriteStorage<'a, RoomData>,
    entities: Entities<'a>,
    spawn_queue: Write<'a, SpawnQueue>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    job_data: WriteStorage<'a, JobData>,
}

pub struct MissionExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub room_data: &'a WriteStorage<'a, RoomData>,
    pub entities: &'a Entities<'a>,
    pub spawn_queue: &'a Write<'a, SpawnQueue>,
    pub creep_owner: &'a ReadStorage<'a, CreepOwner>,
    pub job_data: &'a WriteStorage<'a, JobData>,
}

pub struct MissionExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
}

pub enum MissionResult {
    Running,
    Success,
    Failure,
}

pub trait Mission {
    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult;
}

pub struct MissionSystem;

impl<'a> System<'a> for MissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("MissionSystem");

        let system_data = MissionExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            spawn_queue: &data.spawn_queue,
            creep_owner: &data.creep_owner,
            job_data: &data.job_data,
        };

        for (entity, mission) in (&data.entities, &mut data.missions).join() {
            let runtime_data = MissionExecutionRuntimeData { entity: &entity };

            let cleanup_mission = match mission
                .as_mission()
                .run_mission(&system_data, &runtime_data)
            {
                MissionResult::Running => false,
                MissionResult::Success => {
                    info!("Mission complete, cleaning up.");

                    true
                }
                MissionResult::Failure => {
                    info!("Mission failed, cleaning up.");

                    true
                }
            };

            if cleanup_mission {
                data.updater.exec_mut(move |world| {
                    if let Err(err) = world.delete_entity(entity) {
                        warn!(
                            "Trying to clean up mission entity that no longer exists. Error: {}",
                            err
                        );
                    }
                });
            }
        }
    }
}
