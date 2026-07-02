//! Shared inter-room route-pricing rules (REC-024).
//!
//! The claim pipeline's reach oracle (`PathfinderService::route_distance_via`)
//! and the live mover (screeps-rover, priced by
//! `MovementSystemExternalProvider::get_room_cost` in
//! `pathing/movementsystem.rs`) must agree on which rooms an economy creep may
//! traverse. ADR 0038 made `is_claim_feasible` the SOLE reach gate for
//! expansion, so an oracle that prices a hostile corridor as merely expensive
//! approves routes the claimer's own mover (`HostileBehavior::Deny`) refuses
//! to walk — claimers die en route or `PathNotFound`-loop, the soft-stall
//! flavor ADR 0038 set out to kill. The old `compute_route` callback read
//! `game::rooms()` (only currently-VISIBLE rooms), so invisible remote
//! corridors always priced at the traversable default.
//!
//! `is_hostile_for_movement` mirrors the mover's hostile derivation
//! (`movementsystem.rs` `get_room_cost`). That file is owned by a parallel
//! workstream this cycle, so the predicate is DUPLICATED here with this
//! cross-reference instead of extracted — keep the two in sync (folding the
//! mover onto this kernel is the follow-up seam). Room-STATUS traversability
//! (`can_traverse_between_room_status`) is deliberately not mirrored:
//! `find_route` handles closed rooms internally.
//!
//! Pure kernel over a plain-bool DTO (EP-6.2): the adapter reads cached
//! `RoomData` dynamic-visibility intel through public getters; tests construct
//! the DTO directly.

use crate::room::data::RoomDynamicVisibilityData;

/// Route-callback cost for an unscouted room (no cached dynamic intel).
/// Passable — denying unknowns would wall off the whole frontier — but priced
/// ABOVE known-neutral (2.0) so routes prefer scouted corridors. This is the
/// cheap conservatism REC-024 asked for against the `hops × 50` travel
/// estimate's terrain blindness: a `[CLAIM, MOVE]` claimer pays ~5 ticks per
/// swamp tile against only a 50-tick arrival margin, and preferring known
/// rooms is the cheapest hedge short of a terrain-aware router (explicitly out
/// of scope).
pub const UNSCOUTED_ROUTE_COST: f64 = 2.5;

/// Cached room intel needed to price a room for economy routing. Plain bools
/// so the pricing kernel stays world-free and host-testable
/// (`RoomDynamicVisibilityData` is not constructible outside `room::data`).
#[derive(Debug, Clone, Copy)]
pub struct RouteRoomIntel {
    /// Source-keeper room.
    pub source_keeper: bool,
    /// Reserved by a hostile player.
    pub reservation_hostile: bool,
    /// Hostile creeps sighted.
    pub hostile_creeps: bool,
    /// Armed hostile towers sighted.
    pub hostile_towers: bool,
    /// Owned by a hostile player.
    pub owner_hostile: bool,
    /// Owner or reservation is mine/friendly.
    pub friendly: bool,
    /// Raw derelict classification (hostile-owned but militarily dead at the
    /// last sighting) — the mover's deliberately-loose pathing gate, NOT the
    /// stricter `confirmed_derelict` action gate (see `get_room_cost`'s
    /// rationale: gating pathing on fresh confirmation deadlocked the creeps
    /// that would refresh it).
    pub derelict: bool,
}

impl RouteRoomIntel {
    /// Adapter from cached dynamic visibility (public getters only).
    pub fn from_dynamic(d: &RoomDynamicVisibilityData) -> RouteRoomIntel {
        RouteRoomIntel {
            source_keeper: d.source_keeper(),
            reservation_hostile: d.reservation().hostile(),
            hostile_creeps: d.hostile_creeps(),
            hostile_towers: d.hostile_towers(),
            owner_hostile: d.owner().hostile(),
            friendly: d.owner().mine() || d.owner().friendly() || d.reservation().mine() || d.reservation().friendly(),
            derelict: d.derelict(),
        }
    }
}

/// The mover's hostile-room predicate — the exact derivation
/// `MovementSystemExternalProvider::get_room_cost` applies before its
/// `HostileBehavior` dispatch (`pathing/movementsystem.rs`): SK rooms, hostile
/// reservations, hostile creeps, armed towers, or a hostile OWNER unless the
/// room is derelict-passable (`derelict_pathing_on` = `features.derelict.on`).
pub fn is_hostile_for_movement(i: &RouteRoomIntel, derelict_pathing_on: bool) -> bool {
    let derelict = derelict_pathing_on && i.derelict;
    i.source_keeper || i.reservation_hostile || i.hostile_creeps || i.hostile_towers || (i.owner_hostile && !derelict)
}

