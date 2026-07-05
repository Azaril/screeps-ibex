//! The **e/t sink market** — ADR 0040 §D1, milestone M4 (Part A): every civilian energy sink
//! priced in ONE currency, energy-equivalent value per unit of energy spent here now, quantized
//! to integer milli units ([`BID_SCALE`]) at the source — floats never reach a comparison.
//! **Storage is the numeraire: depositing to storage bids exactly [`STORAGE_BID`] = 1.000.**
//!
//! These are CANDIDATE kernels behind the M3 seam: the sim tournament (screeps-econ-eval M4)
//! consumes them directly with raw bids; the live bot adopts them at M5a (numeric-bid tickets).
//! Nothing here reaches `game::*` — plain integer functions over caller-gathered facts.
//!
//! **Survival overrides are NOT bids** (§D1 guardrails, spec Part A): the controller downgrade
//! clock, hostile-tower refill, and the container <50% high-value floor VETO outside the market
//! ([`downgrade_veto`], [`tower_refill_bid`]'s hostile lane, [`container_survival_override`]).
//! Catastrophe guards don't bid, they veto.
//!
//! **Ratified ingredients reused, not re-invented** (spec constraint): the refill ROI arms are
//! the §D5.4 civilian `w` arms ratified in `screeps-rover-eval/src/value.rs` (2026-07-01) —
//! harvester = income unlocked, hauler = logistics rate, worker = `min(WORK·k, supply)·V_SINK`
//! with `V_SINK = 1.0` (value.rs:135) — amortized over `CREEP_LIFE_TIME = 1500` (value.rs:45,
//! "the ubiquitous amortization denominator"). rover-eval is host-only, so the scalar constants
//! are TRANSCRIBED with citations (the value.rs:28-31 layering convention: transcribing scalars
//! is cheaper than a layering violation), each pinned by `ratified_constant_pins`.
//!
//! Every v0 constant below is NAMED (EP-4.6) and sim-swept by the M4 tournament
//! (`screeps-econ-eval::tournament`); the swept bundle is [`MarketConsts`] (`Default` == the
//! seeded values, the rover `PolicyParams` idiom).

use crate::priority::TransferPriority;

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The currency.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Bids are integer MILLI energy-equivalent value per energy (ADR §D1): 1000 = par.
pub const BID_SCALE: u32 = 1000;

/// The numeraire (ADR §D1): depositing to storage bids exactly 1.000. Everything is priced
/// relative to this.
pub const STORAGE_BID: u32 = BID_SCALE;

