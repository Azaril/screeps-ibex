use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::jobsystem::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;

// The pickup/delivery SELECTION policy (nearest-wins compositions, the tier-interleave
// pickup+delivery pairing, the per-node ticket construction) lives in
// `screeps_econ_decision::snapshot` since ADR 0040 M3 (K2 / ADR 0007 item 1) — one
// implementation, consumed by these adapters (through `TransferQueue`'s per-tick snapshot)
// AND the economy sim. The helpers below keep the FSM state construction + booking
// registration; the anchor-range pin tests moved with the kernel.

#[allow(clippy::too_many_arguments)]
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
    get_new_nearby_pickup_state_fill_resource(
        creep,
        data,
        pickup_rooms,
        allowed_priorities,
        transfer_types,
        desired_resource,
        transfer_queue,
        None,
        state_map,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_nearby_pickup_state_fill_resource<F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    transfer_types: TransferTypeFlags,
    desired_resource: ResourceType,
    transfer_queue: &mut TransferQueue,
    range_anchor: Option<(screeps::Position, u32)>,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket) -> R,
{
    // Safe on general stores (engine-mechanics folklore row 26).
    let free_capacity = creep.store().get_free_capacity(None).max(0) as u32;

    if free_capacity > 0 {
        let pickup_room_names = pickup_rooms.iter().map(|r| r.name).collect_vec();

        if let Some(pickup) = transfer_queue.select_nearest_pickup(
            data,
            &pickup_room_names,
            allowed_priorities,
            transfer_types,
            desired_resource,
            free_capacity,
            creep.pos(),
            range_anchor,
        ) {
            transfer_queue.register_pickup(&pickup);

            return Some(state_map(pickup));
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
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
    // Carried resources in store_types() order (the adapter-deterministic candidate order).
    let available_resources: Vec<(ResourceType, u32)> = creep
        .store()
        .store_types()
        .into_iter()
        .map(|r| (r, creep.store().get_used_capacity(Some(r))))
        .collect();
    let available_capacity = TransferCapacity::Finite(available_resources.iter().map(|(_, a)| a).sum());

    if !available_capacity.empty() {
        let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();

        if let Some(delivery) = transfer_queue.select_nearest_delivery(
            data,
            &delivery_room_names,
            allowed_priorities,
            transfer_types,
            &available_resources,
            available_capacity,
            creep.pos(),
            target_filter,
        ) {
            transfer_queue.register_delivery(&delivery);

            let deliveries = vec![delivery];

            //TODO: Add multi-delivery expansion.

            return Some(state_map(deliveries));
        }
    }

    None
}

/// ADR 0040 M5a — the LIVE bid-native HAUL-lane selection (the wiring the M5a-core slice left
/// undone). Runs the SHARED market kernel (`market_pass`, the same one the sim's MARKET tournament
/// arm delegates to) over this hauler, ranking (pickup, delivery) pairs by RAW bid-density instead
/// of the tier-interleave. Covers BOTH a loaded hauler (delivers carried cargo — an empty pickup
/// leg + a `Delivery`-shaped deposit) and an empty hauler (`Pickup` with paired deposits). The
/// caller registers the returned tickets and transitions into the mapped state. Returns `None`
/// when the market assigns nothing (drained lane / full creep) so the Idle cascade falls through
/// to the tier path (which keeps the crate tier-capable for the non-market lanes).
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_market_pickup_and_delivery_state<TF, PF, DF, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    delivery_rooms: &[&RoomData],
    transfer_queue: &mut TransferQueue,
    dist: &mut dyn crate::transfer::market_adapter::HaulDistance,
    target_filter: TF,
    pickup_state: PF,
    delivery_state: DF,
) -> Option<R>
where
    TF: Fn(&TransferTarget) -> bool,
    PF: Fn(TransferWithdrawTicket, Vec<TransferDepositTicket>) -> R,
    DF: Fn(Vec<TransferDepositTicket>) -> R,
{
    // Safe on general stores (engine-mechanics folklore row 26).
    let free_capacity = creep.store().get_free_capacity(None).max(0) as u32;
    let carried_energy = creep.store().get_used_capacity(Some(ResourceType::Energy));

    if free_capacity == 0 && carried_energy == 0 {
        return None;
    }

    let pickup_room_names = pickup_rooms.iter().map(|r| r.name).collect_vec();
    let delivery_room_names = delivery_rooms.iter().map(|r| r.name).collect_vec();

    let (pickup, delivery) = transfer_queue.select_market_pickup_and_delivery(
        data,
        &pickup_room_names,
        &delivery_room_names,
        TransferType::Haul,
        creep.pos(),
        free_capacity,
        carried_energy,
        dist,
        target_filter,
    )?;

    transfer_queue.register_delivery(&delivery);

    // A loaded hauler's pickup leg is empty (it already carries its cargo) — go straight to the
    // Delivery state; an empty hauler picks up first.
    if pickup.resources().is_empty() {
        Some(delivery_state(vec![delivery]))
    } else {
        transfer_queue.register_pickup(&pickup);
        Some(pickup_state(pickup, vec![delivery]))
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_pickup_and_delivery_state<TF, F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    allowed_secondary_priorities: TransferPriorityFlags,
    allowed_secondary_range: u32,
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
            transfer_queue.register_pickup(&pickup);
            transfer_queue.register_delivery(&delivery);

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
                allowed_secondary_range,
            );

            return Some(state_map(pickup, deliveries));
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
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
    additional_delivery_range: u32,
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
                let Some(last_delivery) = deliveries.last() else {
                    break;
                };
                let last_delivery_pos = last_delivery.target().local_pos();

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
                    |target| {
                        if target_filter(target) {
                            let target_pos = target.pos();

                            deliveries
                                .iter()
                                .any(|d| d.target().pos().get_range_to(&target_pos) <= additional_delivery_range)
                        } else {
                            false
                        }
                    },
                ) {
                    transfer_queue.register_pickup(&additional_pickup);
                    pickup.combine_with(&additional_pickup);

                    transfer_queue.register_delivery(&additional_delivery);

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

                        let mut destinations = std::mem::take(deliveries);

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

#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_pickup_and_delivery_full_capacity_state<TF, F, R>(
    creep: &Creep,
    data: &dyn TransferRequestSystemData,
    pickup_rooms: &[&RoomData],
    delivery_rooms: &[&RoomData],
    allowed_priorities: TransferPriorityFlags,
    allowed_secondary_priorities: TransferPriorityFlags,
    allowed_secondary_range: u32,
    transfer_type: TransferType,
    transfer_queue: &mut TransferQueue,
    target_filter: TF,
    state_map: F,
) -> Option<R>
where
    F: Fn(TransferWithdrawTicket, Vec<TransferDepositTicket>) -> R,
    TF: Fn(&TransferTarget) -> bool + Copy,
{
    // Safe on general stores (engine-mechanics folklore row 26).
    let available_capacity = creep.store().get_free_capacity(None).max(0) as u32;

    get_new_pickup_and_delivery_state(
        creep,
        data,
        pickup_rooms,
        delivery_rooms,
        allowed_priorities,
        allowed_secondary_priorities,
        allowed_secondary_range,
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
    let pos: screeps::Position = ticket.target().pos().into();

    if !creep.pos().is_near_to(pos) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            // Live w-as-priority (ADR 0033 §D5.4 hauler arm, ratified decision (4)): the pickup
            // leg bids its expected cargo rate in the (Low, Normal) numeric band. Every caller
            // of this helper is CIVILIAN (haul/harvest/upgrade/build — see `tick_delivery`'s
            // `bid_cargo_value` for the one military-shared seam).
            let bid = crate::pathing::value::haul_move_bid(creep, pos);
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, pos)
                .range(1)
                .priority_value(bid);
        }

        return None;
    }

    loop {
        if let Some((resource, amount)) = ticket.get_next_withdrawl() {
            if !action_flags.intersects(SimultaneousActionFlags::TRANSFER) {
                ticket.consume_withdrawl(resource, amount);

                if ticket.target().withdraw_resource_amount(creep, resource, amount).is_ok() {
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
    if game::time().is_multiple_of(5) {
        let creep = tick_context.runtime_data.owner;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Pickup Tick",
            room_data: tick_context.system_data.room_data,
        };

        // Safe on general stores (engine-mechanics folklore row 26).
        let free_capacity = creep.store().get_free_capacity(None).max(0) as u32;

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
            tick_context.runtime_data.transfer_queue.register_pickup(&additional_withdrawl);
            ticket.combine_with(&additional_withdrawl);
        }
    }

    tick_pickup(tick_context, ticket, next_state)
}

pub fn visualize_pickup(_describe_data: &mut JobDescribeData, _ticket: &TransferWithdrawTicket) {
    // Visualization is handled by the central RenderSystem.
}

/// `bid_cargo_value`: whether the delivery leg's move request bids the quantized §D5.4 cargo
/// rate on the numeric priority lane (ADR 0033 live adoption, ratified decision (4)). CIVILIAN
/// callers (haul/harvest) pass `true`; the MILITARY caller (dismantle — salvage delivery) passes
/// `false` and keeps its enum tier THIS pass: military w needs war-layer objective EV, frozen
/// with operations/war.rs (unblock-after-merge).
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_delivery<F, R>(
    tick_context: &mut JobTickContext,
    tickets: &mut Vec<TransferDepositTicket>,
    bid_cargo_value: bool,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut transfered = false;

    while let Some(ticket) = tickets.first_mut() {
        //TODO: Use visibility to query if target should be visible.
        if ticket.target().is_valid() && ticket.get_next_deposit().is_some() {
            let pos: screeps::Position = ticket.target().pos().into();

            if !creep_pos.is_near_to(pos) {
                if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                    let mut builder = tick_context
                        .runtime_data
                        .movement
                        .move_to(tick_context.runtime_data.creep_entity, pos);
                    builder.range(1);
                    if bid_cargo_value {
                        builder.priority_value(crate::pathing::value::haul_move_bid(creep, pos));
                    }
                }

                return None;
            }

            // Confirm-then-consume (ADR 0007 Q5 item 3 via ADR 0040 M3): the ticket entry is
            // consumed only AFTER the transfer intent is accepted, so a rejected transfer no
            // longer transiently mis-accounts the ticket. A REJECTED resource consumes only its
            // own entries (deliberate abandonment, not the mis-account window item 3 closed) and
            // the loop retries the ticket's remaining resources same-tick — the pre-M3
            // entry-by-entry drain semantics for the multi-resource case (a whole-ticket drop
            // stalled a mixed-cargo hauler for the full wait backoff — M3 review finding); the
            // drained ticket is removed by the get_next_deposit guard above.
            if let Some((resource, amount)) = ticket.get_next_deposit() {
                if !tick_context.action_flags.intersects(SimultaneousActionFlags::TRANSFER) {
                    if ticket.target().creep_transfer_resource_amount(creep, resource, amount).is_ok() {
                        ticket.consume_deposit(resource, amount);
                        tick_context.action_flags.insert(SimultaneousActionFlags::TRANSFER);

                        transfered = true;
                    } else {
                        ticket.consume_deposit(resource, amount);
                    }
                } else {
                    return None;
                }
            }
        } else {
            tickets.remove(0);
        }
    }

    if transfered {
        //
        // NOTE: Delay further execution by a tick as inventory cannot be trusted. (Needs predicted storage that can be shared across systems.)
        //

        None
    } else {
        Some(next_state())
    }
}

