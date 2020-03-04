use crate::findnearest::*;
use crate::ui::*;
use crate::visualize::*;
use itertools::*;
use remoteobjectid::*;
use screeps::*;
use serde::*;
use specs::prelude::{ResourceId, WriteStorage, Write, LazyUpdate, Read, Entities, SystemData, System, World};
use std::collections::hash_map::*;
use std::collections::HashMap;

//TODO: Use None as a priority instead of using option type? May be simpler...
#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Clone, Copy)]
#[repr(u8)]
pub enum TransferPriority {
    High = 0,
    Medium = 1,
    Low = 2,
    None = 3,
}

impl From<TransferPriority> for TransferPriorityFlags {
    fn from(priority: TransferPriority) -> TransferPriorityFlags {
        match priority {
            TransferPriority::High => TransferPriorityFlags::HIGH,
            TransferPriority::Medium => TransferPriorityFlags::MEDIUM,
            TransferPriority::Low => TransferPriorityFlags::LOW,
            TransferPriority::None => TransferPriorityFlags::NONE,
        }
    }
}

impl From<&TransferPriority> for TransferPriorityFlags {
    fn from(priority: &TransferPriority) -> TransferPriorityFlags {
        TransferPriorityFlags::from(*priority)
    }
}

pub const ACTIVE_TRANSFER_PRIORITIES: &[TransferPriority] = &[TransferPriority::High, TransferPriority::Medium, TransferPriority::Low];
pub const ALL_TRANSFER_PRIORITIES: &[TransferPriority] = &[
    TransferPriority::High,
    TransferPriority::Medium,
    TransferPriority::Low,
    TransferPriority::None,
];

bitflags! {
    pub struct TransferPriorityFlags: u8 {
        const UNSET = 0;

        const HIGH = 1u8 << (TransferPriority::High as u8);
        const MEDIUM = 1u8 << (TransferPriority::Medium as u8);
        const LOW = 1u8 << (TransferPriority::Low as u8);
        const NONE = 1u8 << (TransferPriority::None as u8);

        const ALL = Self::HIGH.bits | Self::MEDIUM.bits | Self::LOW.bits | Self::NONE.bits;
    }
}

//TODO: Need to support tombstones and dropped resources.
#[derive(Eq, PartialEq, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TransferTarget {
    Container(RemoteObjectId<StructureContainer>),
    Spawn(RemoteObjectId<StructureSpawn>),
    Extension(RemoteObjectId<StructureExtension>),
    Storage(RemoteObjectId<StructureStorage>),
    Tower(RemoteObjectId<StructureTower>),
    Link(RemoteObjectId<StructureLink>),
    Ruin(RemoteObjectId<Ruin>),
    Tombstone(RemoteObjectId<Tombstone>),
    Resource(RemoteObjectId<Resource>)
}

impl TransferTarget {
    fn is_valid_from_id<T>(target: &RemoteObjectId<T>) -> bool where T: HasId + SizedRoomObject {
        if game::rooms::get(target.pos().room_name()).is_some() {
            target.resolve().is_some()
        } else {
            true
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            TransferTarget::Container(id) => Self::is_valid_from_id(id),
            TransferTarget::Spawn(id) => Self::is_valid_from_id(id),
            TransferTarget::Extension(id) => Self::is_valid_from_id(id),
            TransferTarget::Storage(id) => Self::is_valid_from_id(id),
            TransferTarget::Tower(id) => Self::is_valid_from_id(id),
            TransferTarget::Link(id) => Self::is_valid_from_id(id),
            TransferTarget::Ruin(id) => Self::is_valid_from_id(id),
            TransferTarget::Tombstone(id) => Self::is_valid_from_id(id),
            TransferTarget::Resource(id) => Self::is_valid_from_id(id),
        }
    }

