use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::prelude::*;
use specs::saveload::*;
use specs_derive::*;

#[derive(Clone, ConvertSaveload)]
pub enum OperationOrMissionEntity {
    Operation(Entity),
    Mission(Entity),
}
