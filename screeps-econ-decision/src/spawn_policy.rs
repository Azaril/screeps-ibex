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
//! The S6 defect (capacity-sized replacement bodies head-of-line-banking trickle income) was
//! preserved through the M3 extraction; it is FIXED here (RULING-11 root B, 2026-09-07) by
//! [`replacement_body_energy`] — the per-tick starvation sizing — together with the income ladder
//! ([`SPAWN_BID_MINER`] / the bootstrap floors) and the need-scaled [`hauler_bid`].

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
/// Every other civilian/combat bid is capped STRICTLY below this ([`hauler_bid`],
/// [`forming_completion_bid`]); only the two BOOTSTRAP floors below sit above it.
pub const SPAWN_BID_CRITICAL: u32 = 100 * BID_SCALE;
/// The static-miner bid (link / container miners): the CRITICAL income band — income out-ranks
/// logistics (the [`hauler_bid`] cap) and every combat-forming slot by construction. RULING-11
/// root B (2026-09-07): the live callers had drifted to HIGH (75_000) while the comments, the
/// forming-band docs and the squad-manager pins all asserted CRITICAL — so an 1800e capacity-sized
/// hauler at 99_999 head-of-line-banked over a 550e miner with full containers behind it. One
/// constant, consumed by the callers; the contract is now what the pins say.
pub const SPAWN_BID_MINER: u32 = SPAWN_BID_CRITICAL;
/// The FIRST LOCAL HAULER's bid (an empty hauler roster): strictly above the miner band. A room
/// with stock in its containers/links/storage and no carrier has no lane inflow at all — a
/// capacity-sized miner at CRITICAL would head-of-line-bank at the 300 the spawn regenerates and
/// the affordable 300e carrier behind it would never be looked at (the queue `break`s on the
/// first unaffordable head). The first carrier is what turns stock into lane; it is sized from
/// available-now energy (always fieldable), so it never banks.
pub const SPAWN_BID_BOOTSTRAP_HAULER: u32 = SPAWN_BID_CRITICAL + BID_SCALE;
/// The RESTART harvester's bid (a local source with NO harvesting creep of any kind): the single
/// self-sufficient body (mines AND delivers) that can restart an empty room, strictly above both
/// the first-hauler floor and the miner band — so a bid TIE with a capacity-sized miner (which
/// would resolve by registration order) can never put the miner at the head and bank the lane
/// at zero income. Sized from available-now energy ([`harvester_body_energy`]); never banks.
pub const SPAWN_BID_BOOTSTRAP_HARVESTER: u32 = SPAWN_BID_CRITICAL + 2 * BID_SCALE;
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

/// **The forming-completion bid for a FORMING combat squad's slots** (ADR 0042, the R_net model) —
/// priced on the objective's REAL COMPLETED VALUE, not a band ordinal and not time-stalled.
/// `r_o_completed_milli` is the objective's rate for the squad we are forming:
/// `round(p_win_completed · value_e · 1000 / est_ticks)` — the same `military_priority_bid` numerator
/// the movement lane uses, but with `p_win` over the FULL requested roster's caps (not the members
/// present so far — else a 0-present squad would price ~0 and never bootstrap its first member).
///
/// The bid ORDERS `r_o_completed_milli` WITHIN a reserved sub-CRITICAL band `[HIGH, CRITICAL)`:
///   * never below [`SPAWN_BID_HIGH`] — so member 1 always wins its lane against the economy bulk,
///     AND no `>= SPAWN_BID_HIGH` consumer / `spawn_bid_label` classification silently flips;
///   * never reaching [`SPAWN_BID_CRITICAL`] — income/miners are NEVER preempted;
///   * a higher-value objective bids higher within the band (a base-under-assault outbids a mediocre
///     remote), instead of every objective pinning at the cap the way a raw
///     `p_win·value_e·1000/est_ticks` would (`value_e` runs to ~1e6, so the raw rate saturates).
///
/// The give-up decision (ADR 0042 §5) reads the SAME `r_o_completed_milli` against the burn + the
/// economy's opportunity floor — one quantity governs both bid and abandon.
/// Pure integer math (saturating; deterministic — no float reaches an ordering). Caller applies the
/// CRITICAL-defense intra-band edge (a bot constant) on top of this and re-clamps below CRITICAL.
pub fn forming_completion_bid(r_o_completed_milli: u32) -> u32 {
    // Order the completed rate within the reserved band; leave the top slot for CRITICAL (miners).
    let window = SPAWN_BID_CRITICAL - SPAWN_BID_HIGH; // 25_000
    let escal = r_o_completed_milli.min(window - 1);
    SPAWN_BID_HIGH + escal // ∈ [SPAWN_BID_HIGH, SPAWN_BID_CRITICAL - 1]
}

