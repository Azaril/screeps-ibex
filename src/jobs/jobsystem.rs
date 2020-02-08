use specs::*;
use specs::prelude::*;
use screeps::*;

use ::creep::CreepOwner;
use super::data::JobData;

#[derive(SystemData)]
pub struct JobSystemData<'a> {
    creep_owners: ReadStorage<'a, CreepOwner>,
    jobs: WriteStorage<'a, JobData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
}

pub struct JobRuntimeData<'a> {
    pub updater: Read<'a, LazyUpdate>,
    pub entities: Entities<'a>,
}

pub trait Job {
    fn run_job(&mut self, data: &JobRuntimeData, owner: &Creep);
}

pub struct JobSystem;

impl<'a> System<'a> for JobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        scope_timing!("JobSystem");

        let runtime_data = JobRuntimeData{
            updater: data.updater,
            entities: data.entities
        };

        for (creep, job) in (&data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                job.as_job().run_job(&runtime_data, &owner);
            }
        }
    }
}