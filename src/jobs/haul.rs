use super::actions::*;
use super::jobsystem::*;
use super::utility::haulbehavior::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum HaulState {
    Idle,
    Pickup(TransferWithdrawTicket, Vec<TransferDepositTicket>),
    Delivery(Vec<TransferDepositTicket>),
}

#[derive(Clone, ConvertSaveload)]
pub struct HaulJob {
    pub haul_rooms: EntityVec,
    pub state: HaulState,
}

#[cfg_attr(feature = "time", timing)]
impl HaulJob {
    #[cfg_attr(feature = "time", timing)]
    pub fn new(haul_rooms: &[Entity]) -> HaulJob {
        HaulJob {
            haul_rooms: haul_rooms.into(),
            state: HaulState::Idle,
        }
    }

    #[cfg_attr(feature = "time", timing)]
    fn run_idle_state(creep: &Creep, haul_rooms: &[&RoomData], transfer_queue: &mut TransferQueue) -> Option<HaulState> {
        get_new_delivery_current_resources_state(
            creep,
            haul_rooms,
            TransferPriorityFlags::ACTIVE,
            transfer_queue,
            HaulState::Delivery,
        )
        .or_else(|| {
            get_new_delivery_current_resources_state(creep, haul_rooms, TransferPriorityFlags::NONE, transfer_queue, HaulState::Delivery)
        })
        .or_else(|| {
            ACTIVE_TRANSFER_PRIORITIES
                .iter()
                .filter_map(|priority| {
                    get_new_pickup_and_delivery_full_capacity_state(
                        creep,
                        haul_rooms,
                        TransferPriorityFlags::from(priority),
                        transfer_queue,
                        HaulState::Pickup,
                    )
                })
                .next()
        })
    }
}

#[cfg_attr(feature = "time", timing)]
impl Job for HaulJob {
    #[cfg_attr(feature = "time", timing)]
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
                    HaulState::Pickup(pickup_ticket, delivery_tickets) => {
                        room_ui.jobs().add_text(format!("Haul - {} - Pickup", name), None);

                        if crate::features::transfer::visualize_haul() {
                            let pickup_pos = pickup_ticket.target().pos();
                            room_ui.visualizer().line(
                                (pos.x() as f32, pos.y() as f32),
                                (pickup_pos.x() as f32, pickup_pos.y() as f32),
                                Some(LineStyle::default().color("blue")),
                            );

                            let mut last_pos = pickup_pos;
                            for delivery_ticket in delivery_tickets.iter() {
                                let delivery_pos = delivery_ticket.target().pos();
                                room_ui.visualizer().line(
                                    (last_pos.x() as f32, last_pos.y() as f32),
                                    (delivery_pos.x() as f32, delivery_pos.y() as f32),
                                    Some(LineStyle::default().color("green")),
                                );
                                last_pos = delivery_pos;
                            }
                        }
                    }
                    HaulState::Delivery(delivery_tickets) => {
                        room_ui.jobs().add_text(format!("Haul - {} - Delivery", name), None);

                        if crate::features::transfer::visualize_haul() {
                            let mut last_pos = pos;
                            for delivery_ticket in delivery_tickets.iter() {
                                let delivery_pos = delivery_ticket.target().pos();
                                room_ui.visualizer().line(
                                    (last_pos.x() as f32, last_pos.y() as f32),
                                    (delivery_pos.x() as f32, delivery_pos.y() as f32),
                                    Some(LineStyle::default().color("green")),
                                );
                                last_pos = delivery_pos;
                            }
                        }
                    }
                })
        }
    }

    #[cfg_attr(feature = "time", timing)]
    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            HaulState::Idle => {}
            HaulState::Pickup(pickup_ticket, delivery_tickets) => {
                runtime_data.transfer_queue.register_pickup(&pickup_ticket);
                for delivery_ticket in delivery_tickets.iter() {
                    runtime_data.transfer_queue.register_delivery(&delivery_ticket);
                }
            }
            HaulState::Delivery(delivery_tickets) => {
                for delivery_ticket in delivery_tickets.iter() {
                    runtime_data.transfer_queue.register_delivery(&delivery_ticket);
                }
            }
        };
    }

    #[cfg_attr(feature = "time", timing)]
    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        let haul_rooms = self.haul_rooms.0.iter().filter_map(|e| system_data.room_data.get(*e)).collect_vec();

        let mut action_flags = SimultaneousActionFlags::UNSET;

        loop {
            let state_result = match &mut self.state {
                HaulState::Idle => Self::run_idle_state(creep, &haul_rooms, runtime_data.transfer_queue),
                HaulState::Pickup(pickup_ticket, delivery_tickets) => {
                    run_pickup_state(creep, &mut action_flags, pickup_ticket, runtime_data.transfer_queue, || {
                        HaulState::Delivery(delivery_tickets.clone())
                    })
                }
                HaulState::Delivery(delivery_tickets) => {
                    run_delivery_state(creep, &mut action_flags, delivery_tickets, runtime_data.transfer_queue, || {
                        HaulState::Idle
                    })
                }
            };

            if let Some(next_state) = state_result {
                self.state = next_state;
            } else {
                break;
            }
        }
    }
}
