//! K1 — the transfer demand-registration policy, MOVED at ADR 0040 M3 from
//! `screeps-ibex/src/missions/localsupply/room_transfer.rs` (the HAUL|USE generator's
//! per-structure request policy + `controller_link_deposit` + the link priority ladders). Lives
//! here now, consumed by the bot (`RoomTransferMission`'s generators build a [`RoomEconDto`]
//! and execute the returned [`Demand`]s) and by the sim (`screeps-econ-eval::baseline`'s
//! `deposits()`/`pickups()` adapters, whose transcribed tier policy is deleted).
//!
//! **Arithmetic note:** the live tier ladders compared `f32` store fractions; this kernel
//! compares exact integer cross-products — identical on every reachable store size (< 2^24;
//! the same argument as [`crate::repair`]'s maps).
//!
//! **Emission order** is deterministic and defined here (spawns, extensions, containers in
//! DTO order, storage, ruins, tombstones, dropped — deposit-before-withdraw within a
//! structure). The live queue sums requests commutatively so order is invisible live; the sim
//! uses it as the selection candidate order (its documented determinism convention).

use crate::priority::{TransferPriority, TransferType};
use screeps::ResourceType;

/// Opaque structure identity within one [`RoomEconDto`] — an adapter-side index. The adapter
/// keeps the aligned table mapping back to real targets.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ItemRef(pub u32);

/// Which side of the transfer market a demand rides.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DemandSide {
    /// Supply: others may take from this structure (`request_withdraw`).
    Withdraw,
    /// Demand: others should bring to this structure (`request_deposit`).
    Deposit,
}

/// One demand-registration decision (K1 output; the `RegisterWithdraw`/`RegisterDeposit`
/// intent payload). Withdraw demands always carry `Some(resource)`; deposit demands may carry
/// `None` (accepts-any).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Demand {
    pub item: ItemRef,
    pub side: DemandSide,
    pub resource: Option<ResourceType>,
    pub priority: TransferPriority,
    pub amount: u32,
    pub transfer_type: TransferType,
}

/// A container's economic role (the live `structure_data` classification: source/mineral
/// provider containers, controller upgrade-buffer containers, everything else).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ContainerRole {
    /// Fed by miners; drained by haulers (`sources_to_containers` +
    /// `mineral_extractors_to_containers`).
    Provider,
    /// The controller upgrade buffer (`controllers_to_containers`).
    Controller,
    /// Overflow/storage containers (everything not provider/controller).
    Other,
}

/// One spawn/extension as K1 sees it.
#[derive(Copy, Clone, Debug)]
pub struct RefillStructDto {
    pub item: ItemRef,
    /// `get_free_capacity(Some(Energy))`, floored at 0.
    pub free_energy: u32,
}

/// One container as K1 sees it.
#[derive(Clone, Debug)]
pub struct ContainerDto {
    pub item: ItemRef,
    pub role: ContainerRole,
    /// Store contents in `store_types()` order (adapter-controlled).
    pub store: Vec<(ResourceType, u32)>,
    /// Total store capacity (`get_capacity(None)`; for a container this also equals
    /// `get_capacity(Some(Energy))` — fungible store).
    pub capacity: u32,
}

impl ContainerDto {
    fn total_used(&self) -> u32 {
        self.store.iter().map(|(_, a)| a).sum()
    }

    fn energy(&self) -> u32 {
        self.store
            .iter()
            .find(|(r, _)| *r == ResourceType::Energy)
            .map(|(_, a)| *a)
            .unwrap_or(0)
    }
}

/// The room's storage as K1 sees it.
#[derive(Clone, Debug)]
pub struct StorageDto {
    pub item: ItemRef,
    pub store: Vec<(ResourceType, u32)>,
    /// `get_capacity(None)`.
    pub capacity: u32,
}

/// A ruin/tombstone as K1 sees it.
#[derive(Clone, Debug)]
pub struct LootDto {
    pub item: ItemRef,
    pub store: Vec<(ResourceType, u32)>,
}

