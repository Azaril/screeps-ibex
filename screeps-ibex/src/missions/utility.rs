use crate::pathing::pathfinderservice::PathfinderService;
use crate::room::data::*;
use screeps::*;

// ── Expansion feasibility (dynamic build/claim range) ───────────────────────
//
// The reach of an expansion is gated by creep economics, not a config cap:
// a creep must arrive with enough lifetime left to do useful work. These
// helpers reuse `PathfinderService::travel_ticks` (the budgeted route-cache
// estimate, `hops * 50`) — the same pattern salvage/composition sizing uses.

/// Normal creep lifetime (ticks). Local const mirrors `military/composition.rs`
/// (the screeps crate constant is not conveniently typed for this arithmetic).
const CREEP_LIFE_TIME: u32 = 1500;
/// CLAIM-creep lifetime (ticks): claimers live only 600.
const CREEP_CLAIM_LIFE_TIME: u32 = 600;
/// Minimum on-site working life for a remote builder to be worth spawning —
/// it must arrive with enough life to harvest and build. Structural threshold,
/// not a deployment knob (cf. salvage's 300-tick floor).
pub const MIN_USEFUL_BUILD_TICKS: u32 = 300;
/// Slack for a claimer to actually claim on arrival.
const CLAIM_ARRIVAL_MARGIN: u32 = 50;
/// Rough ticks per room-hop, matching `PathfinderService` (`hops * 50`) and the
/// engine's "hops × 50" travel convention. Used only for radius→time bounds.
pub const TICKS_PER_HOP: u32 = 50;

/// On-site working life a normal-lifetime builder has after travelling from
/// `home` to `target` (`CREEP_LIFE_TIME` minus one-way travel). `None` if the
/// target is unreachable.
pub fn build_effective_life(pathfinder: &mut PathfinderService, home: RoomName, target: RoomName) -> Option<u32> {
    let travel = pathfinder.travel_ticks(home, target, game::time())?;
    Some(CREEP_LIFE_TIME.saturating_sub(travel))
}

/// Whether a remote builder from `home` can do meaningful work at `target`
/// within its lifetime.
pub fn is_build_feasible(pathfinder: &mut PathfinderService, home: RoomName, target: RoomName) -> bool {
    build_effective_life(pathfinder, home, target)
        .map(|life| life >= MIN_USEFUL_BUILD_TICKS)
        .unwrap_or(false)
}

/// Whether a CLAIM creep from `home` can reach `target` while still alive to
/// claim it. Claim feasibility implies build feasibility (claimers are
/// shorter-lived than builders), so it is the binding gate on which rooms we
/// are willing to claim and establish.
pub fn is_claim_feasible(pathfinder: &mut PathfinderService, home: RoomName, target: RoomName) -> bool {
    pathfinder
        .travel_ticks(home, target, game::time())
        .map(|travel| travel.saturating_add(CLAIM_ARRIVAL_MARGIN) <= CREEP_CLAIM_LIFE_TIME)
        .unwrap_or(false)
}

/// Widest claim search radius (room-hops) worth exploring: beyond it a claimer
/// cannot reach the target alive. Derived from game constants, not config — it
/// is the dynamic upper bound for the adaptive search radius.
pub fn max_claim_radius_hops() -> u32 {
    CREEP_CLAIM_LIFE_TIME.saturating_sub(CLAIM_ARRIVAL_MARGIN) / TICKS_PER_HOP
}

pub fn is_valid_home_room(room_data: &RoomData) -> bool {
    if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
        if dynamic_visibility_data.visible() {
            if dynamic_visibility_data.owner().mine() {
                return true;
            }

            if room_data
                .get_structures()
                .map(|structures| structures.spawns().iter().any(|spawn| spawn.my()))
                .unwrap_or(false)
            {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dynamic claim-radius ceiling is derived from claim-creep reach
    /// (life 600, margin 50, 50 ticks/hop) → 11 hops. Pinned so a constant
    /// change is a deliberate, reviewed edit.
    #[test]
    fn max_claim_radius_is_derived_from_claim_creep_reach() {
        assert_eq!(max_claim_radius_hops(), (CREEP_CLAIM_LIFE_TIME - CLAIM_ARRIVAL_MARGIN) / TICKS_PER_HOP);
        assert_eq!(max_claim_radius_hops(), 11);
    }
}
