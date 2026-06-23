//! Offense force-sizing oracle (ADR 0020 §12.2) — the pure, host-tested model that decides whether a
//! single squad can take a defended target and, if so, HOW (direct breach vs tower-drain). It replaces
//! the tower-count winnability proxy: it accounts for tower ENERGY (a drained tower deals no damage),
//! tower range/damage (the bot's tower curve), the breach-corridor cost (§12.3 — the cheapest corridor,
//! NOT a room-wide rampart sum), out-heal feasibility, and the squad's ON-SITE budget
//! (`CREEP_LIFE_TIME − spawn − travel`, supplied by the caller via
//! [`SquadComposition::estimated_combat_time`]).
//!
//! Inputs are scalars / plain data so the oracle is decoupled from the live `SquadComposition` and the
//! game API (host-testable, and the same math could drive the sim). `war.rs` builds the
//! [`DefenseProfile`] from `RoomThreatData` + the candidate objective, supplies a [`ForceBudget`] from
//! the chosen composition + RCL, and maps the verdict back to a sized composition.
//!
//! Conservative by construction (so a "yes" is safe to commit and a "no" defers, never the reverse):
//! unboosted, full tower DPS sustained across the whole drain, and **single squad only** — v1 has no
//! synchronized pre-spawn replacement (P2.S1), so a siege that can't finish within one creep lifetime
//! is judged unwinnable and deferred to the multi-squad **G4-HEAVY** path (P5), not committed.

use crate::military::damage::tower_attack_damage_at_range;

/// Energy a tower spends per shot — below this it cannot fire (engine `TOWER_ENERGY_COST`).
const TOWER_ENERGY_COST: u32 = 10;

/// HOLD margin (ADR 0020 §12.5/§12.6): size heal to out-heal the incoming damage by this factor, NOT
/// break-even — so the squad HEALS THROUGH transient / approach / focused damage instead of tripping the
/// runtime `assess_engage` retreat on the first hit, and so `assess_engage`'s `tower_dps > our_heal` veto
/// stays clear. Also the commit gate: only field a squad whose margin-heal is affordable (never commit a
/// fragile break-even squad). Seed; tuned by the SK/sim scenarios (R5 makes it importance·P(win)-driven).
pub(crate) const HOLD_MARGIN: f32 = 1.3;

/// One hostile tower's threat to the planned assault position.
#[derive(Clone, Copy, Debug)]
pub struct TowerThreat {
    /// Chebyshev range from the tower to the assault tile (the tower-damage curve's input).
    pub range_to_assault: u32,
    /// Current stored energy; a tower with `< TOWER_ENERGY_COST` can't fire (counts as 0 DPS).
    pub energy: u32,
}

/// The target's defense as the oracle sees it — built bot-side from `RoomThreatData` + the objective.
#[derive(Clone, Debug, Default)]
pub struct DefenseProfile {
    pub towers: Vec<TowerThreat>,
    /// Breach-corridor hits to the objective (ADR 0020 §12.3; 0 = already reachable without dismantling).
    pub breach_hits: u32,
    /// Objective structure hits to destroy once reached (e.g. the invader core itself).
    pub objective_hits: u32,
    /// Hostile creep damage/tick at the objective.
    pub enemy_dps: f32,
    /// Defensive repair/tick of the breach target (tower/creep repair of ramparts); 0 for cores.
    pub repair_per_tick: f32,
    /// Owner safe-mode active → zero damage possible → a hard veto.
    pub safe_mode: bool,
}

/// What ONE squad brings + how long it has on-site. `onsite_budget_ticks` =
/// `CREEP_LIFE_TIME − spawn − travel` (from [`SquadComposition::estimated_combat_time`]).
#[derive(Clone, Copy, Debug)]
pub struct ForceBudget {
    pub max_heal_per_tick: f32,
    pub max_dismantle_dps: f32,
    /// Effective HP of the squad's tank / front member — the drain-survival reserve.
    pub tank_effective_hp: f32,
    pub onsite_budget_ticks: u32,
}

/// How a winnable target is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssaultMode {
    /// Out-heal the towers and dismantle straight through the breach.
    Breach,
    /// A tank soaks tower fire until the towers run dry (10 energy/shot), then the squad breaches the
    /// dead base. Still a SINGLE squad (the tank drains, then the same squad dismantles).
    Drain,
}