/// Economy-corridor route cost: `None` = DENY (the mover's
/// `HostileBehavior::Deny`, mapped to an infinite room cost by the route
/// callback), `Some(cost)` mirrors the mover's tile-agnostic room pricing
/// (friendly 1.0, derelict 2.5, neutral 2.0). The one deliberate divergence
/// from `get_room_cost`: a room with NO cached intel prices at
/// [`UNSCOUTED_ROUTE_COST`] (2.5) instead of the mover's 2.0 default — route
/// PREFERENCE conservatism only (see the constant's doc); it never denies.
pub fn economy_route_cost(intel: Option<RouteRoomIntel>, derelict_pathing_on: bool) -> Option<f64> {
    let Some(i) = intel else {
        return Some(UNSCOUTED_ROUTE_COST);
    };
    if is_hostile_for_movement(&i, derelict_pathing_on) {
        return None;
    }
    if i.friendly {
        Some(1.0)
    } else if derelict_pathing_on && i.derelict {
        // Passable, but prefer truly neutral routes on ties (mover parity).
        Some(2.5)
    } else {
        Some(2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral() -> RouteRoomIntel {
        RouteRoomIntel {
            source_keeper: false,
            reservation_hostile: false,
            hostile_creeps: false,
            hostile_towers: false,
            owner_hostile: false,
            friendly: false,
            derelict: false,
        }
    }

    /// REC-024 pin: the reach oracle's hostility predicate must match the
    /// mover's (`movementsystem.rs` `get_room_cost`) signal-for-signal. An
    /// oracle more permissive than the mover approves corridors the claimer
    /// refuses to walk (repeated claimer deaths / `PathNotFound` loops — the
    /// ADR 0038 soft-stall class this module exists to close).
    #[test]
    fn hostile_predicate_mirrors_the_mover() {
        type Mutator = fn(&mut RouteRoomIntel);
        let cases: &[(&str, Mutator, bool)] = &[
            ("neutral room", |_| {}, false),
            ("source keeper", |i| i.source_keeper = true, true),
            ("hostile reservation", |i| i.reservation_hostile = true, true),
            ("hostile creeps", |i| i.hostile_creeps = true, true),
            ("armed hostile towers", |i| i.hostile_towers = true, true),
            ("hostile owner", |i| i.owner_hostile = true, true),
        ];
        for (label, setup, expect) in cases {
            let mut i = neutral();
            setup(&mut i);
            assert_eq!(is_hostile_for_movement(&i, true), *expect, "{label}");
        }

        // Derelict loosening (mover parity): hostile-OWNED but militarily dead
        // is NOT hostile for pathing…
        let mut derelict = neutral();
        derelict.owner_hostile = true;
        derelict.derelict = true;
        assert!(!is_hostile_for_movement(&derelict, true), "derelict room is passable");
        // …only while derelict pathing is feature-enabled…
        assert!(is_hostile_for_movement(&derelict, false), "derelict loosening is feature-gated");
        // …and anything ARMED stays hostile even if flagged derelict.
        derelict.hostile_towers = true;
        assert!(is_hostile_for_movement(&derelict, true), "armed derelict stays hostile");
    }

    /// REC-024 pin: hostile rooms are DENIED (None → infinite route cost) —
    /// not the legacy 10.0 high-cost, which still let `find_route` send a
    /// claimer through a room its mover would refuse — and the passable tiers
    /// price exactly like the mover (friendly 1.0 / neutral 2.0 / derelict
    /// 2.5).
    #[test]
    fn economy_route_cost_denies_hostile_and_prices_like_the_mover() {
        let mut hostile = neutral();
        hostile.owner_hostile = true;
        assert_eq!(economy_route_cost(Some(hostile), true), None);

        let mut friendly = neutral();
        friendly.friendly = true;
        assert_eq!(economy_route_cost(Some(friendly), true), Some(1.0));

        assert_eq!(economy_route_cost(Some(neutral()), true), Some(2.0));

        let mut derelict = neutral();
        derelict.owner_hostile = true;
        derelict.derelict = true;
        assert_eq!(economy_route_cost(Some(derelict), true), Some(2.5));
    }

    /// REC-024 conservatism pin: an UNSCOUTED room is passable (denying
    /// unknowns would wall off the frontier) but priced strictly above
    /// known-neutral, so routes prefer scouted corridors — the cheap hedge
    /// against `hops × 50` terrain blindness vs the claimer's 50-tick margin.
    #[test]
    fn unscouted_rooms_are_passable_but_dispreferred() {
        let unscouted = economy_route_cost(None, true).expect("must never deny an unknown room");
        assert_eq!(unscouted, UNSCOUTED_ROUTE_COST);
        let known_neutral = economy_route_cost(Some(neutral()), true).expect("neutral is passable");
        assert!(unscouted > known_neutral, "must price above known-neutral ({unscouted} vs {known_neutral})");
        assert!(unscouted.is_finite());
    }
}
