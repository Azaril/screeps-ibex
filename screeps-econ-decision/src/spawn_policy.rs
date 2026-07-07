//! K4 — the spawn-request policy for the localsupply roles: body shapes, sizing, and
//! priority-band selection. MOVED at ADR 0040 M3 from `screeps-ibex/src/spawnsystem.rs` (the
//! `SPAWN_PRIORITY_*` bands), `missions/localsupply/body_helpers.rs` (`harvester_body`),
//! `missions/localsupply/source_mining.rs` (the harvester energy/priority arms),
//! `missions/haul.rs` (the hauler body/desired/priority arms), `missions/upgrade.rs`
//! (`work_parts_for_upkeep`, the upgrader sizing/priority arms) and `missions/localbuild.rs`
//! (the builder tables + the repairer arm + the builder body cap). Lives here now, consumed by
//! the bot missions (which keep their alive-count/ECS bookkeeping and pass plain facts) and by
//! the sim (`screeps-econ-eval::baseline::spawn_requests`, whose transcriptions are deleted).
//!
//! Body EXPANSION stays the already-shared `screeps_combat_decision::spawning::create_body`
//! (one implementation; the bot re-exports it as `crate::creep::spawning::create_body`) — this
//! module owns the body *definitions* and the *policy* numbers.
//!
//! The S6 defect (capacity-sized replacement bodies head-of-line-banking trickle income) is
//! deliberately preserved — extracted faithfully; M4 owns the fix.

use crate::repair::RepairPriority;
use crate::sink_economics::{body_roi_milli, BID_SCALE};
use screeps::Part;
use screeps_combat_decision::spawning::SpawnBodyDefinition;

// ── The spawn bids (ADR 0040 §D2, M5b — one currency, milli-e/t) ─────────────────────────────
//
// The spawn queue joins the e/t currency at M5b: `SpawnRequest.priority` is now a `u32`
// MILLI-e/t bid (the same [`BID_SCALE`] = 1000 = par lane the M5a transfer market runs on), so
// "what energy FLOWS TO" (transfer) and "what energy BECOMES" (spawn) share one priority
// architecture and the descending head-of-line-banking queue orders both by the same units.
//
// The f32 `SPAWN_PRIORITY_*` bands (100/85/75/50/25/0) are DELETED (EP-2.6). They are replaced
// by these milli bid-equivalents: the OLD band value × [`BID_SCALE`], so every relative ordering
// the bands encoded is preserved BY CONSTRUCTION (`CRITICAL > COMBAT_FORMING > HIGH > MEDIUM >
// LOW > NONE` maps to `100_000 > 85_000 > 75_000 > 50_000 > 25_000 > 0`). Civilian roles whose
// value the ROI kernel can price bid the real ROI ([`body_roi_milli`]) INSIDE their band window
// (mirroring the M5a transfer lane's tier-window idiom, so the S6 cost-amortization is expressed
// without inverting the combat-vs-economy gate the lifecycle harness pins). Coarse roles
// (claim/scout/reserve/salvage) and the body-sizing-coupled builder keep the band-equivalent.

/// The CRITICAL band-equivalent bid (miners / clock-saving upgraders): the top of the civilian
/// spawn lane — income is NEVER preempted (ADR §D2). = old `SPAWN_PRIORITY_CRITICAL` (100) × 1000.
pub const SPAWN_BID_CRITICAL: u32 = 100 * BID_SCALE;
/// The STARTING bid for a FORMING combat squad's slots — the floor of [`forming_completion_bid`].
/// A squad with no members yet bids here (== [`SPAWN_BID_HIGH`]): it competes FAIRLY with the HIGH
/// economy bulk to START, so speculative squads do not preempt the economy just to spawn a first
/// member. Once the squad is COMMITTED (has present members), its remaining slots ESCALATE above
/// this via [`forming_completion_bid`], pricing the lifetime/renew being wasted while incomplete —
/// so it finishes rather than stalling tied-with-economy forever. Never a static band above economy
/// (the M5b "85" starved the economy) nor tied-forever (the roster never completes); the escalation
/// is the atomic-commit middle path.
pub const SPAWN_BID_COMBAT_FORMING: u32 = SPAWN_BID_HIGH;
/// The HIGH economy-bulk band-equivalent bid. = old `SPAWN_PRIORITY_HIGH` (75) × 1000.
pub const SPAWN_BID_HIGH: u32 = 75 * BID_SCALE;
/// The MEDIUM band-equivalent bid. = old `SPAWN_PRIORITY_MEDIUM` (50) × 1000.
pub const SPAWN_BID_MEDIUM: u32 = 50 * BID_SCALE;
/// The LOW band-equivalent bid. = old `SPAWN_PRIORITY_LOW` (25) × 1000.
pub const SPAWN_BID_LOW: u32 = 25 * BID_SCALE;
/// The NONE band-equivalent bid (no demand). = old `SPAWN_PRIORITY_NONE` (0).
pub const SPAWN_BID_NONE: u32 = 0;

