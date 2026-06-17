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
//! **Landed:** [`resolve`] — the full two-phase tick (combat accumulate → movement → apply +
//! netting + deaths) and [`movement`] — same-tile conflict resolution (eligibility/fatigue, swap +
//! moves/weight tiebreak, obstacle + chain-block). 24 host tests: the kill inequality, focus-fire,
//! tower drain, safe mode, melee attack-back (EXP-FOUND-1/EXP-FOCUS-1) and range-3 kiting at MOVE
//! parity (EXP-KITE-1).
//!
//! **Next slice:** structures as damage targets (ramparts/walls/spawn) + dismantle + tower
//! heal/repair, pull-based movement (rate2/rate3), then `CombatRecording` (replay artifact) and the
//! server-captured conformance vectors (P2.H1 *done* = byte-exact on those).
//!
//! Provenance + the engine→code source map + the reconciliation procedure live in `AGENTS.md`;
//! user-facing overview in `README.md`. Read `AGENTS.md` before changing any formula.

pub mod body;
pub mod constants;
pub mod damage;
pub mod movement;
pub mod resolve;
pub mod state;

pub use body::{BodyPartDef, BoostTier, SimBody};
pub use movement::resolve_moves;
pub use resolve::{resolve_tick, CombatAction, Intents, TickReport, TowerAction};
pub use state::CombatTerrain;
pub use state::{CombatWorld, CreepId, PlayerId, SimCreep, SimTower};
