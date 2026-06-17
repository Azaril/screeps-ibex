//! Combat world state — JS-free value types over `screeps::Position`. The deterministic tick
//! (`resolve.rs`, next slice) operates on a `CombatWorld`. Scope: a single 50×50 room (ADR 0006
//! Part B); ramparts/walls/safe-mode-per-room and terrain are added with the resolve slice.

use crate::body::SimBody;
use screeps::Position;

/// A combatant identity (self-play: side 0 vs side 1; NPCs get their own ids later).
pub type PlayerId = u8;
/// Stable per-engagement creep id (minted by the scenario; NOT a game `ObjectId`).
pub type CreepId = u32;

/// A creep in the sim.
#[derive(Clone, Debug)]
pub struct SimCreep {
    pub id: CreepId,
    pub owner: PlayerId,
    pub pos: Position,
    pub body: SimBody,
    /// Fatigue carried into this tick; the creep may move only when it is 0.
    pub fatigue: u32,
}

impl SimCreep {
    pub fn is_alive(&self) -> bool {
        self.body.is_alive()
    }
}

/// A tower in the sim. Towers fire once per tick for [`crate::constants::TOWER_ENERGY_COST`]
/// energy and resolve in the same two-phase step as creep combat (the drain math).
#[derive(Clone, Debug)]
pub struct SimTower {
    pub owner: PlayerId,
    pub pos: Position,
    pub energy: u32,
    pub hits: u32,
}

/// One room's combat state for a tick.
#[derive(Clone, Debug, Default)]
pub struct CombatWorld {
    pub tick: u32,
    pub creeps: Vec<SimCreep>,
    pub towers: Vec<SimTower>,
    /// Owner whose controller is in safe mode this tick (all *hostile* combat zeroed), if any.
    pub safe_mode_owner: Option<PlayerId>,
}

impl CombatWorld {
    pub fn living_creeps(&self) -> impl Iterator<Item = &SimCreep> {
        self.creeps.iter().filter(|c| c.is_alive())
    }
}
