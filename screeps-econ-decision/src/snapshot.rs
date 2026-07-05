//! K2 — the **TransferSnapshot** and the pure pickup/delivery selection kernels (ADR 0007 Q5
//! item 1, delivered via ADR 0040 M3 reconciliation R1). MOVED (ported) at M3 from
//! `screeps-ibex/src/transfer/transfersystem.rs` (`TransferNode::select_pickup`/
//! `select_delivery`/`select_single_delivery`, `TransferQueue::select_pickups`/
//! `select_deliveries`/`select_best_delivery`/`select_pickup_and_delivery`/`get_delivery`/
//! `get_delivery_from_target`/`get_pickup_from_target`/`total_unfufilled_resources`) and
//! `screeps-ibex/src/jobs/utility/haulbehavior.rs` (the nearest-wins compositions). Lives here
//! now, consumed by the bot (the `TransferQueue` adapter builds the snapshot once per tick at
//! the top of the hauling pass and keeps the booking layer) and by the sim
//! (`screeps-econ-eval::baseline`, whose transcribed selection kernels are deleted).
//!
//! **The model** (0007 item 1): the snapshot is the IMMUTABLE per-tick image of the *requested*
//! transfer demand (per node, per `(resource, priority, transfer-type)` key). Bookings (the live
//! `pending_*` reservation maps / the sim's booking table) stay adapter-side and are passed in
//! as the mutable-across-the-tick [`SnapshotBookings`] view; node-level availability is
//! `requested − booked` exactly like the live `get_available_withdrawl`/`get_available_deposit`.
//!
//! **Live stats semantics, ported faithfully:** at the ROOM-stats level the live queue's
//! `unfufilled_amount()` never subtracts reservations (`pending_amount` is never written) and
//! ticket *registrations* INFLATE `stats.amount` — so the room-level totals feeding
//! `select_best_delivery` are `requested + registered`, while double-serve protection lives at
//! the node level only. [`stats_withdrawl_totals_by_priority`] reproduces exactly that
//! (snapshot + bookings ADDED).
//!
//! **Determinism deviations (documented, the sim baseline's M1 convention):** node candidate
//! order is adapter-controlled and deterministic (live iterated `HashMap`s — VM-dependent);
//! resource maps are insertion-ordered `Vec<(K, V)>` (live: `HashMap` iteration order); value
//! comparisons are exact integer rationals (`a1·d2 ⋛ a2·d1`, live compared `f32` quotients);
//! exact ties break to the lowest `(pickup node, delivery node)` in candidate order (live:
//! whatever the hash order produced). Same policy, fence-safe arithmetic.

use crate::priority::{generate_active_priorities, TransferPriority, TransferPriorityFlags, TransferType, TransferTypeFlags};
use crate::CreepEconDto;
use screeps::{Position, ResourceType, RoomName};
use std::collections::BTreeMap;

/// Opaque node identity: an index into the snapshot's node table. Adapters keep the aligned
/// side table mapping ids back to real targets (live: `Vec<TransferTarget>`; sim: sink/source
/// keys).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct NodeId(pub u32);

/// A withdraw-side demand key (the live `TransferWithdrawlKey`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct WithdrawKey {
    pub resource: ResourceType,
    pub priority: TransferPriority,
    pub allowed_type: TransferType,
}

impl WithdrawKey {
    pub fn matches(&self, resource: ResourceType, allowed_priorities: TransferPriorityFlags, allowed_types: TransferTypeFlags) -> bool {
        self.resource == resource
            && allowed_priorities.intersects(self.priority.into())
            && allowed_types.intersects(self.allowed_type.into())
    }
}

/// A deposit-side demand key (the live `TransferDepositKey`; `resource: None` = accepts-any).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DepositKey {
    pub resource: Option<ResourceType>,
    pub priority: TransferPriority,
    pub allowed_type: TransferType,
}

impl DepositKey {
    pub fn matches(
        &self,
        resource: Option<ResourceType>,
        allowed_priorities: TransferPriorityFlags,
        allowed_types: TransferTypeFlags,
    ) -> bool {
        self.resource == resource
            && allowed_priorities.intersects(self.priority.into())
            && allowed_types.intersects(self.allowed_type.into())
    }
}

/// An insertion-ordered small map (the kernel's deterministic replacement for the live
/// `HashMap`s — module docs). Lookup is linear; key sets here are tiny (a node has a handful of
/// demand keys; a creep carries a handful of resource types).
pub type VecMap<K, V> = Vec<(K, V)>;

fn vec_map_get<'a, K: PartialEq, V>(map: &'a [(K, V)], key: &K) -> Option<&'a V> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn vec_map_entry<K: PartialEq + Copy, V: Default>(map: &mut VecMap<K, V>, key: K) -> &mut V {
    if let Some(i) = map.iter().position(|(k, _)| *k == key) {
        &mut map[i].1
    } else {
        map.push((key, V::default()));
        &mut map.last_mut().expect("just pushed").1
    }
}

/// One node's REQUESTED transfer demand this tick (immutable once built).
#[derive(Clone, Debug)]
pub struct SnapshotNode {
    pub pos: Position,
    /// Requested withdraw amounts per key, in adapter-deterministic order (summed per key).
    pub withdrawls: Vec<(WithdrawKey, u32)>,
    /// Requested deposit amounts per key, in adapter-deterministic order (summed per key).
    pub deposits: Vec<(DepositKey, u32)>,
}

/// One room's slice of the snapshot, with the live room-stats priority-presence gates
/// (the OR of requested priorities — set by requests only, never by registrations, exactly
/// like the live `stats.withdrawl_priorities`/`deposit_priorities`).
#[derive(Clone, Debug, Default)]
struct SnapshotRoom {
    nodes: Vec<NodeId>,
    withdrawl_priorities: u8,
    deposit_priorities: u8,
}

/// The immutable per-tick transfer snapshot (0007 item 1): every room's materialized demand,
/// built ONCE at the top of the hauling pass.
#[derive(Clone, Debug, Default)]
pub struct TransferSnapshot {
    nodes: Vec<SnapshotNode>,
    rooms: BTreeMap<RoomName, SnapshotRoom>,
}

impl TransferSnapshot {
    pub fn new() -> TransferSnapshot {
        TransferSnapshot::default()
    }

