use specs::*;

use ::creep::CreepOwner;
use super::data::JobData;

pub struct JobSystem;

impl<'a> System<'a> for JobSystem {
    type SystemData = (
        ReadStorage<'a, CreepOwner>, 
        WriteStorage<'a, JobData>
    );

    fn run(&mut self, (creeps, mut jobs): Self::SystemData) {
        for (creep, job) in (&creeps, &mut jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                match job {
                    JobData::Idle => {},
                    JobData::Harvest(ref mut data) => {
                        data.run_creep(&owner);
                    }
                }
            }
        }
    }
}