//! The **TRANSCRIBED current-bot policy** — the A-arm (ADR 0040 M1 spec Part C.3). Every kernel
//! here is a verbatim transcription of a live decision path, each with its source file:line —
//! including the DISEASE paths (§1 root causes S1/S3/S6) reproduced faithfully: flat-ACTIVE
//! nearest-wins carried-cargo delivery, ungated ≥Medium opportunistic repair on the Pipeline-A
//! work lane, and capacity-sized replacement bodies banking head-of-line.
//!
//! **Determinism deviations (uniform, documented once):** the live selection points iterate ECS
//! storages / HashMap room nodes (unordered) and break float ties by iteration order; every
//! kernel here iterates a DETERMINISTIC candidate order (spawns by index, extensions by index,
//! containers by tile, storage last) and compares exact integers or exact rationals
//! (`a1·d2 vs a2·d1` instead of `f32` division) — same policy, fence-safe arithmetic.

use crate::layout::{ContainerRole, LayoutInfo};
use screeps::Position;
use screeps_econ_engine::constants::{REPAIR_HITS_PER_ENERGY, SPAWN_ENERGY_CAPACITY};
use screeps_econ_engine::{EconWorld, SimResource, StructRef};
use std::collections::BTreeMap;

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Priorities — the live 4-tier TransferPriority (transfer/transfersystem.rs:16-23,36-42) and the
// 5-tier RepairPriority (jobs/utility/repair.rs:7-13), mirrored as-is (the M5a market replaces
// them; the M1 baseline must speak them).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// `TransferPriority` mirror. Ordering: High > Medium > Low > None (the ACTIVE set excludes None
/// — transfersystem.rs:36,55).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    High,
    Medium,
    Low,
    NonePri,
}

/// `RepairPriority` mirror (repair.rs:7-13; Ord: Critical highest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepairPriority {
    VeryLow,
    Low,
    Medium,
    High,
    Critical,
}

/// Roads (and every structure without a special arm): <25% → High, <50% → Medium, <75% → Low,
/// else VeryLow — `map_normal_priority`, repair.rs:23-37 VERBATIM (float thresholds kept: the
/// comparison is a ratio against fixed quarters — exact in integer cross-multiplication).
pub fn map_normal_priority(hits: u32, hits_max: u32) -> RepairPriority {
    // hits/hits_max < k/4  ⟺  4·hits < k·hits_max (exact integers).
    let (h, m) = (hits as u64 * 4, hits_max as u64);
    if h < m {
        RepairPriority::High
    } else if h < 2 * m {
        RepairPriority::Medium
    } else if h < 3 * m {
        RepairPriority::Low
    } else {
        RepairPriority::VeryLow
    }
}

