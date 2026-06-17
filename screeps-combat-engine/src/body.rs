//! The creep body model — per-part 100-hit pools, back-to-front degradation, boost-aware
//! action power (`calcBodyEffectiveness`), and the TOUGH/boost damage reduction (`_applyDamage`).
//! Faithful to the engine; see the cited source per function.

use crate::constants::*;
use screeps::Part;

/// Boost tier for a body part. The engine keys boosts by mineral (`BOOSTS[type][mineral]`); the
/// three tiers per part type map exactly onto these multipliers, so a tier abstraction is faithful
/// and avoids threading mineral `ResourceType`s through the sim. The live `CombatView` adapter
/// (H2) maps `ResourceType -> BoostTier` when ingesting a real creep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BoostTier {
    #[default]
    None,
    T1,
    T2,
    T3,
}

impl BoostTier {
    /// Multiplier for an offensive/heal/dismantle action (attack/rangedAttack/heal/dismantle):
    /// ×1/2/3/4 (`BOOSTS[ATTACK][UH|UH2O|XUH2O].attack` etc.).
    pub fn action_mult(self) -> f64 {
        match self {
            BoostTier::None => 1.0,
            BoostTier::T1 => 2.0,
            BoostTier::T2 => 3.0,
            BoostTier::T3 => 4.0,
        }
    }

    /// Incoming-damage multiplier for a TOUGH part (`BOOSTS[TOUGH][GO|GHO2|XGHO2].damage`):
    /// 1.0 / 0.7 / 0.5 / 0.3. Lower = more mitigation.
    pub fn tough_damage_ratio(self) -> f64 {
        match self {
            BoostTier::None => 1.0,
            BoostTier::T1 => 0.7,
            BoostTier::T2 => 0.5,
            BoostTier::T3 => 0.3,
        }
    }

    /// Fatigue-clear multiplier for a MOVE part (`BOOSTS[MOVE][ZO|ZHO2|XZHO2].fatigue`): ×1/2/3/4.
    pub fn move_mult(self) -> f64 {
        self.action_mult()
    }
}

/// One body part: its type and boost tier. (Per-part current hits are *derived* from the body
/// total via [`SimBody::part_hits`], exactly as the engine recomputes them in `_recalc-body`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyPartDef {
    pub part: Part,
    pub boost: BoostTier,
}

impl BodyPartDef {
    pub fn new(part: Part) -> Self {
        Self {
            part,
            boost: BoostTier::None,
        }
    }
    pub fn boosted(part: Part, boost: BoostTier) -> Self {
        Self { part, boost }
    }
}

/// A creep body: an ordered part list (front = index 0) plus the single `hits` total. Part hits
/// are derived from `hits`, not stored, matching the engine's `object.hits` + `_recalc-body`.
#[derive(Clone, Debug)]
pub struct SimBody {
    /// Ordered front (index 0) → back. Front parts degrade first (engine `_recalc-body`), so put
    /// TOUGH/expendable parts front and MOVE/HEAL back.
    pub parts: Vec<BodyPartDef>,
    /// Current total hit points (0..=`hits_max`).
    pub hits: u32,
}

impl SimBody {
    /// A full-health body.
    pub fn new(parts: Vec<BodyPartDef>) -> Self {
        let hits = parts.len() as u32 * BODYPART_HITS;
        Self { parts, hits }
    }

    /// Convenience: a full-health unboosted body from a part slice.
    pub fn unboosted(parts: &[Part]) -> Self {
        Self::new(parts.iter().map(|&p| BodyPartDef::new(p)).collect())
    }

    pub fn hits_max(&self) -> u32 {
        self.parts.len() as u32 * BODYPART_HITS
    }

    pub fn is_alive(&self) -> bool {
        self.hits > 0
    }

    /// Current hits of part `i`, derived back-to-front (engine `_recalc-body.js`: the loop fills
    /// from the last part toward index 0, 100 each). So trailing parts stay full and `body[0]`
    /// (front) is the first to drop to 0 — the basis for "TOUGH front, MOVE back".
    pub fn part_hits(&self, i: usize) -> u32 {
        let len = self.parts.len();
        if i >= len {
            return 0;
        }
        let behind = (len - 1 - i) as i64; // parts after `i` fill before it
        (self.hits as i64 - BODYPART_HITS as i64 * behind).clamp(0, BODYPART_HITS as i64) as u32
    }

