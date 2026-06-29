//! Pure, reusable **room-economics net-ROI** kernel (ADR 0032 §"economic-value-unlocked").
//!
//! The economic value of *controlling* a room = the energy gain it enables, as a FULL NET-ROI:
//! `net = gross_income − hold_cost − mining − haul − cpu/distance_penalty`, amortized to **energy/tick**
//! and then projected over a `horizon` for the total energy-equivalent upside. This is the SAME shape as
//! the SK-farm scorer ([`crate::operations::sourcekeeper::score_sk_farm`], ADR 0018 §3.2) and the remote
//! `MiningOutpost` economics — GENERALIZED so it values ANY controlled/reservable room, not just an SK
//! farm. A room with no exploitable economy (0 sources) nets ~0.
//!
//! # Why this lives in the bot crate as a standalone, world-free module
//! It is a **functionally PURE kernel** — NO `game::*` / world reads. It takes plain room FACTS
//! ([`RoomEconomyFacts`]) and returns an energy-equivalent net-ROI. It is positioned here (not in
//! `war.rs`, not in the combat-decision crate) deliberately (ADR 0032 §architecture): it must be
//! importable by BOTH
//! - combat target selection (`operations::war` — value a winnable invader-core room by the economy it
//!   unlocks, not just threat/proximity), AND
//! - a FUTURE expansion / claim-selection scorer (`expansion` / `claim.rs` — value a claim target by its
//!   net-ROI over the *owned*-room horizon).
//!
//! Putting it in the combat-decision crate would force a future expansion scorer to depend on combat;
//! putting it in `war.rs` would couple expansion to the war operation. A pure module in the bot crate is
//! importable by both with no such coupling. The bot ADAPTERS (war.rs / a future claim scorer) GATHER the
//! facts from `RoomData`/visibility and pass them in; the kernel stays pure + unit-testable + bit-
//! deterministic (scalar arithmetic, no `HashMap`, no float fed into a discrete branch by this module —
//! callers quantize before any discrete decision, exactly as the EV path does).
//!
//! `value_e`'s economic arms ([`screeps_combat_decision::objective_value`] `FarmCore`/`FarmSourceKeeper`)
//! CONSUME this output via `ObjectiveIntel { income_per_tick, horizon, upkeep_per_tick }`; the net-ROI
//! COMPUTATION itself lives here, the one reusable place.

// ─── Engine values (mirrored locally so the kernel stays world-free) ─────────
// Provenance in `docs/references/engine-mechanics.md`; same constants the SK scorer mirrors.

/// `SOURCE_ENERGY_NEUTRAL_CAPACITY` — a *reservable/neutral* source holds 1500 per regen cycle (a RESERVED
/// remote source restores to `SOURCE_ENERGY_CAPACITY` = 3000; we use the conservative neutral figure for an
/// as-yet-unreserved room, and the caller may pass the reserved figure once the reservation is held).
pub const SOURCE_ENERGY_NEUTRAL_CAPACITY: f64 = 1500.0;
/// `SOURCE_ENERGY_CAPACITY` — a source in a room WE reserve restores 3000 per cycle (the upside controlling
/// the room unlocks — double the neutral yield).
pub const SOURCE_ENERGY_RESERVED_CAPACITY: f64 = 3000.0;
/// `ENERGY_REGEN_TIME` — sources refill on a 300-tick timer.
pub const SOURCE_REGEN_TICKS: f64 = 300.0;
/// `HARVEST_POWER` — energy harvested per WORK part per tick.
const HARVEST_POWER: f64 = 2.0;
/// `CARRY_CAPACITY` — resources a CARRY part holds.
const CARRY_CAPACITY: f64 = 50.0;
/// `CREEP_LIFE_TIME` — body-spawn cost amortizes over this many ticks.
const CREEP_LIFETIME: f64 = 1500.0;
/// `BODYPART_COST` for WORK / CARRY / MOVE (the parts the cost model sizes).
const WORK_COST: f64 = 100.0;
const CARRY_COST: f64 = 50.0;
const MOVE_COST: f64 = 50.0;
/// `BODYPART_COST[CLAIM]` — a reserver is one CLAIM (600) + one MOVE (50); the hold cost of *controlling* a
/// reservable remote (keeping the reservation up so the sources yield the reserved 3000/cycle).
const CLAIM_COST: f64 = 600.0;
/// Rough per-tile pathfinding-CPU charge (in energy-equivalent e/t) so distant rooms are penalised even
/// when the haul body alone would pencil out (ADR 0018 §3.2 — same figure the SK scorer uses).
const CPU_PENALTY_PER_TILE: f64 = 0.02;