/// The per-tick BURN a forming squad bleeds while incomplete, in milli-e/t (ADR 0042 §4). Every present
/// member sitting idle at home consumes its own lifetime doing nothing — whether it decays or is renewed
/// costs the SAME `body_cost / CREEP_LIFE_TIME` e/t (the renew-spend ≡ idle-amortization identity), so the
/// two are ONE flow, charged once: `roster_present_cost_e / 1500 e/t = round(cost · 2 / 3)` milli-e/t. The
/// already-spent (sunk) body cost is NOT here — it is a stock realized only on abandon, at salvage.
pub fn forming_burn_rate_milli(roster_present_cost_e: u32) -> u32 {
    // cost/1500 e/t → ×1000 milli = cost·1000/1500 = cost·2/3, rounded.
    ((roster_present_cost_e as u64 * 2 + 1) / 3) as u32
}

/// The forming-squad ECONOMIC give-up test (ADR 0042 §5) — abandon iff finishing the squad delivers less
/// rate than it costs to keep forming. Both sides are milli-e/t RATES (commensurable): the completed
/// objective's rate `r_o_completed_milli` (`p_win_completed·value_e/est_ticks`) vs the burn of holding the
/// present roster (`forming_burn_rate_milli`) PLUS the economy's opportunity floor (the marginal civilian
/// rate the spawn energy would otherwise earn). `opportunity_floor_milli = 0` is the sound conservative
/// lower bound (abandon only when strictly value-NEGATIVE — the completed rate can't even cover the burn);
/// a positive floor (the civilian alternative, once the civilian lane is true-EV — ADR 0043 A2) raises the
/// bar. The caller K-tick-latches this (kill per-tick oscillation) and exempts a safe-moded target (a
/// bounded window, not permanent unwinnability). Pure; no float reaches an ordering.
pub fn should_abandon_forming(r_o_completed_milli: u32, burn_milli: u32, opportunity_floor_milli: u32) -> bool {
    r_o_completed_milli < burn_milli.saturating_add(opportunity_floor_milli)
}

/// ADR 0042 §5 / ADR 0043 A3 — consecutive reconciles the ECONOMIC forming give-up must hold before it
/// fires (`should_abandon_forming` true: the completed objective's rate can't cover the present roster's
/// burn). K-tick latch so a transient p_win/intel dip does not abandon a valuable squad; small (a fraction
/// of a creep life) so a genuinely worthless/unwinnable objective is dropped in ~a scout cycle, LONG before
/// the manager's `MAX_FORMING_BUDGET` (3000) liveness backstop — the demotion the R_net give-up is about.
/// SHARED (parity M23): the live `SquadManager` and the offline lifecycle harness latch on this ONE
/// constant through [`EconomicGiveUp`] — never a mirrored copy.
pub const FORMING_ABANDON_STREAK: u32 = 20;

/// The K-tick latch over [`should_abandon_forming`] (ADR 0042 §5) — the per-objective streak of
/// consecutive reconciles the economic give-up has held. Pure, `Copy`, ephemeral (the live manager keeps
/// one per forming objective in its NON-serialized runtime resource; the harness keeps one per driver
/// generation) — a VM reload restarts the streak, still bounded. Reset on any covering reconcile (the
/// completed rate covers the burn again) and by the caller whenever the squad stops forming / re-fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EconomicGiveUp {
    /// Consecutive reconciles `abandon_now` has held.
    pub streak: u32,
}

