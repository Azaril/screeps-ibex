use super::jobsystem::*;
use screeps::*;
use serde::*;

#[derive(Clone, Deserialize, Serialize)]
pub struct ScoutJob {
    room_target: RoomName,
    #[serde(default)]
    room_history: Vec<RoomName>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutJob {
    pub fn new(room_target: RoomName) -> ScoutJob {
        ScoutJob {
            room_target,
            room_history: Vec::new(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
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

        let creep_pos = creep.pos();
        let target_pos = Position::new(25, 25, self.room_target);

        //TODO: Need better stuck detection.
        if self.room_history.last().map(|r| *r != creep_pos.room_name()).unwrap_or(true) {
            self.room_history.push(creep_pos.room_name());

            if self.room_history.iter().filter(|r| **r == creep_pos.room_name()).count() > 4 {
                creep.suicide();

                return;
            }
        }

        if creep_pos.get_range_to(&target_pos) > 20 {
            runtime_data.movement.move_to_range(runtime_data.creep_entity, target_pos, 20);
        }
    }
}
