use super::actions::*;
use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum UpgradeState {
    Idle(),
    Harvest(RemoteObjectId<Source>, u8),
    Pickup(TransferWithdrawTicket),
    FinishedPickup(),
    Upgrade(RemoteObjectId<StructureController>),
    Wait(u32)
}

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeJob {
    home_room: Entity,
    state: UpgradeState,
    allow_harvest: bool
}

#[cfg_attr(feature = "time", timing)]
impl UpgradeJob {
    pub fn new(home_room: Entity, allow_harvest: bool) -> UpgradeJob {
        UpgradeJob {
            home_room,
            state: UpgradeState::Idle(),
            allow_harvest
        }
    }

    fn run_idle_state(creep: &Creep, home_room_data: &RoomData, transfer_queue: &mut TransferQueue, allow_harvest: bool) -> Option<UpgradeState> {
        get_new_pickup_state_fill_resource(
            creep,
            &[home_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            transfer_queue,
            UpgradeState::Pickup,
        )
        .or_else(|| if allow_harvest {
            get_new_harvest_state(creep, home_room_data, |id| UpgradeState::Harvest(id, 0))
        } else {
            None
        })
        .or_else(|| get_new_upgrade_state(creep, home_room_data, UpgradeState::Upgrade))
        .or_else(|| Some(UpgradeState::Wait(5)))
    }

    fn run_finished_pickup_state(creep: &Creep, delivery_room_data: &RoomData, transfer_queue: &mut TransferQueue) -> Option<UpgradeState> {
        get_new_pickup_state_fill_resource(
            creep,
            &[delivery_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            transfer_queue,
            UpgradeState::Pickup,
        )
        .or_else(|| Some(UpgradeState::Idle()))
    }
}

#[cfg_attr(feature = "time", timing)]
impl Job for UpgradeJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        let pos = describe_data.owner.pos();

        if let Some(room) = describe_data.owner.room() {
            describe_data
                .ui
                .with_room(room.name(), &mut describe_data.visualizer, |room_ui| match &self.state {
                    UpgradeState::Idle() => {
                        room_ui.jobs().add_text(format!("Upgrade - {} - Idle", name), None);
                    }
                    UpgradeState::Harvest(_, _) => {
                        room_ui.jobs().add_text(format!("Upgrade - {} - Harvest", name), None);
                    },
                    UpgradeState::Pickup(ticket) => {
                        room_ui.jobs().add_text(format!("Upgrade - {} - Pickup", name), None);

                        let to = ticket.target().pos();
                        room_ui.visualizer().line(
                            (pos.x() as f32, pos.y() as f32),
                            (to.x() as f32, to.y() as f32),
                            Some(LineStyle::default().color("blue")),
                        );
                    },
                    UpgradeState::FinishedPickup() => {
                        room_ui.jobs().add_text(format!("Upgrade - {} - FinishedPickup", name), None);
                    },
                    UpgradeState::Upgrade(_) => {
                        room_ui.jobs().add_text(format!("Upgrade - {} - Upgrade                                                                                                                                                                    ", name), None);
                    }
                    UpgradeState::Wait(_) => {
                        room_ui.jobs().add_text(format!("Upgrade - {} - Wait                                                                                                                                                                    ", name), None);
                    }
                })
        }
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            UpgradeState::Idle() => {}
            UpgradeState::Harvest(_, _) => {}
            UpgradeState::Pickup(ticket) => runtime_data.transfer_queue.register_pickup(&ticket, TransferType::Haul),
            UpgradeState::FinishedPickup() => {}
            UpgradeState::Upgrade(_) => {}
            UpgradeState::Wait(_) => {}
        };
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        let mut action_flags = SimultaneousActionFlags::UNSET;

        if let Some(home_room_data) = system_data.room_data.get(self.home_room) {
            loop {
                let state_result = match &mut self.state {
                    UpgradeState::Idle() => Self::run_idle_state(creep, home_room_data, runtime_data.transfer_queue, self.allow_harvest),
                    UpgradeState::Harvest(source_id, stuck_count) => run_harvest_state(creep, &mut action_flags, source_id, false, stuck_count, UpgradeState::Idle),
                    UpgradeState::Pickup(ticket) => run_pickup_state(creep, &mut action_flags, ticket, runtime_data.transfer_queue, UpgradeState::FinishedPickup),
                    UpgradeState::FinishedPickup() => Self::run_finished_pickup_state(creep, home_room_data, runtime_data.transfer_queue),
                    UpgradeState::Upgrade(controller_id) => run_upgrade_state(creep, controller_id, UpgradeState::Idle),
                    UpgradeState::Wait(time) => run_wait_state(time, UpgradeState::Idle)
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
