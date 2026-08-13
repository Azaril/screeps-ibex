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
    Scout(super::scout::ScoutOperation),
    War(super::war::WarOperation),
    Salvage(super::salvage::SalvageOperation),
    SourceKeeper(super::sourcekeeper::SourceKeeperOperation),
}

impl OperationData {
    pub fn as_operation(&mut self) -> &mut dyn Operation {
        match self {
            OperationData::MiningOutpost(ref mut data) => data,
            OperationData::Claim(ref mut data) => data,
            OperationData::Colony(ref mut data) => data,
            OperationData::Scout(ref mut data) => data,
            OperationData::War(ref mut data) => data,
            OperationData::Salvage(ref mut data) => data,
            OperationData::SourceKeeper(ref mut data) => data,
        }
    }

    /// Dispatch describe_operation to the concrete operation type (read-only).
    pub fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        match self {
            OperationData::MiningOutpost(ref data) => data.describe_operation(ctx),
            OperationData::Claim(ref data) => data.describe_operation(ctx),
            OperationData::Colony(ref data) => data.describe_operation(ctx),
            OperationData::Scout(ref data) => data.describe_operation(ctx),
            OperationData::War(ref data) => data.describe_operation(ctx),
            OperationData::Salvage(ref data) => data.describe_operation(ctx),
            OperationData::SourceKeeper(ref data) => data.describe_operation(ctx),
        }
    }
}

/// Typed access to a concrete operation inside `OperationData` — the
/// operation-side mirror of `mission_type!` (missions/data.rs). Spawn
/// callbacks and systems that must reach a specific operation's fields (e.g.
/// the `ScoutOperation` fleet roster attach, ADR 0046 D4/design-review
/// resolution #9) use `<&mut ConcreteOp>::try_from(&mut operation_data)`
/// instead of hand-rolled matches.
macro_rules! operation_type {
    ($operation:path, $operation_entry:path) => {
        impl<'a> TryFrom<&'a OperationData> for &'a $operation {
            type Error = ();

            fn try_from(value: &'a OperationData) -> Result<Self, Self::Error> {
                if let $operation_entry(data) = value {
                    Ok(data)
                } else {
                    Err(())
                }
            }
        }

        impl<'a> TryFrom<&'a mut OperationData> for &'a mut $operation {
            type Error = ();

            fn try_from(value: &'a mut OperationData) -> Result<Self, Self::Error> {
                if let $operation_entry(data) = value {
                    Ok(data)
                } else {
                    Err(())
                }
            }
        }
    };
}

operation_type!(super::miningoutpost::MiningOutpostOperation, OperationData::MiningOutpost);
operation_type!(super::claim::ClaimOperation, OperationData::Claim);
operation_type!(super::colony::ColonyOperation, OperationData::Colony);
operation_type!(super::scout::ScoutOperation, OperationData::Scout);
operation_type!(super::war::WarOperation, OperationData::War);
operation_type!(super::salvage::SalvageOperation, OperationData::Salvage);
operation_type!(super::sourcekeeper::SourceKeeperOperation, OperationData::SourceKeeper);
