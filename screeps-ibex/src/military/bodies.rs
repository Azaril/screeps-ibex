use crate::creep::SpawnBodyDefinition;
use crate::military::damage;
use screeps::Part;

// The force-sizing body primitives (`CombatBodySpec`, `MoveProfile`, `build_combat_body`) + the
// heal-part helper now live in the shared decision crate (the force-sizing core); re-exported so the
// bot's `military::bodies` namespace still exposes them next to the static template bodies below.
pub use screeps_combat_decision::bodies::{build_combat_body, defender_heal_parts_for_dps, CombatBodySpec, MoveProfile};

/// Solo defender body (unboosted, emergency response): RANGED_ATTACK + MOVE,
/// scaling with available energy.
///
/// Floors at one RANGED_ATTACK + MOVE (200e) so it spawns even in a bare RCL2
/// room. The previous body forced a HEAL into the minimum repeat unit
/// (`[RangedAttack, Move, Heal, Move]` = 500e, `minimum_repeat: 1`), which a
/// young or contested RCL2 room — capacity below 500 until all 5 extensions are
/// built — could never afford, so `create_body` returned `Err` and NO solo
/// defender ever spawned (e.g. against an unarmed controller-attacker in a
/// towerless room). Solo is the lowest escalation for weak single threats;
/// HEAL support arrives with the Duo/Quad escalations.
pub fn solo_defender_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[],
        repeat_body: &[Part::RangedAttack, Part::Move],
        post_body: &[],
    }
}

// ── Threat-matched sized defender bodies ────────────────────────────────────
//
// Unlike the repeat-template bodies above (which `create_body` expands to fit
// `energy_capacity`), these build the FINAL `Vec<Part>` directly from the
// threat picture — so the part counts are exact and there is no `&'static`
// slice constraint. The spawn path passes the result straight to
// `SpawnRequest::new(.., &body, ..)`.

/// Maximum body parts on a creep (Screeps engine limit).
const MAX_CREEP_SIZE: usize = 50;

/// Assemble the final body for a defender/healer from desired offense + HEAL
/// counts within an energy `budget`. Adds MOVE (~1 per 2 other parts) and a
/// small TOUGH front when HEAL is present and it fits. Degrades to fit the
/// budget and the 50-part cap in priority order — drop TOUGH, then HEAL, then
/// trim offense — but never below the role floor (at least 1 offense for an
/// attacker, at least 1 HEAL for a pure healer), so it always returns a usable
/// body once the room can afford it. Parts are ordered TOUGH, offense, HEAL,
/// MOVE so TOUGH soaks damage first.
fn assemble_combat_body(budget: u32, offense_parts: u32, offense_kind: Part, heal_parts: u32) -> Vec<Part> {
    let off_floor: u32 = if offense_parts > 0 { 1 } else { 0 };
    let heal_floor: u32 = if offense_parts == 0 { 1 } else { 0 };

    let mut off = offense_parts.max(off_floor).min(MAX_CREEP_SIZE as u32);
    let mut heal = heal_parts.max(heal_floor).min(MAX_CREEP_SIZE as u32);
    let mut tough: u32 = if heal > 0 { 2 } else { 0 };

    let cfg = |off: u32, heal: u32, tough: u32| -> (u32, u32) {
        let work = off + heal + tough;
        let moves = work.div_ceil(2).max(1); // ~1 MOVE per 2 other parts, at least 1
        let parts = work + moves;
        let cost = off * offense_kind.cost() + heal * Part::Heal.cost() + tough * Part::Tough.cost() + moves * Part::Move.cost();
        (cost, parts)
    };

    loop {
        let (cost, parts) = cfg(off, heal, tough);
        if cost <= budget && parts as usize <= MAX_CREEP_SIZE {
            break;
        }
        if tough > 0 {
            tough -= 1;
        } else if heal > heal_floor {
            heal -= 1;
        } else if off > off_floor {
            off -= 1;
        } else {
            // At the role floor and still over budget: emit the floor body. The
            // spawn queue won't fire it until the room can afford it (body_cost
            // > available ⇒ the request waits), so this never panics or returns
            // an empty body.
            break;
        }
    }

    let moves = (off + heal + tough).div_ceil(2).max(1);
    let mut body = Vec::with_capacity((off + heal + tough + moves) as usize);
    body.extend(std::iter::repeat_n(Part::Tough, tough as usize));
    body.extend(std::iter::repeat_n(offense_kind, off as usize));
    body.extend(std::iter::repeat_n(Part::Heal, heal as usize));
    body.extend(std::iter::repeat_n(Part::Move, moves as usize));
    body
}

