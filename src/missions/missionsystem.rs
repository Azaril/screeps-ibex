use specs::prelude::*;

use super::data::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::room::data::*;
use crate::spawnsystem::*;
use crate::visualize::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    updater: Read<'a, LazyUpdate>,
    missions: WriteStorage<'a, MissionData>,
    room_data: WriteStorage<'a, RoomData>,
    entities: Entities<'a>,
    spawn_queue: Write<'a, SpawnQueue>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    job_data: WriteStorage<'a, JobData>,
    visualizer: Option<Write<'a, Visualizer>>,
}

pub struct MissionExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub room_data: &'a WriteStorage<'a, RoomData>,
    pub entities: &'a Entities<'a>,
    pub creep_owner: &'a ReadStorage<'a, CreepOwner>,
    pub job_data: &'a WriteStorage<'a, JobData>,
}

pub struct MissionExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
    pub spawn_queue: &'a mut SpawnQueue,
    pub visualizer: Option<&'a mut Visualizer>,
}

pub enum MissionResult {
    Running,
    Success,
    Failure,
}

pub trait Mission {
    fn describe(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    );

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
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
            creep_owner: &data.creep_owner,
            job_data: &data.job_data,
        };

        for (entity, mission_data) in (&data.entities, &mut data.missions).join() {
            let mut runtime_data = MissionExecutionRuntimeData {
                entity: &entity,
                spawn_queue: &mut data.spawn_queue,
                visualizer: data
                    .visualizer
                    .as_mut()
                    .map(|v| v as &mut crate::visualize::Visualizer),
            };

            let mission = mission_data.as_mission();

            mission.describe(&system_data, &mut runtime_data);

            let cleanup_mission = match mission.run_mission(&system_data, &mut runtime_data) {
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