    pub fn pos(&self) -> RoomPosition {
        match self {
            TransferTarget::Container(id) => id.pos(),
            TransferTarget::Spawn(id) => id.pos(),
            TransferTarget::Extension(id) => id.pos(),
            TransferTarget::Storage(id) => id.pos(),
            TransferTarget::Tower(id) => id.pos(),
            TransferTarget::Link(id) => id.pos(),
            TransferTarget::Ruin(id) => id.pos(),
            TransferTarget::Tombstone(id) => id.pos(),
            TransferTarget::Resource(id) => id.pos(),
        }
    }

    fn withdraw_resource_amount_from_id<T>(target: &RemoteObjectId<T>, creep: &Creep, resource: ResourceType, amount: u32) -> ReturnCode where T: Withdrawable + HasStore + HasId + SizedRoomObject {
        if let Some(obj) = target.resolve() {
            let withdraw_amount = obj.store_used_capacity(Some(resource)).min(amount);

            creep.withdraw_amount(&obj, resource, withdraw_amount)
        } else {
            ReturnCode::NotFound
        }
    }

    fn pickup_resource_from_id(target: &RemoteObjectId<Resource>, creep: &Creep) -> ReturnCode {
        if let Some(obj) = target.resolve() {
            creep.pickup(&obj)
        } else {
            ReturnCode::NotFound
        }
    }

