use super::utility::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::ui::*;
use crate::visualize::*;
use bitflags::*;
use itertools::*;
use log::*;
use screeps::*;
use serde::*;
use specs::prelude::{Entities, Entity, LazyUpdate, Read, ResourceId, System, SystemData, World, Write, WriteStorage};
use std::borrow::*;
use std::collections::hash_map::*;
use std::collections::HashMap;
use std::collections::HashSet;

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum TransferPriority {
    High = 0,
    Medium = 1,
    Low = 2,
    None = 3,
}

pub const ACTIVE_TRANSFER_PRIORITIES: &[TransferPriority] = &[TransferPriority::High, TransferPriority::Medium, TransferPriority::Low];
pub const ALL_TRANSFER_PRIORITIES: &[TransferPriority] = &[
    TransferPriority::High,
    TransferPriority::Medium,
    TransferPriority::Low,
    TransferPriority::None,
];

bitflags! {
    #[derive(Copy, Clone)]
    pub struct TransferPriorityFlags: u8 {
        const UNSET = 0;

        const HIGH = 1u8 << (TransferPriority::High as u8);
        const MEDIUM = 1u8 << (TransferPriority::Medium as u8);
        const LOW = 1u8 << (TransferPriority::Low as u8);
        const NONE = 1u8 << (TransferPriority::None as u8);

        const ALL = Self::HIGH.bits() | Self::MEDIUM.bits() | Self::LOW.bits() | Self::NONE.bits();
        const ACTIVE = Self::HIGH.bits() | Self::MEDIUM.bits() | Self::LOW.bits();
    }
}

