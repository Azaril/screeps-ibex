use serde::*;
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::jobsystem::*;

#[derive(Clone, Component, ConvertSaveload)]
pub enum JobData {
    Harvest(super::harvest::HarvestJob),
    Upgrade(super::upgrade::UpgradeJob),
    Build(super::build::BuildJob),
    StaticMine(super::staticmine::StaticMineJob),
    Haul(super::haul::HaulJob),
    Scout(super::scout::ScoutJob),
    Reserve(super::reserve::ReserveJob),
    Claim(super::claim::ClaimJob),
}

impl JobData {
    pub fn as_job(&mut self) -> &mut dyn Job {
        match self {
            JobData::Harvest(ref mut data) => data,
            JobData::Upgrade(ref mut data) => data,
            JobData::Build(ref mut data) => data,
            JobData::StaticMine(ref mut data) => data,
            JobData::Haul(ref mut data) => data,
            JobData::Scout(ref mut data) => data,
            JobData::Reserve(ref mut data) => data,
            JobData::Claim(ref mut data) => data,
        }
    }
}
