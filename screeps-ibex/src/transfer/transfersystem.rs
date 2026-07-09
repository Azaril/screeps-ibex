use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::visualize::*;
use itertools::*;
use log::*;
use screeps::*;
use serde::*;
use specs::prelude::{Entities, Entity, LazyUpdate, Read, ReadStorage, ResourceId, System, SystemData, World, Write, WriteStorage};
use screeps::Position;
use std::collections::hash_map::*;
use std::collections::HashMap;
use std::collections::HashSet;
// The K2 selection kernels (ADR 0040 M3 / ADR 0007 item 1) — the snapshot module this
// adapter builds views for.
use screeps_econ_decision::snapshot as econ;
// ADR 0040 M5a — the e/t sink market: the numeric-bid vocabulary + bid functions (the live
// tickets ride `bid: u32`; the live market-selection kernel `market::market_pass` is consumed
// by the `market_adapter` module).
use screeps_econ_decision::sink_economics::{bid_to_tier, bid_label, tier_to_bid, MarketConsts};

// The transfer priority/type vocabulary (`TransferPriority`/`TransferType` + flags + the
// ACTIVE/ALL priority lists) lives in `screeps_econ_decision::priority` since ADR 0040 M3 —
// one implementation, consumed by this queue AND the economy sim. Re-exported here so every
// bot call site keeps its `transfer::transfersystem::*` path; the serde shapes are unchanged
// (the enums ride in serialized tickets/HaulState — zero WFV).
#[allow(unused_imports)] // ACTIVE_TRANSFER_PRIORITIES is re-exported API (pre-move pub const)
pub use screeps_econ_decision::priority::{
    TransferPriority, TransferPriorityFlags, TransferType, TransferTypeFlags, ACTIVE_TRANSFER_PRIORITIES, ALL_TRANSFER_PRIORITIES,
};

/// ADR 0040 M5a market observability (§D8 #5): the per-room bid readout the
/// live transfer-market pass publishes each tick — the opportunity floor + the
/// top unmet deposit bids. Ephemeral resource (cleared/rebuilt each tick),
/// exported per-room by `metrics.rs` into the seg-57 stats block, and echoed to
/// one grep-able console line per room. "An emergent system without a readout
/// is an operations regression" (§D4).
#[derive(Default)]
pub struct MarketBidSummary {
    /// Per owned-room-name summary.
    pub rooms: HashMap<RoomName, RoomBidSummary>,
}

/// One room's market bid readout for this tick.
#[derive(Debug, Default, Clone)]
pub struct RoomBidSummary {
    /// The opportunity floor (highest materially-unmet deposit bid, milli-e/t).
    pub opportunity_floor: u32,
    /// The top unmet deposit bids (milli-e/t), descending, up to three.
    pub top_unmet_bids: Vec<u32>,
}

impl MarketBidSummary {
    /// Clear all counters (called at the start of each tick).
    pub fn clear(&mut self) {
        self.rooms.clear();
    }

    /// Publish (or overwrite) a room's summary and emit the one-line, grep-able
    /// console readout (`[market] <room> floor=<f> unmet=[..]`).
    pub fn publish(&mut self, room: RoomName, floor: u32, top_unmet_bids: Vec<u32>) {
        log::info!(
            "[market] {room} floor={floor} unmet={:?}",
            top_unmet_bids
        );
        self.rooms.insert(room, RoomBidSummary { opportunity_floor: floor, top_unmet_bids });
    }
}

/// System that clears the market bid summary at the start of each tick.
#[derive(Default)]
pub struct MarketBidSummaryClearSystem;

impl<'a> System<'a> for MarketBidSummaryClearSystem {
    type SystemData = Write<'a, MarketBidSummary>;

    fn run(&mut self, mut summary: Self::SystemData) {
        summary.clear();
    }
}

