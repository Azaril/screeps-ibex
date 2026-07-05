//! # screeps-econ-decision
//!
//! The economy decision seam (ADR 0040 §D5, milestone M3) — the combat-decision pattern
//! replicated for the civilian economy: a pure, JS-free crate that owns the bot's economy
//! *decisions* behind value-type DTOs, consumed identically by the live bot (screeps-ibex's
//! transfer/haul/mission adapters) and the offline economy sim (screeps-econ-eval). After M3
//! there is exactly ONE implementation of each extracted policy (EP-2.6) — the sim's M1
//! transcription mirrors are deleted and import these kernels instead.
//!
//! **The seam vocabulary** (§D5): [`EconomyView`]-side DTOs are plain value types —
//! `BTreeMap`/sorted collections, opaque node ids, no game handles, no `game::*` below this
//! boundary (the crate cannot even reach the live game). [`EconomyIntent`] is the write seam:
//! what a kernel decided, for an adapter to execute.
//!
//! **The kernels** (each module header carries its live provenance; every moved function keeps
//! its tests):
//! - **K1 — demand registration** ([`demand`]): the per-structure transfer-demand policy from
//!   `missions/localsupply/room_transfer.rs` (spawns/extensions High, the provider/controller
//!   container ladders, storage None, dropped/tombstone tiers, the link deposit/withdraw
//!   ladders). Lives here now, consumed by `RoomTransferMission` (live) and
//!   `screeps-econ-eval::baseline` (sim).
//! - **K2 — the TransferSnapshot + selection** ([`snapshot`], ADR 0007 Q5 items 1–3 via 0040
//!   reconciliation R1): the immutable per-tick snapshot + the pure pickup/delivery selection
//!   kernels (tier-interleave, value-density scoring, nearest-wins) from
//!   `transfer/transfersystem.rs` + `jobs/utility/haulbehavior.rs`. Bookings stay adapter-side
//!   (the live `TransferQueue` pending maps / the sim's booking table) and are passed in as the
//!   [`snapshot::SnapshotBookings`] view.
//! - **K3 — repair admission** ([`stress`], [`repair`]): the S1 energy-stress allowance kernel
//!   from `energy_stress.rs` + the repair priority maps/ordering from `jobs/utility/repair.rs`
//!   and `repairqueue.rs`.
//! - **K4 — spawn-request policy** ([`spawn_policy`]): body-shape + sizing + priority-band
//!   selection for the localsupply roles from `missions/localsupply/source_mining.rs`,
//!   `missions/haul.rs`, `missions/upgrade.rs`, `missions/localbuild.rs`. Missions keep their
//!   alive-count/ECS bookkeeping; the sim's K4 arm calls the same functions.
//! - **Re-match cadence** ([`cadence`], ADR 0007 item 2): the governor-tier re-match/backoff
//!   policy; the governor *read* stays adapter-side.
//! - **The M4 MARKET CANDIDATE kernels** (ADR 0040 §D1/§D3, behind the same seam — the sim
//!   tournament consumes them now, the live bot at M5a): [`sink_economics`] (the e/t sink bids,
//!   the opportunity floor + admission, the survival vetoes, K4 deficit-priced bodies) and
//!   [`matching`] (the deterministic greedy bid-density assignment, the shipped §D3 v1; the
//!   exact oracle is sim-only in econ-eval, never a bot dependency).
//!
//! **Determinism:** kernels iterate deterministic orders (sorted DTO collections, adapter-
//! controlled candidate order) and compare exact integer rationals (`a1·d2 ⋛ a2·d1`) instead of
//! floats, with first-in-candidate-order winning exact ties. The live code's HashMap-iteration
//! tie-breaks were nondeterministic (VM-reset-dependent); same policy, fence-safe arithmetic —
//! the documented M3 determinism deviation (identical to the sim baseline's M1 convention).

pub mod cadence;
pub mod demand;
pub mod market;
pub mod matching;
pub mod priority;
pub mod repair;
pub mod sink_economics;
pub mod snapshot;
pub mod spawn_policy;
pub mod stress;

/// A creep as the K2 selection kernels see it (the `creep_dto` of ADR 0040 §D5's
/// `select_pickup_and_delivery(&snapshot, creep_dto)` seam): opaque id + position + capacity +
/// carried resources. Adapters build it from the live `Creep` handle / the sim's creep store.
#[derive(Clone, Debug)]
pub struct CreepEconDto {
    /// Opaque creep identity (live: entity id bits; sim: creep id). Not interpreted here.
    pub id: u64,
    pub pos: screeps::Position,
    /// Free capacity (the selection budget for pickups).
    pub free_capacity: u32,
    /// Carried resources in adapter-deterministic order (the selection budget for
    /// carried-cargo deliveries).
    pub store: Vec<(screeps::ResourceType, u32)>,
}

impl CreepEconDto {
    /// Total carried amount across all resources.
    pub fn carried_total(&self) -> u32 {
        self.store.iter().map(|(_, a)| a).sum()
    }
}

/// The write seam (ADR 0040 §D5): one decided economy action, for an adapter to execute.
/// K1 emits `RegisterWithdraw`/`RegisterDeposit`; K2 emits `AssignTickets` (the caller pairs the
/// selection output with the creep); K3 emits `AdmitRepair`; K4 emits `RequestSpawn`.
#[derive(Clone, Debug)]
pub enum EconomyIntent {
    /// K1: register a withdraw (supply) request against a demand item.
    RegisterWithdraw(demand::Demand),
    /// K1: register a deposit (demand) request against a demand item.
    RegisterDeposit(demand::Demand),
    /// K2: assign a pickup+delivery ticket set to a creep.
    AssignTickets {
        creep: u64,
        pickup: Option<snapshot::WithdrawTicketDto>,
        deliveries: Vec<snapshot::DepositTicketDto>,
    },
    /// K3: the room's repair admission posture this tick.
    AdmitRepair { allowance: stress::RepairAllowance },
    /// K4: enqueue a spawn request.
    RequestSpawn(spawn_policy::SpawnPlan),
}
