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
// K4 — spawn requests (rebuilt per tick, spawnsystem re-enqueue semantics).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The spawn priority bands (spawnsystem.rs:22-39).
pub const SPAWN_PRIORITY_CRITICAL: f32 = 100.0;
pub const SPAWN_PRIORITY_HIGH: f32 = 75.0;
pub const SPAWN_PRIORITY_MEDIUM: f32 = 50.0;

/// What a queued body is for — carried alongside the request so births map to roles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleSpec {
    Harvester { source_idx: usize },
    Hauler,
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
pub fn spawn_requests(
    world: &EconWorld,
    roles: &BTreeMap<u32, RoleSpec>,
    unfulfilled_hauling: u32,
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
        let reqs = spawn_requests(&w, &roles, 800);
        let harv = reqs.iter().find(|r| matches!(r.role, RoleSpec::Harvester { .. })).unwrap();
        assert_eq!(harv.body.len(), 4, "bootstrap harvester: 1 repeat of [M,M,C,W] at 300 budget");
        assert_eq!(harv.priority, SPAWN_PRIORITY_CRITICAL, "first harvester is CRITICAL");

        // With one harvester alive, the replacement sizes from CAPACITY (800 → 3 repeats = 750).
        let mut roles = BTreeMap::new();
        roles.insert(1u32, RoleSpec::Harvester { source_idx: 0 });
        let reqs = spawn_requests(&w, &roles, 800);
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
}
