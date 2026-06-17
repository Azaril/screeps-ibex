//! Pure combat range/falloff formulas (engine `processor/intents`). JS-free, deterministic.
//!
//! Per-part *power* lives on [`crate::body::SimBody`] (`attack_power`, `ranged_attack_power`, …);
//! this module is the *range-dependent* math: rangedMassAttack distance falloff and tower output
//! falloff. The tower formula is kept identical to the bot kernel `military/damage.rs` so the
//! sim and the live sizing heuristic never disagree on tower numbers.

use crate::constants::*;

/// `rangedMassAttack` damage rate at a Chebyshev distance (`rangedMassAttack.js`): full at 0..=1,
/// 0.4 at 2, 0.1 at 3, and 0 beyond range 3.
pub fn ranged_mass_attack_rate(range: u32) -> f64 {
    RANGED_MASS_ATTACK_FALLOFF
        .get(range as usize)
        .copied()
        .unwrap_or(0.0)
}

/// rangedMassAttack damage dealt to ONE target at `range` by a body of `ranged_attack_power`
/// (engine rounds the per-target amount).
pub fn ranged_mass_attack_damage(ranged_attack_power: u32, range: u32) -> u32 {
    (ranged_attack_power as f64 * ranged_mass_attack_rate(range)).round() as u32
}

/// Tower output at `range` for a given base power (attack 600 / heal 400 / repair 800), floored
/// per the engine (`towers/attack.js:32-46`): full damage at range ≤ 5, linear falloff to 25% at
/// range ≥ 20.
pub fn tower_amount_at_range(base: f64, range: u32) -> u32 {
    let r = range.clamp(TOWER_OPTIMAL_RANGE, TOWER_FALLOFF_RANGE);
    let span = (TOWER_FALLOFF_RANGE - TOWER_OPTIMAL_RANGE) as f64;
    let factor = 1.0 - TOWER_FALLOFF * (r - TOWER_OPTIMAL_RANGE) as f64 / span;
    (base * factor).floor() as u32
}

pub fn tower_attack_damage_at_range(range: u32) -> u32 {
    tower_amount_at_range(TOWER_POWER_ATTACK, range)
}
pub fn tower_heal_at_range(range: u32) -> u32 {
    tower_amount_at_range(TOWER_POWER_HEAL, range)
}
pub fn tower_repair_at_range(range: u32) -> u32 {
    tower_amount_at_range(TOWER_POWER_REPAIR, range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rma_falloff_matches_engine() {
        assert_eq!(ranged_mass_attack_rate(0), 1.0);
        assert_eq!(ranged_mass_attack_rate(1), 1.0);
        assert_eq!(ranged_mass_attack_rate(2), 0.4);
        assert_eq!(ranged_mass_attack_rate(3), 0.1);
        assert_eq!(ranged_mass_attack_rate(4), 0.0);
        // 70 ranged power: 70 @r1, 28 @r2, 7 @r3.
        assert_eq!(ranged_mass_attack_damage(70, 1), 70);
        assert_eq!(ranged_mass_attack_damage(70, 2), 28);
        assert_eq!(ranged_mass_attack_damage(70, 3), 7);
    }

    #[test]
    fn tower_damage_falloff_matches_engine() {
        assert_eq!(tower_attack_damage_at_range(0), 600);
        assert_eq!(tower_attack_damage_at_range(5), 600);
        assert_eq!(tower_attack_damage_at_range(10), 450); // 600 × (1 − 0.75·5/15) = 450
        assert_eq!(tower_attack_damage_at_range(20), 150);
        assert_eq!(tower_attack_damage_at_range(25), 150); // clamped at falloff range
        assert_eq!(tower_heal_at_range(20), 100);
        assert_eq!(tower_repair_at_range(20), 200);
    }
}
