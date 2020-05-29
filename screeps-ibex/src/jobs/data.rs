use super::jobsystem::*;
use serde::*;
use specs::saveload::*;
use specs::*;

#[derive(Clone, Component, ConvertSaveload)]
pub enum JobData {
    Harvest(super::harvest::HarvestJob),
    Upgrade(super::upgrade::UpgradeJob),
    Build(super::build::BuildJob),
    StaticMine(super::staticmine::StaticMineJob),
    LinkMine(super::linkmine::LinkMineJob),
    Haul(super::haul::HaulJob),
    Scout(super::scout::ScoutJob),
    Reserve(super::reserve::ReserveJob),
    Claim(super::claim::ClaimJob),
    Dismantle(super::dismantle::DismantleJob),
}

impl JobData {
    pub fn as_job(&mut self) -> &mut dyn Job {
        match self {
            JobData::Harvest(ref mut data) => data,
            JobData::Upgrade(ref mut data) => data,
            JobData::Build(ref mut data) => data,
            JobData::StaticMine(ref mut data) => data,
            JobData::LinkMine(ref mut data) => data,
            JobData::Haul(ref mut data) => data,
            JobData::Scout(ref mut data) => data,
            JobData::Reserve(ref mut data) => data,
            JobData::Claim(ref mut data) => data,
            JobData::Dismantle(ref mut data) => data,
        }
    }
}