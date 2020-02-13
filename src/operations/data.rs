use specs::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};

use super::operationsystem::*;

#[derive(Clone, Copy, Debug, Component, Serialize, Deserialize)]
pub enum OperationData {
    LocalSupply(super::localsupply::LocalSupplyOperation),
    Upgrade(super::upgrade::UpgradeOperation),
    LocalBuild(super::localbuild::LocalBuildOperation),
    Tower(super::tower::TowerOperation)
}

impl OperationData
{
    pub fn as_operation(&mut self) -> &mut dyn Operation
    {
        match self {
            OperationData::LocalSupply(ref mut data) => data,
            OperationData::Upgrade(ref mut data) => data,
            OperationData::LocalBuild(ref mut data) => data,
            OperationData::Tower(ref mut data) => data
        }
    }
}