use serde::*;
use specs::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum JobData {
    Harvest(super::harvest::HarvestJob)
}

impl Component for JobData {
    type Storage = VecStorage<Self>;
}

impl JobData
{
    pub fn as_job(&mut self) -> &mut dyn Job
    {
        match self {
            JobData::Harvest(ref mut data) => data
        }
    }
}