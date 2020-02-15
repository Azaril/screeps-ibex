use serde::*;
use screeps::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct StaticMineJob {
    pub mine_target: ObjectId<Source>,
    pub mine_location: RoomPosition
}

impl StaticMineJob
{
    pub fn new(source_id: ObjectId<Source>, position: RoomPosition) -> StaticMineJob {
        StaticMineJob {
            mine_target: source_id,
            mine_location: position
        }
    }
}

impl Job for StaticMineJob
{
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("StaticMine Job - {}", creep.name());

        //
        // Harvest energy from source.
        //

        //TODO: Validate container still exists? Recyle or reuse miner if it doesn't?

        if creep.pos().is_equal_to(&self.mine_location) {
            if let Some(source) = self.mine_target.resolve() {
                creep.harvest(&source);
            } else {
                error!("Harvester has no assigned harvesting source! Name: {}", creep.name());
            }
        } else {
            creep.move_to(&self.mine_location);
        }
    }
}
