use screeps::*;
use serde::*;
use std::collections::HashMap;

use super::jobsystem::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum HaulState {
    Idle,
    Pickup(TransferWithdrawTicket),
    FinishedPickup,
    Delivery(TransferDepositTicket),
    FinishedDelivery
}

#[derive(Clone, Deserialize, Serialize)]
pub struct HaulJob {
    pub haul_rooms: Vec<RoomName>,
    pub state: HaulState,
}

impl HaulJob {
    pub fn new(haul_rooms: &[RoomName]) -> HaulJob {
        HaulJob {
            haul_rooms: haul_rooms.to_vec(),
            state: HaulState::Idle,
        }
    }

    fn get_new_pickup(
        creep: &Creep,
        haul_rooms: &[RoomName],
        priority: TransferPriority,
        transfer_queue: &mut TransferQueue,
    ) -> Option<HaulState> {
        let available_capacity = creep.store_free_capacity(None);

        if available_capacity > 0 {
            if let Some(pickup) = transfer_queue.select_pickup(haul_rooms, priority, creep.pos(), available_capacity) {
                transfer_queue.register_pickup(&pickup);

                return Some(HaulState::Pickup(pickup));
            }
        }

        None
    }

    fn get_new_delivery(
        creep: &Creep,
        haul_rooms: &[RoomName],
        priority: TransferPriority,
        transfer_queue: &mut TransferQueue,
    ) -> Option<HaulState> {
        let store_types = creep.store_types();
        let used_capacity: u32 = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum();
        //TODO: Use store used capacity when double counting resource bug if fixed. (Extra _sum field.)
        //let used_capacity = creep.store_used_capacity(None);

        if used_capacity > 0 {
            let store_info = store_types.into_iter().map(|r| (r, creep.store_of(r))).collect();
            //TODO: Get delivery claim ticket before hauling? This would prevent getting stuck with resources in hand
            //      but nowhere to take them.
            if let Some(delivery) = transfer_queue.select_delivery(haul_rooms, priority, creep.pos(), &store_info) {
                transfer_queue.register_delivery(&delivery);

                return Some(HaulState::Delivery(delivery));
            }
        }

        None
    }

    fn run_idle_state(creep: &Creep, haul_rooms: &[RoomName], transfer_queue: &mut TransferQueue) -> StateResult<HaulState> {
        TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                Self::get_new_delivery(creep, haul_rooms, *priority, transfer_queue)
                    .or_else(|| Self::get_new_pickup(creep, haul_rooms, *priority, transfer_queue))
            })
            .map(|state| StateResult::NewState { state, tick_again: true })
            .next()
            .unwrap_or(StateResult::Continue)
    }

    fn run_pickup_state(
        creep: &Creep,
        ticket: &mut TransferWithdrawTicket,
        transfer_queue: &mut TransferQueue,
    ) -> StateResult<HaulState> {
        let capacity = creep.store_capacity(None);
        let store_types = creep.store_types();
        let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
        //let used_capacity = creep.store_used_capacity(None);
        let free_capacity = capacity - used_capacity;
        let available_capacity = free_capacity as i32 - ticket.resources().iter().map(|(_, a)| a).sum::<u32>() as i32;

        if available_capacity > 0 {
            if let Some(additional_ticket) = transfer_queue.request_additional_pickup(ticket, available_capacity as u32) {
                transfer_queue.register_pickup(ticket);

                ticket.combine_with(&additional_ticket);
            }
        }

        let pos = ticket.target().pos();

        if !creep.pos().is_near_to(&pos) {
            creep.move_to(&pos);

            return StateResult::Continue;
        }

        loop {
            if let Some((resource, amount)) = ticket.get_next_withdrawl() {
                ticket.consume_withdrawl(resource, amount);

                if let Some(structure) = ticket.target().as_structure() {
                    let withdraw_amount = if let Some(store) = structure.as_has_store() {
                        store.store_used_capacity(Some(resource)).min(amount)
                    } else {
                        amount
                    };

                    if let Some(withdrawable) = structure.as_withdrawable() {
                        if creep.withdraw_amount(withdrawable, resource, withdraw_amount) == ReturnCode::Ok {
                            break StateResult::Continue
                        }
                    }
                }
            } else {
                break StateResult::NewState {
                    state: HaulState::FinishedPickup,
                    tick_again: true,
                }
            }
        }
    }

    fn run_finished_pickup_state(
        creep: &Creep,
        haul_rooms: &[RoomName],
        transfer_queue: &mut TransferQueue,
    ) -> StateResult<HaulState> {
        TRANSFER_PRIORITIES
                .iter()
                .filter_map(|priority| Self::get_new_pickup(creep, haul_rooms, *priority, transfer_queue).or_else(|| Self::get_new_delivery(creep, haul_rooms, *priority, transfer_queue)))
                .map(|state| StateResult::NewState { state, tick_again: true })
                .next()
                .unwrap_or(StateResult::NewState {
                    state: HaulState::Idle,
                    tick_again: true,
                })
    }

    fn run_delivery_state(
        creep: &Creep,
        ticket: &mut TransferDepositTicket,
        transfer_queue: &mut TransferQueue,
    ) -> StateResult<HaulState> {
        let store_types = creep.store_types();
        let stored_resources = store_types.iter().map(|r| (r, creep.store_used_capacity(Some(*r))));

        let extra_resources: HashMap<_, _> = stored_resources.filter_map(|(store_resource, store_amount)| {
            let ticket_amount = ticket.resources().get(store_resource).map(|entries| entries.iter().map(|entry| entry.amount()).sum::<u32>()).unwrap_or(0);
            let extra_amount = (store_amount as i32) - (ticket_amount as i32);
            if store_amount > 0 {
                Some((*store_resource, extra_amount as u32))
            } else {
                None
            }
        }).collect();

        if !extra_resources.is_empty() {
            if let Some(additional_ticket) = transfer_queue.request_additional_delivery(ticket, &extra_resources) {
                transfer_queue.register_delivery(ticket);

                ticket.combine_with(&additional_ticket);
            }
        }

        let pos = ticket.target().pos();

        if !creep.pos().is_near_to(&pos) {
            creep.move_to(&pos);

            return StateResult::Continue;
        }

        loop {
            if let Some((resource, amount)) = ticket.get_next_deposit() {
                ticket.consume_deposit(resource, amount);

                if let Some(structure) = ticket.target().as_structure() {
                    let transfer_amount = if let Some(store) = structure.as_has_store() {
                        store.store_free_capacity(Some(resource)).min(amount)
                    } else {
                        amount
                    };

                    if let Some(transferable) = structure.as_transferable() {
                        if creep.transfer_amount(transferable, resource, transfer_amount) == ReturnCode::Ok {
                            break StateResult::Continue
                        }
                    }
                }
            } else {
                break StateResult::NewState {
                    state: HaulState::FinishedDelivery,
                    tick_again: true,
                }
            }
        }
    }

    fn run_finished_delivery_state(
        creep: &Creep,
        haul_rooms: &[RoomName],
        transfer_queue: &mut TransferQueue,
    ) -> StateResult<HaulState> {
        TRANSFER_PRIORITIES
                .iter()
                .filter_map(|priority| Self::get_new_delivery(creep, haul_rooms, *priority, transfer_queue).or_else(|| Self::get_new_pickup(creep, haul_rooms, *priority, transfer_queue)))
                .map(|state| StateResult::NewState { state, tick_again: true })
                .next()
                .unwrap_or(StateResult::NewState {
                    state: HaulState::Idle,
                    tick_again: true,
                })
    }
}

