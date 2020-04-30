use super::data::JobData;
use crate::creep::CreepOwner;
use crate::entitymappingsystem::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
use screeps::*;
use specs::prelude::*;
use screeps_rover::*;

#[derive(specs::SystemData)]
pub struct JobSystemData<'a> {
    creep_owners: ReadStorage<'a, CreepOwner>,
    jobs: WriteStorage<'a, JobData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
    transfer_queue: Write<'a, TransferQueue>,
    room_data: ReadStorage<'a, RoomData>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    mapping: Read<'a, EntityMappingData>,
}

pub struct JobExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub room_data: &'a ReadStorage<'a, RoomData>,
}

pub struct JobExecutionRuntimeData<'a> {
    pub creep_entity: Entity,
    pub owner: &'a Creep,
    pub mapping: &'a EntityMappingData,
    pub transfer_queue: &'a mut TransferQueue,
    pub movement: &'a mut MovementData<Entity>,
}

pub struct JobDescribeData<'a> {
    pub owner: &'a Creep,
    pub visualizer: &'a mut Visualizer,
    pub ui: &'a mut UISystem,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Job {
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData);

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData);
}

pub struct PreRunJobSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
        };

        for (creep_entity, creep, job_data) in (&data.entities, &data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    creep_entity,
                    owner: &owner,
                    mapping: &mut data.mapping,
                    transfer_queue: &mut data.transfer_queue,
                    movement: &mut data.movement,
                };

                job_data.as_job().pre_run_job(&system_data, &mut runtime_data);
            }
        }

        //TODO: Is this the right phase for visualization? Potentially better at the end of tick?
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                for (creep, job_data) in (&data.creep_owners, &mut data.jobs).join() {
                    if let Some(owner) = creep.owner.resolve() {
                        let mut describe_data = JobDescribeData {
                            owner: &owner,
                            visualizer,
                            ui,
                        };

                        job_data.as_job().describe(&system_data, &mut describe_data);
                    }
                }
            }
        }
    }
}

pub struct RunJobSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
        };

        for (creep_entity, creep, job_data) in (&data.entities, &data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    creep_entity,
                    owner: &owner,
                    mapping: &mut data.mapping,
                    transfer_queue: &mut data.transfer_queue,
                    movement: &mut data.movement,
                };

                job_data.as_job().run_job(&system_data, &mut runtime_data);
            }
        }
    }
}
