//! Source Keeper room exploitation — the economy-family operation (ADR 0018).
//!
//! SK rooms are unclaimable, controller-less rooms (the ring around each sector
//! centre) holding 3 sources (4000/regen — the highest-yield remote) + 1
//! mineral, each guarded by a 5000-HP keeper that respawns 300t after damage.
//! This operation decides *which* adjacent SK rooms are worth farming (the pure
//! ROI scorer below, P2.K2a) and owns a persistent `SourceKeeperFarmMission`
//! that suppresses the keepers and mines around them (P2.K2b/c).
//!
//! The scorer is **pure arithmetic** (no `game::*`) so the commit/withhold/veto
//! gate is unit-testable against hand-computed energy/tick — see ADR 0018 §3.2.

// ─── ROI scorer (P2.K2a) — pure, kernel-testable ────────────────────────────

// Engine values (mirrored locally so the scorer stays a pure kernel; provenance
// in the doc names). See `docs/references/engine-mechanics.md`.
/// `SOURCE_ENERGY_KEEPER_CAPACITY` — an SK source holds 4000 per regen cycle.
const SK_SOURCE_CAPACITY: f64 = 4000.0;
/// `ENERGY_REGEN_TIME` — sources refill on a 300-tick timer.
const SOURCE_REGEN_TICKS: f64 = 300.0;
/// `HARVEST_POWER` — energy harvested per WORK part per tick.
const HARVEST_POWER: f64 = 2.0;
/// `CARRY_CAPACITY` — resources a CARRY part holds.
const CARRY_CAPACITY: f64 = 50.0;
/// `CREEP_LIFE_TIME` — body-spawn cost amortizes over this many ticks.
const CREEP_LIFETIME: f64 = 1500.0;
/// `BODYPART_COST` for WORK / CARRY / MOVE (the parts the model sizes).
const WORK_COST: f64 = 100.0;
const CARRY_COST: f64 = 50.0;
const MOVE_COST: f64 = 50.0;
/// Rough per-tile pathfinding-CPU charge (in energy-equivalent e/t) so distant
/// farms are penalised even when the haul body alone would pencil out.
const CPU_PENALTY_PER_TILE: f64 = 0.02;

/// Net e/t a candidate must clear to *start* a farm. Tunable (sim-calibrated).
pub const MIN_SK_ROI: f64 = 5.0;
/// Hysteresis band: an already-committed farm is only withdrawn once net falls
/// below `MIN_SK_ROI − SK_ROI_HYSTERESIS`, so a farm never flaps on a marginal
/// estimate or a transient spawn-pressure blip.
pub const SK_ROI_HYSTERESIS: f64 = 3.0;

/// Inputs to the SK-farm decision — all derivable from intel + already-tracked
/// state by the operation (§3.2). No `game::*`.
#[derive(Debug, Clone, Copy)]
pub struct SkRoiInputs {
    /// Harvestable sources in the SK room (typically 3).
    pub live_sources: u32,
    /// One-way path length (tiles) to the nearest capable home.
    pub haul_tiles: u32,
    /// Total body-energy of the suppression duo (`duo_sk_farmer`) — the op
    /// computes this from the composition; amortized over a creep lifetime.
    pub suppression_cost: u32,
    /// The nearest home can actually spawn the farm's creeps (op-computed).
    pub affordable: bool,
    /// Another player is farming it / hostile reservation / non-keeper
    /// hostiles — a hard veto (we don't contest an SK farm).
    pub contested: bool,
    /// CPU tier is not critical (ADR 0004).
    pub cpu_ok: bool,
    /// Military capacity is not wanted for active defense / declared war (ADR 0014).
    pub military_free: bool,
    /// Below `max_concurrent_sk_farms` (only gates *new* commitments).
    pub under_farm_cap: bool,
    /// We are already farming this room — applies the withdraw-hysteresis floor.
    pub already_committed: bool,
}

/// The commit/withhold/veto verdict for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkRoiDecision {
    /// Start (or keep) the farm.
    Commit,
    /// Profitable enough is not met — don't farm (but no hard blocker).
    Withhold,
    /// A hard gate failed (contested / CPU / military / unaffordable / over cap).
    Veto,
}

/// Scored candidate: the gross + net energy/tick and the verdict.
#[derive(Debug, Clone, Copy)]
pub struct SkRoiScore {
    pub gross_per_tick: f64,
    pub net_per_tick: f64,
    pub decision: SkRoiDecision,
}

