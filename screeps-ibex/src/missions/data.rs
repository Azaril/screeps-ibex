use super::missionsystem::*;
use crate::serialize::*;
use serde::*;
use specs::saveload::*;
use specs::*;
use std::cell::*;
use std::convert::*;

#[derive(Component, ConvertSaveload)]
pub enum MissionData {
    LocalSupply(EntityRefCell<super::localsupply::LocalSupplyMission>),
    Upgrade(EntityRefCell<super::upgrade::UpgradeMission>),
    LocalBuild(EntityRefCell<super::localbuild::LocalBuildMission>),
    Tower(EntityRefCell<super::tower::TowerMission>),
    RemoteMine(EntityRefCell<super::remotemine::RemoteMineMission>),
    Scout(EntityRefCell<super::scout::ScoutMission>),
    Construction(EntityRefCell<super::construction::ConstructionMission>),
    Reserve(EntityRefCell<super::reserve::ReserveMission>),
    Claim(EntityRefCell<super::claim::ClaimMission>),
    RemoteBuild(EntityRefCell<super::remotebuild::RemoteBuildMission>),
    Haul(EntityRefCell<super::haul::HaulMission>),
    Terminal(EntityRefCell<super::terminal::TerminalMission>),
    MiningOutpost(EntityRefCell<super::miningoutpost::MiningOutpostMission>),
    Raid(EntityRefCell<super::raid::RaidMission>),
    Dismantle(EntityRefCell<super::dismantle::DismantleMission>),
    Colony(EntityRefCell<super::colony::ColonyMission>),
    Defend(EntityRefCell<super::defend::DefendMission>),
    PowerSpawn(EntityRefCell<super::powerspawn::PowerSpawnMission>),
    Labs(EntityRefCell<super::labs::LabsMission>),
}