// (Force-driven body builder + `CombatBodySpec`/`MoveProfile`/`build_combat_body` moved to
// `screeps_combat_decision::bodies`; re-exported at the top of this module.)

/// Threat-matched defender body sized to an energy `budget`. Offense
/// (RANGED_ATTACK) is sized to kill the worst target within
/// [`damage::KILL_WINDOW_TICKS`] net of the enemy's focused heal; HEAL is sized
/// to survive `incoming_dps` (0 against a zero-DPS threat such as a CLAIM creep)
/// and included only when it fits. Always returns at least `[RangedAttack, Move]`
/// (200e) so a bare RCL2 towerless room still gets an armed defender. `boosted` =
/// whether OUR creep is boosted (the enemy's boosts are already folded into the
/// threat figures by `threatmap`).
pub fn sized_defender_body(budget: u32, incoming_dps: f32, target_hp: f32, enemy_focus_heal: f32, boosted: bool) -> Vec<Part> {
    let ra_dmg = if boosted { 10.0 * 4.0 } else { 10.0 };
    let want_off = damage::attack_parts_to_kill(target_hp, enemy_focus_heal, damage::KILL_WINDOW_TICKS, ra_dmg)
        .unwrap_or(damage::MAX_OFFENSE_PARTS)
        .max(1);
    let want_heal = defender_heal_parts_for_dps(incoming_dps, boosted);
    assemble_combat_body(budget, want_off, Part::RangedAttack, want_heal)
}

/// Threat-matched defender HEALER body (HEAL + MOVE, TOUGH front when it fits)
/// sized to sustain `incoming_dps`. Spawns even at RCL2 by dropping the TOUGH
/// front — fixing the old `duo_healer_body` 660e floor that produced no healer
/// below RCL3.
pub fn sized_healer_body(budget: u32, incoming_dps: f32, boosted: bool) -> Vec<Part> {
    let want_heal = defender_heal_parts_for_dps(incoming_dps, boosted).max(1);
    assemble_combat_body(budget, 0, Part::RangedAttack, want_heal)
}

/// Duo attacker body (ranged variant).
/// TOUGH front for damage absorption, RANGED_ATTACK + MOVE repeat.
/// Enough MOVE so creep keeps full speed in formation.
pub fn duo_ranged_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::RangedAttack, Part::Move],
        post_body: &[Part::Move, Part::Move, Part::Move, Part::Move],
    }
}

/// Duo attacker body (melee variant).
/// TOUGH front, ATTACK + MOVE repeat.
/// Enough MOVE so creep keeps full speed in formation.
pub fn duo_melee_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::Attack, Part::Move],
        post_body: &[Part::Move, Part::Move, Part::Move, Part::Move],
    }
}

/// Duo healer body.
/// TOUGH front for damage absorption, HEAL + MOVE repeat.
/// Enough MOVE so creep keeps full speed (MOVE >= non-MOVE parts for formation).
pub fn duo_healer_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::Heal, Part::Move],
        post_body: &[Part::Move, Part::Move, Part::Move, Part::Move, Part::Move, Part::Move],
    }
}

/// Minimum energy for full quad member (pre + 1 repeat + post).
const QUAD_MEMBER_FULL_MIN: u32 = 40 + 500 + 1200; // 1740

/// Quad member body (boosted, RCL 8).
/// TOUGH front, mixed RANGED_ATTACK + HEAL + MOVE.
/// For low RCL (e.g. RCL 5), returns a minimal ranged body so the room still gets spawn queue entries.
pub fn quad_member_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    if max_energy < QUAD_MEMBER_FULL_MIN {
        // Light variant: RANGED_ATTACK + MOVE only, fits RCL 5 (550+).
        return SpawnBodyDefinition {
            maximum_energy: max_energy,
            minimum_repeat: Some(1),
            maximum_repeat: None,
            pre_body: &[],
            repeat_body: &[Part::RangedAttack, Part::Move],
            post_body: &[],
        };
    }
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::RangedAttack, Part::Move, Part::Heal, Part::Move],
        post_body: &[
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
        ],
    }
}

/// Tank body for front-line damage absorption.
/// Heavy TOUGH front, ATTACK for counter-damage.
/// Enough MOVE (1:1 with non-MOVE) so creep keeps full speed in formation.
pub fn tank_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
        ],
        repeat_body: &[Part::Attack, Part::Move],
        post_body: &[
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
        ],
    }
}

