use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, Eq, PartialEq, ConvertSaveload)]
pub enum OperationOrMissionEntity {
    Operation(Entity),
    Mission(Entity),
}
