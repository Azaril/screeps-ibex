//! Economy-mechanics constants transcribed from the Screeps engine. The ground truth is
//! `docs/references/engine-mechanics.md` (which itself pins engine source file:line); **every
//! constant here carries an `engine-mechanics.md:<line>` citation and is pinned by the
//! `citation_pins` test below**, so a drive-by edit cannot silently diverge from the reference.
//! (Movement/fatigue/body constants live in `screeps_sim_core::constants` — not duplicated here.)

use screeps::Part;

/// Energy harvested per WORK part per harvest intent — `HARVEST_POWER` 2
/// (engine-mechanics.md:457, `common/constants.js:117-119`).
pub const HARVEST_POWER: u32 = 2;

/// Source pool capacity, owned OR reserved room: 3000 (engine-mechanics.md:466, §7.3 :445).
pub const SOURCE_CAPACITY_OWNED: u32 = 3000;
/// Source pool capacity, neutral unreserved room: 1500 (engine-mechanics.md:466, §7.3 :445).
pub const SOURCE_CAPACITY_NEUTRAL: u32 = 1500;
/// Source pool capacity, controller-less (SK/center) room: 4000 (engine-mechanics.md:466, §7.3 :445).
pub const SOURCE_CAPACITY_KEEPER: u32 = 4000;

/// Source regen delay — `ENERGY_REGEN_TIME` 300 (engine-mechanics.md:466). The timer **starts at
/// the first harvest below capacity** and the source refills when `gameTime >=
/// nextRegenerationTime − 1` (engine-mechanics.md:445, `sources/tick.js:10-29`).
pub const ENERGY_REGEN_TIME: u32 = 300;

/// Spawn store capacity AND the room-energy threshold below which every spawn self-charges
/// +1 energy/tick — `SPAWN_ENERGY_CAPACITY` 300 (engine-mechanics.md:279: "+1 energy/tick while
/// room spawn+extension energy < 300", `spawns/tick.js:43-47`).
pub const SPAWN_ENERGY_CAPACITY: u32 = 300;

/// Spawn duration: 3 ticks per body part — `CREEP_SPAWN_TIME` (engine-mechanics.md:454; the
/// `needTime = 3 × body.length` formula at engine-mechanics.md:242).
pub const CREEP_SPAWN_TIME: u32 = 3;

/// Maximum body size — `MAX_CREEP_SIZE` 50 (engine-mechanics.md:453; oversize bodies are silently
/// truncated by the engine at engine-mechanics.md:242 — this sim REJECTS them instead, loudly,
/// because a policy layer emitting an oversize body is a bug, not a request).
pub const MAX_CREEP_SIZE: usize = 50;

/// Non-CLAIM creep lifetime — `CREEP_LIFE_TIME` 1500 (engine-mechanics.md:453).
pub const CREEP_LIFE_TIME: u32 = 1500;

/// Dropped-resource decay divisor: piles lose `ceil(amount / 1000)` per tick
/// (engine-mechanics.md:431, `energy/tick.js:12`).
pub const DROPPED_DECAY_DIVISOR: u32 = 1000;

/// Extension energy capacity by controller level: 50 (RCL ≤ 6), 100 (RCL 7), 200 (RCL 8) —
/// `EXTENSION_ENERGY_CAPACITY` (engine-mechanics.md:456). NOTE: the engine RECOMPUTES this from
/// the room's **current** controller level every tick (`extensions/tick.js:11`), not once at
/// construction — the tick pipeline's step 0 mirrors that whenever the world has a controller.
pub fn extension_capacity(rcl: u8) -> u32 {
    match rcl {
        8.. => 200,
        7 => 100,
        _ => 50,
    }
}

/// Energy cost of one body part — `BODYPART_COST` (engine-mechanics.md:452): move/carry 50,
/// work 100, attack 80, ranged 150, heal 250, tough 10, claim 600.
pub fn part_cost(part: Part) -> u32 {
    match part {
        Part::Move | Part::Carry => 50,
        Part::Work => 100,
        Part::Attack => 80,
        Part::RangedAttack => 150,
        Part::Heal => 250,
        Part::Tough => 10,
        Part::Claim => 600,
        // `Part` is #[non_exhaustive] in screeps-game-api; no other variants exist in the engine.
        _ => unreachable!("unknown body part"),
    }
}

/// Total energy cost of a body (Σ [`part_cost`]).
pub fn body_cost(body: &[Part]) -> u32 {
    body.iter().map(|&p| part_cost(p)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The citation pins: every constant above asserted against the value at its cited
    /// engine-mechanics.md line, so the constant and its citation cannot drift apart silently.
    #[test]
    fn citation_pins() {
        // engine-mechanics.md:457 — HARVEST_POWER 2 per WORK.
        assert_eq!(HARVEST_POWER, 2);
        // engine-mechanics.md:466 (§7.3 :445-446) — source capacity 3000 owned/reserved, 1500
        // neutral, 4000 keeper/controller-less; regen 300.
        assert_eq!(SOURCE_CAPACITY_OWNED, 3000);
        assert_eq!(SOURCE_CAPACITY_NEUTRAL, 1500);
        assert_eq!(SOURCE_CAPACITY_KEEPER, 4000);
        assert_eq!(ENERGY_REGEN_TIME, 300);
        // engine-mechanics.md:279 — spawn self-charge threshold = spawn capacity = 300.
        assert_eq!(SPAWN_ENERGY_CAPACITY, 300);
        // engine-mechanics.md:454 / :242 — 3 ticks per part.
        assert_eq!(CREEP_SPAWN_TIME, 3);
        // engine-mechanics.md:453 — MAX_CREEP_SIZE 50 / CREEP_LIFE_TIME 1500.
        assert_eq!(MAX_CREEP_SIZE, 50);
        assert_eq!(CREEP_LIFE_TIME, 1500);
        // engine-mechanics.md:431 — dropped decay ceil(amount/1000)/tick.
        assert_eq!(DROPPED_DECAY_DIVISOR, 1000);
    }

    /// engine-mechanics.md:456 — extension capacity 50 (RCL ≤ 6) / 100 (RCL 7) / 200 (RCL 8).
    #[test]
    fn extension_capacity_citation_pin() {
        for rcl in 0..=6 {
            assert_eq!(extension_capacity(rcl), 50, "RCL {rcl}");
        }
        assert_eq!(extension_capacity(7), 100);
        assert_eq!(extension_capacity(8), 200);
    }

    /// engine-mechanics.md:452 — the BODYPART_COST table, cross-checked against screeps-game-api's
    /// own `Part::cost()` (the binding transcribes the same engine table; if they ever diverge,
    /// this test flags whichever transcription drifted).
    #[test]
    fn part_cost_citation_pin() {
        let table = [
            (Part::Move, 50),
            (Part::Carry, 50),
            (Part::Work, 100),
            (Part::Attack, 80),
            (Part::RangedAttack, 150),
            (Part::Heal, 250),
            (Part::Tough, 10),
            (Part::Claim, 600),
        ];
        for (part, cost) in table {
            assert_eq!(part_cost(part), cost, "{part:?}");
            assert_eq!(part.cost(), cost, "game-api transcription for {part:?}");
        }
        assert_eq!(body_cost(&[Part::Work, Part::Carry, Part::Move]), 200);
    }
}
