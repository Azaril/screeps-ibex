use serde::*;
use specs::*;
use specs_derive::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Component)]
pub enum JobData {
    Harvest(super::harvest::HarvestJob),
    Upgrade(super::upgrade::UpgradeJob)
}

impl JobData
{
    pub fn as_job(&mut self) -> &mut dyn Job
    {
        match self {
            JobData::Harvest(ref mut data) => data,
            JobData::Upgrade(ref mut data) => data
        }
    }
}