use super::actions::*;
use super::jobsystem::*;
use super::utility::repair::*;
use super::utility::buildbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum BuildState {
    Idle(),
    Pickup(TransferWithdrawTicket),
    FinishedPickup(),
    Harvest(RemoteObjectId<Source>, u8),
    Build(RemoteObjectId<ConstructionSite>),
    FinishedBuild(),
    Repair(RemoteStructureIdentifier),
    FinishedRepair(),
    Wait(u32)
}

#[derive(Clone, ConvertSaveload)]
pub struct BuildJob {
    home_room: Entity,
    build_room: Entity,
    state: BuildState,
    allow_harvest: bool,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl BuildJob {
    pub fn new(home_room: Entity, build_room: Entity, allow_harvest: bool) -> BuildJob {
        BuildJob {
            home_room,
            build_room,
            state: BuildState::Idle(),
            allow_harvest
        }
    }

    fn run_idle_state(creep: &Creep, build_room_data: &RoomData, transfer_queue: &mut TransferQueue, allow_harvest: bool) -> Option<BuildState> {
        get_new_repair_state(creep, build_room_data, Some(RepairPriority::High), BuildState::Repair)
            .or_else(|| get_new_build_state(creep, build_room_data, BuildState::Build))
            .or_else(|| get_new_repair_state(creep, build_room_data, None, BuildState::Repair))
            .or_else(|| {
                get_new_pickup_state_fill_resource(
                    creep,
                    &[build_room_data],
                    TransferPriorityFlags::ALL,
                    TransferTypeFlags::HAUL | TransferTypeFlags::USE,
                    ResourceType::Energy,
                    transfer_queue,
                    BuildState::Pickup,
                )
            })
            .or_else(|| if allow_harvest {
                get_new_harvest_state(creep, build_room_data, |id| BuildState::Harvest(id, 0))
            } else {
                None
            })
            .or_else(|| Some(BuildState::Wait(5)))
    }

    fn run_finished_pickup_state(creep: &Creep, pickup_rooms: &[&RoomData], transfer_queue: &mut TransferQueue) -> Option<BuildState> {
        get_new_pickup_state_fill_resource(
            creep,
            pickup_rooms,
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            transfer_queue,
            BuildState::Pickup,
        )
        .or(Some(BuildState::Idle()))
    }

    fn run_finished_build_state(creep: &Creep, build_room: &RoomData) -> Option<BuildState> {
        get_new_build_state(&creep, build_room, BuildState::Build).or(Some(BuildState::Idle()))
    }

    fn run_finished_repair_state(creep: &Creep, repair_room: &RoomData) -> Option<BuildState> {
        get_new_repair_state(&creep, repair_room, None, BuildState::Repair).or(Some(BuildState::Idle()))
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for BuildJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        let pos = describe_data.owner.pos();

        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                match &self.state {
                    BuildState::Idle() => {
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
                    BuildState::FinishedPickup() => {
                        room_ui.jobs().add_text(format!("Build - {} - FinishedPickup", name), None);
                    }
                    BuildState::Harvest(_, _) => {
                        room_ui.jobs().add_text(format!("Build - {} - Harvest", name), None);
                    }
                    BuildState::Build(_) => {
                        room_ui.jobs().add_text(format!("Build - {} - Build", name), None);
                    }
                    BuildState::FinishedBuild() => {
                        room_ui.jobs().add_text(format!("Build - {} - FinishedBuild", name), None);
                    }
                    BuildState::Repair(_) => {
                        room_ui.jobs().add_text(format!("Build - {} - Repair", name), None);
                    }
                    BuildState::FinishedRepair() => {
                        room_ui.jobs().add_text(format!("Build - {} - FinishedRepair", name), None);
                    }
                    BuildState::Wait(_) => {
                        room_ui.jobs().add_text(format!("Build - {} - Wait", name), None);
                    }
                };
            })
        }
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            BuildState::Idle() => {}
            BuildState::Pickup(ticket) => runtime_data.transfer_queue.register_pickup(&ticket, TransferType::Haul),
            BuildState::FinishedPickup() => {}
            BuildState::Harvest(_, _) => {}
            BuildState::Build(_) => {}
            BuildState::FinishedBuild() => {}
            BuildState::Repair(_) => {}
            BuildState::FinishedRepair() => {}
            BuildState::Wait(_) => {}
        };
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        let mut action_flags = SimultaneousActionFlags::UNSET;

        if let Some(build_room_data) = system_data.room_data.get(self.build_room) {
            loop {
                let state_result = match &mut self.state {
                    BuildState::Idle() => Self::run_idle_state(creep, build_room_data, runtime_data.transfer_queue, self.allow_harvest),
                    BuildState::Pickup(ticket) => run_pickup_state(creep, &mut action_flags, ticket, runtime_data.transfer_queue, BuildState::FinishedPickup),
                    BuildState::FinishedPickup() => Self::run_finished_pickup_state(creep, &[build_room_data], runtime_data.transfer_queue),
                    BuildState::Harvest(source_id, stuck_count) => run_harvest_state(creep, &mut action_flags, source_id, false, stuck_count, BuildState::Idle),
                    BuildState::Build(construction_site_id) => run_build_state(creep, &mut action_flags, construction_site_id, BuildState::FinishedBuild),
                    BuildState::FinishedBuild() => Self::run_finished_build_state(creep, build_room_data),
                    BuildState::Repair(structure_id) => run_repair_state(creep, &mut action_flags, structure_id, BuildState::FinishedRepair),
                    BuildState::FinishedRepair() => Self::run_finished_repair_state(creep, build_room_data),
                    BuildState::Wait(time) => run_wait_state(time, BuildState::Idle)
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