enum StateResult<T> {
    Continue,
    NewState { state: T, tick_again: bool },
}

impl Job for HaulJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        let pos = describe_data.owner.pos();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Haul - {}", name), None);

                match &self.state {
                    HaulState::Idle => {}
                    HaulState::Pickup(ticket) => {
                        let to = ticket.target().pos();
                        room_ui.visualizer().line(
                            (pos.x() as f32, pos.y() as f32),
                            (to.x() as f32, to.y() as f32),
                            Some(LineStyle::default().color("blue")),
                        );
                    },
                    HaulState::FinishedPickup => {}
                    HaulState::Delivery(ticket) => {
                        let to = ticket.target().pos();
                        room_ui.visualizer().line(
                            (pos.x() as f32, pos.y() as f32),
                            (to.x() as f32, to.y() as f32),
                            Some(LineStyle::default().color("green")),
                        );
                    },
                    HaulState::FinishedDelivery => {}
                }
            })
        }
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            HaulState::Idle => {}
            HaulState::Pickup(ticket) => runtime_data.transfer_queue.register_pickup(&ticket),
            HaulState::FinishedPickup => {},
            HaulState::Delivery(ticket) => runtime_data.transfer_queue.register_delivery(&ticket),
            HaulState::FinishedDelivery => {}
        };
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("Haul Job - {}", creep.name());

        let mut continue_tick = true;

        while continue_tick {
            let state_result = match &mut self.state {
                HaulState::Idle => Self::run_idle_state(creep, &self.haul_rooms, runtime_data.transfer_queue),
                HaulState::Pickup(ticket) => Self::run_pickup_state(creep, ticket, runtime_data.transfer_queue),
                HaulState::FinishedPickup => Self::run_finished_pickup_state(creep, &self.haul_rooms, runtime_data.transfer_queue),
                HaulState::Delivery(ticket) => Self::run_delivery_state(creep, ticket, runtime_data.transfer_queue),
                HaulState::FinishedDelivery => Self::run_finished_delivery_state(creep, &self.haul_rooms, runtime_data.transfer_queue)
            };

            continue_tick = match state_result {
                StateResult::Continue => false,
                StateResult::NewState { state, tick_again } => {
                    self.state = state;
                    tick_again
                }
            };
        }
    }
}
