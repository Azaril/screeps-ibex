use super::jobsystem::*;
use crate::visualization::SummaryContent;
use serde::*;
#[allow(deprecated)]
use specs::error::NoError;
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
    /// Dispatch summarize() to the concrete job type (read-only).
    pub fn summarize(&self) -> SummaryContent {
        match self {
            JobData::Harvest(ref data) => data.summarize(),
            JobData::Upgrade(ref data) => data.summarize(),
            JobData::Build(ref data) => data.summarize(),
            JobData::StaticMine(ref data) => data.summarize(),
            JobData::LinkMine(ref data) => data.summarize(),
            JobData::Haul(ref data) => data.summarize(),
            JobData::Scout(ref data) => data.summarize(),
            JobData::Reserve(ref data) => data.summarize(),
            JobData::Claim(ref data) => data.summarize(),
            JobData::Dismantle(ref data) => data.summarize(),
        }
    }

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