/// Drain creep body -- heavy TOUGH + HEAL for soaking tower energy.
/// Baseline HEAL in post_body so the creep can sustain tower damage (e.g. one tower at
/// range 20+ = 150 damage/tick; 13 HEAL = 156 heal/tick adjacent). MOVE in post matches
/// fixed TOUGH + fixed HEAL; repeat adds Heal + one MOVE per Heal.
pub fn drain_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(2),
        maximum_repeat: None,
        pre_body: &[
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
        ],
        repeat_body: &[Part::Heal, Part::Move],
        post_body: &[
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
        ],
    }
}

/// Drain body with more HEAL for rooms with higher tower DPS (e.g. 2 towers at edge).
/// 18 HEAL in post + 2 repeat = 20 HEAL (240 heal/tick).
fn drain_body_heavy(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(2),
        maximum_repeat: None,
        pre_body: &[
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
        ],
        repeat_body: &[Part::Heal, Part::Move],
        post_body: &[
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
        ],
    }
}

/// Drain body sized for the given tower DPS (e.g. from target room).
/// Uses standard body (13 HEAL) when required heal parts ≤ 13, heavy body (20 HEAL) otherwise.
pub fn drain_body_for_tower_dps(max_energy: u32, tower_damage_per_tick: f32) -> SpawnBodyDefinition<'static> {
    let min_heal = damage::drain_heal_parts_for_dps(tower_damage_per_tick);
    if min_heal > 13 {
        drain_body_heavy(max_energy)
    } else {
        drain_body(max_energy)
    }
}

/// Cheap harasser body for disrupting remote mining.
/// Fast, expendable: all RANGED_ATTACK + MOVE.
pub fn harasser_body() -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: 1300,
        minimum_repeat: Some(3),
        maximum_repeat: Some(5),
        pre_body: &[],
        repeat_body: &[Part::RangedAttack, Part::Move],
        post_body: &[],
    }
}

/// Dismantler body for structure destruction.
/// TOUGH front, WORK + MOVE repeat. MOVE to match TOUGH in post_body.
pub fn dismantler_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::Work, Part::Move],
        post_body: &[Part::Move, Part::Move, Part::Move, Part::Move],
    }
}

// ─── Boosted body definitions ──────────────────────────────────────────────────
//
// Boosted bodies use fewer MOVE parts because T3 XZHO2 reduces fatigue by 100%
// per boosted MOVE. This allows more combat parts within the 50-part limit.

/// Boosted quad member body (T3 boosts, RCL 8).
/// With XZHO2 on MOVE, each MOVE handles 4 fatigue instead of 2.
/// TOUGH boosted with XGHO2 (70% damage reduction).
/// HEAL boosted with XLHO2 (300% effectiveness).
/// RANGED_ATTACK boosted with XKHO2 (300% damage).
pub fn boosted_quad_member_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::RangedAttack, Part::RangedAttack, Part::Heal, Part::Move],
        post_body: &[
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Heal,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
        ],
    }
}

/// Boosted duo healer body (T3 boosts).
/// Heavy HEAL with minimal MOVE (boosted XZHO2).
pub fn boosted_duo_healer_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(2),
        maximum_repeat: None,
        pre_body: &[
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
        ],
        repeat_body: &[Part::Heal, Part::Heal, Part::Heal, Part::Move],
        post_body: &[Part::Move, Part::Move],
    }
}

/// Boosted duo attacker body (T3 boosts, ranged variant).
/// Heavy RANGED_ATTACK with TOUGH front.
pub fn boosted_duo_ranged_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[Part::Tough, Part::Tough, Part::Tough, Part::Tough, Part::Tough, Part::Tough],
        repeat_body: &[Part::RangedAttack, Part::RangedAttack, Part::RangedAttack, Part::Move],
        post_body: &[Part::Move, Part::Move, Part::Move],
    }
}

/// Boosted tank body (T3 boosts).
/// Heavy TOUGH (XGHO2) + ATTACK (XUH2O) with minimal MOVE (XZHO2).
pub fn boosted_tank_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
        ],
        repeat_body: &[Part::Attack, Part::Attack, Part::Attack, Part::Move],
        post_body: &[Part::Move, Part::Move, Part::Move, Part::Move],
    }
}

// ─── Source Keeper body definitions ──────────────────────────────────────────