/// A dropped resource pile as K1 sees it.
#[derive(Copy, Clone, Debug)]
pub struct DroppedDto {
    pub item: ItemRef,
    pub resource: ResourceType,
    pub amount: u32,
}

/// The K1 input: one room's haul-relevant structures (the `EconomyView` room slice for demand
/// registration). Vec orders are adapter-controlled and become the emission order.
#[derive(Clone, Debug, Default)]
pub struct RoomEconDto {
    pub spawns: Vec<RefillStructDto>,
    pub extensions: Vec<RefillStructDto>,
    pub containers: Vec<ContainerDto>,
    pub storage: Vec<StorageDto>,
    pub ruins: Vec<LootDto>,
    pub tombstones: Vec<LootDto>,
    pub dropped: Vec<DroppedDto>,
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Per-structure tier policies (each a pure fn; the fraction ladders in exact-integer form).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Provider-container withdraw tier: total fill > 75% → Medium, > 50% → Low, else None
/// (room_transfer.rs provider arm).
pub fn provider_container_withdraw_priority(total_used: u32, capacity: u32) -> TransferPriority {
    let (u, c) = (total_used as u64, capacity as u64);
    if u * 4 > c * 3 {
        TransferPriority::Medium
    } else if u * 2 > c {
        TransferPriority::Low
    } else {
        TransferPriority::None
    }
}

/// Controller-container deposit tier: energy fill < 75% → Low, else None (room_transfer.rs
/// controller arm — the Family-C diversion-bait sink).
pub fn controller_container_deposit_priority(energy_used: u32, energy_capacity: u32) -> TransferPriority {
    let (u, c) = (energy_used as u64, energy_capacity as u64);
    if u * 4 < c * 3 {
        TransferPriority::Low
    } else {
        TransferPriority::None
    }
}

/// Dropped-pile withdraw tier: amount > 500 or non-energy → High, else Medium.
pub fn dropped_resource_priority(resource: ResourceType, amount: u32) -> TransferPriority {
    if amount > 500 || resource != ResourceType::Energy {
        TransferPriority::High
    } else {
        TransferPriority::Medium
    }
}

/// Tombstone withdraw tier: amount > 200 or non-energy → High, else Medium.
pub fn tombstone_priority(resource: ResourceType, amount: u32) -> TransferPriority {
    if amount > 200 || resource != ResourceType::Energy {
        TransferPriority::High
    } else {
        TransferPriority::Medium
    }
}

/// Storage-link withdraw tier: energy fill > 50% → High, > 25% → Low, else None.
pub fn storage_link_withdraw_priority(used: u32, capacity: u32) -> TransferPriority {
    let (u, c) = (used as u64, capacity as u64);
    if u * 2 > c {
        TransferPriority::High
    } else if u * 4 > c {
        TransferPriority::Low
    } else {
        TransferPriority::None
    }
}

/// Source-link withdraw tier: energy fill > 50% → High, > 25% → Medium, else Low.
pub fn source_link_withdraw_priority(used: u32, capacity: u32) -> TransferPriority {
    let (u, c) = (used as u64, capacity as u64);
    if u * 2 > c {
        TransferPriority::High
    } else if u * 4 > c {
        TransferPriority::Medium
    } else {
        TransferPriority::Low
    }
}

/// The controller link's active-priority intake is gated to a horizon of this many ticks of
/// expected drain (see [`controller_link_deposit`]).
pub const CONTROLLER_LINK_BUFFER_TICKS: u32 = 30;

/// At or above this fraction of its (gated) buffer the controller link defers to storage
/// (advertises the remaining deficit at `None` instead of `Low`) — the RCL8 storage-link
/// starvation fix. Expressed as the exact rational 3/4 in [`controller_link_deposit`].
pub const CONTROLLER_LINK_DEFER_FILL: f32 = 0.75;

/// Pure decision for what (if anything) a controller link should advertise as a `Link`
/// deposit, given its energy `capacity`/`used`/`free` and the controller's expected per-tick
/// drain (`Some(rate)` at max RCL — buffer only `rate × CONTROLLER_LINK_BUFFER_TICKS`; `None`
/// below max — keep the whole link topped). Priority escalates as the buffer runs low and
/// de-escalates to `None` once mostly full. MOVED from room_transfer.rs.
pub fn controller_link_deposit(capacity: u32, used: u32, free: u32, expected_drain_per_tick: Option<u32>) -> Option<(TransferPriority, u32)> {
    let target_buffer = match expected_drain_per_tick {
        Some(drain) => drain.saturating_mul(CONTROLLER_LINK_BUFFER_TICKS).min(capacity),
        None => capacity,
    };

    let deficit = target_buffer.saturating_sub(used).min(free);

    if deficit == 0 {
        return None;
    }

    // fill_fraction = used / target_buffer (target_buffer == 0 reads as 1.0 — fully deferred).
    // Exact-integer thresholds: < 1/4 → High, < 1/2 → Medium, < 3/4 → Low, else None.
    let priority = if target_buffer == 0 {
        TransferPriority::None
    } else {
        let (u, t) = (used as u64, target_buffer as u64);
        if u * 4 < t {
            TransferPriority::High
        } else if u * 2 < t {
            TransferPriority::Medium
        } else if u * 4 < t * 3 {
            TransferPriority::Low
        } else {
            TransferPriority::None
        }
    };

    Some((priority, deficit))
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The K1 kernel: the room's haul-lane demand set.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The HAUL|USE demand set for a room — the live `transfer_request_haul_generator` policy
/// verbatim:
/// - spawns/extensions with free energy: deposit **High**, Haul;
/// - provider containers: per-resource withdraw at the total-fill ladder, Haul; NO deposit
///   (filled by miners' direct transfers, never through demand);
/// - controller containers: energy deposit at the fill ladder (< 75% → Low, else None), Haul;
///   energy withdraw at **None** on the **Use** lane (upgrader supply — invisible to haulers);
/// - other containers: accepts-any deposit at None + per-resource withdraw at None, Haul;
/// - storage: per-resource withdraw at None + accepts-any deposit at None, Haul;
/// - ruins: per-resource withdraw at **Medium**, Haul;
/// - tombstones: per-resource withdraw (amount > 200 or non-energy → High, else Medium), Haul;
/// - dropped: withdraw (amount > 500 or non-energy → High, else Medium), Haul.
pub fn room_haul_demand(room: &RoomEconDto) -> Vec<Demand> {
    let mut out = Vec::new();

    for spawn in &room.spawns {
        if spawn.free_energy > 0 {
            out.push(Demand {
                item: spawn.item,
                side: DemandSide::Deposit,
                resource: Some(ResourceType::Energy),
                priority: TransferPriority::High,
                amount: spawn.free_energy,
                transfer_type: TransferType::Haul,
            });
        }
    }

    for extension in &room.extensions {
        if extension.free_energy > 0 {
            out.push(Demand {
                item: extension.item,
                side: DemandSide::Deposit,
                resource: Some(ResourceType::Energy),
                priority: TransferPriority::High,
                amount: extension.free_energy,
                transfer_type: TransferType::Haul,
            });
        }
    }

    for container in &room.containers {
        match container.role {
            ContainerRole::Provider => {
                let total_used = container.total_used();
                if total_used > 0 {
                    let priority = provider_container_withdraw_priority(total_used, container.capacity);
                    for (resource, amount) in &container.store {
                        out.push(Demand {
                            item: container.item,
                            side: DemandSide::Withdraw,
                            resource: Some(*resource),
                            priority,
                            amount: *amount,
                            transfer_type: TransferType::Haul,
                        });
                    }
                }
            }
            ContainerRole::Controller => {
                let energy_used = container.energy();
                let free = container.capacity.saturating_sub(energy_used);
                if free > 0 {
                    out.push(Demand {
                        item: container.item,
                        side: DemandSide::Deposit,
                        resource: Some(ResourceType::Energy),
                        priority: controller_container_deposit_priority(energy_used, container.capacity),
                        amount: free,
                        transfer_type: TransferType::Haul,
                    });
                }
                if energy_used > 0 {
                    out.push(Demand {
                        item: container.item,
                        side: DemandSide::Withdraw,
                        resource: Some(ResourceType::Energy),
                        priority: TransferPriority::None,
                        amount: energy_used,
                        transfer_type: TransferType::Use,
                    });
                }
            }
            ContainerRole::Other => {
                let free = container.capacity.saturating_sub(container.total_used());
                if free > 0 {
                    out.push(Demand {
                        item: container.item,
                        side: DemandSide::Deposit,
                        resource: None,
                        priority: TransferPriority::None,
                        amount: free,
                        transfer_type: TransferType::Haul,
                    });
                }
                for (resource, amount) in &container.store {
                    out.push(Demand {
                        item: container.item,
                        side: DemandSide::Withdraw,
                        resource: Some(*resource),
                        priority: TransferPriority::None,
                        amount: *amount,
                        transfer_type: TransferType::Haul,
                    });
                }
            }
        }
    }

    for storage in &room.storage {
        let mut used_capacity = 0;
        for (resource, amount) in &storage.store {
            out.push(Demand {
                item: storage.item,
                side: DemandSide::Withdraw,
                resource: Some(*resource),
                priority: TransferPriority::None,
                amount: *amount,
                transfer_type: TransferType::Haul,
            });
            used_capacity += *amount;
        }
        let free_capacity = storage.capacity.saturating_sub(used_capacity);
        if free_capacity > 0 {
            out.push(Demand {
                item: storage.item,
                side: DemandSide::Deposit,
                resource: None,
                priority: TransferPriority::None,
                amount: free_capacity,
                transfer_type: TransferType::Haul,
            });
        }
    }

    for ruin in &room.ruins {
        for (resource, amount) in &ruin.store {
            out.push(Demand {
                item: ruin.item,
                side: DemandSide::Withdraw,
                resource: Some(*resource),
                priority: TransferPriority::Medium,
                amount: *amount,
                transfer_type: TransferType::Haul,
            });
        }
    }

    for tombstone in &room.tombstones {
        for (resource, amount) in &tombstone.store {
            out.push(Demand {
                item: tombstone.item,
                side: DemandSide::Withdraw,
                resource: Some(*resource),
                priority: tombstone_priority(*resource, *amount),
                amount: *amount,
                transfer_type: TransferType::Haul,
            });
        }
    }

    for dropped in &room.dropped {
        out.push(Demand {
            item: dropped.item,
            side: DemandSide::Withdraw,
            resource: Some(dropped.resource),
            priority: dropped_resource_priority(dropped.resource, dropped.amount),
            amount: dropped.amount,
            transfer_type: TransferType::Haul,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy_container(item: u32, role: ContainerRole, energy: u32, capacity: u32) -> ContainerDto {
        ContainerDto {
            item: ItemRef(item),
            role,
            store: if energy > 0 { vec![(ResourceType::Energy, energy)] } else { vec![] },
            capacity,
        }
    }

    /// Pre-move fixtures from room_transfer.rs's registration arms: the spawn/extension High
    /// deposits, the provider ladder, the controller container's dual registration (Low/None
    /// deposit + None-USE withdraw), storage's None pair, and the dropped tiers.
    #[test]
    fn room_demand_matches_live_registration_policy() {
        let room = RoomEconDto {
            spawns: vec![RefillStructDto { item: ItemRef(0), free_energy: 100 }, RefillStructDto { item: ItemRef(1), free_energy: 0 }],
            extensions: vec![RefillStructDto { item: ItemRef(2), free_energy: 50 }],
            containers: vec![
                energy_container(3, ContainerRole::Provider, 1600, 2000), // 80% → Medium
                energy_container(4, ContainerRole::Controller, 1400, 2000), // 70% < 75% → Low deposit
                energy_container(5, ContainerRole::Other, 500, 2000),
            ],
            storage: vec![StorageDto {
                item: ItemRef(6),
                store: vec![(ResourceType::Energy, 10_000)],
                capacity: 1_000_000,
            }],
            ruins: vec![],
            tombstones: vec![LootDto { item: ItemRef(7), store: vec![(ResourceType::Energy, 300)] }],
            dropped: vec![
                DroppedDto { item: ItemRef(8), resource: ResourceType::Energy, amount: 501 },
                DroppedDto { item: ItemRef(9), resource: ResourceType::Energy, amount: 500 },
                DroppedDto { item: ItemRef(10), resource: ResourceType::Utrium, amount: 10 },
            ],
        };
        let demands = room_haul_demand(&room);

        let find = |item: u32, side: DemandSide| -> Vec<&Demand> {
            demands.iter().filter(|d| d.item.0 == item && d.side == side).collect()
        };

        // Spawn with free capacity: High deposit; the full one: nothing.
        assert_eq!(find(0, DemandSide::Deposit)[0].priority, TransferPriority::High);
        assert_eq!(find(0, DemandSide::Deposit)[0].amount, 100);
        assert!(find(1, DemandSide::Deposit).is_empty());
        // Extension: High.
        assert_eq!(find(2, DemandSide::Deposit)[0].priority, TransferPriority::High);
        // Provider at 80%: Medium withdraw, NO deposit (miners fill it directly).
        assert_eq!(find(3, DemandSide::Withdraw)[0].priority, TransferPriority::Medium);
        assert!(find(3, DemandSide::Deposit).is_empty());
        // Controller container at 70%: Low deposit of the free 600 + None withdraw on USE.
        let ctl_dep = find(4, DemandSide::Deposit);
        assert_eq!((ctl_dep[0].priority, ctl_dep[0].amount), (TransferPriority::Low, 600));
        let ctl_wd = find(4, DemandSide::Withdraw);
        assert_eq!(ctl_wd[0].transfer_type, TransferType::Use, "upgrader supply — invisible to haulers");
        assert_eq!(ctl_wd[0].priority, TransferPriority::None);
        // Other container: accepts-any None deposit + None withdraw.
        assert_eq!(find(5, DemandSide::Deposit)[0].resource, None);
        assert_eq!(find(5, DemandSide::Withdraw)[0].priority, TransferPriority::None);
        // Storage: per-resource None withdraw + accepts-any None deposit of the remaining space.
        assert_eq!(find(6, DemandSide::Withdraw)[0].amount, 10_000);
        assert_eq!(find(6, DemandSide::Deposit)[0].amount, 990_000);
        // Tombstone at 300 energy: High (> 200).
        assert_eq!(find(7, DemandSide::Withdraw)[0].priority, TransferPriority::High);
        // Dropped: 501 → High, 500 → Medium, non-energy → High.
        assert_eq!(find(8, DemandSide::Withdraw)[0].priority, TransferPriority::High);
        assert_eq!(find(9, DemandSide::Withdraw)[0].priority, TransferPriority::Medium);
        assert_eq!(find(10, DemandSide::Withdraw)[0].priority, TransferPriority::High);
    }

    /// The provider/controller fraction ladders at their exact boundaries.
    #[test]
    fn container_ladders_match_live_thresholds() {
        // Provider: > 75% Medium, > 50% Low, else None (strict inequalities).
        assert_eq!(provider_container_withdraw_priority(1500, 2000), TransferPriority::Low, "exactly 75% is NOT > 75%");
        assert_eq!(provider_container_withdraw_priority(1501, 2000), TransferPriority::Medium);
        assert_eq!(provider_container_withdraw_priority(1000, 2000), TransferPriority::None, "exactly 50% is NOT > 50%");
        assert_eq!(provider_container_withdraw_priority(1001, 2000), TransferPriority::Low);
        // Controller deposit: < 75% Low, else None.
        assert_eq!(controller_container_deposit_priority(1499, 2000), TransferPriority::Low);
        assert_eq!(controller_container_deposit_priority(1500, 2000), TransferPriority::None, "exactly 75% is NOT < 75%");
    }

    // ── The controller-link deposit policy (tests MOVED from room_transfer.rs) ─────────────────

    /// At max RCL the controller drains only CONTROLLER_MAX_UPGRADE_PER_TICK (15) e/t, so the
    /// gated buffer is 15 × 30 = 450, below the 800 link capacity.
    const MAX_LEVEL_DRAIN: Option<u32> = Some(15);

    /// Pin (RCL8 storage-link starvation fix): once the controller link holds its gated buffer,
    /// it advertises NO active deposit.
    #[test]
    fn controller_link_full_gated_buffer_requests_nothing() {
        assert_eq!(controller_link_deposit(800, 450, 350, MAX_LEVEL_DRAIN), None);
        assert_eq!(controller_link_deposit(800, 600, 200, MAX_LEVEL_DRAIN), None);
    }

    /// Pin: below the buffer, the controller link tops up only its (small) gated deficit.
    #[test]
    fn controller_link_tops_up_only_the_gated_deficit() {
        // used 300 of a 450 buffer -> deficit 150, fill 0.667 -> Low.
        assert_eq!(controller_link_deposit(800, 300, 500, MAX_LEVEL_DRAIN), Some((TransferPriority::Low, 150)));
    }

    /// Pin (operator requirement): priority escalates as the buffer runs low.
    #[test]
    fn controller_link_escalates_priority_when_low() {
        assert_eq!(controller_link_deposit(800, 100, 700, MAX_LEVEL_DRAIN), Some((TransferPriority::High, 350)));
        assert_eq!(controller_link_deposit(800, 0, 800, MAX_LEVEL_DRAIN), Some((TransferPriority::High, 450)));
        assert_eq!(controller_link_deposit(800, 200, 600, MAX_LEVEL_DRAIN), Some((TransferPriority::Medium, 250)));
    }

    /// Pin: below max RCL (None drain) the link out-prioritizes storage until the defer
    /// threshold, escalating as it empties.
    #[test]
    fn controller_link_below_max_prioritizes_until_threshold() {
        assert_eq!(controller_link_deposit(800, 100, 700, None), Some((TransferPriority::High, 700)));
        assert_eq!(controller_link_deposit(800, 300, 500, None), Some((TransferPriority::Medium, 500)));
        assert_eq!(controller_link_deposit(800, 500, 300, None), Some((TransferPriority::Low, 300)));
    }

    /// Pin (operator requirement, RCL≤7): once mostly full the link defers to storage (None,
    /// not Low), and a completely full link requests nothing.
    #[test]
    fn controller_link_defers_to_storage_when_nearly_full() {
        assert_eq!(controller_link_deposit(800, 600, 200, None), Some((TransferPriority::None, 200)));
        assert_eq!(controller_link_deposit(800, 720, 80, None), Some((TransferPriority::None, 80)));
        assert_eq!(controller_link_deposit(800, 800, 0, None), None);
        assert_eq!(controller_link_deposit(800, 400, 400, MAX_LEVEL_DRAIN), Some((TransferPriority::None, 50)));
    }

    /// The link withdraw ladders at their boundaries.
    #[test]
    fn link_withdraw_ladders() {
        // Storage link: > 50% High, > 25% Low, else None (strict: exactly 50% falls to Low).
        assert_eq!(storage_link_withdraw_priority(400, 800), TransferPriority::Low, "exactly 50% is NOT > 50%");
        assert_eq!(storage_link_withdraw_priority(401, 800), TransferPriority::High);
        assert_eq!(storage_link_withdraw_priority(201, 800), TransferPriority::Low);
        assert_eq!(storage_link_withdraw_priority(200, 800), TransferPriority::None, "exactly 25% is NOT > 25%");
        // Source link: > 50% High, > 25% Medium, else Low.
        assert_eq!(source_link_withdraw_priority(401, 800), TransferPriority::High);
        assert_eq!(source_link_withdraw_priority(201, 800), TransferPriority::Medium);
        assert_eq!(source_link_withdraw_priority(200, 800), TransferPriority::Low);
    }
}