/// The survival lane's effective bid (hostile-tower refill; EP-4.3 never-shed): top of market by
/// construction — no computed bid may reach it (the refill ROI cap is orders of magnitude below).
pub const SURVIVAL_BID: u32 = 1_000_000;

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Ratified §D5.4 ingredients (transcribed, cited, pinned — module docs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// `CREEP_LIFE_TIME` — the ratified amortization denominator (value.rs:45; engine
/// engine-mechanics.md:453).
pub const CREEP_LIFE_TIME: u32 = 1500;
/// Harvest income per WORK part per tick (engine `HARVEST_POWER`, engine-mechanics.md:457) —
/// the harvester arm's "income unlocked" rate ingredient.
pub const HARVEST_POWER_E_T: u32 = 2;
/// One source's income ceiling: 3000 / 300-tick regen = 10 e/t (engine-mechanics.md:466) —
/// caps the harvester income-unlocked arm.
pub const SOURCE_RATE_E_T: u32 = 10;
/// Builder WORK conversion, 5 e/t per WORK (`BUILD_POWER` — value.rs:50).
pub const BUILD_POWER_E_T: u32 = 5;
/// Upgrader WORK conversion, 1 e/t per WORK (`UPGRADE_CONTROLLER_POWER` — value.rs:51).
pub const UPGRADE_POWER_E_T: u32 = 1;
/// The ratified worker sink-value multiplier, in milli: `V_SINK = 1.0` (value.rs:135, §D5.4
/// decision (3): build and upgrade energy at par until a sink-value kernel lands — THIS module
/// is that kernel for the sinks it prices; the worker `w` arms keep par).
pub const V_SINK_Q: u32 = 1000;
/// `CARRY_CAPACITY` — 50 per CARRY part (engine-mechanics.md:453) — the hauler logistics-rate
/// arm's cargo ingredient (value.rs `Role::Haul`: ρ = Q/T*).
pub const CARRY_CAPACITY: u32 = 50;

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The v0 sweep constants (§D8 #1 — shapes approved, numbers land as the M4 reviewed diff).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// `V_UPGRADE` (§D1): the declared e/t worth of controller progress, milli. Seeded at par;
/// **TUNED 1000 → 2000 by the M4 sweep** (fast corpus, coordinate descent; re-confirmed against
/// the DERIVED refill floor: C H 0.478 / D H 0.217 at 2000 vs C H 0.294 / D H 0.216 at 1000 —
/// the strongest single lever under the derived floor. Controller progress above par keeps
/// upgraders supplied through mild deficits without touching the collapse hoard, which the
/// derived instant-spawnability premium prices well above 2000 whenever the lane is deficient).
pub const V_UPGRADE_MILLI: u32 = 2000;
/// The RCL-unlock step premium added to the upgrade bid near level-up (§D1), milli; SWEPT via
/// the [`MarketConsts`] bundle.
pub const UPGRADE_STEP_PREMIUM_MILLI: u32 = 1000;
/// "Near level-up" = remaining controller progress at or below this (progress units).
pub const UPGRADE_STEP_WINDOW_PROGRESS: u32 = 2000;

/// Build-class table (§D1 v0 seeds; per-energy value of the class completing, milli):
/// a missing SPAWN is existential (everything chains through it).
pub const BUILD_BID_SPAWN_MILLI: u32 = 10_000;
/// Extensions are the compounding investment post-wipe (capacity → bodies → income);
/// **TUNED 4000 → 8000 by the M4 sweep** (re-run against the DERIVED refill floor: G ΔT −3195
/// vs −2717 at a flat C/D — pricing extension sites toward the spawn class accelerates the
/// greenfield capacity→body→income chain; the derived floor keeps the collapse hoard safe so
/// the higher build bid no longer competes with rebootstrap refill).
pub const BUILD_BID_EXTENSION_MILLI: u32 = 8_000;
/// Containers price harvest efficiency (the provider loop).
pub const BUILD_BID_CONTAINER_MILLI: u32 = 2_000;
/// Storage/tower: real but non-compounding infrastructure.
pub const BUILD_BID_STORAGE_MILLI: u32 = 1_500;
pub const BUILD_BID_TOWER_MILLI: u32 = 1_500;
/// Roads price their movement savings (§D1: the rover corpus measures e/t per fatigue tile);
/// **TUNED 500 → 250 by the M4 sweep** (re-confirmed against the derived floor: G ΔT −3195 at
/// 250 vs −2420 at 500 — greenfield rushes waste less builder time on road sites; C/D
/// indifferent).
pub const BUILD_BID_ROAD_MILLI: u32 = 250;

/// Repair imminence horizon (ticks): a structure `horizon` ticks from wear-death prices its
/// full rebuild-avoidance ratio; further out decays hyperbolically ([`imminence_q`]). Seeded
/// 750 so a 50%-health plain road under base decay prices ~0.3–0.4 (the §D1 claim:
/// 750·0.1/2500 = 0.03 imminence × ratio 12 = 0.36); **TUNED 750 → 1500 by the M4 full-corpus
/// Family-S gate** (gates-first). At 750 the 10k-tick healthy-room Family-S run (the gate the
/// tournament actually runs — `DEFAULT_S_TICK_CAP`) FAILED the road-stock-end band on E13S29-rcl4
/// (0.947 baseline → 0.806): roads glided steadily downward without reaching equilibrium inside
/// the horizon. At 1500 the §D1 low-riding equilibrium is real over the gate's 10k ticks (road
/// stock min 0.938 / end 0.960 vs the 0.947 baseline end — an IMPROVEMENT), and a longer 30k
/// diagnostic confirms it holds (min 0.938, end 0.984) rather than being 10k-transient. Cost on
/// C is small and well inside the market's win margin.
pub const REPAIR_IMMINENCE_HORIZON_TICKS: u32 = 1_500;
/// Floor on `repair_cost_remaining` (energy) in the repair ratio — kills the spurious
/// nearly-full blowup (1 missing hit ⇒ ratio → ∞) while leaving the §D1 mid-range untouched
/// (a ≤ [`REPAIR_COST_FLOOR_E`]-from-full road caps at ratio·imminence ≪ par).
pub const REPAIR_COST_FLOOR_E: u32 = 10;
/// The container's functional-value premium (§D1: "containers add their functional value —
/// harvest throughput carried"), milli, scaled by imminence like the rebuild ratio.
pub const CONTAINER_THROUGHPUT_MILLI: u32 = 4_000;

/// Refill ROI cap, milli (spec Part A "refill ROI cap" sweep axis): bounds the top-blocked-
/// request ROI so a degenerate tiny body cannot price the lane at infinity; **TUNED
/// 20000 → 10000 by the M4 sweep** (decisive under the DERIVED floor: C H 0.478 / D H 0.217 at
/// 10000 vs C H 0.290 / D H 0.159 at 20000 — with the derived premium already pricing a
/// deficient lane high, a large ROI ceiling ADDITIONALLY over-prices the few above-10× repair/
/// build emergencies enough to divert rebootstrap energy, regressing C/D to baseline. 10000 is
/// also exactly §D1's "~10×" narrative figure).
pub const REFILL_ROI_CAP_MILLI: u32 = 10_000;

// (The former flat `REFILL_FLOOR_MILLI` constant was REPLACED at M4-review by the DERIVED
// `instant_spawnability_premium` — the banking model computes the floor from the lane deficit
// and next-body cost; there is no swept floor constant to ship. See `refill_bid`.)

/// Peaceful tower top-up (§D1: maintenance value, "loses to any real demand — deleting the S4
/// leak"): below the numeraire by construction.
pub const TOWER_PEACE_MILLI: u32 = 500;

/// A deposit bid enters the opportunity floor only if its unmet amount is at least this
/// (the §D1 "materially-unmet" qualifier), energy.
pub const FLOOR_MATERIAL_MIN_E: u32 = 50;

/// The downgrade-clock survival veto (§D1: a veto, not a bid): active while the clock is below
/// this per-mille of the level's full clock. Seeded at half-max — the SAME boundary the live
/// upkeep mission calls downgrade risk (missions/upgrade.rs:94, `work_parts_for_upkeep`'s safe
/// threshold), so there is one "the clock is in danger" line in the codebase.
pub const DOWNGRADE_VETO_Q: u32 = 500;

/// K4 wait-penalty (per-mille): how strongly time-to-afford discounts a candidate body's ROI
/// ([`deficit_priced_pick`]). 1000 = a tick of waiting costs a tick of lifetime; 0 = pure
/// per-energy ROI (banking allowed, the S6 shape). Seeded 3000 so trickle-income bootstrap
/// (1 e/t self-charge) resolves to affordable-now while healthy income (≥ ~1.5 e/t) may bank
/// briefly for a saturating body (the slot-cost interplay — [`K4_SLOT_COST_E`] docs); SWEPT.
pub const K4_WAIT_PENALTY_Q: u32 = 3000;

/// K4 slot-occupancy overhead (energy): every body occupies one roster slot (the missions'
/// desired-count caps), so a candidate's ROI denominator carries this fixed overhead —
/// breaking the equal-per-energy-ROI tie of linear body ladders toward SATURATING bodies when
/// energy is on hand, while the wait penalty still favors affordable-now under collapse.
/// Found by construction in M4: with 0 overhead the wait penalty made every ladder resolve to
/// perpetual minimum bodies (sources never saturated, Family G regressed); SWEPT {0 = the
/// pure per-energy shape, 200, 600}.
pub const K4_SLOT_COST_E: u32 = 200;

/// The container survival override threshold (spec Part A: "container <50% — the high-value
/// floor" is OUTSIDE the market), per-mille of hits_max.
pub const CONTAINER_SURVIVAL_Q: u32 = 500;

/// The swept constant bundle (the rover-eval `PolicyParams` idiom): `Default` IS the named
/// seeds above, bit-for-bit; the M4 tournament probes off-default points and the tuned values
/// land as a reviewed diff to the constants above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketConsts {
    pub v_upgrade_milli: u32,
    pub upgrade_step_premium_milli: u32,
    pub upgrade_step_window_progress: u32,
    pub build_bid_spawn_milli: u32,
    pub build_bid_extension_milli: u32,
    pub build_bid_container_milli: u32,
    pub build_bid_storage_milli: u32,
    pub build_bid_tower_milli: u32,
    pub build_bid_road_milli: u32,
    pub imminence_horizon_ticks: u32,
    pub repair_cost_floor_e: u32,
    pub container_throughput_milli: u32,
    pub refill_roi_cap_milli: u32,
    pub tower_peace_milli: u32,
    pub floor_material_min_e: u32,
    pub downgrade_veto_q: u32,
    pub k4_wait_penalty_q: u32,
    pub k4_slot_cost_e: u32,
}

impl Default for MarketConsts {
    fn default() -> Self {
        MarketConsts {
            v_upgrade_milli: V_UPGRADE_MILLI,
            upgrade_step_premium_milli: UPGRADE_STEP_PREMIUM_MILLI,
            upgrade_step_window_progress: UPGRADE_STEP_WINDOW_PROGRESS,
            build_bid_spawn_milli: BUILD_BID_SPAWN_MILLI,
            build_bid_extension_milli: BUILD_BID_EXTENSION_MILLI,
            build_bid_container_milli: BUILD_BID_CONTAINER_MILLI,
            build_bid_storage_milli: BUILD_BID_STORAGE_MILLI,
            build_bid_tower_milli: BUILD_BID_TOWER_MILLI,
            build_bid_road_milli: BUILD_BID_ROAD_MILLI,
            imminence_horizon_ticks: REPAIR_IMMINENCE_HORIZON_TICKS,
            repair_cost_floor_e: REPAIR_COST_FLOOR_E,
            container_throughput_milli: CONTAINER_THROUGHPUT_MILLI,
            refill_roi_cap_milli: REFILL_ROI_CAP_MILLI,
            tower_peace_milli: TOWER_PEACE_MILLI,
            floor_material_min_e: FLOOR_MATERIAL_MIN_E,
            downgrade_veto_q: DOWNGRADE_VETO_Q,
            k4_wait_penalty_q: K4_WAIT_PENALTY_Q,
            k4_slot_cost_e: K4_SLOT_COST_E,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Refill (§D1: refill inherits the bid of what the energy enables).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// `ROI(request) ≈ w(request) · CREEP_LIFE_TIME / body_cost`, milli (§D1's refill shape):
/// the marginal e/t return per energy invested in the body, amortized over the ratified 1500.
/// `w_milli` is the request's §D5.4 civilian rate (module docs). `body_cost == 0` prices 0
/// (a degenerate request enables nothing).
pub fn body_roi_milli(w_milli: u32, body_cost: u32) -> u32 {
    if body_cost == 0 {
        return 0;
    }
    ((w_milli as u64 * CREEP_LIFE_TIME as u64) / body_cost as u64).min(u32::MAX as u64) as u32
}

/// **The instant-spawnability premium** (milli) — the DERIVED refill floor (M4 review finding
/// #1: replaces the flat swept constant with a first-principles premium the live bot can ship).
///
/// Energy already in the spawn lane is spawnable THIS tick (the head-of-line banking mechanism,
/// `spawn_queue::spawn_step`); the same energy in a depot is not — it must be hauled to the lane
/// before any request can draw it, and until it is, the top request BANKS while income trickles.
/// Depositing to the lane instead of storage therefore buys the room the banking time it would
/// otherwise pay on its next replacement. That saved time is worth the room's marginal income
/// over the body it unblocks.
///
/// Derivation. Let `d` = current lane deficit (e), `ρ` = room income (e/t), `cost` = the
/// representative next-request body cost (e). Filling `d` now saves the next request `d/ρ`
/// banking-ticks; each saved tick lets that body live and earn one tick sooner, i.e. is worth
/// its own per-tick value. Normalizing to the numeraire (storage = 1.0), the deposit's marginal
/// value multiplier over par is `1 + (banking_ticks_saved · ρ) / cost` — the fraction of a body
/// the saved income represents. With `banking_ticks_saved = d/ρ` this telescopes to
/// `1 + d/cost` (income cancels: the premium is the deficit as a fraction of the body it will
/// spawn — a deep deficit relative to the next body prices the lane well above par; a lane one
/// small extension short of full barely clears par). Quantized: `1000 + 1000·d/cost`, clamped to
/// `[1000, ROI cap]`. `cost == 0` ⇒ par (no body to reason about).
///
/// This is NOT tuned to the tournament: it is the banking model's own arithmetic. The M4 sweep
/// (kept for the record) confirms the resulting bids land in the same regime the flat 2500 did
/// and beat baseline on C/D — but the number is now derived, not fitted (operator directive:
/// end-state, not a compromised magic constant).
pub fn instant_spawnability_premium(lane_deficit_e: u32, next_body_cost_e: u32) -> u32 {
    if next_body_cost_e == 0 {
        return BID_SCALE;
    }
    BID_SCALE + ((lane_deficit_e as u64 * BID_SCALE as u64) / next_body_cost_e as u64).min(u32::MAX as u64) as u32
}

/// Spawn/extension refill bid (§D1, review-#1 derived form): `max(derived_floor,
/// ROI(top energy-blocked request))`, capped at the swept ROI cap. `top_blocked_roi_milli` =
/// [`body_roi_milli`] of the HIGHEST-PRIORITY request the room cannot afford right now (the
/// head-of-line banker). `derived_floor` = [`instant_spawnability_premium`] from the lane
/// deficit + next-body cost — so even an unblocked queue prices the lane above par exactly as
/// much as the deficit is worth. A FULL room registers zero deposit amount (structural, K1) —
/// the bid is moot there.
pub fn refill_bid(consts: &MarketConsts, top_blocked_roi_milli: Option<u32>, lane_deficit_e: u32, next_body_cost_e: u32) -> u32 {
    let floor = instant_spawnability_premium(lane_deficit_e, next_body_cost_e).max(BID_SCALE);
    let cap = consts.refill_roi_cap_milli.max(floor);
    top_blocked_roi_milli.unwrap_or(0).clamp(floor, cap)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Repair (§D1: rebuild-avoidance ratio × wear imminence).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Wear imminence, per-mille (§D1's `imminence(hits, wear_rate)`): how close the structure is
/// to wear-death on the horizon scale. `ttd = hits / wear_rate`; `q = horizon / ttd`, clamped
/// to [0, 1000]. `wear_milli_hits_per_tick` = decay PLUS observed trailing traffic wear (the
/// M1 engine timer-pull model, adapter-observed). Zero wear ⇒ 0 (it never dies); zero hits ⇒
/// 1000 (it is dying now).
pub fn imminence_q(hits: u32, wear_milli_hits_per_tick: u32, horizon_ticks: u32) -> u32 {
    if hits == 0 {
        return 1000;
    }
    if wear_milli_hits_per_tick == 0 {
        return 0;
    }
    // q = horizon·1000 / ttd, ttd = hits·1000/wear  ⇒  q = horizon·wear / hits.
    ((horizon_ticks as u64 * wear_milli_hits_per_tick as u64) / hits as u64).min(1000) as u32
}

/// The §D1 repair bid, milli: `(rebuild_cost_avoided / repair_cost_remaining) · imminence`.
/// `repair_cost_remaining` is floored at the named [`MarketConsts::repair_cost_floor_e`]
/// (kills the nearly-full ratio blowup — constant docs). All inputs in whole energy; exact
/// integer arithmetic.
pub fn repair_bid(consts: &MarketConsts, rebuild_cost_avoided_e: u32, repair_cost_remaining_e: u32, imminence_q: u32) -> u32 {
    let cost = repair_cost_remaining_e.max(consts.repair_cost_floor_e).max(1);
    ((rebuild_cost_avoided_e as u64 * imminence_q.min(1000) as u64) / cost as u64).min(u32::MAX as u64) as u32
}

/// The container functional-value term (§D1: "containers add their functional value"), scaled
/// by the same imminence, added to the rebuild ratio by the caller's container arm:
/// `container_repair_bid = repair_bid(...) + container_function_milli(...)`.
pub fn container_function_milli(consts: &MarketConsts, imminence_q: u32) -> u32 {
    ((consts.container_throughput_milli as u64 * imminence_q.min(1000) as u64) / 1000) as u32
}

/// The container <50% survival override (spec Part A): admitted OUTSIDE the market — a veto,
/// not a bid. `hits·1000 < hits_max·CONTAINER_SURVIVAL_Q`.
pub fn container_survival_override(hits: u32, hits_max: u32) -> bool {
    (hits as u64) * 1000 < (hits_max as u64) * CONTAINER_SURVIVAL_Q as u64
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Build / upgrade / towers.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The build-bid class vocabulary (this crate deliberately does not depend on the sim engine's
/// `StructureKind`; adapters map).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BuildClass {
    Spawn,
    Extension,
    Road,
    Container,
    Storage,
    Tower,
}

/// The §D1 v0 per-class build bid table, milli (sim-swept).
pub fn build_bid(consts: &MarketConsts, class: BuildClass) -> u32 {
    match class {
        BuildClass::Spawn => consts.build_bid_spawn_milli,
        BuildClass::Extension => consts.build_bid_extension_milli,
        BuildClass::Road => consts.build_bid_road_milli,
        BuildClass::Container => consts.build_bid_container_milli,
        BuildClass::Storage => consts.build_bid_storage_milli,
        BuildClass::Tower => consts.build_bid_tower_milli,
    }
}

/// The upgrade bid (§D1): `V_UPGRADE` + the RCL-unlock step premium near level-up. The HARD
/// downgrade override is [`downgrade_veto`] — a veto outside the market, never folded in here.
pub fn upgrade_bid(consts: &MarketConsts, near_level_up: bool) -> u32 {
    if near_level_up {
        consts.v_upgrade_milli.saturating_add(consts.upgrade_step_premium_milli)
    } else {
        consts.v_upgrade_milli
    }
}

/// The downgrade survival veto (§D1 guardrail): fires while the clock is below
/// `downgrade_veto_q` per-mille of the level's full clock. A vetoing room admits controller
/// supply regardless of the floor.
pub fn downgrade_veto(consts: &MarketConsts, downgrade_ticks: u32, full_clock: u32) -> bool {
    (downgrade_ticks as u64) * 1000 < (full_clock as u64) * consts.downgrade_veto_q as u64
}

/// Tower refill (§D1): hostiles present ⇒ the survival lane (top of market, never-shed);
/// peaceful ⇒ the low maintenance constant (loses to any real demand — the S4 leak deleted).
pub fn tower_refill_bid(consts: &MarketConsts, hostiles_present: bool) -> u32 {
    if hostiles_present {
        SURVIVAL_BID
    } else {
        consts.tower_peace_milli
    }
}

/// **Buffer deposit pricing** (an M4 measured refinement, §D1's "marginal value returned per
/// unit of energy spent here, NOW" taken seriously for buffer sinks): a buffer structure
/// (controller container, overflow container) holding for a downstream sink is NOT worth the
/// downstream bid per marginal energy — a mostly-full buffer's next energy just sits. Price it
/// `base_bid · (free/capacity)²`: near-empty (the consumer about to starve) → the full
/// downstream bid; mostly-full → priced out by any real demand. Quadratic: the buffer's slack
/// grows linearly with fill while its refill urgency falls with it — found empirically in M4
/// (linear falloff still let a 70%-full 2000-capacity controller container out-density the
/// spawn lane's residual deficits and regress Family S refill latency). Storage itself is the
/// numeraire, never shaped.
pub fn buffer_deposit_bid(base_bid: u32, free: u32, capacity: u32) -> u32 {
    if capacity == 0 {
        return 0;
    }
    let f = free.min(capacity) as u64;
    ((base_bid as u64 * f * f) / (capacity as u64 * capacity as u64)) as u32
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The opportunity floor + admission (§D1 withdraw admission; K3 repair admission).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// `floor(room)` = the highest MATERIALLY-unmet deposit bid (§D1): deposits with unmet amount
/// below `floor_material_min_e` don't move the floor. 0 when nothing is materially unmet
/// (every withdraw admits).
pub fn opportunity_floor(consts: &MarketConsts, unmet_deposits: impl IntoIterator<Item = (u32, u32)>) -> u32 {
    let mut floor = 0u32;
    for (bid, unmet) in unmet_deposits {
        if unmet >= consts.floor_material_min_e && bid > floor {
            floor = bid;
        }
    }
    floor
}

/// Use-lane withdraw admission (§D1): admitted iff the DESTINATION sink's bid meets the floor.
/// Quantized compare; the tie (bid == floor) ADMITS deterministically — a sink exactly as
/// valuable as the best unmet deposit may compete.
pub fn admit_use_withdraw(sink_bid: u32, floor: u32) -> bool {
    sink_bid >= floor
}

/// K3-market repair admission (spec Part B): `repair_bid ≥ floor` (same quantized compare,
/// ties admit) — replacing the S1 threshold gate in the candidate arm. Survival overrides
/// bypass this entirely (module docs).
pub fn admit_repair(repair_bid: u32, floor: u32) -> bool {
    repair_bid >= floor
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K1's tier projection (M4 sim back-compat surface — the live numeric lane is M5a).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Tier-map thresholds (named): a bid this far above par reads High / Medium; anything above
/// par reads Low; par-and-below reads None (the storage lane's tier today).
pub const TIER_HIGH_MIN_MILLI: u32 = 4_000;
pub const TIER_MEDIUM_MIN_MILLI: u32 = 1_500;

/// Map a quantized bid onto the EXISTING 4-tier enum (K1-market, spec Part B: amounts
/// unchanged; the sim's matching uses the raw bid directly, this projection is the
/// display/back-compat surface until M5a's numeric ticket lane).
pub fn bid_to_tier(bid_milli: u32) -> TransferPriority {
    if bid_milli >= TIER_HIGH_MIN_MILLI {
        TransferPriority::High
    } else if bid_milli >= TIER_MEDIUM_MIN_MILLI {
        TransferPriority::Medium
    } else if bid_milli > BID_SCALE {
        TransferPriority::Low
    } else {
        TransferPriority::None
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// M5a bot bid vocabulary: the numeric-bid lane the LIVE tickets/keys/requests ride (spec Part 1).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Representative bids for the 4 tier bands — the M5a bot registration mapping for the
/// NON-MARKET lanes that still register through the transfer queue by tier (links / terminal /
/// labs / powerspawn / salvage / siege-tower): the demand kernels + the ad-hoc mission sites
/// emit a `TransferPriority`, and [`tier_to_bid`] carries it onto the numeric ticket lane so the
/// whole queue is keyed by one currency. Each band's bid sits strictly inside its
/// [`bid_to_tier`] window, so `bid_to_tier(tier_to_bid(t)) == t` round-trips (pinned below) — a
/// tier request and its numeric bid read identically for the display/HUD label.
///
/// These are DELIBERATELY coarse: they are the priorities of sinks the ECONOMIC market does not
/// price (a lab reaction's intra-room shuffle, a terminal send). The market-priced lanes
/// (spawn/extension refill, repair, build, upgrade, tower) call the derived bid functions
/// directly ([`refill_bid`], [`repair_bid`], [`build_bid`], [`upgrade_bid`], [`tower_refill_bid`])
/// — they never route through this band table.
pub const BID_TIER_HIGH: u32 = 5_000;
pub const BID_TIER_MEDIUM: u32 = 2_000;
pub const BID_TIER_LOW: u32 = 1_250;
/// The `None` tier is the storage numeraire lane (par) — a request with no urgency over storage.
pub const BID_TIER_NONE: u32 = STORAGE_BID;

/// Carry a tier onto the numeric bid lane (spec Part 1: the ~15 non-market registration sites).
/// The inverse-ish of [`bid_to_tier`]: it lands each tier at a representative bid inside that
/// tier's window (`bid_to_tier(tier_to_bid(t)) == t`).
pub fn tier_to_bid(priority: TransferPriority) -> u32 {
    match priority {
        TransferPriority::High => BID_TIER_HIGH,
        TransferPriority::Medium => BID_TIER_MEDIUM,
        TransferPriority::Low => BID_TIER_LOW,
        TransferPriority::None => BID_TIER_NONE,
    }
}

/// A coarse grep-able label for a numeric bid (logs / HUD): the tier band it reads as plus the
/// milli value. The spec's "display helper maps bid ranges → coarse labels."
pub fn bid_label(bid_milli: u32) -> &'static str {
    match bid_to_tier(bid_milli) {
        TransferPriority::High => "High",
        TransferPriority::Medium => "Medium",
        TransferPriority::Low => "Low",
        TransferPriority::None => "None",
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// K4 — deficit-priced bodies (spec Part B; the S6 fix).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One candidate body for a role: expanded cost + its §D5.4 rate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BodyCandidate {
    /// Total spawn energy cost.
    pub cost: u32,
    /// The body's §D5.4 civilian rate, milli e/t (harvester = income unlocked; hauler =
    /// logistics rate; worker = `min(WORK·k, supply)·V_SINK`).
    pub w_milli: u32,
}

/// **The S6 fix**: pick the ROI-max candidate body *including time-to-afford under head-of-line
/// banking*. Score = `ROI(body) · LIFE / (LIFE + wait · k4_wait_penalty_q/1000)` where
/// `ROI = w·LIFE/(cost + k4_slot_cost_e)` and `wait = ceil((cost − available)/income)` (0 when
/// affordable now) — exact rationals, no floats. Ties break to the LARGER `w` (fewer spawn
/// slots), then the SMALLER cost, then the lower index (deterministic). Returns the winning
/// index; `None` on an empty/degenerate candidate set.
///
/// The slot overhead ([`K4_SLOT_COST_E`]) makes linear ladders prefer SATURATING bodies when
/// energy is on hand (a roster slot is scarce capital), while the wait penalty keeps collapse
/// spawning affordable-now — a 1700-cost replacement no longer silently banks trickle income
/// when a 300-cost body has comparable ROI (ADR §D2). Penalty 0 = pure per-energy ROI (the
/// sweep's banking-allowed arm).
pub fn deficit_priced_pick(
    consts: &MarketConsts,
    candidates: &[BodyCandidate],
    available_now: u32,
    income_milli_e_t: u32,
) -> Option<usize> {
    let life = CREEP_LIFE_TIME as u64;
    let mut best: Option<(usize, u128, u128)> = None; // (index, score_num, score_den)
    for (i, c) in candidates.iter().enumerate() {
        if c.cost == 0 || c.w_milli == 0 {
            continue;
        }
        let wait: u64 = if c.cost <= available_now {
            0
        } else {
            // ceil((cost − available) · 1000 / income_milli): ticks of head-of-line banking.
            let deficit = (c.cost - available_now) as u64 * 1000;
            deficit.div_ceil(income_milli_e_t.max(1) as u64)
        };
        // score = (w·LIFE/(cost + slot)) · LIFE·1000 / (LIFE·1000 + wait·penalty)
        let num = c.w_milli as u128 * life as u128 * (life * 1000) as u128;
        let den = (c.cost + consts.k4_slot_cost_e) as u128 * (life * 1000 + wait * consts.k4_wait_penalty_q as u64) as u128;
        let better = match &best {
            None => true,
            Some((bi, bn, bd)) => {
                let lhs = num * bd;
                let rhs = *bn * den;
                lhs > rhs
                    || (lhs == rhs
                        && (c.w_milli, std::cmp::Reverse(c.cost)) > (candidates[*bi].w_milli, std::cmp::Reverse(candidates[*bi].cost)))
            }
        };
        if better {
            best = Some((i, num, den));
        }
    }
    best.map(|(i, _, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> MarketConsts {
        MarketConsts::default()
    }

    /// The transcribed §D5.4 / engine ingredients pinned to their cited values (value.rs:45,
    /// :50-51, :135; engine-mechanics.md:453/:457/:466) — a drive-by edit cannot silently
    /// diverge from the ratified constants.
    #[test]
    fn ratified_constant_pins() {
        assert_eq!(CREEP_LIFE_TIME, 1500, "value.rs:45");
        assert_eq!(HARVEST_POWER_E_T, 2, "engine-mechanics.md:457");
        assert_eq!(SOURCE_RATE_E_T, 10, "engine-mechanics.md:466 (3000/300)");
        assert_eq!(BUILD_POWER_E_T, 5, "value.rs:50 BUILD_POWER_E_T");
        assert_eq!(UPGRADE_POWER_E_T, 1, "value.rs:51 UPGRADE_POWER_E_T");
        assert_eq!(V_SINK_Q, 1000, "value.rs:135 V_SINK = 1.0, ratified");
        assert_eq!(CARRY_CAPACITY, 50, "engine-mechanics.md:453");
        assert_eq!(STORAGE_BID, 1000, "the numeraire is exactly 1.000 (spec Part A)");
        assert_eq!(BID_SCALE, 1000);
        // Default == the named seeds, field for field (the PolicyParams idiom).
        let d = MarketConsts::default();
        assert_eq!(d.v_upgrade_milli, V_UPGRADE_MILLI);
        assert_eq!(d.imminence_horizon_ticks, REPAIR_IMMINENCE_HORIZON_TICKS);
        assert_eq!(d.refill_roi_cap_milli, REFILL_ROI_CAP_MILLI);
        assert_eq!(d.k4_wait_penalty_q, K4_WAIT_PENALTY_Q);
    }

    /// §D1's headline claim, derived not decreed: post-wipe the FIRST harvester's refill ROI
    /// (~12×) dominates every other sink — upgrade at par, extensions' build bid, roads, the
    /// peaceful tower — and the whole ordering matches the §D1 narrative.
    #[test]
    fn post_wipe_refill_dominates_everything() {
        let consts = c();
        // First harvester [M,M,C,W]: cost 250, income unlocked = min(2·1, 10) = 2 e/t.
        let roi = body_roi_milli(2 * BID_SCALE, 250);
        assert_eq!(roi, 12_000, "2 e/t × 1500 / 250e = 12× — the §D1 '~10×' claim, exact");
        // Post-wipe: the whole lane is deficit (say a 550-cap RCL2 lane, 0 filled); the ROI
        // dominates and clamps at the 10× ceiling.
        let refill = refill_bid(&consts, Some(roi), 550, 250);
        assert_eq!(refill, consts.refill_roi_cap_milli, "capped at the tuned 10× ceiling");

        // The §D1 ordering claim is pinned at the SEED horizon (750 — the ADR's own example
        // numbers); the tuned 1500 doubles the quiet-road bid but leaves every ordering below
        // intact except tower-vs-road (both sub-numeraire noise).
        let seed = MarketConsts { imminence_horizon_ticks: 750, ..consts };
        let road_40pct = repair_bid(&seed, 300, 30, imminence_q(2000, 100, seed.imminence_horizon_ticks));
        let upgrade = upgrade_bid(&consts, false);
        let ext_build = build_bid(&consts, BuildClass::Extension);
        let tower = tower_refill_bid(&consts, false);
        assert!(refill > ext_build, "refill {refill} > extension build {ext_build}");
        assert!(ext_build > upgrade, "extension build > upgrade");
        assert!(upgrade >= STORAGE_BID, "upgrade at or above par");
        assert!(STORAGE_BID > tower, "peaceful tower loses to the numeraire (S4 deleted)");
        assert!(tower > road_40pct, "a decayed road under base wear rides the bottom");
        // At the TUNED horizon the quiet 40% road still prices below the numeraire (§D1's
        // "roads ride low" survives the tuning).
        let road_tuned = repair_bid(&consts, 300, 30, imminence_q(2000, 100, consts.imminence_horizon_ticks));
        assert!(road_tuned < STORAGE_BID, "tuned quiet road stays below par ({road_tuned})");

        // The floor under stress IS the refill bid; upgrade withdraw is priced out (§D1's
        // "upgraders stop draining the container the refill hauler needs").
        let floor = opportunity_floor(&consts, [(refill, 300), (STORAGE_BID, 500_000)]);
        assert_eq!(floor, refill);
        assert!(!admit_use_withdraw(upgrade, floor));
    }

    /// **The DERIVED instant-spawnability premium** (review #1): the floor is the banking
    /// model's own arithmetic `1 + deficit/next_body`, not a swept constant. A full lane prices
    /// at par; a lane one small extension short barely clears par; a deep deficit relative to
    /// the next body prices the lane well above par.
    #[test]
    fn refill_floor_is_the_derived_banking_premium() {
        // Zero deficit ⇒ par exactly.
        assert_eq!(instant_spawnability_premium(0, 250), BID_SCALE, "a full lane is worth par");
        // A 50e extension short of a 250e next body: 1 + 50/250 = 1.2×.
        assert_eq!(instant_spawnability_premium(50, 250), 1200);
        // A whole 550e lane deficit against a 250e shuttle: 1 + 550/250 = 3.2× (the post-wipe
        // regime the flat 2500 approximated — now derived).
        assert_eq!(instant_spawnability_premium(550, 250), 3200);
        // Deficit as large as the body ⇒ exactly 2× (the deposit doubles the body's value).
        assert_eq!(instant_spawnability_premium(250, 250), 2000);
        assert_eq!(instant_spawnability_premium(100, 0), BID_SCALE, "no body to reason about");
    }

    /// The refill bid clamps: the DERIVED floor when nothing is blocked; the swept ROI cap on
    /// top; and the top-blocked ROI wins between them.
    #[test]
    fn refill_bid_clamps_to_derived_floor_and_cap() {
        let consts = c();
        // Unblocked queue rides at the derived premium (deficit 300 vs a 250 body ⇒ 2.2×).
        assert_eq!(refill_bid(&consts, None, 300, 250), 2200, "unblocked queue rides the derived floor");
        // A tiny top-blocked ROI still floors at the derived premium.
        assert_eq!(refill_bid(&consts, Some(1), 300, 250), 2200);
        // A full lane (deficit 0): the floor is par, so a mid ROI passes through.
        assert_eq!(refill_bid(&consts, Some(3000), 0, 250), 3000);
        // A huge ROI clamps at the cap regardless of the floor.
        assert_eq!(refill_bid(&consts, Some(999_999), 300, 250), consts.refill_roi_cap_milli, "capped");
        assert_eq!(body_roi_milli(1000, 0), 0, "degenerate zero-cost body enables nothing");
    }

    /// Healthy-room floor = the numeraire (spec Part A shape property): with storage present
    /// and everything else topped, the only materially-unmet deposit is storage at exactly
    /// 1.000.
    #[test]
    fn healthy_room_floor_is_the_numeraire() {
        let consts = c();
        let floor = opportunity_floor(
            &consts,
            [
                (STORAGE_BID, 900_000), // storage free capacity, bid exactly par
                (refill_bid(&consts, None, 0, 250), 0), // lane full: deficit 0 ⇒ par, amount 0 — moot
                (upgrade_bid(&consts, false), 20), // sub-material controller top-off
            ],
        );
        assert_eq!(floor, STORAGE_BID, "the healthy floor is the numeraire, exactly");
        // …and par sinks are admitted at the boundary (quantized tie admits).
        assert!(admit_use_withdraw(STORAGE_BID, floor));
        assert!(admit_use_withdraw(upgrade_bid(&consts, false), floor), "par upgrade competes");
    }

    /// The §D1 road story end-to-end (pinned at the SEED horizon 750 — the ADR's example
    /// numbers; the tuned 1500 shifts levels, not the shape): a 40% road under base decay
    /// prices ~0.36 (below the healthy floor, FAR below the stressed floor); the SAME road on
    /// a trafficked corridor (10× observed wear) prices above the healthy floor but still
    /// below the stressed one — "decay-only pricing kills trafficked corridors" is fixed by
    /// the wear term.
    #[test]
    fn road_pricing_matches_the_d1_claims() {
        let consts = MarketConsts { imminence_horizon_ticks: 750, ..c() };
        // 40% plain road: hits 2000/5000, repair 30e, rebuild 300e, base wear 100 milli-h/t.
        let imm_base = imminence_q(2000, 100, consts.imminence_horizon_ticks);
        assert_eq!(imm_base, 37, "750·100/2000 = 37‰");
        let quiet = repair_bid(&consts, 300, 30, imm_base);
        assert_eq!(quiet, 370, "≈0.37 — the §D1 '~0.3–0.4' claim");
        assert!(!admit_repair(quiet, STORAGE_BID), "below the healthy floor: rides low");

        let imm_traffic = imminence_q(2000, 1000, consts.imminence_horizon_ticks);
        let trafficked = repair_bid(&consts, 300, 30, imm_traffic);
        assert!(trafficked > STORAGE_BID, "trafficked corridor prices above the healthy floor ({trafficked})");
        assert!(trafficked < 12_000, "…but never beats the stressed refill floor");
        assert!(admit_repair(trafficked, STORAGE_BID));
        assert!(!admit_repair(trafficked, 12_000), "under collapse even the corridor waits");

        // Imminent death prices high (§D1): a near-dead road under base decay clears par.
        let dying = repair_bid(&consts, 300, 49, imminence_q(100, 100, consts.imminence_horizon_ticks));
        assert!(dying > STORAGE_BID, "imminent death prices high ({dying})");
        // The nearly-full blowup is dead: 1 missing hit prices under par (the cost floor).
        let nearly_full = repair_bid(&consts, 300, 1, imminence_q(4999, 100, consts.imminence_horizon_ticks));
        assert!(nearly_full < STORAGE_BID, "no spurious top-off bids ({nearly_full})");

        // The TUNED horizon (1500) keeps every claim's DIRECTION: quiet sub-par, trafficked
        // above the healthy floor, both below the stressed ceiling.
        let tuned = c();
        let quiet_t = repair_bid(&tuned, 300, 30, imminence_q(2000, 100, tuned.imminence_horizon_ticks));
        let traffic_t = repair_bid(&tuned, 300, 30, imminence_q(2000, 1000, tuned.imminence_horizon_ticks));
        assert!(quiet_t < STORAGE_BID && traffic_t > STORAGE_BID && traffic_t < tuned.refill_roi_cap_milli);
    }

    /// Imminence boundaries: clamps at [0, 1000]; zero wear never dies; zero hits is dying now.
    #[test]
    fn imminence_quantization_boundaries() {
        assert_eq!(imminence_q(1, 100_000, 750), 1000, "clamped at 1000");
        assert_eq!(imminence_q(5000, 0, 750), 0, "no wear ⇒ never dies");
        assert_eq!(imminence_q(0, 0, 750), 1000, "dead now");
        assert_eq!(imminence_q(75_000, 100, 750), 1, "the last whole milli");
        assert_eq!(imminence_q(75_001, 100, 750), 0, "…quantizes to zero past it");
    }

    /// Container arm: the <50% survival override is a veto outside the market; above it the
    /// functional-value term scales with imminence.
    #[test]
    fn container_override_and_function_term() {
        assert!(container_survival_override(124_999, 250_000));
        assert!(!container_survival_override(125_000, 250_000), "exactly 50% is NOT <50%");
        let consts = c();
        assert_eq!(container_function_milli(&consts, 1000), consts.container_throughput_milli);
        assert_eq!(container_function_milli(&consts, 500), consts.container_throughput_milli / 2);
        assert_eq!(container_function_milli(&consts, 0), 0);
    }

    /// The downgrade veto is the survival guardrail, not a bid: fires below half-max (the live
    /// upkeep boundary), off at it.
    #[test]
    fn downgrade_veto_boundary() {
        let consts = c();
        assert!(downgrade_veto(&consts, 9_999, 20_000));
        assert!(!downgrade_veto(&consts, 10_000, 20_000), "exactly half-max is safe");
    }

    /// The tier projection's exact boundaries (K1's M4 back-compat surface).
    #[test]
    fn bid_to_tier_boundaries() {
        assert_eq!(bid_to_tier(TIER_HIGH_MIN_MILLI), TransferPriority::High);
        assert_eq!(bid_to_tier(TIER_HIGH_MIN_MILLI - 1), TransferPriority::Medium);
        assert_eq!(bid_to_tier(TIER_MEDIUM_MIN_MILLI), TransferPriority::Medium);
        assert_eq!(bid_to_tier(TIER_MEDIUM_MIN_MILLI - 1), TransferPriority::Low);
        assert_eq!(bid_to_tier(BID_SCALE + 1), TransferPriority::Low);
        assert_eq!(bid_to_tier(BID_SCALE), TransferPriority::None, "par is the storage tier");
        assert_eq!(bid_to_tier(0), TransferPriority::None);
    }

    /// M5a bot bid vocabulary: `tier_to_bid` lands each tier inside its own `bid_to_tier`
    /// window, so a tier request and its numeric bid read as the SAME band (round-trip).
    #[test]
    fn tier_to_bid_round_trips() {
        for t in [
            TransferPriority::High,
            TransferPriority::Medium,
            TransferPriority::Low,
            TransferPriority::None,
        ] {
            assert_eq!(bid_to_tier(tier_to_bid(t)), t, "{t:?} round-trips through the numeric lane");
        }
        // The labels agree with the tiers.
        assert_eq!(bid_label(tier_to_bid(TransferPriority::High)), "High");
        assert_eq!(bid_label(tier_to_bid(TransferPriority::None)), "None");
        assert_eq!(bid_label(REFILL_ROI_CAP_MILLI), "High", "a stressed refill reads High");
    }

    /// The floor's materiality boundary: unmet exactly AT the minimum moves the floor; one
    /// below does not; an empty set floors at 0 (everything admits).
    #[test]
    fn floor_materiality_boundary() {
        let consts = c();
        assert_eq!(opportunity_floor(&consts, [(5000, consts.floor_material_min_e)]), 5000);
        assert_eq!(opportunity_floor(&consts, [(5000, consts.floor_material_min_e - 1)]), 0);
        assert_eq!(opportunity_floor(&consts, []), 0);
        assert!(admit_use_withdraw(0, 0), "no unmet demand: everything admits");
    }

    /// THE S6 pin (K4): post-wipe, the 250-cost shuttle beats the 1250-cost capacity body once
    /// time-to-afford is priced; with energy on hand the BIGGER body wins the per-energy-ROI
    /// tie; penalty 0 restores the banking arm (pure per-energy ROI); ties are deterministic.
    #[test]
    fn deficit_priced_bodies_fix_s6() {
        let consts = c();
        // Linear harvester ladder: [M,M,C,W]×r — cost 250r, w = min(2r, 10) e/t.
        let ladder: Vec<BodyCandidate> = (1..=5u32)
            .map(|r| BodyCandidate { cost: 250 * r, w_milli: (2000 * r).min(10_000) })
            .collect();

        // Post-wipe: 300e on hand, trickle income (1 e/t self-charge). The shuttle spawns NOW.
        let pick = deficit_priced_pick(&consts, &ladder, 300, 1000).unwrap();
        assert_eq!(ladder[pick].cost, 250, "affordable-now shuttle beats banking 950 ticks");

        // Energy on hand: the slot overhead resolves the linear ladder to the LARGEST
        // (saturating) body — a roster slot is scarce capital.
        let pick = deficit_priced_pick(&consts, &ladder, 1250, 1000).unwrap();
        assert_eq!(ladder[pick].cost, 1250, "capacity body wins when affordable now");

        // Healthy income (10 e/t): a SHORT bank (~70-95 ticks) for a near-saturating body is
        // worth it (the Family-G shape the pure per-energy tie got wrong — perpetual minimum
        // bodies never saturated the sources).
        let pick = deficit_priced_pick(&consts, &ladder, 300, 10_000).unwrap();
        assert!(ladder[pick].cost >= 1000, "healthy income banks briefly toward saturation (picked {})", ladder[pick].cost);

        // Saturation: a 5-WORK static-miner shape where extra parts add nothing — the cheap
        // saturating body beats the gold-plated one even with energy on hand (ADR §D2's
        // "300-cost body has 5× the ROI" shape).
        let saturated = [
            BodyCandidate { cost: 1700, w_milli: 10_000 },
            BodyCandidate { cost: 850, w_milli: 10_000 },
        ];
        let pick = deficit_priced_pick(&consts, &saturated, 1700, 1000).unwrap();
        assert_eq!(saturated[pick].cost, 850, "same w, half the cost: per-energy ROI decides");

        // The banking arm (penalty 0): pure per-energy ROI — the ladder ties, largest w wins,
        // i.e. the S6 capacity body IS selected (this is the sweep's off-arm, deliberately).
        let banking = MarketConsts { k4_wait_penalty_q: 0, ..consts };
        let pick = deficit_priced_pick(&banking, &banking_ladder(), 300, 1000).unwrap();
        assert_eq!(banking_ladder()[pick].cost, 1250, "penalty 0 restores capacity-banking");

        // Degenerate candidates are skipped; an all-degenerate set returns None.
        assert_eq!(deficit_priced_pick(&consts, &[BodyCandidate { cost: 0, w_milli: 5 }], 300, 1000), None);
        // Determinism: identical candidates resolve to the first index.
        let twins = [BodyCandidate { cost: 250, w_milli: 2000 }, BodyCandidate { cost: 250, w_milli: 2000 }];
        assert_eq!(deficit_priced_pick(&consts, &twins, 300, 1000), Some(0));
    }

    fn banking_ladder() -> Vec<BodyCandidate> {
        (1..=5u32)
            .map(|r| BodyCandidate { cost: 250 * r, w_milli: (2000 * r).min(10_000) })
            .collect()
    }
}