/// Tiles per room-hop — the conversion a *room-route* caller (war.rs, the SK scorer) applies to turn the
/// route's `hops` into the **actual tiles** [`RoomEconomyFacts::haul_tiles`] expects (one room edge to the
/// next is ~50 tiles). The SK scorer ([`crate::operations::sourcekeeper`]) has its own copy of this figure
/// for its candidate scoring; this is the shared, kernel-side source of truth so room-hop callers don't
/// hardcode a bare `50` against `haul_tiles`.
pub const TILES_PER_ROOM: u32 = 50;

/// The default horizon (ticks) a controlled-room net-ROI accrues over when the caller does not supply one:
/// one creep lifetime, the natural amortization window the per-tick body costs already use, and a
/// conservative floor (a remote we hold typically pays out far longer).
pub const DEFAULT_HOLD_HORIZON: f64 = CREEP_LIFETIME;

/// How a room's economy is *unlocked* — drives the per-tick HOLD cost (the only term that differs between a
/// reservable remote and a suppressed SK room).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldModel {
    /// A reservable remote (e.g. a level-0 invader-core room once the core is cleared): we hold it with a
    /// reserver (CLAIM+MOVE), renewed each lifetime. Cheap; unlocks the reserved 3000/cycle yield.
    Reserve,
    /// A Source-Keeper room: we hold it by SUPPRESSING the keepers — the caller supplies the suppression
    /// duo's body-energy in [`RoomEconomyFacts::hold_body_cost`].
    Suppress,
    /// No standing hold needed (a one-shot raze / already-owned room): hold cost = 0 beyond mining + haul.
    None,
}

/// Plain room FACTS the net-ROI kernel reads — gathered by the bot adapter (war.rs / a future claim
/// scorer) from `RoomData`/visibility. NO `game::*` here; every field is a scalar the caller fills.
#[derive(Debug, Clone, Copy)]
pub struct RoomEconomyFacts {
    /// Harvestable sources in the room (0 ⇒ no exploitable economy ⇒ ~0 net-ROI).
    pub source_count: u32,
    /// Per-source energy capacity per regen cycle. Use [`SOURCE_ENERGY_RESERVED_CAPACITY`] for a room we
    /// will reserve, [`SOURCE_ENERGY_NEUTRAL_CAPACITY`] for an unreserved/neutral room, or the SK figure.
    pub source_capacity: f64,
    /// One-way path length (tiles) to the nearest capable home — drives haul body sizing + the CPU penalty.
    /// The dominant distance term; far rooms net less.
    pub haul_tiles: u32,
    /// How the room is held (drives the per-tick hold cost). [`HoldModel::Suppress`] reads `hold_body_cost`.
    pub hold_model: HoldModel,
    /// For [`HoldModel::Suppress`]: the suppression squad's total body-energy (the op computes it from the
    /// composition); amortized over a creep lifetime. Ignored for `Reserve`/`None`.
    pub hold_body_cost: u32,
    /// Horizon (ticks) the net e/t accrues over for the total energy-equivalent value. `<= 0` ⇒
    /// [`DEFAULT_HOLD_HORIZON`].
    pub horizon: f64,
}

