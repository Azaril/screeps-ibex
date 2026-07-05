//! The **current-bot baseline policy** — the A-arm (ADR 0040 M1 spec Part C.3), re-plumbed at
//! M3 onto the REAL decision kernels: since ADR 0040 M3 the policy math lives in
//! `screeps-econ-decision` (the SAME crate the live bot ships — K1 demand, K2 snapshot
//! selection, K3 repair admission, K4 spawn policy), and this module keeps only the SIM-SIDE
//! ADAPTERS (EconWorld → DTO views, stable sim identities, booking subtraction) plus the few
//! transcriptions the extraction does not cover (job-FSM-shell kernels + the foreman build
//! priorities — each still citation-pinned). The M1 mirrors were DELETED (EP-2.6); their pinned
//! tests moved into the kernel crate.
//!
//! The DISEASE paths (§1 root causes S1/S3/S6) are therefore reproduced by the exact shipped
//! code: flat-ACTIVE nearest-wins carried-cargo delivery, ungated ≥Medium opportunistic repair
//! on the Pipeline-A work lane, and capacity-sized replacement bodies banking head-of-line.
//!
//! **Determinism deviations (uniform, documented once):** the live selection points iterate ECS
//! storages / HashMap room nodes (unordered) and break float ties by iteration order; the
//! kernels iterate a DETERMINISTIC candidate order (this adapter feeds spawns by index,
//! extensions by index, containers by tile, storage last) and compare exact integer rationals —
//! same policy, fence-safe arithmetic.

use crate::layout::{ContainerRole as LayoutContainerRole, LayoutInfo};
use screeps::Position;
use screeps_econ_decision::demand::{
    self as demand, ContainerDto, DemandSide, DroppedDto, ItemRef, RefillStructDto, RoomEconDto, StorageDto,
};
use screeps_econ_decision::snapshot as econ;
use screeps_econ_decision::spawn_policy;
use screeps_econ_engine::constants::SPAWN_ENERGY_CAPACITY;
use screeps_econ_engine::{EconWorld, SimResource, StructRef};
use std::collections::BTreeMap;

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The shared vocabulary — re-exported from the kernel crate (the M1 `Tier`/mask/`RepairPriority`
// mirrors are deleted; `Tier::NonePri` is now `Tier::None`).
// ═════════════════════════════════════════════════════════════════════════════════════════════

pub use screeps_econ_decision::priority::TransferPriority as Tier;
pub use screeps_econ_decision::priority::{TransferPriorityFlags, TransferType};
pub use screeps_econ_decision::repair::{map_high_value_priority, map_normal_priority, repair_energy_consumed, RepairPriority};
pub use screeps_econ_decision::stress::{
    effective_min_repair_priority, refill_deficit_q, repair_allowance, RepairAllowance, REPAIR_UNRESTRICTED_MAX_DEFICIT_Q,
    REPAIR_UNRESTRICTED_STORED_ENERGY,
};

/// The mask aliases the runner's as-hauler arms use (the live `TransferPriorityFlags` values).
pub const MASK_HIGH: TransferPriorityFlags = TransferPriorityFlags::HIGH;
pub const MASK_MEDIUM: TransferPriorityFlags = TransferPriorityFlags::MEDIUM;
pub const MASK_LOW: TransferPriorityFlags = TransferPriorityFlags::LOW;
pub const MASK_NONE: TransferPriorityFlags = TransferPriorityFlags::NONE;
pub const MASK_ACTIVE: TransferPriorityFlags = TransferPriorityFlags::ACTIVE;
pub const MASK_ALL: TransferPriorityFlags = TransferPriorityFlags::ALL;

/// The policy ARM configuration (the M4 tournament vocabulary):
/// - `Default` = the BASELINE arm (the live pre-S1 behavior — the disease);
/// - `s1_allowance` = the S1 stopgap arm (the allowance at every repair admission point);
/// - `tiered_delivery` (+ s1) = the PTRP arm: carried-cargo deliveries honor tiers
///   High→Medium→Low before the None fallback (the tier-faithful S3 fix — the ADR's
///   "tiers + gates" alternative, costed for the M4 report);
/// - `market` = the MARKET arms (ADR §D1/§D3 candidate kernels; `k4_bodies` splits the full
///   MARKET arm from MARKET-minus-K4 for S6 attribution).
#[derive(Clone, Copy, Debug, Default)]
pub struct PolicyConfig {
    pub s1_allowance: bool,
    pub tiered_delivery: bool,
    pub market: Option<crate::market::MarketArmCfg>,
}

impl PolicyConfig {
    pub fn baseline() -> Self {
        PolicyConfig::default()
    }

    pub fn s1() -> Self {
        PolicyConfig { s1_allowance: true, ..Default::default() }
    }

    pub fn ptrp() -> Self {
        PolicyConfig { s1_allowance: true, tiered_delivery: true, ..Default::default() }
    }

    pub fn market(cfg: crate::market::MarketArmCfg) -> Self {
        PolicyConfig { market: Some(cfg), ..Default::default() }
    }
}

/// The room's allowance under this config (Unrestricted when the arm is off — the flag-off
/// fail-open of energy_stress.rs). The allowance KERNEL is the live one
/// (`screeps_econ_decision::stress`); this adapter gathers the sim world's facts.
pub fn allowance_for(cfg: &PolicyConfig, world: &EconWorld) -> RepairAllowance {
    if !cfg.s1_allowance {
        return RepairAllowance::Unrestricted;
    }
    let capacity = spawn_lane_capacity(world);
    let stored = world.storage.as_ref().map(|s| s.store.amount(SimResource::Energy)).unwrap_or(0)
        + world
            .containers
            .iter()
            .map(|c| c.store.amount(SimResource::Energy))
            .sum::<u32>();
    repair_allowance(refill_deficit_q(world.room_spawn_energy(), capacity), stored)
}

/// `room.energy_capacity_available()`: spawns × 300 + Σ extension capacities.
pub fn spawn_lane_capacity(world: &EconWorld) -> u32 {
    world.spawns.len() as u32 * SPAWN_ENERGY_CAPACITY
        + world.extensions.iter().map(|e| e.capacity).sum::<u32>()
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K1 — the demand set (deposits + pickups): the sim adapter over the shared
// `demand::room_haul_demand` kernel. Stable sim identities + booking subtraction stay here.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// A deposit target's stable identity. Spawn/extension indices are stable (never removed);
/// containers are keyed by tile (they can die — the compaction contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SinkKey {
    Spawn(usize),
    Extension(usize),
    Container(u8, u8),
    Storage,
}

impl SinkKey {
    /// **Engine-fungible-pool membership** (M4 review #5): whether this sink is a member of a
    /// pool the ENGINE treats as ONE fungible reservoir — energy in any member is drawn from all
    /// members for the pool's function, so the market prices the ECONOMIC pool, not its plumbing,
    /// and the matcher aggregates the members into ONE demand node (see `market::market_pass`).
    ///
    /// The ONLY fungible pool is the spawn lane: `room.energy_available()` /
    /// `energy_capacity_available()` is `Σ(spawns + extensions)` and the head-of-line banker draws
    /// spawns-then-extensions-closest from that single total (engine `spawns/tick.js`,
    /// `spawnCreep`'s energy draw). Containers and storage are NOT fungible — each is a distinct
    /// stockpile with its own function (a controller container feeds ONE controller; storage is
    /// the numeraire depot) — so they are priced and matched per-structure. This method is the
    /// single predicate; a future fungible pool (e.g. a link network, if ever modeled) adds one
    /// arm here, and the matcher's aggregation follows automatically.
    pub fn is_fungible_pool_member(&self) -> bool {
        matches!(self, SinkKey::Spawn(_) | SinkKey::Extension(_))
    }
}

/// A withdraw/pickup source's stable identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SrcKey {
    Container(u8, u8),
    Storage,
    Dropped(u8, u8),
}

/// The live `TransferType` lane a request rides: every HAUL-side selection (K2, the hauling
/// stat) sees only `Haul`-lane requests — a `Use` registration (the controller container's
/// withdraw) is INVISIBLE to haulers; its consumers (upgraders pulling supply) see both lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Haul,
    Use,
}

