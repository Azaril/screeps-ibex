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
    Attack(super::attack::AttackJob),
    Heal(super::heal::HealJob),
    RangedAttack(super::ranged::RangedAttackJob),
    Tank(super::tank::TankJob),
    SquadCombat(super::squad_combat::SquadCombatJob),
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
            JobData::Attack(ref data) => data.summarize(),
            JobData::Heal(ref data) => data.summarize(),
            JobData::RangedAttack(ref data) => data.summarize(),
            JobData::Tank(ref data) => data.summarize(),
            JobData::SquadCombat(ref data) => data.summarize(),
        }
    }

    /// Extract the squad entity id if this job is associated with a squad.
    /// Returns `None` for non-squad jobs.
    pub fn squad_entity_id(&self) -> Option<u32> {
        match self {
            JobData::Attack(ref data) => data.context.squad_entity,
            JobData::Heal(ref data) => data.context.squad_entity,
            JobData::RangedAttack(ref data) => data.context.squad_entity,
            JobData::Tank(ref data) => data.context.squad_entity,
            JobData::SquadCombat(ref data) => data.context.squad_entity,
            _ => None,
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
            JobData::Attack(ref mut data) => data,
            JobData::Heal(ref mut data) => data,
            JobData::RangedAttack(ref mut data) => data,
            JobData::Tank(ref mut data) => data,
            JobData::SquadCombat(ref mut data) => data,
        }
    }
}