impl RoomEconomyFacts {
    /// A reservable remote (the common combat case: a level-0 invader-core room) with `source_count`
    /// sources at the RESERVED yield, held by a reserver, hauled `haul_tiles` to home, over the default
    /// horizon. The exact facts war.rs passes for a winnable lvl0 core.
    pub fn reservable_remote(source_count: u32, haul_tiles: u32) -> Self {
        Self {
            source_count,
            source_capacity: SOURCE_ENERGY_RESERVED_CAPACITY,
            haul_tiles,
            hold_model: HoldModel::Reserve,
            hold_body_cost: 0,
            horizon: DEFAULT_HOLD_HORIZON,
        }
    }
}

/// The scored economics: gross income, net e/t, and the total energy-equivalent over the horizon (what the
/// EV layer multiplies by `P(win)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomEconomyValue {
    /// Gross energy/tick the room's sources yield (before any cost).
    pub gross_per_tick: f64,
    /// Net energy/tick = gross − hold − mining − haul − cpu, floored at 0 (a net-negative room is worth 0).
    pub net_per_tick: f64,
    /// Total energy-equivalent net-ROI = `net_per_tick × horizon` (floored at 0) — the value the EV layer
    /// weighs by `P(win)`. THIS is "the economic value unlocked by controlling the room."
    pub net_roi: f64,
}

