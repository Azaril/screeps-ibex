use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::operationsystem::*;

#[derive(Clone, Component, ConvertSaveload)]
pub enum OperationData {
    LocalSupply(super::localsupply::LocalSupplyOperation),
    Upgrade(super::upgrade::UpgradeOperation),
    LocalBuild(super::localbuild::LocalBuildOperation),
    Tower(super::tower::TowerOperation),
    RemoteMine(super::remotemine::RemoteMineOperation),
    Construction(super::construction::ConstructionOperation),
    Claim(super::claim::ClaimOperation),
}

impl OperationData {
    pub fn as_operation(&mut self) -> &mut dyn Operation {
        match self {
            OperationData::LocalSupply(ref mut data) => data,
            OperationData::Upgrade(ref mut data) => data,
            OperationData::LocalBuild(ref mut data) => data,
            OperationData::Tower(ref mut data) => data,
            OperationData::RemoteMine(ref mut data) => data,
            OperationData::Construction(ref mut data) => data,
            OperationData::Claim(ref mut data) => data,
        }
    }
}
