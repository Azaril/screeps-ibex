use specs::*;
use serde::*;

use super::operationsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum OperationData {
    Bootstrap(super::bootstrap::BootstrapOperation)
}

impl Component for OperationData {
    type Storage = HashMapStorage<Self>;
}

impl OperationData
{
    pub fn as_operation(&mut self) -> &mut dyn Operation
    {
        match self {
            OperationData::Bootstrap(ref mut data) => data
        }
    }
}