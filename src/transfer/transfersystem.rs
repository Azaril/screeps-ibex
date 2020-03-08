use crate::ui::*;
use crate::visualize::*;
use itertools::*;
use remoteobjectid::*;
use screeps::*;
use serde::*;
use specs::prelude::{ResourceId, WriteStorage, Write, LazyUpdate, Read, Entities, SystemData, System, World};
use std::collections::hash_map::*;
use std::collections::HashMap;
#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
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
        const ACTIVE = Self::HIGH.bits | Self::MEDIUM.bits | Self::LOW.bits;
    }
}

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
    Resource(RemoteObjectId<Resource>),
    Terminal(RemoteObjectId<StructureTerminal>),
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
            TransferTarget::Terminal(id) => Self::is_valid_from_id(id),
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
            TransferTarget::Terminal(id) => id.pos(),
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
            TransferTarget::Terminal(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
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
            TransferTarget::Terminal(id) => Self::transfer_resource_amount_to_id(id, creep, resource, amount),
            //TODO: Split pickup and deposit targets.
            TransferTarget::Ruin(_) => { panic!("Attempting to transfer resources to a dropped resource.") },
            TransferTarget::Tombstone(_) => { panic!("Attempting to transfer resources to a dropped resource.") },
            TransferTarget::Resource(_) => { panic!("Attempting to transfer resources to a dropped resource.") },
        }
    }
}

pub struct TransferNode {
    withdrawls: HashMap<ResourceType, HashMap<TransferPriority, u32>>,
    pending_withdrawls: HashMap<ResourceType, HashMap<TransferPriority, u32>>,
    deposits: HashMap<Option<ResourceType>, HashMap<TransferPriority, u32>>,
    pending_deposits: HashMap<Option<ResourceType>, HashMap<TransferPriority, u32>>,
}

#[cfg_attr(feature = "time", timing)]
impl TransferNode {
    pub fn new() -> TransferNode {
        TransferNode {
            withdrawls: HashMap::new(),
            pending_withdrawls: HashMap::new(),
            deposits: HashMap::new(),
            pending_deposits: HashMap::new(),
        }
    }

    pub fn get_available_withdrawl(&self, resource: ResourceType, priority: TransferPriority) -> u32 {
        let available = self.withdrawls.get(&resource).and_then(|e| e.get(&priority)).unwrap_or(&0);
        let pending = self.pending_withdrawls.get(&resource).and_then(|e| e.get(&priority)).unwrap_or(&0);

        ((*available as i32) - (*pending as i32)).max(0) as u32
    }

