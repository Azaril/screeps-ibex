use super::operationsystem::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, Component, ConvertSaveload)]
pub enum OperationData {
    MiningOutpost(super::miningoutpost::MiningOutpostOperation),
    Claim(super::claim::ClaimOperation),
    Colony(super::colony::ColonyOperation),
}

impl OperationData {
    pub fn as_operation(&mut self) -> &mut dyn Operation {
        match self {
            OperationData::MiningOutpost(ref mut data) => data,
            OperationData::Claim(ref mut data) => data,
            OperationData::Colony(ref mut data) => data,
        }
    }
}