/// SK ranged attacker body -- heavy RANGED_ATTACK + MOVE for kiting at range 3.
/// No TOUGH needed since the strategy is to never be in melee range.
/// At 12,900 energy: 4 pre + 20 repeat (10 RA + 10 M) + 2 post = 26 parts.
/// DPS: 10 RANGED_ATTACK * 10 = 100 DPS at range 3.
pub fn sk_ranged_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(3),
        maximum_repeat: Some(15),
        pre_body: &[Part::Tough, Part::Tough],
        repeat_body: &[Part::RangedAttack, Part::Move],
        post_body: &[Part::Move, Part::Heal, Part::Move],
    }
}

/// SK healer body -- focused HEAL + MOVE for keeping the SK attacker alive.
/// Needs to outheal incidental damage (SK does 168 melee DPS if it catches up).
/// At 12,900 energy: 2 pre + 20 repeat (10 H + 10 M) + 2 post = 24 parts.
/// Heal: 10 HEAL * 12 = 120 HP/tick adjacent.
pub fn sk_healer_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(3),
        maximum_repeat: Some(15),
        pre_body: &[Part::Tough, Part::Tough],
        repeat_body: &[Part::Heal, Part::Move],
        post_body: &[Part::Move, Part::Move],
    }
}

// ─── Specialized body definitions (Phase 6) ────────────────────────────────

/// Power bank attacker body -- heavy ATTACK + MOVE, capped at 20×ATTACK
/// (P1.D5 / ADR 0013 D1.2): the bank reflects 50% of damage dealt, and
/// the duo healer maxes at 25×HEAL = 300 heal/tick — 20×ATTACK deals
/// 600 (300 reflected, exactly healable; kill in ~3334 ticks), while
/// the old 25×ATTACK cap reflected 375/tick and out-damaged its own
/// healer.
pub fn power_bank_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(5),
        maximum_repeat: Some(20),
        pre_body: &[],
        repeat_body: &[Part::Attack, Part::Move],
        post_body: &[],
    }
}

/// Power bank healer body -- heavy HEAL + MOVE.
/// Needs ~32 HEAL parts (384 heal/tick) to outheal 375 damage/tick reflection.
pub fn power_bank_healer_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(5),
        maximum_repeat: Some(25),
        pre_body: &[],
        repeat_body: &[Part::Heal, Part::Move],
        post_body: &[],
    }
}

/// Siege dismantler body -- TOUGH front, WORK + MOVE for dismantling under fire.
/// MOVE to match TOUGH in post_body; repeat is only Work + one MOVE per Work.
pub fn siege_dismantler_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(2),
        maximum_repeat: None,
        pre_body: &[
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
            Part::Tough,
        ],
        repeat_body: &[Part::Work, Part::Move],
        post_body: &[
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
            Part::Move,
        ],
    }
}

/// Core attacker body -- cheap ATTACK + MOVE for destroying level 0 invader cores.
/// Minimal investment since cores have low HP and no defenders when deploying.
pub fn core_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy.min(1300),
        minimum_repeat: Some(2),
        maximum_repeat: Some(5),
        pre_body: &[],
        repeat_body: &[Part::Attack, Part::Move],
        post_body: &[],
    }
}