    /// Add a node with its requested demand. Candidate ORDER within a room is the insertion
    /// order (the deterministic tie-break order — module docs).
    pub fn add_node(
        &mut self,
        room: RoomName,
        pos: Position,
        withdrawls: Vec<(WithdrawKey, u32)>,
        deposits: Vec<(DepositKey, u32)>,
    ) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        let room_entry = self.rooms.entry(room).or_default();
        room_entry.nodes.push(id);
        for (key, _) in &withdrawls {
            room_entry.withdrawl_priorities |= TransferPriorityFlags::from(key.priority).bits();
        }
        for (key, _) in &deposits {
            room_entry.deposit_priorities |= TransferPriorityFlags::from(key.priority).bits();
        }
        self.nodes.push(SnapshotNode {
            pos,
            withdrawls,
            deposits,
        });
        id
    }

    pub fn node(&self, id: NodeId) -> &SnapshotNode {
        &self.nodes[id.0 as usize]
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn room(&self, room: &RoomName) -> Option<&SnapshotRoom> {
        self.rooms.get(room)
    }

    /// The nodes of a room, in candidate order (empty if the room has no demand).
    pub fn room_nodes(&self, room: &RoomName) -> &[NodeId] {
        self.room(room).map(|r| r.nodes.as_slice()).unwrap_or(&[])
    }
}

/// The adapter-owned booking view: registered (reserved) amounts per node/key — the live
/// `pending_withdrawls`/`pending_deposits` mirror / the sim's booking table. Mutated as
/// assignments register during the tick; node availability is `requested − booked`.
#[derive(Clone, Debug, Default)]
pub struct SnapshotBookings {
    withdrawls: BTreeMap<NodeId, Vec<(WithdrawKey, u32)>>,
    deposits: BTreeMap<NodeId, Vec<(DepositKey, u32)>>,
}

impl SnapshotBookings {
    pub fn new() -> SnapshotBookings {
        SnapshotBookings::default()
    }

    pub fn book_withdraw(&mut self, node: NodeId, key: WithdrawKey, amount: u32) {
        let entries = self.withdrawls.entry(node).or_default();
        if let Some(e) = entries.iter_mut().find(|(k, _)| *k == key) {
            e.1 += amount;
        } else {
            entries.push((key, amount));
        }
    }

    pub fn book_deposit(&mut self, node: NodeId, key: DepositKey, amount: u32) {
        let entries = self.deposits.entry(node).or_default();
        if let Some(e) = entries.iter_mut().find(|(k, _)| *k == key) {
            e.1 += amount;
        } else {
            entries.push((key, amount));
        }
    }

    /// Book every entry of a withdraw ticket (the live `register_pickup` node half).
    pub fn book_withdraw_ticket(&mut self, ticket: &WithdrawTicketDto) {
        for (resource, entries) in &ticket.resources {
            for e in entries {
                self.book_withdraw(
                    ticket.node,
                    WithdrawKey {
                        resource: *resource,
                        priority: e.priority,
                        allowed_type: e.transfer_type,
                    },
                    e.amount,
                );
            }
        }
    }

    /// Book every entry of a deposit ticket (the live `register_delivery` node half).
    pub fn book_deposit_ticket(&mut self, ticket: &DepositTicketDto) {
        for (_, entries) in &ticket.resources {
            for e in entries {
                self.book_deposit(
                    ticket.node,
                    DepositKey {
                        resource: e.target_resource,
                        priority: e.priority,
                        allowed_type: e.transfer_type,
                    },
                    e.amount,
                );
            }
        }
    }

    fn booked_withdraw(&self, node: NodeId, key: &WithdrawKey) -> u32 {
        self.withdrawls
            .get(&node)
            .and_then(|v| v.iter().find(|(k, _)| k == key))
            .map(|(_, a)| *a)
            .unwrap_or(0)
    }

    fn booked_deposit(&self, node: NodeId, key: &DepositKey) -> u32 {
        self.deposits
            .get(&node)
            .and_then(|v| v.iter().find(|(k, _)| k == key))
            .map(|(_, a)| *a)
            .unwrap_or(0)
    }

    /// Per-resource booked withdraw amounts on a node for keys matching the masks (used by the
    /// live stats-inflation port — module docs).
    fn booked_withdraw_matching(
        &self,
        node: NodeId,
        transfer_types: TransferTypeFlags,
        priorities: TransferPriorityFlags,
    ) -> VecMap<ResourceType, u32> {
        let mut out: VecMap<ResourceType, u32> = Vec::new();
        if let Some(entries) = self.withdrawls.get(&node) {
            for (key, amount) in entries {
                if priorities.intersects(key.priority.into()) && transfer_types.intersects(key.allowed_type.into()) {
                    *vec_map_entry(&mut out, key.resource) += amount;
                }
            }
        }
        out
    }
}

/// Node-level availability: `requested − booked`, floored at 0 (the live
/// `get_available_withdrawl`).
pub fn available_withdrawl(snapshot: &TransferSnapshot, bookings: &SnapshotBookings, node: NodeId, key: &WithdrawKey) -> u32 {
    let requested: u32 = snapshot
        .node(node)
        .withdrawls
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, a)| *a)
        .sum();
    requested.saturating_sub(bookings.booked_withdraw(node, key))
}

/// Node-level availability: `requested − booked`, floored at 0 (the live
/// `get_available_deposit`).
pub fn available_deposit(snapshot: &TransferSnapshot, bookings: &SnapshotBookings, node: NodeId, key: &DepositKey) -> u32 {
    let requested: u32 = snapshot
        .node(node)
        .deposits
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, a)| *a)
        .sum();
    requested.saturating_sub(bookings.booked_deposit(node, key))
}

/// The live `TransferCapacity` (ported unchanged).
#[derive(Copy, Clone, Debug)]
pub enum TransferCapacity {
    Infinite,
    Finite(u32),
}

impl TransferCapacity {
    pub fn empty(&self) -> bool {
        match self {
            TransferCapacity::Infinite => false,
            TransferCapacity::Finite(current) => *current == 0,
        }
    }

    pub fn consume(&mut self, amount: u32) {
        match self {
            TransferCapacity::Infinite => {}
            TransferCapacity::Finite(current) => {
                *current -= amount;
            }
        }
    }

    pub fn clamp(&self, amount: u32) -> u32 {
        match self {
            TransferCapacity::Infinite => amount,
            TransferCapacity::Finite(current) => amount.min(*current),
        }
    }
}

/// One withdraw ticket entry (the live `TransferWithdrawlTicketResourceEntry`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WithdrawEntryDto {
    pub amount: u32,
    pub transfer_type: TransferType,
    pub priority: TransferPriority,
}

/// One deposit ticket entry (the live `TransferDepositTicketResourceEntry`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DepositEntryDto {
    pub target_resource: Option<ResourceType>,
    pub amount: u32,
    pub transfer_type: TransferType,
    pub priority: TransferPriority,
}

/// A selected pickup (the live `TransferWithdrawTicket`, DTO form — the adapter maps `node`
/// back to the real target and converts entries).
#[derive(Clone, Debug)]
pub struct WithdrawTicketDto {
    pub node: NodeId,
    pub pos: Position,
    /// Per-resource entries, in kernel-deterministic (insertion) order.
    pub resources: VecMap<ResourceType, Vec<WithdrawEntryDto>>,
}

