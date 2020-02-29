use screeps::*;
use specs::prelude::*;

use super::data::JobData;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
use creep::CreepOwner;

#[derive(SystemData)]
pub struct JobSystemData<'a> {
    creep_owners: ReadStorage<'a, CreepOwner>,
    jobs: WriteStorage<'a, JobData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
    transfer_queue: Write<'a, TransferQueue>,
}

pub struct JobExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
}

pub struct JobExecutionRuntimeData<'a> {
    pub owner: &'a Creep,
    pub transfer_queue: &'a mut TransferQueue,
}

pub struct JobDescribeData<'a> {
    pub owner: &'a Creep,
    pub visualizer: &'a mut Visualizer,
    pub ui: &'a mut UISystem,
}

pub trait Job {
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData);

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData);
}

pub struct PreRunJobSystem;

impl<'a> System<'a> for PreRunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("PreRunJobSystem");

        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
        };

        for (creep, job_data) in (&data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    owner: &owner,
                    transfer_queue: &mut data.transfer_queue,
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

impl<'a> System<'a> for RunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("RunJobSystem");

        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
        };

        for (creep, job_data) in (&data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    owner: &owner,
                    transfer_queue: &mut data.transfer_queue,
                };

                job_data.as_job().run_job(&system_data, &mut runtime_data);
            }
        }
    }
}
