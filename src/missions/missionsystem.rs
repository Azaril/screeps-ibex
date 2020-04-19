use super::data::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::operations::data::*;
use crate::ownership::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::spawnsystem::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
use log::*;
use specs::prelude::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    updater: Read<'a, LazyUpdate>,
    missions: WriteStorage<'a, MissionData>,
    room_data: WriteStorage<'a, RoomData>,
    room_plan_data: ReadStorage<'a, RoomPlanData>,
    room_plan_queue: Write<'a, RoomPlanQueue>,
    entities: Entities<'a>,
    spawn_queue: Write<'a, SpawnQueue>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    creep_spawning: ReadStorage<'a, CreepSpawning>,
    job_data: WriteStorage<'a, JobData>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
    transfer_queue: Write<'a, TransferQueue>,
    order_queue: Write<'a, OrderQueue>,
}

pub struct MissionExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub room_data: &'a WriteStorage<'a, RoomData>,
    pub room_plan_data: &'a ReadStorage<'a, RoomPlanData>,
    pub entities: &'a Entities<'a>,
    pub creep_owner: &'a ReadStorage<'a, CreepOwner>,
    pub creep_spawning: &'a ReadStorage<'a, CreepSpawning>,
    pub job_data: &'a WriteStorage<'a, JobData>,
}

pub struct MissionExecutionRuntimeData<'a> {
    pub entity: &'a Entity,
    pub spawn_queue: &'a mut SpawnQueue,
    pub room_plan_queue: &'a mut RoomPlanQueue,
    pub visualizer: Option<&'a mut Visualizer>,
    pub transfer_queue: &'a mut TransferQueue,
    pub order_queue: &'a mut OrderQueue,
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Mission {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity>;

    fn get_room(&self) -> Entity;

    fn child_complete(&mut self, _child: Entity) {}

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

fn queue_cleanup_mission(updater: &LazyUpdate, mission_entity: Entity) {
    updater.exec_mut(move |world| {
        {
            let mission_data_storage = &mut world.write_storage::<MissionData>();

            let owner = if let Some(mission_data) = mission_data_storage.get_mut(mission_entity) {
                let mission = mission_data.as_mission();

                let room_data_entity = mission.get_room();

                let room_data_storage = &mut world.write_storage::<RoomData>();

                if let Some(room_data) = room_data_storage.get_mut(room_data_entity) {
                    room_data.remove_mission(mission_entity);
                }

                mission.get_owner().clone()
            } else {
                None
            };

            match owner {
                Some(OperationOrMissionEntity::Operation(operation_entity)) => {
                    let operation_data_storage = &mut world.write_storage::<OperationData>();
                    if let Some(operation_data) = operation_data_storage.get_mut(operation_entity) {
                        operation_data.as_operation().child_complete(mission_entity);
                    }
                }
                Some(OperationOrMissionEntity::Mission(mission_entity)) => {
                    if let Some(mission_data) = mission_data_storage.get_mut(mission_entity) {
                        mission_data.as_mission().child_complete(mission_entity);
                    }
                }
                None => {}
            }
        }

        if let Err(err) = world.delete_entity(mission_entity) {
            warn!("Trying to clean up mission entity that no longer exists. Error: {}", err);
        }
    });
}

pub struct PreRunMissionSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = MissionExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            room_plan_data: &data.room_plan_data,
            creep_owner: &data.creep_owner,
            creep_spawning: &data.creep_spawning,
            job_data: &data.job_data,
        };

        for (entity, mission_data) in (&data.entities, &mut data.missions).join() {
            let mut runtime_data = MissionExecutionRuntimeData {
                entity: &entity,
                spawn_queue: &mut data.spawn_queue,
                room_plan_queue: &mut data.room_plan_queue,
                visualizer: data.visualizer.as_deref_mut(),
                transfer_queue: &mut data.transfer_queue,
                order_queue: &mut data.order_queue,
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
                queue_cleanup_mission(&data.updater, entity);
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = MissionExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            room_plan_data: &data.room_plan_data,
            creep_owner: &data.creep_owner,
            creep_spawning: &data.creep_spawning,
            job_data: &data.job_data,
        };

        for (entity, mission_data) in (&data.entities, &mut data.missions).join() {
            let mut runtime_data = MissionExecutionRuntimeData {
                entity: &entity,
                spawn_queue: &mut data.spawn_queue,
                room_plan_queue: &mut data.room_plan_queue,
                visualizer: data.visualizer.as_deref_mut(),
                transfer_queue: &mut data.transfer_queue,
                order_queue: &mut data.order_queue,
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
                queue_cleanup_mission(&data.updater, entity);
            }
        }
    }
}