impl EconomicGiveUp {
    /// Advance one reconcile with this tick's `abandon_now` (= `!target_safe_mode &&
    /// should_abandon_forming(r_o, burn, floor)` — the caller composes the safe-mode exemption, a bounded
    /// window that is not permanent unwinnability). Returns whether the give-up has FIRED: the streak has
    /// reached [`FORMING_ABANDON_STREAK`]. A covering tick resets the streak to zero (no hysteresis — the
    /// latch is the K-tick debounce the ADR specifies, not a relaxed re-arm).
    pub fn advance(&mut self, abandon_now: bool) -> bool {
        self.streak = if abandon_now { self.streak.saturating_add(1) } else { 0 };
        self.streak >= FORMING_ABANDON_STREAK
    }
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
/// available-now energy (floored at the 300 spawn), every replacement from capacity
/// (source_mining.rs). The live caller runs the capacity arm through
/// [`replacement_body_energy`] (the starvation sizing); this is the bootstrap arm + the
/// steady-state target the sim baseline consumes verbatim.
pub fn harvester_body_energy(total_harvesting_creeps: usize, energy_available: u32, energy_capacity: u32) -> u32 {
    if total_harvesting_creeps == 0 {
        energy_available.max(SPAWN_LANE_REGEN_FLOOR_E)
    } else {
        energy_capacity
    }
}

/// The energy a room's spawn lane regenerates to for FREE (engine: every spawn gains +1 e/t
/// while the room's `energyAvailable` is below `SPAWN_ENERGY_CAPACITY` = 300). A body costing
/// ≤ 300 is therefore ALWAYS fieldable with zero income; anything above it needs a carrier.
pub const SPAWN_LANE_REGEN_FLOOR_E: u32 = 300;

/// **K4 starvation sizing** (ADR 0040 §D2 "K4 fixes S6"; RULING-11 root B, 2026-09-07) — the body
/// budget for a REPLACEMENT civilian body, a pure per-tick function of the lane's current facts
/// (no mode, no latch, no history):
///
/// * `reachable_energy_e` = `energy_available` + the home room's HAULABLE stock (storage + source
///   containers + links — what the lane can be refilled to with NO new income). If the capacity
///   body is reachable, the lane will get there (refill is the top-priced haul sink), so the
///   capacity body is the target and head-of-line banking toward it is correct — the steady
///   state, unchanged.
/// * Otherwise the room cannot pay for the capacity body from anything it holds; waiting means
///   waiting on income that does not exist (the collapse regime: lane pinned at the 300 regen,
///   containers empty, no miner). The body is sized from what is affordable NOW, floored at the
///   regen floor — a 300e hauler / 250e harvester / 250e miner is always fieldable, the lane
///   recovers, and the next replacement (a per-tick re-evaluation) grows with the room.
///
/// Deterministic integer compare; the body CHOICE is a function of the current lane, not a mode.
pub fn replacement_body_energy(energy_available: u32, energy_capacity: u32, reachable_energy_e: u32, capacity_body_cost: u32) -> u32 {
    if reachable_energy_e >= capacity_body_cost {
        energy_capacity
    } else {
        energy_available.max(SPAWN_LANE_REGEN_FLOOR_E)
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

/// The harvester spawn bid the LIVE caller uses (source_mining.rs): the ADR 0043 C2 bootstrap
/// floor made explicit — a LOCAL source with no harvesting creep of any kind bids
/// [`SPAWN_BID_BOOTSTRAP_HARVESTER`] (strictly above the miner band, so the one self-sufficient
/// body always heads the queue); every other shape is the [`harvester_priority`] lerp.
pub fn harvester_bid(total_harvesting_creeps: usize, current: usize, desired: usize, room_manhattan_distance: u32) -> u32 {
    if room_manhattan_distance == 0 && total_harvesting_creeps == 0 {
        SPAWN_BID_BOOTSTRAP_HARVESTER
    } else {
        harvester_priority(current, desired, room_manhattan_distance)
    }
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

/// One hauler body's throughput in the demand-sizing currency [`hauler_desired`] runs on
/// (milli): `carry × CARRY_CAPACITY × 1/((max_distance·2)+1)` — the same round-trip factor that
/// converts the pickup room's unfulfilled hauling into a hauler count, so throughput and unmet
/// demand are commensurable by construction.
pub fn hauler_throughput_milli(carry_parts: u32, max_distance: u32) -> u32 {
    (carry_parts as u64 * CARRY_CAPACITY as u64 * BID_SCALE as u64 / (max_distance as u64 * 2 + 1)).min(u32::MAX as u64) as u32
}

/// The hauler spawn ROI bid (ADR §D2, M5b — civilian `body_roi_milli`; ADR 0043 A10 marginal
/// form, RULING-11 root B 2026-09-07). The hauler's §D5.4 `w` is the MARGINAL throughput this body
/// unblocks: `min(body throughput, residual unmet demand)`, where the residual is the pickup
/// room's unfulfilled hauling (`unfulfilled_hauling`, the demand-sizing stat) minus what the
/// `current` roster already serves — so a lane whose demand is met (or covered by the alive
/// carriers) prices at its coarse band, and only a genuinely unserved lane bids the ROI up. The
/// old form priced a FIXED per-body constant (`carry·50` per `carry·100` of cost = 750_000 for
/// every 1:1 body) and so pinned every hauler at the 99_999 cap regardless of need.
///
/// Amortized over the body cost and clamped strictly below the [`SPAWN_BID_MINER`] income band
/// (`[band, SPAWN_BID_CRITICAL - 1]`): a genuinely stressed logistics lane can bid ABOVE the
/// shared HIGH/combat-forming band (logistics is never starved by speculative combat forming) but
/// is only ever out-ranked by income. The FIRST local hauler (`current == 0`) is the bootstrap
/// carrier and bids [`SPAWN_BID_BOOTSTRAP_HAULER`] instead (see that constant).
pub fn hauler_bid(
    current: usize,
    desired_for_unfulfilled: u32,
    max_distance: u32,
    unfulfilled_hauling: u32,
    carry_parts: u32,
    body_cost: u32,
) -> u32 {
    if current == 0 && max_distance == 0 {
        return SPAWN_BID_BOOTSTRAP_HAULER;
    }
    let band = hauler_priority(current, desired_for_unfulfilled, max_distance);
    let per_body_milli = hauler_throughput_milli(carry_parts, max_distance);
    // What the alive roster already serves, in the same currency (each alive carrier assumed to be
    // this body — the `hauler_desired` sizing convention).
    let served_milli = per_body_milli as u64 * current as u64;
    let residual_milli = (unfulfilled_hauling as u64 * BID_SCALE as u64).saturating_sub(served_milli).min(u32::MAX as u64) as u32;
    let w_milli = per_body_milli.min(residual_milli);
    let roi = body_roi_milli(w_milli, body_cost);
    // Blend: the ROI refines the ordering WITHIN the economy class. Take the larger of the coarse
    // band and the marginal ROI (an unserved lane bids up), capped strictly below the miner band
    // so logistics never preempts income but CAN out-rank a forming squad.
    band.max(roi).min(SPAWN_BID_MINER - 1)
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
        // ROI refinement (M5b, marginal form): an UNSERVED lane bids up. It is capped only below the
        // CRITICAL miner band, so a genuinely stressed logistics lane can out-rank a forming combat
        // squad (combat must not starve the economy) while income (miners) is still never preempted.
        // (current=1 so the bootstrap floor does not apply; 800e unmet vs a 3C roster serving 150.)
        assert!(hauler_bid(1, 5, 0, 800, 3, 300) >= SPAWN_BID_HIGH, "a strong-ROI hauler bids at least its band");
        assert!(
            hauler_bid(1, 5, 0, 4_000, 10, 1_000) > SPAWN_BID_COMBAT_FORMING,
            "a genuinely unserved high-throughput lane can out-bid speculative combat forming"
        );
        assert!(hauler_bid(1, 5, 0, 4_000, 10, 1_000) < SPAWN_BID_CRITICAL, "but logistics never preempts income (miners)");
    }

    /// RULING-11 root B (2026-09-07), pin 1 — INCOME OUTRANKS LOGISTICS under every roster / need
    /// shape: the static-miner bid is the CRITICAL band and the hauler bid can never reach it,
    /// whatever the unmet demand, body or roster; the two bootstrap floors (restart harvester,
    /// first carrier) sit strictly above the miner so a tie can never bank the lane at zero income.
    #[test]
    fn miner_outranks_every_hauler_and_the_bootstrap_floors_outrank_the_miner() {
        assert_eq!(SPAWN_BID_MINER, SPAWN_BID_CRITICAL, "static miners bid in the CRITICAL income band");
        // Every hauler shape: roster 1..=6, unmet 0..=20k, body 300..=2000, local and remote.
        for current in 1..=6usize {
            for &unmet in &[0u32, 100, 800, 4_000, 20_000] {
                for &(carry, cost) in &[(3u32, 300u32), (13, 1_300), (18, 1_800), (20, 2_000)] {
                    for distance in 0..=2u32 {
                        let bid = hauler_bid(current, 5, distance, unmet, carry, cost);
                        assert!(
                            bid < SPAWN_BID_MINER,
                            "hauler(current={current}, unmet={unmet}, carry={carry}, cost={cost}, d={distance}) bid {bid} must stay below the miner band {SPAWN_BID_MINER}"
                        );
                    }
                }
            }
        }
        // The live W5N49 shape: an 1800e capacity hauler behind full containers (4000e unmet) with
        // 2 small haulers alive — bids high, but the 550e miner still heads the queue.
        assert!(hauler_bid(2, 5, 0, 4_000, 18, 1_800) < SPAWN_BID_MINER, "the live 1800e hauler never out-bids the miner");
        // Replacement harvesters (roster > 0) sit below the miner; the RESTART harvester and the
        // FIRST carrier sit strictly above it (ordered restart > carrier > miner).
        assert!(harvester_bid(1, 1, 4, 0) < SPAWN_BID_MINER, "a replacement harvester never out-bids a miner");
        assert!(harvester_bid(1, 0, 4, 0) <= SPAWN_BID_MINER, "a same-source replacement at worst ties the income band");
        assert_eq!(harvester_bid(0, 0, 4, 0), SPAWN_BID_BOOTSTRAP_HARVESTER, "no harvesting creep at all → the restart floor");
        assert_eq!(harvester_bid(0, 0, 4, 1), SPAWN_BID_MEDIUM, "a remote source never bootstraps above income");
        assert_eq!(hauler_bid(0, 5, 0, 4_000, 3, 300), SPAWN_BID_BOOTSTRAP_HAULER, "an empty local hauler roster → the carrier floor");
        assert!(hauler_bid(0, 5, 1, 4_000, 3, 300) < SPAWN_BID_MINER, "an empty REMOTE roster never bootstraps above income");
        assert_eq!(hauler_bid(0, 5, 1, 0, 3, 300), SPAWN_BID_MEDIUM, "…and with nothing unmet it is exactly its remote band");
        const _: () = assert!(SPAWN_BID_BOOTSTRAP_HARVESTER > SPAWN_BID_BOOTSTRAP_HAULER);
        const _: () = assert!(SPAWN_BID_BOOTSTRAP_HAULER > SPAWN_BID_MINER);
        assert_eq!(spawn_bid_label(SPAWN_BID_BOOTSTRAP_HARVESTER), "Critical", "the floors label as the CRITICAL band");
    }

    /// RULING-11 root B, pin 2 — the NEED-SCALED hauler bid: with zero unmet demand (or demand the
    /// alive roster already covers) the bid falls to the coarse band floor; it rises with residual
    /// unmet demand, monotonically, and saturates strictly below the miner band.
    #[test]
    fn need_scaled_hauler_bid_falls_to_the_band_floor_when_unmet_demand_is_zero() {
        // Zero unmet → exactly the band (local relaxed: MEDIUM; local urgent: HIGH).
        assert_eq!(hauler_bid(4, 5, 0, 0, 18, 1_800), hauler_priority(4, 5, 0), "zero unmet demand → the band floor");
        assert_eq!(hauler_bid(4, 5, 0, 0, 18, 1_800), SPAWN_BID_MEDIUM);
        assert_eq!(hauler_bid(1, 5, 0, 0, 18, 1_800), SPAWN_BID_HIGH, "urgent band floor at zero unmet");
        // Demand the roster already serves is not marginal: 2 × 18C (900 each) cover 1800 unmet.
        assert_eq!(hauler_bid(2, 5, 0, 1_800, 18, 1_800), hauler_priority(2, 5, 0), "covered demand → the band floor");
        // Residual unmet lifts the bid, monotonically, up to (never reaching) the miner band.
        // (residual 100e → roi 100_000·1500/1800 = 83_333 > HIGH; 110e → 91_666; 18_200e → cap.)
        let low = hauler_bid(2, 5, 0, 1_900, 18, 1_800);
        let mid = hauler_bid(2, 5, 0, 1_910, 18, 1_800);
        let high = hauler_bid(2, 5, 0, 20_000, 18, 1_800);
        assert!(low > hauler_priority(2, 5, 0), "any residual unmet demand prices above the band ({low})");
        assert!(mid > low, "more residual demand → higher bid ({mid} > {low})");
        assert!(high >= mid && high == SPAWN_BID_MINER - 1, "a deeply unserved lane saturates just below the miner band ({high})");
        // The throughput currency is the demand-sizing one: 18C local = 900 × 1000 milli.
        assert_eq!(hauler_throughput_milli(18, 0), 900_000);
        assert_eq!(hauler_throughput_milli(3, 1), 50_000, "remote d=1: 150 / 3");
    }

    /// RULING-11 root B, pin 3 — STARVATION SIZING is a per-tick function of the lane: the capacity
    /// body is the target iff the room can reach its cost from what it holds (lane + haulable
    /// stock); otherwise the body is sized from available-now energy, floored at the 300 the spawn
    /// regenerates — so a 300e hauler / 250e harvester / 250e miner is always fieldable. No mode.
    #[test]
    fn starvation_sizing_picks_the_affordable_body_when_the_capacity_body_is_unreachable() {
        // The live W13N51 shape: lane 300 of 2300, nothing in storage/containers, 1250e capacity
        // harvester / 1800e hauler requested → size from the lane (300), not capacity.
        assert_eq!(replacement_body_energy(300, 2_300, 300, 1_250), 300, "unreachable capacity body → the lane");
        assert_eq!(replacement_body_energy(300, 1_800, 300, 1_800), 300);
        // Below the regen floor the budget is still 300 (the spawn regenerates to it for free).
        assert_eq!(replacement_body_energy(50, 1_800, 50, 1_800), SPAWN_LANE_REGEN_FLOOR_E, "floored at the 300 regen");
        // A partially-filled lane sizes from what it holds NOW (per-tick optimal, not a mode).
        assert_eq!(replacement_body_energy(1_000, 1_800, 1_500, 1_800), 1_000, "a 1000e lane fields a 1000e body");
        // Reachable (stock in containers/storage) → the capacity body banks, exactly as before.
        assert_eq!(replacement_body_energy(300, 1_800, 4_300, 1_800), 1_800, "full containers → capacity sizing (steady state)");
        assert_eq!(replacement_body_energy(300, 550, 300 + 250, 550), 550, "reachable at exactly the cost → capacity");
        assert_eq!(replacement_body_energy(1_800, 1_800, 1_800, 1_800), 1_800, "a full lane is trivially reachable");
        // The resulting bodies are the always-fieldable ones.
        let body = create_body(&hauler_body(false, replacement_body_energy(300, 1_800, 300, 1_800))).unwrap();
        assert_eq!(body.iter().map(|p| p.cost()).sum::<u32>(), 300, "a 3C3M hauler under starvation");
        let body = create_body(&harvester_body(replacement_body_energy(300, 2_300, 300, 1_250))).unwrap();
        assert_eq!(body.iter().map(|p| p.cost()).sum::<u32>(), 250, "a [M,M,C,W] harvester under starvation");
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
        const _: () = assert!(SPAWN_BID_CRITICAL > SPAWN_BID_COMBAT_FORMING, "income is never preempted");
        assert_eq!(SPAWN_BID_COMBAT_FORMING, SPAWN_BID_HIGH, "a forming squad SHARES the HIGH band — it must not starve the economy");
        const _: () = assert!(SPAWN_BID_HIGH > SPAWN_BID_MEDIUM);
        const _: () = assert!(SPAWN_BID_MEDIUM > SPAWN_BID_LOW);
        const _: () = assert!(SPAWN_BID_LOW > SPAWN_BID_NONE);
    }

    #[test]
    fn forming_completion_bid_orders_objective_value_in_a_reserved_band() {
        // BOOTSTRAP: a zero-value / just-fielded squad (r_o == 0) still bids at HIGH — so member 1
        // always wins its lane and no `>= SPAWN_BID_HIGH` consumer flips. It never prices below HIGH.
        assert_eq!(forming_completion_bid(0), SPAWN_BID_HIGH, "r_o=0 bootstraps at the HIGH floor");

        // ORDERS by the objective's real completed value WITHIN the reserved band [HIGH, CRITICAL).
        let cheap = forming_completion_bid(2_000);
        let dear = forming_completion_bid(20_000);
        assert!(cheap > SPAWN_BID_HIGH, "a valued objective bids above the HIGH floor ({cheap})");
        assert!(dear > cheap, "a higher-value objective outbids a lower-value one ({dear} > {cheap})");

        // Pinned STRICTLY below CRITICAL — income (miners) is never preempted, however large value_e is.
        let huge = forming_completion_bid(999_999);
        assert!(huge < SPAWN_BID_CRITICAL, "the reserved band never reaches CRITICAL ({huge})");
        assert_eq!(huge, SPAWN_BID_CRITICAL - 1, "a saturating rate pins just below CRITICAL");
    }

    #[test]
    fn forming_burn_and_giveup_are_coherent_rates() {
        // Burn = roster_cost / 1500 e/t → milli. A 2800e RangedDPS bleeds ~1867 milli-e/t idle.
        assert_eq!(forming_burn_rate_milli(0), 0);
        assert_eq!(forming_burn_rate_milli(2_800), 1_867, "cost·2/3 rounded");
        assert!(forming_burn_rate_milli(5_600) > forming_burn_rate_milli(2_800), "more present roster ⇒ more burn");

        // Give-up (floor 0): abandon iff the completed rate can't cover the burn of holding the roster.
        assert!(
            should_abandon_forming(/*r_o*/ 0, /*burn*/ 1_867, /*floor*/ 0),
            "a zero-value objective can't cover any burn ⇒ abandon"
        );
        assert!(
            !should_abandon_forming(/*r_o*/ 5_000, /*burn*/ 1_867, /*floor*/ 0),
            "a valued objective that beats the burn keeps forming"
        );
        // The opportunity floor raises the bar: the same squad abandons once the economy alternative
        // out-earns the net (r_o − burn). Commensurable — both milli-e/t rates.
        assert!(
            should_abandon_forming(/*r_o*/ 5_000, /*burn*/ 1_867, /*floor*/ 4_000),
            "when the economy alternative beats (r_o − burn), abandon"
        );
    }

    /// ADR 0042 §5 (parity M23) — the shared K-tick latch: the economic give-up FIRES on exactly the
    /// `FORMING_ABANDON_STREAK`-th consecutive abandon reconcile, never earlier; ONE covering reconcile
    /// resets the streak (a transient dip does not abandon a valuable squad); the latch has no memory
    /// past the reset (no hysteresis — a fresh streak must run the full K again).
    #[test]
    fn economic_giveup_latches_on_the_kth_consecutive_abandon_and_resets_on_cover() {
        let mut g = EconomicGiveUp::default();
        for k in 1..FORMING_ABANDON_STREAK {
            assert!(!g.advance(true), "reconcile {k} (< K={FORMING_ABANDON_STREAK}) must not fire");
        }
        assert!(g.advance(true), "the K-th consecutive abandon reconcile fires");
        assert_eq!(g.streak, FORMING_ABANDON_STREAK);
        assert!(g.advance(true), "and it stays fired while the abandon holds");

        // ONE covering reconcile resets the streak entirely.
        assert!(!g.advance(false), "a covering reconcile un-fires the give-up");
        assert_eq!(g.streak, 0, "the streak resets to zero on cover (no partial memory)");
        for _ in 1..FORMING_ABANDON_STREAK {
            assert!(!g.advance(true));
        }
        assert!(g.advance(true), "a fresh streak needs the full K again");
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