/// Milli-e/t of bid escalation per WASTED member-lifetime-tick (one present, idle-at-home forming
/// member, for one tick). The [`forming_completion_bid`] climbs `SPAWN_BID_HIGH → SPAWN_BID_CRITICAL`
/// over the accumulated waste; at this step the window (`CRITICAL - HIGH = 25_000`) is crossed at
/// 1000 wasted member-ticks — e.g. a 4-member roster stalled ~250 ticks, or a lone member stalled
/// ~1000. Tuned in the `run_forming` harness (completion time vs economy-disruption); a bot constant,
/// not serialized — no WFV.
pub const FORMING_WASTE_STEP_MILLI: u32 = 25;

/// **The escalating completion bid for a FORMING combat squad's next slot** — priced on the LIFETIME
/// being WASTED while the roster is incomplete, not a fixed band. `present_members` idle at home each
/// burn their own lifetime (and renew energy) contributing nothing; `ticks_forming` is how long this
/// generation has been forming. Their product is the sunk investment bleeding out — the pressure to
/// FINISH before a give-up wastes it entirely.
///
/// The bid climbs from [`SPAWN_BID_HIGH`] (a just-started squad — 0 present or 0 elapsed — competes
/// FAIRLY with economy; starting a speculative squad must not preempt income) up to but never
/// reaching [`SPAWN_BID_CRITICAL`] (miners/income are NEVER preempted). Properties:
///   * **Self-limiting** — a fresh squad sits at HIGH; only a genuinely-stalling one escalates.
///   * **Self-terminating give-up signal** — once pinned at `CRITICAL - 1` (max escalation) the
///     squad still can't complete only if the blocker is affordability/no-home, not priority, so the
///     caller can retire it (a bounded, principled give-up) rather than escalate forever.
///   * **Prices the real waste** — the "renew time / lifetime ticks wasted on forming" directly, in
///     the market's own currency, instead of the arbitrary M5b `85` band.
/// Pure integer math (saturating; deterministic — no float reaches an ordering).
///
/// The waste accrues as `ticks_forming × (present_members + 1)`: the `+ 1` is the pending slot the
/// squad is always waiting on, so the bid escalates on ELAPSED TIME even at zero present members
/// (the committed objective going unaddressed is itself waste) — otherwise a squad tied with economy
/// could never win the FIRST lane, `present` would stay 0, and the escalation would never bootstrap.
/// Each present member adds proportional urgency (its own sunk lifetime bleeding while it idles).
pub fn forming_completion_bid(present_members: u32, ticks_forming: u32) -> u32 {
    let wasted = (ticks_forming as u64).saturating_mul(present_members as u64 + 1);
    // The escalation window is [HIGH, CRITICAL); never touch CRITICAL (income is never preempted).
    let window = (SPAWN_BID_CRITICAL - SPAWN_BID_HIGH).saturating_sub(1) as u64;
    let escalation = wasted.saturating_mul(FORMING_WASTE_STEP_MILLI as u64).min(window) as u32;
    SPAWN_BID_HIGH + escalation
}

/// Bounded lerp between two u32 bids (integer, saturating — the milli lane never overflows within
/// the band range). Replaces the old f32 `lerp::Lerp::lerp_bounded` on the deleted bands; `t` is
/// clamped to `[0, 1]`. Deterministic integer math (no float reaches an ordering).
pub fn lerp_bid(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    // Interpolate in i64 so a > b (descending lerp) is exact; result is back in the band range.
    let a = a as i64;
    let b = b as i64;
    (a + ((b - a) as f64 * t as f64).round() as i64).max(0) as u32
}

/// A coarse label for a spawn bid (logs/HUD) — the surviving role of the deleted band vocabulary.
/// Maps a milli bid to the nearest band-equivalent name.
pub fn spawn_bid_label(bid_milli: u32) -> &'static str {
    if bid_milli >= SPAWN_BID_CRITICAL {
        "Critical"
    } else if bid_milli >= SPAWN_BID_HIGH {
        // A forming combat squad's slots also land here — `SPAWN_BID_COMBAT_FORMING` shares the
        // HIGH band. Combat spawns are still identifiable by role in the `[SpawnQueue]` log.
        "High"
    } else if bid_milli >= SPAWN_BID_MEDIUM {
        "Medium"
    } else if bid_milli >= SPAWN_BID_LOW {
        "Low"
    } else {
        "None"
    }
}

