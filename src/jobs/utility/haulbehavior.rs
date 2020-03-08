use crate::room::data::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use std::collections::HashMap;
#[cfg(feature = "time")]
use timing_annotate::*;
use findnearest::*;
use crate::jobs::actions::*;

#[cfg_attr(feature = "time", timing)]
pub fn get_new_pickup_state_fill_resource<F, R>(
    creep: &Creep,
    pickup_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    desired_resource: ResourceType,
    transfer_queue: &mut TransferQueue,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket) -> R,
{
    let capacity = creep.store_capacity(None);
    let store_types = creep.store_types();
    let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
    //TODO: Fix this when double resource counting bug is fixed.
    //let used_capacity = creep.store_used_capacity(None);
    let free_capacity = capacity - used_capacity;

    if free_capacity > 0 {
        let mut desired_resources = HashMap::new();

        desired_resources.insert(Some(desired_resource), free_capacity);

        let pickup_room_names = pickup_rooms.iter().map(|r| r.name).collect_vec();

        let pickups = transfer_queue.select_pickups(&pickup_room_names, allowed_priorities, &desired_resources, TransferCapacity::Infinite);

        if let Some(pickup) = pickups.into_iter().find_nearest_linear_by(creep.pos(), |ticket| ticket.target().pos()) {
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
    F: Fn(Vec<TransferDepositTicket>) -> R,
{
    let available_resources: HashMap<ResourceType, u32> = creep.store_types().into_iter().map(|r| (r, creep.store_of(r))).collect();
    let available_capacity = TransferCapacity::Finite(available_resources.values().sum());

    if !available_capacity.empty() {
        let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();

        let deliveries = transfer_queue.select_deliveries(&delivery_room_names, allowed_priorities, &available_resources, available_capacity);

        if let Some(delivery) = deliveries.into_iter().find_nearest_linear_by(creep.pos(), |ticket| ticket.target().pos()) {
            transfer_queue.register_delivery(&delivery);

            let deliveries = vec!(delivery);

            //TODO: Add multi-delivery expansion.

            return Some(state_map(deliveries));
        }
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn get_new_pickup_and_delivery_state<F, R>(
    creep: &Creep,
    pickup_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    available_capacity: TransferCapacity,
    transfer_queue: &mut TransferQueue,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket, Vec<TransferDepositTicket>) -> R,
{
    if !available_capacity.empty() {
        let pickup_room_names = pickup_rooms.iter().map(|r| r.name).collect_vec();

        if let Some((mut pickup, delivery)) = transfer_queue.select_pickup_and_delivery(&pickup_room_names, allowed_priorities, creep.pos(), available_capacity) {
            transfer_queue.register_pickup(&pickup);
            transfer_queue.register_delivery(&delivery);

            let mut deliveries = vec!(delivery);

            let mut remaining_capacity = available_capacity;

            for entries in pickup.resources().values() {
                for entry in entries {
                    remaining_capacity.consume(entry.amount());
                }
            }

            while !remaining_capacity.empty() {
                let last_delivery_pos = deliveries.last().unwrap().target().pos();

                if let Some((additional_pickup, additional_delivery)) = transfer_queue.get_additional_delivery_from_target(&pickup_room_names, pickup.target(), allowed_priorities, remaining_capacity, last_delivery_pos) {
                    transfer_queue.register_pickup(&additional_pickup);
                    pickup.combine_with(&additional_pickup);

                    transfer_queue.register_delivery(&additional_delivery);

                    deliveries.push(additional_delivery);

                    for entries in additional_pickup.resources().values() {
                        for entry in entries {
                            remaining_capacity.consume(entry.amount());
                        }
                    }
                } else {
                    break;
                }                
            }

            return Some(state_map(pickup, deliveries));
        }
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn get_new_pickup_and_delivery_full_capacity_state<F, R>(
    creep: &Creep,
    pickup_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_queue: &mut TransferQueue,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket, Vec<TransferDepositTicket>) -> R,
{
    let capacity = creep.store_capacity(None);
    let store_types = creep.store_types();
    let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
    //let used_capacity = creep.store_used_capacity(None);
    let available_capacity = capacity - used_capacity;

    get_new_pickup_and_delivery_state(creep, pickup_rooms, allowed_priorities, TransferCapacity::Finite(available_capacity), transfer_queue, state_map)
}

#[cfg_attr(feature = "time", timing)]
pub fn run_pickup_state<F, R>(
    creep: &Creep,
    action_flags: &mut SimultaneousActionFlags,
    ticket: &mut TransferWithdrawTicket,
    _transfer_queue: &mut TransferQueue,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    if !ticket.target().is_valid() || ticket.get_next_withdrawl().is_none() {
        return Some(next_state());
    }

    let pos = ticket.target().pos();

    if !creep.pos().is_near_to(&pos) {
        if !action_flags.contains(SimultaneousActionFlags::MOVE) {
            action_flags.insert(SimultaneousActionFlags::MOVE);
            creep.move_to(&pos);
        }

        return None;
    }

    loop {
        if let Some((resource, amount)) = ticket.get_next_withdrawl() {
            if !action_flags.contains(SimultaneousActionFlags::TRANSFER) {
                action_flags.insert(SimultaneousActionFlags::TRANSFER);

                ticket.consume_withdrawl(resource, amount);

                if ticket.target().withdraw_resource_amount(creep, resource, amount) == ReturnCode::Ok {
                    break None;
                }
            } else {
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
    action_flags: &mut SimultaneousActionFlags,
    tickets: &mut Vec<TransferDepositTicket>,
    _transfer_queue: &mut TransferQueue,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    while let Some(ticket) = tickets.first_mut() {
        if ticket.target().is_valid() && ticket.get_next_deposit().is_some() {
            let pos = ticket.target().pos();
        
            if !creep.pos().is_near_to(&pos) {
                if !action_flags.contains(SimultaneousActionFlags::MOVE) {
                    action_flags.insert(SimultaneousActionFlags::MOVE);
                    creep.move_to(&pos);
                }
        
                return None;
            }
        
            while let Some((resource, amount)) = ticket.get_next_deposit() {
                if !action_flags.contains(SimultaneousActionFlags::TRANSFER) {
                    action_flags.insert(SimultaneousActionFlags::TRANSFER);

                    ticket.consume_deposit(resource, amount);
        
                    if ticket.target().transfer_resource_amount(creep, resource, amount) == ReturnCode::Ok {
                        return None;
                    }
                } else {
                    return None;
                }
            }
        } else {
            tickets.remove(0);
        }
    }

    Some(next_state())
}