/// Hauler body -- CARRY + MOVE for collecting dropped resources.
/// Sized to carry resources efficiently. For power banks,
/// ceil(power_amount / 50) CARRY parts needed.
pub fn hauler_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(2),
        maximum_repeat: Some(25),
        pre_body: &[],
        repeat_body: &[Part::Carry, Part::Move],
        post_body: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creep::spawning::create_body;

    // (R1 build_combat_body / MoveProfile tests moved with the code to
    // `screeps_combat_decision::bodies`.)

    /// Regression (W11N57, live): a solo defender MUST build at RCL1/RCL2
    /// energy levels. The old body forced HEAL into a 500e minimum repeat unit,
    /// so a room with capacity < 500 (a bare/contested RCL2 room) produced
    /// `Err(())` and no defender ever spawned against an unarmed
    /// controller-attacker — the room got declaimed undefended.
    #[test]
    fn solo_defender_builds_at_low_rcl() {
        // 300 = bare RCL2 spawn; 350/550 = partial/full RCL2 extensions.
        for capacity in [300u32, 350, 550, 800] {
            let body = create_body(&solo_defender_body(capacity)).unwrap_or_else(|_| panic!("solo defender must build at {capacity}e"));
            assert!(body.iter().any(|&p| p == Part::RangedAttack), "needs RANGED_ATTACK at {capacity}e");
            assert!(body.iter().any(|&p| p == Part::Move), "needs MOVE at {capacity}e");
            let cost: u32 = body.iter().map(|p| p.cost()).sum();
            assert!(cost <= capacity, "body cost {cost} exceeds capacity {capacity}");
        }
    }

    /// More energy yields a larger (more ranged) solo defender.
    #[test]
    fn solo_defender_scales_with_energy() {
        let ranged_at = |cap| {
            create_body(&solo_defender_body(cap))
                .unwrap()
                .iter()
                .filter(|&&p| p == Part::RangedAttack)
                .count()
        };
        assert!(ranged_at(800) > ranged_at(300), "solo defender should scale up with energy");
    }

    // ── Threat-matched sized bodies ─────────────────────────────────────────

    fn cost(body: &[Part]) -> u32 {
        body.iter().map(|p| p.cost()).sum()
    }

    /// Bare RCL2 vs an unarmed CLAIM creep (zero DPS): armed defender, NO HEAL
    /// forced, fits the budget. Preserves the live W11N57 fix.
    #[test]
    fn sized_defender_rcl2_vs_claim_is_armed_with_no_heal() {
        let body = sized_defender_body(300, 0.0, 700.0, 0.0, false);
        assert!(!body.is_empty());
        assert!(body.iter().any(|&p| p == Part::RangedAttack), "must be armed");
        assert!(body.iter().any(|&p| p == Part::Move), "must move");
        assert!(!body.iter().any(|&p| p == Part::Heal), "no HEAL vs a zero-DPS threat");
        assert!(cost(&body) <= 300, "cost {} > 300", cost(&body));
    }

    /// HEAL is dropped (not forced) when it doesn't fit a tight budget, but the
    /// defender is still armed.
    #[test]
    fn sized_defender_drops_heal_when_unaffordable() {
        let body = sized_defender_body(400, 90.0, 600.0, 0.0, false);
        assert!(body.iter().any(|&p| p == Part::RangedAttack));
        assert!(cost(&body) <= 400, "cost {} > 400", cost(&body));
    }

    /// A capable budget vs a real attacker ⇒ defender carries HEAL.
    #[test]
    fn sized_defender_carries_heal_when_affordable() {
        let body = sized_defender_body(2000, 120.0, 1000.0, 0.0, false);
        assert!(body.iter().any(|&p| p == Part::Heal), "should carry HEAL when affordable");
        assert!(body.iter().any(|&p| p == Part::RangedAttack));
        assert!(cost(&body) <= 2000);
    }

    /// Regression for the duo_healer 660e floor: a healer MUST build at RCL2
    /// (drops the TOUGH front).
    #[test]
    fn sized_healer_builds_at_rcl2() {
        let body = sized_healer_body(550, 90.0, false);
        assert!(!body.is_empty());
        assert!(body.iter().any(|&p| p == Part::Heal));
        assert!(body.iter().any(|&p| p == Part::Move));
        assert!(cost(&body) <= 550, "cost {} > 550", cost(&body));
    }

    /// Never exceed the 50-part engine cap, however large the budget/threat.
    #[test]
    fn sized_bodies_respect_part_cap() {
        let d = sized_defender_body(50_000, 5000.0, 1_000_000.0, 5000.0, false);
        assert!(d.len() <= 50, "defender len {}", d.len());
        let h = sized_healer_body(50_000, 5000.0, false);
        assert!(h.len() <= 50, "healer len {}", h.len());
    }
}

/// Standard military boost compounds (T3).
pub mod boosts {
    use screeps::ResourceType;

    /// TOUGH damage reduction (70%) -- XGHO2.
    pub const TOUGH_BOOST: ResourceType = ResourceType::CatalyzedGhodiumAlkalide;
    /// HEAL effectiveness (300%) -- XLHO2.
    pub const HEAL_BOOST: ResourceType = ResourceType::CatalyzedLemergiumAlkalide;
    /// MOVE fatigue reduction (100%) -- XZHO2.
    pub const MOVE_BOOST: ResourceType = ResourceType::CatalyzedZynthiumAlkalide;
    /// RANGED_ATTACK damage (300%) -- XKHO2.
    pub const RANGED_ATTACK_BOOST: ResourceType = ResourceType::CatalyzedKeaniumAlkalide;
    /// ATTACK damage (300%) -- XUH2O.
    pub const ATTACK_BOOST: ResourceType = ResourceType::CatalyzedUtriumAcid;
}