    /// `calcBodyEffectiveness(body, part_type, _, base)` (`utils.js:623`): sum over **alive**
    /// (`hits > 0`) parts of `part_type` of `base × boost_mult`.
    pub fn effective_power(&self, part_type: Part, base: u32) -> u32 {
        let mut power = 0.0;
        for (i, p) in self.parts.iter().enumerate() {
            if p.part == part_type && self.part_hits(i) > 0 {
                power += base as f64 * p.boost.action_mult();
            }
        }
        power as u32
    }

    pub fn attack_power(&self) -> u32 {
        self.effective_power(Part::Attack, ATTACK_POWER)
    }
    pub fn ranged_attack_power(&self) -> u32 {
        self.effective_power(Part::RangedAttack, RANGED_ATTACK_POWER)
    }
    pub fn heal_power(&self) -> u32 {
        self.effective_power(Part::Heal, HEAL_POWER)
    }
    pub fn ranged_heal_power(&self) -> u32 {
        self.effective_power(Part::Heal, RANGED_HEAL_POWER)
    }
    pub fn dismantle_power(&self) -> u32 {
        self.effective_power(Part::Work, DISMANTLE_POWER)
    }

    /// Fatigue cleared per tick: `2 × Σ alive MOVE parts` (boost-weighted), per `creeps/tick.js:107`.
    pub fn fatigue_clear(&self) -> u32 {
        let mut mult = 0.0;
        for (i, p) in self.parts.iter().enumerate() {
            if p.part == Part::Move && self.part_hits(i) > 0 {
                mult += p.boost.move_mult();
            }
        }
        (FATIGUE_CLEAR_PER_MOVE as f64 * mult) as u32
    }