    pub fn withdraw_resource_amount(&self, creep: &Creep, resource: ResourceType, amount: u32) -> ReturnCode {
        match self {
            TransferTarget::Container(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Spawn(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Extension(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Storage(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Tower(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Link(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Ruin(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Tombstone(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Resource(id) => Self::pickup_resource_from_id(id, creep),
        }
    }

    fn transfer_resource_amount_to_id<T>(target: &RemoteObjectId<T>, creep: &Creep, resource: ResourceType, amount: u32) -> ReturnCode where T: Transferable + HasStore + HasId + SizedRoomObject {
        if let Some(obj) = target.resolve() {
            let transfer_amount = obj.store_free_capacity(Some(resource)).min(amount);

            creep.transfer_amount(&obj, resource, transfer_amount)
        } else {
            ReturnCode::NotFound
        }
    }

    pub fn transfer_resource_amount(&self, creep: &Creep, resource: ResourceType, amount: u32) -> ReturnCode {
        match self {
            TransferTarget::Container(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Spawn(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Extension(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Storage(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Tower(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Link(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Ruin(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Tombstone(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            //TODO: Split pickup and deposit targets.
            TransferTarget::Resource(_) => { panic!("Attempting to transfer resources to a dropped resource.") }
        }
    }
}

pub struct TransferNodePriorityEntry {
    priority: TransferPriority,
    amount: u32,
}

pub struct TransferNode {
    withdrawls: HashMap<ResourceType, TransferNodePriorityEntry>,
    pending_withdrawls: HashMap<ResourceType, u32>,
    deposits: HashMap<Option<ResourceType>, TransferNodePriorityEntry>,
    pending_deposits: HashMap<Option<ResourceType>, u32>,
}

impl TransferNode {
    pub fn new() -> TransferNode {
        TransferNode {
            withdrawls: HashMap::new(),
            pending_withdrawls: HashMap::new(),
            deposits: HashMap::new(),
            pending_deposits: HashMap::new(),
        }
    }

    pub fn request_withdraw(&mut self, resource: ResourceType, priority: TransferPriority, amount: u32) {
        self.withdrawls
            .entry(resource)
            .and_modify(|e| {
                e.priority = e.priority.max(priority);
                e.amount += amount;
            })
            .or_insert_with(|| TransferNodePriorityEntry { priority, amount });
    }

    pub fn request_deposit(&mut self, resource: Option<ResourceType>, priority: TransferPriority, amount: u32) {
        self.deposits
            .entry(resource)
            .and_modify(|e| {
                e.priority = e.priority.max(priority);
                e.amount += amount;
            })
            .or_insert_with(|| TransferNodePriorityEntry { priority, amount });
    }

    pub fn register_pickup(&mut self, withdrawls: &HashMap<ResourceType, u32>) {
        for (resource, amount) in withdrawls {
            self.pending_withdrawls
                .entry(*resource)
                .and_modify(|e| *e += amount)
                .or_insert(*amount);
        }
    }

    pub fn register_delivery(&mut self, deposits: &HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>>) {
        for resource_entries in deposits.values() {
            for resource_entry in resource_entries {
                self.pending_deposits
                    .entry(resource_entry.target_resource)
                    .and_modify(|e| *e += resource_entry.amount)
                    .or_insert(resource_entry.amount);
            }
        }
    }

    pub fn select_pickup(&self, allowed_priorities: TransferPriorityFlags, available_capacity: u32) -> HashMap<ResourceType, u32> {
        let mut pickup_resources = HashMap::new();
        let mut remaining_capacity = available_capacity;

        for (resource, withdrawl) in self.withdrawls.iter() {
            if (TransferPriorityFlags::from(withdrawl.priority) & allowed_priorities) != TransferPriorityFlags::UNSET {
                let pending_withdrawl_amount = *(self.pending_withdrawls.get(resource).unwrap_or(&0)) as i32;
                let remaining_amount = withdrawl.amount as i32 - pending_withdrawl_amount;

                if remaining_amount > 0 {
                    let pickup_amount = (remaining_amount as u32).min(remaining_capacity);

                    pickup_resources
                        .entry(*resource)
                        .and_modify(|e| *e += pickup_amount)
                        .or_insert(pickup_amount);

                    remaining_capacity -= pickup_amount;
                }
            }
        }

        pickup_resources
    }

    //TODO: Likely just want min priority, not compare actual priority.
    pub fn select_delivery(
        &self,
        allowed_priorities: TransferPriorityFlags,
        available_resources: &HashMap<ResourceType, u32>,
    ) -> HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> {
        let mut delivery_resources = HashMap::new();
        let mut used_any = 0;

        for (resource, amount) in available_resources.iter() {
            if let Some(deposit) = self.deposits.get(&Some(*resource)) {
                if (TransferPriorityFlags::from(deposit.priority) & allowed_priorities) != TransferPriorityFlags::UNSET {
                    let pending_deposit_amount = *(self.pending_deposits.get(&Some(*resource)).unwrap_or(&0)) as i32;
                    let remaining_amount = deposit.amount as i32 - pending_deposit_amount;

                    if remaining_amount > 0 {
                        let delivery_amount = remaining_amount.min(*amount as i32);

                        delivery_resources
                            .entry(*resource)
                            .or_insert_with(Vec::new)
                            .push(TransferDepositTicketResourceEntry {
                                target_resource: Some(*resource),
                                amount: delivery_amount as u32,
                            });
                    }
                }
            }

            if let Some(deposit) = self.deposits.get(&None) {
                if (TransferPriorityFlags::from(deposit.priority) & allowed_priorities) != TransferPriorityFlags::UNSET {
                    let pending_deposit_amount = *(self.pending_deposits.get(&None).unwrap_or(&0)) as i32;
                    let remaining_amount = (deposit.amount as i32 - pending_deposit_amount) - used_any;

                    if remaining_amount > 0 {
                        let delivery_amount = remaining_amount.min(*amount as i32);

                        delivery_resources
                            .entry(*resource)
                            .or_insert_with(Vec::new)
                            .push(TransferDepositTicketResourceEntry {
                                target_resource: None,
                                amount: delivery_amount as u32,
                            });

                        used_any += delivery_amount;
                    }
                }
            }
        }

        delivery_resources
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer, pos: RoomPosition) {
        let withdraw_text = self
            .withdrawls
            .iter()
            .map(|(resource, withdrawl)| format!("{:?} {:?} {:?}", resource, withdrawl.priority, withdrawl.amount));

        let pending_withdraw_text = self
            .pending_withdrawls
            .iter()
            .map(|(resource, amount)| format!("{:?} {}", resource, amount));

        let deposit_text = self
            .deposits
            .iter()
            .map(|(resource, deposit)| format!("{:?} {:?} {:?}", resource, deposit.priority, deposit.amount));

        let pending_deposit_text = self
            .pending_deposits
            .iter()
            .map(|(resource, amount)| format!("{:?} {}", resource, amount));

        let full_text = withdraw_text
            .chain(pending_withdraw_text)
            .chain(deposit_text)
            .chain(pending_deposit_text)
            .join("\n");

        //TODO: Use priority and color to visualize.
        visualizer.text(pos.x() as f32, pos.y() as f32, full_text, None);
    }
}

pub struct TransferWithdrawRequest {
    target: TransferTarget,
    resource: ResourceType,
    priority: TransferPriority,
    amount: u32,
}

impl TransferWithdrawRequest {
    pub fn new(target: TransferTarget, resource: ResourceType, priority: TransferPriority, amount: u32) -> TransferWithdrawRequest {
        TransferWithdrawRequest {
            target,
            resource,
            priority,
            amount,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TransferWithdrawTicket {
    target: TransferTarget,
    resources: HashMap<ResourceType, u32>,
}

impl TransferWithdrawTicket {
    pub fn target(&self) -> &TransferTarget {
        &self.target
    }

    pub fn resources(&self) -> &HashMap<ResourceType, u32> {
        &self.resources
    }

    pub fn combine_with(&mut self, other: &TransferWithdrawTicket) {
        for (resource, amount) in other.resources.iter() {
            self.resources.entry(*resource).and_modify(|e| *e += amount).or_insert(*amount);
        }
    }

    pub fn get_next_withdrawl(&self) -> Option<(ResourceType, u32)> {
        self.resources.iter().next().map(|(r, a)| (*r, *a))
    }

    pub fn consume_withdrawl(&mut self, resource: ResourceType, amount: u32) {
        if let Entry::Occupied(mut e) = self.resources.entry(resource) {
            *e.get_mut() -= amount;

            if *e.get() == 0 {
                e.remove();
            }
        }
    }
}

pub struct TransferDepositRequest {
    target: TransferTarget,
    resource: Option<ResourceType>,
    priority: TransferPriority,
    amount: u32,
}

impl TransferDepositRequest {
    pub fn new(target: TransferTarget, resource: Option<ResourceType>, priority: TransferPriority, amount: u32) -> TransferDepositRequest {
        TransferDepositRequest {
            target,
            resource,
            priority,
            amount,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferDepositTicketResourceEntry {
    target_resource: Option<ResourceType>,
    amount: u32,
}

impl TransferDepositTicketResourceEntry {
    pub fn target_resource(&self) -> Option<ResourceType> {
        self.target_resource
    }

    pub fn amount(&self) -> u32 {
        self.amount
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferDepositTicket {
    target: TransferTarget,
    resources: HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>>,
}

impl TransferDepositTicket {
    pub fn target(&self) -> &TransferTarget {
        &self.target
    }

    pub fn resources(&self) -> &HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> {
        &self.resources
    }

    pub fn combine_with(&mut self, other: &TransferDepositTicket) {
        for (resource, entries) in other.resources.iter() {
            self.resources
                .entry(*resource)
                .and_modify(|existing| {
                    for entry in entries {
                        if let Some(deposit_resource_entry) = existing.iter_mut().find(|oe| oe.target_resource == entry.target_resource) {
                            deposit_resource_entry.amount += entry.amount;
                        } else {
                            existing.push(entry.clone());
                        }
                    }
                })
                .or_insert_with(|| entries.clone());
        }
    }

    pub fn get_next_deposit(&self) -> Option<(ResourceType, u32)> {
        self.resources
            .iter()
            .next()
            .map(|(resource, entries)| (*resource, entries.iter().map(|e| e.amount).sum::<u32>()))
    }

    pub fn consume_deposit(&mut self, resource: ResourceType, amount: u32) {
        if let Entry::Occupied(mut e) = self.resources.entry(resource) {
            let mut remaining_amount = amount;

            let entries = e.get_mut();

            for entry in entries.iter_mut() {
                let consumed_amount = entry.amount.min(remaining_amount);

                entry.amount -= consumed_amount;
                remaining_amount -= consumed_amount
            }

            entries.retain(|entry| entry.amount > 0);

            if entries.is_empty() {
                e.remove();
            }
        }
    }
}

pub struct TransferQueueRoomStatsData {
    pub total_withdrawl: u32,
    withdrawl_priorities_by_resource: HashMap<ResourceType, TransferPriorityFlags>,
    withdrawl_priorities: TransferPriorityFlags,
    pub total_deposit: u32,
    deposit_priorities_by_resource: HashMap<Option<ResourceType>, TransferPriorityFlags>,
    deposit_priorities: TransferPriorityFlags,
}

impl TransferQueueRoomStatsData {
    pub fn new() -> TransferQueueRoomStatsData {
        TransferQueueRoomStatsData {
            total_withdrawl: 0,
            withdrawl_priorities_by_resource: HashMap::new(),
            withdrawl_priorities: TransferPriorityFlags::UNSET,
            total_deposit: 0,
            deposit_priorities_by_resource: HashMap::new(),
            deposit_priorities: TransferPriorityFlags::UNSET,
        }
    }
}

pub struct TransferQueueRoomData {
    nodes: HashMap<TransferTarget, TransferNode>,
    stats: TransferQueueRoomStatsData,
}

impl TransferQueueRoomData {
    pub fn new() -> TransferQueueRoomData {
        TransferQueueRoomData {
            nodes: HashMap::new(),
            stats: TransferQueueRoomStatsData::new(),
        }
    }

    pub fn stats(&self) -> &TransferQueueRoomStatsData {
        &self.stats
    }
}

impl TransferQueueRoomData {
    pub fn get_node(&mut self, target: &TransferTarget) -> &mut TransferNode {
        self.nodes.entry(*target).or_insert_with(TransferNode::new)
    }

    pub fn try_get_node(&self, target: &TransferTarget) -> Option<&TransferNode> {
        self.nodes.get(target)
    }
}

#[derive(Default)]
pub struct TransferQueue {
    rooms: HashMap<RoomName, TransferQueueRoomData>,
}

impl TransferQueue {
    pub fn get_room(&mut self, room: RoomName) -> &mut TransferQueueRoomData {
        self.rooms.entry(room).or_insert_with(TransferQueueRoomData::new)
    }

    pub fn try_get_room(&self, room: RoomName) -> Option<&TransferQueueRoomData> {
        self.rooms.get(&room)
    }

    pub fn request_withdraw(&mut self, withdraw_request: TransferWithdrawRequest) {
        let room = self.get_room(withdraw_request.target.pos().room_name());
        room.stats.total_withdrawl += withdraw_request.amount;
        let priority_flag = withdraw_request.priority.into();
        room.stats.withdrawl_priorities |= priority_flag;
        room.stats
            .withdrawl_priorities_by_resource
            .entry(withdraw_request.resource)
            .and_modify(|e| *e |= priority_flag)
            .or_insert(priority_flag);

        let node = room.get_node(&withdraw_request.target);
        node.request_withdraw(withdraw_request.resource, withdraw_request.priority, withdraw_request.amount);
    }

    pub fn request_deposit(&mut self, deposit_request: TransferDepositRequest) {
        let room = self.get_room(deposit_request.target.pos().room_name());
        room.stats.total_deposit += deposit_request.amount;
        let priority_flag = deposit_request.priority.into();
        room.stats.deposit_priorities |= priority_flag;
        room.stats
            .deposit_priorities_by_resource
            .entry(deposit_request.resource)
            .and_modify(|e| *e = priority_flag)
            .or_insert(priority_flag);

        let node = room.get_node(&deposit_request.target);
        node.request_deposit(deposit_request.resource, deposit_request.priority, deposit_request.amount);
    }

    pub fn register_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        let room = self.get_room(ticket.target.pos().room_name());
        let node = room.get_node(&ticket.target);

        node.register_pickup(&ticket.resources);
    }

    pub fn register_delivery(&mut self, ticket: &TransferDepositTicket) {
        let room = self.get_room(ticket.target.pos().room_name());
        let node = room.get_node(&ticket.target);

        node.register_delivery(&ticket.resources);
    }

    //TODO: How does this handle None priority? (Likely need to make sure there is a delivery needed, then pick top priority.)
    pub fn select_pickup(
        &mut self,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        current_position: RoomPosition,
        available_capacity: u32,
    ) -> Option<TransferWithdrawTicket> {
        rooms
            .iter()
            .filter_map(|room_name| self.rooms.get(room_name))
            .filter(|room| (room.stats.withdrawl_priorities & allowed_priorities) != TransferPriorityFlags::UNSET)
            .flat_map(|room| room.nodes.iter())
            .filter_map(|(target, node)| {
                let pickup_resources = node.select_pickup(allowed_priorities, available_capacity);

                if !pickup_resources.is_empty() {
                    Some(TransferWithdrawTicket {
                        target: *target,
                        resources: pickup_resources,
                    })
                } else {
                    None
                }
            })
            .find_nearest_linear_by(current_position, |ticket| ticket.target().pos())
    }

    pub fn request_additional_pickup(
        &mut self,
        ticket: &TransferWithdrawTicket,
        additional_capacity: u32,
    ) -> Option<TransferWithdrawTicket> {
        let target = ticket.target();

        let room = self.try_get_room(target.pos().room_name())?;
        let node = room.try_get_node(&target)?;

        //TODO: Sanity check this makes sense with priority. In theory if the creep is already visiting this node the priority
        //      irrelevant if it wants more resources.
        let pickup_resources = node.select_pickup(TransferPriorityFlags::ALL, additional_capacity);

        if !pickup_resources.is_empty() {
            Some(TransferWithdrawTicket {
                target: *target,
                resources: pickup_resources,
            })
        } else {
            None
        }
    }

    pub fn select_delivery(
        &mut self,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        current_position: RoomPosition,
        available_resources: &HashMap<ResourceType, u32>,
    ) -> Option<TransferDepositTicket> {
        rooms
            .iter()
            .filter_map(|room_name| self.rooms.get(room_name))
            .filter(|room| (room.stats.deposit_priorities & allowed_priorities) != TransferPriorityFlags::UNSET)
            .flat_map(|room| room.nodes.iter())
            .filter_map(|(target, node)| {
                let delivery_resources = node.select_delivery(allowed_priorities, available_resources);

                if !delivery_resources.is_empty() {
                    Some(TransferDepositTicket {
                        target: *target,
                        resources: delivery_resources,
                    })
                } else {
                    None
                }
            })
            //TODO: Use real path distance.
            .find_nearest_linear_by(current_position, |ticket| ticket.target().pos())
    }

    pub fn request_additional_delivery(
        &self,
        ticket: &TransferDepositTicket,
        available_resources: &HashMap<ResourceType, u32>,
    ) -> Option<TransferDepositTicket> {
        let target = ticket.target();

        let room = self.try_get_room(target.pos().room_name())?;
        let node = room.try_get_node(&target)?;

        //TODO: Sanity check this makes sense with priority. In theory if the creep is already visiting this node the priority
        //      irrelevant if it wants to dump resources.
        let delivery_resources = node.select_delivery(TransferPriorityFlags::ALL, available_resources);

        if !delivery_resources.is_empty() {
            Some(TransferDepositTicket {
                target: *target,
                resources: delivery_resources,
            })
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
    }

    fn visualize(&self, ui: &mut UISystem, visualizer: &mut Visualizer) {
        for (room_name, room) in &self.rooms {
            ui.with_room(*room_name, visualizer, |room_ui| {
                for (target, node) in &room.nodes {
                    node.visualize(room_ui.visualizer(), target.pos());
                }
            });
        }
    }
}

#[derive(SystemData)]
pub struct TransferQueueSystemData<'a> {
    transfer_queue: Write<'a, TransferQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, ::room::data::RoomData>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

pub struct TransferQueueExecutionSystemData<'a> {
    pub updater: Read<'a, LazyUpdate>,
}

pub struct TransferQueueSystem;

impl<'a> System<'a> for TransferQueueSystem {
    type SystemData = TransferQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                data.transfer_queue.visualize(ui, visualizer);
            }
        }

        data.transfer_queue.clear();
    }
}