impl<T> From<T> for TransferPriorityFlags
where
    T: Borrow<TransferPriority>,
{
    fn from(priority: T) -> TransferPriorityFlags {
        match priority.borrow() {
            TransferPriority::High => TransferPriorityFlags::HIGH,
            TransferPriority::Medium => TransferPriorityFlags::MEDIUM,
            TransferPriority::Low => TransferPriorityFlags::LOW,
            TransferPriority::None => TransferPriorityFlags::NONE,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub enum TransferType {
    Haul = 0,
    Link = 1,
    Terminal = 2,
    Use = 3,
}

bitflags! {
    #[derive(Copy, Clone)]
    pub struct TransferTypeFlags: u8 {
        const UNSET = 0;

        const HAUL = 1u8 << (TransferType::Haul as u8);
        const LINK = 1u8 << (TransferType::Link as u8);
        const TERMINAL = 1u8 << (TransferType::Terminal as u8);
        const USE = 1u8 << (TransferType::Use as u8);
    }
}

impl<T> From<T> for TransferTypeFlags
where
    T: Borrow<TransferType>,
{
    fn from(transfer_type: T) -> TransferTypeFlags {
        match transfer_type.borrow() {
            TransferType::Haul => TransferTypeFlags::HAUL,
            TransferType::Link => TransferTypeFlags::LINK,
            TransferType::Terminal => TransferTypeFlags::TERMINAL,
            TransferType::Use => TransferTypeFlags::USE,
        }
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
    Lab(RemoteObjectId<StructureLab>),
    Factory(RemoteObjectId<StructureFactory>),
    Nuker(RemoteObjectId<StructureNuker>),
    PowerSpawn(RemoteObjectId<StructurePowerSpawn>),
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TransferTarget {
    fn is_valid_from_id<T>(target: &RemoteObjectId<T>) -> bool
    where
        T: HasId + wasm_bindgen::JsCast,
    {
        if game::rooms().get(target.pos().room_name()).is_some() {
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
            TransferTarget::Lab(id) => Self::is_valid_from_id(id),
            TransferTarget::Factory(id) => Self::is_valid_from_id(id),
            TransferTarget::Nuker(id) => Self::is_valid_from_id(id),
            TransferTarget::PowerSpawn(id) => Self::is_valid_from_id(id),
        }
    }

    pub fn pos(&self) -> RoomPosition {
        match self {
            TransferTarget::Container(id) => id.pos().into(),
            TransferTarget::Spawn(id) => id.pos().into(),
            TransferTarget::Extension(id) => id.pos().into(),
            TransferTarget::Storage(id) => id.pos().into(),
            TransferTarget::Tower(id) => id.pos().into(),
            TransferTarget::Link(id) => id.pos().into(),
            TransferTarget::Ruin(id) => id.pos().into(),
            TransferTarget::Tombstone(id) => id.pos().into(),
            TransferTarget::Resource(id) => id.pos().into(),
            TransferTarget::Terminal(id) => id.pos().into(),
            TransferTarget::Lab(id) => id.pos().into(),
            TransferTarget::Factory(id) => id.pos().into(),
            TransferTarget::Nuker(id) => id.pos().into(),
            TransferTarget::PowerSpawn(id) => id.pos().into(),
        }
    }

    fn withdraw_resource_amount_from_id<T>(target: &RemoteObjectId<T>, creep: &Creep, resource: ResourceType, amount: u32) -> Result<(), ErrorCode>
    where
        T: Withdrawable + HasStore + HasId + wasm_bindgen::JsCast,
    {
        if let Some(obj) = target.resolve() {
            let withdraw_amount = obj.store().get_used_capacity(Some(resource)).min(amount);

            creep.withdraw(&obj, resource, Some(withdraw_amount)).map_err(Into::into)
        } else {
            Err(ErrorCode::NotFound)
        }
    }

    fn pickup_resource_from_id(target: &RemoteObjectId<Resource>, creep: &Creep) -> Result<(), ErrorCode> {
        if let Some(obj) = target.resolve() {
            creep.pickup(&obj).map_err(Into::into)
        } else {
            Err(ErrorCode::NotFound)
        }
    }

    pub fn withdraw_resource_amount(&self, creep: &Creep, resource: ResourceType, amount: u32) -> Result<(), ErrorCode> {
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
            TransferTarget::Lab(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            TransferTarget::Factory(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
            //TODO: Split pickup and deposit targets.
            TransferTarget::Nuker(_id) => panic!("Attempting to withdraw resources from a nuker."),
            TransferTarget::PowerSpawn(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
        }
    }

    fn creep_transfer_resource_amount_to_id<T>(target: &RemoteObjectId<T>, creep: &Creep, resource: ResourceType, amount: u32) -> Result<(), ErrorCode>
    where
        T: Transferable + HasStore + HasId + wasm_bindgen::JsCast,
    {
        if let Some(obj) = target.resolve() {
            let transfer_amount = obj.store().get_free_capacity(Some(resource)).min(amount as i32);

            if transfer_amount > 0 {
                creep.transfer(&obj, resource, Some(transfer_amount as u32)).map_err(Into::into)
            } else {
                Err(ErrorCode::InvalidArgs)
            }
        } else {
            Err(ErrorCode::NotFound)
        }
    }

    pub fn creep_transfer_resource_amount(&self, creep: &Creep, resource: ResourceType, amount: u32) -> Result<(), ErrorCode> {
        match self {
            TransferTarget::Container(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Spawn(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Extension(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Storage(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Tower(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Link(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Terminal(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Lab(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Factory(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::Nuker(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            TransferTarget::PowerSpawn(id) => Self::creep_transfer_resource_amount_to_id(id, creep, resource, amount),
            //TODO: Split pickup and deposit targets.
            TransferTarget::Ruin(_) => panic!("Attempting to transfer resources to a ruin."),
            TransferTarget::Tombstone(_) => panic!("Attempting to transfer resources to a tombstone."),
            TransferTarget::Resource(_) => panic!("Attempting to transfer resources to a dropped resource."),
        }
    }

    fn link_transfer_energy_amount_to_id(target: &RemoteObjectId<StructureLink>, link: &StructureLink, amount: u32) -> Result<(), ErrorCode> {
        if let Some(obj) = target.resolve() {
            let transfer_amount = obj.store().get_free_capacity(Some(ResourceType::Energy)).min(amount as i32);

            if transfer_amount > 0 {
                link.transfer_energy(&obj, Some(transfer_amount as u32)).map_err(Into::into)
            } else {
                Err(ErrorCode::InvalidArgs)
            }
        } else {
            Err(ErrorCode::NotFound)
        }
    }

    //TODO: This is a bad API.
    pub fn link_transfer_energy_amount(&self, link: &StructureLink, amount: u32) -> Result<(), ErrorCode> {
        match self {
            TransferTarget::Container(_) => panic!("Attempting to link transfer resources to a container!"),
            TransferTarget::Spawn(_) => panic!("Attempting to link transfer resources to a spawn!"),
            TransferTarget::Extension(_) => panic!("Attempting to link transfer resources to a extension!"),
            TransferTarget::Storage(_) => panic!("Attempting to link transfer resources to a storage!"),
            TransferTarget::Tower(_) => panic!("Attempting to link transfer resources to a tower!"),
            TransferTarget::Link(id) => Self::link_transfer_energy_amount_to_id(id, link, amount),
            TransferTarget::Terminal(_) => panic!("Attempting to link transfer resources to a container!"),
            TransferTarget::Lab(_) => panic!("Attempting to link transfer resources to a container!"),
            TransferTarget::Factory(_) => panic!("Attempting to link transfer resources to a factory!"),
            TransferTarget::Nuker(_) => panic!("Attempting to link transfer resources to a nuker!"),
            TransferTarget::PowerSpawn(_) => panic!("Attempting to link transfer resources to a power spawn!"),
            TransferTarget::Ruin(_) => panic!("Attempting to link transfer resources to a ruin!"),
            TransferTarget::Tombstone(_) => panic!("Attempting to link transfer resources to a tombstone!"),
            TransferTarget::Resource(_) => panic!("Attempting to link transfer resources to a resource!"),
        }
    }
}

pub mod target_filters {
    use super::*;

    pub fn all(_: &TransferTarget) -> bool {
        true
    }

    pub fn storage(target: &TransferTarget) -> bool {
        match target {
            TransferTarget::Container(_) => true,
            TransferTarget::Storage(_) => true,
            TransferTarget::Terminal(_) => true,
            _ => false,
        }
    }

    pub fn link(target: &TransferTarget) -> bool {
        match target {
            TransferTarget::Link(_) => true,
            _ => false,
        }
    }

    pub fn terminal(target: &TransferTarget) -> bool {
        match target {
            TransferTarget::Terminal(_) => true,
            _ => false,
        }
    }
}

impl std::convert::TryFrom<&StructureObject> for TransferTarget {
    type Error = ();

    fn try_from(val: &StructureObject) -> Result<TransferTarget, ()> {
        match val {
            StructureObject::StructureContainer(s) => Ok(s.into()),
            StructureObject::StructureSpawn(s) => Ok(s.into()),
            StructureObject::StructureExtension(s) => Ok(s.into()),
            StructureObject::StructureStorage(s) => Ok(s.into()),
            StructureObject::StructureTower(s) => Ok(s.into()),
            StructureObject::StructureLink(s) => Ok(s.into()),
            StructureObject::StructureTerminal(s) => Ok(s.into()),
            StructureObject::StructureLab(s) => Ok(s.into()),
            StructureObject::StructureFactory(s) => Ok(s.into()),
            StructureObject::StructureNuker(s) => Ok(s.into()),
            StructureObject::StructurePowerSpawn(s) => Ok(s.into()),
            _ => Err(()),
        }
    }
}

impl From<&StructureContainer> for TransferTarget {
    fn from(val: &StructureContainer) -> TransferTarget {
        TransferTarget::Container(val.remote_id())
    }
}

impl From<&StructureSpawn> for TransferTarget {
    fn from(val: &StructureSpawn) -> TransferTarget {
        TransferTarget::Spawn(val.remote_id())
    }
}

impl From<&StructureExtension> for TransferTarget {
    fn from(val: &StructureExtension) -> TransferTarget {
        TransferTarget::Extension(val.remote_id())
    }
}

impl From<&StructureStorage> for TransferTarget {
    fn from(val: &StructureStorage) -> TransferTarget {
        TransferTarget::Storage(val.remote_id())
    }
}

impl From<&StructureTower> for TransferTarget {
    fn from(val: &StructureTower) -> TransferTarget {
        TransferTarget::Tower(val.remote_id())
    }
}

impl From<&StructureLink> for TransferTarget {
    fn from(val: &StructureLink) -> TransferTarget {
        TransferTarget::Link(val.remote_id())
    }
}

impl From<&StructureTerminal> for TransferTarget {
    fn from(val: &StructureTerminal) -> TransferTarget {
        TransferTarget::Terminal(val.remote_id())
    }
}

impl From<&Ruin> for TransferTarget {
    fn from(val: &Ruin) -> TransferTarget {
        TransferTarget::Ruin(val.remote_id())
    }
}

impl From<&Tombstone> for TransferTarget {
    fn from(val: &Tombstone) -> TransferTarget {
        TransferTarget::Tombstone(val.remote_id())
    }
}

impl From<&Resource> for TransferTarget {
    fn from(val: &Resource) -> TransferTarget {
        TransferTarget::Resource(val.remote_id())
    }
}

impl From<&StructureLab> for TransferTarget {
    fn from(val: &StructureLab) -> TransferTarget {
        TransferTarget::Lab(val.remote_id())
    }
}

impl From<&StructureFactory> for TransferTarget {
    fn from(val: &StructureFactory) -> TransferTarget {
        TransferTarget::Factory(val.remote_id())
    }
}

impl From<&StructureNuker> for TransferTarget {
    fn from(val: &StructureNuker) -> TransferTarget {
        TransferTarget::Nuker(val.remote_id())
    }
}

impl From<&StructurePowerSpawn> for TransferTarget {
    fn from(val: &StructurePowerSpawn) -> TransferTarget {
        TransferTarget::PowerSpawn(val.remote_id())
    }
}

#[derive(Eq, PartialEq, Hash, Clone, Copy)]
pub struct TransferWithdrawlKey {
    resource: ResourceType,
    priority: TransferPriority,
    allowed_type: TransferType,
}

impl TransferWithdrawlKey {
    pub fn matches(&self, resource: ResourceType, allowed_priorities: TransferPriorityFlags, allowed_types: TransferTypeFlags) -> bool {
        self.resource == resource && allowed_priorities.intersects(self.priority.into()) && allowed_types.intersects(self.allowed_type.into())
    }
}

#[derive(Eq, PartialEq, Hash, Clone, Copy)]
pub struct TransferDepositKey {
    resource: Option<ResourceType>,
    priority: TransferPriority,
    allowed_type: TransferType,
}

impl TransferDepositKey {
    pub fn matches(
        &self,
        resource: Option<ResourceType>,
        allowed_priorities: TransferPriorityFlags,
        allowed_types: TransferTypeFlags,
    ) -> bool {
        self.resource == resource && allowed_priorities.intersects(self.priority.into()) && allowed_types.intersects(self.allowed_type.into())
    }
}

pub struct TransferNode {
    withdrawls: HashMap<TransferWithdrawlKey, u32>,
    pending_withdrawls: HashMap<TransferWithdrawlKey, u32>,
    deposits: HashMap<TransferDepositKey, u32>,
    pending_deposits: HashMap<TransferDepositKey, u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TransferNode {
    pub fn new() -> TransferNode {
        TransferNode {
            withdrawls: HashMap::new(),
            pending_withdrawls: HashMap::new(),
            deposits: HashMap::new(),
            pending_deposits: HashMap::new(),
        }
    }

    pub fn get_withdrawl(&self, key: &TransferWithdrawlKey) -> u32 {
        self.withdrawls.get(key).copied().unwrap_or(0)
    }

    pub fn get_pending_withdrawl(&self, key: &TransferWithdrawlKey) -> u32 {
        self.pending_withdrawls.get(key).copied().unwrap_or(0)
    }

    pub fn get_available_withdrawl(&self, key: &TransferWithdrawlKey) -> u32 {
        ((self.get_withdrawl(key) as i32) - (self.get_pending_withdrawl(key) as i32)).max(0) as u32
    }

    pub fn get_deposit(&self, key: &TransferDepositKey) -> u32 {
        self.deposits.get(key).copied().unwrap_or(0)
    }

    pub fn get_pending_deposit(&self, key: &TransferDepositKey) -> u32 {
        self.pending_deposits.get(key).copied().unwrap_or(0)
    }

    pub fn get_available_deposit(&self, key: &TransferDepositKey) -> u32 {
        ((self.get_deposit(key) as i32) - (self.get_pending_deposit(key) as i32)).max(0) as u32
    }

    pub fn get_available_withdrawl_by_resource(
        &self,
        transfer_types: TransferTypeFlags,
        allowed_priorities: TransferPriorityFlags,
        resource: ResourceType,
    ) -> u32 {
        let mut available_resources: u32 = 0;

        for key in self.withdrawls.keys().filter(|key| {
            allowed_priorities.intersects(key.priority.into()) && transfer_types.intersects(key.allowed_type.into()) && key.resource == resource
        }) {
            available_resources += self.get_available_withdrawl(key);
        }

        available_resources
    }

    pub fn get_available_withdrawl_totals(
        &self,
        transfer_types: TransferTypeFlags,
        allowed_priorities: TransferPriorityFlags,
    ) -> HashMap<ResourceType, u32> {
        let mut available_resources: HashMap<ResourceType, u32> = HashMap::new();

        for key in self
            .withdrawls
            .keys()
            .filter(|key| allowed_priorities.intersects(key.priority.into()) && transfer_types.intersects(key.allowed_type.into()))
        {
            let available = self.get_available_withdrawl(key);

            if available > 0 {
                let current = available_resources.entry(key.resource).or_insert(0);

                *current += available;
            }
        }

        available_resources
    }

    pub fn request_withdraw(&mut self, key: TransferWithdrawlKey, amount: u32) {
        let current = self.withdrawls.entry(key).or_insert(0);

        *current += amount;
    }

    pub fn request_deposit(&mut self, key: TransferDepositKey, amount: u32) {
        let current = self.deposits.entry(key).or_insert(0);

        *current += amount;
    }

    pub fn register_pickup(
        &mut self,
        withdrawls: &HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>>,
    ) {
        for (resource, resource_entries) in withdrawls {
            for resource_entry in resource_entries {
                let key = TransferWithdrawlKey {
                    resource: *resource,
                    priority: resource_entry.priority,
                    allowed_type: resource_entry.transfer_type,
                };

                let current = self.pending_withdrawls.entry(key).or_insert(0);

                *current += resource_entry.amount;
            }
        }
    }

    pub fn register_delivery(
        &mut self,
        deposits: &HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>>,
    ) {
        for resource_entries in deposits.values() {
            for resource_entry in resource_entries {
                let key = TransferDepositKey {
                    resource: resource_entry.target_resource,
                    priority: resource_entry.priority,
                    allowed_type: resource_entry.transfer_type,
                };

                let current = self.pending_deposits.entry(key).or_insert(0);

                *current += resource_entry.amount;
            }
        }
    }

    pub fn select_pickup(
        &self,
        allowed_priorities: TransferPriorityFlags,
        pickup_types: TransferTypeFlags,
        desired_resources: &HashMap<Option<ResourceType>, u32>,
        available_capacity: TransferCapacity,
    ) -> HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>> {
        let mut pickup_resources: HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>> = HashMap::new();

        let mut remaining_capacity = available_capacity;

        let mut fill_none = None;

        for (desired_resource, amount) in desired_resources {
            if let Some(resource) = desired_resource {
                for key in self.withdrawls.keys() {
                    if key.matches(*resource, allowed_priorities, pickup_types) {
                        //TODO: This does a double look up on the key...
                        let remaining_amount = self.get_available_withdrawl(key);

                        if remaining_amount > 0 {
                            let pickup_amount = remaining_capacity.clamp((remaining_amount as u32).min(*amount));

                            pickup_resources
                                .entry(*resource)
                                .or_insert_with(Vec::new)
                                .push(TransferWithdrawlTicketResourceEntry {
                                    amount: pickup_amount as u32,
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

            for key in self.withdrawls.keys() {
                if allowed_priorities.intersects(key.priority.into()) && pickup_types.intersects(key.allowed_type.into()) {
                    let remaining_amount = self.get_available_withdrawl(key);

                    if remaining_amount > 0 {
                        let pickedup_resources = pickup_resources
                            .get(&key.resource)
                            .map(|entries| entries.iter().filter(|e| e.priority == key.priority).map(|e| e.amount).sum())
                            .unwrap_or(0);

                        let unconsumed_remaining_amount = remaining_amount - pickedup_resources;

                        if unconsumed_remaining_amount > 0 {
                            let pickup_amount = remaining_none_amount.clamp(remaining_capacity.clamp(unconsumed_remaining_amount as u32));

                            pickup_resources
                                .entry(key.resource)
                                .or_insert_with(Vec::new)
                                .push(TransferWithdrawlTicketResourceEntry {
                                    amount: pickup_amount as u32,
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

    pub fn select_delivery(
        &self,
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
    ) -> HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> {
        let mut delivery_resources: HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> = HashMap::new();
        let mut remaining_capacity = available_capacity;

        for (resource, amount) in available_resources {
            for key in self.deposits.keys() {
                if key.matches(Some(*resource), allowed_priorities, delivery_types) {
                    let remaining_amount = self.get_available_deposit(key);

                    if remaining_amount > 0 {
                        let delivery_amount = remaining_capacity.clamp((remaining_amount as u32).min(*amount));

                        if delivery_amount > 0 {
                            delivery_resources
                                .entry(*resource)
                                .or_insert_with(Vec::new)
                                .push(TransferDepositTicketResourceEntry {
                                    target_resource: Some(*resource),
                                    amount: delivery_amount as u32,
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

        let none_deposits = self.deposits.keys().filter(|key| {
            key.resource == None && delivery_types.intersects(key.allowed_type.into()) && allowed_priorities.intersects(key.priority.into())
        });

        for key in none_deposits {
            let mut remaining_none_amount = TransferCapacity::Finite(self.get_available_deposit(key));

            if !remaining_none_amount.empty() {
                for (resource, amount) in available_resources {
                    let deposited_resources = delivery_resources
                        .get(resource)
                        .map(|entries| entries.iter().filter(|e| e.priority == key.priority).map(|e| e.amount).sum())
                        .unwrap_or(0);

                    let unconsumed_remaining_amount = amount - deposited_resources;

                    if unconsumed_remaining_amount > 0 {
                        let delivery_amount = remaining_none_amount.clamp(remaining_capacity.clamp(unconsumed_remaining_amount as u32));

                        if delivery_amount > 0 {
                            delivery_resources
                                .entry(*resource)
                                .or_insert_with(Vec::new)
                                .push(TransferDepositTicketResourceEntry {
                                    target_resource: None,
                                    amount: delivery_amount as u32,
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

    pub fn select_single_delivery(
        &self,
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
    ) -> Option<(ResourceType, Vec<TransferDepositTicketResourceEntry>)> {
        let mut delivery_resources: HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> = HashMap::new();

        for (resource, amount) in available_resources {
            let mut remaining_capacity = available_capacity;

            for key in self.deposits.keys() {
                if key.matches(Some(*resource), allowed_priorities, delivery_types)
                    || (key.resource == None
                        && delivery_types.intersects(key.allowed_type.into())
                        && allowed_priorities.intersects(key.priority.into()))
                {
                    let remaining_amount = self.get_available_deposit(key);

                    if remaining_amount > 0 {
                        let delivery_amount = remaining_capacity.clamp((remaining_amount as u32).min(*amount));

                        if delivery_amount > 0 {
                            delivery_resources
                                .entry(*resource)
                                .or_insert_with(Vec::new)
                                .push(TransferDepositTicketResourceEntry {
                                    target_resource: Some(*resource),
                                    amount: delivery_amount as u32,
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
            .map(|(r, e)| (r, e))
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer, pos: RoomPosition) {
        let withdraw_text = self
            .withdrawls
            .iter()
            .map(|(key, amount)| format!("{:?} {:?} {:?} {:?}", key.resource, key.priority, key.allowed_type, amount));

        let pending_withdraw_text = self
            .pending_withdrawls
            .iter()
            .map(|(key, amount)| format!("{:?} {:?} {:?} {:?}", key.resource, key.priority, key.allowed_type, amount));

        let deposit_text = self
            .deposits
            .iter()
            .map(|(key, amount)| format!("{:?} {:?} {:?} {:?}", key.resource, key.priority, key.allowed_type, amount));

        let pending_deposit_text = self
            .pending_deposits
            .iter()
            .map(|(key, amount)| format!("{:?} {:?} {:?} {:?}", key.resource, key.priority, key.allowed_type, amount));

        let full_text = withdraw_text
            .chain(pending_withdraw_text)
            .chain(deposit_text)
            .chain(pending_deposit_text)
            .join("\n");

        //TODO: Use priority and color to visualize.
        visualizer.text(pos.x() as f32, pos.y() as f32, full_text, Some(TextStyle::default().font(0.3)));
    }
}

pub struct TransferWithdrawRequest {
    target: TransferTarget,
    resource: ResourceType,
    priority: TransferPriority,
    amount: u32,
    allowed_type: TransferType,
}

impl TransferWithdrawRequest {
    pub fn new(
        target: TransferTarget,
        resource: ResourceType,
        priority: TransferPriority,
        amount: u32,
        allowed_type: TransferType,
    ) -> TransferWithdrawRequest {
        TransferWithdrawRequest {
            target,
            resource,
            priority,
            amount,
            allowed_type,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferWithdrawlTicketResourceEntry {
    amount: u32,
    transfer_type: TransferType,
    priority: TransferPriority,
}

impl TransferWithdrawlTicketResourceEntry {
    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn transfer_type(&self) -> TransferType {
        self.transfer_type
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
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
                        if let Some(withdrawl_resource_entry) = existing.iter_mut().find(|oe| oe.priority == entry.priority && oe.transfer_type == entry.transfer_type) {
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
    allowed_type: TransferType,
}

impl TransferDepositRequest {
    pub fn new(
        target: TransferTarget,
        resource: Option<ResourceType>,
        priority: TransferPriority,
        amount: u32,
        allowed_type: TransferType,
    ) -> TransferDepositRequest {
        TransferDepositRequest {
            target,
            resource,
            priority,
            amount,
            allowed_type,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferDepositTicketResourceEntry {
    target_resource: Option<ResourceType>,
    amount: u32,
    transfer_type: TransferType,
    priority: TransferPriority,
}

impl TransferDepositTicketResourceEntry {
    pub fn target_resource(&self) -> Option<ResourceType> {
        self.target_resource
    }

    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn transfer_type(&self) -> TransferType {
        self.transfer_type
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
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
                        if let Some(deposit_resource_entry) = existing
                            .iter_mut()
                            .find(|oe| oe.target_resource == entry.target_resource && oe.priority == entry.priority && oe.transfer_type == entry.transfer_type)
                        {
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

    pub fn consume_deposit(&mut self, resource: ResourceType, amount: u32) -> u32 {
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

            return amount - remaining_amount;
        } else {
            return 0;
        }
    }
}

pub fn consume_resource_from_deposits(deposits: &mut [TransferDepositTicket], resource: ResourceType, amount: u32) {
    let mut remaining_to_consume = amount;

    for deposit in deposits {
        remaining_to_consume -= deposit.consume_deposit(resource, remaining_to_consume);

        if remaining_to_consume == 0 {
            break;
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
    total_withdrawl: u32,
    total_active_withdrawl: u32,
    withdrawl_resource_stats: HashMap<TransferWithdrawlKey, TransferQueueResourceStatsData>,
    withdrawl_priorities: TransferPriorityFlags,
    total_deposit: u32,
    total_active_deposit: u32,
    deposit_resource_stats: HashMap<TransferDepositKey, TransferQueueResourceStatsData>,
    deposit_priorities: TransferPriorityFlags,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
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

    pub fn total_withdrawl(&self) -> u32 {
        self.total_withdrawl
    }

    pub fn total_active_withdrawl(&self) -> u32 {
        self.total_active_withdrawl
    }

    pub fn total_deposit(&self) -> u32 {
        self.total_deposit
    }

    pub fn total_active_deposit(&self) -> u32 {
        self.total_active_deposit
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TransferQueueRoomData {
    pub fn get_node(&mut self, target: &TransferTarget) -> &mut TransferNode {
        self.nodes.entry(*target).or_insert_with(TransferNode::new)
    }

    pub fn try_get_node(&self, target: &TransferTarget) -> Option<&TransferNode> {
        self.nodes.get(target)
    }

    fn get_mut_withdrawl_stats(&mut self, key: TransferWithdrawlKey) -> &mut TransferQueueResourceStatsData {
        self.stats
            .withdrawl_resource_stats
            .entry(key)
            .or_insert_with(TransferQueueResourceStatsData::new)
    }

    fn get_mut_deposit_stats(&mut self, key: TransferDepositKey) -> &mut TransferQueueResourceStatsData {
        self.stats
            .deposit_resource_stats
            .entry(key)
            .or_insert_with(TransferQueueResourceStatsData::new)
    }
}

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

pub trait TransferRequestSystem {
    fn request_withdraw(&mut self, withdraw_request: TransferWithdrawRequest);

    fn request_deposit(&mut self, deposit_request: TransferDepositRequest);

    fn register_pickup(&mut self, ticket: &TransferWithdrawTicket);

    fn register_delivery(&mut self, ticket: &TransferDepositTicket);
}

pub struct TransferQueueGeneratorData<'a, 's, RD>
where
    RD: std::ops::Deref<Target = specs::storage::MaskedStorage<RoomData>>,
{
    //TODO: Make this private.
    pub cause: &'a str,
    pub room_data: &'a specs::storage::Storage<'s, RoomData, RD>,
}

impl<'a, 's, RD> TransferRequestSystemData for TransferQueueGeneratorData<'a, 's, RD>
where
    RD: std::ops::Deref<Target = specs::storage::MaskedStorage<RoomData>>,
{
    fn get_cause(&self) -> &str {
        self.cause
    }

    fn get_room_data(&self, entity: Entity) -> Option<&RoomData> {
        self.room_data.get(entity)
    }
}

pub trait TransferRequestSystemData {
    fn get_cause(&self) -> &str;

    fn get_room_data(&self, entity: Entity) -> Option<&RoomData>;
}

pub type TransferQueueGenerator =
    Box<dyn Fn(&dyn TransferRequestSystemData, &mut dyn TransferRequestSystem, RoomName) -> Result<(), String>>;

struct GeneratorEntry {
    transfer_types: TransferTypeFlags,
    generator: TransferQueueGenerator,
}

#[derive(Default)]
struct LazyTransferQueueRooms {
    generators: HashMap<RoomName, Vec<GeneratorEntry>>,
    rooms: HashMap<RoomName, TransferQueueRoomData>,
}

//TODO: Return a 'resolved' interface once the initial flush has happened. Right now the 'data' propagates to many objects.
impl LazyTransferQueueRooms {
    fn register_generator(&mut self, room: RoomName, transfer_types: TransferTypeFlags, generator: TransferQueueGenerator) {
        self.generators
            .entry(room)
            .or_insert_with(Vec::new)
            .push(GeneratorEntry { transfer_types, generator });
    }

    fn flush_generators(&mut self, data: &dyn TransferRequestSystemData, room: RoomName, transfer_types: TransferTypeFlags) {
        while let Some(entry) = self.get_next_generator(room, transfer_types) {
            match (entry.generator)(data, self, room) {
                Ok(_) => {}
                Err(err) => info!("Transfer information generator error: {}", err),
            }
        }
    }

    fn get_next_generator(&mut self, room: RoomName, transfer_types: TransferTypeFlags) -> Option<GeneratorEntry> {
        if let Some(generators) = self.generators.get_mut(&room) {
            if let Some((index, _)) = generators.iter().find_position(|d| d.transfer_types.intersects(transfer_types)) {
                return Some(generators.swap_remove(index));
            }
        }

        None
    }

    pub fn get_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        room: RoomName,
        transfer_types: TransferTypeFlags,
    ) -> &mut TransferQueueRoomData {
        self.flush_generators(data, room, transfer_types);

        self.get_room_no_flush(room)
    }

    pub fn get_room_no_flush(&mut self, room: RoomName) -> &mut TransferQueueRoomData {
        self.rooms.entry(room).or_insert_with(TransferQueueRoomData::new)
    }

    pub fn try_get_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        room: RoomName,
        transfer_types: TransferTypeFlags,
    ) -> Option<&TransferQueueRoomData> {
        self.flush_generators(data, room, transfer_types);

        self.try_get_room_no_flush(room)
    }

    pub fn try_get_room_no_flush(&mut self, room: RoomName) -> Option<&TransferQueueRoomData> {
        self.rooms.get(&room)
    }

    pub fn clear(&mut self) {
        self.generators.clear();
        self.rooms.clear();
    }

    pub fn get_all_rooms(&self) -> HashSet<RoomName> {
        self.generators.keys().cloned().chain(self.rooms.keys().cloned()).collect()
    }
}

impl TransferRequestSystem for LazyTransferQueueRooms {
    fn request_withdraw(&mut self, withdraw_request: TransferWithdrawRequest) {
        let room = self.get_room_no_flush(withdraw_request.target.pos().room_name());
        room.stats.total_withdrawl += withdraw_request.amount;

        let priority_flag = withdraw_request.priority.into();
        room.stats.withdrawl_priorities |= priority_flag;

        if TransferPriorityFlags::ACTIVE.intersects(priority_flag) {
            room.stats.total_active_withdrawl += withdraw_request.amount;
        }

        let key = TransferWithdrawlKey {
            resource: withdraw_request.resource,
            priority: withdraw_request.priority,
            allowed_type: withdraw_request.allowed_type,
        };

        let resource_stats = room.get_mut_withdrawl_stats(key);
        resource_stats.amount += withdraw_request.amount;

        let node = room.get_node(&withdraw_request.target);
        node.request_withdraw(key, withdraw_request.amount);
    }

    fn request_deposit(&mut self, deposit_request: TransferDepositRequest) {
        let room = self.get_room_no_flush(deposit_request.target.pos().room_name());
        room.stats.total_deposit += deposit_request.amount;

        let priority_flag = deposit_request.priority.into();
        room.stats.deposit_priorities |= priority_flag;

        if TransferPriorityFlags::ACTIVE.intersects(priority_flag) {
            room.stats.total_active_deposit += deposit_request.amount;
        }

        let key = TransferDepositKey {
            resource: deposit_request.resource,
            priority: deposit_request.priority,
            allowed_type: deposit_request.allowed_type,
        };

        let resource_stats = room.get_mut_deposit_stats(key);
        resource_stats.amount += deposit_request.amount;

        let node = room.get_node(&deposit_request.target);
        node.request_deposit(key, deposit_request.amount);
    }

    fn register_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        let room = self.get_room_no_flush(ticket.target.pos().room_name());

        for (resource, entries) in ticket.resources() {
            for entry in entries {
                let key = TransferWithdrawlKey {
                    resource: *resource,
                    priority: entry.priority,
                    allowed_type: entry.transfer_type,
                };

                let resource_stats = room.get_mut_withdrawl_stats(key);
                resource_stats.amount += entry.amount;
            }
        }

        let node = room.get_node(&ticket.target);
        node.register_pickup(&ticket.resources);
    }

    fn register_delivery(&mut self, ticket: &TransferDepositTicket) {
        let room = self.get_room_no_flush(ticket.target.pos().room_name());

        for entries in ticket.resources().values() {
            for entry in entries {
                let key = TransferDepositKey {
                    resource: entry.target_resource,
                    priority: entry.priority,
                    allowed_type: entry.transfer_type,
                };

                let resource_stats = room.get_mut_deposit_stats(key);
                resource_stats.amount += entry.amount;
            }
        }

        let node = room.get_node(&ticket.target);
        node.register_delivery(&ticket.resources);
    }
}

#[derive(Default)]
pub struct TransferQueue {
    rooms: LazyTransferQueueRooms,
}

impl TransferRequestSystem for TransferQueue {
    fn request_withdraw(&mut self, withdraw_request: TransferWithdrawRequest) {
        self.rooms.request_withdraw(withdraw_request)
    }

    fn request_deposit(&mut self, deposit_request: TransferDepositRequest) {
        self.rooms.request_deposit(deposit_request)
    }

    fn register_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        self.rooms.register_pickup(ticket)
    }

    fn register_delivery(&mut self, ticket: &TransferDepositTicket) {
        self.rooms.register_delivery(ticket)
    }
}
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TransferQueue {
    pub fn register_generator(&mut self, room: RoomName, transfer_types: TransferTypeFlags, generator: TransferQueueGenerator) {
        self.rooms.register_generator(room, transfer_types, generator)
    }

    pub fn get_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        room: RoomName,
        transfer_types: TransferTypeFlags,
    ) -> &mut TransferQueueRoomData {
        self.rooms.get_room(data, room, transfer_types)
    }

    pub fn get_all_rooms(&self) -> HashSet<RoomName> {
        self.rooms.get_all_rooms()
    }

    pub fn try_get_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        room: RoomName,
        transfer_types: TransferTypeFlags,
    ) -> Option<&TransferQueueRoomData> {
        self.rooms.try_get_room(data, room, transfer_types)
    }

    pub fn select_pickups(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        pickup_types: TransferTypeFlags,
        desired_resources: &HashMap<Option<ResourceType>, u32>,
        available_capacity: TransferCapacity,
    ) -> Vec<TransferWithdrawTicket> {
        let mut tickets = Vec::new();

        for pickup_room in pickup_rooms.iter() {
            if let Some(room) = self.try_get_room(data, *pickup_room, pickup_types) {
                if room.stats.withdrawl_priorities.intersects(allowed_priorities) {
                    for (target, node) in room.nodes.iter() {
                        let pickup_resources = node.select_pickup(allowed_priorities, pickup_types, desired_resources, available_capacity);

                        if !pickup_resources.is_empty() {
                            tickets.push(TransferWithdrawTicket {
                                target: *target,
                                resources: pickup_resources,
                            })
                        }
                    }
                }
            }
        }

        tickets
    }

    pub fn select_single_delivery(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
    ) -> Vec<TransferDepositTicket> {
        delivery_rooms
            .iter()
            .flat_map(|room| {
                self.select_single_delivery_for_room(
                    data,
                    *room,
                    allowed_priorities,
                    delivery_types,
                    available_resources,
                    available_capacity,
                )
            })
            .collect::<Vec<_>>()
    }

    pub fn select_single_delivery_for_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_room: RoomName,
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
    ) -> Vec<TransferDepositTicket> {
        let mut tickets = Vec::new();

        if let Some(room) = self.try_get_room(data, delivery_room, delivery_types) {
            if room.stats.deposit_priorities.intersects(allowed_priorities) {
                for (target, node) in room.nodes.iter() {
                    if let Some((delivery_resource, delivery_entries)) =
                        node.select_single_delivery(allowed_priorities, delivery_types, available_resources, available_capacity)
                    {
                        let mut delivery_resources = HashMap::new();

                        delivery_resources.insert(delivery_resource, delivery_entries);

                        tickets.push(TransferDepositTicket {
                            target: *target,
                            resources: delivery_resources,
                        })
                    }
                }
            }
        }

        tickets
    }

    pub fn select_deliveries<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
        target_filter: TF,
    ) -> Vec<TransferDepositTicket>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        let mut tickets = Vec::new();

        for delivery_room in delivery_rooms.iter() {
            if let Some(room) = self.try_get_room(data, *delivery_room, delivery_types) {
                if room.stats.deposit_priorities.intersects(allowed_priorities) {
                    for (target, node) in room.nodes.iter() {
                        if target_filter(target) {
                            let delivery_resources =
                                node.select_delivery(allowed_priorities, delivery_types, available_resources, available_capacity);

                            if !delivery_resources.is_empty() {
                                tickets.push(TransferDepositTicket {
                                    target: *target,
                                    resources: delivery_resources,
                                })
                            }
                        }
                    }
                }
            }
        }

        tickets
    }

    pub fn get_available_withdrawl_totals(
        &mut self,
        data: &dyn TransferRequestSystemData,
        rooms: &[RoomName],
        transfer_type: TransferType,
    ) -> HashMap<ResourceType, u32> {
        let mut available_resources: HashMap<_, u32> = HashMap::new();

        for room_name in rooms {
            if let Some(room) = self.try_get_room(data, *room_name, transfer_type.into()) {
                for (key, stats) in &room.stats().withdrawl_resource_stats {
                    if key.allowed_type == transfer_type {
                        let unfufilled_amount = stats.unfufilled_amount();

                        if unfufilled_amount > 0 {
                            let current_amount = available_resources.entry(key.resource).or_insert(0);

                            *current_amount += unfufilled_amount as u32;
                        }
                    }
                }
            }
        }

        available_resources
    }

    pub fn get_available_withdrawl_totals_by_priority(
        &mut self,
        data: &dyn TransferRequestSystemData,
        rooms: &[RoomName],
        transfer_type: TransferType,
        withdrawl_priorities: TransferPriorityFlags,
    ) -> HashMap<ResourceType, u32> {
        let mut available_resources: HashMap<_, u32> = HashMap::new();

        for room_name in rooms {
            if let Some(room) = self.try_get_room(data, *room_name, transfer_type.into()) {
                for (key, stats) in &room.stats().withdrawl_resource_stats {
                    if withdrawl_priorities.intersects(key.priority.into()) && key.allowed_type == transfer_type {
                        let unfufilled_amount = stats.unfufilled_amount();

                        if unfufilled_amount > 0 {
                            let current_amount = available_resources.entry(key.resource).or_insert(0);

                            *current_amount += unfufilled_amount as u32;
                        }
                    }
                }
            }
        }

        available_resources
    }

    pub fn get_available_deposit_totals(
        &mut self,
        data: &dyn TransferRequestSystemData,
        rooms: &[RoomName],
        deposit_priority: TransferPriority,
        transfer_type: TransferType,
    ) -> HashMap<Option<ResourceType>, u32> {
        let mut available_resources: HashMap<_, u32> = HashMap::new();

        for room_name in rooms {
            if let Some(room) = self.try_get_room(data, *room_name, transfer_type.into()) {
                for (key, stats) in &room.stats().deposit_resource_stats {
                    if key.priority == deposit_priority && key.allowed_type == transfer_type {
                        let unfufilled_amount = stats.unfufilled_amount();

                        if unfufilled_amount > 0 {
                            let current_amount = available_resources.entry(key.resource).or_insert(0);

                            *current_amount += unfufilled_amount as u32;
                        }
                    }
                }
            }
        }

        available_resources
    }

    pub fn select_best_delivery<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
        pickup_priorities: TransferPriorityFlags,
        delivery_priorities: TransferPriorityFlags,
        transfer_type: TransferType,
        current_position: RoomPosition,
        available_capacity: TransferCapacity,
        target_filter: TF,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        if available_capacity.empty() {
            return None;
        }

        let global_available_resources =
            self.get_available_withdrawl_totals_by_priority(data, pickup_rooms, transfer_type, pickup_priorities);

        if global_available_resources.is_empty() {
            return None;
        }

        self.select_deliveries(
            data,
            delivery_rooms,
            delivery_priorities,
            transfer_type.into(),
            &global_available_resources,
            available_capacity,
            target_filter,
        )
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

            let pickups = self.select_pickups(
                data,
                pickup_rooms,
                pickup_priorities,
                transfer_type.into(),
                &delivery_resources,
                available_capacity,
            );

            (pickups, delivery)
        })
        .flat_map(|(pickups, delivery)| {
            let delivery_pos = delivery.target().pos();
            let current_position = current_position.clone();

            pickups.into_iter().map(move |pickup| {
                let pickup_pos = pickup.target.pos();
                let pickup_length = current_position.get_range_to(&pickup_pos);

                let delivery_length = pickup_pos.get_range_to(&delivery_pos);

                let resources = pickup
                    .resources
                    .iter()
                    .flat_map(|(_, entries)| entries.iter().map(|e| e.amount))
                    .sum::<u32>();
                let value = (resources as f32) / (pickup_length as f32 + delivery_length as f32);

                (pickup, delivery, value)
            })
        })
        .max_by(|(_, _, a), (_, _, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(pickup, delivery, _)| (pickup, delivery.clone()))
    }

    pub fn get_terminal_delivery_from_target(
        &mut self,
        data: &dyn TransferRequestSystemData,
        target: &TransferTarget,
        allowed_pickup_priorities: TransferPriorityFlags,
        allowed_delivery_priorities: TransferPriorityFlags,
        delivery_type: TransferType,
        available_transfer_energy: u32,
        available_capacity: TransferCapacity,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)> {
        if available_capacity.empty() {
            return None;
        }

        let available_resources = self
            .try_get_room(data, target.pos().room_name(), delivery_type.into())
            .and_then(|room| room.try_get_node(target))
            .map(|node| node.get_available_withdrawl_totals(delivery_type.into(), allowed_pickup_priorities))?;

        if available_resources.is_empty() {
            return None;
        }

        let source_room = target.pos().room_name();

        let mut all_rooms = self.get_all_rooms();

        all_rooms.remove(&source_room);

        let target_rooms = all_rooms.into_iter().collect::<Vec<_>>();

        let delivery = self.get_terminal_delivery(
            data,
            &target_rooms,
            allowed_delivery_priorities,
            delivery_type.into(),
            available_transfer_energy,
            &available_resources,
            available_capacity,
            source_room,
        )?;

        let delivery_resources = delivery
            .resources()
            .iter()
            .map(|(resource, entries)| {
                let total = entries.iter().map(|entry| entry.amount).sum();

                (Some(*resource), total)
            })
            .collect();

        let node = self
            .try_get_room(data, target.pos().room_name(), delivery_type.into())
            .and_then(|r| r.try_get_node(target))?;

        let pickup = TransferWithdrawTicket {
            target: *target,
            resources: node.select_pickup(
                allowed_pickup_priorities,
                delivery_type.into(),
                &delivery_resources,
                available_capacity,
            ),
        };

        Some((pickup, delivery))
    }

    pub fn get_pickup_from_target(
        &mut self,
        data: &dyn TransferRequestSystemData,
        target: &TransferTarget,
        allowed_pickup_priorities: TransferPriorityFlags,
        transfer_types: TransferTypeFlags,
        available_capacity: TransferCapacity,
        resource_type: ResourceType,
    ) -> Option<TransferWithdrawTicket> {
        if available_capacity.empty() {
            return None;
        }

        let node = self
            .try_get_room(data, target.pos().room_name(), transfer_types)
            .and_then(|room| room.try_get_node(target))?;

        let resource_amount = available_capacity.clamp(u32::MAX);

        let mut desired_resources = HashMap::new();

        desired_resources.insert(Some(resource_type), resource_amount);

        let pickup_resources = node.select_pickup(allowed_pickup_priorities, transfer_types, &desired_resources, available_capacity);

        if pickup_resources.is_empty() {
            return None;
        }

        let pickup_ticket = TransferWithdrawTicket {
            target: *target,
            resources: pickup_resources,
        };

        Some(pickup_ticket)
    }

    pub fn get_delivery_from_target<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_rooms: &[RoomName],
        target: &TransferTarget,
        allowed_pickup_priorities: TransferPriorityFlags,
        allowed_delivery_priorities: TransferPriorityFlags,
        delivery_type: TransferType,
        available_capacity: TransferCapacity,
        anchor_location: RoomPosition,
        target_filter: TF,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        if available_capacity.empty() {
            return None;
        }

        let available_resources = self
            .try_get_room(data, target.pos().room_name(), delivery_type.into())
            .and_then(|room| room.try_get_node(target))
            .map(|node| node.get_available_withdrawl_totals(delivery_type.into(), allowed_pickup_priorities))?;

        if available_resources.is_empty() {
            return None;
        }

        let delivery = self.get_delivery(
            data,
            delivery_rooms,
            allowed_delivery_priorities,
            delivery_type.into(),
            &available_resources,
            available_capacity,
            anchor_location,
            target_filter,
        )?;

        let delivery_resources = delivery
            .resources()
            .iter()
            .map(|(resource, entries)| {
                let total = entries.iter().map(|entry| entry.amount).sum();

                (Some(*resource), total)
            })
            .collect();

        let node = self
            .try_get_room(data, target.pos().room_name(), delivery_type.into())
            .and_then(|r| r.try_get_node(target))?;

        let pickup = TransferWithdrawTicket {
            target: *target,
            resources: node.select_pickup(
                allowed_pickup_priorities,
                delivery_type.into(),
                &delivery_resources,
                available_capacity,
            ),
        };

        Some((pickup, delivery))
    }

    pub fn get_delivery<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
        anchor_location: RoomPosition,
        target_filter: TF,
    ) -> Option<TransferDepositTicket>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        if available_capacity.empty() {
            return None;
        }

        self.select_deliveries(
            data,
            delivery_rooms,
            allowed_priorities,
            delivery_types,
            &available_resources,
            available_capacity,
            target_filter,
        )
        .iter()
        .map(|delivery| {
            let resources = delivery
                .resources
                .iter()
                .flat_map(|(_, entries)| entries.iter().map(|e| e.amount))
                .sum::<u32>();

            let length = anchor_location.get_range_to(&delivery.target.pos());
            let value = (resources as f32) / (length as f32);

            (delivery, value)
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(delivery, _)| delivery.clone())
    }

    pub fn get_terminal_delivery(
        &mut self,
        data: &dyn TransferRequestSystemData,
        rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_transfer_energy: u32,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
        anchor_location: RoomName,
    ) -> Option<TransferDepositTicket> {
        if available_capacity.empty() {
            return None;
        }

        rooms
            .iter()
            .flat_map(|room| {
                let cost_per_unit = super::utility::calc_transaction_cost_fractional(anchor_location, *room);

                let max_resources = (available_transfer_energy as f64 / cost_per_unit).floor() as u32;

                let capacity = TransferCapacity::Finite(available_capacity.clamp(max_resources));

                self.select_single_delivery_for_room(data, *room, allowed_priorities, delivery_types, available_resources, capacity)
            })
            .map(|delivery| {
                let resources = delivery
                    .resources
                    .iter()
                    .flat_map(|(_, entries)| entries.iter().map(|e| e.amount))
                    .sum::<u32>();

                let to = delivery.target.pos().room_name();

                let cost_per_unit = super::utility::calc_transaction_cost_fractional(anchor_location, to);

                let cost = (cost_per_unit * resources as f64).ceil();
                let value = (resources as f32) / (cost as f32);

                (delivery, value)
            })
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(delivery, _)| delivery.clone())
    }

    pub fn select_pickup_and_delivery<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        transfer_type: TransferType,
        current_position: RoomPosition,
        available_capacity: TransferCapacity,
        target_filter: TF,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    where
        TF: Fn(&TransferTarget) -> bool + Copy,
    {
        let priorities = generate_active_priorities(allowed_priorities, allowed_priorities);

        for (pickup_priorities, delivery_priorities) in priorities {
            if let Some((pickup_ticket, delivery_ticket)) = self.select_best_delivery(
                data,
                pickup_rooms,
                delivery_rooms,
                pickup_priorities,
                delivery_priorities,
                transfer_type,
                current_position.clone(),
                available_capacity,
                target_filter,
            ) {
                return Some((pickup_ticket, delivery_ticket));
            }
        }

        None
    }

    pub fn total_unfufilled_resources(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
        transfer_type: TransferType,
    ) -> HashMap<ResourceType, u32> {
        struct StatsEntry {
            active: u32,
            inactive: u32,
        }

        let mut withdrawls: HashMap<ResourceType, StatsEntry> = HashMap::new();
        let mut deposits: HashMap<Option<ResourceType>, StatsEntry> = HashMap::new();

        let mut total_pickup: HashMap<ResourceType, u32> = HashMap::new();

        let mut add_resource = |resource: ResourceType, amount: u32| {
            let current = total_pickup.entry(resource).or_insert(0);

            *current += amount;
        };

        //
        // Get current unfufilled requests.
        //

        for pickup_room in pickup_rooms {
            if let Some(room) = self.try_get_room(data, *pickup_room, transfer_type.into()) {
                for (key, stats) in &room.stats.withdrawl_resource_stats {
                    if key.allowed_type == transfer_type {
                        let resource_entry = withdrawls.entry(key.resource).or_insert(StatsEntry { active: 0, inactive: 0 });

                        if TransferPriorityFlags::ACTIVE.intersects(key.priority.into()) {
                            resource_entry.active += stats.unfufilled_amount().max(0) as u32;
                        } else {
                            resource_entry.inactive += stats.unfufilled_amount().max(0) as u32;
                        }
                    }
                }
            }
        }

        for pickup_room in delivery_rooms {
            if let Some(room) = self.try_get_room(data, *pickup_room, transfer_type.into()) {
                for (key, stats) in &room.stats.deposit_resource_stats {
                    if key.allowed_type == transfer_type {
                        let resource_entry = deposits.entry(key.resource).or_insert(StatsEntry { active: 0, inactive: 0 });

                        if TransferPriorityFlags::ACTIVE.intersects(key.priority.into()) {
                            resource_entry.active += stats.unfufilled_amount().max(0) as u32;
                        } else {
                            resource_entry.inactive += stats.unfufilled_amount().max(0) as u32;
                        }
                    }
                }
            }
        }

        //
        // Active <-> Active
        //

        for (resource, deposit_stats) in &mut deposits {
            if let Some(resource) = resource {
                if let Some(withdrawl_stats) = withdrawls.get_mut(&resource) {
                    let consume = withdrawl_stats.active.min(deposit_stats.active);

                    withdrawl_stats.active -= consume;
                    deposit_stats.active -= consume;

                    add_resource(*resource, consume);
                }
            }
        }

        for (resource, deposit_stats) in &mut deposits {
            if let None = resource {
                for (other_resource, withdrawl_stats) in &mut withdrawls {
                    let consume = withdrawl_stats.active.min(deposit_stats.active);

                    withdrawl_stats.active -= consume;
                    deposit_stats.active -= consume;

                    add_resource(*other_resource, consume);
                }
            }
        }

        //
        // Inactive -> Active
        //

        for (resource, deposit_stats) in &mut deposits {
            if let Some(resource) = resource {
                if let Some(withdrawl_stats) = withdrawls.get_mut(&resource) {
                    let consume = withdrawl_stats.inactive.min(deposit_stats.active);

                    withdrawl_stats.inactive -= consume;
                    deposit_stats.active -= consume;

                    add_resource(*resource, consume);
                }
            }
        }

        for (resource, deposit_stats) in &mut deposits {
            if let None = resource {
                for (other_resource, withdrawl_stats) in &mut withdrawls {
                    let consume = withdrawl_stats.inactive.min(deposit_stats.active);

                    withdrawl_stats.inactive -= consume;
                    deposit_stats.active -= consume;

                    add_resource(*other_resource, consume);
                }
            }
        }

        //
        // Active -> Inactive
        //

        for (resource, withdrawl_stats) in &mut withdrawls {
            if let Some(deposit_stats) = deposits.get_mut(&Some(*resource)) {
                let consume = withdrawl_stats.active.min(deposit_stats.inactive);

                withdrawl_stats.active -= consume;
                deposit_stats.inactive -= consume;

                add_resource(*resource, consume);
            }
        }

        for (resource, withdrawl_stats) in &mut withdrawls {
            for (other_resource, deposit_stats) in &mut deposits {
                if let None = other_resource {
                    let consume = withdrawl_stats.active.min(deposit_stats.inactive);

                    withdrawl_stats.active -= consume;
                    deposit_stats.inactive -= consume;

                    add_resource(*resource, consume);
                }
            }
        }

        total_pickup.retain(|_, amount| *amount > 0);

        total_pickup
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
    }

    fn visualize(&mut self, data: &dyn TransferRequestSystemData, ui: &mut UISystem, visualizer: &mut Visualizer) {
        if crate::features::features().transfer.visualize.demand() {
            let room_names = self.rooms.get_all_rooms();

            for room_name in room_names.iter() {
                let room = self.get_room(data, *room_name, TransferTypeFlags::all());
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
pub struct TransferQueueUpdateSystemData<'a> {
    transfer_queue: Write<'a, TransferQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

pub struct TransferQueueUpdateSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for TransferQueueUpdateSystem {
    type SystemData = TransferQueueUpdateSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                let transfer_queue_data = TransferQueueGeneratorData {
                    cause: "Transfer System",
                    room_data: &data.room_data,
                };

                data.transfer_queue.visualize(&transfer_queue_data, ui, visualizer);
            }
        }

        data.transfer_queue.clear();
    }
}