    /// Damage actually inflicted after TOUGH/boost reduction, given `raw` incoming this tick
    /// (engine `_applyDamage`, `creeps/tick.js:7-29`). Pure — does **not** mutate `hits`; the
    /// caller nets damage-then-heal in the resolve pass. Iterates parts front-to-back, each
    /// absorbing up to `part_hits / damage_ratio` "effective" hits; only TOUGH boosts have a
    /// `damage_ratio < 1`, and the accumulated reduction is rounded **once** at the end.
    pub fn damage_after_tough(&self, raw: u32) -> u32 {
        if raw == 0 {
            return 0;
        }
        // The reduction loop only runs if any part is boosted (engine `_.any(body, i => !!i.boost)`).
        if !self.parts.iter().any(|p| p.boost != BoostTier::None) {
            return raw;
        }
        let mut damage_reduce = 0.0;
        let mut damage_effective = raw as f64;
        for (i, p) in self.parts.iter().enumerate() {
            if damage_effective <= 0.0 {
                break;
            }
            let part_hits = self.part_hits(i) as f64;
            let ratio = if p.part == Part::Tough {
                p.boost.tough_damage_ratio()
            } else {
                1.0
            };
            let effective = part_hits / ratio;
            let absorbed = effective.min(damage_effective);
            damage_reduce += absorbed * (1.0 - ratio);
            damage_effective -= absorbed;
        }
        (raw as f64 - damage_reduce.round()).max(0.0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(parts: &[(Part, BoostTier)]) -> SimBody {
        SimBody::new(
            parts
                .iter()
                .map(|&(p, b)| BodyPartDef::boosted(p, b))
                .collect(),
        )
    }

    #[test]
    fn action_power_unboosted_and_boosted() {
        let five_attack = body(&[(Part::Attack, BoostTier::None); 5]);
        assert_eq!(five_attack.attack_power(), 150); // 5 × 30

        let five_attack_t3 = body(&[(Part::Attack, BoostTier::T3); 5]);
        assert_eq!(five_attack_t3.attack_power(), 600); // 5 × 30 × 4

        let heal = body(&[(Part::Heal, BoostTier::None); 4]);
        assert_eq!(heal.heal_power(), 48); // 4 × 12
        assert_eq!(heal.ranged_heal_power(), 16); // 4 × 4

        let heal_t3 = body(&[(Part::Heal, BoostTier::T3); 4]);
        assert_eq!(heal_t3.heal_power(), 192); // 4 × 12 × 4
        assert_eq!(heal_t3.ranged_heal_power(), 64); // 4 × 4 × 4

        let ranged = body(&[(Part::RangedAttack, BoostTier::None); 7]);
        assert_eq!(ranged.ranged_attack_power(), 70); // 7 × 10

        let work = body(&[(Part::Work, BoostTier::T3); 10]);
        assert_eq!(work.dismantle_power(), 2000); // 10 × 50 × 4
    }

    #[test]
    fn part_hits_fill_back_to_front() {
        // [Tough, Attack, Move] — full = 300, all parts 100.
        let mut b = SimBody::unboosted(&[Part::Tough, Part::Attack, Part::Move]);
        assert_eq!(
            (b.part_hits(0), b.part_hits(1), b.part_hits(2)),
            (100, 100, 100)
        );
        // At 150 hits the trailing parts fill first: Move=100, Attack=50, Tough=0.
        b.hits = 150;
        assert_eq!(
            (b.part_hits(0), b.part_hits(1), b.part_hits(2)),
            (0, 50, 100)
        );
    }

    #[test]
    fn power_degrades_as_front_parts_die() {
        let mut b = SimBody::unboosted(&[Part::Tough, Part::Attack, Part::Move]);
        assert_eq!(b.attack_power(), 30); // attack part alive
        b.hits = 150; // attack part (index 1) still has 50 hits
        assert_eq!(b.attack_power(), 30);
        b.hits = 100; // now only the Move part (index 2) is alive; attack is dead
        assert_eq!(b.attack_power(), 0);
        assert_eq!(b.fatigue_clear(), 2); // the surviving MOVE still clears fatigue
    }

    #[test]
    fn fatigue_clear_counts_move_parts() {
        assert_eq!(body(&[(Part::Move, BoostTier::None); 3]).fatigue_clear(), 6); // 2 × 3
        assert_eq!(body(&[(Part::Move, BoostTier::T1); 3]).fatigue_clear(), 12);
        // 2 × 3 × 2
    }

    #[test]
    fn unboosted_takes_full_damage() {
        let b = SimBody::unboosted(&[Part::Tough, Part::Attack]);
        assert_eq!(b.damage_after_tough(100), 100); // no boost → no reduction
    }

    #[test]
    fn tough_reduces_within_capacity() {
        // 10 full XGHO2 TOUGH (T3, ×0.3) + 1 MOVE: 100 raw is well within the ~3333 effective
        // capacity, so it's reduced straight to ×0.3 = 30.
        let mut parts: Vec<(Part, BoostTier)> = vec![(Part::Tough, BoostTier::T3); 10];
        parts.push((Part::Move, BoostTier::None));
        assert_eq!(body(&parts).damage_after_tough(100), 30);
    }

    #[test]
    fn tough_capacity_exceeded_spills_unreduced() {
        // 1 full XGHO2 TOUGH (eff capacity 100/0.3 ≈ 333) + 1 MOVE, raw 500: the first 333
        // effective is reduced (Σ reduce = 333.33 × 0.7 = 233.33 → round 233), the rest hits
        // unreduced. Result = 500 − 233 = 267.
        let b = body(&[(Part::Tough, BoostTier::T3), (Part::Move, BoostTier::None)]);
        assert_eq!(b.damage_after_tough(500), 267);
    }

    #[test]
    fn dead_tough_gives_no_mitigation() {
        // [Tough(T3), Attack] at 100 hits → Tough (front) is dead, Attack (back) full. A dead
        // TOUGH part absorbs nothing, so damage passes unreduced.
        let mut b = body(&[
            (Part::Tough, BoostTier::T3),
            (Part::Attack, BoostTier::None),
        ]);
        b.hits = 100;
        assert_eq!(b.part_hits(0), 0);
        assert_eq!(b.damage_after_tough(50), 50);
    }
}