/// The oracle's verdict for one squad vs one defense.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForceAssessment {
    pub winnable: bool,
    pub mode: AssaultMode,
    /// Heal/tick the fielded squad must sustain (the binding survival constraint of the chosen mode).
    pub required_heal_per_tick: f32,
    /// Dismantle DPS (net of repair) the fielded squad must bring.
    pub required_dismantle_dps: f32,
    /// Estimated ticks to win (drain + breach + kill) — for ROI / the war supervisor.
    pub est_ticks: u32,
    /// Why unwinnable, or the chosen mode — for logging.
    pub reason: &'static str,
}

/// Ticks to deliver `amount` at `rate`/tick (ceil). `rate <= 0` ⇒ never (`u32::MAX`).
fn ticks_for(amount: f32, rate: f32) -> u32 {
    if rate <= 0.0 {
        u32::MAX
    } else {
        (amount / rate).ceil() as u32
    }
}

/// Damage/tick of the ENERGIZED towers at the assault position (the bot tower curve; drained towers
/// contribute 0 — the fix the per-tower energy intel enables).
fn tower_dps_at_assault(towers: &[TowerThreat]) -> f32 {
    towers
        .iter()
        .filter(|t| t.energy >= TOWER_ENERGY_COST)
        .map(|t| tower_attack_damage_at_range(t.range_to_assault))
        .sum()
}

/// Ticks for the energized towers to run dry under sustained fire (each fires once/tick, −10 energy);
/// the slowest tower (the last to go silent) bounds the drain.
fn drain_ticks(towers: &[TowerThreat]) -> u32 {
    towers
        .iter()
        .filter(|t| t.energy >= TOWER_ENERGY_COST)
        .map(|t| t.energy.div_ceil(TOWER_ENERGY_COST))
        .max()
        .unwrap_or(0)
}

/// The force-sizing oracle (ADR 0020 §12.2): can `budget` (a single squad) beat `profile`, and via
/// which mode? See the module docs for the conservatism contract.
pub fn assess(profile: &DefenseProfile, budget: &ForceBudget) -> ForceAssessment {
    let unwinnable = |reason| ForceAssessment {
        winnable: false,
        mode: AssaultMode::Breach,
        required_heal_per_tick: 0.0,
        required_dismantle_dps: 0.0,
        est_ticks: 0,
        reason,
    };

    if profile.safe_mode {
        return unwinnable("enemy safe mode — zero damage possible");
    }

    let net_dismantle = budget.max_dismantle_dps - profile.repair_per_tick;
    if profile.breach_hits > 0 && net_dismantle <= 0.0 {
        return unwinnable("repair out-paces our dismantle");
    }
    let breach_ticks = ticks_for(profile.breach_hits as f32, net_dismantle.max(1.0));
    let kill_ticks = ticks_for(profile.objective_hits as f32, budget.max_dismantle_dps.max(1.0));

    let tower_dps = tower_dps_at_assault(&profile.towers);
    let incoming = tower_dps + profile.enemy_dps;

    // Direct breach: out-heal towers + creeps the whole time (with the HOLD margin so HP recovers
    // through damage and the squad doesn't early-retreat), dismantle through.
    let required_heal = incoming * HOLD_MARGIN;
    if required_heal <= budget.max_heal_per_tick {
        let total = breach_ticks.saturating_add(kill_ticks);
        if total <= budget.onsite_budget_ticks {
            return ForceAssessment {
                winnable: true,
                mode: AssaultMode::Breach,
                required_heal_per_tick: required_heal,
                required_dismantle_dps: net_dismantle.max(1.0),
                est_ticks: total,
                reason: "breach: out-heal the towers and dismantle through",
            };
        }
        return unwinnable("breach too slow for one creep lifetime");
    }

    // Drain: a tank soaks tower fire until the towers run dry, then the squad breaches the dead base.
    let dt = drain_ticks(&profile.towers);
    let tank_sustain = budget.tank_effective_hp + budget.max_heal_per_tick * dt as f32;
    let drain_damage = tower_dps * dt as f32;
    if dt > 0 && tank_sustain >= drain_damage {
        // After the drain only the enemy creeps remain — they must be out-healed (with the HOLD margin)
        // for the breach phase.
        let required_heal = profile.enemy_dps.max(1.0) * HOLD_MARGIN;
        if required_heal <= budget.max_heal_per_tick {
            let total = dt.saturating_add(breach_ticks).saturating_add(kill_ticks);
            if total <= budget.onsite_budget_ticks {
                return ForceAssessment {
                    winnable: true,
                    mode: AssaultMode::Drain,
                    required_heal_per_tick: required_heal,
                    required_dismantle_dps: net_dismantle.max(1.0),
                    est_ticks: total,
                    reason: "drain: soak the towers dry, then breach",
                };
            }
            return unwinnable("drain + breach too slow for one creep lifetime");
        }
        return unwinnable("enemy creeps out-heal our damage after the drain");
    }

    unwinnable("towers out-damage a single squad — needs heavy assault (G4-HEAVY)")
}

