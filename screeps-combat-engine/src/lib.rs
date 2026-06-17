//! # screeps-combat-engine
//!
//! A **deterministic, JS-free** port of the Screeps combat tick — the *mechanism* layer of the
//! combat micro-sim (ADR 0006 Part B; the behaviors it validates live in ADR 0008a). It models a
//! single 50×50 room of creeps + structures and resolves combat exactly as the real engine does,
//! so the bot's own decision code can be exercised against it in milliseconds, deterministically,
//! with full introspection — no Docker, no server, no JS.
//!
//! Ground truth is the cloned engine at `C:\code\screeps-engine` (NOT folklore / docs): every
//! formula here cites the engine source it ports. The companion bot kernel `military/damage.rs`
//! is a *sizing heuristic*; this crate is the *exact tick resolution* — they overlap on the tower
//! falloff (kept identical) but serve different roles.
//!
//! ## Status (P2.H1)
//! **First slice (this commit):** the combat-math kernel — [`constants`], the [`body`] model
//! (per-part 100-hit pools, back-to-front degradation, `calcBodyEffectiveness`, TOUGH/boost
//! `_applyDamage` reduction) and [`damage`] (range falloff: rangedMassAttack + tower), plus the
//! [`state`] value types ([`CombatWorld`]). All host-tested against hand-computed engine values
//! (the EXP-FOUND-2 degradation/TOUGH conformance checks).
//!
//! **Landed (P2.H1 — engine port closed):** the full two-phase tick ([`resolve`]: combat
//! accumulate → movement → apply + netting + deaths), same-tile movement-conflict resolution
//! ([`movement`]: eligibility/fatigue, swap + moves/weight tiebreak, obstacle + chain-block, **pull**
//! rate2/rate3), structures ([`state`]: ramparts/walls/spawn **and towers** as attack/dismantle/RMA
//! targets with rampart RMA-shielding; towers fire heal/repair/attack *and* are themselves
//! targetable + repairable), and the [`record`] replay artifact (`CombatRecording`: per-tick state +
//! intents + reason tags + outcomes → the "see WHY a squad did X" introspection). **40 host tests**:
//! kill inequality, focus-fire, tower drain, safe mode, attack-back (EXP-FOUND-1/EXP-FOCUS-1),
//! kiting at MOVE parity (EXP-KITE-1), wall-breach/spawn-kill/rampart-shield/tower-heal/
//! repair-vs-dismantle/tower-as-target (EXP-BREACH/EXP-DEF), pull, and recording capture/replay.
//!
//! **Now (P2.H2):** the `CombatView`/`CombatIntent` trait seam lives in `screeps-ibex::combat`; the
//! sim adapter + `IbexAgent` (this engine driving the bot's real decision code) live in
//! `screeps-combat-agent`. The byte-exact server-captured conformance vectors moved to P2.H5.
//!
//! Provenance + the engine→code source map + the reconciliation procedure live in `AGENTS.md`;
//! user-facing overview in `README.md`. Read `AGENTS.md` before changing any formula.

pub mod body;
pub mod constants;
pub mod damage;
pub mod movement;
pub mod record;
pub mod resolve;
pub mod state;

pub use body::{BodyPartDef, BoostTier, SimBody};
pub use movement::resolve_moves;
pub use record::{record_tick, CombatRecording, TickFrame};
pub use resolve::{resolve_tick, CombatAction, Intents, TickReport, TowerAction};
pub use state::{
    CombatTerrain, CombatWorld, CreepId, PlayerId, SimCreep, SimStructure, SimTower, StructureId,
    StructureKind,
};
