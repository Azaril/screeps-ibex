use super::data::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::room::data::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
use specs::prelude::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    updater: Read<'a, LazyUpdate>,
    missions: WriteStorage<'a, MissionData>,
    room_data: WriteStorage<'a, RoomData>,
    entities: Entities<'a>,
    spawn_queue: Write<'a, SpawnQueue>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    creep_spawning: ReadStorage<'a, CreepSpawning>,
    job_data: WriteStorage<'a, JobData>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
    transfer_queue: Write<'a, TransferQueue>,
}

pub struct MissionExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub room_data: &'a WriteStorage<'a, RoomData>,
    pub entities: &'a Entities<'a>,
    pub creep_owner: &'a ReadStorage<'a, CreepOwner>,
    pub creep_spawning: &'a ReadStorage<'a, CreepSpawning>,
    pub job_data: &'a WriteStorage<'a, JobData>,
}

pub struct MissionExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
    pub spawn_queue: &'a mut SpawnQueue,
    pub visualizer: Option<&'a mut Visualizer>,
    pub transfer_queue: &'a mut TransferQueue,
}

pub struct MissionDescribeData<'a> {
    pub entity: &'a Entity,
    pub visualizer: &'a mut Visualizer,
    pub ui: &'a mut UISystem,
}

pub enum MissionResult {
    Running,
    Success,
}

pub trait Mission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData);

    fn pre_run_mission(
        &mut self,
        _system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String>;
}

pub struct PreRunMissionSystem;

#[cfg_attr(feature = "time", timing)]
impl<'a> System<'a> for PreRunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = MissionExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            creep_owner: &data.creep_owner,
            creep_spawning: &data.creep_spawning,
            job_data: &data.job_data,
        };

        for (entity, mission_data) in (&data.entities, &mut data.missions).join() {
            let mut runtime_data = MissionExecutionRuntimeData {
                entity: &entity,
                spawn_queue: &mut data.spawn_queue,
                visualizer: data.visualizer.as_deref_mut(),
                transfer_queue: &mut data.transfer_queue,
            };

            let mission = mission_data.as_mission();

            let cleanup_mission = match mission.pre_run_mission(&system_data, &mut runtime_data) {
                Ok(()) => false,
                Err(error) => {
                    info!("Mission failed, cleaning up. Error: {}", error);
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

        //TODO: Is this the right phase for visualization? Potentially better at the end of tick?
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                for (entity, mission_data) in (&data.entities, &mut data.missions).join() {
                    let mut describe_data = MissionDescribeData {
                        entity: &entity,
                        visualizer,
                        ui,
                    };

                    let mission = mission_data.as_mission();

                    mission.describe(&system_data, &mut describe_data);
                }
            }
        }
    }
}

pub struct RunMissionSystem;

#[cfg_attr(feature = "time", timing)]
impl<'a> System<'a> for RunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = MissionExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            creep_owner: &data.creep_owner,
            creep_spawning: &data.creep_spawning,
            job_data: &data.job_data,
        };

        for (entity, mission_data) in (&data.entities, &mut data.missions).join() {
            let mut runtime_data = MissionExecutionRuntimeData {
                entity: &entity,
                spawn_queue: &mut data.spawn_queue,
                visualizer: data.visualizer.as_deref_mut(),
                transfer_queue: &mut data.transfer_queue,
            };

            let mission = mission_data.as_mission();

            let cleanup_mission = match mission.run_mission(&system_data, &mut runtime_data) {
                Ok(MissionResult::Running) => false,
                Ok(MissionResult::Success) => {
                    info!("Mission complete, cleaning up.");
                    true
                }
                Err(error) => {
                    info!("Mission failed, cleaning up. Error: {}", error);
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