/// Score one SK-room candidate. Pure: hard gates first, then the energy/tick
/// model `net = gross − suppression − mining − haul − cpu`, then the
/// floor + hysteresis decision (ADR 0018 §3.2).
pub fn score_sk_farm(inp: &SkRoiInputs) -> SkRoiScore {
    // Hard gates — no farm regardless of yield.
    let vetoed = inp.contested
        || !inp.cpu_ok
        || !inp.military_free
        || !inp.affordable
        || (!inp.already_committed && !inp.under_farm_cap);
    if vetoed {
        return SkRoiScore { gross_per_tick: 0.0, net_per_tick: 0.0, decision: SkRoiDecision::Veto };
    }

    let n = inp.live_sources as f64;
    let gross = n * SK_SOURCE_CAPACITY / SOURCE_REGEN_TICKS;

    // Suppression: the duo's body cost, renewed each lifetime.
    let suppression = inp.suppression_cost as f64 / CREEP_LIFETIME;

    // Mining: WORK to saturate the yield + ~2 MOVE per source-miner, amortized.
    let work_parts = (gross / HARVEST_POWER).ceil();
    let mining_body = work_parts * WORK_COST + n * 2.0 * MOVE_COST;
    let mining = mining_body / CREEP_LIFETIME;

    // Haul: CARRY to move `gross` over a `2 × dist` round trip (+ matched MOVE),
    // amortized. This is the term that grows with distance and kills far farms.
    let carry_parts = gross * 2.0 * inp.haul_tiles as f64 / CARRY_CAPACITY;
    let haul_body = carry_parts * (CARRY_COST + MOVE_COST);
    let haul = haul_body / CREEP_LIFETIME;

    let cpu_penalty = inp.haul_tiles as f64 * CPU_PENALTY_PER_TILE;

    let net = gross - suppression - mining - haul - cpu_penalty;

    let floor = if inp.already_committed { MIN_SK_ROI - SK_ROI_HYSTERESIS } else { MIN_SK_ROI };
    let decision = if net >= floor { SkRoiDecision::Commit } else { SkRoiDecision::Withhold };

    SkRoiScore { gross_per_tick: gross, net_per_tick: net, decision }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean 3-source candidate with all gates open and a typical duo cost.
    fn nearby() -> SkRoiInputs {
        SkRoiInputs {
            live_sources: 3,
            haul_tiles: 50, // one room away
            suppression_cost: 5350,
            affordable: true,
            contested: false,
            cpu_ok: true,
            military_free: true,
            under_farm_cap: true,
            already_committed: false,
        }
    }

    #[test]
    fn a_close_three_source_room_commits_and_is_strongly_positive() {
        let s = score_sk_farm(&nearby());
        assert_eq!(s.decision, SkRoiDecision::Commit);
        // gross = 3 * 4000 / 300 = 40 e/t; net well above the floor.
        assert!((s.gross_per_tick - 40.0).abs() < 1e-9);
        assert!(s.net_per_tick > 20.0, "net {}", s.net_per_tick);
    }

    #[test]
    fn distance_eventually_makes_it_unprofitable() {
        let close = score_sk_farm(&nearby());
        let far = score_sk_farm(&SkRoiInputs { haul_tiles: 600, ..nearby() });
        assert!(far.net_per_tick < close.net_per_tick, "haul cost must grow with distance");
        assert_eq!(far.decision, SkRoiDecision::Withhold, "a far SK room is not worth the haul");
    }

    #[test]
    fn hard_gates_veto_regardless_of_yield() {
        for bad in [
            SkRoiInputs { contested: true, ..nearby() },
            SkRoiInputs { cpu_ok: false, ..nearby() },
            SkRoiInputs { military_free: false, ..nearby() },
            SkRoiInputs { affordable: false, ..nearby() },
            SkRoiInputs { under_farm_cap: false, ..nearby() }, // not yet committed + over cap
        ] {
            assert_eq!(score_sk_farm(&bad).decision, SkRoiDecision::Veto);
        }
    }

    #[test]
    fn an_existing_farm_keeps_its_slot_over_the_concurrency_cap() {
        // Over the cap but already committed → the cap no longer vetoes it.
        let existing = SkRoiInputs { under_farm_cap: false, already_committed: true, ..nearby() };
        assert_eq!(score_sk_farm(&existing).decision, SkRoiDecision::Commit);
    }

    #[test]
    fn hysteresis_keeps_a_marginal_farm_but_will_not_start_one() {
        // ~250 tiles lands net ≈ 3.2 e/t — inside the band [MIN−h, MIN) = [2, 5).
        let marginal = SkRoiInputs { haul_tiles: 250, ..nearby() };
        let fresh = score_sk_farm(&marginal);
        let committed = score_sk_farm(&SkRoiInputs { already_committed: true, ..marginal });

        assert!(
            fresh.net_per_tick >= MIN_SK_ROI - SK_ROI_HYSTERESIS && fresh.net_per_tick < MIN_SK_ROI,
            "expected a band net, got {}",
            fresh.net_per_tick
        );
        assert_eq!(fresh.decision, SkRoiDecision::Withhold, "won't START a marginal farm");
        assert_eq!(committed.decision, SkRoiDecision::Commit, "won't DROP an existing marginal farm");
    }
}