pub fn visualize_delivery(_describe_data: &mut JobDescribeData, _tickets: &[TransferDepositTicket]) {
    // Visualization is handled by the central RenderSystem.
}

pub fn visualize_delivery_from(_describe_data: &mut JobDescribeData, _tickets: &[TransferDepositTicket], _from: RoomPosition) {
    // Visualization is handled by the central RenderSystem.
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_deposit_all_resources_state<F, R>(tick_context: &mut JobTickContext, target: TransferTarget, next_state: F) -> Option<R>
where
    F: FnOnce() -> R,
{
    if target.is_valid() {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();

        let pos: screeps::Position = target.pos().into();

        if !creep_pos.is_near_to(pos) {
            if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                // Sole caller is link-mine (CIVILIAN) dumping to its link/container: bid the
                // carried-cargo rate like every other haul leg (decision (4)).
                let bid = crate::pathing::value::haul_move_bid(creep, pos);
                tick_context
                    .runtime_data
                    .movement
                    .move_to(tick_context.runtime_data.creep_entity, pos)
                    .range(1)
                    .priority_value(bid);
            }

            return None;
        }

        let store_types = creep.store().store_types();

        if let Some(resource) = store_types.first() {
            if tick_context.action_flags.consume(SimultaneousActionFlags::TRANSFER) {
                let amount = creep.store().get_used_capacity(Some(*resource));

                if target.creep_transfer_resource_amount(creep, *resource, amount).is_ok() {
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

// The anchor-range pin tests MOVED with the kernel to
// `screeps_econ_decision::snapshot` (nearest_pickup_honors_anchor — ADR 0040 M3).