impl WithdrawTicketDto {
    pub fn total_amount(&self) -> u32 {
        self.resources.iter().flat_map(|(_, v)| v.iter().map(|e| e.amount)).sum()
    }
}

/// A selected delivery (the live `TransferDepositTicket`, DTO form).
#[derive(Clone, Debug)]
pub struct DepositTicketDto {
    pub node: NodeId,
    pub pos: Position,
    /// Keyed by the CARRIED resource being deposited (entries may target `None` keys).
    pub resources: VecMap<ResourceType, Vec<DepositEntryDto>>,
}

impl DepositTicketDto {
    pub fn total_amount(&self) -> u32 {
        self.resources.iter().flat_map(|(_, v)| v.iter().map(|e| e.amount)).sum()
    }

    /// Per-target-resource totals — the `desired_resources` a paired pickup is selected
    /// against (the live `select_best_delivery` interior map).
    pub fn desired_resources(&self) -> VecMap<Option<ResourceType>, u32> {
        let mut out: VecMap<Option<ResourceType>, u32> = Vec::new();
        for (_, entries) in &self.resources {
            for e in entries {
                *vec_map_entry(&mut out, e.target_resource) += e.amount;
            }
        }
        out
    }
}

/// Chebyshev range (the live `get_range_to`).
fn range(a: Position, b: Position) -> u32 {
    a.get_range_to(b)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Node-level selection (ports of TransferNode::select_pickup / select_delivery /
// select_single_delivery — the per-node ticket-entry construction).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Port of `TransferNode::select_pickup`: build the withdraw entries this node can serve for
/// `desired_resources` (a `None` key = "fill with anything") under the masks and capacity.
/// `saturating_sub` guards the live code's unchecked `remaining_amount - pickedup_resources`
/// (unreachable in the current request vocabulary; defensive here).
pub fn node_select_pickup(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    node: NodeId,
    allowed_priorities: TransferPriorityFlags,
    pickup_types: TransferTypeFlags,
    desired_resources: &[(Option<ResourceType>, u32)],
    available_capacity: TransferCapacity,
) -> VecMap<ResourceType, Vec<WithdrawEntryDto>> {
    let mut pickup_resources: VecMap<ResourceType, Vec<WithdrawEntryDto>> = Vec::new();
    let mut remaining_capacity = available_capacity;
    let mut fill_none = None;

    for (desired_resource, amount) in desired_resources {
        if let Some(resource) = desired_resource {
            for (key, _) in &snapshot.node(node).withdrawls {
                if key.matches(*resource, allowed_priorities, pickup_types) {
                    let remaining_amount = available_withdrawl(snapshot, bookings, node, key);

                    if remaining_amount > 0 {
                        let pickup_amount = remaining_capacity.clamp(remaining_amount.min(*amount));

                        vec_map_entry(&mut pickup_resources, *resource).push(WithdrawEntryDto {
                            amount: pickup_amount,
                            transfer_type: key.allowed_type,
                            priority: key.priority,
                        });

                        remaining_capacity.consume(pickup_amount);

                        if remaining_capacity.empty() {
                            break;
                        }
                    }
                }
            }
        } else {
            fill_none = Some(*amount);
        }

        if remaining_capacity.empty() {
            break;
        }
    }

    if let Some(fill_none_amount) = fill_none {
        let mut remaining_none_amount = TransferCapacity::Finite(fill_none_amount);

        for (key, _) in &snapshot.node(node).withdrawls {
            if allowed_priorities.intersects(key.priority.into()) && pickup_types.intersects(key.allowed_type.into()) {
                let remaining_amount = available_withdrawl(snapshot, bookings, node, key);

                if remaining_amount > 0 {
                    let pickedup_resources: u32 = vec_map_get(&pickup_resources, &key.resource)
                        .map(|entries| entries.iter().filter(|e| e.priority == key.priority).map(|e| e.amount).sum())
                        .unwrap_or(0);

                    let unconsumed_remaining_amount = remaining_amount.saturating_sub(pickedup_resources);

                    if unconsumed_remaining_amount > 0 {
                        let pickup_amount = remaining_none_amount.clamp(remaining_capacity.clamp(unconsumed_remaining_amount));

                        vec_map_entry(&mut pickup_resources, key.resource).push(WithdrawEntryDto {
                            amount: pickup_amount,
                            transfer_type: key.allowed_type,
                            priority: key.priority,
                        });

                        remaining_capacity.consume(pickup_amount);
                        remaining_none_amount.consume(pickup_amount);

                        if remaining_capacity.empty() || remaining_none_amount.empty() {
                            break;
                        }
                    }
                }
            }
        }
    }

    pickup_resources
}

/// Port of `TransferNode::select_delivery`: build the deposit entries this node can absorb from
/// `available_resources` (Some-resource keys first, then the accepts-any `None` keys).
pub fn node_select_delivery(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    node: NodeId,
    allowed_priorities: TransferPriorityFlags,
    delivery_types: TransferTypeFlags,
    available_resources: &[(ResourceType, u32)],
    available_capacity: TransferCapacity,
) -> VecMap<ResourceType, Vec<DepositEntryDto>> {
    let mut delivery_resources: VecMap<ResourceType, Vec<DepositEntryDto>> = Vec::new();
    let mut remaining_capacity = available_capacity;

    for (resource, amount) in available_resources {
        for (key, _) in &snapshot.node(node).deposits {
            if key.matches(Some(*resource), allowed_priorities, delivery_types) {
                let remaining_amount = available_deposit(snapshot, bookings, node, key);

                if remaining_amount > 0 {
                    let delivery_amount = remaining_capacity.clamp(remaining_amount.min(*amount));

                    if delivery_amount > 0 {
                        vec_map_entry(&mut delivery_resources, *resource).push(DepositEntryDto {
                            target_resource: Some(*resource),
                            amount: delivery_amount,
                            transfer_type: key.allowed_type,
                            priority: key.priority,
                        });

                        remaining_capacity.consume(delivery_amount);

                        if remaining_capacity.empty() {
                            break;
                        }
                    }
                }
            }
        }

        if remaining_capacity.empty() {
            break;
        }
    }

    let none_keys: Vec<DepositKey> = snapshot
        .node(node)
        .deposits
        .iter()
        .map(|(k, _)| *k)
        .filter(|key| {
            key.resource.is_none()
                && delivery_types.intersects(key.allowed_type.into())
                && allowed_priorities.intersects(key.priority.into())
        })
        .collect();

    for key in none_keys {
        let mut remaining_none_amount = TransferCapacity::Finite(available_deposit(snapshot, bookings, node, &key));

        if !remaining_none_amount.empty() {
            for (resource, amount) in available_resources {
                let deposited_resources: u32 = vec_map_get(&delivery_resources, resource)
                    .map(|entries| entries.iter().filter(|e| e.priority == key.priority).map(|e| e.amount).sum())
                    .unwrap_or(0);

                let unconsumed_remaining_amount = amount.saturating_sub(deposited_resources);

                if unconsumed_remaining_amount > 0 {
                    let delivery_amount = remaining_none_amount.clamp(remaining_capacity.clamp(unconsumed_remaining_amount));

                    if delivery_amount > 0 {
                        vec_map_entry(&mut delivery_resources, *resource).push(DepositEntryDto {
                            target_resource: None,
                            amount: delivery_amount,
                            transfer_type: key.allowed_type,
                            priority: key.priority,
                        });

                        remaining_capacity.consume(delivery_amount);
                        remaining_none_amount.consume(delivery_amount);

                        if remaining_capacity.empty() || remaining_none_amount.empty() {
                            break;
                        }
                    }
                }
            }
        }

        if remaining_capacity.empty() || remaining_none_amount.empty() {
            break;
        }
    }

    delivery_resources
}

/// Port of `TransferNode::select_single_delivery`: the single best resource this node can
/// absorb (per-resource capacity reset — the terminal-send shape), max by total amount; exact
/// ties keep the LAST maximal resource in candidate order (the live `max_by_key` convention).
pub fn node_select_single_delivery(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    node: NodeId,
    allowed_priorities: TransferPriorityFlags,
    delivery_types: TransferTypeFlags,
    available_resources: &[(ResourceType, u32)],
    available_capacity: TransferCapacity,
) -> Option<(ResourceType, Vec<DepositEntryDto>)> {
    let mut delivery_resources: VecMap<ResourceType, Vec<DepositEntryDto>> = Vec::new();

    for (resource, amount) in available_resources {
        let mut remaining_capacity = available_capacity;

        for (key, _) in &snapshot.node(node).deposits {
            if key.matches(Some(*resource), allowed_priorities, delivery_types)
                || (key.resource.is_none()
                    && delivery_types.intersects(key.allowed_type.into())
                    && allowed_priorities.intersects(key.priority.into()))
            {
                let remaining_amount = available_deposit(snapshot, bookings, node, key);

                if remaining_amount > 0 {
                    let delivery_amount = remaining_capacity.clamp(remaining_amount.min(*amount));

                    if delivery_amount > 0 {
                        vec_map_entry(&mut delivery_resources, *resource).push(DepositEntryDto {
                            target_resource: Some(*resource),
                            amount: delivery_amount,
                            transfer_type: key.allowed_type,
                            priority: key.priority,
                        });

                        remaining_capacity.consume(delivery_amount);

                        if remaining_capacity.empty() {
                            break;
                        }
                    }
                }
            }
        }
    }

    delivery_resources
        .into_iter()
        .max_by_key(|(_, entries)| entries.iter().map(|e| e.amount).sum::<u32>())
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Room-level selection (ports of the TransferQueue methods).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Port of `TransferQueue::select_pickups`: every node ticket across the pickup rooms that can
/// serve the desired resources (gated by the room's requested-priority presence, like the live
/// `stats.withdrawl_priorities` check).
pub fn select_pickups(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    pickup_rooms: &[RoomName],
    allowed_priorities: TransferPriorityFlags,
    pickup_types: TransferTypeFlags,
    desired_resources: &[(Option<ResourceType>, u32)],
    available_capacity: TransferCapacity,
) -> Vec<WithdrawTicketDto> {
    let mut tickets = Vec::new();

    for pickup_room in pickup_rooms {
        if let Some(room) = snapshot.room(pickup_room) {
            if TransferPriorityFlags::from_bits_truncate(room.withdrawl_priorities).intersects(allowed_priorities) {
                for &node in &room.nodes {
                    let pickup_resources =
                        node_select_pickup(snapshot, bookings, node, allowed_priorities, pickup_types, desired_resources, available_capacity);

                    if !pickup_resources.is_empty() {
                        tickets.push(WithdrawTicketDto {
                            node,
                            pos: snapshot.node(node).pos,
                            resources: pickup_resources,
                        });
                    }
                }
            }
        }
    }

    tickets
}

/// Port of `TransferQueue::select_deliveries`: every node ticket across the delivery rooms that
/// can absorb the available resources, filtered by the adapter's target filter.
#[allow(clippy::too_many_arguments)]
pub fn select_deliveries<TF>(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    delivery_rooms: &[RoomName],
    allowed_priorities: TransferPriorityFlags,
    delivery_types: TransferTypeFlags,
    available_resources: &[(ResourceType, u32)],
    available_capacity: TransferCapacity,
    target_filter: TF,
) -> Vec<DepositTicketDto>
where
    TF: Fn(NodeId) -> bool,
{
    let mut tickets = Vec::new();

    for delivery_room in delivery_rooms {
        if let Some(room) = snapshot.room(delivery_room) {
            if TransferPriorityFlags::from_bits_truncate(room.deposit_priorities).intersects(allowed_priorities) {
                for &node in &room.nodes {
                    if target_filter(node) {
                        let delivery_resources = node_select_delivery(
                            snapshot,
                            bookings,
                            node,
                            allowed_priorities,
                            delivery_types,
                            available_resources,
                            available_capacity,
                        );

                        if !delivery_resources.is_empty() {
                            tickets.push(DepositTicketDto {
                                node,
                                pos: snapshot.node(node).pos,
                                resources: delivery_resources,
                            });
                        }
                    }
                }
            }
        }
    }

    tickets
}

/// Port of `TransferQueue::get_available_withdrawl_totals_by_priority` — the ROOM-STATS level
/// totals with the live semantics preserved verbatim: requested amounts PLUS registered
/// (booked) amounts, never net of reservations (`pending_amount` was never written live;
/// registrations inflate `stats.amount` — module docs).
pub fn stats_withdrawl_totals_by_priority(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    rooms: &[RoomName],
    transfer_type: TransferType,
    withdrawl_priorities: TransferPriorityFlags,
) -> VecMap<ResourceType, u32> {
    let mut available_resources: VecMap<ResourceType, u32> = Vec::new();
    let types: TransferTypeFlags = transfer_type.into();

    for room_name in rooms {
        if let Some(room) = snapshot.room(room_name) {
            for &node in &room.nodes {
                for (key, amount) in &snapshot.node(node).withdrawls {
                    if withdrawl_priorities.intersects(key.priority.into()) && key.allowed_type == transfer_type && *amount > 0 {
                        *vec_map_entry(&mut available_resources, key.resource) += *amount;
                    }
                }
                // Registered (booked) amounts inflate the stats totals exactly like the live
                // `register_pickup` stats write.
                for (resource, amount) in bookings.booked_withdraw_matching(node, types, withdrawl_priorities) {
                    if amount > 0 {
                        *vec_map_entry(&mut available_resources, resource) += amount;
                    }
                }
            }
        }
    }

    available_resources.retain(|(_, v)| *v > 0);
    available_resources
}

/// An exact-rational transfer value `amount / divisor` for comparison without floats:
/// `a` better-than `b` ⟺ `a.num · b.den > b.num · a.den`. Divisor floored at 1 (the live
/// `finite_transfer_value` guard, IBEX-046).
#[derive(Copy, Clone, Debug)]
pub struct TransferValue {
    num: u64,
    den: u64,
}

impl TransferValue {
    pub fn new(resources: u32, divisor: u32) -> TransferValue {
        TransferValue {
            num: resources as u64,
            den: divisor.max(1) as u64,
        }
    }

    pub fn better_than(&self, other: &TransferValue) -> bool {
        self.num * other.den > other.num * self.den
    }

    pub fn equals(&self, other: &TransferValue) -> bool {
        self.num * other.den == other.num * self.den
    }
}

/// Port of `TransferQueue::select_best_delivery`: the best (pickup, delivery) pair for the
/// given masks — deliveries selected against the room-stats totals, a pickup per delivery, the
/// pair maximizing `pickup_total / (creep→pickup + pickup→delivery)` (exact rationals; ties to
/// the lowest `(pickup node, delivery node)` — module docs).
#[allow(clippy::too_many_arguments)]
pub fn select_best_delivery<TF>(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    pickup_rooms: &[RoomName],
    delivery_rooms: &[RoomName],
    pickup_priorities: TransferPriorityFlags,
    delivery_priorities: TransferPriorityFlags,
    transfer_type: TransferType,
    current_position: Position,
    available_capacity: TransferCapacity,
    target_filter: TF,
) -> Option<(WithdrawTicketDto, DepositTicketDto)>
where
    TF: Fn(NodeId) -> bool,
{
    if available_capacity.empty() {
        return None;
    }

    let global_available_resources =
        stats_withdrawl_totals_by_priority(snapshot, bookings, pickup_rooms, transfer_type, pickup_priorities);

    if global_available_resources.is_empty() {
        return None;
    }

    let deliveries = select_deliveries(
        snapshot,
        bookings,
        delivery_rooms,
        delivery_priorities,
        transfer_type.into(),
        &global_available_resources
            .iter()
            .map(|(r, a)| (*r, *a))
            .collect::<Vec<_>>(),
        available_capacity,
        target_filter,
    );

    let mut best: Option<(WithdrawTicketDto, DepositTicketDto, TransferValue)> = None;

    for delivery in deliveries {
        let desired = delivery.desired_resources();
        let pickups = select_pickups(
            snapshot,
            bookings,
            pickup_rooms,
            pickup_priorities,
            transfer_type.into(),
            &desired,
            available_capacity,
        );

        let delivery_pos = delivery.pos;

        for pickup in pickups {
            let pickup_length = range(current_position, pickup.pos);
            let delivery_length = range(pickup.pos, delivery_pos);
            let value = TransferValue::new(pickup.total_amount(), pickup_length + delivery_length);

            let better = match &best {
                None => true,
                Some((bp, bd, bv)) => {
                    value.better_than(bv) || (value.equals(bv) && (pickup.node, delivery.node) < (bp.node, bd.node))
                }
            };
            if better {
                best = Some((pickup, delivery.clone(), value));
            }
        }
    }

    best.map(|(pickup, delivery, _)| (pickup, delivery))
}

/// Port of `TransferQueue::select_pickup_and_delivery` (the K2 seam entry,
/// `select_pickup_and_delivery(&snapshot, creep_dto)`): walk the tier-interleave combinations
/// and return the first combination's best pair.
#[allow(clippy::too_many_arguments)]
pub fn select_pickup_and_delivery<TF>(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    creep: &CreepEconDto,
    pickup_rooms: &[RoomName],
    delivery_rooms: &[RoomName],
    allowed_priorities: TransferPriorityFlags,
    transfer_type: TransferType,
    available_capacity: TransferCapacity,
    target_filter: TF,
) -> Option<(WithdrawTicketDto, DepositTicketDto)>
where
    TF: Fn(NodeId) -> bool + Copy,
{
    for (pickup_priorities, delivery_priorities) in generate_active_priorities(allowed_priorities, allowed_priorities) {
        if let Some(result) = select_best_delivery(
            snapshot,
            bookings,
            pickup_rooms,
            delivery_rooms,
            pickup_priorities,
            delivery_priorities,
            transfer_type,
            creep.pos,
            available_capacity,
            target_filter,
        ) {
            return Some(result);
        }
    }

    None
}

/// Port of `TransferQueue::get_delivery`: the best single delivery for carried resources,
/// ranked by `amount / range(anchor→target)` (exact rationals; ties to the lowest node).
#[allow(clippy::too_many_arguments)]
pub fn get_delivery<TF>(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    delivery_rooms: &[RoomName],
    allowed_priorities: TransferPriorityFlags,
    delivery_types: TransferTypeFlags,
    available_resources: &[(ResourceType, u32)],
    available_capacity: TransferCapacity,
    anchor_location: Position,
    target_filter: TF,
) -> Option<DepositTicketDto>
where
    TF: Fn(NodeId) -> bool,
{
    if available_capacity.empty() {
        return None;
    }

    let deliveries = select_deliveries(
        snapshot,
        bookings,
        delivery_rooms,
        allowed_priorities,
        delivery_types,
        available_resources,
        available_capacity,
        target_filter,
    );

    let mut best: Option<(DepositTicketDto, TransferValue)> = None;
    for delivery in deliveries {
        let value = TransferValue::new(delivery.total_amount(), range(anchor_location, delivery.pos));
        let better = match &best {
            None => true,
            Some((bd, bv)) => value.better_than(bv) || (value.equals(bv) && delivery.node < bd.node),
        };
        if better {
            best = Some((delivery, value));
        }
    }
    best.map(|(delivery, _)| delivery)
}

/// Port of `TransferQueue::get_delivery_from_target`: pair a specific pickup NODE's available
/// resources (node-level, net of bookings) with the best delivery for them — the
/// additional-deliveries + link-routing arm.
#[allow(clippy::too_many_arguments)]
pub fn get_delivery_from_node<TF>(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    delivery_rooms: &[RoomName],
    node: NodeId,
    allowed_pickup_priorities: TransferPriorityFlags,
    allowed_delivery_priorities: TransferPriorityFlags,
    delivery_type: TransferType,
    available_capacity: TransferCapacity,
    anchor_location: Position,
    target_filter: TF,
) -> Option<(WithdrawTicketDto, DepositTicketDto)>
where
    TF: Fn(NodeId) -> bool,
{
    if available_capacity.empty() {
        return None;
    }

    // Node-level availability (net of bookings) — the live `node.get_available_withdrawl_totals`.
    let mut available_resources: VecMap<ResourceType, u32> = Vec::new();
    for (key, _) in &snapshot.node(node).withdrawls {
        if allowed_pickup_priorities.intersects(key.priority.into())
            && TransferTypeFlags::from(delivery_type).intersects(key.allowed_type.into())
        {
            let available = available_withdrawl(snapshot, bookings, node, key);
            if available > 0 {
                *vec_map_entry(&mut available_resources, key.resource) += available;
            }
        }
    }

    if available_resources.is_empty() {
        return None;
    }

    let delivery = get_delivery(
        snapshot,
        bookings,
        delivery_rooms,
        allowed_delivery_priorities,
        delivery_type.into(),
        &available_resources,
        available_capacity,
        anchor_location,
        target_filter,
    )?;

    let delivery_resources: VecMap<Option<ResourceType>, u32> = delivery
        .resources
        .iter()
        .map(|(resource, entries)| (Some(*resource), entries.iter().map(|e| e.amount).sum()))
        .collect();

    let pickup_resources = node_select_pickup(
        snapshot,
        bookings,
        node,
        allowed_pickup_priorities,
        delivery_type.into(),
        &delivery_resources,
        available_capacity,
    );

    let pickup = WithdrawTicketDto {
        node,
        pos: snapshot.node(node).pos,
        resources: pickup_resources,
    };

    Some((pickup, delivery))
}

/// Port of `TransferQueue::get_pickup_from_target`: a single-resource top-off pickup from a
/// specific node (the `tick_pickup_and_fill` arm).
pub fn get_pickup_from_node(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    node: NodeId,
    allowed_pickup_priorities: TransferPriorityFlags,
    transfer_types: TransferTypeFlags,
    available_capacity: TransferCapacity,
    resource_type: ResourceType,
) -> Option<WithdrawTicketDto> {
    if available_capacity.empty() {
        return None;
    }

    let resource_amount = available_capacity.clamp(u32::MAX);
    let desired_resources = vec![(Some(resource_type), resource_amount)];

    let pickup_resources = node_select_pickup(
        snapshot,
        bookings,
        node,
        allowed_pickup_priorities,
        transfer_types,
        &desired_resources,
        available_capacity,
    );

    if pickup_resources.is_empty() {
        return None;
    }

    Some(WithdrawTicketDto {
        node,
        pos: snapshot.node(node).pos,
        resources: pickup_resources,
    })
}

/// The nearest-wins delivery composition (the live `get_new_delivery_current_resources_state` =
/// `select_deliveries` + `find_nearest_linear_by`): the NEAREST candidate ticket by linear
/// range, first-minimal in candidate order.
#[allow(clippy::too_many_arguments)]
pub fn select_nearest_delivery<TF>(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    delivery_rooms: &[RoomName],
    allowed_priorities: TransferPriorityFlags,
    delivery_types: TransferTypeFlags,
    available_resources: &[(ResourceType, u32)],
    available_capacity: TransferCapacity,
    creep_pos: Position,
    target_filter: TF,
) -> Option<DepositTicketDto>
where
    TF: Fn(NodeId) -> bool,
{
    if available_capacity.empty() {
        return None;
    }
    select_deliveries(
        snapshot,
        bookings,
        delivery_rooms,
        allowed_priorities,
        delivery_types,
        available_resources,
        available_capacity,
        target_filter,
    )
    .into_iter()
    .min_by_key(|ticket| range(creep_pos, ticket.pos))
}

/// The nearest-wins pickup composition (the live `get_new_[nearby_]pickup_state_fill_resource`
/// = `select_pickups` + anchor filter + `find_nearest_linear_by`). The anchor is the WORK SITE
/// (e.g. the controller for slow upgraders), never the creep (the live deadlock note).
#[allow(clippy::too_many_arguments)]
pub fn select_nearest_pickup(
    snapshot: &TransferSnapshot,
    bookings: &SnapshotBookings,
    pickup_rooms: &[RoomName],
    allowed_priorities: TransferPriorityFlags,
    transfer_types: TransferTypeFlags,
    desired_resource: ResourceType,
    free_capacity: u32,
    creep_pos: Position,
    range_anchor: Option<(Position, u32)>,
) -> Option<WithdrawTicketDto> {
    if free_capacity == 0 {
        return None;
    }

    let desired_resources = vec![(Some(desired_resource), free_capacity)];

    select_pickups(
        snapshot,
        bookings,
        pickup_rooms,
        allowed_priorities,
        transfer_types,
        &desired_resources,
        TransferCapacity::Infinite,
    )
    .into_iter()
    .filter(|ticket| match range_anchor {
        Some((anchor, r)) => range(anchor, ticket.pos) <= r,
        None => true,
    })
    .min_by_key(|ticket| range(creep_pos, ticket.pos))
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The matched-flow hauling statistic (port of `TransferQueue::total_unfufilled_resources`'s
// stage math — the live inputs come from the room stats, the sim's from its demand lists; the
// STAGE MATH is the one shared implementation).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Per-resource (active, inactive) unfulfilled sums — the stage-math inputs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StageSums {
    pub active: u32,
    pub inactive: u32,
}

/// The 3-stage (6-loop) supply↔demand match: (a) active↔active (Some-resource then None-deposit),
/// (b) inactive-withdraw→active-deposit (Some then None), (c) active-withdraw→inactive-deposit
/// (Some then None) — each consuming `min(remaining supply, remaining demand)`. Deterministic
/// insertion order replaces the live HashMap iteration (order only matters when several
/// resources compete for one accepts-any deposit — a documented determinism stand-in).
pub fn matched_unfulfilled_resources(
    withdrawls: &[(ResourceType, StageSums)],
    deposits: &[(Option<ResourceType>, StageSums)],
) -> VecMap<ResourceType, u32> {
    let mut w: VecMap<ResourceType, StageSums> = withdrawls.to_vec();
    let mut d: VecMap<Option<ResourceType>, StageSums> = deposits.to_vec();
    let mut total_pickup: VecMap<ResourceType, u32> = Vec::new();

    fn add_resource(total_pickup: &mut VecMap<ResourceType, u32>, resource: ResourceType, amount: u32) {
        *vec_map_entry(total_pickup, resource) += amount;
    }

    // Active <-> Active (Some-resource deposits).
    for (resource, deposit_stats) in d.iter_mut() {
        if let Some(resource) = resource {
            if let Some(withdrawl_stats) = w.iter_mut().find(|(r, _)| r == resource).map(|(_, s)| s) {
                let consume = withdrawl_stats.active.min(deposit_stats.active);
                withdrawl_stats.active -= consume;
                deposit_stats.active -= consume;
                add_resource(&mut total_pickup, *resource, consume);
            }
        }
    }
    // Active <-> Active (accepts-any deposits).
    for (resource, deposit_stats) in d.iter_mut() {
        if resource.is_none() {
            for (other_resource, withdrawl_stats) in w.iter_mut() {
                let consume = withdrawl_stats.active.min(deposit_stats.active);
                withdrawl_stats.active -= consume;
                deposit_stats.active -= consume;
                add_resource(&mut total_pickup, *other_resource, consume);
            }
        }
    }
    // Inactive -> Active.
    for (resource, deposit_stats) in d.iter_mut() {
        if let Some(resource) = resource {
            if let Some(withdrawl_stats) = w.iter_mut().find(|(r, _)| r == resource).map(|(_, s)| s) {
                let consume = withdrawl_stats.inactive.min(deposit_stats.active);
                withdrawl_stats.inactive -= consume;
                deposit_stats.active -= consume;
                add_resource(&mut total_pickup, *resource, consume);
            }
        }
    }
    for (resource, deposit_stats) in d.iter_mut() {
        if resource.is_none() {
            for (other_resource, withdrawl_stats) in w.iter_mut() {
                let consume = withdrawl_stats.inactive.min(deposit_stats.active);
                withdrawl_stats.inactive -= consume;
                deposit_stats.active -= consume;
                add_resource(&mut total_pickup, *other_resource, consume);
            }
        }
    }
    // Active -> Inactive.
    for (resource, withdrawl_stats) in w.iter_mut() {
        if let Some(deposit_stats) = d.iter_mut().find(|(r, _)| *r == Some(*resource)).map(|(_, s)| s) {
            let consume = withdrawl_stats.active.min(deposit_stats.inactive);
            withdrawl_stats.active -= consume;
            deposit_stats.inactive -= consume;
            add_resource(&mut total_pickup, *resource, consume);
        }
    }
    for (resource, withdrawl_stats) in w.iter_mut() {
        for (other_resource, deposit_stats) in d.iter_mut() {
            if other_resource.is_none() {
                let consume = withdrawl_stats.active.min(deposit_stats.inactive);
                withdrawl_stats.active -= consume;
                deposit_stats.inactive -= consume;
                add_resource(&mut total_pickup, *resource, consume);
            }
        }
    }

    total_pickup.retain(|(_, amount)| *amount > 0);
    total_pickup
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::RoomCoordinate;

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }

    fn wkey(priority: TransferPriority, allowed_type: TransferType) -> WithdrawKey {
        WithdrawKey {
            resource: ResourceType::Energy,
            priority,
            allowed_type,
        }
    }

    fn dkey(resource: Option<ResourceType>, priority: TransferPriority, allowed_type: TransferType) -> DepositKey {
        DepositKey {
            resource,
            priority,
            allowed_type,
        }
    }

    fn creep(p: Position, free: u32) -> CreepEconDto {
        CreepEconDto {
            id: 1,
            pos: p,
            free_capacity: free,
            store: Vec::new(),
        }
    }

    /// Fixture from the PRE-MOVE live behavior: a storage (None-priority supply) feeding a High
    /// spawn wins the FIRST interleave combination before any Medium/Low pairing; the value
    /// score amount/(d1+d2) picks the bigger-closer pair (moved from the sim baseline pin —
    /// the same math, byte-compatible).
    #[test]
    fn interleave_serves_high_first_and_scores_by_value_density() {
        let mut snapshot = TransferSnapshot::new();
        let spawn = snapshot.add_node(
            room(),
            pos(30, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::High, TransferType::Haul), 300)],
        );
        let _container = snapshot.add_node(
            room(),
            pos(40, 40),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::Low, TransferType::Haul), 2000)],
        );
        let storage = snapshot.add_node(
            room(),
            pos(20, 25),
            vec![(wkey(TransferPriority::None, TransferType::Haul), 50_000)],
            vec![],
        );
        let bookings = SnapshotBookings::new();

        let (p, d) = select_pickup_and_delivery(
            &snapshot,
            &bookings,
            &creep(pos(25, 25), 200),
            &[room()],
            &[room()],
            TransferPriorityFlags::ALL,
            TransferType::Haul,
            TransferCapacity::Finite(200),
            |_| true,
        )
        .unwrap();
        assert_eq!(p.node, storage);
        assert_eq!(d.node, spawn, "High delivery served first (interleave #1)");
        assert_eq!(p.total_amount(), 200, "pickup clamped to capacity");

        // Two High deliveries: amount/(d1+d2) decides — the far spawn (300 over 5+20) loses to
        // the near extension (200 over 5+4); compared as exact rationals.
        let mut snapshot = TransferSnapshot::new();
        let _far_spawn = snapshot.add_node(
            room(),
            pos(40, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::High, TransferType::Haul), 300)],
        );
        let near_ext = snapshot.add_node(
            room(),
            pos(24, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::High, TransferType::Haul), 200)],
        );
        let _storage = snapshot.add_node(
            room(),
            pos(20, 25),
            vec![(wkey(TransferPriority::None, TransferType::Haul), 50_000)],
            vec![],
        );
        let (_, d) = select_pickup_and_delivery(
            &snapshot,
            &bookings,
            &creep(pos(25, 25), 400),
            &[room()],
            &[room()],
            TransferPriorityFlags::ALL,
            TransferType::Haul,
            TransferCapacity::Finite(400),
            |_| true,
        )
        .unwrap();
        assert_eq!(d.node, near_ext, "value density picks the near refill");
    }

    /// The Use lane: a Use-type withdraw is INVISIBLE to a Haul-type selection even when it is
    /// the only supply (moved from the sim baseline pin).
    #[test]
    fn use_lane_pickups_are_invisible_to_haul_selection() {
        let mut snapshot = TransferSnapshot::new();
        let _spawn = snapshot.add_node(
            room(),
            pos(30, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::High, TransferType::Haul), 300)],
        );
        let _controller_container = snapshot.add_node(
            room(),
            pos(20, 25),
            vec![(wkey(TransferPriority::None, TransferType::Use), 2000)],
            vec![],
        );
        let bookings = SnapshotBookings::new();
        assert!(
            select_pickup_and_delivery(
                &snapshot,
                &bookings,
                &creep(pos(25, 25), 200),
                &[room()],
                &[room()],
                TransferPriorityFlags::ALL,
                TransferType::Haul,
                TransferCapacity::Finite(200),
                |_| true,
            )
            .is_none(),
            "a Use-lane pickup never feeds a haul pairing"
        );
    }

    /// Nearest-wins carried-cargo delivery is priority-blind INSIDE the mask (the S3 disease,
    /// pre-move fixture): a near Low sink beats a far High sink under the flat-ACTIVE mask;
    /// None (storage) never competes.
    #[test]
    fn carried_cargo_delivery_is_nearest_wins_priority_blind() {
        let mut snapshot = TransferSnapshot::new();
        let _spawn = snapshot.add_node(
            room(),
            pos(30, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::High, TransferType::Haul), 300)],
        );
        let near_low = snapshot.add_node(
            room(),
            pos(22, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::Low, TransferType::Haul), 2000)],
        );
        let _storage = snapshot.add_node(
            room(),
            pos(21, 25),
            vec![],
            vec![(dkey(None, TransferPriority::None, TransferType::Haul), 100_000)],
        );
        let bookings = SnapshotBookings::new();
        let carried = vec![(ResourceType::Energy, 50)];
        let ticket = select_nearest_delivery(
            &snapshot,
            &bookings,
            &[room()],
            TransferPriorityFlags::ACTIVE,
            TransferTypeFlags::HAUL,
            &carried,
            TransferCapacity::Finite(50),
            pos(20, 25),
            |_| true,
        )
        .unwrap();
        assert_eq!(ticket.node, near_low, "the NEAR Low sink wins over the far High — S3");
        assert_eq!(ticket.total_amount(), 50);
    }

    /// Bookings reserve node availability: a fully-booked deposit is invisible to the next
    /// selection (the live pending_* reservation contract, node level).
    #[test]
    fn bookings_reserve_node_availability() {
        let mut snapshot = TransferSnapshot::new();
        let spawn = snapshot.add_node(
            room(),
            pos(30, 25),
            vec![],
            vec![(dkey(Some(ResourceType::Energy), TransferPriority::High, TransferType::Haul), 300)],
        );
        let _storage = snapshot.add_node(
            room(),
            pos(20, 25),
            vec![(wkey(TransferPriority::None, TransferType::Haul), 50_000)],
            vec![],
        );
        let mut bookings = SnapshotBookings::new();

        let (p, d) = select_pickup_and_delivery(
            &snapshot,
            &bookings,
            &creep(pos(25, 25), 300),
            &[room()],
            &[room()],
            TransferPriorityFlags::ALL,
            TransferType::Haul,
            TransferCapacity::Finite(300),
            |_| true,
        )
        .unwrap();
        assert_eq!(d.node, spawn);
        bookings.book_withdraw_ticket(&p);
        bookings.book_deposit_ticket(&d);

        // Second hauler: the spawn's 300 is fully booked — only the storage's None deposit
        // remains, which cannot pair with the storage's own withdraw (no (None,None) combo).
        let second = select_pickup_and_delivery(
            &snapshot,
            &bookings,
            &creep(pos(25, 25), 300),
            &[room()],
            &[room()],
            TransferPriorityFlags::ALL,
            TransferType::Haul,
            TransferCapacity::Finite(300),
            |_| true,
        );
        assert!(second.is_none(), "the booked spawn is reserved; no double-serve");
    }

    /// The room-stats totals keep the LIVE semantics: registrations INFLATE the totals
    /// (requested + booked), never net them out — the pre-move `stats.amount` behavior.
    #[test]
    fn stats_totals_inflate_with_registrations() {
        let mut snapshot = TransferSnapshot::new();
        let storage = snapshot.add_node(
            room(),
            pos(20, 25),
            vec![(wkey(TransferPriority::None, TransferType::Haul), 1000)],
            vec![],
        );
        let mut bookings = SnapshotBookings::new();
        let before = stats_withdrawl_totals_by_priority(
            &snapshot,
            &bookings,
            &[room()],
            TransferType::Haul,
            TransferPriorityFlags::ALL,
        );
        assert_eq!(vec_map_get(&before, &ResourceType::Energy), Some(&1000));

        bookings.book_withdraw(storage, wkey(TransferPriority::None, TransferType::Haul), 400);
        let after = stats_withdrawl_totals_by_priority(
            &snapshot,
            &bookings,
            &[room()],
            TransferType::Haul,
            TransferPriorityFlags::ALL,
        );
        assert_eq!(
            vec_map_get(&after, &ResourceType::Energy),
            Some(&1400),
            "registered amounts ADD at the stats level (live parity; node level still reserves)"
        );
        // …while the node level reserves:
        assert_eq!(
            available_withdrawl(&snapshot, &bookings, storage, &wkey(TransferPriority::None, TransferType::Haul)),
            600
        );
    }

    /// The matched-flow stat stage order (moved from the sim baseline pin): supply-bounded,
    /// inactive fills active remainder, leftover active flows to inactive deposits, and
    /// inactive→inactive does not exist.
    #[test]
    fn matched_unfulfilled_is_supply_bounded() {
        let energy = ResourceType::Energy;
        let mk_w = |active: u32, inactive: u32| vec![(energy, StageSums { active, inactive })];
        let mk_d = |some_active: u32, none_inactive: u32| {
            let mut m: Vec<(Option<ResourceType>, StageSums)> = Vec::new();
            if some_active > 0 {
                m.push((Some(energy), StageSums { active: some_active, inactive: 0 }));
            }
            if none_inactive > 0 {
                m.push((None, StageSums { active: 0, inactive: none_inactive }));
            }
            m
        };
        // Drained world: demand exists, zero supply ⇒ 0.
        assert!(matched_unfulfilled_resources(&mk_w(0, 0), &mk_d(300, 100_000)).is_empty());
        // Supply-bounded: 40 active supply against 300 active demand ⇒ 40.
        assert_eq!(
            vec_map_get(&matched_unfulfilled_resources(&mk_w(40, 0), &mk_d(300, 0)), &energy),
            Some(&40)
        );
        // Inactive supply fills the active-demand remainder.
        assert_eq!(
            vec_map_get(&matched_unfulfilled_resources(&mk_w(40, 5000), &mk_d(300, 0)), &energy),
            Some(&300)
        );
        // Leftover ACTIVE supply flows to the inactive (storage) deposit; inactive→inactive
        // does not exist.
        assert_eq!(
            vec_map_get(&matched_unfulfilled_resources(&mk_w(1040, 5000), &mk_d(300, 100_000)), &energy),
            Some(&(300 + 740))
        );
    }

    /// select_nearest_pickup honors the WORK-SITE anchor (pre-move fixture, live geometry from
    /// the W7N4 deadlock): the controller-side container stays eligible, distant storage is
    /// excluded.
    #[test]
    fn nearest_pickup_honors_anchor() {
        let mut snapshot = TransferSnapshot::new();
        let container = snapshot.add_node(
            room(),
            pos(36, 9),
            vec![(wkey(TransferPriority::None, TransferType::Use), 500)],
            vec![],
        );
        let _storage = snapshot.add_node(
            room(),
            pos(31, 32),
            vec![(wkey(TransferPriority::None, TransferType::Haul), 5000)],
            vec![],
        );
        let bookings = SnapshotBookings::new();
        let anchor = Some((pos(39, 12), 5));
        let ticket = select_nearest_pickup(
            &snapshot,
            &bookings,
            &[room()],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            100,
            pos(34, 12),
            anchor,
        )
        .unwrap();
        assert_eq!(ticket.node, container, "anchor keeps the controller-side source");
        // Full creep: no pickup.
        assert!(select_nearest_pickup(
            &snapshot,
            &bookings,
            &[room()],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            0,
            pos(34, 12),
            anchor,
        )
        .is_none());
    }
}
