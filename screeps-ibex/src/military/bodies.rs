use crate::creep::SpawnBodyDefinition;
use crate::military::damage;
use screeps::Part;

/// Solo defender body (unboosted, emergency response).
/// Balanced ranged attack + heal + move.
pub fn solo_defender_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(1),
        maximum_repeat: None,
        pre_body: &[],
        repeat_body: &[Part::RangedAttack, Part::Move, Part::Heal, Part::Move],
        post_body: &[],
    }
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

/// Power bank attacker body -- heavy ATTACK + MOVE.
/// Needs ~25 ATTACK parts (750 DPS) to destroy a 2M HP bank in ~2667 ticks.
/// Must be paired with a healer to survive 50% damage reflection (375 damage/tick).
pub fn power_bank_attacker_body(max_energy: u32) -> SpawnBodyDefinition<'static> {
    SpawnBodyDefinition {
        maximum_energy: max_energy,
        minimum_repeat: Some(5),
        maximum_repeat: Some(25),
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
