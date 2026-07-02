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
    Declaim(super::declaim::DeclaimJob),
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
            JobData::Declaim(ref data) => data.summarize(),
            JobData::SquadCombat(ref data) => data.summarize(),
        }
    }

    /// The squad entity this job coordinates with, if any (`None` for non-squad jobs). The stored ref is
    /// the marker-converted (`EntityOption<Entity>`) squad handle (REC-009b); callers must still
    /// `is_alive`-guard it (a recycled/dead slot is not a live squad).
    pub fn squad_ref(&self) -> Option<Entity> {
        match self {
            JobData::SquadCombat(ref data) => *data.context.squad_entity,
            _ => None,
        }
    }

    /// Serialize-time backstop for this job's cross-entity references: scrub any dangling `Entity` ref to
    /// `None` so the specs `ConvertSaveload for Entity` marker lookup cannot panic at serialize time
    /// (`[[ecs-dangling-ref-serialize]]`). `is_valid` is the same `is_alive(e) && markers.get(e).is_some()`
    /// predicate the other `repair_entity_integrity` passes use. Only `SquadCombat` carries such a ref (its
    /// `squad_entity`); every other variant is a no-op. REC-009b.
    pub fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        if let JobData::SquadCombat(ref mut data) = self {
            if let Some(squad) = *data.context.squad_entity {
                if !is_valid(squad) {
                    *data.context.squad_entity = None;
                }
            }
        }
    }

    /// MILITARY vs CIVILIAN for the live idle-disposition split (ADR 0033 M5 live adoption,
    /// operator-ratified 2026-07-01 decision (3)): a request-less MILITARY creep is HOLDING —
    /// an in-position fighter that decided to act, not move, this tick — and registers as an
    /// `Immovable` hold (never displaced; shoving it out of formation was the combat-agent
    /// `register_idle_creeps: false` finding, combat-agent/src/pathing.rs `combat_mover_config`).
    /// A request-less CIVILIAN is parked junk and registers as a shoveable Low idle
    /// (`set_idle_creep_positions`). Conservative by construction: war-adjacent jobs
    /// (squad combat, raid/dismantle in hostile rooms, de-claim) are military; pure economy
    /// (harvest/haul/build/upgrade/mine) and unarmed solo travellers (scout/reserve/claim) are
    /// civilian. Callers must default UNKNOWN/no-job to MILITARY — mis-classifying a fighter as
    /// shoveable breaks formations, a hauler held `Immovable` merely costs a detour. Exhaustive
    /// match (no wildcard) so a new job variant forces this decision.
    pub fn is_military(&self) -> bool {
        match self {
            JobData::SquadCombat(_) | JobData::Dismantle(_) | JobData::Declaim(_) => true,
            JobData::Harvest(_)
            | JobData::Upgrade(_)
            | JobData::Build(_)
            | JobData::StaticMine(_)
            | JobData::LinkMine(_)
            | JobData::Haul(_)
            | JobData::Scout(_)
            | JobData::Reserve(_)
            | JobData::Claim(_) => false,
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
            JobData::Declaim(ref mut data) => data,
            JobData::SquadCombat(ref mut data) => data,
        }
    }
}
