use screeps::*;
use specs::prelude::*;

use super::data::JobData;
use creep::CreepOwner;

#[derive(SystemData)]
pub struct JobSystemData<'a> {
    creep_owners: ReadStorage<'a, CreepOwner>,
    jobs: WriteStorage<'a, JobData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
}

pub struct JobRuntimeData<'a> {
    pub owner: &'a Creep,
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
}

pub trait Job {
    fn run_job(&mut self, data: &JobRuntimeData);
}

pub struct JobSystem;

impl<'a> System<'a> for JobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("JobSystem");

        for (creep, job) in (&data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let runtime_data = JobRuntimeData {
                    owner: &owner,
                    updater: &data.updater,
                    entities: &data.entities,
                };

                job.as_job().run_job(&runtime_data);
            }
        }
    }
}
