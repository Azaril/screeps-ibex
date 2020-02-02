use serde::*;
use specs::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum JobData {
    Idle,
    Harvest(::jobs::harvest::HarvestJob)
}

impl Component for JobData {
    type Storage = VecStorage<Self>;
}