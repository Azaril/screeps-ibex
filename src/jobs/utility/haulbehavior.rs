use crate::room::data::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use std::collections::HashMap;
#[cfg(feature = "time")]
use timing_annotate::*;

#[cfg_attr(feature = "time", timing)]
pub fn get_new_pickup_state<F, R>(
    creep: &Creep,
    pickup_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_queue: &mut TransferQueue,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket) -> R,
{
    let capacity = creep.store_capacity(None);
    let store_types = creep.store_types();
    let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
    //TODO: Fix this when _sum double count bug is fixed.
    //let free_capacity = creep.store_free_capacity(None);
    let free_capacity = capacity - used_capacity;
    
    //TODO: Need to pass in desired resources.

    if free_capacity > 0 {
        //TODO: This should be asking for energy only for some usages.
        let pickup_room_names = pickup_rooms.iter().map(|r| r.name).collect_vec();
        if let Some(pickup) = transfer_queue.select_pickup(&pickup_room_names, allowed_priorities, creep.pos(), free_capacity) {
            transfer_queue.register_pickup(&pickup);

            return Some(state_map(pickup));
        }
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn get_new_delivery_current_resources_state<F, R>(
    creep: &Creep,
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_queue: &mut TransferQueue,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferDepositTicket) -> R,
{
    let store_types = creep.store_types();
    let used_capacity: u32 = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum();
    //TODO: Use store used capacity when double counting resource bug if fixed. (Extra _sum field.)
    //let used_capacity = creep.store_used_capacity(None);

    if used_capacity > 0 {
        let store_info = store_types.into_iter().map(|r| (r, creep.store_of(r))).collect();
        let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();
        if let Some(delivery) = transfer_queue.select_delivery(&delivery_room_names, allowed_priorities, creep.pos(), &store_info) {
            transfer_queue.register_delivery(&delivery);

            return Some(state_map(delivery));
        }
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn run_pickup_state<F, R>(
    creep: &Creep,
    ticket: &mut TransferWithdrawTicket,
    transfer_queue: &mut TransferQueue,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    if !ticket.target().is_valid() || ticket.get_next_withdrawl().is_none() {
        return Some(next_state());
    }

    let capacity = creep.store_capacity(None);
    let store_types = creep.store_types();
    let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
    //let used_capacity = creep.store_used_capacity(None);
    let free_capacity = capacity - used_capacity;
    let available_capacity = free_capacity as i32 - ticket.resources().iter().map(|(_, a)| a).sum::<u32>() as i32;

    if available_capacity > 0 {
        if let Some(additional_ticket) = transfer_queue.request_additional_pickup(ticket, available_capacity as u32) {
            transfer_queue.register_pickup(&additional_ticket);

            ticket.combine_with(&additional_ticket);
        }
    }

    let pos = ticket.target().pos();

    if !creep.pos().is_near_to(&pos) {
        creep.move_to(&pos);

        return None;
    }

    loop {
        if let Some((resource, amount)) = ticket.get_next_withdrawl() {
            ticket.consume_withdrawl(resource, amount);

            if ticket.target().withdraw_resource_amount(creep, resource, amount) == ReturnCode::Ok {
                break None;
            }
        } else {
            break Some(next_state());
        }
    }
}

#[cfg_attr(feature = "time", timing)]
pub fn run_delivery_state<F, R>(
    creep: &Creep,
    ticket: &mut TransferDepositTicket,
    transfer_queue: &mut TransferQueue,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    if !ticket.target().is_valid() || ticket.get_next_deposit().is_none() {
        return Some(next_state());
    }

    let store_types = creep.store_types();
    let stored_resources = store_types.iter().map(|r| (r, creep.store_used_capacity(Some(*r))));

    let extra_resources: HashMap<_, _> = stored_resources
        .filter_map(|(store_resource, store_amount)| {
            let ticket_amount = ticket
                .resources()
                .get(store_resource)
                .map(|entries| entries.iter().map(|entry| entry.amount()).sum::<u32>())
                .unwrap_or(0);
            let extra_amount = (store_amount as i32) - (ticket_amount as i32);
            if store_amount > 0 {
                Some((*store_resource, extra_amount as u32))
            } else {
                None
            }
        })
        .collect();

    if !extra_resources.is_empty() {
        if let Some(additional_ticket) = transfer_queue.request_additional_delivery(ticket, &extra_resources) {
            transfer_queue.register_delivery(&additional_ticket);

            ticket.combine_with(&additional_ticket);
        }
    }

    let pos = ticket.target().pos();

    if !creep.pos().is_near_to(&pos) {
        creep.move_to(&pos);

        return None;
    }

    loop {
        if let Some((resource, amount)) = ticket.get_next_deposit() {
            ticket.consume_deposit(resource, amount);

            if ticket.target().transfer_resource_amount(creep, resource, amount) == ReturnCode::Ok {
                break None;
            }
        } else {
            break Some(next_state());
        }
    }
}
