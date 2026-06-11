//! The game-API boundary seam (P1.A6 / ADR 0006) — SKELETON.
//!
//! This trait names the seam between the bot and the live game API so
//! later increments can stand test doubles and (at Inc 6) the
//! record/replay machinery behind it. Phase 1 deliberately ships the
//! NAME plus the reads its own telemetry needs — not a big-bang
//! abstraction: consumers migrate when their increment touches them
//! (the kernel-vs-shell rule; ADR 0015's S-contract for this seam is
//! DRAFT until the Inc-6 replay work freezes it).
//!
//! Growth rule: add a method here when a consumer is MIGRATED to the
//! trait, never speculatively.

/// The reads the Phase-1 telemetry/governor path performs against the
/// game API. `LiveGame` is the wasm-side passthrough; tests provide
/// [`FixedGameView`].
pub trait GameView {
    fn time(&self) -> u32;
    fn cpu_used(&self) -> f64;
    fn cpu_limit(&self) -> f64;
    fn cpu_tick_limit(&self) -> f64;
    fn bucket(&self) -> i32;
}

/// The live game API (wasm runtime only — the calls trap on a host
/// target, which is fine: host consumers use [`FixedGameView`]).
pub struct LiveGame;

impl GameView for LiveGame {
    fn time(&self) -> u32 {
        screeps::game::time()
    }
    fn cpu_used(&self) -> f64 {
        screeps::game::cpu::get_used()
    }
    fn cpu_limit(&self) -> f64 {
        screeps::game::cpu::limit() as f64
    }
    fn cpu_tick_limit(&self) -> f64 {
        screeps::game::cpu::tick_limit()
    }
    fn bucket(&self) -> i32 {
        screeps::game::cpu::bucket()
    }
}

/// Host-side double: fixed values, good enough for kernel tests of
/// anything that migrates onto the seam.
#[derive(Debug, Clone, Copy)]
pub struct FixedGameView {
    pub time: u32,
    pub cpu_used: f64,
    pub cpu_limit: f64,
    pub cpu_tick_limit: f64,
    pub bucket: i32,
}

impl Default for FixedGameView {
    fn default() -> Self {
        FixedGameView {
            time: 1,
            cpu_used: 0.0,
            cpu_limit: 100.0,
            cpu_tick_limit: 500.0,
            bucket: 10_000,
        }
    }
}

impl GameView for FixedGameView {
    fn time(&self) -> u32 {
        self.time
    }
    fn cpu_used(&self) -> f64 {
        self.cpu_used
    }
    fn cpu_limit(&self) -> f64 {
        self.cpu_limit
    }
    fn cpu_tick_limit(&self) -> f64 {
        self.cpu_tick_limit
    }
    fn bucket(&self) -> i32 {
        self.bucket
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam is object-safe (doubles and the live impl swap behind
    /// `&dyn GameView`) — the property the Inc-6 replay work needs.
    #[test]
    fn seam_is_object_safe_and_doubles_swap() {
        let fixed = FixedGameView {
            bucket: 4_321,
            ..Default::default()
        };
        let view: &dyn GameView = &fixed;
        assert_eq!(view.bucket(), 4_321);
        assert_eq!(view.cpu_tick_limit(), 500.0);
    }
}
