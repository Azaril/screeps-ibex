use crate::findnearest::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::jobsystem::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use std::collections::HashMap;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_pickup_state_fill_resource<F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_types: TransferTypeFlags,
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

        let pickups = transfer_queue.select_pickups(
            data,
            &pickup_room_names,
            allowed_priorities,
            transfer_types,
            &desired_resources,
            TransferCapacity::Infinite,
        );

        if let Some(pickup) = pickups
            .into_iter()
            .find_nearest_linear_by(creep.pos(), |ticket| ticket.target().pos())
        {
            transfer_queue.register_pickup(&pickup, TransferType::Haul);

            return Some(state_map(pickup));
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_delivery_current_resources_state<TF, F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_types: TransferTypeFlags,
    transfer_queue: &mut TransferQueue,
    target_filter: TF,
    state_map: F,
) -> Option<R>
where
    TF: Fn(&TransferTarget) -> bool,
    F: Fn(Vec<TransferDepositTicket>) -> R,
{
    let available_resources: HashMap<ResourceType, u32> = creep
        .store_types()
        .into_iter()
        .map(|r| (r, creep.store_used_capacity(Some(r))))
        .collect();
    let available_capacity = TransferCapacity::Finite(available_resources.values().sum());

    if !available_capacity.empty() {
        let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();

        let deliveries = transfer_queue.select_deliveries(
            data,
            &delivery_room_names,
            allowed_priorities,
            transfer_types,
            &available_resources,
            available_capacity,
            target_filter,
        );

        if let Some(delivery) = deliveries
            .into_iter()
            .find_nearest_linear_by(creep.pos(), |ticket| ticket.target().pos())
        {
            transfer_queue.register_delivery(&delivery, TransferType::Haul);

            let deliveries = vec![delivery];

            //TODO: Add multi-delivery expansion.

            return Some(state_map(deliveries));
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_pickup_and_delivery_state<TF, F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    allowed_secondary_priorities: TransferPriorityFlags,
    transfer_type: TransferType,
    available_capacity: TransferCapacity,
    transfer_queue: &mut TransferQueue,
    target_filter: TF,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket, Vec<TransferDepositTicket>) -> R,
    TF: Fn(&TransferTarget) -> bool + Copy,
{
    if !available_capacity.empty() {
        let pickup_room_names = pickup_rooms.iter().map(|r| r.name).collect_vec();
        let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();

        if let Some((mut pickup, delivery)) = transfer_queue.select_pickup_and_delivery(
            data,
            &pickup_room_names,
            &delivery_room_names,
            allowed_priorities,
            transfer_type,
            creep.pos(),
            available_capacity,
            target_filter,
        ) {
            transfer_queue.register_pickup(&pickup, transfer_type);
            transfer_queue.register_delivery(&delivery, transfer_type);

            let mut deliveries = vec![delivery];

            let mut remaining_capacity = available_capacity;

            for entries in pickup.resources().values() {
                for entry in entries {
                    remaining_capacity.consume(entry.amount());
                }
            }

            get_additional_deliveries(
                data,
                delivery_rooms,
                allowed_secondary_priorities,
                transfer_type,
                remaining_capacity,
                transfer_queue,
                &mut pickup,
                &mut deliveries,
                target_filter,
            );

            return Some(state_map(pickup, deliveries));
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_additional_deliveries<TF>(
    data: &dyn TransferRequestSystemData,
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_type: TransferType,
    available_capacity: TransferCapacity,
    transfer_queue: &mut TransferQueue,
    pickup: &mut TransferWithdrawTicket,
    deliveries: &mut Vec<TransferDepositTicket>,
    target_filter: TF,
) where
    TF: Fn(&TransferTarget) -> bool + Copy,
{
    if !available_capacity.empty() {
        let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();

        let mut remaining_capacity = available_capacity;

        let target_priorities = ALL_TRANSFER_PRIORITIES
            .iter()
            .map(|p| p.into())
            .filter(|p| allowed_priorities.contains(*p));

        for allowed_target_priorities in target_priorities {
            while !remaining_capacity.empty() {
                let last_delivery_pos = deliveries.last().unwrap().target().pos();

                //
                // NOTE: Pickup priority is ignored here as it's already known that the delivery priority is allowed. Additionally,
                //       the node is already being visited so it's worthwhile picking up any resource that can be transfered
                //       on the route.
                //

                let mut allowed_pickup_priorities = TransferPriorityFlags::ALL;

                if allowed_target_priorities.contains(TransferPriorityFlags::NONE) {
                    allowed_pickup_priorities.remove(TransferPriorityFlags::NONE);
                }

                //TODO: This should be multiple anchor points.
                if let Some((additional_pickup, additional_delivery)) = transfer_queue.get_delivery_from_target(
                    data,
                    &delivery_room_names,
                    pickup.target(),
                    allowed_pickup_priorities,
                    allowed_target_priorities,
                    transfer_type,
                    remaining_capacity,
                    last_delivery_pos,
                    target_filter,
                ) {
                    transfer_queue.register_pickup(&additional_pickup, transfer_type);
                    pickup.combine_with(&additional_pickup);

                    transfer_queue.register_delivery(&additional_delivery, transfer_type);

                    for entries in additional_pickup.resources().values() {
                        for entry in entries {
                            remaining_capacity.consume(entry.amount());
                        }
                    }

                    let mut merged_delivery = false;

                    for delivery in deliveries.iter_mut() {
                        if delivery.target() == additional_delivery.target() {
                            delivery.combine_with(&additional_delivery);

                            merged_delivery = true;

                            break;
                        }
                    }

                    if !merged_delivery {
                        deliveries.push(additional_delivery);

                        let start_pos = pickup.target().pos();

                        let mut destinations = std::mem::replace(deliveries, Vec::new());

                        while let Some(nearest_index) = destinations
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, delivery)| delivery.target().pos().get_range_to(&start_pos))
                            .map(|(index, _)| index)
                        {
                            deliveries.push(destinations.remove(nearest_index));
                        }
                    }
                } else {
                    break;
                }
            }

            if remaining_capacity.empty() {
                break;
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_pickup_and_delivery_full_capacity_state<TF, F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    allowed_secondary_priorities: TransferPriorityFlags,
    transfer_type: TransferType,
    transfer_queue: &mut TransferQueue,
    target_filter: TF,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket, Vec<TransferDepositTicket>) -> R,
    TF: Fn(&TransferTarget) -> bool + Copy,
{
    let capacity = creep.store_capacity(None);
    let store_types = creep.store_types();
    let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
    //let used_capacity = creep.store_used_capacity(None);
    let available_capacity = capacity - used_capacity;

    get_new_pickup_and_delivery_state(
        creep,
        data,
        pickup_rooms,
        delivery_rooms,
        allowed_priorities,
        allowed_secondary_priorities,
        transfer_type,
        TransferCapacity::Finite(available_capacity),
        transfer_queue,
        target_filter,
        state_map,
    )
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_pickup<F, R>(tick_context: &mut JobTickContext, ticket: &mut TransferWithdrawTicket, next_state: F) -> Option<R>
where
    F: FnOnce() -> R,
{
    //TODO: Use visibility to query if target should be visible.
    if !ticket.target().is_valid() || ticket.get_next_withdrawl().is_none() {
        return Some(next_state());
    }

    let creep = tick_context.runtime_data.owner;
    let action_flags = &mut tick_context.action_flags;
    let pos = ticket.target().pos();

    if !creep.pos().is_near_to(&pos) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, pos)
                .range(1);
        }

        return None;
    }

    loop {
        if let Some((resource, amount)) = ticket.get_next_withdrawl() {
            if !action_flags.intersects(SimultaneousActionFlags::TRANSFER) {
                ticket.consume_withdrawl(resource, amount);

                if ticket.target().withdraw_resource_amount(creep, resource, amount) == ReturnCode::Ok {
                    action_flags.insert(SimultaneousActionFlags::TRANSFER);
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_pickup_and_fill<F, R>(
    tick_context: &mut JobTickContext,
    ticket: &mut TransferWithdrawTicket,
    resource_type: ResourceType,
    transfer_types: TransferTypeFlags,
    priorities: TransferPriorityFlags,
    next_state: F,
) -> Option<R>
where
    F: FnOnce() -> R,
{
    //
    // NOTE: All users run this at the same time so that transfer data is only hydrated on this tick.
    //

    //TODO: Factor this in to common code.
    if game::time() % 5 == 0 {
        let creep = tick_context.runtime_data.owner;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Pickup Tick",
            room_data: &*tick_context.system_data.room_data,
        };

        let capacity = creep.store_capacity(None);
        let store_types = creep.store_types();
        let used_capacity = store_types.iter().map(|r| creep.store_used_capacity(Some(*r))).sum::<u32>();
        //TODO: Fix this when double resource counting bug is fixed.
        //let used_capacity = creep.store_used_capacity(None);
        let free_capacity = capacity - used_capacity;

        let mut available_capacity = TransferCapacity::Finite(free_capacity);

        for entries in ticket.resources().values() {
            for entry in entries {
                available_capacity.consume(entry.amount());
            }
        }

        if let Some(additional_withdrawl) = tick_context.runtime_data.transfer_queue.get_pickup_from_target(
            &transfer_queue_data,
            ticket.target(),
            priorities,
            transfer_types,
            available_capacity,
            resource_type,
        ) {
            ticket.combine_with(&additional_withdrawl);
        }
    }

    tick_pickup(tick_context, ticket, next_state)
}

pub fn visualize_pickup(describe_data: &mut JobDescribeData, ticket: &TransferWithdrawTicket) {
    let pos = describe_data.owner.pos();
    let to = ticket.target().pos();

    if pos.room_name() == to.room_name() {
        describe_data.visualizer.get_room(pos.room_name()).line(
            (pos.x() as f32, pos.y() as f32),
            (to.x() as f32, to.y() as f32),
            Some(LineStyle::default().color("blue")),
        );
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_delivery<F, R>(tick_context: &mut JobTickContext, tickets: &mut Vec<TransferDepositTicket>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    while let Some(ticket) = tickets.first_mut() {
        //TODO: Use visibility to query if target should be visible.
        if ticket.target().is_valid() && ticket.get_next_deposit().is_some() {
            let pos = ticket.target().pos();

            if !creep_pos.is_near_to(&pos) {
                if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(tick_context.runtime_data.creep_entity, pos)
                        .range(1);
                }

                return None;
            }

            while let Some((resource, amount)) = ticket.get_next_deposit() {
                if !tick_context.action_flags.intersects(SimultaneousActionFlags::TRANSFER) {
                    ticket.consume_deposit(resource, amount);

                    if ticket.target().creep_transfer_resource_amount(creep, resource, amount) == ReturnCode::Ok {
                        tick_context.action_flags.insert(SimultaneousActionFlags::TRANSFER);
                        break;
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

pub fn visualize_delivery(describe_data: &mut JobDescribeData, tickets: &Vec<TransferDepositTicket>) {
    let pos = describe_data.owner.pos();

    visualize_delivery_from(describe_data, tickets, pos);
}

pub fn visualize_delivery_from(describe_data: &mut JobDescribeData, tickets: &Vec<TransferDepositTicket>, from: RoomPosition) {
    let mut last_pos = from;

    for ticket in tickets.iter() {
        let delivery_pos = ticket.target().pos();

        if delivery_pos.room_name() != last_pos.room_name() {
            break;
        }

        describe_data.visualizer.get_room(delivery_pos.room_name()).line(
            (last_pos.x() as f32, last_pos.y() as f32),
            (delivery_pos.x() as f32, delivery_pos.y() as f32),
            Some(LineStyle::default().color("green")),
        );

        last_pos = delivery_pos;
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_deposit_all_resources_state<F, R>(tick_context: &mut JobTickContext, target: TransferTarget, next_state: F) -> Option<R>
where
    F: FnOnce() -> R,
{
    if target.is_valid() {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();

        let pos = target.pos();

        if !creep_pos.is_near_to(&pos) {
            if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                tick_context
                    .runtime_data
                    .movement
                    .move_to(tick_context.runtime_data.creep_entity, pos)
                    .range(1);
            }

            return None;
        }

        let store_types = creep.store_types();

        if let Some(resource) = store_types.first() {
            if tick_context.action_flags.consume(SimultaneousActionFlags::TRANSFER) {
                let amount = creep.store_used_capacity(Some(*resource));

                if target.creep_transfer_resource_amount(creep, *resource, amount) == ReturnCode::Ok {
                    if store_types.len() == 1 {
                        return Some(next_state());
                    } else {
                        return None;
                    }
                }
            } else {
                return None;
            }
        }
    }

    Some(next_state())
}
