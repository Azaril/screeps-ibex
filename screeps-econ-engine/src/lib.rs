//! # screeps-econ-engine
//!
//! The **deterministic, JS-free economy overlay** on the shared `screeps-sim-core` movement kernel
//! (ADR 0040 §D7, milestone M0) — exactly as `screeps-combat-engine` overlays combat: [`EconWorld`]
//! embeds a [`MovementState`], [`EconIntents`] embeds `MoveIntents`, and [`resolve_econ_tick`]
//! calls `resolve_movement` at the movement point of its pipeline (the `Simulation` layering
//! contract — "No change to that crate is needed to add a layer").
//!
//! **Scope (M0 + M1):** source harvest + regen (with the first-harvest-below-cap timer),
//! per-resource stores, transfer/withdraw/pickup, atomic `spawnCreep` energy debit with the
//! engine's draw order plus spawn self-charge and extensions-by-RCL, TTL death dropping stores to
//! ground, dropped-resource decay, and an **exact integer energy-conservation audit every tick**,
//! surfaced in the tick report (never panic-only — the eval harness gates on it, EP-6.12).
//! **M1 adds:** the Repair work intent (roads/containers, engine `creeps/repair.js` pricing,
//! ledgered by class + the [`RepairLeak`] `repair_leak_e` report mirror), road decay with
//! `ROAD_WEAROUT` per-step traffic wear, and container decay (death drops the store) — dead
//! structures are removed and a dead road's tile reverts to natural terrain.
//! **M2 adds:** the controller (UpgradeController on its own Pipeline-E lane, the RCL8 15 e/t
//! shared cap, level-ups with remainder carry + the near-full-clock gate, the downgrade clock
//! with its +100/upgrade-tick restore, downgrades refunding 0.9× the new level's cost), Build +
//! construction sites (5 progress/WORK/t at 1 e/progress, completion materializes
//! spawn/extension/road/container/storage/tower-stub with engine birth state), and the
//! RCL-allowance-enforced site placement API ([`EconWorld::add_construction_site`]).
//! Labs/minerals land M6 ([`state::SimMineral`] is a stub).
//!
//! **Ground truth** is `docs/references/engine-mechanics.md` (which pins the cloned engine source);
//! every constant in [`constants`] cites it and is pinned by a unit test. Deviations from the real
//! engine's per-intent pipelines are documented at each pipeline step in [`tick`].
//!
//! **Determinism (EP-6.13):** no HashMap/HashSet iteration reaches any decision, ordering, or
//! emitted result — stores are `BTreeMap` keyed by u32 ids, action processing is creep-id-ordered
//! after a stable sort, tie-breaks are packed-coordinate/index order. No ambient entropy; all
//! quantities are exact integers. The in-crate fence (`tests/determinism.rs`) proves 5-run digest
//! spread 0 + insertion-order invariance + conservation under randomized-intent fuzzing.
//!
//! [`spawn_queue`] is the spawn QUEUE policy kernel (descending-priority head-of-line banking),
//! moved verbatim from `screeps-combat-decision::spawn_throughput` (ADR 0040 §D8 #3) so the
//! economy sim's spawn system and the combat lifecycle harness share one implementation (EP-2.6).

pub mod constants;
pub mod intents;
pub mod ledger;
pub mod spawn_queue;
pub mod state;
pub mod tick;

pub use intents::{EconAction, EconIntents, StructRef};
pub use ledger::{audit_conservation, ConservationViolation, TickLedger};
pub use spawn_queue::{spawn_step, HomeLanes, QueuedSpawn, Spawned};
pub use state::{
    EconWorld, PendingCreep, SimConstructionSite, SimContainer, SimController, SimDropped,
    SimExtension, SimMineral, SimResource, SimRoad, SimSource, SimSpawn, SimStorage, SimStore,
    SimTower, SitePlacementError, StructureKind,
};
pub use tick::{resolve_econ_tick, EconSim, EconTickReport, RepairLeak};

// The movement-layer value types come from the kernel; re-export the ones economy call sites need
// (the combat-engine convention) so `screeps_econ_engine::CreepId` etc. resolve.
pub use screeps_sim_core::{CreepId, MovementState, PlayerId, SimCreep, SimTerrain, StructureId};
