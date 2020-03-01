use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::jobsystem::*;
use super::utility::haulbehavior::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum HaulState {
    Idle,
    Pickup(TransferWithdrawTicket),
    FinishedPickup,
    Delivery(TransferDepositTicket),
    FinishedDelivery,
}

#[derive(Clone, ConvertSaveload)]
pub struct HaulJob {
    pub haul_rooms: EntityVec,
    pub state: HaulState,
}

impl HaulJob {
    pub fn new(haul_rooms: &[Entity]) -> HaulJob {
        HaulJob {
            haul_rooms: haul_rooms.into(),
            state: HaulState::Idle,
        }
    }

    fn run_idle_state(creep: &Creep, haul_rooms: &[&RoomData], transfer_queue: &mut TransferQueue) -> Option<HaulState> {
        ACTIVE_TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                get_new_delivery_current_resources_state(
                    creep,
                    haul_rooms,
                    TransferPriorityFlags::from(priority),
                    transfer_queue,
                    HaulState::Delivery,
                )
                .or_else(|| {
                    get_new_pickup_state(
                        creep,
                        haul_rooms,
                        TransferPriorityFlags::from(priority),
                        transfer_queue,
                        HaulState::Pickup,
                    )
                })
            })
            .next()
            .or_else(|| {
                get_new_delivery_current_resources_state(
                    creep,
                    haul_rooms,
                    TransferPriorityFlags::NONE,
                    transfer_queue,
                    HaulState::Delivery,
                )
            })
    }

    fn run_finished_pickup_state(creep: &Creep, haul_rooms: &[&RoomData], transfer_queue: &mut TransferQueue) -> Option<HaulState> {
        ACTIVE_TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                get_new_pickup_state(
                    creep,
                    haul_rooms,
                    TransferPriorityFlags::from(priority),
                    transfer_queue,
                    HaulState::Pickup,
                )
                .or_else(|| {
                    get_new_delivery_current_resources_state(
                        creep,
                        haul_rooms,
                        TransferPriorityFlags::from(priority),
                        transfer_queue,
                        HaulState::Delivery,
                    )
                })
            })
            .next()
            .or(Some(HaulState::Idle))
    }

    fn run_finished_delivery_state(creep: &Creep, haul_rooms: &[&RoomData], transfer_queue: &mut TransferQueue) -> Option<HaulState> {
        ACTIVE_TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                get_new_delivery_current_resources_state(
                    creep,
                    haul_rooms,
                    TransferPriorityFlags::from(priority),
                    transfer_queue,
                    HaulState::Delivery,
                )
                .or_else(|| {
                    get_new_pickup_state(
                        creep,
                        haul_rooms,
                        TransferPriorityFlags::from(priority),
                        transfer_queue,
                        HaulState::Pickup,
                    )
                })
            })
            .next()
            .or_else(|| {
                get_new_delivery_current_resources_state(
                    creep,
                    haul_rooms,
                    TransferPriorityFlags::NONE,
                    transfer_queue,
                    HaulState::Delivery,
                )
            })
            .or(Some(HaulState::Idle))
    }
}

impl Job for HaulJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        let pos = describe_data.owner.pos();

        if let Some(room) = describe_data.owner.room() {
            describe_data
                .ui
                .with_room(room.name(), &mut describe_data.visualizer, |room_ui| match &self.state {
                    HaulState::Idle => {
                        room_ui.jobs().add_text(format!("Haul - {} - Idle", name), None);
                    }
                    HaulState::Pickup(ticket) => {
                        room_ui.jobs().add_text(format!("Haul - {} - Pickup", name), None);

                        let to = ticket.target().pos();
                        room_ui.visualizer().line(
                            (pos.x() as f32, pos.y() as f32),
                            (to.x() as f32, to.y() as f32),
                            Some(LineStyle::default().color("blue")),
                        );
                    }
                    HaulState::FinishedPickup => {
                        room_ui.jobs().add_text(format!("Haul - {} - Finished Pickup", name), None);
                    }
                    HaulState::Delivery(ticket) => {
                        room_ui.jobs().add_text(format!("Haul - {} - Delivery", name), None);

                        let to = ticket.target().pos();
                        room_ui.visualizer().line(
                            (pos.x() as f32, pos.y() as f32),
                            (to.x() as f32, to.y() as f32),
                            Some(LineStyle::default().color("green")),
                        );
                    }
                    HaulState::FinishedDelivery => {
                        room_ui.jobs().add_text(format!("Haul - {} - Finished Delivery", name), None);
                    }
                })
        }
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            HaulState::Idle => {}
            HaulState::Pickup(ticket) => runtime_data.transfer_queue.register_pickup(&ticket),
            HaulState::FinishedPickup => {}
            HaulState::Delivery(ticket) => runtime_data.transfer_queue.register_delivery(&ticket),
            HaulState::FinishedDelivery => {}
        };
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("Haul Job - {}", creep.name());

        let haul_rooms = self.haul_rooms.0.iter().filter_map(|e| system_data.room_data.get(*e)).collect_vec();

        loop {
            let state_result = match &mut self.state {
                HaulState::Idle => Self::run_idle_state(creep, &haul_rooms, runtime_data.transfer_queue),
                HaulState::Pickup(ticket) => run_pickup_state(creep, ticket, runtime_data.transfer_queue, || HaulState::FinishedPickup),
                HaulState::FinishedPickup => Self::run_finished_pickup_state(creep, &haul_rooms, runtime_data.transfer_queue),
                HaulState::Delivery(ticket) => {
                    run_delivery_state(creep, ticket, runtime_data.transfer_queue, || HaulState::FinishedDelivery)
                }
                HaulState::FinishedDelivery => Self::run_finished_delivery_state(creep, &haul_rooms, runtime_data.transfer_queue),
            };

            if let Some(next_state) = state_result {
                self.state = next_state;
            } else {
                break;
            }
        }
    }
}
