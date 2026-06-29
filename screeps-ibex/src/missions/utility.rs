use crate::military::threatmap::{RoomThreatData, ThreatLevel};
use crate::pathing::pathfinderservice::PathfinderService;
use crate::room::data::*;
use screeps::*;

// ── Pre-claim safety gate (ADR 0017) ────────────────────────────────────────

/// Whether a claim target is safe to commit a claimer into. Consumes the cheap
/// always-cached dynamic-visibility signals plus the richer `RoomThreatData`.
///
/// Principle (ADR 0017): *absence of fresh intel is NOT safety.* Stale/missing
/// visibility reads as unsafe so we re-scout rather than commit a claimer
/// blind into a room that looked clean on a single old scout. The escort/
/// pre-clear path for *marginal* rooms is deferred (squad-system overhaul), so
/// until then ANY live threat signal → unsafe (reject), which is the
/// conservative choice.
pub fn is_claim_target_safe(threat: Option<&RoomThreatData>, dynamic: &RoomDynamicVisibilityData, intel_freshness_ticks: u32) -> bool {
    // Fresh intel required — a clean read older than the window is not trusted.
    if !dynamic.updated_within(intel_freshness_ticks) {
        return false;
    }
    // A foreign owner or reservation blocks claimController outright
    // (ERR_INVALID_TARGET), and any militarised presence means the room is
    // contested. `militarily_active` covers hostile combat creeps, active
    // hostile spawns, and armed hostile towers.
    if dynamic.owner().hostile() || dynamic.owner().friendly() {
        return false;
    }
    if dynamic.reservation().hostile() || dynamic.reservation().friendly() {
        return false;
    }
    if dynamic.militarily_active() || dynamic.tower_dps_at_edge().is_some() {
        return false;
    }
    // Rich threat signal when a component is present (a parked enemy combat
    // creep classifies as at least PlayerRaid).
    if let Some(t) = threat {
        if t.threat_level >= ThreatLevel::PlayerRaid
            || t.estimated_attack_dps > 0.0
            || !t.hostile_tower_positions.is_empty()
            || !t.incoming_nukes.is_empty()
        {
            return false;
        }
    }
    true
}

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

/// Whether to abandon (un-claim) a nascent colony that is losing a contested
/// claim (ADR 0017). The robust, un-gameable rule: a **spawnless** owned room
/// (it never built its spawn / can't defend itself — no towers until RCL3)
/// under **sustained player-hostile presence**, or being actively
/// `attackController`-ed (`upgrade_blocked`, which also freezes our upgrade so
/// we can never earn a safe-mode charge → unwinnable). Never abandons a room
/// that has its own spawn or is our only colony. Pure, host-testable.
pub fn should_abandon_claim(
    spawnless: bool,
    is_only_colony: bool,
    contested_persist_ticks: u32,
    upgrade_blocked: bool,
    abort_persistence_ticks: u32,
) -> bool {
    // Established (has a spawn) or last-stand (only colony) rooms are never
    // abandoned — defend them instead.
    if !spawnless || is_only_colony {
        return false;
    }
    // Decisive: an enemy is neutralizing the controller. Otherwise: a hostile
    // that has held past the anti-flap window.
    upgrade_blocked || contested_persist_ticks >= abort_persistence_ticks
}

// ── Remote-room safety gate (replaces the vestigial DefendMission) ───────────

/// Whether a remote mining outpost room is safe to keep spawning economy creeps
/// (reservers / miners / haulers) into. Pure predicate over the room's dynamic
/// visibility intel — the kernel that the old `DefendMission::is_room_safe()`
/// state machine collapsed to (ADR 0027 P3). It spawned nothing and held no
/// real squad; its only output was this boolean.
///
/// The room is UNSAFE only while we currently see an ACTUAL threat —
/// combat-capable hostile creeps, active hostile spawns, or armed active towers
/// (`militarily_active`). It is NOT made unsafe by inert hostile structures
/// (leftover enemy walls / ramparts / husks in a neutral remote, e.g. a
/// de-claimed derelict room can't hurt a creep); treating those as an attack
/// kept the room unsafe forever and permanently gated off the outpost's
/// reserver / miners / haulers (the post-`4fae295` fix this preserves).
///
/// Absence of fresh intel reads SAFE — matching the old consumer's
/// `unwrap_or(true)` fallback (a brief vision gap, with the outpost's own
/// miners keeping the room visible, must not freeze economy spawning). The old
/// state machine added a 20-tick de-escalation debounce backed by cross-tick
/// state; a pure predicate over current intel intentionally drops that latch —
/// the gate flips back to safe the moment we observe the threat is gone, which
/// is the per-tick-optimal behavior (no hysteresis unless oscillation is
/// actually observed).
pub fn is_remote_room_safe(dynamic: Option<&RoomDynamicVisibilityData>) -> bool {
    match dynamic {
        Some(d) => !(d.visible() && d.militarily_active()),
        None => true,
    }
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
        assert_eq!(
            max_claim_radius_hops(),
            (CREEP_CLAIM_LIFE_TIME - CLAIM_ARRIVAL_MARGIN) / TICKS_PER_HOP
        );
        assert_eq!(max_claim_radius_hops(), 11);
    }

    // ── Contested-claim abort predicate (ADR 0017) ──────────────────────────

    const PERSIST: u32 = 50;

    #[test]
    fn abort_never_for_spawned_or_only_colony() {
        // Has its own spawn → defend, never abandon (even if hammered).
        assert!(!should_abandon_claim(false, false, 10_000, true, PERSIST));
        // Spawnless but our only colony → last stand, never abandon.
        assert!(!should_abandon_claim(true, true, 10_000, true, PERSIST));
    }

    #[test]
    fn abort_on_sustained_hostile_past_window() {
        // Spawnless expansion, not the only colony, hostile held past the window.
        assert!(should_abandon_claim(true, false, PERSIST, false, PERSIST));
        // Within the anti-flap window → hold (don't abandon yet).
        assert!(!should_abandon_claim(true, false, PERSIST - 1, false, PERSIST));
        // No hostile, no attackController → never abandon a merely-slow room.
        assert!(!should_abandon_claim(true, false, 0, false, PERSIST));
    }

    #[test]
    fn abort_immediately_when_controller_attacked() {
        // upgrade_blocked (enemy attackController) is decisive, even with no
        // sustained-hostile persistence yet — it freezes our upgrade, unwinnable.
        assert!(should_abandon_claim(true, false, 0, true, PERSIST));
    }
}
