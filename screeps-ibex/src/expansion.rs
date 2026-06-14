//! Cross-system expansion state shared by the claim pipeline and the colony
//! lifecycle.
//!
//! [`ExpansionAvoidance`] is the "avoid-cooldown" map (ADR 0017): rooms a claim
//! recently failed or abandoned, with a retry-after tick. It is written by the
//! claimer abort (`ClaimMission`) and the colony abort (`ColonyState` unclaim),
//! and read by the pre-claim safety gate (`ClaimOperation`). Keeping it in one
//! ephemeral `Resource` (rather than serialized on `ClaimOperation`) lets both
//! an operation and a mission touch it without a cross-owner write path; the
//! cooldown only needs to prevent immediate re-claim thrash *within* a VM
//! lifetime — after a reset the safety gate re-vetoes a still-contested room on
//! its own.

use screeps::RoomName;
use std::collections::HashMap;

/// Rooms to avoid claiming again until a retry-after tick (the avoid-cooldown
/// map). A specs `Resource`; `Default`-constructed.
#[derive(Default)]
pub struct ExpansionAvoidance {
    rooms: HashMap<RoomName, u32>,
}

impl ExpansionAvoidance {
    /// Avoid `room` until at least `until_tick` (extends an existing entry,
    /// never shortens it).
    pub fn avoid(&mut self, room: RoomName, until_tick: u32) {
        let entry = self.rooms.entry(room).or_insert(until_tick);
        *entry = (*entry).max(until_tick);
    }

    /// Whether `room` is currently in avoid-cooldown.
    pub fn is_avoided(&self, room: RoomName, now: u32) -> bool {
        self.rooms.get(&room).map(|until| *until > now).unwrap_or(false)
    }

    /// Drop entries whose cooldown has elapsed (bounds the map).
    pub fn prune(&mut self, now: u32) {
        self.rooms.retain(|_, until| *until > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avoid_sets_and_expires() {
        let room: RoomName = "E5N5".parse().unwrap();
        let mut a = ExpansionAvoidance::default();
        assert!(!a.is_avoided(room, 0));
        a.avoid(room, 1000);
        assert!(a.is_avoided(room, 999));
        assert!(!a.is_avoided(room, 1000));
        // Extends, never shortens.
        a.avoid(room, 2000);
        a.avoid(room, 1500);
        assert!(a.is_avoided(room, 1999));
        a.prune(2000);
        assert!(!a.is_avoided(room, 2001));
    }
}