// ─── R2: required-force → part counts (ADR 0020 §12.6) ───────────────────────
//
// The inverse of `SquadComposition::capabilities()` (P2a, forward): turn the oracle's required
// CAPABILITIES into the total PARTS a squad must field. R3 distributes these across the role structure
// and builds member bodies via `bodies::build_combat_body` (R1); the gate then becomes "can an in-range
// home afford these parts?". Reuses the existing defense-path part math (`defender_heal_parts_for_dps`)
// so heal sizing is consistent across defense and offense.

/// WORK dismantle per part/tick (engine `DISMANTLE_POWER`).
const DISMANTLE_POWER: u32 = 50;

/// Total parts a squad must field to satisfy a [`ForceAssessment`] (R2). R3 splits these across the
/// composition's roles + builds bodies (R1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequiredForce {
    /// Σ HEAL parts — out-heal the assault position (`required_heal_per_tick`).
    pub heal_parts: u32,
    /// Σ WORK parts — breach + kill the structure (`required_dismantle_dps`).
    pub dismantle_parts: u32,
    /// Σ TOUGH parts — the effective-HP buffer. v1 = 0 (role bodies carry their own HP); the
    /// margin-driven EHP buffer is R5/D2.
    pub tough_parts: u32,
}

impl RequiredForce {
    /// Map a winnable assessment to total part counts. Reuses `defender_heal_parts_for_dps` (incoming-
    /// dps → HEAL parts) so heal sizing matches the defense path. Unwinnable ⇒ all-zero (nothing to field).
    pub fn from_assessment(a: &ForceAssessment) -> Self {
        if !a.winnable {
            return Self::default();
        }
        RequiredForce {
            heal_parts: crate::military::damage::defender_heal_parts_for_dps(a.required_heal_per_tick, false),
            dismantle_parts: parts_for_rate(a.required_dismantle_dps, DISMANTLE_POWER),
            tough_parts: 0,
        }
    }

    /// As a single-creep [`CombatBodySpec`] — the solo case + the R1 round-trip seam. R3 splits the
    /// totals across the squad's members instead of stacking them on one creep.
    pub fn as_solo_spec(&self) -> crate::military::bodies::CombatBodySpec {
        crate::military::bodies::CombatBodySpec {
            heal: self.heal_parts,
            work: self.dismantle_parts,
            tough: self.tough_parts,
            ..Default::default()
        }
    }

    /// Scale every part count up by `factor` (ceil), keeping zeros at zero (R5 importance-weighted
    /// investment). `factor >= 1.0` over-invests for high-value targets (more margin → higher P(win));
    /// `factor == 1.0` is a no-op. Used to lift the base hold-margin force by [`importance_margin`].
    pub fn scaled(self, factor: f32) -> RequiredForce {
        let s = |n: u32| if n == 0 { 0 } else { (n as f32 * factor.max(1.0)).ceil() as u32 };
        RequiredForce {
            heal_parts: s(self.heal_parts),
            dismantle_parts: s(self.dismantle_parts),
            tough_parts: s(self.tough_parts),
        }
    }
}

/// R4 — probability we win/hold the engagement given our sustained `heal` vs the `incoming` damage.
/// A logistic on the heal surplus (`heal/incoming - 1`): 0.5 at break-even, rising as heal exceeds
/// incoming, → 1 when nothing hits us. This is the principled reading of the [`HOLD_MARGIN`]: a 1.3×
/// margin (+30% surplus) lands ≈ 0.82, i.e. "field enough to win ~4 times in 5". Used to log the
/// fielded force's confidence and (via [`importance_margin`]) to decide how hard to over-invest.
pub fn win_probability(heal: f32, incoming: f32) -> f32 {
    if incoming <= 0.0 {
        return 1.0;
    }
    let surplus = heal / incoming - 1.0;
    1.0 / (1.0 + (-WIN_PROB_STEEPNESS * surplus).exp())
}

/// Logistic steepness for [`win_probability`], tuned so break-even = 0.5 and the +30% [`HOLD_MARGIN`]
/// surplus ≈ 0.82.
const WIN_PROB_STEEPNESS: f32 = 5.0;

