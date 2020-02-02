use specs::*;
use screeps::*;

use ::creep::CreepOwner;
use super::data::JobData;

pub struct JobSystem;

impl<'a> System<'a> for JobSystem {
    type SystemData = (ReadStorage<'a, CreepOwner>, WriteStorage<'a, JobData>);

    fn run(&mut self, (creeps, mut jobs): Self::SystemData) {
        for (creep, job) in (&creeps, &mut jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                match job {
                    JobData::Idle => {
                        //TODO: wiarchbe: Remove hacky code to assign job.
                        if let Some(source) = owner.room().find(find::SOURCES).first() {
                            info!("Assigning harvest job");
                            *job = JobData::Harvest(super::harvest::HarvestJob::new(&source))
                        }
                    },
                    JobData::Harvest(mut data) => {
                        data.run_creep(&owner);
                    }
                }
            }
        }
    }
}