use screeps::*;
use serde::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct ScoutJob {
    pub room_target: RoomName,
}

impl ScoutJob {
    pub fn new(room_target: RoomName) -> ScoutJob {
        ScoutJob { room_target }
    }
}

impl Job for ScoutJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Scout - {}", name), None);
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("Scout Job - {}", creep.name());

        let creep_pos = creep.pos();
        let target_pos = Position::new(25, 25, self.room_target);

        //TODO: Handle stuck - it burns a lot of CPU.

        if creep_pos.get_range_to(&target_pos) > 20 {
            creep.move_to(&target_pos);
        }
    }
}