    pub fn get_available_withdrawl_by_priority(&self, priority: TransferPriority) -> HashMap<ResourceType, u32> {
        self.withdrawls
            .keys()
            .filter_map(|resource| {
                let total = self.get_available_withdrawl(*resource, priority);

                if total > 0 {
                    Some((*resource, total))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_available_withdrawl_by_priorities(&self, priorities: TransferPriorityFlags) -> HashMap<ResourceType, u32> {
        self.withdrawls
            .keys()
            .filter_map(|resource| {
                let total = ALL_TRANSFER_PRIORITIES
                    .iter()
                    .filter(|priority| priorities.contains((*priority).into()))
                    .map(|priority| self.get_available_withdrawl(*resource, *priority))
                    .sum();

                if total > 0 {
                    Some((*resource, total))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_available_deposit(&self, resource: Option<ResourceType>, priority: TransferPriority) -> u32 {
        let available = self.deposits.get(&resource).and_then(|e| e.get(&priority)).unwrap_or(&0);
        let pending = self.pending_deposits.get(&resource).and_then(|e| e.get(&priority)).unwrap_or(&0);

        ((*available as i32) - (*pending as i32)).max(0) as u32
    }

    pub fn request_withdraw(&mut self, resource: ResourceType, priority: TransferPriority, amount: u32) {
        self.withdrawls
            .entry(resource)
            .or_insert_with(HashMap::new)
            .entry(priority)
            .and_modify(|e| {
                *e += amount;
            })
            .or_insert(amount);
    }

    pub fn request_deposit(&mut self, resource: Option<ResourceType>, priority: TransferPriority, amount: u32) {
        self.deposits
            .entry(resource)
            .or_insert_with(HashMap::new)
            .entry(priority)
            .and_modify(|e| {
                *e += amount;
            })
            .or_insert(amount);
    }

    pub fn register_pickup(&mut self, withdrawls: &HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>>) {
        for (resource, resource_entries) in withdrawls {
            for resource_entry in resource_entries {
                self.pending_withdrawls
                    .entry(*resource)
                    .or_insert_with(HashMap::new)
                    .entry(resource_entry.priority)
                    .and_modify(|e| *e += resource_entry.amount)
                    .or_insert(resource_entry.amount);
            }
        }
    }

    pub fn register_delivery(&mut self, deposits: &HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>>) {
        for resource_entries in deposits.values() {
            for resource_entry in resource_entries {
                self.pending_deposits
                    .entry(resource_entry.target_resource)
                    .or_insert_with(HashMap::new)
                    .entry(resource_entry.priority)
                    .and_modify(|e| *e += resource_entry.amount)
                    .or_insert(resource_entry.amount);
            }
        }
    }

    pub fn select_pickup(&self, allowed_priorities: TransferPriorityFlags, desired_resources: &HashMap<Option<ResourceType>, u32>, available_capacity: TransferCapacity) -> HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>> {
        let mut pickup_resources: HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>> = HashMap::new();

        let mut remaining_capacity = available_capacity;

        let mut fill_none = None;

        for (desired_resource, amount) in desired_resources {
            if let Some(resource) = desired_resource {
                if let Some(withdrawls) = self.withdrawls.get(resource) {
                    for priority in withdrawls.keys() {
                        if allowed_priorities.contains(priority.into()) {
                            let remaining_amount = self.get_available_withdrawl(*resource, *priority);

                            if remaining_amount > 0 {
                                let pickup_amount = remaining_capacity.clamp((remaining_amount as u32).min(*amount));

                                pickup_resources
                                    .entry(*resource)
                                    .or_insert_with(Vec::new)
                                    .push(TransferWithdrawlTicketResourceEntry {
                                        amount: pickup_amount as u32,
                                        priority: *priority
                                    });

                                remaining_capacity.consume(pickup_amount);
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

        if let Some(amount) = fill_none {
            let mut remaining_none_amount = TransferCapacity::Finite(amount);

            for (resource, withdrawls) in &self.withdrawls {
                for priority in withdrawls.keys() {
                    if allowed_priorities.contains(priority.into()) {
                        let remaining_amount = self.get_available_withdrawl(*resource, *priority);

                        if remaining_amount > 0 {
                            let pickedup_resources = pickup_resources
                                .get(resource)
                                .map(|entries| {
                                    entries
                                        .iter()
                                        .filter(|e| e.priority == *priority)
                                        .map(|e| e.amount)
                                        .sum()
                                })
                                .unwrap_or(0);

                            let unconsumed_remaining_amount = remaining_amount - pickedup_resources;

                            if unconsumed_remaining_amount > 0 {
                                let pickup_amount = remaining_none_amount.clamp(remaining_capacity.clamp(unconsumed_remaining_amount as u32));

                                pickup_resources
                                    .entry(*resource)
                                    .or_insert_with(Vec::new)
                                    .push(TransferWithdrawlTicketResourceEntry {
                                        amount: pickup_amount as u32,
                                        priority: *priority
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

                if remaining_capacity.empty() || remaining_none_amount.empty() {
                    break;
                }
            }
        }

        pickup_resources
    }

    pub fn select_delivery(
        &self,
        allowed_priorities: TransferPriorityFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity
    ) -> HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> {
        let mut delivery_resources: HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> = HashMap::new();
        let mut remaining_capacity = available_capacity;

        for (resource, amount) in available_resources {
            if let Some(deposits) = self.deposits.get(&Some(*resource)) {
                for priority in deposits.keys() {
                    if allowed_priorities.contains(priority.into()) {
                        let remaining_amount = self.get_available_deposit(Some(*resource), *priority);

                        if remaining_amount > 0 {
                            let delivery_amount = remaining_capacity.clamp((remaining_amount as u32).min(*amount));

                            delivery_resources
                                .entry(*resource)
                                .or_insert_with(Vec::new)
                                .push(TransferDepositTicketResourceEntry {
                                    target_resource: Some(*resource),
                                    amount: delivery_amount as u32,
                                    priority: *priority
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

        if let Some(deposits) = self.deposits.get(&None) {
            for priority in deposits.keys() {
                if allowed_priorities.contains(priority.into()) {
                    let mut remaining_none_amount = TransferCapacity::Finite(self.get_available_deposit(None, *priority).max(0) as u32);

                    if !remaining_none_amount.empty() {
                        for (resource, amount) in available_resources {
                            let deposited_resources = delivery_resources
                                .get(resource)
                                .map(|entries| {
                                    entries
                                        .iter()
                                        .filter(|e| e.priority == *priority)
                                        .map(|e| e.amount)
                                        .sum()
                                })
                                .unwrap_or(0);

                            let unconsumed_remaining_amount = amount - deposited_resources;

                            if unconsumed_remaining_amount > 0 {
                                let delivery_amount = remaining_none_amount.clamp(remaining_capacity.clamp(unconsumed_remaining_amount as u32));

                                delivery_resources
                                    .entry(*resource)
                                    .or_insert_with(Vec::new)
                                    .push(TransferDepositTicketResourceEntry {
                                        target_resource: None,
                                        amount: delivery_amount as u32,
                                        priority: *priority
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

                if remaining_capacity.empty() {
                    break;
                }
            }
        }

        delivery_resources
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer, pos: RoomPosition) {
        let withdraw_text = self
            .withdrawls
            .iter()
            .flat_map(|(resource, entries)| {
                entries
                    .iter()
                    .map(move |(priority, amount)| format!("{:?} {:?} {:?}", resource, priority, amount))
            });

        let pending_withdraw_text = self
            .pending_withdrawls
            .iter()
            .flat_map(|(resource, entries)| {
                entries
                    .iter()
                    .map(move |(priority, amount)| format!("{:?} {:?} {:?}", resource, priority, amount))
            });

        let deposit_text = self
            .deposits
            .iter()
            .flat_map(|(resource, entries)| {
                entries
                    .iter()
                    .map(move |(priority, amount)| format!("{:?} {:?} {:?}", resource, priority, amount))
            });

        let pending_deposit_text = self
            .pending_deposits
            .iter()
            .flat_map(|(resource, entries)| {
                entries
                    .iter()
                    .map(move |(priority, amount)| format!("{:?} {:?} {:?}", resource, priority, amount))
            });

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

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferWithdrawlTicketResourceEntry {
    amount: u32,
    priority: TransferPriority
}

impl TransferWithdrawlTicketResourceEntry {
    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn priority(&self) -> TransferPriority {
        self.priority
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferWithdrawTicket {
    target: TransferTarget,
    resources: HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>>,
}

impl TransferWithdrawTicket {
    pub fn target(&self) -> &TransferTarget {
        &self.target
    }

    pub fn resources(&self) -> &HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>> {
        &self.resources
    }

    pub fn combine_with(&mut self, other: &TransferWithdrawTicket) {
        for (resource, entries) in other.resources.iter() {
            self.resources
                .entry(*resource)
                .and_modify(|existing| {
                    for entry in entries {
                        if let Some(withdrawl_resource_entry) = existing.iter_mut().find(|oe| oe.priority == entry.priority) {
                            withdrawl_resource_entry.amount += entry.amount;
                        } else {
                            existing.push(entry.clone());
                        }
                    }
                })
                .or_insert_with(|| entries.clone());
        }
    }

    pub fn get_next_withdrawl(&self) -> Option<(ResourceType, u32)> {
        let entry = self.resources.iter().next();

        entry.map(|(resource, entries)| {
            let resource_amount = entries.iter().map(|e| e.amount).sum();

            (*resource, resource_amount)
        })
    }

    pub fn consume_withdrawl(&mut self, resource: ResourceType, amount: u32) {
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
    priority: TransferPriority
}

impl TransferDepositTicketResourceEntry {
    pub fn target_resource(&self) -> Option<ResourceType> {
        self.target_resource
    }

    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn priority(&self) -> TransferPriority {
        self.priority
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
                        if let Some(deposit_resource_entry) = existing.iter_mut().find(|oe| oe.target_resource == entry.target_resource && oe.priority == entry.priority) {
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

pub struct TransferQueueResourceStatsData {
    amount: u32,
    pending_amount: u32,
}

impl TransferQueueResourceStatsData {
    pub fn new() -> TransferQueueResourceStatsData {
        TransferQueueResourceStatsData {
            amount: 0,
            pending_amount: 0,
        }
    }

    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn pending_amount(&self) -> u32 {
        self.amount
    }

    pub fn unfufilled_amount(&self) -> i32 {
        (self.amount as i32) - (self.pending_amount as i32)
    }
}

pub struct TransferQueueRoomStatsData {
    pub total_withdrawl: u32,
    pub total_active_withdrawl: u32,
    withdrawl_resource_stats: HashMap<ResourceType, HashMap<TransferPriority, TransferQueueResourceStatsData>>,
    withdrawl_priorities: TransferPriorityFlags,
    pub total_deposit: u32,
    pub total_active_deposit: u32,
    deposit_resource_stats: HashMap<Option<ResourceType>, HashMap<TransferPriority, TransferQueueResourceStatsData>>,
    deposit_priorities: TransferPriorityFlags,
}

impl TransferQueueRoomStatsData {
    pub fn new() -> TransferQueueRoomStatsData {
        TransferQueueRoomStatsData {
            total_withdrawl: 0,
            total_active_withdrawl: 0,
            withdrawl_resource_stats: HashMap::new(),
            withdrawl_priorities: TransferPriorityFlags::UNSET,
            total_deposit: 0,
            total_active_deposit: 0,
            deposit_resource_stats: HashMap::new(),
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

#[derive(Copy, Clone, Debug)]
pub enum TransferCapacity {
    Infinite,
    Finite(u32)
}

impl TransferCapacity {
    pub fn empty(self) -> bool {
        match self {
            TransferCapacity::Infinite => false,
            TransferCapacity::Finite(current) => current == 0
        }
    }

    pub fn consume(&mut self, amount: u32) {
        match self {
            TransferCapacity::Infinite => {},
            TransferCapacity::Finite(current) => { *current -= amount; },
        }
    }

    pub fn clamp(self, amount: u32) -> u32 {
        match self {
            TransferCapacity::Infinite => amount,
            TransferCapacity::Finite(current) => amount.min(current)
        }
    }
}

#[cfg_attr(feature = "time", timing)]
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

        if TransferPriorityFlags::ACTIVE.contains(priority_flag) {
            room.stats.total_active_withdrawl += withdraw_request.amount;
        }
        
        let resource_stats = room.stats
            .withdrawl_resource_stats
            .entry(withdraw_request.resource)
            .or_insert_with(HashMap::new)
            .entry(withdraw_request.priority)
            .or_insert_with(TransferQueueResourceStatsData::new);

        resource_stats.amount += withdraw_request.amount;

        let node = room.get_node(&withdraw_request.target);
        node.request_withdraw(withdraw_request.resource, withdraw_request.priority, withdraw_request.amount);
    }

    pub fn request_deposit(&mut self, deposit_request: TransferDepositRequest) {
        let room = self.get_room(deposit_request.target.pos().room_name());
        room.stats.total_deposit += deposit_request.amount;

        let priority_flag = deposit_request.priority.into();
        room.stats.deposit_priorities |= priority_flag;

        if TransferPriorityFlags::ACTIVE.contains(priority_flag) {
            room.stats.total_active_deposit += deposit_request.amount;
        }

        let resource_stats = room.stats
            .deposit_resource_stats
            .entry(deposit_request.resource)
            .or_insert_with(HashMap::new)
            .entry(deposit_request.priority)
            .or_insert_with(TransferQueueResourceStatsData::new);

        resource_stats.amount += deposit_request.amount;

        let node = room.get_node(&deposit_request.target);
        node.request_deposit(deposit_request.resource, deposit_request.priority, deposit_request.amount);
    }

    pub fn register_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        let room = self.get_room(ticket.target.pos().room_name());
        for (resource, entries) in ticket.resources() {
            for entry in entries {
                let resource_stats = room
                    .stats
                    .withdrawl_resource_stats
                    .entry(*resource)
                    .or_insert_with(HashMap::new)
                    .entry(entry.priority)
                    .or_insert_with(TransferQueueResourceStatsData::new);

                    resource_stats.amount += entry.amount;
            }
        }

        let node = room.get_node(&ticket.target);
        node.register_pickup(&ticket.resources);
    }

    pub fn register_delivery(&mut self, ticket: &TransferDepositTicket) {
        let room = self.get_room(ticket.target.pos().room_name());

        for entries in ticket.resources().values() {
            for entry in entries {
                let resource_stats = room
                    .stats
                    .deposit_resource_stats
                    .entry(entry.target_resource)
                    .or_insert_with(HashMap::new)
                    .entry(entry.priority)
                    .or_insert_with(TransferQueueResourceStatsData::new);

                resource_stats.amount += entry.amount;
            }
        }

        let node = room.get_node(&ticket.target);
        node.register_delivery(&ticket.resources);
    }

    pub fn select_pickups(
        &mut self,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        desired_resources: &HashMap<Option<ResourceType>, u32>,
        available_capacity: TransferCapacity
    ) -> Vec<TransferWithdrawTicket> {
        rooms
            .iter()
            .filter_map(|room_name| self.rooms.get(room_name))
            .filter(|room| room.stats.withdrawl_priorities.intersects(allowed_priorities))
            .flat_map(|room| room.nodes.iter())
            .filter_map(|(target, node)| {
                let pickup_resources = node.select_pickup(allowed_priorities, desired_resources, available_capacity);

                if !pickup_resources.is_empty() {
                    Some(TransferWithdrawTicket {
                        target: *target,
                        resources: pickup_resources,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn select_deliveries(
        &mut self,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity
    ) -> Vec<TransferDepositTicket> {
        rooms
            .iter()
            .filter_map(|room_name| self.rooms.get(room_name))
            .filter(|room| room.stats.deposit_priorities.intersects(allowed_priorities))
            .flat_map(|room| room.nodes.iter())
            .filter_map(|(target, node)| {
                let delivery_resources = node.select_delivery(allowed_priorities, available_resources, available_capacity);

                if !delivery_resources.is_empty() {
                    Some(TransferDepositTicket {
                        target: *target,
                        resources: delivery_resources,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_available_withdrawl_totals(&self, rooms: &[RoomName], withdrawl_priority: TransferPriority) -> HashMap<ResourceType, u32> {
        let mut available_resources: HashMap<_, u32> = HashMap::new();

        for room_name in rooms {
            if let Some(room) = self.try_get_room(*room_name) {
                for (resource, entries) in &room.stats.withdrawl_resource_stats {
                    if let Some(entry) = entries.get(&withdrawl_priority) {
                        let current_amount = available_resources
                            .entry(*resource)
                            .or_insert(0);

                        let unfufilled_amount = entry.unfufilled_amount();

                        if unfufilled_amount > 0 {
                            *current_amount += unfufilled_amount as u32;
                        }
                    }
                }
            }
        }

        available_resources
    }

    pub fn get_available_deposit_totals(&self, rooms: &[RoomName], deposit_priority: TransferPriority) -> HashMap<Option<ResourceType>, u32> {
        let mut available_resources: HashMap<_, u32> = HashMap::new();

        for room_name in rooms {
            if let Some(room) = self.try_get_room(*room_name) {
                for (resource, entries) in &room.stats.deposit_resource_stats {
                    if let Some(entry) = entries.get(&deposit_priority) {
                        let current_amount = available_resources
                            .entry(*resource)
                            .or_insert(0);

                        let unfufilled_amount = entry.unfufilled_amount();

                        if unfufilled_amount > 0 {
                            *current_amount += unfufilled_amount as u32;
                        }
                    }
                }
            }
        }

        available_resources
    }

    pub fn select_best_delivery(&mut self,
        rooms: &[RoomName],
        pickup_priority: TransferPriority,
        delivery_priority: TransferPriority,
        current_position: RoomPosition,
        available_capacity: TransferCapacity) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    {
        if available_capacity.empty() {
            return None;
        }

        let global_available_resources = self.get_available_withdrawl_totals(rooms, pickup_priority);

        if global_available_resources.is_empty() {
            return None;
        }

        self.select_deliveries(rooms, delivery_priority.into(), &global_available_resources, available_capacity)
            .iter()
            .map(|delivery| {
                let mut delivery_resources = HashMap::new();
                
                for entries in delivery.resources.values() {
                    for entry in entries.iter() {
                        delivery_resources
                            .entry(entry.target_resource)
                            .and_modify(|e| *e += entry.amount)
                            .or_insert(entry.amount);
                    }
                }

                let pickups = self.select_pickups(rooms, pickup_priority.into(), &delivery_resources, available_capacity);

                (pickups, delivery)
            })
            .flat_map(|(pickups, delivery)| {
                let delivery_pos = delivery.target().pos();
                
                pickups
                    .into_iter()
                    .map(move |pickup| {
                        let pickup_pos = pickup.target.pos();
                        let pickup_length = current_position.get_range_to(&pickup_pos);

                        let delivery_length = pickup_pos.get_range_to(&delivery_pos);

                        let resources = pickup.resources.iter().flat_map(|(_, entries)| entries.iter().map(|e| e.amount)).sum::<u32>();
                        let value = (resources as f32) / (pickup_length as f32 + delivery_length as f32);

                        (pickup, delivery, value)
                    })
                })
            .max_by(|(_, _, a), (_, _, b)| {
                a.partial_cmp(b).unwrap()
            })
            .map(|(pickup, delivery, _)| {
                (pickup, delivery.clone())
            })
    }

    pub fn get_additional_delivery_from_target(
        &mut self,
        rooms: &[RoomName],
        target: &TransferTarget,
        allowed_priorities: TransferPriorityFlags,
        available_capacity: TransferCapacity,
        anchor_location: RoomPosition
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)> {
        if available_capacity.empty() {
            return None;
        }

        let available_resources = self
            .try_get_room(target.pos().room_name())
            .and_then(|r| r.try_get_node(target))
            .map(|n| n.get_available_withdrawl_by_priorities(TransferPriorityFlags::ALL))?;

        if available_resources.is_empty() {
            return None;
        }

        let delivery = self.get_additional_delivery(rooms, allowed_priorities, &available_resources, available_capacity, anchor_location)?;
            
        let delivery_resources = delivery
            .resources()
            .iter()
            .map(|(resource, entries)| {
                let total = entries.iter().map(|entry| entry.amount).sum();

                (Some(*resource), total)
            })
            .collect();

        let node = self
            .try_get_room(target.pos().room_name())
            .and_then(|r| r.try_get_node(target))?;

        //
        // NOTE: Priority is ignored here as it's already known that the delivery priority is allowed. Additionally,
        //       the node is already being visited so it's worthwhile picking up any resource that can be transfered
        //       on the route.
        //

        let pickup = TransferWithdrawTicket {
            target: *target,
            resources: node.select_pickup(TransferPriorityFlags::ALL, &delivery_resources, available_capacity)
        };
        
        Some((pickup, delivery))
    }

    pub fn get_additional_delivery(
        &mut self,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
        anchor_location: RoomPosition
    ) -> Option<TransferDepositTicket> {
        if available_capacity.empty() {
            return None;
        }

        self.select_deliveries(rooms, allowed_priorities, &available_resources, available_capacity)
            .iter()
            .map(|delivery| {
                let resources = delivery.resources.iter().flat_map(|(_, entries)| entries.iter().map(|e| e.amount)).sum::<u32>();

                let length = anchor_location.get_range_to(&delivery.target.pos());
                let value = (resources as f32) / (length as f32);

                (delivery, value)
            })
            .max_by(|(_, a), (_, b)| {
                a.partial_cmp(b).unwrap()
            })
            .map(|(delivery, _)| delivery.clone())
    }

    pub fn select_pickup_and_delivery(
        &mut self,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        current_position: RoomPosition,
        available_capacity: TransferCapacity
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)> {
        let mut priorities = ALL_TRANSFER_PRIORITIES.iter().cartesian_product(ALL_TRANSFER_PRIORITIES.iter())
            .filter(|(&p1, &p2)| allowed_priorities.contains(p1.into()) || allowed_priorities.contains(p2.into()))
            .collect_vec();
            
        priorities.sort_by(|(a_1, a_2), (b_1, b_2)| {
            a_1.max(a_2).cmp(b_1.max(b_2))
                .then_with(|| a_1.cmp(b_1))
                .then_with(|| a_2.cmp(b_2))
        });      
        
        for (pickup_priority, delivery_priority) in priorities {
            if let Some((pickup_ticket, delivery_ticket)) = self.select_best_delivery(rooms, *pickup_priority, *delivery_priority, current_position, available_capacity) {
                return Some((pickup_ticket, delivery_ticket));
            }
        }

        None
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
    }

    fn visualize(&self, ui: &mut UISystem, visualizer: &mut Visualizer) {
        if crate::features::transfer::visualize_demand() {
            for (room_name, room) in &self.rooms {
                ui.with_room(*room_name, visualizer, |room_ui| {
                    for (target, node) in &room.nodes {
                        node.visualize(room_ui.visualizer(), target.pos());
                    }
                });
            }
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
