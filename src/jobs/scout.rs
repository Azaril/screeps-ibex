use screeps::*;
use serde::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ScoutJob {
    pub room_target: RoomName,
}

impl ScoutJob {
    pub fn new(room_target: RoomName) -> ScoutJob {
        ScoutJob { room_target }
    }
}

impl Job for ScoutJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Scout Job - {}", creep.name());

        let creep_pos = creep.pos();
        let target_pos = Position::new(25, 25, self.room_target);

        if creep_pos.get_range_to(&target_pos) > 20 {
            creep.move_to(&target_pos);
        }
    }
}