/// Containers (and spawns/towers): <50% → Critical, <75% → High, <95% → Low, else VeryLow —
/// `map_high_value_priority`, repair.rs:39-53; the container arm is repair.rs:103.
pub fn map_high_value_priority(hits: u32, hits_max: u32) -> RepairPriority {
    let (h, m) = (hits as u64 * 100, hits_max as u64);
    if h < 50 * m {
        RepairPriority::Critical
    } else if h < 75 * m {
        RepairPriority::High
    } else if h < 95 * m {
        RepairPriority::Low
    } else {
        RepairPriority::VeryLow
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The S1 allowance arm — TRANSCRIBED (not imported) from the live kernel, per the M1 spec's
// explicit call: the bot crate compiles host-side but a dependency would drag the whole ECS in;
// the 3-line kernel is mirrored instead, and THIS COMMENT is the declared mirror
// (screeps-ibex/src/energy_stress.rs:27-73 — constants + refill_deficit_q + repair_allowance +
// effective_min_repair_priority; divergence there must be re-mirrored here).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// energy_stress.rs:27 — stored energy at/above which repair is unrestricted.
pub const REPAIR_UNRESTRICTED_STORED_ENERGY: u32 = 10_000;
/// energy_stress.rs:31 — max per-mille refill deficit that stays unrestricted (100 = 10%).
pub const REPAIR_UNRESTRICTED_MAX_DEFICIT_Q: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairAllowance {
    Unrestricted,
    CriticalOnly,
}

/// energy_stress.rs:45-54 verbatim: per-mille refill deficit (0 when no capacity).
pub fn refill_deficit_q(energy_available: u32, energy_capacity: u32) -> u32 {
    if energy_capacity == 0 {
        return 0;
    }
    let available = energy_available.min(energy_capacity);
    let filled_q = ((available as u64 * 1000) / energy_capacity as u64) as u32;
    1000u32.saturating_sub(filled_q)
}

/// energy_stress.rs:57-63 verbatim: Unrestricted iff stored ≥ 10k or deficit ≤ 10%.
pub fn repair_allowance(deficit_q: u32, stored_energy: u32) -> RepairAllowance {
    if stored_energy >= REPAIR_UNRESTRICTED_STORED_ENERGY || deficit_q <= REPAIR_UNRESTRICTED_MAX_DEFICIT_Q {
        RepairAllowance::Unrestricted
    } else {
        RepairAllowance::CriticalOnly
    }
}

/// energy_stress.rs:68-73 verbatim: CriticalOnly raises the caller's minimum to Critical.
pub fn effective_min_repair_priority(min: RepairPriority, allowance: RepairAllowance) -> RepairPriority {
    match allowance {
        RepairAllowance::Unrestricted => min,
        RepairAllowance::CriticalOnly => RepairPriority::Critical,
    }
}

/// The policy toggle: `s1_allowance = false` is the BASELINE arm (the live pre-S1 behavior — the
/// disease); `true` applies the transcribed allowance at every repair admission point (the S1
/// stopgap arm the bench A/Bs — report-only in M1, the real A/B is M4).
#[derive(Clone, Copy, Debug, Default)]
pub struct PolicyConfig {
    pub s1_allowance: bool,
}

/// The room's allowance under this config (Unrestricted when the arm is off — the flag-off
/// fail-open of energy_stress.rs:79-88).
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
// K1 — the demand set (deposits + pickups), rebuilt per tick from world state exactly as the
// live `RoomTransferMission` re-requests per tick.
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

/// A withdraw/pickup source's stable identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SrcKey {
    Container(u8, u8),
    Storage,
    Dropped(u8, u8),
}

/// The live `TransferType` lane a request rides (transfersystem.rs `TransferType::Haul` vs
/// `Use`): every HAUL-side selection (K2, the hauling stat) sees only `Haul`-lane requests — a
/// `Use` registration (the controller container's withdraw, room_transfer.rs:369-380) is
/// INVISIBLE to haulers; its consumers (upgraders pulling supply) arrive with M2's controller
/// mechanics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Haul,
    Use,
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
/// tick and reduce the unfulfilled amounts — transfersystem's register_pickup/register_delivery).
#[derive(Clone, Debug, Default)]
pub struct Bookings {
    pub deposits: BTreeMap<SinkKey, u32>,
    pub pickups: BTreeMap<SrcKey, u32>,
}

/// The deposit demand set, in the deterministic candidate order (module docs):
/// - spawns with free capacity: **High** (room_transfer.rs:426-443);
/// - extensions with free capacity: **High** (room_transfer.rs:445-462);
/// - the controller container: fill < 75% → **Low**, else None (room_transfer.rs:342-367 — the
///   Family-C diversion-bait sink);
/// - other non-source containers (e.g. the mineral container): accepts-all **None**
///   (room_transfer.rs:394-408);
/// - storage: accepts-all **None** (room_transfer.rs:484-495);
/// - **source containers register NO deposit demand** — the live generic container arm filters
///   `sources_to_containers` OUT (room_transfer.rs:385-392); live source containers are filled
///   only by static miners' DIRECT transfers, never through demand. With M1's miners skipped
///   (workers.rs), source containers exist as structures (decay + repair-bait) and as
///   withdraw-side providers, but nothing fills them — exactly the transcribed no-container
///   harvester economy until M2's miner arm lands.
pub fn deposits(world: &EconWorld, info: &LayoutInfo, bookings: &Bookings) -> Vec<Deposit> {
    let mut out = Vec::new();
    let mut push = |sink: SinkKey, pos: Position, tier: Tier, free: u32| {
        let booked = bookings.deposits.get(&sink).copied().unwrap_or(0);
        let unfulfilled = free.saturating_sub(booked);
        if unfulfilled > 0 {
            out.push(Deposit { sink, pos, tier, unfulfilled });
        }
    };
    for (i, s) in world.spawns.iter().enumerate() {
        let free = SPAWN_ENERGY_CAPACITY.saturating_sub(s.store_energy);
        if free > 0 {
            push(SinkKey::Spawn(i), s.pos, Tier::High, free);
        }
    }
    for (i, e) in world.extensions.iter().enumerate() {
        let free = e.capacity.saturating_sub(e.store_energy);
        if free > 0 {
            push(SinkKey::Extension(i), e.pos, Tier::High, free);
        }
    }
    for c in &world.containers {
        let tile = (c.pos.x().u8(), c.pos.y().u8());
        let free = c.store.free();
        if free == 0 {
            continue;
        }
        match info.container_roles.get(&tile) {
            // Live registers no deposit demand for source containers (doc above).
            Some(ContainerRole::Source) => continue,
            Some(ContainerRole::Controller) => {
                // room_transfer.rs:352-356: fill fraction < 0.75 → Low, else None.
                let used = c.store.amount(SimResource::Energy) as u64;
                let cap = c.store.capacity as u64;
                let tier = if used * 100 < cap * 75 { Tier::Low } else { Tier::NonePri };
                push(SinkKey::Container(tile.0, tile.1), c.pos, tier, free);
            }
            _ => push(SinkKey::Container(tile.0, tile.1), c.pos, Tier::NonePri, free),
        }
    }
    if let Some(st) = &world.storage {
        push(SinkKey::Storage, st.pos, Tier::NonePri, st.store.free());
    }
    out
}

/// The withdraw/pickup set:
/// - source containers: fill > 75% → **Medium**, > 50% → **Low**, else None, Haul lane
///   (room_transfer.rs:309-336 — the provider-container tiering);
/// - the controller container: withdraw **None** on the **Use lane** (room_transfer.rs:369-380 —
///   `TransferType::Use`: upgrader supply, INVISIBLE to every haul selection; M2's upgraders are
///   its consumers);
/// - other containers: withdraw **None**, Haul (room_transfer.rs:410-421);
/// - storage: withdraw **None**, Haul (room_transfer.rs:469-482);
/// - dropped energy piles: amount > 500 → **High**, else **Medium**, Haul
///   (room_transfer.rs:671-684).
pub fn pickups(world: &EconWorld, info: &LayoutInfo, bookings: &Bookings) -> Vec<Pickup> {
    let mut out = Vec::new();
    let mut push = |src: SrcKey, pos: Position, tier: Tier, amount: u32, lane: Lane| {
        let booked = bookings.pickups.get(&src).copied().unwrap_or(0);
        let available = amount.saturating_sub(booked);
        if available > 0 {
            out.push(Pickup { src, pos, tier, available, lane });
        }
    };
    for c in &world.containers {
        let tile = (c.pos.x().u8(), c.pos.y().u8());
        let energy = c.store.amount(SimResource::Energy);
        if energy == 0 {
            continue;
        }
        let (tier, lane) = match info.container_roles.get(&tile) {
            Some(ContainerRole::Source) => {
                let (used, cap) = (c.store.total() as u64, c.store.capacity as u64);
                let tier = if used * 100 > cap * 75 {
                    Tier::Medium
                } else if used * 100 > cap * 50 {
                    Tier::Low
                } else {
                    Tier::NonePri
                };
                (tier, Lane::Haul)
            }
            Some(ContainerRole::Controller) => (Tier::NonePri, Lane::Use),
            _ => (Tier::NonePri, Lane::Haul),
        };
        push(SrcKey::Container(tile.0, tile.1), c.pos, tier, energy, lane);
    }
    if let Some(st) = &world.storage {
        let energy = st.store.amount(SimResource::Energy);
        if energy > 0 {
            push(SrcKey::Storage, st.pos, Tier::NonePri, energy, Lane::Haul);
        }
    }
    for d in &world.dropped {
        if d.resource != SimResource::Energy || d.amount == 0 {
            continue;
        }
        let tier = if d.amount > 500 { Tier::High } else { Tier::Medium };
        let tile = (d.pos.x().u8(), d.pos.y().u8());
        push(SrcKey::Dropped(tile.0, tile.1), d.pos, tier, d.amount, Lane::Haul);
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The matched-flow hauling statistic (finding: the live stat is a supply↔demand MIN-MATCH, not
// demand alone).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// `total_unfufilled_resources` for the single energy resource — the live 3-stage match
/// (transfersystem.rs:2249-2337), collapsed: withdraw supply and deposit demand are split
/// active (tier ≠ None) / inactive (None), then matched in stage order
/// (a) active↔active, (b) inactive-withdraw→active-deposit, (c) active-withdraw→inactive-deposit,
/// each consuming `min(remaining supply, remaining demand)`; the sum of consumes is the stat.
/// Only Haul-lane pickups count (live: `key.allowed_type == transfer_type` filters,
/// transfersystem.rs:2222/2237). The ONLY reduction vs live: the live mission reads this through
/// a 20-tick-stale cache (missions/haul.rs:193-196's `stats.access` window); the sim recomputes
/// per tick — same quantity, uncached.
pub fn matched_unfulfilled_hauling(deposits: &[Deposit], pickups: &[Pickup]) -> u32 {
    let (mut w_active, mut w_inactive) = (0u64, 0u64);
    for p in pickups.iter().filter(|p| p.lane == Lane::Haul) {
        if p.tier != Tier::NonePri {
            w_active += p.available as u64;
        } else {
            w_inactive += p.available as u64;
        }
    }
    let (mut d_active, mut d_inactive) = (0u64, 0u64);
    for d in deposits {
        if d.tier != Tier::NonePri {
            d_active += d.unfulfilled as u64;
        } else {
            d_inactive += d.unfulfilled as u64;
        }
    }
    // (a) Active ↔ Active (transfersystem.rs:2249-2264).
    let m1 = w_active.min(d_active);
    w_active -= m1;
    d_active -= m1;
    // (b) Inactive withdraw → Active deposit (:2279-2307).
    let m2 = w_inactive.min(d_active);
    // (c) Active withdraw → Inactive deposit (:2309-2335).
    let m3 = w_active.min(d_inactive);
    (m1 + m2 + m3).min(u32::MAX as u64) as u32
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K2 — task selection.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Chebyshev range (the live `get_range_to` on same-room positions).
fn range(a: Position, b: Position) -> u32 {
    a.get_range_to(b)
}

/// **The S3 disease, verbatim:** carried-cargo delivery collects High+Medium+Low deposits FLAT
/// (the ACTIVE mask — transfersystem.rs `select_deliveries`:1674-1710 called with
/// `TransferPriorityFlags::ACTIVE`) and takes the NEAREST by linear range
/// (haulbehavior.rs:154-175 `find_nearest_linear_by`) — priority inside the mask is IGNORED.
/// Tie-break: first in the deterministic candidate order (live: map iteration order — deviation
/// note in the module docs).
pub fn select_delivery_flat_active(pos: Position, deposits: &[Deposit], held: u32) -> Option<(SinkKey, Position, u32)> {
    deposits
        .iter()
        .filter(|d| d.tier != Tier::NonePri)
        .min_by_key(|d| range(pos, d.pos))
        .map(|d| (d.sink, d.pos, held.min(d.unfulfilled)))
}

/// The harvester Idle chain's TIERED delivery (harvest.rs:194-210: Medium, then Low, then None,
/// nearest within each tier via `get_new_delivery_current_resources_state`).
pub fn select_delivery_tiered(pos: Position, deposits: &[Deposit], held: u32, tiers: &[Tier]) -> Option<(SinkKey, Position, u32)> {
    for &tier in tiers {
        if let Some(d) = deposits.iter().filter(|d| d.tier == tier).min_by_key(|d| range(pos, d.pos)) {
            return Some((d.sink, d.pos, held.min(d.unfulfilled)));
        }
    }
    None
}

/// A set of [`Tier`]s as a bitmask — the `TransferPriorityFlags` mirror
/// (transfersystem.rs:44-57).
pub type TierMask = u8;
pub const MASK_HIGH: TierMask = 1;
pub const MASK_MEDIUM: TierMask = 2;
pub const MASK_LOW: TierMask = 4;
pub const MASK_NONE: TierMask = 8;
pub const MASK_ACTIVE: TierMask = MASK_HIGH | MASK_MEDIUM | MASK_LOW;
pub const MASK_ALL: TierMask = MASK_ACTIVE | MASK_NONE;

fn tier_bit(t: Tier) -> TierMask {
    match t {
        Tier::High => MASK_HIGH,
        Tier::Medium => MASK_MEDIUM,
        Tier::Low => MASK_LOW,
        Tier::NonePri => MASK_NONE,
    }
}

/// The pickup+delivery TIER-INTERLEAVE combinations for an allowed-priority mask —
/// `generate_active_priorities(allowed, allowed)` (utility.rs:34-98, seeded High/High starting in
/// the Delivery arm; called with the SAME mask on both sides by
/// `select_pickup_and_delivery`, transfersystem.rs:2169): per tier in High→Medium→Low→None order,
/// the delivery arm `(allowed, {tier})` then the pickup arm `({tier}, allowed)`; a named NONE
/// tier masks the opposite side to ACTIVE (utility.rs:49-53 — the null-loop guard). Tiers absent
/// from `allowed` emit nothing (the generator's `contains` skip).
fn interleave_combos(allowed: TierMask) -> Vec<(TierMask, TierMask)> {
    let mut out = Vec::new();
    for bit in [MASK_HIGH, MASK_MEDIUM, MASK_LOW, MASK_NONE] {
        if allowed & bit != 0 {
            let other = if bit == MASK_NONE { allowed & MASK_ACTIVE } else { allowed };
            out.push((other, bit)); // the Delivery arm (state seeds at Delivery — utility.rs:96)
            out.push((bit, other)); // then the same tier's Pickup arm
        }
    }
    out
}

/// **K2 pickup+delivery selection** (shared by the hauler and the harvester's two as-hauler arms
/// — the arms differ only in `allowed`): the first interleave combination with any (pickup,
/// delivery) pair wins; within it, the pair maximizing `amount / (d1 + d2)`
/// (transfersystem.rs:1855-1875: `finite_transfer_value(resources, pickup_length +
/// delivery_length)` with d1 = creep→pickup, d2 = pickup→delivery linear ranges, divisor clamped
/// ≥ 1 at :30-34) — compared as EXACT rationals (`a1·d2 ⋛ a2·d1`), ties to the deterministic
/// candidate order (module docs). `amount` = min(pickup available, delivery unfulfilled,
/// capacity). Only Haul-lane pickups participate ([`Lane`]).
pub fn select_pickup_and_delivery(
    pos: Position,
    capacity: u32,
    deposits: &[Deposit],
    pickups: &[Pickup],
    allowed: TierMask,
) -> Option<(Pickup, Deposit, u32)> {
    if capacity == 0 {
        return None;
    }
    for (pickup_tiers, delivery_tiers) in interleave_combos(allowed) {
        let mut best: Option<(usize, usize, u32, u64, u64)> = None; // (pi, di, amount, num=amount, den=d1+d2)
        for (pi, p) in pickups.iter().enumerate() {
            if p.lane != Lane::Haul || pickup_tiers & tier_bit(p.tier) == 0 {
                continue;
            }
            for (di, d) in deposits.iter().enumerate() {
                if delivery_tiers & tier_bit(d.tier) == 0 {
                    continue;
                }
                // A pickup and delivery on the same structure is a null trip (the live node model
                // can't produce one — a node's own demand nets out); skip.
                if same_structure(p.src, d.sink) {
                    continue;
                }
                let amount = p.available.min(d.unfulfilled).min(capacity);
                if amount == 0 {
                    continue;
                }
                let den = (range(pos, p.pos) + range(p.pos, d.pos)).max(1) as u64; // divisor ≥ 1 (:31)
                let num = amount as u64;
                let better = match best {
                    None => true,
                    // num/den > bnum/bden ⟺ num·bden > bnum·den — exact, no floats.
                    Some((_, _, _, bnum, bden)) => num * bden > bnum * den,
                };
                if better {
                    best = Some((pi, di, amount, num, den));
                }
            }
        }
        if let Some((pi, di, amount, _, _)) = best {
            return Some((pickups[pi], deposits[di], amount));
        }
    }
    None
}

fn same_structure(src: SrcKey, sink: SinkKey) -> bool {
    matches!(
        (src, sink),
        (SrcKey::Storage, SinkKey::Storage)
    ) || matches!((src, sink), (SrcKey::Container(x1, y1), SinkKey::Container(x2, y2)) if x1 == x2 && y1 == y2)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K3 — repair admission.
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

/// Every repairable (M1: roads + containers) with its live priority, deterministic order (roads
/// in construction order, then containers).
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

/// The live PRIMARY repair tie-break — the **RepairQueue's** `(priority, then LOWEST hp
/// fraction)` (repairqueue.rs:54-110 `get_best_target[_in_range]`; the queue is flooded per tick
/// by `LocalBuildMission`, localbuild.rs:186-222, so in an owned room the queue path is what
/// actually runs — the repair.rs:208-232/:162-191 room-scan arms are the dead fallback).
/// Fractions compare as EXACT rationals: on equal priority, `a` (hits_a/max_a) beats `b` iff
/// `hits_a · max_b < hits_b · max_a` (lower fraction = more damaged wins); exact-fraction ties
/// keep the LAST candidate in the deterministic order (the documented determinism stand-in for
/// live's unordered-scan last-max).
fn repair_queue_order(
    a: &(RepairRef, Position, u32, u32, RepairPriority),
    b: &(RepairRef, Position, u32, u32, RepairPriority),
) -> std::cmp::Ordering {
    a.4.cmp(&b.4).then_with(|| {
        let cross_a = a.2 as u64 * b.3 as u64; // hits_a · max_b
        let cross_b = b.2 as u64 * a.3 as u64; // hits_b · max_a
        cross_b.cmp(&cross_a) // lower fraction ranks GREATER (more damaged wins)
    })
}

/// **Opportunistic (drive-by) repair target** — the live in-range queue read
/// (`get_best_target_in_range`, repairqueue.rs:81-110 via repairbehavior.rs:206-213): candidates
/// within Chebyshev `range` of `pos` at ≥ `min`, max by [`repair_queue_order`]. Walls excluded
/// by construction (M1 models no walls). The caller applies the S1 allowance to `min` first
/// (repairbehavior.rs:196-201).
pub fn opportunistic_repair_target(world: &EconWorld, pos: Position, min: RepairPriority) -> Option<RepairRef> {
    repair_candidates(world)
        .into_iter()
        .filter(|(_, p, _, _, pr)| range(pos, *p) <= 3 && *pr >= min)
        .max_by(repair_queue_order)
        .map(|(r, _, _, _, _)| r)
}

/// **Idle full-repair target** — the live room-wide queue read (`get_best_target`,
/// repairqueue.rs:54-78 via repair.rs:168-171): ≥ `min`, max by [`repair_queue_order`].
pub fn full_repair_target(world: &EconWorld, min: RepairPriority) -> Option<(RepairRef, Position)> {
    repair_candidates(world)
        .into_iter()
        .filter(|(_, _, _, _, pr)| *pr >= min)
        .max_by(repair_queue_order)
        .map(|(r, p, _, _, _)| (r, p))
}

/// The exact repair energy a creep will spend this tick — `repair_energy_consumed`
/// (repairbehavior.rs, pinned by its tests: `min(work_parts, carried, ceil(missing /
/// REPAIR_POWER))`) — matches the resolver's `ceil(effect/100)` bit-for-bit, so a same-tick
/// Transfer+Repair pair can split the cargo exactly (the transfersystem.rs:1124-1134
/// `consume_resource_from_deposits` mechanic).
pub fn repair_energy_consumed(work_parts: u32, carried: u32, hits: u32, hits_max: u32) -> u32 {
    let missing = hits_max.saturating_sub(hits);
    work_parts.min(carried).min(missing.div_ceil(REPAIR_HITS_PER_ENERGY))
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// M2 — the upgrader kernels (transcribed from jobs/upgrade.rs + jobs/utility/controllerbehavior.rs
// + missions/upgrade.rs, each line cited).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// jobs/upgrade.rs:30-35 verbatim: a creep is SLOW with > 4 parts and MOVE × 4 < total parts
/// (the RCL > 3 upgrader body `[W,C,M,M] + N×[W]` trips this at 5+ parts).
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

/// The upgrader/builder FILL pickup (jobs/upgrade.rs:112-122 / jobs/build.rs:92-101):
/// `select_pickups` over ALL priorities and BOTH lanes (`TransferTypeFlags::HAUL | USE` —
/// upgrade.rs:117 / build.rs:97; the Use-lane controller container IS visible here, unlike every
/// haul selection), optionally anchor-filtered (haulbehavior.rs:105-112), then NEAREST by linear
/// range (haulbehavior.rs:114-117 `find_nearest_linear_by`). Amount = min(free, available) —
/// ties break to the deterministic candidate order (module docs).
pub fn select_fill_pickup(
    pos: Position,
    free: u32,
    pickups: &[Pickup],
    anchor: Option<(Position, u32)>,
) -> Option<(SrcKey, Position, u32)> {
    if free == 0 {
        return None;
    }
    pickups
        .iter()
        .filter(|p| match anchor {
            Some((a, r)) => a.get_range_to(p.pos) <= r, // within_anchor_range (:64-66)
            None => true,
        })
        .min_by_key(|p| range(pos, p.pos))
        .map(|p| (p.src, p.pos, free.min(p.available)))
}

/// missions/upgrade.rs:183-200 verbatim — `has_excess_energy`: storage present → Σ storage
/// energy ≥ `get_desired_storage_amount(Energy)` / 2 (200_000 / 2, missions/constants.rs:3-8);
/// else containers present → ANY container > 75% full; else TRUE (a bare room is "excess").
pub fn has_excess_energy(world: &EconWorld) -> bool {
    if world.storage.is_some() {
        let energy = world.storage.as_ref().map(|s| s.store.amount(SimResource::Energy)).unwrap_or(0);
        energy >= 200_000 / 2
    } else if !world.containers.is_empty() {
        world
            .containers
            .iter()
            .any(|c| c.store.amount(SimResource::Energy) as u64 * 100 > CONTAINER_CAPACITY_U64 * 75)
    } else {
        true
    }
}

const CONTAINER_CAPACITY_U64: u64 = screeps_econ_engine::constants::CONTAINER_CAPACITY as u64;

/// missions/upgrade.rs:93-130 TRANSCRIBED — the downgrade-upkeep body sizing: the minimum WORK
/// parts restoring the clock from `current_ttd` to `max_ticks / 2` within one lifetime (f64
/// arithmetic on integer inputs, exact in these ranges; the live function's floats kept — the
/// result is a body SIZE, never a per-tick branch).
pub fn work_parts_for_upkeep(current_ttd: u32, max_ticks: u32) -> usize {
    let safe_threshold = max_ticks / 2;
    if current_ttd >= safe_threshold {
        return 1;
    }
    let deficit = (safe_threshold - current_ttd) as f64;
    let net_restore = 100.0 - 1.0; // CONTROLLER_DOWNGRADE_RESTORE − the 1/tick decay (:99)
    for w in 1..=15u32 {
        // CONTROLLER_MAX_UPGRADE_PER_TICK (:104)
        let body_parts = w + 3; // [W,C,M,M] + (w−1)×[W] (:102-105)
        let spawn_ticks = body_parts * 3; // CREEP_SPAWN_TIME (:106)
        let lifetime = 1500u32.saturating_sub(spawn_ticks) as f64; // CREEP_LIFE_TIME (:107)
        let upgrade_ticks_per_cycle = (50.0 / w as f64).floor(); // CARRY_CAPACITY / W (:109-110)
        if upgrade_ticks_per_cycle < 1.0 {
            continue;
        }
        let cycle_ticks = upgrade_ticks_per_cycle; // refill rides along (parallel D+E, :114)
        let net_per_cycle = upgrade_ticks_per_cycle * net_restore;
        let cycles = (lifetime / cycle_ticks).floor();
        if cycles * net_per_cycle >= deficit {
            return w as usize;
        }
    }
    15 // the fallback cap (:129)
}

/// The upgrade body (missions/upgrade.rs:298-316): RCL ≤ 3 → pre `[W,C,M,M]`, repeat `[W,M]` ×
/// 0..=work_parts; RCL > 3 → pre `[W,C,M,M]`, repeat `[W]` × 1..=(work_parts − 1).
pub fn upgrader_body(rcl: u8, maximum_energy: u32, work_parts: Option<usize>) -> Option<Vec<screeps::Part>> {
    use screeps::Part::*;
    let def = if rcl <= 3 {
        screeps_combat_decision::spawning::SpawnBodyDefinition {
            maximum_energy,
            minimum_repeat: Some(0),
            maximum_repeat: work_parts,
            pre_body: &[Work, Carry, Move, Move],
            repeat_body: &[Work, Move],
            post_body: &[],
        }
    } else {
        screeps_combat_decision::spawning::SpawnBodyDefinition {
            maximum_energy,
            minimum_repeat: Some(1),
            maximum_repeat: work_parts.map(|p| p.saturating_sub(1)),
            pre_body: &[Work, Carry, Move, Move],
            repeat_body: &[Work],
            post_body: &[],
        }
    };
    screeps_combat_decision::spawning::create_body(&def).ok()
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// M2 — the builder kernels (transcribed from missions/localbuild.rs + jobs/build.rs +
// jobs/utility/build.rs + foreman's get_build_priority).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// missions/localbuild.rs:232-244 verbatim — `has_sufficient_energy`: storage present → ANY
/// storage ≥ `get_desired_storage_amount(Energy)` / 4 (50_000); else ANY container > 50% full
/// (an empty candidate set is false — the greenfield RCL-1 room reads insufficient).
pub fn has_sufficient_energy(world: &EconWorld) -> bool {
    if world.storage.is_some() {
        world
            .storage
            .as_ref()
            .map(|s| s.store.amount(SimResource::Energy) >= 200_000 / 4)
            .unwrap_or(false)
    } else {
        world
            .containers
            .iter()
            .any(|c| c.store.amount(SimResource::Energy) as u64 * 100 > CONTAINER_CAPACITY_U64 * 50)
    }
}

/// foreman `get_build_priority` (screeps-foreman/src/planner.rs:202-228), the in-vocabulary rows:
/// spawn/storage/tower Critical, extension Critical at RCL ≤ 2 else High, container High,
/// road VeryLow. (Ord: higher = build first.)
pub fn build_priority(kind: screeps_econ_engine::StructureKind, rcl: u8) -> u8 {
    use screeps_econ_engine::StructureKind::*;
    match kind {
        Spawn | Storage | Tower => 4,          // Critical
        Extension => {
            if rcl <= 2 {
                4 // Critical (planner.rs:205-211)
            } else {
                3 // High
            }
        }
        Container => 3,                        // High
        Road => 0,                             // VeryLow (planner.rs:225)
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

/// missions/localbuild.rs:49-111 — `get_builder_priority`: with sites pending, the desired
/// builder count from the required-progress table (by RCL band), collapsed to 1 without
/// sufficient energy; priority = (HIGH+MEDIUM)/2 when NO builders exist, else the max over site
/// kinds (spawn/storage → HIGH, else MEDIUM).
pub fn builder_priority(world: &EconWorld, rcl: u8, sufficient: bool, builders: usize) -> Option<(u32, f32)> {
    if world.sites.is_empty() {
        return None;
    }
    let required_progress: u32 = world.sites.iter().map(|s| s.total - s.progress).sum();
    let desired_for_progress: u32 = if rcl <= 3 {
        match required_progress {
            0 => 0,
            1..=1000 => 1,
            1001..=2000 => 2,
            2001..=3000 => 3,
            3001..=4000 => 4,
            _ => 5,
        }
    } else if rcl <= 6 {
        match required_progress {
            0 => 0,
            1..=2000 => 1,
            2001..=4000 => 2,
            4001..=6000 => 3,
            _ => 4,
        }
    } else {
        match required_progress {
            0 => 0,
            1..=3000 => 1,
            3001..=6000 => 2,
            6001..=9000 => 3,
            _ => 4,
        }
    };
    let desired = if sufficient { desired_for_progress } else { 1 }; // :87
    if desired == 0 {
        return None;
    }
    let priority = if builders == 0 {
        (SPAWN_PRIORITY_HIGH + SPAWN_PRIORITY_MEDIUM) / 2.0 // :90-91 = 62.5
    } else {
        // :93-101 — max over site kinds: Spawn/Storage → HIGH, everything else MEDIUM.
        let any_critical_kind = world.sites.iter().any(|s| {
            matches!(
                s.kind,
                screeps_econ_engine::StructureKind::Spawn | screeps_econ_engine::StructureKind::Storage
            )
        });
        if any_critical_kind {
            SPAWN_PRIORITY_HIGH
        } else {
            SPAWN_PRIORITY_MEDIUM
        }
    };
    Some((desired, priority))
}

/// missions/localbuild.rs:113-127 — `get_repairer_priority`: the queue's best candidate at the
/// allowance-raised minimum decides — ≥ High → (1, HIGH); ≥ Medium → (1, MEDIUM); else none.
/// Under `CriticalOnly` the minimum is Critical (`effective_min_repair_priority(None, allowance)`,
/// the Option-min live form: Unrestricted → no floor at all).
pub fn repairer_priority(world: &EconWorld, allowance: RepairAllowance) -> Option<(u32, f32)> {
    let min = match allowance {
        RepairAllowance::Unrestricted => RepairPriority::VeryLow, // None floor: every candidate
        RepairAllowance::CriticalOnly => RepairPriority::Critical,
    };
    let best = repair_candidates(world)
        .into_iter()
        .filter(|(_, _, _, _, pr)| *pr >= min)
        .max_by(repair_queue_order)
        .map(|(_, _, _, _, pr)| pr)?;
    if best >= RepairPriority::High {
        Some((1, SPAWN_PRIORITY_HIGH))
    } else if best >= RepairPriority::Medium {
        Some((1, SPAWN_PRIORITY_MEDIUM))
    } else {
        None
    }
}

/// The builder body (missions/localbuild.rs:262-277): repeat `[C,W,M,M]` × 1.., capped at 5
/// repeats below HIGH priority, uncapped at ≥ HIGH.
pub fn builder_body(maximum_energy: u32, priority: f32) -> Option<Vec<screeps::Part>> {
    use screeps::Part::*;
    screeps_combat_decision::spawning::create_body(&screeps_combat_decision::spawning::SpawnBodyDefinition {
        maximum_energy,
        minimum_repeat: Some(1),
        maximum_repeat: if priority >= SPAWN_PRIORITY_HIGH { None } else { Some(5) }, // :268
        pre_body: &[],
        repeat_body: &[Carry, Work, Move, Move],
        post_body: &[],
    })
    .ok()
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K4 — spawn requests (rebuilt per tick, spawnsystem re-enqueue semantics).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The spawn priority bands (spawnsystem.rs:22-39).
pub const SPAWN_PRIORITY_CRITICAL: f32 = 100.0;
pub const SPAWN_PRIORITY_HIGH: f32 = 75.0;
pub const SPAWN_PRIORITY_MEDIUM: f32 = 50.0;
pub const SPAWN_PRIORITY_LOW: f32 = 25.0;

/// What a queued body is for — carried alongside the request so births map to roles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleSpec {
    Harvester { source_idx: usize },
    Hauler,
    /// M2 — jobs/upgrade.rs.
    Upgrader,
    /// M2 — jobs/build.rs; `allow_harvest` is FROZEN at spawn-request time
    /// (localbuild.rs:280 `room.storage().is_none()` captured into the job).
    Builder { allow_harvest: bool },
}

/// One K4 spawn request: the body + priority + role.
#[derive(Clone, Debug)]
pub struct SpawnPlan {
    pub body: Vec<screeps::Part>,
    pub priority: f32,
    pub role: RoleSpec,
}

/// The harvester body — body_helpers.rs:88-97 verbatim ([M,M,C,W] × 1..=5 within `energy`),
/// expanded through the live `create_body` kernel (REUSED from screeps-combat-decision — the
/// same code the bot ships).
pub fn harvester_body(energy: u32) -> Option<Vec<screeps::Part>> {
    use screeps::Part::*;
    screeps_combat_decision::spawning::create_body(&screeps_combat_decision::spawning::SpawnBodyDefinition {
        maximum_energy: energy,
        minimum_repeat: Some(1),
        maximum_repeat: Some(5),
        pre_body: &[],
        repeat_body: &[Move, Move, Carry, Work],
        post_body: &[],
    })
    .ok()
}

/// The LOCAL hauler body — missions/haul.rs:254-263 verbatim ([C,M] × 1..=20 within `energy`).
pub fn hauler_body(energy: u32) -> Option<Vec<screeps::Part>> {
    use screeps::Part::*;
    screeps_combat_decision::spawning::create_body(&screeps_combat_decision::spawning::SpawnBodyDefinition {
        maximum_energy: energy,
        minimum_repeat: Some(1),
        maximum_repeat: Some(20),
        pre_body: &[],
        repeat_body: &[Carry, Move],
        post_body: &[],
    })
    .ok()
}

/// `lerp_bounded` (spawnsystem's priority lerp — "coarse ok" per the M1 spec).
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// **K4 — the per-tick spawn request set** (the S6 stall faithfully reproduced):
///
/// - **Harvesters** (source_mining.rs:388-421): per source, `desired_harvesters = 4` (:391);
///   the FIRST harvester (no harvesting creeps anywhere) is sized from
///   `energy_available().max(300)`, every REPLACEMENT from `energy_capacity_available()`
///   (:394-398 — S6: the capacity body head-of-line-banks trickle income); priority lerps
///   CRITICAL→HIGH by `current/desired` (:401-410).
///   *M1 reduction (documented):* static container miners are SKIPPED (spec 3b option) — the
///   live no-container branch (harvesters as the income engine) runs instead, because a skipped
///   miner would leave no income path; the miner+container loop is an M2+ refinement.
/// - **Haulers** (missions/haul.rs:229-291): body from `energy_available().max(300)` when none
///   exist else `energy_capacity_available()` (:229-237); `desired =
///   min(unfulfilled_hauling / (carry × 50), 3)` (:266-274 with max_haulers = 3 + 0 local);
///   priority HIGH below 75% of desired, else MEDIUM (:279-291 local arms).
///   *M1 reduction:* `unfulfilled_hauling` (a 20-tick-cached transfer-queue statistic live) is
///   the CURRENT unbooked ACTIVE deposit demand — same quantity, uncached (documented).
/// - **Upgraders (M2; missions/upgrade.rs:165-347):** roster tracked incl. spawning; ALIVE =
///   over 100 TTL or still spawning (:243-256); `max_upgraders` from hostiles/max-level/
///   has_excess (:227-241 — the CPU governor is assumed willing, documented sim reduction; no
///   hostiles in-sim); WORK sizing from the downgrade-upkeep kernel / the RCL8 cap split / 20
///   with excess / the source-potential half-share (:259-290); body from
///   `energy_available().max(300)` only for the FIRST upgrader under downgrade risk, else
///   CAPACITY (:292-296); priority CRITICAL/HIGH/lerp bands (:319-335).
/// - **Builders (M2; missions/localbuild.rs:224-292):** desired = max(builder table from pending
///   site progress ×has_sufficient gating, repairer arm from the queue's best candidate);
///   priority the max of the two arms; body `[C,W,M,M]` × 1.. capped at 5 repeats below HIGH
///   (:262-277); `allow_harvest = storage.is_none()` FROZEN into the role (:280).
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
        let desired = 4usize; // source_mining.rs:391
        if current < desired {
            let energy = if total_harvesting == 0 {
                available.max(SPAWN_ENERGY_CAPACITY) // source_mining.rs:395 — the bootstrap body
            } else {
                capacity // source_mining.rs:397 — the S6 capacity replacement
            };
            if let Some(body) = harvester_body(energy) {
                let interp = current as f32 / desired as f32;
                let priority = lerp(SPAWN_PRIORITY_CRITICAL, SPAWN_PRIORITY_HIGH, interp);
                out.push(SpawnPlan { body, priority, role: RoleSpec::Harvester { source_idx } });
            }
        }
    }

    let haulers = roles.values().filter(|r| matches!(r, RoleSpec::Hauler)).count();
    let energy = if haulers == 0 { available.max(SPAWN_ENERGY_CAPACITY) } else { capacity };
    if let Some(body) = hauler_body(energy) {
        let carry_parts = body.iter().filter(|p| **p == screeps::Part::Carry).count() as u32;
        let base_amount = (carry_parts * 50).max(1); // haul.rs:268-269, range_multiplier = 1 local
        let desired_unfulfilled = unfulfilled_hauling / base_amount; // haul.rs:273
        let desired = desired_unfulfilled.min(3) as usize; // haul.rs:271-274, max 3 local
        if haulers < desired {
            // haul.rs:279-291 (local arms): HIGH below 75% of the unfulfilled-desired, else MEDIUM.
            let priority = if (haulers as f32) < (desired_unfulfilled as f32 * 0.75).ceil() {
                SPAWN_PRIORITY_HIGH
            } else {
                SPAWN_PRIORITY_MEDIUM
            };
            out.push(SpawnPlan { body, priority, role: RoleSpec::Hauler });
        }
    }

    // ── Upgraders (missions/upgrade.rs:165-347; doc above) ──────────────────────────────────────
    let controller = world.controller.as_ref().filter(|c| c.level > 0);
    if let Some(c) = controller {
        let rcl = c.level;
        let excess = has_excess_energy(world);
        let at_max_level = screeps_econ_engine::constants::controller_levels(rcl).is_none(); // :224
        // Downgrade risk: clock below half of max (:209-220).
        let max_ticks = screeps_econ_engine::constants::controller_downgrade(rcl);
        let downgrade_upkeep_parts: Option<usize> = (c.downgrade_ticks < max_ticks / 2)
            .then(|| work_parts_for_upkeep(c.downgrade_ticks, max_ticks));
        let downgrade_risk = downgrade_upkeep_parts.is_some();
        // :227-241 (governor willing, no hostiles — sim reductions).
        let max_upgraders: usize = if at_max_level {
            1
        } else if excess {
            if rcl <= 3 {
                5
            } else {
                3
            }
        } else {
            1
        };
        let roster: Vec<u32> = roles
            .iter()
            .filter(|(_, r)| matches!(r, RoleSpec::Upgrader))
            .map(|(&id, _)| id)
            .collect();
        // ALIVE = still spawning (no TTL entry yet) or > 100 ticks to live (:243-256).
        let tick = world.tick();
        let alive = roster
            .iter()
            .filter(|id| world.creep_ttl.get(id).map(|&age| age.saturating_sub(tick) > 100).unwrap_or(true))
            .count();
        if alive < max_upgraders {
            let work_parts: Option<usize> = if let Some(upkeep) = downgrade_upkeep_parts {
                if roster.is_empty() {
                    Some(upkeep) // :259-263 — sized to save the clock in one lifetime
                } else {
                    Some(((15.0f32 / max_upgraders as f32).ceil()) as usize) // :264-269
                }
            } else if at_max_level {
                Some(((15.0f32 / max_upgraders as f32).ceil()) as usize) // :271-278
            } else if excess {
                Some(20) // :279-280
            } else {
                // :281-289 — half the room's source potential, split across upgraders.
                let energy_per_second = (3000.0f32 * world.sources.len() as f32) / 300.0;
                Some((((energy_per_second / 2.0) / max_upgraders as f32).floor().max(1.0)) as usize)
            };
            let maximum_energy = if roster.is_empty() && downgrade_risk {
                available.max(SPAWN_ENERGY_CAPACITY) // :292-294
            } else {
                capacity // :295
            };
            if let Some(body) = upgrader_body(rcl, maximum_energy, work_parts) {
                let priority = if downgrade_risk && roster.is_empty() {
                    SPAWN_PRIORITY_CRITICAL // :319-322
                } else if roster.is_empty() {
                    SPAWN_PRIORITY_HIGH // :323-324
                } else if excess && world.storage.is_some() && max_upgraders > 1 {
                    let interp = alive as f32 / (max_upgraders - 1) as f32; // :325-328
                    lerp(SPAWN_PRIORITY_HIGH, SPAWN_PRIORITY_MEDIUM, interp)
                } else if max_upgraders > 1 {
                    let interp = alive as f32 / (max_upgraders - 1) as f32; // :329-332
                    lerp(SPAWN_PRIORITY_MEDIUM, SPAWN_PRIORITY_LOW, interp)
                } else {
                    SPAWN_PRIORITY_MEDIUM // :333-334
                };
                out.push(SpawnPlan { body, priority, role: RoleSpec::Upgrader });
            }
        }
    }

    // ── Builders (missions/localbuild.rs:224-292; doc above) ────────────────────────────────────
    if let Some(c) = controller {
        let rcl = c.level;
        let sufficient = has_sufficient_energy(world);
        let builders = roles.values().filter(|r| matches!(r, RoleSpec::Builder { .. })).count();
        let mut spawn_count = 0u32;
        let mut spawn_priority = 0.0f32; // SPAWN_PRIORITY_NONE (:247)
        if let Some((desired, priority)) = builder_priority(world, rcl, sufficient, builders) {
            spawn_count = spawn_count.max(desired);
            spawn_priority = spawn_priority.max(priority);
        }
        if let Some((desired, priority)) = repairer_priority(world, allowance) {
            spawn_count = spawn_count.max(desired);
            spawn_priority = spawn_priority.max(priority);
        }
        if (builders as u32) < spawn_count {
            let use_energy_max = if builders == 0 && spawn_priority >= SPAWN_PRIORITY_HIGH {
                available.max(SPAWN_ENERGY_CAPACITY) // :262-264
            } else {
                capacity // :265
            };
            if let Some(body) = builder_body(use_energy_max, spawn_priority) {
                let allow_harvest = world.storage.is_none(); // :280
                out.push(SpawnPlan { body, priority: spawn_priority, role: RoleSpec::Builder { allow_harvest } });
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
        Deposit { sink, pos: p, tier, unfulfilled: amount }
    }
    fn pick(src: SrcKey, p: Position, tier: Tier, amount: u32) -> Pickup {
        Pickup { src, pos: p, tier, available: amount, lane: Lane::Haul }
    }

    /// repair.rs:23-37 — the road priority quarters, exact at the boundaries.
    #[test]
    fn road_priority_map_matches_live_thresholds() {
        assert_eq!(map_normal_priority(1249, 5000), RepairPriority::High);
        assert_eq!(map_normal_priority(1250, 5000), RepairPriority::Medium, "exactly 25% is NOT <25%");
        assert_eq!(map_normal_priority(2499, 5000), RepairPriority::Medium);
        assert_eq!(map_normal_priority(2500, 5000), RepairPriority::Low);
        assert_eq!(map_normal_priority(3750, 5000), RepairPriority::VeryLow);
    }

    /// repair.rs:39-53 — the container (high-value) map: a half-dead container is CRITICAL
    /// (passes even the S1 gate — the refuted-siege-suppression shape).
    #[test]
    fn container_priority_map_matches_live_thresholds() {
        assert_eq!(map_high_value_priority(124_999, 250_000), RepairPriority::Critical);
        assert_eq!(map_high_value_priority(125_000, 250_000), RepairPriority::High);
        assert_eq!(map_high_value_priority(187_500, 250_000), RepairPriority::Low);
        assert_eq!(map_high_value_priority(237_500, 250_000), RepairPriority::VeryLow);
    }

    /// The S1 allowance mirror agrees with energy_stress.rs's pinned boundaries.
    #[test]
    fn s1_allowance_mirror_boundaries() {
        assert_eq!(refill_deficit_q(0, 300), 1000);
        assert_eq!(refill_deficit_q(900, 1000), 100);
        assert_eq!(refill_deficit_q(0, 0), 0);
        assert_eq!(repair_allowance(1000, 10_000), RepairAllowance::Unrestricted, "10k stored overrides");
        assert_eq!(repair_allowance(101, 9_999), RepairAllowance::CriticalOnly);
        assert_eq!(repair_allowance(100, 0), RepairAllowance::Unrestricted, "exactly 10% deficit passes");
        assert_eq!(
            effective_min_repair_priority(RepairPriority::Medium, RepairAllowance::CriticalOnly),
            RepairPriority::Critical
        );
    }

    /// S3 verbatim: the flat-ACTIVE nearest ignores priority INSIDE the mask — a Low sink 2 tiles
    /// away beats a High sink 10 tiles away; None (storage) is never in the flat set.
    #[test]
    fn carried_cargo_delivery_is_nearest_wins_priority_blind() {
        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300),
            dep(SinkKey::Container(22, 25), pos(22, 25), Tier::Low, 2000),
            dep(SinkKey::Storage, pos(21, 25), Tier::NonePri, 100_000),
        ];
        let (sink, _, amount) = select_delivery_flat_active(pos(20, 25), &deposits, 50).unwrap();
        assert_eq!(sink, SinkKey::Container(22, 25), "the NEAR Low sink wins over the far High — S3");
        assert_eq!(amount, 50);
        // The storage two tiles nearer never competes: None is outside the ACTIVE mask.
    }

    /// The tier-interleave: (all-pickups, High-delivery) is combination #1 — a storage(None)
    /// pickup feeding a High spawn wins before any Medium/Low pairing is even considered; and the
    /// value score amount/(d1+d2) picks the bigger-closer pair, exact-rationally.
    #[test]
    fn interleave_serves_high_first_and_scores_by_value_density() {
        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300),
            dep(SinkKey::Container(40, 40), pos(40, 40), Tier::Low, 2000),
        ];
        let pickups = vec![pick(SrcKey::Storage, pos(20, 25), Tier::NonePri, 50_000)];
        let (p, d, amount) =
            select_pickup_and_delivery(pos(25, 25), 200, &deposits, &pickups, MASK_ALL).unwrap();
        assert_eq!(p.src, SrcKey::Storage);
        assert_eq!(d.sink, SinkKey::Spawn(0), "High delivery served first (interleave #1)");
        assert_eq!(amount, 200, "clamped to capacity");

        // Two High deliveries: amount/(d1+d2) decides — the far spawn (300 over d1+d2 = 5+20 →
        // 12 e/tile) loses to the near extension (200 over 5+4 → 22.2 e/tile); compared as exact
        // rationals (300·9 < 200·25), no floats.
        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(40, 25), Tier::High, 300),
            dep(SinkKey::Extension(0), pos(24, 25), Tier::High, 200),
        ];
        let (_, d, _) =
            select_pickup_and_delivery(pos(25, 25), 400, &deposits, &pickups, MASK_ALL).unwrap();
        assert_eq!(d.sink, SinkKey::Extension(0), "value density picks the near refill");
    }

    /// The mask parameterization (the harvester as-hauler arms' seam, transfersystem.rs:2169):
    /// arm 1's HIGH|NONE mask pairs a storage(None) pickup with a High spawn but can NEVER emit a
    /// Medium/Low combination; arm 2's MEDIUM|LOW|NONE mask cannot serve a High deposit.
    #[test]
    fn interleave_mask_restricts_the_generator() {
        // Arm-1 combos: (H|N→H), (H→H|N), (H→N), (N→H) — no M/L anywhere.
        let combos = interleave_combos(MASK_HIGH | MASK_NONE);
        assert_eq!(
            combos,
            vec![
                (MASK_HIGH | MASK_NONE, MASK_HIGH),
                (MASK_HIGH, MASK_HIGH | MASK_NONE),
                (MASK_HIGH, MASK_NONE), // None arm: opposite side masked to allowed ∩ ACTIVE
                (MASK_NONE, MASK_HIGH),
            ]
        );

        let deposits = vec![
            dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300),
            dep(SinkKey::Container(40, 40), pos(40, 40), Tier::Medium, 2000),
        ];
        let pickups = vec![pick(SrcKey::Storage, pos(20, 25), Tier::NonePri, 50_000)];
        // Arm 1 (harvest.rs:115): storage → spawn matches.
        let (p, d, _) =
            select_pickup_and_delivery(pos(25, 25), 200, &deposits, &pickups, MASK_HIGH | MASK_NONE)
                .unwrap();
        assert_eq!((p.src, d.sink), (SrcKey::Storage, SinkKey::Spawn(0)));
        // Arm 2 (harvest.rs:149): the High spawn is invisible; the Medium container is served.
        let (_, d, _) = select_pickup_and_delivery(
            pos(25, 25),
            200,
            &deposits,
            &pickups,
            MASK_MEDIUM | MASK_LOW | MASK_NONE,
        )
        .unwrap();
        assert_eq!(d.sink, SinkKey::Container(40, 40), "arm 2 never sees High demand");
    }

    /// The Use lane (room_transfer.rs:369-380): a controller-container withdraw is
    /// `TransferType::Use` — INVISIBLE to every haul selection even when it is the only supply.
    #[test]
    fn use_lane_pickups_are_invisible_to_haul_selection() {
        let deposits = vec![dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300)];
        let use_only = vec![Pickup {
            src: SrcKey::Container(20, 25),
            pos: pos(20, 25),
            tier: Tier::NonePri,
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

    /// The matched-flow hauling stat (transfersystem.rs:2249-2337 collapsed to energy): stage
    /// order (a) active↔active, (b) inactive→active, (c) active→inactive; a drained world (zero
    /// pickups) matches NOTHING — live spawns no hauler for unhaulable demand.
    #[test]
    fn matched_unfulfilled_hauling_is_supply_bounded() {
        let d_active = dep(SinkKey::Spawn(0), pos(30, 25), Tier::High, 300);
        let d_none = dep(SinkKey::Storage, pos(31, 25), Tier::NonePri, 100_000);
        // Drained world: demand exists, zero supply ⇒ 0 (the S0=0 bootstrap window).
        assert_eq!(matched_unfulfilled_hauling(&[d_active, d_none], &[]), 0);
        // Supply-bounded: 40 active supply against 300 active demand ⇒ 40, not 300.
        let w_small = pick(SrcKey::Dropped(10, 10), pos(10, 10), Tier::Medium, 40);
        assert_eq!(matched_unfulfilled_hauling(&[d_active], &[w_small]), 40);
        // Stage (b): inactive (None storage) supply serves the ACTIVE deposit remainder…
        let w_none = pick(SrcKey::Storage, pos(31, 25), Tier::NonePri, 5_000);
        assert_eq!(
            matched_unfulfilled_hauling(&[d_active], &[w_small, w_none]),
            300,
            "active 40 + inactive fills the remaining 260 of the active demand"
        );
        // Stage (c): leftover ACTIVE supply flows to the inactive (storage) deposit; the
        // inactive→inactive pairing does NOT exist (no stage for it — storage never shuttles to
        // itself through the stat).
        let w_big = pick(SrcKey::Dropped(11, 11), pos(11, 11), Tier::High, 1_000);
        assert_eq!(
            matched_unfulfilled_hauling(&[d_active, d_none], &[w_big, w_none]),
            300 + 700,
            "active demand 300 + the active-supply leftover 700 into the None deposit"
        );
    }

    /// The RepairQueue tie-break (repairqueue.rs:54-110): equal priority resolves to the LOWEST
    /// hp fraction — two same-band roads in range, the more-damaged one wins (exact rationals).
    #[test]
    fn repair_tie_break_prefers_lowest_fraction() {
        let mut w = EconWorld::default();
        let a = w.add_road(pos(10, 10), 2000, 5000); // 40% — Medium band
        let b = w.add_road(pos(11, 10), 1500, 5000); // 30% — Medium band, more damaged
        let _ = (a, b);
        let got = opportunistic_repair_target(&w, pos(10, 11), RepairPriority::Medium).unwrap();
        assert_eq!(got, RepairRef::Road(11, 10), "the lower-fraction road wins the tie");
        let (got_full, _) = full_repair_target(&w, RepairPriority::Medium).unwrap();
        assert_eq!(got_full, RepairRef::Road(11, 10));
        // Priority still dominates fraction: a High-band road (<25%) beats a lower-fraction…
        // wait — lower fraction implies higher band at the boundary; use a container: 60% of
        // 250k (High band, fraction 0.6) vs the 30% road (Medium band): High wins.
        let c = w.add_container(pos(12, 10), 2000, 150_000); // 60% — High band (high-value map)
        let _ = c;
        let got = opportunistic_repair_target(&w, pos(11, 11), RepairPriority::Medium).unwrap();
        assert_eq!(got, RepairRef::Container(12, 10), "priority outranks fraction");
    }

    /// K4: the first harvester is available-sized (250 at 300 budget); replacements are
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
        assert_eq!(harv.priority, SPAWN_PRIORITY_CRITICAL, "first harvester is CRITICAL");

        // With one harvester alive, the replacement sizes from CAPACITY (800 → 3 repeats = 750).
        let mut roles = BTreeMap::new();
        roles.insert(1u32, RoleSpec::Harvester { source_idx: 0 });
        let reqs = spawn_requests(&w, &roles, 800, RepairAllowance::Unrestricted);
        let harv = reqs.iter().find(|r| matches!(r.role, RoleSpec::Harvester { .. })).unwrap();
        assert_eq!(harv.body.len(), 12, "replacement: 3 repeats — the S6 capacity body");
        assert!((harv.priority - 93.75).abs() < 1e-6, "1/4 lerp toward HIGH");

        // Hauler demand: none alive, 800 unfulfilled → body from available (300 → 3×[C,M],
        // base 150) → desired = min(800/150, 3) = 3 > 0 at HIGH.
        let haul = reqs.iter().find(|r| matches!(r.role, RoleSpec::Hauler)).unwrap();
        assert_eq!(haul.body.len(), 6, "bootstrap hauler: 3 repeats of [C,M] at 300");
        assert_eq!(haul.priority, SPAWN_PRIORITY_HIGH);
    }

    /// The exact-split contract: `repair_energy_consumed` matches the resolver's ceil pricing.
    #[test]
    fn repair_energy_consumed_matches_resolver_pricing() {
        assert_eq!(repair_energy_consumed(3, 10, 0, 1000), 3, "work-limited");
        assert_eq!(repair_energy_consumed(10, 2, 0, 1000), 2, "carry-limited");
        assert_eq!(repair_energy_consumed(10, 10, 899, 1000), 2, "ceil(101/100)");
        assert_eq!(repair_energy_consumed(10, 10, 900, 1000), 1);
        assert_eq!(repair_energy_consumed(10, 10, 1000, 1000), 0, "full target");
    }

    // ── M2 kernels (transcription pins) ─────────────────────────────────────────────────────────

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

    /// missions/upgrade.rs:183-200 / localbuild.rs:232-244 — the excess/sufficient thresholds,
    /// including the bare-room split: NO storage and NO containers ⇒ has_excess TRUE (upgrade.rs
    /// falls through to `true`) but has_sufficient FALSE (localbuild's `any()` over nothing).
    #[test]
    fn excess_and_sufficient_energy_thresholds() {
        let w = EconWorld::default();
        assert!(has_excess_energy(&w), "bare room: excess TRUE (upgrade.rs:197-199)");
        assert!(!has_sufficient_energy(&w), "bare room: sufficient FALSE (any() over empty)");
        // Storage thresholds: 100k excess / 50k sufficient.
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
        // Container thresholds: > 75% excess / > 50% sufficient (capacity 2000).
        let mut w = EconWorld::default();
        let c = w.add_container(pos(10, 10), 2000, 250_000);
        w.containers[c].store.add(SimResource::Energy, 1500);
        assert!(!has_excess_energy(&w), "exactly 75% is NOT > 75%");
        assert!(has_sufficient_energy(&w), "1500 > 50%");
        w.containers[c].store.add(SimResource::Energy, 1);
        assert!(has_excess_energy(&w));
    }

    /// missions/upgrade.rs:93-130 — the upkeep sizing: at/above half-max → 1 WORK; deep deficits
    /// stay 1 WORK (a single WORK restores ~29 cycles × 4950 = ~143k per lifetime — every
    /// realizable deficit fits; the live loop exists for the parameter shape, not the outcome).
    #[test]
    fn work_parts_for_upkeep_matches_live_math() {
        assert_eq!(work_parts_for_upkeep(10_000, 20_000), 1, "at the safe threshold: 1");
        assert_eq!(work_parts_for_upkeep(2_000, 20_000), 1, "RCL-3 at 10%");
        assert_eq!(work_parts_for_upkeep(0, 200_000), 1, "even the RCL-8 full deficit (100k ≤ 143k)");
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

    /// The upgrader bodies (missions/upgrade.rs:298-316): RCL ≤ 3 at the 300 floor = the bare
    /// pre [W,C,M,M]; RCL 3 at capacity 800 with the 10-W target = 3 [W,M] repeats (4 W total);
    /// RCL 4+ at 800 = [W,C,M,M] + 5×[W] energy-capped; RCL > 3 can't build below 350.
    #[test]
    fn upgrader_bodies_match_live_definitions() {
        use screeps::Part::*;
        assert_eq!(upgrader_body(3, 300, Some(10)).unwrap(), vec![Work, Carry, Move, Move], "min repeat 0 at the floor");
        let b = upgrader_body(3, 800, Some(10)).unwrap();
        assert_eq!(b.iter().filter(|p| **p == Work).count(), 4, "3 repeats of [W,M] within 800");
        let b = upgrader_body(4, 800, Some(20)).unwrap();
        assert_eq!(b.iter().filter(|p| **p == Work).count(), 6, "pre W + 5 repeat W within 800");
        assert!(upgrader_body(4, 300, Some(20)).is_none(), "RCL>3 needs ≥ 350 (min repeat 1)");
    }

    /// The builder priority tables (missions/localbuild.rs:49-111) + the repairer arm (:113-127)
    /// + the builder body cap (:268).
    #[test]
    fn builder_priority_and_body_match_live() {
        let mut w = EconWorld::default();
        assert!(builder_priority(&w, 3, true, 0).is_none(), "no sites → no builder demand");
        w.set_controller(pos(40, 40), 3);
        let s = w.add_construction_site(pos(10, 10), screeps_econ_engine::StructureKind::Extension).unwrap();
        // 3000 remaining at RCL ≤ 3 → 3 desired with sufficient energy, 1 without.
        assert_eq!(builder_priority(&w, 3, true, 0), Some((3, 62.5)), "(HIGH+MEDIUM)/2 with no builders");
        assert_eq!(builder_priority(&w, 3, false, 0).unwrap().0, 1, "insufficient energy → 1");
        assert_eq!(builder_priority(&w, 3, true, 1).unwrap().1, SPAWN_PRIORITY_MEDIUM, "extension sites → MEDIUM with a builder");
        w.sites[s].progress = 2_500; // 500 remaining → 1 desired
        assert_eq!(builder_priority(&w, 3, true, 0).unwrap().0, 1);
        // A spawn site raises the with-builders priority to HIGH (:96-97).
        w.add_construction_site(pos(11, 10), screeps_econ_engine::StructureKind::Spawn).unwrap();
        assert_eq!(builder_priority(&w, 3, true, 1).unwrap().1, SPAWN_PRIORITY_HIGH);

        // The repairer arm: a <25% road (High band) → (1, HIGH); allowance CriticalOnly hides it.
        let mut w = EconWorld::default();
        w.add_road(pos(10, 10), 1000, 5000);
        assert_eq!(repairer_priority(&w, RepairAllowance::Unrestricted), Some((1, SPAWN_PRIORITY_HIGH)));
        assert_eq!(repairer_priority(&w, RepairAllowance::CriticalOnly), None, "S1 gate: no repairer spawn");
        // A 40% road (Medium) → (1, MEDIUM); an 80% road (VeryLow) → none (< Medium).
        let mut w = EconWorld::default();
        w.add_road(pos(10, 10), 2000, 5000);
        assert_eq!(repairer_priority(&w, RepairAllowance::Unrestricted), Some((1, SPAWN_PRIORITY_MEDIUM)));
        let mut w = EconWorld::default();
        w.add_road(pos(10, 10), 4000, 5000);
        assert_eq!(repairer_priority(&w, RepairAllowance::Unrestricted), None, "VeryLow best never spawns");

        // The body cap: 5 [C,W,M,M] repeats below HIGH even with huge energy; uncapped at HIGH.
        let b = builder_body(10_000, SPAWN_PRIORITY_MEDIUM).unwrap();
        assert_eq!(b.len(), 20, "5 repeats × 4 parts (localbuild.rs:268 Some(5))");
        let b = builder_body(10_000, SPAWN_PRIORITY_HIGH).unwrap();
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
        // Equal kind: higher progress wins over distance…
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        w.add_construction_site(pos(11, 10), screeps_econ_engine::StructureKind::Extension).unwrap();
        let far = w.add_construction_site(pos(30, 30), screeps_econ_engine::StructureKind::Extension).unwrap();
        w.sites[far].progress = 100;
        assert_eq!(select_construction_site(pos(10, 10), &w, 3), Some((30, 30)), "progress beats range");
        // …and at equal progress the NEAREST wins.
        w.sites[far].progress = 0;
        assert_eq!(select_construction_site(pos(10, 10), &w, 3), Some((11, 10)));
    }

    /// The fill pickup (upgrade.rs:112-122 / haulbehavior.rs:70-125): nearest across ALL tiers
    /// AND both lanes (the Use-lane controller container IS visible — unlike haul selections);
    /// the slow-creep anchor filters to CONTROLLER range 5.
    #[test]
    fn fill_pickup_sees_use_lane_and_honors_anchor() {
        let use_pickup = Pickup {
            src: SrcKey::Container(20, 25),
            pos: pos(20, 25),
            tier: Tier::NonePri,
            available: 500,
            lane: Lane::Use,
        };
        let haul_pickup = pick(SrcKey::Storage, pos(35, 25), Tier::NonePri, 5_000);
        let set = vec![use_pickup, haul_pickup];
        let (src, _, take) = select_fill_pickup(pos(22, 25), 100, &set, None).unwrap();
        assert_eq!(src, SrcKey::Container(20, 25), "the NEAR Use-lane container wins for a filler");
        assert_eq!(take, 100, "min(free, available)");
        // The controller anchor (range 5 of (20,25)) excludes the distant storage entirely.
        let anchored = select_fill_pickup(pos(34, 25), 100, &set, Some((pos(20, 25), 5)));
        assert_eq!(anchored.unwrap().0, SrcKey::Container(20, 25), "anchor keeps the controller-side source");
        // Full creep: no pickup.
        assert!(select_fill_pickup(pos(22, 25), 0, &set, None).is_none());
    }

    /// The upgrader K4 arm end-to-end shapes: a downgrade-risk room with no upgraders emits a
    /// CRITICAL upkeep-sized request from AVAILABLE energy; a healthy bare room emits HIGH.
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
        assert_eq!(up.priority, SPAWN_PRIORITY_CRITICAL, "downgrade risk + no upgraders → CRITICAL");
        assert_eq!(
            up.body,
            vec![screeps::Part::Work, screeps::Part::Carry, screeps::Part::Move, screeps::Part::Move],
            "upkeep w=1 at the 300 floor: the bare pre-body"
        );

        w.controller.as_mut().unwrap().downgrade_ticks = 20_000; // healthy clock
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::Unrestricted);
        let up = reqs.iter().find(|r| matches!(r.role, RoleSpec::Upgrader)).expect("upgrader requested");
        assert_eq!(up.priority, SPAWN_PRIORITY_HIGH, "no upgraders yet → HIGH (:323-324)");
    }

    /// The builder K4 arm: sites → a builder request at 62.5 (no builders), allow_harvest frozen
    /// TRUE without storage; a repair-only room under the S1 arm spawns NO repairer.
    #[test]
    fn builder_spawn_arm_and_s1_gating() {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 10), 3000);
        w.add_spawn(pos(25, 25));
        w.set_controller(pos(40, 40), 2);
        w.add_construction_site(pos(24, 24), screeps_econ_engine::StructureKind::Extension).unwrap();
        let reqs = spawn_requests(&w, &BTreeMap::new(), 0, RepairAllowance::Unrestricted);
        let b = reqs.iter().find(|r| matches!(r.role, RoleSpec::Builder { .. })).expect("builder requested");
        assert_eq!(b.priority, 62.5);
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
}