/// Compute a transfer "value" (resources per unit of distance/cost) with a
/// guarded divisor so a degenerate input (zero length/cost) cannot produce
/// NaN or infinity feeding the priority comparators (IBEX-046). The
/// comparators keep their `unwrap_or(Equal)` runtime backstop; the
/// `debug_assert!` here is the tripwire at the value source.
fn finite_transfer_value(resources: u32, divisor: f32) -> f32 {
    let value = (resources as f32) / divisor.max(1.0);
    debug_assert!(value.is_finite(), "transfer value not finite: {value}");
    value
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
    /// ADR 0044 stage-1 source-floor classifier: this target's outside option as a HAUL PICKUP
    /// source, milli. A LOSSLESS store (storage/terminal — declining an arc truly banks the energy)
    /// bids the par outside option, so only an above-par sink pulls from it (base-stock behavior).
    /// A SATURATING or DECAYING buffer (a source container filling from harvest, a link relay,
    /// dropped/ruin/tombstone energy — declining strands or loses it) bids ~0, so it is freely
    /// drained. Feeds `MarketPickup::source_floor_milli` for the reduced-cost reject test.
    pub fn source_floor_milli(&self) -> u32 {
        match self {
            TransferTarget::Container(_)
            | TransferTarget::Link(_)
            | TransferTarget::Ruin(_)
            | TransferTarget::Tombstone(_)
            | TransferTarget::Resource(_) => 0,
            _ => screeps_econ_decision::sink_economics::STORAGE_BID,
        }
    }

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

    /// The stored local [`Position`] (no JS object — DTO-safe; the K2 snapshot adapter and
    /// host tests use this where `pos()`'s `RoomPosition` wrapper is not constructible).
    pub fn local_pos(&self) -> Position {
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
            TransferTarget::Lab(id) => id.pos(),
            TransferTarget::Factory(id) => id.pos(),
            TransferTarget::Nuker(id) => id.pos(),
            TransferTarget::PowerSpawn(id) => id.pos(),
        }
    }

    /// One-shot (per VM session) warning for the invalid nuker-withdraw
    /// pairing. Logging only -- not used for any control flow.
    fn warn_once_nuker_withdraw() {
        thread_local! {
            static WARNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }

        WARNED.with(|warned| {
            if !warned.get() {
                warned.set(true);
                warn!("Attempted to withdraw resources from a nuker -- invalid TransferTarget pairing (returning InvalidArgs)");
            }
        });
    }

    fn withdraw_resource_amount_from_id<T>(
        target: &RemoteObjectId<T>,
        creep: &Creep,
        resource: ResourceType,
        amount: u32,
    ) -> Result<(), ErrorCode>
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
            TransferTarget::Nuker(_id) => {
                // A nuker cannot be a withdraw source (see the salvage-loot
                // structure registration, missions/salvage.rs). Return an
                // error instead of
                // panicking -- under panic="abort" a panic here aborts the
                // whole tick and skips serialize_world (IBEX-010).
                Self::warn_once_nuker_withdraw();
                Err(ErrorCode::InvalidArgs)
            }
            TransferTarget::PowerSpawn(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
        }
    }

    fn creep_transfer_resource_amount_to_id<T>(
        target: &RemoteObjectId<T>,
        creep: &Creep,
        resource: ResourceType,
        amount: u32,
    ) -> Result<(), ErrorCode>
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

    fn link_transfer_energy_amount_to_id(
        target: &RemoteObjectId<StructureLink>,
        link: &StructureLink,
        amount: u32,
    ) -> Result<(), ErrorCode> {
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
        matches!(
            target,
            TransferTarget::Container(_) | TransferTarget::Storage(_) | TransferTarget::Terminal(_)
        )
    }

    pub fn link(target: &TransferTarget) -> bool {
        matches!(target, TransferTarget::Link(_))
    }

    pub fn terminal(target: &TransferTarget) -> bool {
        matches!(target, TransferTarget::Terminal(_))
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

// ADR 0040 M5a — the numeric-bid lane: the live queue keys / requests / ticket entries carry a
// quantized `bid: u32` (milli-e/t; `sink_economics::BID_SCALE` = 1.000 = par) INSTEAD of the
// 4-tier `TransferPriority` enum (the rover w-lane precedent). `bid_to_tier` projects a bid back
// onto the tier bands for the NON-MARKET lanes (links/terminal/labs) that still consume the
// tier-interleave snapshot kernel (`screeps_econ_decision::snapshot`, kept tier-based for the
// SIM's tournament arms). The market HAUL lane runs the numeric market pass directly.
#[derive(Eq, PartialEq, Hash, Clone, Copy)]
pub struct TransferWithdrawlKey {
    resource: ResourceType,
    bid: u32,
    allowed_type: TransferType,
}

impl TransferWithdrawlKey {
    pub fn matches(&self, resource: ResourceType, allowed_priorities: TransferPriorityFlags, allowed_types: TransferTypeFlags) -> bool {
        self.resource == resource
            && allowed_priorities.intersects(bid_to_tier(self.bid).into())
            && allowed_types.intersects(self.allowed_type.into())
    }
}

#[derive(Eq, PartialEq, Hash, Clone, Copy)]
pub struct TransferDepositKey {
    resource: Option<ResourceType>,
    bid: u32,
    allowed_type: TransferType,
}

impl TransferDepositKey {
    pub fn matches(
        &self,
        resource: Option<ResourceType>,
        allowed_priorities: TransferPriorityFlags,
        allowed_types: TransferTypeFlags,
    ) -> bool {
        self.resource == resource
            && allowed_priorities.intersects(bid_to_tier(self.bid).into())
            && allowed_types.intersects(self.allowed_type.into())
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

    pub fn request_withdraw(&mut self, key: TransferWithdrawlKey, amount: u32) {
        let current = self.withdrawls.entry(key).or_insert(0);

        *current += amount;
    }

    pub fn request_deposit(&mut self, key: TransferDepositKey, amount: u32) {
        let current = self.deposits.entry(key).or_insert(0);

        *current += amount;
    }

    pub fn register_pickup(&mut self, withdrawls: &HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>>) {
        for (resource, resource_entries) in withdrawls {
            for resource_entry in resource_entries {
                let key = TransferWithdrawlKey {
                    resource: *resource,
                    bid: resource_entry.bid,
                    allowed_type: resource_entry.transfer_type,
                };

                let current = self.pending_withdrawls.entry(key).or_insert(0);

                *current += resource_entry.amount;
            }
        }
    }

    pub fn register_delivery(&mut self, deposits: &HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>>) {
        for resource_entries in deposits.values() {
            for resource_entry in resource_entries {
                let key = TransferDepositKey {
                    resource: resource_entry.target_resource,
                    bid: resource_entry.bid,
                    allowed_type: resource_entry.transfer_type,
                };

                let current = self.pending_deposits.entry(key).or_insert(0);

                *current += resource_entry.amount;
            }
        }
    }


    pub fn visualize(&self, visualizer: &mut RoomVisualizer, pos: RoomPosition) {
        let withdraw_text = self
            .withdrawls
            .iter()
            .map(|(key, amount)| format!("{:?} {} {:?} {:?}", key.resource, bid_label(key.bid), key.allowed_type, amount));

        let pending_withdraw_text = self
            .pending_withdrawls
            .iter()
            .map(|(key, amount)| format!("{:?} {} {:?} {:?}", key.resource, bid_label(key.bid), key.allowed_type, amount));

        let deposit_text = self
            .deposits
            .iter()
            .map(|(key, amount)| format!("{:?} {} {:?} {:?}", key.resource, bid_label(key.bid), key.allowed_type, amount));

        let pending_deposit_text = self
            .pending_deposits
            .iter()
            .map(|(key, amount)| format!("{:?} {} {:?} {:?}", key.resource, bid_label(key.bid), key.allowed_type, amount));

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
    bid: u32,
    amount: u32,
    allowed_type: TransferType,
}

impl TransferWithdrawRequest {
    /// Construct a withdraw request from a numeric `bid` (milli-e/t; ADR 0040 M5a). Registration
    /// sites that still speak tiers convert via [`tier_to_bid`] at the call.
    pub fn new(
        target: TransferTarget,
        resource: ResourceType,
        bid: u32,
        amount: u32,
        allowed_type: TransferType,
    ) -> TransferWithdrawRequest {
        TransferWithdrawRequest {
            target,
            resource,
            bid,
            amount,
            allowed_type,
        }
    }

    /// Construct from a legacy tier priority (the non-market registration sites keep their tier
    /// intent readable; the bid rides the numeric lane).
    pub fn new_tier(
        target: TransferTarget,
        resource: ResourceType,
        priority: TransferPriority,
        amount: u32,
        allowed_type: TransferType,
    ) -> TransferWithdrawRequest {
        Self::new(target, resource, tier_to_bid(priority), amount, allowed_type)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferWithdrawlTicketResourceEntry {
    amount: u32,
    transfer_type: TransferType,
    /// The quantized e/t bid (milli; ADR 0040 M5a — replaces the `TransferPriority` tier). Rides
    /// inside the serialized `HaulState` (WFV 26→27).
    bid: u32,
}

impl TransferWithdrawlTicketResourceEntry {
    pub fn amount(&self) -> u32 {
        self.amount
    }

    pub fn transfer_type(&self) -> TransferType {
        self.transfer_type
    }

    pub fn bid(&self) -> u32 {
        self.bid
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
                        if let Some(withdrawl_resource_entry) = existing
                            .iter_mut()
                            .find(|oe| oe.bid == entry.bid && oe.transfer_type == entry.transfer_type)
                        {
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
    bid: u32,
    amount: u32,
    allowed_type: TransferType,
}

impl TransferDepositRequest {
    /// Construct a deposit request from a numeric `bid` (milli-e/t; ADR 0040 M5a).
    pub fn new(
        target: TransferTarget,
        resource: Option<ResourceType>,
        bid: u32,
        amount: u32,
        allowed_type: TransferType,
    ) -> TransferDepositRequest {
        TransferDepositRequest {
            target,
            resource,
            bid,
            amount,
            allowed_type,
        }
    }

    /// Construct from a legacy tier priority (the non-market registration sites).
    pub fn new_tier(
        target: TransferTarget,
        resource: Option<ResourceType>,
        priority: TransferPriority,
        amount: u32,
        allowed_type: TransferType,
    ) -> TransferDepositRequest {
        Self::new(target, resource, tier_to_bid(priority), amount, allowed_type)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferDepositTicketResourceEntry {
    target_resource: Option<ResourceType>,
    amount: u32,
    transfer_type: TransferType,
    /// The quantized e/t bid (milli; ADR 0040 M5a — replaces the `TransferPriority` tier).
    bid: u32,
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

    pub fn bid(&self) -> u32 {
        self.bid
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
                        if let Some(deposit_resource_entry) = existing.iter_mut().find(|oe| {
                            oe.target_resource == entry.target_resource
                                && oe.bid == entry.bid
                                && oe.transfer_type == entry.transfer_type
                        }) {
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

            amount - remaining_amount
        } else {
            0
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
        self.pending_amount
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

// ─── Transfer stats snapshot (for visualization, not serialized) ─────────────

/// Per-resource supply/demand stats for one room.
#[derive(Debug, Clone, Default)]
pub struct TransferResourceStats {
    pub supply: u32,
    pub supply_pending: u32,
    /// Supply by priority: [High, Medium, Low, None].
    pub supply_by_priority: [u32; 4],
    pub demand: u32,
    pub demand_pending: u32,
    /// Demand by priority: [High, Medium, Low, None].
    pub demand_by_priority: [u32; 4],
}

/// Per-room snapshot of transfer queue state for one tick.
#[derive(Debug, Clone, Default)]
pub struct TransferRoomSnapshot {
    /// Per-resource stats (keyed by ResourceType).
    pub resources: HashMap<screeps::ResourceType, TransferResourceStats>,
    /// Demand from deposits with resource=None ("accept any").
    pub generic_demand: u32,
    pub generic_demand_pending: u32,
    pub generic_demand_by_priority: [u32; 4],
}

/// All-rooms snapshot of transfer queue state for one tick. World resource (ephemeral).
#[derive(Debug, Clone, Default)]
pub struct TransferStatsSnapshot {
    pub rooms: HashMap<RoomName, TransferRoomSnapshot>,
}

// `TransferCapacity` lives in `screeps_econ_decision::snapshot` since ADR 0040 M3 (it is the
// K2 selection kernels' capacity type) — re-exported so every bot call site keeps its path.
pub use screeps_econ_decision::snapshot::TransferCapacity;

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
            .or_default()
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

    /// Flush all generators for all rooms so that every room's transfer requests are populated.
    /// Use when visualization is on so the snapshot includes all requests; when viz is off, lazy evaluation is unchanged.
    pub fn flush_all_generators(&mut self, data: &dyn TransferRequestSystemData) {
        let room_names: Vec<RoomName> = self.get_all_rooms().into_iter().collect();
        for room in room_names {
            self.flush_generators(data, room, TransferTypeFlags::all());
        }
    }
}

impl TransferRequestSystem for LazyTransferQueueRooms {
    fn request_withdraw(&mut self, withdraw_request: TransferWithdrawRequest) {
        let room = self.get_room_no_flush(withdraw_request.target.local_pos().room_name());
        room.stats.total_withdrawl += withdraw_request.amount;

        // The tier-presence gates read the bid's projected band (ADR 0040 M5a).
        let priority_flag = bid_to_tier(withdraw_request.bid).into();
        room.stats.withdrawl_priorities |= priority_flag;

        if TransferPriorityFlags::ACTIVE.intersects(priority_flag) {
            room.stats.total_active_withdrawl += withdraw_request.amount;
        }

        let key = TransferWithdrawlKey {
            resource: withdraw_request.resource,
            bid: withdraw_request.bid,
            allowed_type: withdraw_request.allowed_type,
        };

        let resource_stats = room.get_mut_withdrawl_stats(key);
        resource_stats.amount += withdraw_request.amount;

        let node = room.get_node(&withdraw_request.target);
        node.request_withdraw(key, withdraw_request.amount);
    }

    fn request_deposit(&mut self, deposit_request: TransferDepositRequest) {
        let room = self.get_room_no_flush(deposit_request.target.local_pos().room_name());
        room.stats.total_deposit += deposit_request.amount;

        let priority_flag = bid_to_tier(deposit_request.bid).into();
        room.stats.deposit_priorities |= priority_flag;

        if TransferPriorityFlags::ACTIVE.intersects(priority_flag) {
            room.stats.total_active_deposit += deposit_request.amount;
        }

        let key = TransferDepositKey {
            resource: deposit_request.resource,
            bid: deposit_request.bid,
            allowed_type: deposit_request.allowed_type,
        };

        let resource_stats = room.get_mut_deposit_stats(key);
        resource_stats.amount += deposit_request.amount;

        let node = room.get_node(&deposit_request.target);
        node.request_deposit(key, deposit_request.amount);
    }

    fn register_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        let room = self.get_room_no_flush(ticket.target.local_pos().room_name());

        for (resource, entries) in ticket.resources() {
            for entry in entries {
                let key = TransferWithdrawlKey {
                    resource: *resource,
                    bid: entry.bid,
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
        let room = self.get_room_no_flush(ticket.target.local_pos().room_name());

        for entries in ticket.resources().values() {
            for entry in entries {
                let key = TransferDepositKey {
                    resource: entry.target_resource,
                    bid: entry.bid,
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

// ─── The K2 econ view (ADR 0040 M3 / ADR 0007 Q5 item 1) ─────────────────────
//
// The selection POLICY lives in `screeps_econ_decision::snapshot` (the pure kernels over the
// immutable per-tick `TransferSnapshot` + the adapter-owned `SnapshotBookings`). This adapter
// owns the live plumbing: building the snapshot from the materialized queue (once per tick at
// the top of the hauling pass via [`TransferQueue::build_econ_snapshot`], or an on-the-fly
// per-query view for mission-phase consumers — links/terminal — so the kernel stays the ONE
// implementation), the `NodeId ↔ TransferTarget` table, and mirroring ticket registrations
// into the kernel bookings alongside the queue's own `pending_*`/stats writes.

impl TransferWithdrawlKey {
    // The tier-keyed snapshot kernel (`snapshot.rs`, kept tier-based for the SIM) consumes the
    // bid's projected band (ADR 0040 M5a). The market HAUL lane uses the numeric bid directly.
    fn to_econ(self) -> econ::WithdrawKey {
        econ::WithdrawKey {
            resource: self.resource,
            priority: bid_to_tier(self.bid),
            allowed_type: self.allowed_type,
        }
    }
}

impl TransferDepositKey {
    fn to_econ(self) -> econ::DepositKey {
        econ::DepositKey {
            resource: self.resource,
            priority: bid_to_tier(self.bid),
            allowed_type: self.allowed_type,
        }
    }
}

/// Deterministic node candidate order within a room: (x, y, target kind). Two same-kind
/// structures never share a tile; same-tile dropped-resource piles keep their map order (an
/// exact-tie class the live HashMap order also left arbitrary).
fn target_sort_key(target: &TransferTarget) -> (u8, u8, u8) {
    let pos = target.local_pos();
    let kind = match target {
        TransferTarget::Container(_) => 0u8,
        TransferTarget::Spawn(_) => 1,
        TransferTarget::Extension(_) => 2,
        TransferTarget::Storage(_) => 3,
        TransferTarget::Tower(_) => 4,
        TransferTarget::Link(_) => 5,
        TransferTarget::Ruin(_) => 6,
        TransferTarget::Tombstone(_) => 7,
        TransferTarget::Resource(_) => 8,
        TransferTarget::Terminal(_) => 9,
        TransferTarget::Lab(_) => 10,
        TransferTarget::Factory(_) => 11,
        TransferTarget::Nuker(_) => 12,
        TransferTarget::PowerSpawn(_) => 13,
    };
    (pos.x().u8(), pos.y().u8(), kind)
}

/// Deterministic key order within a node (live: HashMap iteration).
fn withdraw_key_sort(key: &econ::WithdrawKey) -> (u32, u8, u8) {
    (key.resource as u32, key.priority as u8, key.allowed_type as u8)
}

fn deposit_key_sort(key: &econ::DepositKey) -> (u32, u8, u8) {
    let resource = match key.resource {
        None => 0u32,
        Some(r) => 1 + r as u32,
    };
    (resource, key.priority as u8, key.allowed_type as u8)
}

/// Build a single-entry energy withdraw ticket (ADR 0040 M5a market selection): the pickup leg a
/// market `PickupDeliver` assigns, keyed by the source node's own numeric bid.
fn build_energy_withdraw_ticket(target: TransferTarget, amount: u32, bid: u32, transfer_type: TransferType) -> TransferWithdrawTicket {
    let mut resources: HashMap<ResourceType, Vec<TransferWithdrawlTicketResourceEntry>> = HashMap::new();
    resources.insert(
        ResourceType::Energy,
        vec![TransferWithdrawlTicketResourceEntry {
            amount,
            transfer_type,
            bid,
        }],
    );
    TransferWithdrawTicket { target, resources }
}

/// Build a single-entry energy deposit ticket (ADR 0040 M5a market selection): the delivery leg a
/// market `Deliver`/`PickupDeliver` assigns, keyed by the sink's own numeric bid.
fn build_energy_deposit_ticket(target: TransferTarget, amount: u32, bid: u32, transfer_type: TransferType) -> TransferDepositTicket {
    let mut resources: HashMap<ResourceType, Vec<TransferDepositTicketResourceEntry>> = HashMap::new();
    resources.insert(
        ResourceType::Energy,
        vec![TransferDepositTicketResourceEntry {
            target_resource: Some(ResourceType::Energy),
            amount,
            transfer_type,
            bid,
        }],
    );
    TransferDepositTicket { target, resources }
}

/// The live-side K2 view: the kernel snapshot + bookings + the node↔target table.
#[derive(Default)]
pub struct EconView {
    snapshot: econ::TransferSnapshot,
    bookings: econ::SnapshotBookings,
    targets: Vec<TransferTarget>,
    node_ids: HashMap<TransferTarget, econ::NodeId>,
}

impl EconView {
    fn target(&self, node: econ::NodeId) -> TransferTarget {
        self.targets[node.0 as usize]
    }

    fn node_id(&self, target: &TransferTarget) -> Option<econ::NodeId> {
        self.node_ids.get(target).copied()
    }

    fn withdraw_ticket(&self, dto: econ::WithdrawTicketDto) -> TransferWithdrawTicket {
        TransferWithdrawTicket {
            target: self.target(dto.node),
            resources: dto
                .resources
                .into_iter()
                .map(|(resource, entries)| {
                    (
                        resource,
                        entries
                            .into_iter()
                            .map(|e| TransferWithdrawlTicketResourceEntry {
                                amount: e.amount,
                                transfer_type: e.transfer_type,
                                // The tier-keyed snapshot kernel emits a tier; carry it onto the
                                // numeric ticket lane (ADR 0040 M5a).
                                bid: tier_to_bid(e.priority),
                            })
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    fn deposit_ticket(&self, dto: econ::DepositTicketDto) -> TransferDepositTicket {
        TransferDepositTicket {
            target: self.target(dto.node),
            resources: dto
                .resources
                .into_iter()
                .map(|(resource, entries)| {
                    (
                        resource,
                        entries
                            .into_iter()
                            .map(|e| TransferDepositTicketResourceEntry {
                                target_resource: e.target_resource,
                                amount: e.amount,
                                transfer_type: e.transfer_type,
                                bid: tier_to_bid(e.priority),
                            })
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    /// Mirror a pickup registration into the kernel bookings (targets absent from the
    /// snapshot — e.g. an in-flight ticket for a store that registered no request this tick —
    /// are skipped: they have no availability to reserve).
    fn book_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        if let Some(node) = self.node_id(&ticket.target) {
            for (resource, entries) in ticket.resources() {
                for entry in entries {
                    self.bookings.book_withdraw(
                        node,
                        econ::WithdrawKey {
                            resource: *resource,
                            priority: bid_to_tier(entry.bid),
                            allowed_type: entry.transfer_type,
                        },
                        entry.amount,
                    );
                }
            }
        }
    }

    /// Mirror a delivery registration into the kernel bookings.
    fn book_delivery(&mut self, ticket: &TransferDepositTicket) {
        if let Some(node) = self.node_id(&ticket.target) {
            for entries in ticket.resources().values() {
                for entry in entries {
                    self.bookings.book_deposit(
                        node,
                        econ::DepositKey {
                            resource: entry.target_resource,
                            priority: bid_to_tier(entry.bid),
                            allowed_type: entry.transfer_type,
                        },
                        entry.amount,
                    );
                }
            }
        }
    }
}

impl LazyTransferQueueRooms {
    /// Build a view over the given (already-name-sorted, deduped) rooms from the materialized
    /// queue: nodes in deterministic candidate order, requested amounts as the snapshot,
    /// current `pending_*` as the initial bookings.
    fn build_econ_view_from_rooms(&self, names: &[RoomName]) -> EconView {
        let mut view = EconView::default();
        for room_name in names {
            let Some(room) = self.rooms.get(room_name) else {
                continue;
            };
            let mut entries: Vec<(&TransferTarget, &TransferNode)> = room.nodes.iter().collect();
            entries.sort_by_key(|(target, _)| target_sort_key(target));
            for (target, node) in entries {
                let mut withdrawls: Vec<(econ::WithdrawKey, u32)> =
                    node.withdrawls.iter().map(|(key, amount)| (key.to_econ(), *amount)).collect();
                withdrawls.sort_by_key(|(key, _)| withdraw_key_sort(key));
                let mut deposits: Vec<(econ::DepositKey, u32)> =
                    node.deposits.iter().map(|(key, amount)| (key.to_econ(), *amount)).collect();
                deposits.sort_by_key(|(key, _)| deposit_key_sort(key));

                let id = view.snapshot.add_node(*room_name, target.local_pos(), withdrawls, deposits);
                view.targets.push(*target);
                view.node_ids.insert(*target, id);

                for (key, amount) in &node.pending_withdrawls {
                    if *amount > 0 {
                        view.bookings.book_withdraw(id, key.to_econ(), *amount);
                    }
                }
                for (key, amount) in &node.pending_deposits {
                    if *amount > 0 {
                        view.bookings.book_deposit(id, key.to_econ(), *amount);
                    }
                }
            }
        }
        view
    }

    /// The per-tick snapshot (0007 item 1): flush EVERY generator once, view every room.
    fn build_econ_view_all(&mut self, data: &dyn TransferRequestSystemData) -> EconView {
        self.flush_all_generators(data);
        let mut names: Vec<RoomName> = self.rooms.keys().cloned().collect();
        names.sort_unstable();
        self.build_econ_view_from_rooms(&names)
    }

    /// An on-the-fly view for mission-phase queries (pre-snapshot): lazy-flush exactly the
    /// queried rooms/types (the pre-M3 `try_get_room` contract), view those rooms.
    fn build_econ_view_for(
        &mut self,
        data: &dyn TransferRequestSystemData,
        rooms: &[RoomName],
        transfer_types: TransferTypeFlags,
    ) -> EconView {
        let mut names: Vec<RoomName> = rooms.to_vec();
        names.sort_unstable();
        names.dedup();
        for room in &names {
            self.flush_generators(data, *room, transfer_types);
        }
        self.build_econ_view_from_rooms(&names)
    }
}

#[derive(Default)]
pub struct TransferQueue {
    rooms: LazyTransferQueueRooms,
    /// The per-tick K2 view (`Some` from `build_econ_snapshot` at the top of the hauling pass
    /// until `clear`); mission-phase queries before the build use per-query views.
    econ: Option<EconView>,
}

impl TransferRequestSystem for TransferQueue {
    fn request_withdraw(&mut self, withdraw_request: TransferWithdrawRequest) {
        self.rooms.request_withdraw(withdraw_request)
    }

    fn request_deposit(&mut self, deposit_request: TransferDepositRequest) {
        self.rooms.request_deposit(deposit_request)
    }

    fn register_pickup(&mut self, ticket: &TransferWithdrawTicket) {
        self.rooms.register_pickup(ticket);
        // Mirror into the per-tick view's bookings (the kernel's reservation half — the queue's
        // pending_* maps stay the source the NEXT snapshot initializes from).
        if let Some(view) = self.econ.as_mut() {
            view.book_pickup(ticket);
        }
    }

    fn register_delivery(&mut self, ticket: &TransferDepositTicket) {
        self.rooms.register_delivery(ticket);
        if let Some(view) = self.econ.as_mut() {
            view.book_delivery(ticket);
        }
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

    /// Flush all room generators so every room's requests are populated (e.g. for accurate visualization snapshot).
    pub fn flush_all_generators(&mut self, data: &dyn TransferRequestSystemData) {
        self.rooms.flush_all_generators(data);
    }

    pub fn try_get_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        room: RoomName,
        transfer_types: TransferTypeFlags,
    ) -> Option<&TransferQueueRoomData> {
        self.rooms.try_get_room(data, room, transfer_types)
    }

    /// Build the per-tick K2 snapshot (ADR 0007 item 1): flush every generator ONCE (generation
    /// provably paid once) and freeze the view. Called at the top of the hauling pass
    /// (`RunJobSystem`); cleared with the queue each tick.
    pub fn build_econ_snapshot(&mut self, data: &dyn TransferRequestSystemData) {
        self.econ = Some(self.rooms.build_econ_view_all(data));
    }

    /// ADR 0040 M5a — publish the LIVE market readout (§D8 #5): for every materialized room,
    /// compute the opportunity floor (the highest materially-unmet DEPOSIT bid, read straight
    /// off the numeric-bid deposit keys — this IS the market's per-room floor the selection
    /// admits against) + the top-3 unmet bids, and emit the grep-able `[market]` console line via
    /// [`MarketBidSummary`]. Called right after [`Self::build_econ_snapshot`] (the demand is
    /// fully materialized). The floor is the same quantity the sim's `begin_tick` computes; the
    /// live selection (the numeric-bid tier-interleave over this demand) and the repair/Use-lane
    /// admission consume it.
    pub fn publish_market_floor(&self, summary: &mut MarketBidSummary) {
        let consts = MarketConsts::default();
        for (room_name, room) in &self.rooms.rooms {
            // Aggregate unmet deposit demand per numeric bid across the room's nodes
            // (`requested − pending`, floored at 0 — the live availability).
            let mut unmet_by_bid: HashMap<u32, u32> = HashMap::new();
            for node in room.nodes.values() {
                for key in node.deposits.keys() {
                    let available = node.get_available_deposit(key);
                    if available > 0 {
                        *unmet_by_bid.entry(key.bid).or_insert(0) += available;
                    }
                }
            }
            if unmet_by_bid.is_empty() {
                continue;
            }
            let floor = screeps_econ_decision::sink_economics::opportunity_floor(
                &consts,
                unmet_by_bid.iter().map(|(&b, &a)| (b, a)),
            );
            let mut top: Vec<u32> = unmet_by_bid
                .iter()
                .filter(|(_, &a)| a >= consts.floor_material_min_e)
                .map(|(&b, _)| b)
                .collect();
            top.sort_unstable_by(|a, b| b.cmp(a));
            top.dedup();
            top.truncate(3);
            summary.publish(*room_name, floor, top);
        }
    }

    /// Run `f` over the per-tick snapshot when it exists (the hauling pass), else over an
    /// on-the-fly view of exactly the queried rooms/types (mission-phase consumers — the
    /// pre-M3 lazy `try_get_room` contract).
    fn with_econ_view<R>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        rooms: &[RoomName],
        transfer_types: TransferTypeFlags,
        f: impl FnOnce(&EconView) -> R,
    ) -> R {
        if let Some(view) = self.econ.as_ref() {
            f(view)
        } else {
            let view = self.rooms.build_econ_view_for(data, rooms, transfer_types);
            f(&view)
        }
    }

    /// The nearest-wins pickup selection (K2 kernel `select_nearest_pickup` — the pre-M3
    /// `select_pickups` + anchor filter + `find_nearest_linear_by` composition).
    #[allow(clippy::too_many_arguments)]
    pub fn select_nearest_pickup(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        transfer_types: TransferTypeFlags,
        desired_resource: ResourceType,
        free_capacity: u32,
        creep_pos: Position,
        range_anchor: Option<(Position, u32)>,
    ) -> Option<TransferWithdrawTicket> {
        self.with_econ_view(data, pickup_rooms, transfer_types, |view| {
            econ::select_nearest_pickup(
                &view.snapshot,
                &view.bookings,
                pickup_rooms,
                allowed_priorities,
                transfer_types,
                desired_resource,
                free_capacity,
                creep_pos,
                range_anchor,
            )
            .map(|dto| view.withdraw_ticket(dto))
        })
    }

    /// The nearest-wins delivery selection for carried resources (K2 kernel
    /// `select_nearest_delivery` — the pre-M3 `select_deliveries` + `find_nearest_linear_by`
    /// composition).
    #[allow(clippy::too_many_arguments)]
    pub fn select_nearest_delivery<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &[(ResourceType, u32)],
        available_capacity: TransferCapacity,
        creep_pos: Position,
        target_filter: TF,
    ) -> Option<TransferDepositTicket>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        self.with_econ_view(data, delivery_rooms, delivery_types, |view| {
            econ::select_nearest_delivery(
                &view.snapshot,
                &view.bookings,
                delivery_rooms,
                allowed_priorities,
                delivery_types,
                available_resources,
                available_capacity,
                creep_pos,
                |node| target_filter(&view.target(node)),
            )
            .map(|dto| view.deposit_ticket(dto))
        })
    }

    /// Terminal-send shape: the single best resource each node of a room can absorb (K2 kernel
    /// `node_select_single_delivery` per node).
    pub fn select_single_delivery_for_room(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_room: RoomName,
        allowed_priorities: TransferPriorityFlags,
        delivery_types: TransferTypeFlags,
        available_resources: &HashMap<ResourceType, u32>,
        available_capacity: TransferCapacity,
    ) -> Vec<TransferDepositTicket> {
        let mut resources: Vec<(ResourceType, u32)> = available_resources.iter().map(|(r, a)| (*r, *a)).collect();
        resources.sort_by_key(|(r, _)| *r as u32);

        self.with_econ_view(data, &[delivery_room], delivery_types, |view| {
            let mut tickets = Vec::new();
            for &node in view.snapshot.room_nodes(&delivery_room) {
                if let Some((delivery_resource, delivery_entries)) = econ::node_select_single_delivery(
                    &view.snapshot,
                    &view.bookings,
                    node,
                    allowed_priorities,
                    delivery_types,
                    &resources,
                    available_capacity,
                ) {
                    tickets.push(view.deposit_ticket(econ::DepositTicketDto {
                        node,
                        pos: view.snapshot.node(node).pos,
                        resources: vec![(delivery_resource, delivery_entries)],
                    }));
                }
            }
            tickets
        })
    }

    /// Room-stats withdraw totals for one transfer type (labs' compound sourcing read). Live
    /// stats semantics preserved: `unfufilled_amount()` = requested + registered (module docs
    /// on the stats inflation).
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

    /// The best (pickup, delivery) pair over the tier-interleave (K2 kernel
    /// `select_pickup_and_delivery` — the 0040 §D5 seam entry).
    #[allow(clippy::too_many_arguments)]
    pub fn select_pickup_and_delivery<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
        allowed_priorities: TransferPriorityFlags,
        transfer_type: TransferType,
        current_position: Position,
        available_capacity: TransferCapacity,
        target_filter: TF,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    where
        TF: Fn(&TransferTarget) -> bool + Copy,
    {
        let rooms: Vec<RoomName> = pickup_rooms.iter().chain(delivery_rooms.iter()).cloned().collect();
        self.with_econ_view(data, &rooms, transfer_type.into(), |view| {
            let creep = screeps_econ_decision::CreepEconDto {
                id: 0,
                pos: current_position,
                free_capacity: available_capacity.clamp(u32::MAX),
                store: Vec::new(),
            };
            econ::select_pickup_and_delivery(
                &view.snapshot,
                &view.bookings,
                &creep,
                pickup_rooms,
                delivery_rooms,
                allowed_priorities,
                transfer_type,
                available_capacity,
                |node| target_filter(&view.target(node)),
            )
            .map(|(pickup, delivery)| (view.withdraw_ticket(pickup), view.deposit_ticket(delivery)))
        })
    }

    /// **ADR 0040 M5a — the LIVE bid-native market selection** (the wiring the M5a-core slice
    /// left undone): run the SHARED market-selection kernel
    /// ([`screeps_econ_decision::market::market_pass`], the same one the sim's MARKET tournament
    /// arm delegates to) over this creep, ranking candidate (pickup, delivery) pairs by RAW
    /// bid-density `v = bid·amount/service` — NOT the tier-interleave `select_pickup_and_delivery`
    /// which projects bids back to 4 tiers via `bid_to_tier` and then scores nearest-wins,
    /// throwing away bid resolution before selecting.
    ///
    /// The pass is run with a SINGLE carrier (this creep), so it honors the FSM's existing
    /// per-mission room scoping + `target_filter` for free (the market kernel is single-room /
    /// single-target-set; a whole-room batch pass cannot honor N heterogeneous per-carrier scopes
    /// — see the report). Because the bot and the sim build the same DTOs and call the ONE kernel,
    /// the assignment is byte-identical to the sim's `market_pass` on the equivalent world (the
    /// live-wiring parity test asserts exactly that).
    ///
    /// Returns the FSM's ticket pair: a loaded hauler (carried `held > 0`) gets an empty pickup +
    /// a `Deliver`; an empty hauler gets a `PickupDeliver`. `None` falls through to the caller's
    /// existing tier path (which keeps the crate tier-capable for links/terminal/labs). Bookings
    /// are NOT registered here — the caller registers the returned tickets (the FSM's
    /// `register_pickup`/`register_delivery` contract, identical to the tier path).
    #[allow(clippy::too_many_arguments)]
    pub fn select_market_pickup_and_delivery<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
        transfer_type: TransferType,
        current_position: Position,
        free_capacity: u32,
        carried_energy: u32,
        target_filter: TF,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        use screeps_econ_decision::market::{MarketCarrier, MarketDeposit, MarketPickup, MarketTask};

        // Flush exactly the queried rooms/types (the lazy `with_econ_view` contract) so the raw
        // nodes carry this tick's full demand before we read the numeric bids off them. At the
        // hauling-pass batch point every generator is already flushed (the snapshot build);
        // this keeps the method correct for any caller.
        {
            let mut names: Vec<RoomName> = pickup_rooms.iter().chain(delivery_rooms.iter()).cloned().collect();
            names.sort_unstable();
            names.dedup();
            for room in &names {
                self.rooms.flush_generators(data, *room, transfer_type.into());
            }
        }

        let types: TransferTypeFlags = transfer_type.into();

        // ── Build the market DTOs from the RAW queue nodes (numeric bids, net of pending). ──
        // Deposits: every delivery-room node that can absorb energy on this lane, aggregated into
        // ONE sink per structure (the kernel's per-structure model) — sum the available energy
        // across its energy-acceptable deposit keys, take the highest bid as the sink's bid, and
        // flag the engine-fungible spawn lane (`is_refill`) so it aggregates. Deterministic:
        // nodes in `target_sort_key` order, indices assigned in that order.
        let mut deposit_targets: Vec<TransferTarget> = Vec::new();
        let mut deposits: Vec<MarketDeposit> = Vec::new();
        for room_name in delivery_rooms {
            let Some(room) = self.rooms.rooms.get(room_name) else {
                continue;
            };
            let mut nodes: Vec<(&TransferTarget, &TransferNode)> = room.nodes.iter().collect();
            nodes.sort_by_key(|(target, _)| target_sort_key(target));
            for (target, node) in nodes {
                if !target_filter(target) {
                    continue;
                }
                let mut unfulfilled = 0u32;
                let mut bid = 0u32;
                let mut keys: Vec<&TransferDepositKey> = node
                    .deposits
                    .keys()
                    .filter(|k| {
                        (k.resource == Some(ResourceType::Energy) || k.resource.is_none())
                            && types.intersects(k.allowed_type.into())
                    })
                    .collect();
                keys.sort_by_key(|k| deposit_key_sort(&k.to_econ()));
                for key in keys {
                    let available = node.get_available_deposit(key);
                    if available > 0 {
                        unfulfilled += available;
                        bid = bid.max(key.bid);
                    }
                }
                if unfulfilled == 0 {
                    continue;
                }
                let sink = deposit_targets.len() as u32;
                deposits.push(MarketDeposit {
                    sink,
                    pos: target.local_pos(),
                    bid_milli: bid,
                    unfulfilled,
                    is_refill: matches!(target, TransferTarget::Spawn(_) | TransferTarget::Extension(_)),
                });
                deposit_targets.push(*target);
            }
        }
        if deposits.is_empty() {
            return None;
        }

        // Pickups (empty-carrier only): every pickup-room node that can supply energy on this
        // lane, one source per structure, summed available energy. Same deterministic order.
        let mut pickup_targets: Vec<TransferTarget> = Vec::new();
        let mut pickups: Vec<MarketPickup> = Vec::new();
        if carried_energy == 0 {
            for room_name in pickup_rooms {
                let Some(room) = self.rooms.rooms.get(room_name) else {
                    continue;
                };
                let mut nodes: Vec<(&TransferTarget, &TransferNode)> = room.nodes.iter().collect();
                nodes.sort_by_key(|(target, _)| target_sort_key(target));
                for (target, node) in nodes {
                    let mut available = 0u32;
                    let mut keys: Vec<&TransferWithdrawlKey> = node
                        .withdrawls
                        .keys()
                        .filter(|k| k.resource == ResourceType::Energy && types.intersects(k.allowed_type.into()))
                        .collect();
                    keys.sort_by_key(|k| withdraw_key_sort(&k.to_econ()));
                    for key in keys {
                        available += node.get_available_withdrawl(key);
                    }
                    if available == 0 {
                        continue;
                    }
                    let src = pickup_targets.len() as u32;
                    pickups.push(MarketPickup {
                        src,
                        pos: target.local_pos(),
                        available,
                        source_floor_milli: target.source_floor_milli(),
                    });
                    pickup_targets.push(*target);
                }
            }
        }

        // The single carrier: a loaded hauler (`held > 0`) delivers carried cargo; an empty
        // hauler picks up + delivers. Haulers have no harvest alternative (`opportunity 0`).
        let carriers = [MarketCarrier {
            id: 0,
            pos: current_position,
            free: free_capacity,
            held: carried_energy,
            opportunity_milli: 0,
        }];

        let consts = MarketConsts::default();
        let input = crate::transfer::market_adapter::RoomMarketInput {
            deposits,
            pickups,
            carriers: carriers.to_vec(),
        };
        // `same_structure(src_idx, sink_idx)`: never withdraw from the very structure being served
        // (the kernel's self-withdraw guard) — the bot's `TransferTarget` identity.
        let result = crate::transfer::market_adapter::run_room_market(&consts, &input, |src_idx, sink_idx| {
            pickup_targets[src_idx as usize] == deposit_targets[sink_idx as usize]
        });

        let assignment = result.assignments.into_iter().next()?;
        // Map the kernel's index-scoped task back to live tickets (energy lane; the delivered bid
        // carries the SINK's numeric bid so the registered ticket reads identically to the queue).
        match assignment.task {
            MarketTask::Deliver { sink, amount, .. } => {
                let sink_target = deposit_targets[sink as usize];
                let sink_bid = input.deposits[sink as usize].bid_milli;
                // A loaded hauler has no pickup leg — an empty withdraw ticket keeps the FSM's
                // (pickup, delivery) shape; the delivery is what actually moves cargo.
                let empty_pickup = TransferWithdrawTicket {
                    target: sink_target,
                    resources: HashMap::new(),
                };
                let delivery = build_energy_deposit_ticket(sink_target, amount, sink_bid, transfer_type);
                Some((empty_pickup, delivery))
            }
            MarketTask::PickupDeliver { src, take, sink, give, .. } => {
                let src_target = pickup_targets[src as usize];
                let sink_target = deposit_targets[sink as usize];
                let sink_bid = input.deposits[sink as usize].bid_milli;
                // The withdraw ticket carries the SOURCE node's own energy bid (so the pickup
                // registration reads identically to the queue's withdraw key); the kernel took
                // `take` from that source.
                let src_bid = self.node_energy_withdraw_bid(&src_target);
                let pickup = build_energy_withdraw_ticket(src_target, take, src_bid, transfer_type);
                let delivery = build_energy_deposit_ticket(sink_target, give, sink_bid, transfer_type);
                Some((pickup, delivery))
            }
        }
    }

    /// The energy withdraw bid to stamp on a pickup ticket for `target`: the highest-bid energy
    /// withdraw key with remaining availability (deterministic; falls back to the numeraire when
    /// the source registered no bid this tick — an in-flight store). Reads the raw node so the
    /// registered ticket's bid matches the queue's key.
    fn node_energy_withdraw_bid(&self, target: &TransferTarget) -> u32 {
        let room_name = target.local_pos().room_name();
        self.rooms
            .rooms
            .get(&room_name)
            .and_then(|room| room.nodes.get(target))
            .and_then(|node| {
                node.withdrawls
                    .keys()
                    .filter(|k| k.resource == ResourceType::Energy && node.get_available_withdrawl(k) > 0)
                    .map(|k| k.bid)
                    .max()
            })
            .unwrap_or(screeps_econ_decision::sink_economics::STORAGE_BID)
    }

    /// Terminal routing: pair a terminal's own supply with the best cross-room delivery
    /// (adapter orchestration — the map-cost ranking stays live-side; node selection is the
    /// K2 kernel).
    #[allow(clippy::too_many_arguments)]
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

        let source_room = target.local_pos().room_name();

        // Node-level available totals (net of bookings) for the terminal's own supply.
        let available_resources: HashMap<ResourceType, u32> =
            self.with_econ_view(data, &[source_room], delivery_type.into(), |view| {
                let node = view.node_id(target)?;
                let mut totals: HashMap<ResourceType, u32> = HashMap::new();
                for (key, _) in &view.snapshot.node(node).withdrawls {
                    if allowed_pickup_priorities.intersects(key.priority.into())
                        && TransferTypeFlags::from(delivery_type).intersects(key.allowed_type.into())
                    {
                        let available = econ::available_withdrawl(&view.snapshot, &view.bookings, node, key);
                        if available > 0 {
                            *totals.entry(key.resource).or_insert(0) += available;
                        }
                    }
                }
                Some(totals)
            })?;

        if available_resources.is_empty() {
            return None;
        }

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

        let delivery_resources: Vec<(Option<ResourceType>, u32)> = {
            let mut v: Vec<(Option<ResourceType>, u32)> = delivery
                .resources()
                .iter()
                .map(|(resource, entries)| {
                    let total: u32 = entries.iter().map(|entry| entry.amount()).sum();
                    (Some(*resource), total)
                })
                .collect();
            v.sort_by_key(|(r, _)| r.map(|r| 1 + r as u32).unwrap_or(0));
            v
        };

        let pickup = self.with_econ_view(data, &[source_room], delivery_type.into(), |view| {
            let node = view.node_id(target)?;
            let pickup_resources = econ::node_select_pickup(
                &view.snapshot,
                &view.bookings,
                node,
                allowed_pickup_priorities,
                delivery_type.into(),
                &delivery_resources,
                available_capacity,
            );
            Some(view.withdraw_ticket(econ::WithdrawTicketDto {
                node,
                pos: view.snapshot.node(node).pos,
                resources: pickup_resources,
            }))
        })?;

        Some((pickup, delivery))
    }

    /// A single-resource top-off pickup from a specific target (K2 kernel
    /// `get_pickup_from_node` — the `tick_pickup_and_fill` arm).
    pub fn get_pickup_from_target(
        &mut self,
        data: &dyn TransferRequestSystemData,
        target: &TransferTarget,
        allowed_pickup_priorities: TransferPriorityFlags,
        transfer_types: TransferTypeFlags,
        available_capacity: TransferCapacity,
        resource_type: ResourceType,
    ) -> Option<TransferWithdrawTicket> {
        let room = target.local_pos().room_name();
        self.with_econ_view(data, &[room], transfer_types, |view| {
            let node = view.node_id(target)?;
            econ::get_pickup_from_node(
                &view.snapshot,
                &view.bookings,
                node,
                allowed_pickup_priorities,
                transfer_types,
                available_capacity,
                resource_type,
            )
            .map(|dto| view.withdraw_ticket(dto))
        })
    }

    /// Pair a specific pickup target's availability with the best delivery for it (K2 kernel
    /// `get_delivery_from_node` — the additional-deliveries + link-routing arm).
    #[allow(clippy::too_many_arguments)]
    pub fn get_delivery_from_target<TF>(
        &mut self,
        data: &dyn TransferRequestSystemData,
        delivery_rooms: &[RoomName],
        target: &TransferTarget,
        allowed_pickup_priorities: TransferPriorityFlags,
        allowed_delivery_priorities: TransferPriorityFlags,
        delivery_type: TransferType,
        available_capacity: TransferCapacity,
        anchor_location: Position,
        target_filter: TF,
    ) -> Option<(TransferWithdrawTicket, TransferDepositTicket)>
    where
        TF: Fn(&TransferTarget) -> bool,
    {
        let mut rooms: Vec<RoomName> = delivery_rooms.to_vec();
        rooms.push(target.local_pos().room_name());
        self.with_econ_view(data, &rooms, delivery_type.into(), |view| {
            let node = view.node_id(target)?;
            econ::get_delivery_from_node(
                &view.snapshot,
                &view.bookings,
                delivery_rooms,
                node,
                allowed_pickup_priorities,
                allowed_delivery_priorities,
                delivery_type,
                available_capacity,
                anchor_location,
                |node| target_filter(&view.target(node)),
            )
            .map(|(pickup, delivery)| (view.withdraw_ticket(pickup), view.deposit_ticket(delivery)))
        })
    }

    /// The cross-room terminal delivery ranking (adapter orchestration: the transaction-cost
    /// model reads `game::map` and stays live-side; per-room node selection is kernel-routed
    /// via [`Self::select_single_delivery_for_room`]).
    #[allow(clippy::too_many_arguments)]
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

                // Hard ceiling on intra-empire send cost (ADR 0012 §3 /
                // IBEX-018): the `resources/cost` ranking below has no floor,
                // so a far room could win with an arbitrarily bad ratio when
                // it was the only candidate. Long-haul energy belongs to
                // battery compression (ADR 0010), not raw terminal sends.
                if cost_per_unit > crate::transfer::fairvalue::MAX_INTRA_EMPIRE_COST_PER_UNIT {
                    return Vec::new();
                }

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

                let to = delivery.target.local_pos().room_name();

                let cost_per_unit = super::utility::calc_transaction_cost_fractional(anchor_location, to);

                let cost = (cost_per_unit * resources as f64).ceil();
                let value = finite_transfer_value(resources, cost as f32);

                (delivery, value)
            })
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(delivery, _)| delivery)
    }

    /// The matched-flow hauling statistic: stats-level per-resource (active, inactive) sums
    /// (live semantics — `unfufilled_amount()` never nets reservations) through the K2 kernel's
    /// shared stage math (`matched_unfulfilled_resources`).
    pub fn total_unfufilled_resources(
        &mut self,
        data: &dyn TransferRequestSystemData,
        pickup_rooms: &[RoomName],
        delivery_rooms: &[RoomName],
        transfer_type: TransferType,
    ) -> HashMap<ResourceType, u32> {
        let mut withdrawls: HashMap<ResourceType, econ::StageSums> = HashMap::new();
        let mut deposits: HashMap<Option<ResourceType>, econ::StageSums> = HashMap::new();

        for pickup_room in pickup_rooms {
            if let Some(room) = self.try_get_room(data, *pickup_room, transfer_type.into()) {
                for (key, stats) in &room.stats.withdrawl_resource_stats {
                    if key.allowed_type == transfer_type {
                        let resource_entry = withdrawls.entry(key.resource).or_default();

                        if TransferPriorityFlags::ACTIVE.intersects(bid_to_tier(key.bid).into()) {
                            resource_entry.active += stats.unfufilled_amount().max(0) as u32;
                        } else {
                            resource_entry.inactive += stats.unfufilled_amount().max(0) as u32;
                        }
                    }
                }
            }
        }

        for delivery_room in delivery_rooms {
            if let Some(room) = self.try_get_room(data, *delivery_room, transfer_type.into()) {
                for (key, stats) in &room.stats.deposit_resource_stats {
                    if key.allowed_type == transfer_type {
                        let resource_entry = deposits.entry(key.resource).or_default();

                        if TransferPriorityFlags::ACTIVE.intersects(bid_to_tier(key.bid).into()) {
                            resource_entry.active += stats.unfufilled_amount().max(0) as u32;
                        } else {
                            resource_entry.inactive += stats.unfufilled_amount().max(0) as u32;
                        }
                    }
                }
            }
        }

        // Deterministic stage-competition order (module docs on the kernel).
        let mut withdraw_input: Vec<(ResourceType, econ::StageSums)> = withdrawls.into_iter().collect();
        withdraw_input.sort_by_key(|(r, _)| *r as u32);
        let mut deposit_input: Vec<(Option<ResourceType>, econ::StageSums)> = deposits.into_iter().collect();
        deposit_input.sort_by_key(|(r, _)| r.map(|r| 1 + r as u32).unwrap_or(0));

        econ::matched_unfulfilled_resources(&withdraw_input, &deposit_input)
            .into_iter()
            .collect()
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
        // The per-tick K2 view dies with the queue (rebuilt at the next hauling pass).
        self.econ = None;
    }

    /// Build a snapshot of transfer queue state for visualization (does not clear the queue).
    /// Requires `&mut self` because `get_room_no_flush` takes `&mut self`.
    pub fn snapshot_for_visualization(&mut self) -> TransferStatsSnapshot {
        let all_rooms = self.rooms.get_all_rooms();
        let mut rooms = HashMap::new();

        for room_name in all_rooms {
            let room_data = self.rooms.get_room_no_flush(room_name);
            let stats = &room_data.stats;

            let mut resources: HashMap<screeps::ResourceType, TransferResourceStats> = HashMap::new();
            let mut generic_demand: u32 = 0;
            let mut generic_demand_pending: u32 = 0;
            let mut generic_demand_by_priority = [0u32; 4];

            // The bid's projected tier band → the [High, Medium, Low, None] display bucket.
            fn priority_index(bid: u32) -> usize {
                bid_to_tier(bid) as usize
            }

            // Aggregate withdrawals (supply side): always a concrete resource.
            for (key, res_stats) in &stats.withdrawl_resource_stats {
                let amount = res_stats.amount();
                let pending = res_stats.pending_amount();
                let idx = priority_index(key.bid).min(3);

                let entry = resources.entry(key.resource).or_default();
                entry.supply += amount;
                entry.supply_pending += pending;
                entry.supply_by_priority[idx] += amount;
            }

            // Aggregate deposits (demand side): Some(resource) -> per-resource; None -> generic.
            for (key, res_stats) in &stats.deposit_resource_stats {
                let amount = res_stats.amount();
                let pending = res_stats.pending_amount();
                let idx = priority_index(key.bid).min(3);

                if let Some(resource) = key.resource {
                    let entry = resources.entry(resource).or_default();
                    entry.demand += amount;
                    entry.demand_pending += pending;
                    entry.demand_by_priority[idx] += amount;
                } else {
                    generic_demand += amount;
                    generic_demand_pending += pending;
                    generic_demand_by_priority[idx] += amount;
                }
            }

            rooms.insert(
                room_name,
                TransferRoomSnapshot {
                    resources,
                    generic_demand,
                    generic_demand_pending,
                    generic_demand_by_priority,
                },
            );
        }

        TransferStatsSnapshot { rooms }
    }
}

// ─── Transfer stats snapshot system ──────────────────────────────────────────

#[derive(SystemData)]
pub struct TransferStatsSnapshotSystemData<'a> {
    viz_gate: Option<Read<'a, crate::visualization::VisualizationData>>,
    transfer_queue: Write<'a, TransferQueue>,
    transfer_stats_snapshot: Option<Write<'a, TransferStatsSnapshot>>,
    room_data: ReadStorage<'a, RoomData>,
}

pub struct TransferStatsSnapshotSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for TransferStatsSnapshotSystem {
    type SystemData = TransferStatsSnapshotSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // Only build snapshot when visualization is on.
        if data.viz_gate.is_none() {
            return;
        }

        // Flush all generators so the snapshot includes every room's requests (when viz is off, generators stay lazy).
        let generator_data = TransferQueueGeneratorData {
            cause: "transfer_stats_snapshot",
            room_data: &data.room_data,
        };
        data.transfer_queue.flush_all_generators(&generator_data);

        let snapshot = data.transfer_queue.snapshot_for_visualization();

        if let Some(ref mut res) = data.transfer_stats_snapshot {
            **res = snapshot;
        }
    }
}

// ─── Transfer queue update system ────────────────────────────────────────────

#[derive(SystemData)]
pub struct TransferQueueUpdateSystemData<'a> {
    transfer_queue: Write<'a, TransferQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
}

pub struct TransferQueueUpdateSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for TransferQueueUpdateSystem {
    type SystemData = TransferQueueUpdateSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.transfer_queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_econ_decision::sink_economics::STORAGE_BID;

    // Pin (IBEX-046): transfer value computations guard their divisors so a
    // zero length/cost cannot produce NaN or infinity for the priority
    // comparators.

    #[test]
    fn finite_transfer_value_guards_zero_divisor() {
        assert_eq!(finite_transfer_value(0, 0.0), 0.0);
        assert_eq!(finite_transfer_value(10, 0.0), 10.0);
    }

    #[test]
    fn finite_transfer_value_normal_cases() {
        assert_eq!(finite_transfer_value(10, 5.0), 2.0);
        assert_eq!(finite_transfer_value(0, 5.0), 0.0);
        // Sub-1 divisors are clamped up to 1.
        assert_eq!(finite_transfer_value(10, 0.5), 10.0);
    }

    #[test]
    fn finite_transfer_value_is_always_finite() {
        for resources in [0u32, 1, 100, 1_000_000] {
            for divisor in [0.0f32, 0.25, 1.0, 50.0, 10_000.0] {
                assert!(finite_transfer_value(resources, divisor).is_finite());
            }
        }
    }

    // ── The M3 snapshot adapter: parity + cost spot-check fixtures ─────────────────────────────

    struct NoRoomData;
    impl TransferRequestSystemData for NoRoomData {
        fn get_cause(&self) -> &str {
            "test"
        }
        fn get_room_data(&self, _entity: Entity) -> Option<&RoomData> {
            None
        }
    }

    fn test_pos(room: &str, x: u8, y: u8) -> Position {
        Position::new(
            screeps::RoomCoordinate::new(x).unwrap(),
            screeps::RoomCoordinate::new(y).unwrap(),
            room.parse().unwrap(),
        )
    }

    fn spawn_target(room: &str, x: u8, y: u8, id: u128) -> TransferTarget {
        TransferTarget::Spawn(RemoteObjectId::new_from_components(
            screeps::local::RawObjectId::from_packed(id).into(),
            test_pos(room, x, y),
        ))
    }

    fn storage_target(room: &str, x: u8, y: u8, id: u128) -> TransferTarget {
        TransferTarget::Storage(RemoteObjectId::new_from_components(
            screeps::local::RawObjectId::from_packed(id).into(),
            test_pos(room, x, y),
        ))
    }

    /// Fill one room's worth of realistic demand: a spawn deficit (High deposit) + a stocked
    /// storage (None withdraw + accepts-any deposit).
    fn fill_room(queue: &mut TransferQueue, room: &str, id_base: u128) -> (TransferTarget, TransferTarget) {
        let spawn = spawn_target(room, 25, 25, id_base);
        let storage = storage_target(room, 20, 25, id_base + 1);
        queue.request_deposit(TransferDepositRequest::new_tier(
            spawn,
            Some(ResourceType::Energy),
            TransferPriority::High,
            300,
            TransferType::Haul,
        ));
        queue.request_withdraw(TransferWithdrawRequest::new_tier(
            storage,
            ResourceType::Energy,
            TransferPriority::None,
            50_000,
            TransferType::Haul,
        ));
        queue.request_deposit(TransferDepositRequest::new_tier(
            storage,
            None,
            TransferPriority::None,
            100_000,
            TransferType::Haul,
        ));
        (spawn, storage)
    }

    /// PRE-MOVE behavior fixture: the storage(None)→spawn(High) pairing wins the first
    /// interleave combination, the pickup is capacity-clamped, and a registered assignment
    /// reserves the node for the next hauler — exactly what the inline
    /// `select_pickup_and_delivery` + `pending_*` reservation did before the K2 extraction.
    #[test]
    fn snapshot_selection_matches_premove_pairing_and_reserves() {
        let mut queue = TransferQueue::default();
        let (spawn, storage) = fill_room(&mut queue, "W1N1", 0x100);
        let rooms = ["W1N1".parse().unwrap()];

        queue.build_econ_snapshot(&NoRoomData);

        let (pickup, delivery) = queue
            .select_pickup_and_delivery(
                &NoRoomData,
                &rooms,
                &rooms,
                TransferPriorityFlags::ALL,
                TransferType::Haul,
                test_pos("W1N1", 22, 25),
                TransferCapacity::Finite(200),
                target_filters::all,
            )
            .expect("the storage→spawn pairing");
        assert_eq!(*pickup.target(), storage);
        assert_eq!(*delivery.target(), spawn);
        let picked: u32 = pickup
            .resources()
            .values()
            .flat_map(|entries| entries.iter().map(|e| e.amount()))
            .sum();
        assert_eq!(picked, 200, "pickup clamped to creep capacity");

        // Register (books the reservation): the delivery ticket was capacity-clamped to 200 of
        // the 300 deficit, so a SECOND hauler sees exactly the 100 remainder — the pre-move
        // `pending_*` reservation contract (reduced availability, never a double-serve).
        queue.register_pickup(&pickup);
        queue.register_delivery(&delivery);
        let (second_pickup, second_delivery) = queue
            .select_pickup_and_delivery(
                &NoRoomData,
                &rooms,
                &rooms,
                TransferPriorityFlags::HIGH | TransferPriorityFlags::NONE,
                TransferType::Haul,
                test_pos("W1N1", 22, 25),
                TransferCapacity::Finite(200),
                target_filters::all,
            )
            .expect("the 100-energy remainder");
        let second_amount: u32 = second_pickup
            .resources()
            .values()
            .flat_map(|entries| entries.iter().map(|e| e.amount()))
            .sum();
        assert_eq!(*second_delivery.target(), spawn);
        assert_eq!(second_amount, 100, "only the unreserved remainder is served");
        queue.register_pickup(&second_pickup);
        queue.register_delivery(&second_delivery);

        // Fully booked: a THIRD hauler on the refill lane finds nothing High to serve.
        let third = queue.select_pickup_and_delivery(
            &NoRoomData,
            &rooms,
            &rooms,
            TransferPriorityFlags::HIGH | TransferPriorityFlags::NONE,
            TransferType::Haul,
            test_pos("W1N1", 22, 25),
            TransferCapacity::Finite(200),
            target_filters::all,
        );
        assert!(third.is_none(), "the booked spawn is reserved; no double-serve");
    }

    /// The carried-cargo nearest-wins path through the snapshot (the pre-move
    /// `select_deliveries` + `find_nearest_linear_by` composition).
    #[test]
    fn snapshot_nearest_delivery_matches_premove() {
        let mut queue = TransferQueue::default();
        let (spawn, _storage) = fill_room(&mut queue, "W1N1", 0x200);
        let rooms = ["W1N1".parse().unwrap()];
        queue.build_econ_snapshot(&NoRoomData);

        let carried = vec![(ResourceType::Energy, 50u32)];
        let ticket = queue
            .select_nearest_delivery(
                &NoRoomData,
                &rooms,
                TransferPriorityFlags::ACTIVE,
                TransferTypeFlags::HAUL,
                &carried,
                TransferCapacity::Finite(50),
                test_pos("W1N1", 10, 25),
                target_filters::all,
            )
            .expect("the spawn deficit");
        assert_eq!(*ticket.target(), spawn, "the only ACTIVE sink; storage(None) never competes");
    }

    // ═════════════════════════════════════════════════════════════════════════════════════════
    // ADR 0040 M5a — the LIVE-SELECTION WIRING PROOF (the gate the M5a-core slice lacked).
    //
    // The M5a-core parity test (`screeps-econ-eval/tests/market_parity.rs`) proved the SIM MARKET
    // arm (`MarketRuntime::market_pass`) equals the shared kernel (`market::market_pass`) by
    // reconstructing kernel DTOs from the world — a kernel-in-isolation proof. What it did NOT
    // exercise is the LIVE hauler-facing selection (the thing the FSM actually calls). These tests
    // close that gap: they drive the REAL live method `TransferQueue::select_market_pickup_and_
    // delivery` on a fixture room, and assert its assignment equals the shared kernel run over the
    // EQUIVALENT DTOs built the exact way the SIM MARKET arm builds them (`market.rs`'s
    // `k_carriers`/`k_deposits`/`k_pickups` field mapping, inlined below). Since
    // market_parity.rs already pins (sim arm == kernel), (live == kernel-with-sim-arm-DTOs) closes
    // the loop: the live selection == the tournament MARKET arm, by construction.
    // ═════════════════════════════════════════════════════════════════════════════════════════

    fn container_target(room: &str, x: u8, y: u8, id: u128) -> TransferTarget {
        TransferTarget::Container(RemoteObjectId::new_from_components(
            screeps::local::RawObjectId::from_packed(id).into(),
            test_pos(room, x, y),
        ))
    }

    /// Build a market-priced fixture room in the LIVE queue with explicit NUMERIC bids (the same
    /// facts the kernel DTOs below carry): a spawn refill deficit (high bid, engine-fungible), a
    /// stressed near container (high bid, per-structure), and a par storage that both supplies
    /// (big withdraw) and buffers (accepts-any par deposit). Returns the targets.
    #[allow(clippy::type_complexity)]
    fn market_fixture(queue: &mut TransferQueue, room: &str, id: u128) -> (TransferTarget, TransferTarget, TransferTarget) {
        let spawn = spawn_target(room, 25, 25, id);
        let container = container_target(room, 11, 10, id + 1);
        let storage = storage_target(room, 30, 25, id + 2);
        // Spawn refill deposit: bid 6000 (a stressed refill), deficit 250, engine-fungible lane.
        queue.request_deposit(TransferDepositRequest::new(spawn, Some(ResourceType::Energy), 6000, 250, TransferType::Haul));
        // Stressed container deposit: bid 5000, unmet 100 (per-structure sink).
        queue.request_deposit(TransferDepositRequest::new(container, Some(ResourceType::Energy), 5000, 100, TransferType::Haul));
        // Storage: par deposit (accepts-any) + a big supply to pick up from.
        queue.request_deposit(TransferDepositRequest::new(storage, None, STORAGE_BID, 500_000, TransferType::Haul));
        queue.request_withdraw(TransferWithdrawRequest::new(storage, ResourceType::Energy, STORAGE_BID, 50_000, TransferType::Haul));
        (spawn, container, storage)
    }

    /// Build the kernel DTOs the way the SIM MARKET arm does (`market.rs` k_deposits/k_pickups/
    /// k_carriers) from the SAME facts — one deposit per sink (bid + unmet + is_refill), one
    /// pickup per haul source, one carrier. This IS the sim arm's `market_pass` core.
    #[allow(clippy::type_complexity)]
    fn sim_arm_kernel_assignment(
        carrier: screeps_econ_decision::market::MarketCarrier,
        deposits: &[(TransferTarget, u32, u32, bool)], // (target, bid, unmet, is_refill)
        pickups: &[(TransferTarget, u32)],             // (target, available)
    ) -> Option<screeps_econ_decision::market::MarketAssignment> {
        use screeps_econ_decision::market::{market_pass, MarketDeposit, MarketPickup};
        let k_deposits: Vec<MarketDeposit> = deposits
            .iter()
            .enumerate()
            .map(|(i, (t, bid, unmet, is_refill))| MarketDeposit {
                sink: i as u32,
                pos: t.local_pos(),
                bid_milli: *bid,
                unfulfilled: *unmet,
                is_refill: *is_refill,
            })
            .collect();
        let k_pickups: Vec<MarketPickup> = pickups
            .iter()
            .enumerate()
            .map(|(i, (t, avail))| MarketPickup {
                src: i as u32,
                pos: t.local_pos(),
                available: *avail,
                source_floor_milli: t.source_floor_milli(),
            })
            .collect();
        let deposit_targets: Vec<TransferTarget> = deposits.iter().map(|(t, ..)| *t).collect();
        let pickup_targets: Vec<TransferTarget> = pickups.iter().map(|(t, _)| *t).collect();
        market_pass(
            &[carrier],
            &k_deposits,
            &k_pickups,
            screeps_econ_decision::sink_economics::HAUL_ROAD_Q_PLAINS_PERMILLE,
            &mut |a: screeps::Position, b: screeps::Position| a.get_range_to(b),
            |src, sink| pickup_targets[src as usize] == deposit_targets[sink as usize],
        )
        .assignments
        .into_iter()
        .next()
    }

    /// LOADED hauler, the BID-RESOLUTION discriminator: two same-band, EQUIDISTANT deposits with
    /// different RAW bids. The tier path would band both to `High` and break the range tie by node
    /// order (blind to the bid — "a 15-bid extension and a 6-bid tower both become High and nearest
    /// wins"); the bid-native live path (and the sim MARKET arm) pick the HIGHER raw bid. The live
    /// selection's sink + amount equal the sim MARKET arm's `Deliver` task.
    #[test]
    fn live_market_selection_loaded_matches_sim_arm() {
        use screeps_econ_decision::market::{MarketCarrier, MarketTask};
        let mut queue = TransferQueue::default();
        // Two equidistant sinks either side of the carrier (both range 1 → identical service):
        // a lower-bid container and a higher-bid spawn refill. Raw bid is the only differentiator.
        let carrier_pos = test_pos("W4N4", 25, 25);
        let low = container_target("W4N4", 24, 25, 0x4001); // bid 4100 (High band)
        let high = spawn_target("W4N4", 26, 25, 0x4002); // bid 6000 (High band), engine-fungible
        queue.request_deposit(TransferDepositRequest::new(low, Some(ResourceType::Energy), 4100, 100, TransferType::Haul));
        queue.request_deposit(TransferDepositRequest::new(high, Some(ResourceType::Energy), 6000, 100, TransferType::Haul));
        let rooms = ["W4N4".parse().unwrap()];
        queue.build_econ_snapshot(&NoRoomData);

        // ── Live path: run the REAL hauler-facing selection for a loaded hauler (held 100). ──
        let (pickup, delivery) = queue
            .select_market_pickup_and_delivery(
                &NoRoomData,
                &rooms,
                &rooms,
                TransferType::Haul,
                carrier_pos,
                0,   // free
                100, // carried energy
                target_filters::all,
            )
            .expect("the market assigns the loaded hauler a delivery");
        assert!(pickup.resources().is_empty(), "a loaded hauler has no pickup leg");
        let live_sink = *delivery.target();
        let live_amount: u32 = delivery.resources().values().flat_map(|e| e.iter().map(|x| x.amount())).sum();

        // ── Sim MARKET arm (kernel over the equivalent DTOs, sim-arm field mapping). ──
        let carrier = MarketCarrier { id: 0, pos: carrier_pos, free: 0, held: 100, opportunity_milli: 0 };
        let deposits = [(low, 4100u32, 100u32, false), (high, 6000, 100, true)];
        let sim = sim_arm_kernel_assignment(carrier, &deposits, &[]).expect("sim arm assigns too");
        match sim.task {
            MarketTask::Deliver { sink, amount, .. } => {
                let sim_sink = deposits[sink as usize].0;
                assert_eq!(live_sink, sim_sink, "live sink target == sim MARKET arm sink");
                assert_eq!(live_amount, amount, "live delivered amount == sim MARKET arm amount");
                // Non-vacuous AND discriminating: the HIGHER raw bid wins even though the two sinks
                // are equidistant and in the same tier band — the exact resolution the tier path
                // throws away.
                assert_eq!(sim_sink, high, "the higher raw bid wins among same-band equidistant sinks");
                assert_eq!(amount, 100, "the whole carried cargo lands");
            }
            _ => panic!("the loaded hauler must Deliver"),
        }
    }

    /// EMPTY hauler: the live method picks up + delivers; the (src, sink, take, give) equal the sim
    /// MARKET arm's `PickupDeliver` task on the equivalent world.
    #[test]
    fn live_market_selection_empty_matches_sim_arm() {
        use screeps_econ_decision::market::{MarketCarrier, MarketTask};
        let mut queue = TransferQueue::default();
        let (spawn, container, storage) = market_fixture(&mut queue, "W2N2", 0x2000);
        let rooms = ["W2N2".parse().unwrap()];
        queue.build_econ_snapshot(&NoRoomData);

        let carrier_pos = test_pos("W2N2", 29, 25); // next to storage — cheapest pickup
        // ── Live path: an empty hauler (free 100). ──
        let (pickup, delivery) = queue
            .select_market_pickup_and_delivery(
                &NoRoomData,
                &rooms,
                &rooms,
                TransferType::Haul,
                carrier_pos,
                100, // free
                0,   // carried
                target_filters::all,
            )
            .expect("the market assigns the empty hauler a pickup+delivery");
        let live_src = *pickup.target();
        let live_sink = *delivery.target();
        let live_take: u32 = pickup.resources().values().flat_map(|e| e.iter().map(|x| x.amount())).sum();
        let live_give: u32 = delivery.resources().values().flat_map(|e| e.iter().map(|x| x.amount())).sum();

        // ── Sim MARKET arm (kernel over equivalent DTOs). ──
        let carrier = MarketCarrier { id: 0, pos: carrier_pos, free: 100, held: 0, opportunity_milli: 0 };
        let deposits = [
            (spawn, 6000u32, 250u32, true),
            (container, 5000, 100, false),
            (storage, STORAGE_BID, 500_000, false),
        ];
        // Only the storage supplies on the haul lane (the sim's `k_pickups`).
        let pickups = [(storage, 50_000u32)];
        let sim = sim_arm_kernel_assignment(carrier, &deposits, &pickups).expect("sim arm assigns too");
        match sim.task {
            MarketTask::PickupDeliver { src, take, sink, give, .. } => {
                let sim_src = pickups[src as usize].0;
                let sim_sink = deposits[sink as usize].0;
                assert_eq!(live_src, sim_src, "live pickup source == sim MARKET arm src");
                assert_eq!(live_sink, sim_sink, "live delivery sink == sim MARKET arm sink");
                assert_eq!(live_take, take, "live take == sim MARKET arm take");
                assert_eq!(live_give, give, "live give == sim MARKET arm give");
                // Non-vacuous: storage supplies, the refill lane is the densest admitted sink.
                assert_eq!(sim_src, storage);
                assert_eq!(sim_sink, spawn);
            }
            _ => panic!("the empty hauler must PickupDeliver"),
        }
    }

    /// S1-REPLACEMENT re-confirmation through the bid-native live path: under a deep refill
    /// deficit the opportunity floor (published off the numeric-bid deposit keys the SAME
    /// selection admits against) prices OUT a quiet-road repair + a par Use-lane withdraw, and a
    /// survival bid bypasses. Mirrors `market_adapter::deep_deficit_prices_out_use_and_repair`,
    /// but reads the floor off the LIVE queue the wiring publishes from.
    #[test]
    fn live_floor_prices_out_use_and_repair_under_deep_deficit() {
        use screeps_econ_decision::sink_economics as econ;
        let mut queue = TransferQueue::default();
        let (_spawn, _container, _storage) = market_fixture(&mut queue, "W3N3", 0x3000);
        queue.build_econ_snapshot(&NoRoomData);

        let mut summary = MarketBidSummary::default();
        queue.publish_market_floor(&mut summary);
        let room: RoomName = "W3N3".parse().unwrap();
        let floor = summary.rooms.get(&room).map(|r| r.opportunity_floor).unwrap_or(0);
        // The floor is the highest materially-unmet deposit bid — the stressed refill (6000).
        assert_eq!(floor, 6000, "the deep-deficit floor is the stressed refill bid");
        // A quiet 40% road (~0.37) + a par Use withdraw are priced OUT; survival bypasses.
        assert!(!econ::admit_repair(370, floor), "quiet road priced out under deep deficit");
        assert!(!econ::admit_use_withdraw(econ::STORAGE_BID, floor), "par Use-lane withdraw priced out");
        assert!(econ::admit_use_withdraw(econ::SURVIVAL_BID, floor), "survival bypasses the floor");
    }

    /// COST SPOT-CHECK (ADR 0007 item 1's "generation provably paid once" + the M3 battery's
    /// per-tick cost check): build the per-tick snapshot over a 10-room colony-scale queue and
    /// run a fleet's worth of selections. Ignored by default — run explicitly with
    /// `cargo test -p screeps-ibex --release econ_snapshot_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn econ_snapshot_cost_spot_check() {
        let rooms: Vec<RoomName> = (1..=10).map(|i| format!("W{i}N1").parse().unwrap()).collect();
        let mut queue = TransferQueue::default();
        for (i, room) in rooms.iter().enumerate() {
            let base = (i as u128 + 1) * 0x10000;
            let room_s = room.to_string();
            fill_room(&mut queue, &room_s, base);
            // Pad with extension-scale deposit fan-out (20 nodes) + dropped piles.
            for e in 0..20u128 {
                let ext = TransferTarget::Extension(RemoteObjectId::new_from_components(
                    screeps::local::RawObjectId::from_packed(base + 0x100 + e).into(),
                    test_pos(&room_s, 10 + (e as u8), 20),
                ));
                queue.request_deposit(TransferDepositRequest::new_tier(
                    ext,
                    Some(ResourceType::Energy),
                    TransferPriority::High,
                    50,
                    TransferType::Haul,
                ));
            }
            for d in 0..5u128 {
                let pile = TransferTarget::Resource(RemoteObjectId::new_from_components(
                    screeps::local::RawObjectId::from_packed(base + 0x200 + d).into(),
                    test_pos(&room_s, 30 + (d as u8), 30),
                ));
                queue.request_withdraw(TransferWithdrawRequest::new_tier(
                    pile,
                    ResourceType::Energy,
                    TransferPriority::High,
                    600,
                    TransferType::Haul,
                ));
            }
        }

        let t0 = std::time::Instant::now();
        queue.build_econ_snapshot(&NoRoomData);
        let build = t0.elapsed();

        // 30 haulers' worth of selections (3 per room), booking as they go — the per-tick
        // hauling-pass shape.
        let t1 = std::time::Instant::now();
        let mut assigned = 0;
        for room in rooms.iter() {
            let one = [*room];
            for h in 0..3u8 {
                if let Some((pickup, delivery)) = queue.select_pickup_and_delivery(
                    &NoRoomData,
                    &one,
                    &one,
                    TransferPriorityFlags::ALL,
                    TransferType::Haul,
                    test_pos(&room.to_string(), 15 + h, 25),
                    TransferCapacity::Finite(200),
                    target_filters::all,
                ) {
                    queue.register_pickup(&pickup);
                    queue.register_delivery(&delivery);
                    assigned += 1;
                }
            }
        }
        let select = t1.elapsed();
        println!(
            "econ snapshot cost: build(10 rooms, ~{} nodes) = {:?}; 30 selections (assigned {}) = {:?} ({:?}/selection)",
            10 * 27,
            build,
            assigned,
            select,
            select / 30,
        );
        assert!(assigned >= 10, "selections found work ({assigned})");
    }
}