impl MissionData {
    pub fn as_mission(&self) -> Ref<dyn Mission> {
        match self {
            MissionData::LocalSupply(data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Upgrade(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::LocalBuild(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Tower(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::RemoteMine(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Scout(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Construction(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Reserve(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Claim(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::RemoteBuild(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Haul(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Terminal(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::MiningOutpost(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Raid(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Dismantle(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Colony(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Defend(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::PowerSpawn(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
            MissionData::Labs(ref data) => Ref::map(data.borrow(), |m| -> &dyn Mission { m }),
        }
    }

    pub fn as_mission_mut(&self) -> RefMut<dyn Mission> {
        match self {
            MissionData::LocalSupply(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Upgrade(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::LocalBuild(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Tower(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::RemoteMine(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Scout(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Construction(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Reserve(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Claim(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::RemoteBuild(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Haul(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Terminal(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::MiningOutpost(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Raid(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Dismantle(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Colony(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Defend(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::PowerSpawn(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
            MissionData::Labs(ref data) => RefMut::map(data.borrow_mut(), |m| -> &mut dyn Mission { m }),
        }
    }
}

//
// Trait
//

pub trait AsMissionType<'a, SM> {
    fn as_mission_type<T>(&'a self) -> Option<Ref<'a, T>>
    where
        Ref<'a, T>: TryFrom<SM>;
}

pub trait AsMissionTypeMut<'a, SM> {
    fn as_mission_type_mut<T>(&'a self) -> Option<RefMut<'a, T>>
    where
        RefMut<'a, T>: TryFrom<SM>;
}

//
// Data
//

impl<'a, M> AsMissionType<'a, &'a MissionData> for M
where
    M: std::borrow::Borrow<MissionData> + 'a,
{
    fn as_mission_type<T>(&'a self) -> Option<Ref<'a, T>>
    where
        Ref<'a, T>: TryFrom<&'a MissionData>,
    {
        self.borrow().try_into().ok()
    }
}

impl<'a, M> AsMissionTypeMut<'a, &'a MissionData> for M
where
    M: std::borrow::Borrow<MissionData> + 'a,
{
    fn as_mission_type_mut<T>(&'a self) -> Option<RefMut<'a, T>>
    where
        RefMut<'a, T>: TryFrom<&'a MissionData>,
    {
        self.borrow().try_into().ok()
    }
}

//
// Option
//

impl<'a, M> AsMissionType<'a, &'a MissionData> for Option<M>
where
    M: std::borrow::Borrow<MissionData> + 'a,
{
    fn as_mission_type<T>(&'a self) -> Option<Ref<'a, T>>
    where
        Ref<'a, T>: TryFrom<&'a MissionData>,
    {
        self.as_ref().and_then(|m| m.borrow().try_into().ok())
    }
}

impl<'a, M> AsMissionTypeMut<'a, &'a MissionData> for Option<M>
where
    M: std::borrow::Borrow<MissionData> + 'a,
{
    fn as_mission_type_mut<T>(&'a self) -> Option<RefMut<'a, T>>
    where
        RefMut<'a, T>: TryFrom<&'a MissionData>,
    {
        self.as_ref().and_then(|m| m.borrow().try_into().ok())
    }
}

macro_rules! mission_type {
    ($mission:path, $mission_entry:path) => {
        impl<'a> TryFrom<&'a MissionData> for Ref<'a, $mission> {
            type Error = ();

            fn try_from(value: &'a MissionData) -> Result<Self, Self::Error> {
                if let $mission_entry(data) = value {
                    Ok(data.borrow())
                } else {
                    Err(())
                }
            }
        }

        impl<'a> TryFrom<&'a mut MissionData> for Ref<'a, $mission> {
            type Error = ();

            fn try_from(value: &'a mut MissionData) -> Result<Self, Self::Error> {
                if let $mission_entry(data) = value {
                    Ok(data.borrow())
                } else {
                    Err(())
                }
            }
        }

        impl<'a> TryFrom<&'a MissionData> for RefMut<'a, $mission> {
            type Error = ();

            fn try_from(value: &'a MissionData) -> Result<Self, Self::Error> {
                if let $mission_entry(data) = value {
                    Ok(data.borrow_mut())
                } else {
                    Err(())
                }
            }
        }

        impl<'a> TryFrom<&'a mut MissionData> for RefMut<'a, $mission> {
            type Error = ();

            fn try_from(value: &'a mut MissionData) -> Result<Self, Self::Error> {
                if let $mission_entry(data) = value {
                    Ok(data.borrow_mut())
                } else {
                    Err(())
                }
            }
        }
    };
}

mission_type!(super::localsupply::LocalSupplyMission, MissionData::LocalSupply);
mission_type!(super::upgrade::UpgradeMission, MissionData::Upgrade);
mission_type!(super::localbuild::LocalBuildMission, MissionData::LocalBuild);
mission_type!(super::tower::TowerMission, MissionData::Tower);
mission_type!(super::remotemine::RemoteMineMission, MissionData::RemoteMine);
mission_type!(super::scout::ScoutMission, MissionData::Scout);
mission_type!(super::construction::ConstructionMission, MissionData::Construction);
mission_type!(super::reserve::ReserveMission, MissionData::Reserve);
mission_type!(super::claim::ClaimMission, MissionData::Claim);
mission_type!(super::remotebuild::RemoteBuildMission, MissionData::RemoteBuild);
mission_type!(super::haul::HaulMission, MissionData::Haul);
mission_type!(super::terminal::TerminalMission, MissionData::Terminal);
mission_type!(super::miningoutpost::MiningOutpostMission, MissionData::MiningOutpost);
mission_type!(super::raid::RaidMission, MissionData::Raid);
mission_type!(super::dismantle::DismantleMission, MissionData::Dismantle);
mission_type!(super::colony::ColonyMission, MissionData::Colony);
mission_type!(super::defend::DefendMission, MissionData::Defend);
mission_type!(super::powerspawn::PowerSpawnMission, MissionData::PowerSpawn);
mission_type!(super::labs::LabsMission, MissionData::Labs);
