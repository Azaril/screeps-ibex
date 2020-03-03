use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::jobsystem::*;
use super::utility::buildbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::repairbehavior::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum BuildState {
    Idle,
    Pickup(TransferWithdrawTicket),
    FinishedPickup,
    Harvest(RemoteObjectId<Source>),
    Build(RemoteObjectId<ConstructionSite>),
    FinishedBuild,
    Repair(RemoteStructureIdentifier),
    FinishedRepair,
}

#[derive(Clone, ConvertSaveload)]
pub struct BuildJob {
    home_room: Entity,
    build_room: Entity,
    state: BuildState,
}

impl BuildJob {
    pub fn new(home_room: Entity, build_room: Entity) -> BuildJob {
        BuildJob {
            home_room,
            build_room,
            state: BuildState::Idle,
        }
    }

    fn run_idle_state(creep: &Creep, build_room_data: &RoomData, transfer_queue: &mut TransferQueue) -> Option<BuildState> {
        get_new_build_state(creep, build_room_data, BuildState::Build)
            .or_else(|| get_new_repair_state(creep, build_room_data, None, BuildState::Repair))
            .or_else(|| {
                get_new_pickup_state(
                    creep,
                    &[build_room_data],
                    TransferPriorityFlags::ALL,
                    transfer_queue,
                    BuildState::Pickup,
                )
            })
            .or_else(|| get_new_harvest_state(creep, build_room_data, BuildState::Harvest))
    }

    fn run_finished_pickup_state(creep: &Creep, pickup_rooms: &[&RoomData], transfer_queue: &mut TransferQueue) -> Option<BuildState> {
        get_new_pickup_state(creep, pickup_rooms, TransferPriorityFlags::ALL, transfer_queue, BuildState::Pickup).or(Some(BuildState::Idle))
    }

    fn run_finished_build_state(creep: &Creep, build_room: &RoomData) -> Option<BuildState> {
        get_new_build_state(&creep, build_room, BuildState::Build).or(Some(BuildState::Idle))
    }

    fn run_finished_repair_state(creep: &Creep, repair_room: &RoomData) -> Option<BuildState> {
        get_new_repair_state(&creep, repair_room, None, BuildState::Repair).or(Some(BuildState::Idle))
    }
}

impl Job for BuildJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        let pos = describe_data.owner.pos();

        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                match &self.state {
                    BuildState::Idle => {
                        room_ui.jobs().add_text(format!("Build - {} - Idle", name), None);
                    }
                    BuildState::Pickup(ticket) => {
                        room_ui.jobs().add_text(format!("Build - {} - Pickup", name), None);

                        let to = ticket.target().pos();
                        room_ui.visualizer().line(
                            (pos.x() as f32, pos.y() as f32),
                            (to.x() as f32, to.y() as f32),
                            Some(LineStyle::default().color("blue")),
                        );
                    }
                    BuildState::FinishedPickup => {
                        room_ui.jobs().add_text(format!("Build - {} - FinishedPickup", name), None);
                    }
                    BuildState::Harvest(_) => {
                        room_ui.jobs().add_text(format!("Build - {} - Harvest", name), None);
                    }
                    BuildState::Build(_) => {
                        room_ui.jobs().add_text(format!("Build - {} - Build", name), None);
                    }
                    BuildState::FinishedBuild => {
                        room_ui.jobs().add_text(format!("Build - {} - FinishedBuild", name), None);
                    }
                    BuildState::Repair(_) => {
                        room_ui.jobs().add_text(format!("Build - {} - Repair", name), None);
                    }
                    BuildState::FinishedRepair => {
                        room_ui.jobs().add_text(format!("Build - {} - FinishedRepair", name), None);
                    }
                };
            })
        }
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            BuildState::Idle => {}
            BuildState::Pickup(ticket) => runtime_data.transfer_queue.register_pickup(&ticket),
            BuildState::FinishedPickup => {}
            BuildState::Harvest(_) => {}
            BuildState::Build(_) => {}
            BuildState::FinishedBuild => {}
            BuildState::Repair(_) => {}
            BuildState::FinishedRepair => {}
        };
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("Build Job - {}", creep.name());

        if let Some(build_room_data) = system_data.room_data.get(self.build_room) {
            loop {
                let state_result = match &mut self.state {
                    BuildState::Idle => Self::run_idle_state(creep, build_room_data, runtime_data.transfer_queue),
                    BuildState::Pickup(ticket) => {
                        run_pickup_state(creep, ticket, runtime_data.transfer_queue, || BuildState::FinishedPickup)
                    }
                    BuildState::FinishedPickup => Self::run_finished_pickup_state(creep, &[build_room_data], runtime_data.transfer_queue),
                    BuildState::Harvest(source_id) => run_harvest_state(creep, source_id, || BuildState::Idle),
                    BuildState::Build(construction_site_id) => run_build_state(creep, construction_site_id, || BuildState::FinishedBuild),
                    BuildState::FinishedBuild => Self::run_finished_build_state(creep, build_room_data),
                    BuildState::Repair(structure_id) => run_repair_state(creep, structure_id, || BuildState::FinishedRepair),
                    BuildState::FinishedRepair => Self::run_finished_repair_state(creep, build_room_data),
                };

                if let Some(next_state) = state_result {
                    self.state = next_state;
                } else {
                    break;
                }
            }
        }
    }
}
