use super::operationsystem::*;
use crate::visualization::SummaryContent;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, Component, ConvertSaveload)]
pub enum OperationData {
    MiningOutpost(super::miningoutpost::MiningOutpostOperation),
    Claim(super::claim::ClaimOperation),
    Colony(super::colony::ColonyOperation),
    Defense(super::defense::DefenseOperation),
    Attack(super::attack::AttackOperation),
    Scout(super::scout::ScoutOperation),
}

impl OperationData {
    pub fn as_operation(&mut self) -> &mut dyn Operation {
        match self {
            OperationData::MiningOutpost(ref mut data) => data,
            OperationData::Claim(ref mut data) => data,
            OperationData::Colony(ref mut data) => data,
            OperationData::Defense(ref mut data) => data,
            OperationData::Attack(ref mut data) => data,
            OperationData::Scout(ref mut data) => data,
        }
    }

    /// Dispatch describe_operation to the concrete operation type (read-only).
    pub fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        match self {
            OperationData::MiningOutpost(ref data) => data.describe_operation(ctx),
            OperationData::Claim(ref data) => data.describe_operation(ctx),
            OperationData::Colony(ref data) => data.describe_operation(ctx),
            OperationData::Defense(ref data) => data.describe_operation(ctx),
            OperationData::Attack(ref data) => data.describe_operation(ctx),
            OperationData::Scout(ref data) => data.describe_operation(ctx),
        }
    }
}