fn lane_type(lane: Lane) -> TransferType {
    match lane {
        Lane::Haul => TransferType::Haul,
        Lane::Use => TransferType::Use,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Deposit {
    pub sink: SinkKey,
    pub pos: Position,
    pub tier: Tier,
    pub unfulfilled: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Pickup {
    pub src: SrcKey,
    pub pos: Position,
    pub tier: Tier,
    pub available: u32,
    /// The transfer-type lane ([`Lane`]) — haul selections filter to `Haul`.
    pub lane: Lane,
}

/// Per-tick bookings (the live pending-ticket registration: in-flight tasks re-register every
/// tick and reduce the unfulfilled amounts — the sim's booking table, ADR 0007 item 1's
/// adapter-side reservation layer).
#[derive(Clone, Debug, Default)]
pub struct Bookings {
    pub deposits: BTreeMap<SinkKey, u32>,
    pub pickups: BTreeMap<SrcKey, u32>,
}

/// The item table one K1 kernel call is built over: `ItemRef` index ↔ sim identity + position.
struct K1Item {
    sink: Option<SinkKey>,
    src: Option<SrcKey>,
    pos: Position,
}

/// Build the K1 [`RoomEconDto`] from the sim world (the EconWorld → DTO adapter; the emission
/// ORDER is the kernel's — spawns, extensions, containers in world order, storage, dropped —
/// which is exactly the documented sim candidate order).
fn k1_view(world: &EconWorld, info: &LayoutInfo) -> (RoomEconDto, Vec<K1Item>) {
    let mut items: Vec<K1Item> = Vec::new();
    let mut dto = RoomEconDto::default();

    fn push_item(items: &mut Vec<K1Item>, sink: Option<SinkKey>, src: Option<SrcKey>, pos: Position) -> ItemRef {
        items.push(K1Item { sink, src, pos });
        ItemRef(items.len() as u32 - 1)
    }

    for (i, s) in world.spawns.iter().enumerate() {
        dto.spawns.push(RefillStructDto {
            item: push_item(&mut items, Some(SinkKey::Spawn(i)), None, s.pos),
            free_energy: SPAWN_ENERGY_CAPACITY.saturating_sub(s.store_energy),
        });
    }
    for (i, e) in world.extensions.iter().enumerate() {
        dto.extensions.push(RefillStructDto {
            item: push_item(&mut items, Some(SinkKey::Extension(i)), None, e.pos),
            free_energy: e.capacity.saturating_sub(e.store_energy),
        });
    }
    for c in &world.containers {
        let tile = (c.pos.x().u8(), c.pos.y().u8());
        let role = match info.container_roles.get(&tile) {
            Some(LayoutContainerRole::Source) => demand::ContainerRole::Provider,
            Some(LayoutContainerRole::Controller) => demand::ContainerRole::Controller,
            _ => demand::ContainerRole::Other,
        };
        let energy = c.store.amount(SimResource::Energy);
        dto.containers.push(ContainerDto {
            item: push_item(
                &mut items,
                Some(SinkKey::Container(tile.0, tile.1)),
                Some(SrcKey::Container(tile.0, tile.1)),
                c.pos,
            ),
            role,
            store: if energy > 0 {
                vec![(screeps::ResourceType::Energy, energy)]
            } else {
                Vec::new()
            },
            capacity: c.store.capacity,
        });
    }
    if let Some(st) = &world.storage {
        let energy = st.store.amount(SimResource::Energy);
        dto.storage.push(StorageDto {
            item: push_item(&mut items, Some(SinkKey::Storage), Some(SrcKey::Storage), st.pos),
            store: if energy > 0 {
                vec![(screeps::ResourceType::Energy, energy)]
            } else {
                Vec::new()
            },
            capacity: st.store.capacity,
        });
    }
    for d in &world.dropped {
        if d.resource != SimResource::Energy || d.amount == 0 {
            continue;
        }
        let tile = (d.pos.x().u8(), d.pos.y().u8());
        dto.dropped.push(DroppedDto {
            item: push_item(&mut items, None, Some(SrcKey::Dropped(tile.0, tile.1)), d.pos),
            resource: screeps::ResourceType::Energy,
            amount: d.amount,
        });
    }

    (dto, items)
}

/// The deposit demand set: the K1 kernel's output filtered to the deposit side, with the sim's
/// booking subtraction (zero-remainder entries dropped, exactly the pre-M3 list shape).
pub fn deposits(world: &EconWorld, info: &LayoutInfo, bookings: &Bookings) -> Vec<Deposit> {
    let (dto, items) = k1_view(world, info);
    let mut out = Vec::new();
    for d in demand::room_haul_demand(&dto) {
        if d.side != DemandSide::Deposit {
            continue;
        }
        let item = &items[d.item.0 as usize];
        let sink = item.sink.expect("deposit demands map to sink items");
        let booked = bookings.deposits.get(&sink).copied().unwrap_or(0);
        let unfulfilled = d.amount.saturating_sub(booked);
        if unfulfilled > 0 {
            out.push(Deposit {
                sink,
                pos: item.pos,
                tier: d.priority,
                unfulfilled,
            });
        }
    }
    out
}

/// The withdraw/pickup set: the K1 kernel's output filtered to the withdraw side, with lane
/// preservation + booking subtraction.
pub fn pickups(world: &EconWorld, info: &LayoutInfo, bookings: &Bookings) -> Vec<Pickup> {
    let (dto, items) = k1_view(world, info);
    let mut out = Vec::new();
    for d in demand::room_haul_demand(&dto) {
        if d.side != DemandSide::Withdraw {
            continue;
        }
        let item = &items[d.item.0 as usize];
        let src = item.src.expect("withdraw demands map to source items");
        let booked = bookings.pickups.get(&src).copied().unwrap_or(0);
        let available = d.amount.saturating_sub(booked);
        if available > 0 {
            out.push(Pickup {
                src,
                pos: item.pos,
                tier: d.priority,
                available,
                lane: match d.transfer_type {
                    TransferType::Use => Lane::Use,
                    _ => Lane::Haul,
                },
            });
        }
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The matched-flow hauling statistic — the shared stage-math kernel over the sim's
// booking-subtracted demand lists (the M1-documented reduction: the live stat reads a 20-tick
// stale, registration-inflated stats cache; the sim recomputes per tick, unbooked).
// ═════════════════════════════════════════════════════════════════════════════════════════════

pub fn matched_unfulfilled_hauling(deposits: &[Deposit], pickups: &[Pickup]) -> u32 {
    let energy = screeps::ResourceType::Energy;
    let mut w = econ::StageSums::default();
    for p in pickups.iter().filter(|p| p.lane == Lane::Haul) {
        if p.tier != Tier::None {
            w.active += p.available;
        } else {
            w.inactive += p.available;
        }
    }
    let mut d = econ::StageSums::default();
    for dep in deposits {
        if dep.tier != Tier::None {
            d.active += dep.unfulfilled;
        } else {
            d.inactive += dep.unfulfilled;
        }
    }
    econ::matched_unfulfilled_resources(&[(energy, w)], &[(Some(energy), d)])
        .into_iter()
        .map(|(_, amount)| amount)
        .sum()
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K2 — task selection: sim adapters over the shared snapshot kernels. Each call builds a
// per-call view (pickup nodes first, then deposit nodes — the tie-break order equals the old
// pickup-outer/deposit-inner iteration) with EMPTY kernel bookings (the amounts are already
// booking-subtracted).
// ═════════════════════════════════════════════════════════════════════════════════════════════

fn haul_view(deposits: &[Deposit], pickups: &[Pickup]) -> (econ::TransferSnapshot, Vec<screeps::RoomName>) {
    let mut snapshot = econ::TransferSnapshot::new();
    let mut rooms: Vec<screeps::RoomName> = Vec::new();
    let note_room = |rooms: &mut Vec<screeps::RoomName>, room: screeps::RoomName| {
        if !rooms.contains(&room) {
            rooms.push(room);
        }
    };
    let energy = screeps::ResourceType::Energy;
    for p in pickups {
        let room = p.pos.room_name();
        note_room(&mut rooms, room);
        snapshot.add_node(
            room,
            p.pos,
            vec![(
                econ::WithdrawKey {
                    resource: energy,
                    priority: p.tier,
                    allowed_type: lane_type(p.lane),
                },
                p.available,
            )],
            vec![],
        );
    }
    for d in deposits {
        let room = d.pos.room_name();
        note_room(&mut rooms, room);
        snapshot.add_node(
            room,
            d.pos,
            vec![],
            vec![(
                econ::DepositKey {
                    resource: Some(energy),
                    priority: d.tier,
                    allowed_type: TransferType::Haul,
                },
                d.unfulfilled,
            )],
        );
    }
    (snapshot, rooms)
}

/// **The S3 disease, via the shared kernel:** carried-cargo delivery collects the flat-ACTIVE
/// deposits and takes the NEAREST by linear range — priority inside the mask is IGNORED
/// (`select_nearest_delivery`, the live `select_deliveries` + `find_nearest_linear_by`
/// composition).
pub fn select_delivery_flat_active(pos: Position, deposits: &[Deposit], held: u32) -> Option<(SinkKey, Position, u32)> {
    select_delivery_masked(pos, deposits, held, TransferPriorityFlags::ACTIVE)
}

/// The harvester Idle chain's TIERED delivery: per tier in order, nearest within each.
pub fn select_delivery_tiered(pos: Position, deposits: &[Deposit], held: u32, tiers: &[Tier]) -> Option<(SinkKey, Position, u32)> {
    for &tier in tiers {
        if let Some(result) = select_delivery_masked(pos, deposits, held, tier.into()) {
            return Some(result);
        }
    }
    None
}

fn select_delivery_masked(
    pos: Position,
    deposits: &[Deposit],
    held: u32,
    mask: TransferPriorityFlags,
) -> Option<(SinkKey, Position, u32)> {
    if held == 0 {
        return None;
    }
    let (snapshot, rooms) = haul_view(deposits, &[]);
    let bookings = econ::SnapshotBookings::new();
    let carried = vec![(screeps::ResourceType::Energy, held)];
    econ::select_nearest_delivery(
        &snapshot,
        &bookings,
        &rooms,
        mask,
        TransferType::Haul.into(),
        &carried,
        econ::TransferCapacity::Finite(held),
        pos,
        |_| true,
    )
    .map(|ticket| {
        let deposit = &deposits[ticket.node.0 as usize];
        (deposit.sink, deposit.pos, ticket.total_amount())
    })
}

/// **K2 pickup+delivery selection** (shared by the hauler and the harvester's two as-hauler
/// arms — the arms differ only in `allowed`): the live tier-interleave + value-density kernel
/// (`select_pickup_and_delivery`). `amount` = the pickup ticket total = min(pickup available,
/// delivery unfulfilled, capacity). Only Haul-lane pickups participate.
pub fn select_pickup_and_delivery(
    pos: Position,
    capacity: u32,
    deposits: &[Deposit],
    pickups: &[Pickup],
    allowed: TransferPriorityFlags,
) -> Option<(Pickup, Deposit, u32)> {
    if capacity == 0 {
        return None;
    }
    let (snapshot, rooms) = haul_view(deposits, pickups);
    let bookings = econ::SnapshotBookings::new();
    let creep = screeps_econ_decision::CreepEconDto {
        id: 0,
        pos,
        free_capacity: capacity,
        store: Vec::new(),
    };
    econ::select_pickup_and_delivery(
        &snapshot,
        &bookings,
        &creep,
        &rooms,
        &rooms,
        allowed,
        TransferType::Haul,
        econ::TransferCapacity::Finite(capacity),
        |_| true,
    )
    .map(|(pickup_ticket, deposit_ticket)| {
        let pickup = pickups[pickup_ticket.node.0 as usize];
        let deposit = deposits[deposit_ticket.node.0 as usize - pickups.len()];
        let amount = pickup_ticket.total_amount();
        (pickup, deposit, amount)
    })
}

/// The upgrader/builder FILL pickup: the live `select_pickups` + anchor filter + nearest
/// composition (`select_nearest_pickup`) over ALL priorities and BOTH lanes (the Use-lane
/// controller container IS visible here, unlike every haul selection).
pub fn select_fill_pickup(
    pos: Position,
    free: u32,
    pickups: &[Pickup],
    anchor: Option<(Position, u32)>,
) -> Option<(SrcKey, Position, u32)> {
    if free == 0 {
        return None;
    }
    let (snapshot, rooms) = haul_view(&[], pickups);
    let bookings = econ::SnapshotBookings::new();
    econ::select_nearest_pickup(
        &snapshot,
        &bookings,
        &rooms,
        TransferPriorityFlags::ALL,
        screeps_econ_decision::priority::TransferTypeFlags::HAUL | screeps_econ_decision::priority::TransferTypeFlags::USE,
        screeps::ResourceType::Energy,
        free,
        pos,
        anchor,
    )
    .map(|ticket| {
        let pickup = &pickups[ticket.node.0 as usize];
        (pickup.src, pickup.pos, ticket.total_amount())
    })
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K3 — repair admission: the sim world scan stays here (structures are sim state); the priority
// maps, the queue ORDERING, and the exact-split pricing are kernel imports.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// A repairable structure reference by stable identity (tile — roads/containers can die).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairRef {
    Road(u8, u8),
    Container(u8, u8),
}

/// Resolve a [`RepairRef`] to the CURRENT tick's StructRef index (None = it died).
pub fn resolve_repair_ref(world: &EconWorld, r: RepairRef) -> Option<StructRef> {
    match r {
        RepairRef::Road(x, y) => world
            .roads
            .iter()
            .position(|rd| rd.pos.x().u8() == x && rd.pos.y().u8() == y)
            .map(StructRef::Road),
        RepairRef::Container(x, y) => world
            .containers
            .iter()
            .position(|c| c.pos.x().u8() == x && c.pos.y().u8() == y)
            .map(StructRef::Container),
    }
}

/// Every repairable (roads + containers) with its live priority, deterministic order (roads
/// in construction order, then containers). Priorities via the kernel maps.
fn repair_candidates(world: &EconWorld) -> Vec<(RepairRef, Position, u32, u32, RepairPriority)> {
    let mut out = Vec::new();
    for r in &world.roads {
        if r.hits < r.hits_max {
            out.push((
                RepairRef::Road(r.pos.x().u8(), r.pos.y().u8()),
                r.pos,
                r.hits,
                r.hits_max,
                map_normal_priority(r.hits, r.hits_max),
            ));
        }
    }
    for c in &world.containers {
        if c.hits < screeps_econ_engine::constants::CONTAINER_HITS {
            out.push((
                RepairRef::Container(c.pos.x().u8(), c.pos.y().u8()),
                c.pos,
                c.hits,
                screeps_econ_engine::constants::CONTAINER_HITS,
                map_high_value_priority(c.hits, screeps_econ_engine::constants::CONTAINER_HITS),
            ));
        }
    }
    out
}

fn meets_min(priority: RepairPriority, min: Option<RepairPriority>) -> bool {
    min.map(|m| priority >= m).unwrap_or(true)
}

/// Chebyshev range (the live `get_range_to` on same-room positions).
fn range(a: Position, b: Position) -> u32 {
    a.get_range_to(b)
}

/// **Opportunistic (drive-by) repair target** — the live in-range queue read: candidates
/// within Chebyshev `range` of `pos` at ≥ `min` (None = no floor), max by the kernel's
/// `(priority, lowest hp fraction)` ordering. The caller applies the S1 allowance to `min`
/// first.
pub fn opportunistic_repair_target(world: &EconWorld, pos: Position, min: Option<RepairPriority>) -> Option<RepairRef> {
    repair_candidates(world)
        .into_iter()
        .filter(|(_, p, _, _, pr)| range(pos, *p) <= 3 && meets_min(*pr, min))
        .max_by(|a, b| screeps_econ_decision::repair::repair_target_order((a.4, a.2, a.3), (b.4, b.2, b.3)))
        .map(|(r, _, _, _, _)| r)
}

/// **Idle full-repair target** — the live room-wide queue read: ≥ `min`, max by the kernel
/// ordering.
pub fn full_repair_target(world: &EconWorld, min: Option<RepairPriority>) -> Option<(RepairRef, Position)> {
    repair_candidates(world)
        .into_iter()
        .filter(|(_, _, _, _, pr)| meets_min(*pr, min))
        .max_by(|a, b| screeps_econ_decision::repair::repair_target_order((a.4, a.2, a.3), (b.4, b.2, b.3)))
        .map(|(r, p, _, _, _)| (r, p))
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The job-FSM-shell kernels the M3 extraction does not cover (jobs/upgrade.rs +
// jobs/utility/controllerbehavior.rs) — still TRANSCRIBED, citation-pinned (they are creep-FSM
// decision arms, not K1-K4 economy policy; report: resisted extraction at M3).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// jobs/upgrade.rs:30-35 verbatim: a creep is SLOW with > 4 parts and MOVE × 4 < total parts
/// (the RCL > 3 upgrader body `[W,C,M,M] + N×[W]` trips this at 9+ parts).
pub fn is_slow_creep(body: &screeps_sim_core::SimBody) -> bool {
    let total = body.parts.len();
    let moves = body.parts.iter().filter(|p| p.part == screeps::Part::Move).count();
    total > 4 && moves * 4 < total
}

/// jobs/upgrade.rs:41 — how far from the CONTROLLER a slow upgrader will fetch energy.
pub const SLOW_UPGRADER_PICKUP_RANGE: u32 = 5;

/// jobs/upgrade.rs:49-57: the slow upgrader's pickup anchor is the CONTROLLER (never the creep —
/// the creep-anchored radius deadlock observed live).
pub fn upgrader_pickup_anchor(body: &screeps_sim_core::SimBody, controller_pos: Position) -> Option<(Position, u32)> {
    is_slow_creep(body).then_some((controller_pos, SLOW_UPGRADER_PICKUP_RANGE))
}

/// jobs/upgrade.rs:64-73: fast creeps always harvest; slow creeps only when the room has NO
/// storage and NO containers (downgrade emergencies / recovery).
pub fn upgrader_should_allow_harvest(body: &screeps_sim_core::SimBody, world: &EconWorld) -> bool {
    if !is_slow_creep(body) {
        return true;
    }
    world.storage.is_none() && world.containers.is_empty()
}

/// controllerbehavior.rs:52-66 verbatim: this tick's upgrade will exhaust the creep — issue the
/// refill withdraw THIS tick (pipeline D + E in parallel). Energy/free are start-of-tick.
pub fn upgrade_about_to_run_dry(work_parts: u32, energy: u32, free: u32) -> bool {
    if energy == 0 || free == 0 {
        return false;
    }
    let per_tick = work_parts.max(1); // × UPGRADE_CONTROLLER_POWER (1), floored ≥ 1 (:63)
    energy <= per_tick
}

/// missions/upgrade.rs `has_excess_energy` via the K4 kernel (adapter over sim world facts).
pub fn has_excess_energy(world: &EconWorld) -> bool {
    let storage_energy = world.storage.as_ref().map(|s| s.store.amount(SimResource::Energy)).unwrap_or(0);
    let container_energies: Vec<u32> = world.containers.iter().map(|c| c.store.amount(SimResource::Energy)).collect();
    spawn_policy::has_excess_energy(world.storage.is_some(), storage_energy, &container_energies)
}

/// missions/localbuild.rs `has_sufficient_energy` via the K4 kernel.
pub fn has_sufficient_energy(world: &EconWorld) -> bool {
    let storage_energies: Vec<u32> = world.storage.as_ref().map(|s| s.store.amount(SimResource::Energy)).into_iter().collect();
    let container_energies: Vec<u32> = world.containers.iter().map(|c| c.store.amount(SimResource::Energy)).collect();
    spawn_policy::has_sufficient_energy(world.storage.is_some(), &storage_energies, &container_energies)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The builder site-selection kernels (foreman `get_build_priority` + jobs/utility/build.rs) —
// TRANSCRIBED (the foreman planner's priority table is outside the K1-K4 extraction; report:
// resisted at M3).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// foreman `get_build_priority` (screeps-foreman/src/planner.rs:202-228), the in-vocabulary rows:
/// spawn/storage/tower Critical, extension Critical at RCL ≤ 2 else High, container High,
/// road VeryLow. (Ord: higher = build first.)
pub fn build_priority(kind: screeps_econ_engine::StructureKind, rcl: u8) -> u8 {
    use screeps_econ_engine::StructureKind::*;
    match kind {
        Spawn | Storage | Tower => 4, // Critical
        Extension => {
            if rcl <= 2 {
                4 // Critical (planner.rs:205-211)
            } else {
                3 // High
            }
        }
        Container => 3, // High
        Road => 0,      // VeryLow (planner.rs:225)
    }
}

/// jobs/utility/build.rs:5-19 — the builder's site selection: max by (foreman build priority,
/// then HIGHEST progress, then NEAREST — the reversed range compare inside max_by). Returns the
/// site's tile (sites compact; tile is the stable identity).
pub fn select_construction_site(pos: Position, world: &EconWorld, rcl: u8) -> Option<(u8, u8)> {
    world
        .sites
        .iter()
        .max_by(|a, b| {
            build_priority(a.kind, rcl)
                .cmp(&build_priority(b.kind, rcl))
                .then_with(|| a.progress.cmp(&b.progress))
                .then_with(|| range(pos, a.pos).cmp(&range(pos, b.pos)).reverse())
        })
        .map(|s| (s.pos.x().u8(), s.pos.y().u8()))
}

/// missions/localbuild.rs `get_builder_priority` via the K4 kernel tables (this adapter keeps
/// the sim's site enumeration).
pub fn builder_priority(world: &EconWorld, rcl: u8, sufficient: bool, builders: usize) -> Option<(u32, u32)> {
    if world.sites.is_empty() {
        return None;
    }
    let required_progress: u32 = world.sites.iter().map(|s| s.total - s.progress).sum();
    let desired_for_progress = spawn_policy::builder_desired_for_progress(rcl, required_progress);
    let desired = if sufficient { desired_for_progress } else { 1 };
    if desired == 0 {
        return None;
    }
    let priority = if builders == 0 {
        spawn_policy::FIRST_BUILDER_PRIORITY
    } else {
        let any_critical_kind = world.sites.iter().any(|s| {
            matches!(
                s.kind,
                screeps_econ_engine::StructureKind::Spawn | screeps_econ_engine::StructureKind::Storage
            )
        });
        spawn_policy::builder_priority_with_builders(any_critical_kind)
    };
    Some((desired, priority))
}

/// missions/localbuild.rs `get_repairer_priority` via the K4 kernel: the queue's best candidate
/// at the allowance-raised minimum decides (`effective_min_repair_priority(None, allowance)` —
/// Unrestricted → no floor at all).
pub fn repairer_priority(world: &EconWorld, allowance: RepairAllowance) -> Option<(u32, u32)> {
    let min = effective_min_repair_priority(None, allowance);
    let best = repair_candidates(world)
        .into_iter()
        .filter(|(_, _, _, _, pr)| meets_min(*pr, min))
        .max_by(|a, b| screeps_econ_decision::repair::repair_target_order((a.4, a.2, a.3), (b.4, b.2, b.3)))
        .map(|(_, _, _, _, pr)| pr)?;
    spawn_policy::repairer_spawn_priority(best)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K4 — spawn requests (rebuilt per tick, spawnsystem re-enqueue semantics): the roster/count
// orchestration stays sim-side (it mirrors the missions' ECS bookkeeping); bodies, sizing and
// priority bands are the shared `spawn_policy` kernels.
// ═════════════════════════════════════════════════════════════════════════════════════════════

pub use screeps_econ_decision::spawn_policy::{SPAWN_BID_CRITICAL, SPAWN_BID_HIGH, SPAWN_BID_LOW, SPAWN_BID_MEDIUM};

/// What a queued body is for — carried alongside the request so births map to roles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleSpec {
    Harvester { source_idx: usize },
    Hauler,
    Upgrader,
    /// `allow_harvest` is FROZEN at spawn-request time (localbuild.rs
    /// `room.storage().is_none()` captured into the job).
    Builder { allow_harvest: bool },
}

/// One K4 spawn request: the body + bid + role. The bid is MILLI-e/t (ADR 0040 §D2, M5b).
#[derive(Clone, Debug)]
pub struct SpawnPlan {
    pub body: Vec<screeps::Part>,
    pub priority: u32,
    pub role: RoleSpec,
}

/// The harvester body via the K4 kernel definition + the shared `create_body` expansion.
pub fn harvester_body(energy: u32) -> Option<Vec<screeps::Part>> {
    screeps_combat_decision::spawning::create_body(&spawn_policy::harvester_body(energy)).ok()
}

/// The LOCAL hauler body via the K4 kernel definition.
pub fn hauler_body(energy: u32) -> Option<Vec<screeps::Part>> {
    screeps_combat_decision::spawning::create_body(&spawn_policy::hauler_body(false, energy)).ok()
}

/// The upgrader body via the K4 kernel definition.
pub fn upgrader_body(rcl: u8, maximum_energy: u32, work_parts: Option<usize>) -> Option<Vec<screeps::Part>> {
    screeps_combat_decision::spawning::create_body(&spawn_policy::upgrader_body(rcl, maximum_energy, work_parts)).ok()
}

/// The builder body via the K4 kernel definition.
pub fn builder_body(maximum_energy: u32, priority: u32) -> Option<Vec<screeps::Part>> {
    screeps_combat_decision::spawning::create_body(&spawn_policy::builder_body(maximum_energy, priority)).ok()
}

/// **K4 — the per-tick spawn request set** (the S6 stall faithfully reproduced through the
/// kernel's `harvester_body_energy` capacity arm):
///
/// - **Harvesters** (source_mining.rs): per source, the kernel's desired count; the FIRST
///   harvester (no harvesting creeps anywhere) is sized from available-now (floored at 300),
///   every REPLACEMENT from capacity (S6); priority lerps CRITICAL→HIGH (the kernel's local
///   band).
///   *M1 reduction (documented):* static container miners are SKIPPED — the live no-container
///   branch (harvesters as the income engine) runs instead.
/// - **Haulers** (missions/haul.rs): body from available (floored) when none exist else
///   capacity; desired + priority via the kernel's `hauler_desired`/`hauler_priority` (local,
///   max_distance = 0).
///   *M1 reduction:* `unfulfilled_hauling` (a 20-tick-cached transfer-queue statistic live) is
///   the CURRENT unbooked ACTIVE match — same quantity, uncached (documented).
/// - **Upgraders** (missions/upgrade.rs): roster tracked incl. spawning; ALIVE = over 100 TTL
///   or still spawning; the roster cap / WORK sizing / priority bands via the kernel (the CPU
///   governor is assumed willing — sim reduction; no hostiles in-sim).
/// - **Builders** (missions/localbuild.rs): desired = max(builder table arm, repairer arm);
///   priority the max of the two arms; body via the kernel; `allow_harvest = storage.is_none()`
///   FROZEN into the role.
///
/// Emission order (harvesters, haulers, upgraders, builders) is the deterministic tie-break for
/// equal-priority requests through the queue kernel's stable sort — the live tie order is
/// mission-iteration-dependent (documented determinism stand-in).
pub fn spawn_requests(
    world: &EconWorld,
    roles: &BTreeMap<u32, RoleSpec>,
    unfulfilled_hauling: u32,
    allowance: RepairAllowance,
) -> Vec<SpawnPlan> {
    let mut out = Vec::new();
    let total_harvesting = roles.values().filter(|r| matches!(r, RoleSpec::Harvester { .. })).count();
    let capacity = spawn_lane_capacity(world);
    let available = world.room_spawn_energy();

    for source_idx in 0..world.sources.len() {
        let current = roles
            .values()
            .filter(|r| matches!(r, RoleSpec::Harvester { source_idx: s } if *s == source_idx))
            .count();
        let desired = spawn_policy::DESIRED_HARVESTERS_PER_SOURCE;
        if current < desired {
            let energy = spawn_policy::harvester_body_energy(total_harvesting, available, capacity);
            if let Some(body) = harvester_body(energy) {
                let priority = spawn_policy::harvester_priority(current, desired, 0);
                out.push(SpawnPlan {
                    body,
                    priority,
                    role: RoleSpec::Harvester { source_idx },
                });
            }
        }
    }

    let haulers = roles.values().filter(|r| matches!(r, RoleSpec::Hauler)).count();
    let energy = if haulers == 0 { available.max(SPAWN_ENERGY_CAPACITY) } else { capacity };
    if let Some(body) = hauler_body(energy) {
        let carry_parts = body.iter().filter(|p| **p == screeps::Part::Carry).count() as u32;
        let (desired_unfulfilled, desired) = spawn_policy::hauler_desired(unfulfilled_hauling, carry_parts, 0);
        if haulers < desired {
            let priority = spawn_policy::hauler_priority(haulers, desired_unfulfilled, 0);
            out.push(SpawnPlan {
                body,
                priority,
                role: RoleSpec::Hauler,
            });
        }
    }

    // ── Upgraders (missions/upgrade.rs; doc above) ──────────────────────────────────────────────
    let controller = world.controller.as_ref().filter(|c| c.level > 0);
    if let Some(c) = controller {
        let rcl = c.level;
        let excess = has_excess_energy(world);
        let at_max_level = screeps_econ_engine::constants::controller_levels(rcl).is_none();
        // Downgrade risk: clock below half of max.
        let max_ticks = screeps_econ_engine::constants::controller_downgrade(rcl);
        let downgrade_upkeep_parts: Option<usize> =
            (c.downgrade_ticks < max_ticks / 2).then(|| spawn_policy::work_parts_for_upkeep(c.downgrade_ticks, max_ticks));
        let downgrade_risk = downgrade_upkeep_parts.is_some();
        // Governor willing, no hostiles — sim reductions.
        let max_upgraders = spawn_policy::max_upgraders(true, false, at_max_level, excess, rcl);
        let roster: Vec<u32> = roles
            .iter()
            .filter(|(_, r)| matches!(r, RoleSpec::Upgrader))
            .map(|(&id, _)| id)
            .collect();
        // ALIVE = still spawning (no TTL entry yet) or > 100 ticks to live.
        let tick = world.tick();
        let alive = roster
            .iter()
            .filter(|id| world.creep_ttl.get(id).map(|&age| age.saturating_sub(tick) > 100).unwrap_or(true))
            .count();
        if alive < max_upgraders {
            let work_parts = spawn_policy::upgrader_work_parts(
                downgrade_upkeep_parts,
                roster.is_empty(),
                at_max_level,
                excess,
                world.sources.len(),
                max_upgraders,
            );
            let maximum_energy = if roster.is_empty() && downgrade_risk {
                available.max(SPAWN_ENERGY_CAPACITY)
            } else {
                capacity
            };
            if let Some(body) = upgrader_body(rcl, maximum_energy, work_parts) {
                let priority = spawn_policy::upgrader_priority(
                    downgrade_risk,
                    roster.is_empty(),
                    excess,
                    world.storage.is_some(),
                    max_upgraders,
                    alive,
                );
                out.push(SpawnPlan {
                    body,
                    priority,
                    role: RoleSpec::Upgrader,
                });
            }
        }
    }

    // ── Builders (missions/localbuild.rs; doc above) ────────────────────────────────────────────
    if let Some(c) = controller {
        let rcl = c.level;
        let sufficient = has_sufficient_energy(world);
        let builders = roles.values().filter(|r| matches!(r, RoleSpec::Builder { .. })).count();
        let mut spawn_count = 0u32;
        let mut spawn_priority = 0u32; // SPAWN_BID_NONE (milli-e/t)
        if let Some((desired, priority)) = builder_priority(world, rcl, sufficient, builders) {
            spawn_count = spawn_count.max(desired);
            spawn_priority = spawn_priority.max(priority);
        }
        if let Some((desired, priority)) = repairer_priority(world, allowance) {
            spawn_count = spawn_count.max(desired);
            spawn_priority = spawn_priority.max(priority);
        }
        if (builders as u32) < spawn_count {
            let use_energy_max = if builders == 0 && spawn_priority >= SPAWN_BID_HIGH {
                available.max(SPAWN_ENERGY_CAPACITY)
            } else {
                capacity
            };
            if let Some(body) = builder_body(use_energy_max, spawn_priority) {
                let allow_harvest = world.storage.is_none();
                out.push(SpawnPlan {
                    body,
                    priority: spawn_priority,
                    role: RoleSpec::Builder { allow_harvest },
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    fn dep(sink: SinkKey, p: Position, tier: Tier, amount: u32) -> Deposit {
        Deposit {
            sink,
            pos: p,
            tier,
            unfulfilled: amount,
        }
    }
    fn pick(src: SrcKey, p: Position, tier: Tier, amount: u32) -> Pickup {
        Pickup {
            src,
            pos: p,
            tier,
            available: amount,
            lane: Lane::Haul,
        }
    }

    // The map/allowance/interleave/value-density/Use-lane/matched-stat/repair-energy pins MOVED
    // with the kernels to `screeps-econ-decision` (ADR 0040 M3). The tests below pin the
    // SIM-SIDE ADAPTERS: identity mapping, booking subtraction, candidate order, and the
    // uncovered transcriptions.

    /// The adapter round-trip: the flat-ACTIVE carried-cargo delivery is nearest-wins and
    /// priority-blind (S3) — mapped back to sim identities.
    #[test]
    fn carried_cargo_delivery_is_nearest_wins_priority_blind() {
        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300),
            dep(SinkKey::Container(22, 25), pos(22, 25), Tier::Low, 2000),
            dep(SinkKey::Storage, pos(21, 25), Tier::None, 100_000),
        ];
        let (sink, _, amount) = select_delivery_flat_active(pos(20, 25), &deposits, 50).unwrap();
        assert_eq!(sink, SinkKey::Container(22, 25), "the NEAR Low sink wins over the far High — S3");
        assert_eq!(amount, 50);
        // The storage two tiles nearer never competes: None is outside the ACTIVE mask.
    }

    /// The adapter round-trip: interleave order + value density through the shared kernel,
    /// mapped back to (Pickup, Deposit, amount).
    #[test]
    fn interleave_serves_high_first_and_scores_by_value_density() {
        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300),
            dep(SinkKey::Container(40, 40), pos(40, 40), Tier::Low, 2000),
        ];
        let pickups = vec![pick(SrcKey::Storage, pos(20, 25), Tier::None, 50_000)];
        let (p, d, amount) = select_pickup_and_delivery(pos(25, 25), 200, &deposits, &pickups, MASK_ALL).unwrap();
        assert_eq!(p.src, SrcKey::Storage);
        assert_eq!(d.sink, SinkKey::Spawn(0), "High delivery served first (interleave #1)");
        assert_eq!(amount, 200, "clamped to capacity");

        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(40, 25), Tier::High, 300),
            dep(SinkKey::Extension(0), pos(24, 25), Tier::High, 200),
        ];
        let (_, d, _) = select_pickup_and_delivery(pos(25, 25), 400, &deposits, &pickups, MASK_ALL).unwrap();
        assert_eq!(d.sink, SinkKey::Extension(0), "value density picks the near refill");
    }

    /// The mask parameterization (the harvester as-hauler arms' seam): arm 1's HIGH|NONE mask
    /// cannot emit a Medium/Low combination; arm 2's MEDIUM|LOW|NONE mask cannot serve a High
    /// deposit.
    #[test]
    fn interleave_mask_restricts_the_selection() {
        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300),
            dep(SinkKey::Container(40, 40), pos(40, 40), Tier::Medium, 2000),
        ];
        let pickups = vec![pick(SrcKey::Storage, pos(20, 25), Tier::None, 50_000)];
        let (p, d, _) = select_pickup_and_delivery(pos(25, 25), 200, &deposits, &pickups, MASK_HIGH | MASK_NONE).unwrap();
        assert_eq!((p.src, d.sink), (SrcKey::Storage, SinkKey::Spawn(0)));
        let (_, d, _) =
            select_pickup_and_delivery(pos(25, 25), 200, &deposits, &pickups, MASK_MEDIUM | MASK_LOW | MASK_NONE).unwrap();
        assert_eq!(d.sink, SinkKey::Container(40, 40), "arm 2 never sees High demand");
    }

    /// The Use lane through the adapter: invisible to haul pairings and the hauling stat.
    #[test]
    fn use_lane_pickups_are_invisible_to_haul_selection() {
        let deposits = vec![dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300)];
        let use_only = vec![Pickup {
            src: SrcKey::Container(20, 25),
            pos: pos(20, 25),
            tier: Tier::None,
            available: 2000,
            lane: Lane::Use,
        }];
        assert!(
            select_pickup_and_delivery(pos(25, 25), 200, &deposits, &use_only, MASK_ALL).is_none(),
            "a Use-lane pickup never feeds a haul pairing"
        );
        assert_eq!(
            matched_unfulfilled_hauling(&deposits, &use_only),
            0,
            "…and never counts toward the hauling stat"
        );
    }

    /// The matched-flow stat through the shared stage kernel (adapter inputs).
    #[test]
    fn matched_unfulfilled_hauling_is_supply_bounded() {
        let d_active = dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300);
        let d_none = dep(SinkKey::Storage, pos(31, 25), Tier::None, 100_000);
        assert_eq!(matched_unfulfilled_hauling(&[d_active, d_none], &[]), 0);
        let w_small = pick(SrcKey::Dropped(10, 10), pos(10, 10), Tier::Medium, 40);
        assert_eq!(matched_unfulfilled_hauling(&[d_active], &[w_small]), 40);
        let w_none = pick(SrcKey::Storage, pos(31, 25), Tier::None, 5_000);
        assert_eq!(
            matched_unfulfilled_hauling(&[d_active], &[w_small, w_none]),
            300,
            "active 40 + inactive fills the remaining 260 of the active demand"
        );
        let w_big = pick(SrcKey::Dropped(11, 11), pos(11, 11), Tier::High, 1_000);
        assert_eq!(
            matched_unfulfilled_hauling(&[d_active, d_none], &[w_big, w_none]),
            300 + 700,
            "active demand 300 + the active-supply leftover 700 into the None deposit"
        );
    }

    /// The RepairQueue tie-break through the shared kernel ordering: equal priority resolves to
    /// the LOWEST hp fraction; priority outranks fraction.
    #[test]
    fn repair_tie_break_prefers_lowest_fraction() {
        let mut w = EconWorld::default();
        let a = w.add_road(pos(10, 10), 2000, 5000); // 40% — Medium band
        let b = w.add_road(pos(11, 10), 1500, 5000); // 30% — Medium band, more damaged
        let _ = (a, b);
        let got = opportunistic_repair_target(&w, pos(10, 11), Some(RepairPriority::Medium)).unwrap();
        assert_eq!(got, RepairRef::Road(11, 10), "the lower-fraction road wins the tie");
        let (got_full, _) = full_repair_target(&w, Some(RepairPriority::Medium)).unwrap();
        assert_eq!(got_full, RepairRef::Road(11, 10));
        let c = w.add_container(pos(12, 10), 2000, 150_000); // 60% — High band (high-value map)
        let _ = c;
        let got = opportunistic_repair_target(&w, pos(11, 11), Some(RepairPriority::Medium)).unwrap();
        assert_eq!(got, RepairRef::Container(12, 10), "priority outranks fraction");
    }

    /// K4 through the kernels: the first harvester is available-sized; replacements are
    /// capacity-sized (S6); priorities lerp CRITICAL→HIGH; the first hauler is available-sized.
    #[test]
    fn spawn_requests_reproduce_s6_and_the_bands() {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 10), 3000);
        w.add_spawn(pos(25, 25));
        for i in 0..10 {
            w.add_extension(pos(20 + i, 20), 3);
        }
        w.spawns[0].store_energy = 300;
        // capacity = 300 + 10×50 = 800; available = 300.
        let roles: BTreeMap<u32, RoleSpec> = BTreeMap::new();
        let reqs = spawn_requests(&w, &roles, 800, RepairAllowance::Unrestricted);
        let harv = reqs.iter().find(|r| matches!(r.role, RoleSpec::Harvester { .. })).unwrap();
        assert_eq!(harv.body.len(), 4, "bootstrap harvester: 1 repeat of [M,M,C,W] at 300 budget");
        assert_eq!(harv.priority, SPAWN_BID_CRITICAL, "first harvester is CRITICAL");

        // With one harvester alive, the replacement sizes from CAPACITY (800 → 3 repeats = 750).
        let mut roles = BTreeMap::new();
        roles.insert(1u32, RoleSpec::Harvester { source_idx: 0 });
        let reqs = spawn_requests(&w, &roles, 800, RepairAllowance::Unrestricted);
        let harv = reqs.iter().find(|r| matches!(r.role, RoleSpec::Harvester { .. })).unwrap();
        assert_eq!(harv.body.len(), 12, "replacement: 3 repeats — the S6 capacity body");
        assert_eq!(harv.priority, 93_750, "1/4 lerp toward HIGH (milli)");

        // Hauler demand: none alive, 800 unfulfilled → body from available (300 → 3×[C,M],
        // base 150) → desired = min(800/150, 3) = 3 > 0 at HIGH.
        let haul = reqs.iter().find(|r| matches!(r.role, RoleSpec::Hauler)).unwrap();
        assert_eq!(haul.body.len(), 6, "bootstrap hauler: 3 repeats of [C,M] at 300");
        assert_eq!(haul.priority, SPAWN_BID_HIGH);
    }

    // ── The uncovered transcriptions (pins stay sim-side) ──────────────────────────────────────

    /// jobs/upgrade.rs:30-35 — the slow-creep threshold per the live CODE
    /// (`total > 4 && moves × 4 < total`): a 2-MOVE `[W,C,M,M] + N×[W]` body turns slow at 9+
    /// parts (N ≥ 5). *The live doc comment claims "5+ parts" — it contradicts its own
    /// arithmetic; the transcription follows the code (the ground-truth convention).* The
    /// RCL ≤ 3 `[W,C,M,M] + N×[W,M]` shape is NEVER slow (moves scale with parts).
    #[test]
    fn slow_creep_matches_live_threshold() {
        use screeps::Part::*;
        use screeps_sim_core::SimBody;
        let rcl4_body = |extra_work: usize| {
            let mut b = vec![Work, Carry, Move, Move];
            b.extend(std::iter::repeat_n(Work, extra_work));
            SimBody::unboosted(&b)
        };
        assert!(!is_slow_creep(&rcl4_body(0)), "4 parts: never slow (> 4 required)");
        assert!(!is_slow_creep(&rcl4_body(4)), "8 parts, 2 MOVE: 8 < 8 false → still fast");
        assert!(is_slow_creep(&rcl4_body(5)), "9 parts, 2 MOVE: 8 < 9 → slow");
        let rcl3_body = SimBody::unboosted(&[Work, Carry, Move, Move, Work, Move, Work, Move]);
        assert!(!is_slow_creep(&rcl3_body), "the [W,M]-repeat body keeps MOVE×4 ≥ total");
    }

    /// The excess/sufficient adapters over the K4 kernel, including the bare-room split: NO
    /// storage and NO containers ⇒ has_excess TRUE but has_sufficient FALSE.
    #[test]
    fn excess_and_sufficient_energy_thresholds() {
        let w = EconWorld::default();
        assert!(has_excess_energy(&w), "bare room: excess TRUE (upgrade.rs:197-199)");
        assert!(!has_sufficient_energy(&w), "bare room: sufficient FALSE (any() over empty)");
        let mut w = EconWorld::default();
        w.set_storage(pos(10, 10), 1_000_000);
        w.storage.as_mut().unwrap().store.add(SimResource::Energy, 99_999);
        assert!(!has_excess_energy(&w));
        assert!(has_sufficient_energy(&w), "≥ 50k is sufficient");
        w.storage.as_mut().unwrap().store.add(SimResource::Energy, 1);
        assert!(has_excess_energy(&w), "≥ 100k is excess");
        let mut w = EconWorld::default();
        w.set_storage(pos(10, 10), 1_000_000);
        w.storage.as_mut().unwrap().store.add(SimResource::Energy, 49_999);
        assert!(!has_sufficient_energy(&w));
        let mut w = EconWorld::default();
        let c = w.add_container(pos(10, 10), 2000, 250_000);
        w.containers[c].store.add(SimResource::Energy, 1500);
        assert!(!has_excess_energy(&w), "exactly 75% is NOT > 75%");
        assert!(has_sufficient_energy(&w), "1500 > 50%");
        w.containers[c].store.add(SimResource::Energy, 1);
        assert!(has_excess_energy(&w));
    }

    /// controllerbehavior.rs:52-66 — the draining trigger: energy ≤ WORK × 1 with free space.
    #[test]
    fn upgrade_about_to_run_dry_boundaries() {
        assert!(!upgrade_about_to_run_dry(3, 0, 10), "empty takes the Err path instead");
        assert!(!upgrade_about_to_run_dry(3, 4, 0), "full creep has nothing to refill into");
        assert!(upgrade_about_to_run_dry(3, 3, 10), "energy == per-tick spend → refill now");
        assert!(!upgrade_about_to_run_dry(3, 4, 10), "one tick of slack left");
        assert!(upgrade_about_to_run_dry(0, 1, 10), "0 WORK floors per_tick at 1 (:63)");
    }

    /// The builder priority adapters + the repairer arm + the body cap through the kernels.
    #[test]
    fn builder_priority_and_body_match_live() {
        let mut w = EconWorld::default();
        assert!(builder_priority(&w, 3, true, 0).is_none(), "no sites → no builder demand");
        w.set_controller(pos(40, 40), 3);
        let s = w.add_construction_site(pos(10, 10), screeps_econ_engine::StructureKind::Extension).unwrap();
        // 3000 remaining at RCL ≤ 3 → 3 desired with sufficient energy, 1 without.
        assert_eq!(builder_priority(&w, 3, true, 0), Some((3, 62_500)), "(HIGH+MEDIUM)/2 with no builders");
        assert_eq!(builder_priority(&w, 3, false, 0).unwrap().0, 1, "insufficient energy → 1");
        assert_eq!(
            builder_priority(&w, 3, true, 1).unwrap().1,
            SPAWN_BID_MEDIUM,
            "extension sites → MEDIUM with a builder"
        );
        w.sites[s].progress = 2_500; // 500 remaining → 1 desired
        assert_eq!(builder_priority(&w, 3, true, 0).unwrap().0, 1);
        // A spawn site raises the with-builders priority to HIGH.
        w.add_construction_site(pos(11, 10), screeps_econ_engine::StructureKind::Spawn).unwrap();
        assert_eq!(builder_priority(&w, 3, true, 1).unwrap().1, SPAWN_BID_HIGH);

        // The repairer arm: a <25% road (High band) → (1, HIGH); allowance CriticalOnly hides it.
        let mut w = EconWorld::default();
        w.add_road(pos(10, 10), 1000, 5000);
        assert_eq!(repairer_priority(&w, RepairAllowance::Unrestricted), Some((1, SPAWN_BID_HIGH)));
        assert_eq!(repairer_priority(&w, RepairAllowance::CriticalOnly), None, "S1 gate: no repairer spawn");
        let mut w = EconWorld::default();
        w.add_road(pos(10, 10), 2000, 5000);
        assert_eq!(repairer_priority(&w, RepairAllowance::Unrestricted), Some((1, SPAWN_BID_MEDIUM)));
        let mut w = EconWorld::default();
        w.add_road(pos(10, 10), 4000, 5000);
        assert_eq!(repairer_priority(&w, RepairAllowance::Unrestricted), None, "VeryLow best never spawns");

        // The body cap: 5 [C,W,M,M] repeats below HIGH even with huge energy; uncapped at HIGH.
        let b = builder_body(10_000, SPAWN_BID_MEDIUM).unwrap();
        assert_eq!(b.len(), 20, "5 repeats × 4 parts (localbuild.rs Some(5))");
        let b = builder_body(10_000, SPAWN_BID_HIGH).unwrap();
        assert!(b.len() > 20, "≥ HIGH: uncapped repeats");
    }

    /// jobs/utility/build.rs:5-19 — site selection: foreman priority first (spawn beats road),
    /// then higher PROGRESS, then nearest.
    #[test]
    fn site_selection_matches_live_ordering() {
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        let road = w.add_construction_site(pos(11, 10), screeps_econ_engine::StructureKind::Road).unwrap();
        w.sites[road].progress = 299; // nearly done, but VeryLow priority
        w.add_construction_site(pos(30, 30), screeps_econ_engine::StructureKind::Spawn).unwrap();
        assert_eq!(
            select_construction_site(pos(10, 10), &w, 3),
            Some((30, 30)),
            "the far Critical spawn beats the nearly-done adjacent road"
        );
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        w.add_construction_site(pos(11, 10), screeps_econ_engine::StructureKind::Extension).unwrap();
        let far = w.add_construction_site(pos(30, 30), screeps_econ_engine::StructureKind::Extension).unwrap();
        w.sites[far].progress = 100;
        assert_eq!(select_construction_site(pos(10, 10), &w, 3), Some((30, 30)), "progress beats range");
        w.sites[far].progress = 0;
        assert_eq!(select_construction_site(pos(10, 10), &w, 3), Some((11, 10)));
    }

    /// The fill pickup adapter: nearest across ALL tiers AND both lanes (the Use-lane controller
    /// container IS visible), the slow-creep anchor filters to CONTROLLER range 5.
    #[test]
    fn fill_pickup_sees_use_lane_and_honors_anchor() {
        let use_pickup = Pickup {
            src: SrcKey::Container(20, 25),
            pos: pos(20, 25),
            tier: Tier::None,
            available: 500,
            lane: Lane::Use,
        };
        let haul_pickup = pick(SrcKey::Storage, pos(35, 25), Tier::None, 5_000);
        let set = vec![use_pickup, haul_pickup];
        let (src, _, take) = select_fill_pickup(pos(22, 25), 100, &set, None).unwrap();
        assert_eq!(src, SrcKey::Container(20, 25), "the NEAR Use-lane container wins for a filler");
        assert_eq!(take, 100, "min(free, available)");
        let anchored = select_fill_pickup(pos(34, 25), 100, &set, Some((pos(20, 25), 5)));
        assert_eq!(anchored.unwrap().0, SrcKey::Container(20, 25), "anchor keeps the controller-side source");
        assert!(select_fill_pickup(pos(22, 25), 0, &set, None).is_none());
    }

    /// The upgrader K4 arm end-to-end shapes through the kernels.
    #[test]
    fn upgrader_spawn_arm_downgrade_and_first_priorities() {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 10), 3000);
        w.add_spawn(pos(25, 25));
        w.set_controller(pos(40, 40), 3);
        w.controller.as_mut().unwrap().downgrade_ticks = 2_000; // 10% of 20k → risk
        w.spawns[0].store_energy = 0; // drained: available = 0 → the 300 floor
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::Unrestricted);
        let up = reqs.iter().find(|r| matches!(r.role, RoleSpec::Upgrader)).expect("upgrader requested");
        assert_eq!(up.priority, SPAWN_BID_CRITICAL, "downgrade risk + no upgraders → CRITICAL");
        assert_eq!(
            up.body,
            vec![screeps::Part::Work, screeps::Part::Carry, screeps::Part::Move, screeps::Part::Move],
            "upkeep w=1 at the 300 floor: the bare pre-body"
        );

        w.controller.as_mut().unwrap().downgrade_ticks = 20_000; // healthy clock
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::Unrestricted);
        let up = reqs.iter().find(|r| matches!(r.role, RoleSpec::Upgrader)).expect("upgrader requested");
        assert_eq!(up.priority, SPAWN_BID_HIGH, "no upgraders yet → HIGH");
    }

    /// The builder K4 arm: sites → a builder request at 62_500 milli (no builders), allow_harvest
    /// frozen TRUE without storage; a repair-only room under the S1 arm spawns NO repairer.
    #[test]
    fn builder_spawn_arm_and_s1_gating() {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 10), 3000);
        w.add_spawn(pos(25, 25));
        w.set_controller(pos(40, 40), 2);
        w.add_construction_site(pos(24, 24), screeps_econ_engine::StructureKind::Extension).unwrap();
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::Unrestricted);
        let b = reqs.iter().find(|r| matches!(r.role, RoleSpec::Builder { .. })).expect("builder requested");
        assert_eq!(b.priority, 62_500);
        assert!(matches!(b.role, RoleSpec::Builder { allow_harvest: true }), "no storage → harvest frozen ON");

        // Repair-bait-only room: baseline arm spawns the repairer-builder; the S1 arm does not.
        let mut w = EconWorld::default();
        w.add_source(pos(10, 10), 3000);
        w.add_spawn(pos(25, 25));
        w.set_controller(pos(40, 40), 2);
        w.add_road(pos(20, 20), 1000, 5000); // High band
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::Unrestricted);
        assert!(reqs.iter().any(|r| matches!(r.role, RoleSpec::Builder { .. })), "repairer arm fires");
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::CriticalOnly);
        assert!(!reqs.iter().any(|r| matches!(r.role, RoleSpec::Builder { .. })), "S1 gate blocks the repairer spawn");
    }

    /// The K1 adapter (deposits/pickups over `room_haul_demand`): identity mapping, tiers,
    /// lanes, and booking subtraction match the pre-M3 lists.
    #[test]
    fn k1_adapter_maps_demand_to_sim_identities() {
        let mut w = EconWorld::default();
        w.add_spawn(pos(25, 25));
        w.spawns[0].store_energy = 100; // free 200 → High deposit
        let ctl = w.add_container(pos(40, 40), 2000, 250_000);
        w.containers[ctl].store.add(SimResource::Energy, 1400); // 70% → Low deposit + Use withdraw
        let src = w.add_container(pos(10, 10), 2000, 250_000);
        w.containers[src].store.add(SimResource::Energy, 1700); // 85% → Medium withdraw, no deposit
        w.set_storage(pos(30, 25), 1_000_000);
        w.storage.as_mut().unwrap().store.add(SimResource::Energy, 5_000);
        w.drop_resource(pos(12, 12), SimResource::Energy, 600); // High pile

        let mut info = LayoutInfo {
            room: "W1N1".parse().unwrap(),
            controller_pos: pos(40, 40),
            container_roles: BTreeMap::new(),
            source_containers: BTreeMap::new(),
            plan_structures: Vec::new(),
            furniture_tiles: Vec::new(),
        };
        info.container_roles.insert((40, 40), LayoutContainerRole::Controller);
        info.container_roles.insert((10, 10), LayoutContainerRole::Source);

        let bookings = Bookings::default();
        let deps = deposits(&w, &info, &bookings);
        assert_eq!(deps[0].sink, SinkKey::Spawn(0));
        assert_eq!((deps[0].tier, deps[0].unfulfilled), (Tier::High, 200));
        assert_eq!(deps[1].sink, SinkKey::Container(40, 40));
        assert_eq!((deps[1].tier, deps[1].unfulfilled), (Tier::Low, 600), "controller container at 70% → Low");
        assert_eq!(deps[2].sink, SinkKey::Storage);
        assert_eq!(deps[2].tier, Tier::None);
        assert!(!deps.iter().any(|d| d.sink == SinkKey::Container(10, 10)), "source containers register NO deposit");

        let picks = pickups(&w, &info, &bookings);
        let ctl_pick = picks.iter().find(|p| p.src == SrcKey::Container(40, 40)).unwrap();
        assert_eq!((ctl_pick.tier, ctl_pick.lane), (Tier::None, Lane::Use), "controller withdraw rides the Use lane");
        let src_pick = picks.iter().find(|p| p.src == SrcKey::Container(10, 10)).unwrap();
        assert_eq!((src_pick.tier, src_pick.lane), (Tier::Medium, Lane::Haul), "85% provider → Medium");
        let drop_pick = picks.iter().find(|p| p.src == SrcKey::Dropped(12, 12)).unwrap();
        assert_eq!(drop_pick.tier, Tier::High, "600 > 500 → High");

        // Booking subtraction drops the remainder like the pre-M3 lists.
        let mut booked = Bookings::default();
        booked.deposits.insert(SinkKey::Spawn(0), 200);
        booked.pickups.insert(SrcKey::Dropped(12, 12), 100);
        let deps = deposits(&w, &info, &booked);
        assert!(!deps.iter().any(|d| d.sink == SinkKey::Spawn(0)), "fully-booked deposit vanishes");
        let picks = pickups(&w, &info, &booked);
        let drop_pick = picks.iter().find(|p| p.src == SrcKey::Dropped(12, 12)).unwrap();
        assert_eq!(drop_pick.available, 500, "booked 100 of 600");
    }
}