/// R5 — extra force multiplier for objective `importance` ∈ [0,1]: over-invest (more margin, higher
/// P(win)) for high-value targets, down to a no-op (1.0) for marginal ones. Multiplies the base
/// hold-margin [`RequiredForce`] via [`RequiredForce::scaled`].
pub fn importance_margin(importance: f32) -> f32 {
    1.0 + importance.clamp(0.0, 1.0) * IMPORTANCE_MAX_EXTRA
}

/// Most a fully-important objective adds on top of the base hold margin (a CRITICAL target fields
/// 1.5× the minimum winning force).
const IMPORTANCE_MAX_EXTRA: f32 = 0.5;

/// Parts to deliver `rate`/tick at `power`/part (ceil). 0 when nothing is required.
fn parts_for_rate(rate: f32, power: u32) -> u32 {
    if rate <= 0.0 || power == 0 {
        0
    } else {
        (rate / power as f32).ceil() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── R2: RequiredForce (capability → parts) ──
    fn assessment(winnable: bool, heal: f32, dps: f32) -> ForceAssessment {
        ForceAssessment {
            winnable,
            mode: AssaultMode::Breach,
            required_heal_per_tick: heal,
            required_dismantle_dps: dps,
            est_ticks: 50,
            reason: "test",
        }
    }

    #[test]
    fn required_force_inverts_capabilities_with_ceil() {
        let rf = RequiredForce::from_assessment(&assessment(true, 120.0, 300.0));
        assert_eq!(rf.heal_parts, 10, "120 dmg/tick ÷ 12 HEAL/part");
        assert_eq!(rf.dismantle_parts, 6, "300 dps ÷ 50 DISMANTLE/part");
        // Round-trip: the fielded parts meet-or-exceed the requirement (ceil, never under).
        assert!(rf.heal_parts * 12 >= 120 && rf.dismantle_parts * DISMANTLE_POWER >= 300);
    }

    #[test]
    fn required_force_is_zero_when_unwinnable() {
        assert_eq!(RequiredForce::from_assessment(&assessment(false, 999.0, 999.0)), RequiredForce::default());
    }

    #[test]
    fn required_force_spec_is_buildable_by_r1() {
        // R1∘R2 seam: the spec R2 produces builds into a real body at RCL7 energy.
        let rf = RequiredForce::from_assessment(&assessment(true, 120.0, 300.0));
        let spec = rf.as_solo_spec();
        assert_eq!(spec.heal, 10);
        assert_eq!(spec.work, 6);
        assert!(
            crate::military::bodies::build_combat_body(&spec, crate::military::bodies::MoveProfile::Plains, 5600).is_some(),
            "the required-force spec is affordable + fits at RCL7"
        );
    }

    /// A budget that can heal/dismantle a lot with a long on-site window — so the DEFENSE drives each
    /// test's outcome, not the budget.
    fn strong_budget() -> ForceBudget {
        ForceBudget {
            // Heal headroom so the HOLD_MARGIN (1.3×) still affords a direct breach vs a 600-dps tower
            // (600 × 1.3 = 780 ≤ 900) — the DEFENSE, not the budget, drives each test's outcome.
            max_heal_per_tick: 900.0,
            max_dismantle_dps: 600.0,
            tank_effective_hp: 50_000.0,
            onsite_budget_ticks: 1400,
        }
    }

    fn tower(range: u32, energy: u32) -> TowerThreat {
        TowerThreat { range_to_assault: range, energy }
    }

    #[test]
    fn safe_mode_is_a_hard_veto() {
        let profile = DefenseProfile { safe_mode: true, ..Default::default() };
        assert!(!assess(&profile, &strong_budget()).winnable);
    }

    #[test]
    fn weak_single_tower_is_a_direct_breach() {
        // One energized tower at range 5 = 600 dps; a 600 heal/tick squad out-heals it and breaches.
        let profile = DefenseProfile {
            towers: vec![tower(5, 1000)],
            breach_hits: 30_000,
            objective_hits: 100_000,
            ..Default::default()
        };
        let a = assess(&profile, &strong_budget());
        assert!(a.winnable);
        assert_eq!(a.mode, AssaultMode::Breach);
    }

    #[test]
    fn drained_towers_do_not_count() {
        // Three towers but all below the firing threshold → no incoming → trivially a direct breach,
        // and a modest-heal budget that could NOT out-heal three live towers still wins.
        let profile = DefenseProfile {
            towers: vec![tower(1, 5), tower(1, 0), tower(1, 9)],
            breach_hits: 10_000,
            objective_hits: 50_000,
            ..Default::default()
        };
        let weak_heal = ForceBudget { max_heal_per_tick: 50.0, ..strong_budget() };
        let a = assess(&profile, &weak_heal);
        assert!(a.winnable, "drained towers deal no damage, so this is winnable: {}", a.reason);
        assert_eq!(a.mode, AssaultMode::Breach);
    }

    #[test]
    fn strong_towers_force_the_drain_path() {
        // Six energized towers at range 1 (max 600 each = 3600 dps) — un-out-healable by a 600-heal
        // squad, but each holds little energy (100 → 10 shots), so a 50k-EHP tank survives the ~10-tick
        // drain (50k + 600×10 = 56k ≥ 3600×10 = 36k) and the squad breaches the dead base after.
        let profile = DefenseProfile {
            towers: vec![tower(1, 100); 6],
            breach_hits: 20_000,
            objective_hits: 80_000,
            ..Default::default()
        };
        let a = assess(&profile, &strong_budget());
        assert!(a.winnable, "should be drainable: {}", a.reason);
        assert_eq!(a.mode, AssaultMode::Drain);
    }

    #[test]
    fn deep_energy_towers_are_unwinnable_for_one_squad() {
        // Six full towers (range 1, 3600 dps) with deep energy: the drain takes too long and the tank
        // can't survive it → defer to heavy assault rather than feed a squad to its death.
        let profile = DefenseProfile {
            towers: vec![tower(1, 100_000); 6],
            breach_hits: 20_000,
            objective_hits: 80_000,
            ..Default::default()
        };
        let a = assess(&profile, &strong_budget());
        assert!(!a.winnable);
        assert!(a.reason.contains("heavy assault"), "reason: {}", a.reason);
    }

    #[test]
    fn breach_too_slow_for_one_lifetime_is_unwinnable() {
        // A huge wall behind a single weak tower: out-healable, but un-dismantlable in one lifetime
        // at the budget's DPS → unwinnable (a real force calc, not a tower count).
        let profile = DefenseProfile {
            towers: vec![tower(5, 1000)],
            breach_hits: 10_000_000,
            objective_hits: 100_000,
            ..Default::default()
        };
        let a = assess(&profile, &strong_budget());
        assert!(!a.winnable);
        assert!(a.reason.contains("too slow"), "reason: {}", a.reason);
    }

    #[test]
    fn undefended_room_is_a_no_breach_win() {
        let profile = DefenseProfile { objective_hits: 50_000, ..Default::default() };
        let a = assess(&profile, &strong_budget());
        assert!(a.winnable);
        assert_eq!(a.mode, AssaultMode::Breach);
        assert_eq!(a.required_heal_per_tick, 0.0, "nothing is shooting us");
    }

    // ── R4: P(win) model ──
    #[test]
    fn win_probability_reads_the_hold_margin() {
        assert_eq!(win_probability(100.0, 0.0), 1.0, "nothing hitting us → certain");
        assert!((win_probability(100.0, 100.0) - 0.5).abs() < 1e-3, "break-even → coin-flip");
        // The +30% HOLD_MARGIN is a ~0.82 win — the principled reading of the magic 1.3.
        let p = win_probability(130.0, 100.0);
        assert!(p > 0.80 && p < 0.85, "the 1.3 hold margin ≈ 0.82 P(win), got {p}");
        // Monotone: more heal surplus is never less confidence.
        assert!(win_probability(200.0, 100.0) > win_probability(130.0, 100.0));
    }

    // ── R5: importance-weighted investment ──
    #[test]
    fn importance_scales_the_invested_force() {
        // A marginal target (importance 0) fields exactly the base hold-margin force …
        assert_eq!(importance_margin(0.0), 1.0);
        let base = RequiredForce { heal_parts: 10, dismantle_parts: 6, tough_parts: 0 };
        assert_eq!(base.scaled(importance_margin(0.0)), base, "importance 0 → no over-invest");
        // … a CRITICAL target (importance 1) over-invests by IMPORTANCE_MAX_EXTRA (1.5×), lifting P(win).
        assert_eq!(importance_margin(1.0), 1.5);
        let crit = base.scaled(importance_margin(1.0));
        assert_eq!(crit.heal_parts, 15, "10 × 1.5");
        assert_eq!(crit.dismantle_parts, 9, "6 × 1.5");
        // Scaling never adds parts to a role that needs none, and never shrinks below the base.
        assert_eq!(crit.tough_parts, 0, "zero stays zero");
        assert!(crit.heal_parts >= base.heal_parts && base.scaled(0.5) == base, "factor < 1 is clamped to no-op");
    }
}