/// THE pure room-economics net-ROI kernel (ADR 0032). Generalizes the SK net model to ANY controlled room.
///
/// `net e/t = gross − hold − mining − haul − cpu`, then `net_roi = max(net, 0) × horizon`:
/// - **gross** = `source_count × source_capacity / regen` (reserved 3000/cycle for a reserved remote).
/// - **hold** = reserver upkeep (CLAIM+MOVE / lifetime) for `Reserve`, the suppression body / lifetime for
///   `Suppress`, 0 for `None`.
/// - **mining** = WORK to saturate the gross yield (`gross / HARVEST_POWER`) + 2 MOVE/source, amortized.
/// - **haul** = CARRY (+ matched MOVE) to move `gross` over a `2 × haul_tiles` round trip, amortized — the
///   term that grows with distance and kills far rooms.
/// - **cpu** = `haul_tiles × CPU_PENALTY_PER_TILE` (the distance penalty even when the body pencils out).
///
/// A room with no exploitable economy (`source_count == 0`) ⇒ gross 0 ⇒ net floored at 0 ⇒ `net_roi 0`.
/// Pure + bit-deterministic: scalar `f64` arithmetic, no `HashMap`, no `game::*`. (This module never feeds
/// a float into a discrete branch — callers quantize before any discrete EV decision.)
pub fn room_net_roi(facts: &RoomEconomyFacts) -> RoomEconomyValue {
    let n = facts.source_count as f64;
    let gross = n * facts.source_capacity.max(0.0) / SOURCE_REGEN_TICKS;

    // Hold cost (e/t): the standing force that keeps the room CONTROLLED.
    let hold = match facts.hold_model {
        HoldModel::Reserve => (CLAIM_COST + MOVE_COST) / CREEP_LIFETIME,
        HoldModel::Suppress => facts.hold_body_cost as f64 / CREEP_LIFETIME,
        HoldModel::None => 0.0,
    };

    // Mining: WORK to saturate the yield + ~2 MOVE per source-miner, amortized.
    let work_parts = (gross / HARVEST_POWER).ceil();
    let mining_body = work_parts * WORK_COST + n * 2.0 * MOVE_COST;
    let mining = mining_body / CREEP_LIFETIME;

    // Haul: CARRY (+ matched MOVE) to move `gross` over a `2 × dist` round trip, amortized.
    let carry_parts = gross * 2.0 * facts.haul_tiles as f64 / CARRY_CAPACITY;
    let haul_body = carry_parts * (CARRY_COST + MOVE_COST);
    let haul = haul_body / CREEP_LIFETIME;

    let cpu_penalty = facts.haul_tiles as f64 * CPU_PENALTY_PER_TILE;

    let net = gross - hold - mining - haul - cpu_penalty;
    let net_floored = net.max(0.0);

    let horizon = if facts.horizon > 0.0 { facts.horizon } else { DEFAULT_HOLD_HORIZON };
    RoomEconomyValue { gross_per_tick: gross, net_per_tick: net_floored, net_roi: net_floored * horizon }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) THE reach-bug #3 ordering proof, world-free: a CLOSE reservable remote core room out-values a
    /// FAR one, and BOTH out-value a room with NO exploitable economy (0 sources → 0 net-ROI). This is the
    /// pure kernel the war target-EV scorer consumes.
    #[test]
    fn close_reservable_remote_beats_far_beats_no_economy() {
        let close = room_net_roi(&RoomEconomyFacts::reservable_remote(2, 2));
        let far = room_net_roi(&RoomEconomyFacts::reservable_remote(2, 25));
        let no_economy = room_net_roi(&RoomEconomyFacts::reservable_remote(0, 2));

        assert!(close.net_roi > far.net_roi, "close ({}) > far ({})", close.net_roi, far.net_roi);
        assert!(far.net_roi > no_economy.net_roi, "far ({}) > no-economy ({})", far.net_roi, no_economy.net_roi);
        assert_eq!(no_economy.net_roi, 0.0, "a room with no sources unlocks no economy");
    }

    /// (b) An undefended winnable reservable core room has a HEALTHY positive economic value — the defect
    /// being fixed (the old `Denial`-with-dps-0 path read ~0). A close 2-source reserved remote pays out a
    /// large, clearly-positive energy-equivalent net-ROI.
    #[test]
    fn winnable_reservable_core_has_healthy_positive_value() {
        let v = room_net_roi(&RoomEconomyFacts::reservable_remote(2, 3));
        assert!(v.gross_per_tick > 15.0, "2 reserved sources gross ~20 e/t, got {}", v.gross_per_tick);
        assert!(v.net_per_tick > 5.0, "net e/t is solidly positive after all costs, got {}", v.net_per_tick);
        // Energy-equivalent over the horizon: thousands of energy, NOT ~0 (the bug).
        assert!(v.net_roi > 5_000.0, "the economic value unlocked is large, got {}", v.net_roi);
    }

    /// Determinism: same facts → byte-identical value (no HashMap, no time/world).
    #[test]
    fn room_net_roi_is_deterministic() {
        let facts = RoomEconomyFacts { source_count: 3, source_capacity: 2500.0, haul_tiles: 7, hold_model: HoldModel::Reserve, hold_body_cost: 0, horizon: 1200.0 };
        let a = room_net_roi(&facts);
        let b = room_net_roi(&facts);
        assert_eq!(a, b);
        assert!(a.net_roi.is_finite());
    }

    /// The hold model differentiates cost: an SK room (expensive suppression duo) nets LESS than the same
    /// yield held by a cheap reserver — the generalization is faithful to the SK scorer's suppression term.
    #[test]
    fn suppress_hold_costs_more_than_reserve() {
        let reserve = room_net_roi(&RoomEconomyFacts {
            source_count: 3,
            source_capacity: SOURCE_ENERGY_RESERVED_CAPACITY,
            haul_tiles: 5,
            hold_model: HoldModel::Reserve,
            hold_body_cost: 0,
            horizon: DEFAULT_HOLD_HORIZON,
        });
        let suppress = room_net_roi(&RoomEconomyFacts {
            source_count: 3,
            source_capacity: SOURCE_ENERGY_RESERVED_CAPACITY,
            haul_tiles: 5,
            hold_model: HoldModel::Suppress,
            hold_body_cost: 5350, // the SK duo body cost (ops::sourcekeeper SK_DUO_BODY_COST)
            horizon: DEFAULT_HOLD_HORIZON,
        });
        assert!(suppress.net_per_tick < reserve.net_per_tick, "suppression hold is dearer than a reserver");
    }
}
