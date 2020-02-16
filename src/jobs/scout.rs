use screeps::*;
use serde::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ScoutJob {
    pub room_target: RoomName,
}

impl ScoutJob {
    pub fn new(room_target: RoomName) -> ScoutJob {
        ScoutJob {
            room_target,
        }
    }
}

impl Job for ScoutJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Scout Job - {}", creep.name());

        creep.move_to(&Position::new(25, 25, self.room_target));
    }
}