/// A role tag + body + bid — one K4 spawn request (the `RequestSpawn` intent payload). The `body`
/// is fully expanded (via the shared `create_body`); adapters attach their own callbacks/tokens.
/// `priority` is the MILLI-e/t spawn bid (ADR 0040 §D2, M5b — the unified currency).
#[derive(Clone, Debug)]
pub struct SpawnPlan {
    pub body: Vec<Part>,
    pub priority: u32,
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Harvesters (source_mining.rs + body_helpers.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The harvester body definition ([M,M,C,W] × 1..=5 within `energy`) — body_helpers.rs verbatim.
pub fn harvester_body(energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: energy,
        minimum_repeat: Some(1),
        maximum_repeat: Some(5),
        pre_body: &[],
        repeat_body: &[Part::Move, Part::Move, Part::Carry, Part::Work],
        post_body: &[],
    }
}

/// The harvester body budget: the FIRST harvester (no harvesting creeps anywhere) sizes from
/// available-now energy (floored at the 300 spawn), every replacement from capacity — the S6
/// arm, preserved (source_mining.rs).
pub fn harvester_body_energy(total_harvesting_creeps: usize, energy_available: u32, energy_capacity: u32) -> u32 {
    if total_harvesting_creeps == 0 {
        energy_available.max(300)
    } else {
        energy_capacity
    }
}

/// The per-source desired harvester count (source_mining.rs `desired_harvesters`).
pub const DESIRED_HARVESTERS_PER_SOURCE: usize = 4;

/// The harvester spawn bid (milli-e/t): lerped across the (range-start, range-end) band-equivalent
/// for the home's Manhattan room distance — local (CRITICAL→HIGH), adjacent (MEDIUM→NONE), far
/// (LOW→NONE) (source_mining.rs). Income is the top of the civilian lane (CRITICAL) so it is never
/// preempted (ADR §D2); the lerp fades the bid as the source's roster fills. On the unified
/// milli-e/t currency the ordering the f32 band encoded is preserved by construction (×1000).
pub fn harvester_priority(current: usize, desired: usize, room_manhattan_distance: u32) -> u32 {
    let priority_range = if room_manhattan_distance == 0 {
        (SPAWN_BID_CRITICAL, SPAWN_BID_HIGH)
    } else if room_manhattan_distance <= 1 {
        (SPAWN_BID_MEDIUM, SPAWN_BID_NONE)
    } else {
        (SPAWN_BID_LOW, SPAWN_BID_NONE)
    };
    let interp = (current as f32) / (desired as f32);
    lerp_bid(priority_range.0, priority_range.1, interp)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Haulers (missions/haul.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The hauler body definition: [C,M] × 1..=20; multi-room haulers prepend [W,M] (the road-
/// repair work part) — missions/haul.rs verbatim.
pub fn hauler_body(is_multi_room: bool, energy: u32) -> SpawnBodyDefinition<'static> {
    if is_multi_room {
        SpawnBodyDefinition {
            maximum_energy: energy,
            minimum_repeat: Some(1),
            maximum_repeat: Some(20),
            pre_body: &[Part::Work, Part::Move],
            repeat_body: &[Part::Carry, Part::Move],
            post_body: &[],
        }
    } else {
        SpawnBodyDefinition {
            maximum_energy: energy,
            minimum_repeat: Some(1),
            maximum_repeat: Some(20),
            pre_body: &[],
            repeat_body: &[Part::Carry, Part::Move],
            post_body: &[],
        }
    }
}

/// The hauler demand sizing (missions/haul.rs): `range_multiplier = 1/((max_distance·2)+1)`,
/// `base = carry_parts × CARRY_CAPACITY × multiplier`, desired-for-unfulfilled =
/// `unfulfilled / base` (f32 truncation, live verbatim), capped at `3 + max_distance·3`.
/// Returns `(desired_for_unfulfilled, desired_capped)`.
pub fn hauler_desired(unfulfilled_hauling: u32, carry_parts: u32, max_distance: u32) -> (u32, usize) {
    let range_multiplier = 1.0 / ((max_distance as f32 * 2.0) + 1.0);
    let base_amount = carry_parts as f32 * 50.0 * range_multiplier;
    let max_haulers = 3 + (max_distance * 3);
    let desired_for_unfulfilled = (unfulfilled_hauling as f32 / base_amount) as u32;
    let desired = desired_for_unfulfilled.min(max_haulers) as usize;
    (desired_for_unfulfilled, desired)
}

/// The hauler spawn bid (milli-e/t, missions/haul.rs): below 75% of the unfulfilled-desired count →
/// the urgent band-equivalent (HIGH local / MEDIUM remote), else the relaxed band-equivalent
/// (MEDIUM local / LOW remote). Haulers sit in the economy bulk (below COMBAT_FORMING), where the
/// ROI refinement ([`hauler_bid`]) is safe (never crosses the combat gate).
pub fn hauler_priority(current: usize, desired_for_unfulfilled: u32, max_distance: u32) -> u32 {
    if (current as f32) < (desired_for_unfulfilled as f32 * 0.75).ceil() {
        if max_distance == 0 {
            SPAWN_BID_HIGH
        } else {
            SPAWN_BID_MEDIUM
        }
    } else if max_distance == 0 {
        SPAWN_BID_MEDIUM
    } else {
        SPAWN_BID_LOW
    }
}

/// The hauler spawn ROI bid (ADR §D2, M5b — civilian `body_roi_milli`): the hauler's §D5.4 `w` is
/// its logistics rate (throughput unblocked); amortized over the body cost and clamped only below
/// the CRITICAL miner band (`[SPAWN_BID_LOW, SPAWN_BID_CRITICAL - 1]`). A genuinely stressed
/// logistics lane (high throughput-per-cost) can therefore bid ABOVE the shared HIGH/combat-forming
/// band — logistics is never starved by speculative combat forming, only ever out-ranked by income
/// (miners). `logistics_rate_milli` is the caller's throughput estimate (milli-e/t).
pub fn hauler_bid(current: usize, desired_for_unfulfilled: u32, max_distance: u32, logistics_rate_milli: u32, body_cost: u32) -> u32 {
    let band = hauler_priority(current, desired_for_unfulfilled, max_distance);
    let roi = body_roi_milli(logistics_rate_milli, body_cost);
    // Blend: the ROI refines the ordering WITHIN the economy class. Take the larger of the coarse
    // band and the ROI (a high-throughput cheap hauler bids up), capped only strictly below the
    // CRITICAL miner band so logistics never preempts income but CAN out-rank a forming squad.
    band.max(roi).min(SPAWN_BID_CRITICAL - 1)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Upgraders (missions/upgrade.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Engine constants used by the upkeep model (each cited in engine-mechanics.md; the live code
/// read them from screeps-game-api).
const CONTROLLER_DOWNGRADE_RESTORE: u32 = 100;
const CONTROLLER_MAX_UPGRADE_PER_TICK: u32 = 15;
const CREEP_SPAWN_TIME: u32 = 3;
const CREEP_LIFE_TIME: u32 = 1500;
const CARRY_CAPACITY: u32 = 50;
const UPGRADE_CONTROLLER_POWER: u32 = 1;

/// The minimum WORK parts for an upkeep upgrader to restore the downgrade clock from
/// `current_ttd` back to `max_ticks / 2` within one lifetime (missions/upgrade.rs verbatim —
/// the f64 arithmetic kept; the result is a body size, never a per-tick branch).
pub fn work_parts_for_upkeep(current_ttd: u32, max_ticks: u32) -> usize {
    let safe_threshold = max_ticks / 2;
    if current_ttd >= safe_threshold {
        return 1;
    }
    let deficit = (safe_threshold - current_ttd) as f64;
    let net_restore_per_upgrade_tick = (CONTROLLER_DOWNGRADE_RESTORE as f64) - 1.0;

    for w in 1..=CONTROLLER_MAX_UPGRADE_PER_TICK {
        let body_parts = w + 3;
        let spawn_ticks = body_parts * CREEP_SPAWN_TIME;
        let lifetime = CREEP_LIFE_TIME.saturating_sub(spawn_ticks) as f64;

        let carry_cap = CARRY_CAPACITY as f64;
        let upgrade_ticks_per_cycle = (carry_cap / w as f64).floor();
        if upgrade_ticks_per_cycle < 1.0 {
            continue;
        }
        let cycle_ticks = upgrade_ticks_per_cycle;
        let net_per_cycle = upgrade_ticks_per_cycle * net_restore_per_upgrade_tick;
        if net_per_cycle <= 0.0 {
            continue;
        }

        let cycles = (lifetime / cycle_ticks).floor();
        let total_restore = cycles * net_per_cycle;

        if total_restore >= deficit {
            return w as usize;
        }
    }

    CONTROLLER_MAX_UPGRADE_PER_TICK as usize
}

/// The upgrader roster cap (missions/upgrade.rs): governor-unwilling / hostiles / max-level → 1;
/// excess energy → 5 (RCL ≤ 3) or 3; else 1.
pub fn max_upgraders(governor_willing: bool, hostile_creeps: bool, at_max_level: bool, has_excess_energy: bool, rcl: u8) -> usize {
    if !governor_willing {
        return 1;
    }
    if hostile_creeps || at_max_level {
        1
    } else if has_excess_energy {
        if rcl <= 3 {
            5
        } else {
            3
        }
    } else {
        1
    }
}

/// The upgrader WORK sizing (missions/upgrade.rs): downgrade-risk first-body sized to save the
/// clock; otherwise the max-level cap split / 20-with-excess / half the source potential.
pub fn upgrader_work_parts(
    downgrade_upkeep_parts: Option<usize>,
    roster_empty: bool,
    at_max_level: bool,
    has_excess_energy: bool,
    source_count: usize,
    max_upgraders: usize,
) -> Option<usize> {
    if let Some(upkeep_parts) = downgrade_upkeep_parts {
        if roster_empty {
            Some(upkeep_parts)
        } else {
            let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);
            Some((work_parts_per_tick / (max_upgraders as f32)).ceil() as usize)
        }
    } else if at_max_level {
        let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);
        Some((work_parts_per_tick / (max_upgraders as f32)).ceil() as usize)
    } else if has_excess_energy {
        Some(20)
    } else {
        // Half the room's source potential, split across upgraders (3000/300 e/t per source).
        let energy_per_second = ((3000 * source_count as u32) as f32) / 300.0;
        let upgrade_per_second = energy_per_second / (UPGRADE_CONTROLLER_POWER as f32);
        Some(((upgrade_per_second / 2.0) / max_upgraders as f32).floor().max(1.0) as usize)
    }
}

/// The upgrader body definition (missions/upgrade.rs): RCL ≤ 3 → pre `[W,C,M,M]`, repeat
/// `[W,M]` × 0..=work_parts; RCL > 3 → pre `[W,C,M,M]`, repeat `[W]` × 1..=(work_parts − 1).
pub fn upgrader_body(rcl: u8, maximum_energy: u32, work_parts: Option<usize>) -> SpawnBodyDefinition<'static> {
    if rcl <= 3 {
        SpawnBodyDefinition {
            maximum_energy,
            minimum_repeat: Some(0),
            maximum_repeat: work_parts,
            pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
            repeat_body: &[Part::Work, Part::Move],
            post_body: &[],
        }
    } else {
        SpawnBodyDefinition {
            maximum_energy,
            minimum_repeat: Some(1),
            maximum_repeat: work_parts.map(|p| p.saturating_sub(1)),
            pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
            repeat_body: &[Part::Work],
            post_body: &[],
        }
    }
}

/// The upgrader spawn bid (milli-e/t, missions/upgrade.rs): downgrade-risk-with-empty-roster
/// CRITICAL (a survival-class clock save — never preempted); empty roster HIGH; excess+storage
/// lerp HIGH→MEDIUM; multi lerp MEDIUM→LOW; else MEDIUM. Band-equivalents on the unified currency
/// (×1000), so the ordering the f32 bands encoded is preserved.
pub fn upgrader_priority(
    downgrade_risk: bool,
    roster_empty: bool,
    has_excess_energy: bool,
    has_storage: bool,
    max_upgraders: usize,
    alive_upgraders: usize,
) -> u32 {
    if downgrade_risk && roster_empty {
        SPAWN_BID_CRITICAL
    } else if roster_empty {
        SPAWN_BID_HIGH
    } else if has_excess_energy && has_storage && max_upgraders > 1 {
        let interp = (alive_upgraders as f32) / ((max_upgraders - 1) as f32);
        lerp_bid(SPAWN_BID_HIGH, SPAWN_BID_MEDIUM, interp)
    } else if max_upgraders > 1 {
        let interp = (alive_upgraders as f32) / ((max_upgraders - 1) as f32);
        lerp_bid(SPAWN_BID_MEDIUM, SPAWN_BID_LOW, interp)
    } else {
        SPAWN_BID_MEDIUM
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Builders + the repairer arm (missions/localbuild.rs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The builder count table by pending construction progress + RCL band
/// (missions/localbuild.rs).
pub fn builder_desired_for_progress(rcl: u8, required_progress: u32) -> u32 {
    if rcl <= 3 {
        match required_progress {
            0 => 0,
            1..=1000 => 1,
            1001..=2000 => 2,
            2001..=3000 => 3,
            3001..=4000 => 4,
            _ => 5,
        }
    } else if rcl <= 6 {
        match required_progress {
            0 => 0,
            1..=2000 => 1,
            2001..=4000 => 2,
            4001..=6000 => 3,
            _ => 4,
        }
    } else {
        match required_progress {
            0 => 0,
            1..=3000 => 1,
            3001..=6000 => 2,
            6001..=9000 => 3,
            _ => 4,
        }
    }
}

/// The first-builder spawn bid: (HIGH + MEDIUM) / 2 = 62_500 milli (missions/localbuild.rs).
pub const FIRST_BUILDER_PRIORITY: u32 = (SPAWN_BID_HIGH + SPAWN_BID_MEDIUM) / 2;

/// The with-builders priority: HIGH iff any spawn/storage site is pending, else MEDIUM
/// (missions/localbuild.rs — the per-site max collapses to this).
pub fn builder_priority_with_builders(any_spawn_or_storage_site: bool) -> u32 {
    if any_spawn_or_storage_site {
        SPAWN_BID_HIGH
    } else {
        SPAWN_BID_MEDIUM
    }
}

/// The repairer-builder arm (missions/localbuild.rs `get_repairer_priority` tail): the queue's
/// best candidate at the allowance-raised minimum decides — ≥ High → (1, HIGH); ≥ Medium →
/// (1, MEDIUM); else none.
pub fn repairer_spawn_priority(best_candidate: RepairPriority) -> Option<(u32, u32)> {
    if best_candidate >= RepairPriority::High {
        Some((1, SPAWN_BID_HIGH))
    } else if best_candidate >= RepairPriority::Medium {
        Some((1, SPAWN_BID_MEDIUM))
    } else {
        None
    }
}

/// The builder body definition (missions/localbuild.rs): repeat `[C,W,M,M]` × 1.., capped at 5
/// repeats below HIGH priority, uncapped at ≥ HIGH.
pub fn builder_body(maximum_energy: u32, spawn_bid: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy,
        minimum_repeat: Some(1),
        maximum_repeat: if spawn_bid >= SPAWN_BID_HIGH { None } else { Some(5) },
        pre_body: &[],
        repeat_body: &[Part::Carry, Part::Work, Part::Move, Part::Move],
        post_body: &[],
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Energy posture thresholds (missions/upgrade.rs `has_excess_energy` / missions/localbuild.rs
// `has_sufficient_energy`).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The desired storage buffer the excess/sufficient fractions divide
/// (missions/constants.rs `get_desired_storage_amount(Energy)`).
pub const DESIRED_STORAGE_ENERGY: u32 = 200_000;
/// Container capacity (engine constant; the fraction ladders' denominator).
pub const CONTAINER_CAPACITY: u32 = 2000;

/// `has_excess_energy` (missions/upgrade.rs): storage present → Σ storage energy ≥ 100k; else
/// containers present → ANY container > 75% full; else TRUE (a bare room reads "excess").
/// `container_energies` are per-container energy amounts.
pub fn has_excess_energy(storage_present: bool, total_storage_energy: u32, container_energies: &[u32]) -> bool {
    if storage_present {
        total_storage_energy >= DESIRED_STORAGE_ENERGY / 2
    } else if !container_energies.is_empty() {
        container_energies.iter().any(|&e| e as u64 * 100 > CONTAINER_CAPACITY as u64 * 75)
    } else {
        true
    }
}

/// `has_sufficient_energy` (missions/localbuild.rs): storage present → ANY storage ≥ 50k; else
/// ANY container > 50% full (an empty candidate set is false — the greenfield RCL-1 room reads
/// insufficient). `storage_energies` are per-storage energy amounts.
pub fn has_sufficient_energy(storage_present: bool, storage_energies: &[u32], container_energies: &[u32]) -> bool {
    if storage_present {
        storage_energies.iter().any(|&e| e >= DESIRED_STORAGE_ENERGY / 4)
    } else {
        container_energies.iter().any(|&e| e as u64 * 100 > CONTAINER_CAPACITY as u64 * 50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_combat_decision::spawning::create_body;

    /// The harvester arm end-to-end shapes (pre-move fixtures, source_mining.rs +
    /// body_helpers.rs): bootstrap available-sized, replacement capacity-sized (S6),
    /// CRITICAL→HIGH lerp.
    #[test]
    fn harvester_policy_matches_live() {
        assert_eq!(harvester_body_energy(0, 250, 800), 300, "bootstrap floors at the bare spawn");
        assert_eq!(harvester_body_energy(0, 450, 800), 450);
        assert_eq!(harvester_body_energy(1, 450, 800), 800, "replacement is capacity-sized — S6 preserved");
        let body = create_body(&harvester_body(300)).unwrap();
        assert_eq!(body.len(), 4, "1 repeat of [M,M,C,W] at 300");
        let body = create_body(&harvester_body(800)).unwrap();
        assert_eq!(body.len(), 12, "3 repeats at 800");
        assert_eq!(harvester_priority(0, 4, 0), SPAWN_BID_CRITICAL);
        assert_eq!(harvester_priority(1, 4, 0), 93_750, "1/4 lerp toward HIGH (milli)");
        assert_eq!(harvester_priority(0, 4, 1), SPAWN_BID_MEDIUM, "adjacent-room band");
        assert_eq!(harvester_priority(0, 4, 2), SPAWN_BID_LOW, "far band");
    }

    /// The hauler arm (pre-move fixtures, missions/haul.rs): local body, desired sizing,
    /// priority bands incl. the remote arms.
    #[test]
    fn hauler_policy_matches_live() {
        let body = create_body(&hauler_body(false, 300)).unwrap();
        assert_eq!(body.len(), 6, "3 repeats of [C,M] at 300");
        let body = create_body(&hauler_body(true, 300)).unwrap();
        assert_eq!(body.iter().filter(|p| **p == Part::Work).count(), 1, "multi-room prepends [W,M]");
        // 800 unfulfilled / (3 carry × 50) = 5 → desired for unfulfilled 5, capped at 3 local.
        assert_eq!(hauler_desired(800, 3, 0), (5, 3));
        // Remote distance 1: multiplier 1/3 → base 50, 800/50 = 16, cap 6.
        assert_eq!(hauler_desired(800, 3, 1), (16, 6));
        assert_eq!(hauler_priority(0, 5, 0), SPAWN_BID_HIGH);
        assert_eq!(hauler_priority(4, 5, 0), SPAWN_BID_MEDIUM, "≥ ceil(75%) of desired");
        assert_eq!(hauler_priority(0, 5, 1), SPAWN_BID_MEDIUM, "remote urgent band");
        assert_eq!(hauler_priority(4, 5, 1), SPAWN_BID_LOW, "remote relaxed band");
        // ROI refinement (M5b): a cheap high-throughput hauler bids up. It is capped only below the
        // CRITICAL miner band, so a genuinely stressed logistics lane can out-rank a forming combat
        // squad (combat must not starve the economy) while income (miners) is still never preempted.
        assert!(hauler_bid(0, 5, 0, 8_000, 300) >= SPAWN_BID_HIGH, "a strong-ROI hauler bids at least its band");
        assert!(
            hauler_bid(0, 5, 0, 60_000, 1_000) > SPAWN_BID_COMBAT_FORMING,
            "a genuinely high-throughput hauler can now out-bid speculative combat forming"
        );
        assert!(hauler_bid(0, 5, 0, 60_000, 1_000) < SPAWN_BID_CRITICAL, "but logistics never preempts income (miners)");
    }

    /// The upkeep sizing (pre-move fixture, missions/upgrade.rs): at/above half-max → 1 WORK;
    /// every realizable deficit fits in 1 WORK (the live loop exists for the parameter shape).
    #[test]
    fn work_parts_for_upkeep_matches_live_math() {
        assert_eq!(work_parts_for_upkeep(10_000, 20_000), 1, "at the safe threshold: 1");
        assert_eq!(work_parts_for_upkeep(2_000, 20_000), 1, "RCL-3 at 10%");
        assert_eq!(work_parts_for_upkeep(0, 200_000), 1, "even the RCL-8 full deficit");
    }

    /// The upgrader bodies (pre-move fixtures, missions/upgrade.rs).
    #[test]
    fn upgrader_bodies_match_live_definitions() {
        let b = create_body(&upgrader_body(3, 300, Some(10))).unwrap();
        assert_eq!(b, vec![Part::Work, Part::Carry, Part::Move, Part::Move], "min repeat 0 at the floor");
        let b = create_body(&upgrader_body(3, 800, Some(10))).unwrap();
        assert_eq!(b.iter().filter(|p| **p == Part::Work).count(), 4, "3 repeats of [W,M] within 800");
        let b = create_body(&upgrader_body(4, 800, Some(20))).unwrap();
        assert_eq!(b.iter().filter(|p| **p == Part::Work).count(), 6, "pre W + 5 repeat W within 800");
        assert!(create_body(&upgrader_body(4, 300, Some(20))).is_err(), "RCL>3 needs ≥ 350 (min repeat 1)");
    }

    /// The upgrader roster/sizing/priority arms (pre-move fixtures, missions/upgrade.rs).
    #[test]
    fn upgrader_policy_matches_live() {
        assert_eq!(max_upgraders(false, false, false, true, 2), 1, "governor-unwilling caps at 1");
        assert_eq!(max_upgraders(true, true, false, true, 2), 1);
        assert_eq!(max_upgraders(true, false, true, true, 8), 1);
        assert_eq!(max_upgraders(true, false, false, true, 3), 5);
        assert_eq!(max_upgraders(true, false, false, true, 4), 3);
        assert_eq!(max_upgraders(true, false, false, false, 3), 1);

        assert_eq!(upgrader_work_parts(Some(3), true, false, false, 2, 1), Some(3), "clock-saving first body");
        assert_eq!(upgrader_work_parts(Some(3), false, false, false, 2, 3), Some(5), "replacement: ceil(15/3)");
        assert_eq!(upgrader_work_parts(None, true, true, false, 2, 1), Some(15), "max-level cap");
        assert_eq!(upgrader_work_parts(None, true, false, true, 2, 3), Some(20), "excess");
        // 2 sources: 20 e/t, half = 10, / 1 upgrader = 10.
        assert_eq!(upgrader_work_parts(None, true, false, false, 2, 1), Some(10));

        assert_eq!(upgrader_priority(true, true, false, false, 1, 0), SPAWN_BID_CRITICAL);
        assert_eq!(upgrader_priority(false, true, true, true, 3, 0), SPAWN_BID_HIGH, "empty roster");
        assert_eq!(upgrader_priority(false, false, true, true, 3, 2), SPAWN_BID_MEDIUM, "full lerp");
        assert_eq!(upgrader_priority(false, false, false, false, 1, 1), SPAWN_BID_MEDIUM);
    }

    /// The builder tables + the repairer arm + the body cap (pre-move fixtures,
    /// missions/localbuild.rs).
    #[test]
    fn builder_policy_matches_live() {
        assert_eq!(builder_desired_for_progress(3, 0), 0);
        assert_eq!(builder_desired_for_progress(3, 3000), 3);
        assert_eq!(builder_desired_for_progress(3, 4001), 5);
        assert_eq!(builder_desired_for_progress(5, 3000), 2);
        assert_eq!(builder_desired_for_progress(8, 3000), 1);
        assert_eq!(FIRST_BUILDER_PRIORITY, 62_500);
        assert_eq!(builder_priority_with_builders(true), SPAWN_BID_HIGH);
        assert_eq!(builder_priority_with_builders(false), SPAWN_BID_MEDIUM);

        assert_eq!(repairer_spawn_priority(RepairPriority::Critical), Some((1, SPAWN_BID_HIGH)));
        assert_eq!(repairer_spawn_priority(RepairPriority::High), Some((1, SPAWN_BID_HIGH)));
        assert_eq!(repairer_spawn_priority(RepairPriority::Medium), Some((1, SPAWN_BID_MEDIUM)));
        assert_eq!(repairer_spawn_priority(RepairPriority::Low), None);

        let b = create_body(&builder_body(10_000, SPAWN_BID_MEDIUM)).unwrap();
        assert_eq!(b.len(), 20, "5 repeats × 4 parts below HIGH");
        let b = create_body(&builder_body(10_000, SPAWN_BID_HIGH)).unwrap();
        assert!(b.len() > 20, "≥ HIGH: uncapped repeats");
    }

    /// The excess/sufficient thresholds incl. the bare-room split (pre-move fixtures).
    #[test]
    fn excess_and_sufficient_energy_thresholds() {
        assert!(has_excess_energy(false, 0, &[]), "bare room: excess TRUE");
        assert!(!has_sufficient_energy(false, &[], &[]), "bare room: sufficient FALSE");
        assert!(!has_excess_energy(true, 99_999, &[]));
        assert!(has_excess_energy(true, 100_000, &[]));
        assert!(has_sufficient_energy(true, &[50_000], &[]));
        assert!(!has_sufficient_energy(true, &[49_999], &[]));
        assert!(!has_excess_energy(false, 0, &[1500]), "exactly 75% is NOT > 75%");
        assert!(has_excess_energy(false, 0, &[1501]));
        assert!(has_sufficient_energy(false, &[], &[1001]));
        assert!(!has_sufficient_energy(false, &[], &[1000]), "exactly 50% is NOT > 50%");
    }

    /// M5b spawn-currency: the band-equivalents preserve the economy ordering the deleted f32 bands
    /// encoded (CRITICAL > HIGH = COMBAT_FORMING > MEDIUM > LOW > NONE), on the milli-e/t lane
    /// (× BID_SCALE), so the descending head-of-line-banking queue orders spawns by the same units
    /// the M5a transfer market runs on — one currency. Combat forming SHARES the HIGH band (it must
    /// not starve the economy); only CRITICAL income and a stressed logistics ROI out-rank it.
    #[test]
    fn spawn_bid_band_equivalents_preserve_the_ordering() {
        assert_eq!(SPAWN_BID_CRITICAL, 100_000);
        assert_eq!(SPAWN_BID_HIGH, 75_000);
        assert_eq!(SPAWN_BID_MEDIUM, 50_000);
        assert_eq!(SPAWN_BID_LOW, 25_000);
        assert_eq!(SPAWN_BID_NONE, 0);
        assert!(SPAWN_BID_CRITICAL > SPAWN_BID_COMBAT_FORMING, "income is never preempted");
        assert_eq!(SPAWN_BID_COMBAT_FORMING, SPAWN_BID_HIGH, "a forming squad SHARES the HIGH band — it must not starve the economy");
        assert!(SPAWN_BID_HIGH > SPAWN_BID_MEDIUM);
        assert!(SPAWN_BID_MEDIUM > SPAWN_BID_LOW);
        assert!(SPAWN_BID_LOW > SPAWN_BID_NONE);
    }

    #[test]
    fn forming_completion_bid_escalates_with_wasted_lifetime() {
        // At the instant of fielding (zero elapsed) the bid starts at HIGH — it competes with economy.
        assert_eq!(forming_completion_bid(0, 0), SPAWN_BID_HIGH, "zero elapsed ⇒ start at HIGH");
        assert_eq!(forming_completion_bid(3, 0), SPAWN_BID_HIGH, "zero elapsed ⇒ start at HIGH regardless of roster");

        // BOOTSTRAP: escalates on elapsed time even at ZERO present members — otherwise a squad tied
        // with economy could never win its FIRST lane and the escalation would never start.
        assert!(
            forming_completion_bid(0, 10) > SPAWN_BID_HIGH,
            "a squad that can't even start must escalate over time to win the first lane"
        );

        // Escalates further as lifetime is wasted (elapsed time × the roster it is bleeding).
        let a = forming_completion_bid(2, 50);
        let b = forming_completion_bid(2, 100);
        assert!(b > a, "more elapsed forming time ⇒ a higher completion bid ({b} > {a})");

        // More PRESENT members (more sunk investment at risk) escalate faster for the same elapsed time.
        assert!(
            forming_completion_bid(4, 50) > forming_completion_bid(2, 50),
            "more members waiting ⇒ more at stake ⇒ higher bid"
        );

        // Pinned STRICTLY below CRITICAL — income (miners) is never preempted, however long it stalls.
        let maxed = forming_completion_bid(8, 100_000);
        assert!(maxed < SPAWN_BID_CRITICAL, "escalation never reaches CRITICAL ({maxed})");
        assert_eq!(maxed, SPAWN_BID_CRITICAL - 1, "max escalation pins just below CRITICAL (the give-up signal)");
    }

    /// `lerp_bid` is a deterministic integer lerp (descending band lerps are exact) and the label
    /// helper maps a bid back to its coarse band name (the deleted enum's surviving role).
    #[test]
    fn lerp_bid_and_label() {
        assert_eq!(lerp_bid(SPAWN_BID_CRITICAL, SPAWN_BID_HIGH, 0.0), SPAWN_BID_CRITICAL);
        assert_eq!(lerp_bid(SPAWN_BID_CRITICAL, SPAWN_BID_HIGH, 1.0), SPAWN_BID_HIGH);
        assert_eq!(lerp_bid(SPAWN_BID_CRITICAL, SPAWN_BID_HIGH, 0.25), 93_750, "1/4 toward HIGH");
        assert_eq!(lerp_bid(SPAWN_BID_MEDIUM, SPAWN_BID_LOW, 2.0), SPAWN_BID_LOW, "t clamps to 1");
        assert_eq!(spawn_bid_label(SPAWN_BID_CRITICAL), "Critical");
        assert_eq!(spawn_bid_label(SPAWN_BID_COMBAT_FORMING), "High", "forming shares the HIGH band");
        assert_eq!(spawn_bid_label(SPAWN_BID_HIGH), "High");
        assert_eq!(spawn_bid_label(SPAWN_BID_MEDIUM), "Medium");
        assert_eq!(spawn_bid_label(SPAWN_BID_LOW), "Low");
        assert_eq!(spawn_bid_label(SPAWN_BID_NONE), "None");
    }
}
