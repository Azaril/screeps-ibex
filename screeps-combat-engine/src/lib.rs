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
//! **Landed:** [`resolve`] — the two-phase accumulate-then-apply tick (intent priority/exclusion
//! table, per-target damage/heal pooling, damage-then-heal netting, deaths, fatigue regen) for the
//! *stationary* engagement (drives EXP-FOUND-1 / EXP-FOCUS-1: the kill inequality, focus-fire,
//! tower drain, safe mode, melee attack-back are all tested).
//!
//! **Next slice:** same-tile movement-conflict resolution (`rate1..rate4`, pull/swap — where the
//! kiting/cohesion bugs live), structures as damage targets (ramparts/walls/spawn) + dismantle +
//! tower heal/repair, then `CombatRecording` and the server-captured conformance vectors.
//!
//! Provenance + the engine→code source map + the reconciliation procedure live in `AGENTS.md`;
//! user-facing overview in `README.md`. Read `AGENTS.md` before changing any formula.

pub mod body;
pub mod constants;
pub mod damage;
pub mod resolve;
pub mod state;

pub use body::{BodyPartDef, BoostTier, SimBody};
pub use resolve::{resolve_tick, CombatAction, Intents, TickReport, TowerAction};
pub use state::{CombatWorld, CreepId, PlayerId, SimCreep, SimTower};
