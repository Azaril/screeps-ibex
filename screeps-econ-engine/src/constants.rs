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

// ── Repair (M1) ─────────────────────────────────────────────────────────────────────────────────

/// Hits restored per WORK part per repair intent — `REPAIR_POWER` 100 (engine-mechanics.md:118,
/// `common/constants.js:120`; the engine repair pipeline is `creeps/repair.js:23-27`).
pub const REPAIR_POWER: u32 = 100;

/// Repair energy pricing, expressed as its integer inverse: `REPAIR_COST` is 0.01 energy/hit
/// (engine `creeps/repair.js:24,27`), i.e. **100 hits repaired per 1 energy** — kept as
/// hits-per-energy so all repair arithmetic stays exact-integer (the effect clamp is
/// `energy × REPAIR_HITS_PER_ENERGY`, the cost is `effect.div_ceil(REPAIR_HITS_PER_ENERGY)`,
/// mirroring the engine's `Math.ceil(repairEffect * REPAIR_COST)`).
pub const REPAIR_HITS_PER_ENERGY: u32 = 100;

/// Chebyshev repair range — 3 (engine-mechanics.md:118, `creeps/repair.js:19`).
pub const REPAIR_RANGE: u32 = 3;

// ── Road decay + traffic wear (M1) ──────────────────────────────────────────────────────────────

/// Hits a road loses per decay event, before the terrain ratio — `ROAD_DECAY_AMOUNT` 100
/// (engine-mechanics.md:430, `roads/tick.js:11-21`, `constants.js:155-159`).
pub const ROAD_DECAY_AMOUNT: u32 = 100;
/// Ticks between road decay events — `ROAD_DECAY_TIME` 1000 (engine-mechanics.md:430).
pub const ROAD_DECAY_TIME: u32 = 1000;
/// Road hitsMax on plain terrain — `ROAD_HITS` 5000; swamp roads multiply BOTH hitsMax and the
/// decay amount by [`ROAD_SWAMP_RATIO`] (engine-mechanics.md:430: "−(100 × terrain ratio) hits per
/// 1,000 ticks (swamp ×5, tunnel ×150)"; hitsMax `constants.js:192-211`). Wall tunnels (×150) are
/// deliberately NOT modeled — foreman plans no tunnels (documented M1 scope cut).
pub const ROAD_HITS: u32 = 5000;
/// The swamp terrain ratio for road hitsMax + decay — `CONSTRUCTION_COST_ROAD_SWAMP_RATIO` 5
/// (engine-mechanics.md:430, `constants.js:155-159`).
pub const ROAD_SWAMP_RATIO: u32 = 5;
/// Per-creep-step road wear — `ROAD_WEAROUT` 1: each creep STEP onto a road tile pulls the road's
/// `nextDecayTime` FORWARD by `ROAD_WEAROUT × body.length` ticks (engine-mechanics.md:430,
/// `movement.js:215-219`) — traffic accelerates the decay CLOCK; it never damages hits directly.
/// (`ROAD_WEAROUT_POWER_CREEP` 100 is not modeled — no power creeps in the sim.)
pub const ROAD_WEAROUT: u32 = 1;

/// A road's hitsMax for its terrain (plain 5000 / swamp 25000 — engine-mechanics.md:430).
pub fn road_hits_max(swamp: bool) -> u32 {
    if swamp {
        ROAD_HITS * ROAD_SWAMP_RATIO
    } else {
        ROAD_HITS
    }
}

// ── Container decay (M1) ────────────────────────────────────────────────────────────────────────

/// Hits a container loses per decay event — `CONTAINER_DECAY` 5000 (engine-mechanics.md:429,
/// `containers/tick.js:10-31`, `constants.js:339-343`).
pub const CONTAINER_DECAY: u32 = 5_000;
/// Decay window where the room controller is level 0 (incl. reserved remotes) —
/// `CONTAINER_DECAY_TIME` 100 (engine-mechanics.md:429).
pub const CONTAINER_DECAY_TIME: u32 = 100;
/// Decay window at RCL ≥ 1 — `CONTAINER_DECAY_TIME_OWNED` 500 (engine-mechanics.md:429).
pub const CONTAINER_DECAY_TIME_OWNED: u32 = 500;
/// Container hitsMax — 250K (engine-mechanics.md:429).
pub const CONTAINER_HITS: u32 = 250_000;
/// Container general-store capacity — `CONTAINER_CAPACITY` 2000 (`common/constants.js:341`, the
/// sibling row of the engine-mechanics.md:429 container entry). Moved here from econ-eval's
/// layout module at M2 (build completion needs it engine-side; one citation-pinned definition).
pub const CONTAINER_CAPACITY: u32 = 2_000;
/// Storage general-store capacity — `STORAGE_CAPACITY` 1,000,000 (`common/constants.js`).
pub const STORAGE_CAPACITY: u32 = 1_000_000;

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

// ── Controller / RCL (M2) ───────────────────────────────────────────────────────────────────────

/// Controller progress per WORK part per upgrade intent — `UPGRADE_CONTROLLER_POWER` 1
/// (engine-mechanics.md:458, `common/constants.js:124`). Each point of progress costs exactly
/// 1 energy (`creeps/upgradeController.js:33-34,92`: `buildEffect = min(WORK × 1, energy)` and
/// `store.energy -= buildEffect`).
pub const UPGRADE_CONTROLLER_POWER: u32 = 1;

/// The RCL-8 room-wide upgrade energy cap — `CONTROLLER_MAX_UPGRADE_PER_TICK` 15
/// (engine-mechanics.md:438,458, `common/constants.js:238`), shared across ALL upgraders via the
/// controller's per-tick `_upgraded` accumulator (`creeps/upgradeController.js:42-52,88`).
pub const CONTROLLER_MAX_UPGRADE_PER_TICK: u32 = 15;

/// Downgrade-clock ticks restored per TICK-WITH-ANY-UPGRADE — `CONTROLLER_DOWNGRADE_RESTORE` 100
/// (engine-mechanics.md:228,440, `common/constants.js:233`). The engine applies it ONCE per tick
/// when the `_upgraded` accumulator is truthy (`controllers/tick.js:38-43`), never per action or
/// per energy.
pub const CONTROLLER_DOWNGRADE_RESTORE: u32 = 100;

/// Progress required to advance FROM `level` to `level + 1` — `CONTROLLER_LEVELS`
/// {1:200, 2:45K, 3:135K, 4:405K, 5:1.215M, 6:3.645M, 7:10.935M} (engine-mechanics.md:467,
/// `common/constants.js:213`). `None` for level 8 (max) and level 0 (claim, not upgrade).
pub fn controller_levels(level: u8) -> Option<u32> {
    match level {
        1 => Some(200),
        2 => Some(45_000),
        3 => Some(135_000),
        4 => Some(405_000),
        5 => Some(1_215_000),
        6 => Some(3_645_000),
        7 => Some(10_935_000),
        _ => None,
    }
}

/// The FULL downgrade clock per RCL — `CONTROLLER_DOWNGRADE`
/// {1:20K, 2:10K, 3:20K, 4:40K, 5:80K, 6:120K, 7:150K, 8:200K} (engine-mechanics.md:228,467,
/// `common/constants.js:232`). 0 for level 0 (no clock).
pub fn controller_downgrade(level: u8) -> u32 {
    match level {
        1 => 20_000,
        2 => 10_000,
        3 => 20_000,
        4 => 40_000,
        5 => 80_000,
        6 => 120_000,
        7 => 150_000,
        8 => 200_000,
        _ => 0,
    }
}

// ── Build + construction sites (M2) ─────────────────────────────────────────────────────────────

/// Construction progress per WORK part per build intent — `BUILD_POWER` 5
/// (`common/constants.js:122`; the engine build pipeline is `creeps/build.js:67-69`). Each point
/// of progress costs exactly 1 energy (`build.js:69,83`: `buildEffect = min(5 × WORK, remaining,
/// energy)` and `store.energy -= buildEffect`).
pub const BUILD_POWER: u32 = 5;

/// Chebyshev build range — 3 (`creeps/build.js:23`).
pub const BUILD_RANGE: u32 = 3;

/// Road construction cost multiplier on swamp — `CONSTRUCTION_COST_ROAD_SWAMP_RATIO` 5
/// (engine-mechanics.md:430 "build cost 300 × 1/5/150 plain/swamp/tunnel",
/// `common/constants.js:210`; applied at site creation, `room/create-construction-site.js:37-41`).
/// The wall/tunnel ×150 (`constants.js:211`) is NOT modeled — no tunnels in scope (the M1 road
/// scope cut carries over).
pub const CONSTRUCTION_COST_ROAD_SWAMP_RATIO: u32 = 5;

/// Base construction cost (= site `progressTotal` on plain terrain) per structure kind —
/// `CONSTRUCTION_COST` (`common/constants.js:192-209`): spawn 15000, extension 3000, road 300,
/// storage 30000, tower 5000, container 5000.
pub fn construction_cost(kind: crate::state::StructureKind) -> u32 {
    use crate::state::StructureKind::*;
    match kind {
        Spawn => 15_000,
        Extension => 3_000,
        Road => 300,
        Storage => 30_000,
        Tower => 5_000,
        Container => 5_000,
    }
}

/// Structure-count allowance by controller level — `CONTROLLER_STRUCTURES`
/// (`common/constants.js:214-231`): extension {2:5, 3:10, 4:20, 5:30, 6:40, 7:50, 8:60}, spawn
/// {1:1, 7:2, 8:3}, storage {4+:1}, tower {3:1, 4:1, 5:2, 6:2, 7:3, 8:6}, container 5 at every
/// level, road 2500. Placement counts BUILT structures + PENDING sites of the kind against this
/// (`utils.js:338-354` `checkControllerAvailability`).
pub fn controller_structures(kind: crate::state::StructureKind, rcl: u8) -> u32 {
    use crate::state::StructureKind::*;
    match kind {
        Spawn => match rcl {
            0 => 0,
            1..=6 => 1,
            7 => 2,
            _ => 3,
        },
        Extension => match rcl {
            0 | 1 => 0,
            2 => 5,
            3 => 10,
            4 => 20,
            5 => 30,
            6 => 40,
            7 => 50,
            _ => 60,
        },
        Storage => u32::from(rcl >= 4),
        Tower => match rcl {
            0..=2 => 0,
            3 | 4 => 1,
            5 | 6 => 2,
            7 => 3,
            _ => 6,
        },
        Container => 5,
        Road => 2500,
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

// ── Minerals + extractor (M6) ─────────────────────────────────────────────────────────────────────

/// Energy-analog harvest for minerals: `HARVEST_MINERAL_POWER` 1 mineral per WORK per intent
/// (engine-mechanics.md:446,457, `common/constants.js:118`; the engine pipeline is
/// `creeps/harvest.js:88`, `calcBodyEffectiveness(body, WORK, 'harvest', HARVEST_MINERAL_POWER)`).
/// UNLIKE the source's `HARVEST_POWER` 2, and boosted by the WORK **harvest** ladder ×3/5/7
/// ([`work_harvest_mult`]) — NOT the ×1/2/3/4 action ladder.
pub const HARVEST_MINERAL_POWER: u32 = 1;

/// Mineral regen delay after exhaustion — `MINERAL_REGEN_TIME` 50000 (engine-mechanics.md:446,
/// `common/constants.js:298`, `minerals/tick.js:11`). The timer starts when the pool hits 0 and
/// the pool refills (density-amount) when `gameTime >= nextRegenerationTime − 1`
/// (`minerals/tick.js:14`).
pub const MINERAL_REGEN_TIME: u32 = 50_000;

/// The four mineral density tiers — `MINERAL_DENSITY` {1:15K, 2:35K, 3:70K, 4:100K}
/// (engine-mechanics.md:446, `common/constants.js:310`). `density` is 1..=4 (LOW/MODERATE/HIGH/
/// ULTRA — `constants.js:324-327`); any other value is out of the engine's vocabulary.
pub fn mineral_density_amount(density: u8) -> u32 {
    match density {
        1 => 15_000,
        2 => 35_000,
        3 => 70_000,
        4 => 100_000,
        _ => 0,
    }
}

/// The cumulative density-selection probability table — `MINERAL_DENSITY_PROBABILITY`
/// {1:0.1, 2:0.5, 3:0.9, 4:1.0} (`common/constants.js:316`), expressed per-mille for exact
/// integer selection (`minerals/tick.js:22-30`: a uniform draw picks the first density whose
/// cumulative probability is ≥ the draw). Quantized to per-mille — the exact table values are
/// all whole per-mille, so no rounding.
pub const MINERAL_DENSITY_PROBABILITY_Q: [(u8, u32); 4] = [(1, 100), (2, 500), (3, 900), (4, 1000)];

/// Re-roll probability for MODERATE/HIGH densities — `MINERAL_DENSITY_CHANGE` 0.05
/// (engine-mechanics.md:446, `common/constants.js:322`, `minerals/tick.js:20`), per-mille for the
/// seeded integer draw. LOW (1) and ULTRA (4) ALWAYS re-roll on regen (`minerals/tick.js:19`).
pub const MINERAL_DENSITY_CHANGE_Q: u32 = 50;

/// Density enum values (`common/constants.js:324-327`).
pub const DENSITY_LOW: u8 = 1;
pub const DENSITY_MODERATE: u8 = 2;
pub const DENSITY_HIGH: u8 = 3;
pub const DENSITY_ULTRA: u8 = 4;

/// Extractor cooldown ticks after a successful mineral harvest — `EXTRACTOR_COOLDOWN` 5
/// (engine-mechanics.md:446, `common/constants.js:272`; the harvest sets `extractor._cooldown`,
/// `creeps/harvest.js:108`, and the extractor tick decrements it, `extractors/tick.js:9-19`).
pub const EXTRACTOR_COOLDOWN: u32 = 5;

// ── Terminal recovery lever (M6) ────────────────────────────────────────────────────────────────
//
// The ABSTRACTION only (ADR 0040 §D4/§D7): a fixed mineral→energy exchange rate for the
// sell-mineral-for-energy recovery lever. This is NOT the engine's market — the real credit /
// order-book / MARKET_FEE 0.05 mechanics belong to ADR 0012 (`fairvalue.rs`, untouched). The sim
// prices one unit of a base mineral at a conservative fixed energy-equivalent so the recovery
// scenario can ask "does dumping a stocked mineral stash for energy speed T_recover?" without
// coupling to a market model. Chosen at 1 mineral = 1 energy (par) as the deliberately-neutral
// floor: the lever adds LIQUIDITY (a stuck stash becomes spendable energy), not free value — so a
// measured T_recover improvement is attributable to the liquidity, not an assumed exchange premium.

/// Energy credited per unit mineral sold via the terminal recovery lever — a fixed
/// num/den exchange rate (par: 1:1). Deliberately conservative (ADR 0040 §D4 documents the
/// abstraction; the market-priced version is ADR 0012's `mineral_value_e` follow-up).
pub const TERMINAL_SELL_ENERGY_NUM: u32 = 1;
pub const TERMINAL_SELL_ENERGY_DEN: u32 = 1;

/// Energy proceeds of selling `mineral_amount` units (exact integer, floored).
pub fn terminal_sale_energy(mineral_amount: u32) -> u32 {
    ((mineral_amount as u64 * TERMINAL_SELL_ENERGY_NUM as u64) / TERMINAL_SELL_ENERGY_DEN as u64) as u32
}

// ── Labs (M6) ─────────────────────────────────────────────────────────────────────────────────────

/// Lab mineral-store capacity — `LAB_MINERAL_CAPACITY` 3000 (engine-mechanics.md:303,460,
/// `common/constants.js:275`).
pub const LAB_MINERAL_CAPACITY: u32 = 3_000;
/// Lab energy-store capacity — `LAB_ENERGY_CAPACITY` 2000 (engine-mechanics.md:303,460,
/// `common/constants.js:276`).
pub const LAB_ENERGY_CAPACITY: u32 = 2_000;
/// Reaction/consumption amount per `runReaction`: `LAB_REACTION_AMOUNT` 5 — 5 in from EACH input
/// lab, 5 out to the output lab (engine-mechanics.md:301,460, `common/constants.js:280`;
/// `labs/run-reaction.js:12,55,67,77`). (`PWR_OPERATE_LAB` boosts this — power creeps NOT modeled.)
pub const LAB_REACTION_AMOUNT: u32 = 5;
/// Mineral consumed per body part boosted — `LAB_BOOST_MINERAL` 30 (engine-mechanics.md:317,460,
/// `common/constants.js:278`; `labs/boost-creep.js:15,43`).
pub const LAB_BOOST_MINERAL: u32 = 30;
/// Energy consumed per body part boosted — `LAB_BOOST_ENERGY` 20 (engine-mechanics.md:317,460,
/// `common/constants.js:277`; `labs/boost-creep.js:15,44`).
pub const LAB_BOOST_ENERGY: u32 = 20;

/// Per-compound reaction cooldown — `REACTION_TIME[product]` (engine-mechanics.md:302,311,468,
/// `common/constants.js:733-768`; `labs/run-reaction.js:56`). The annotated-unused `LAB_COOLDOWN`
/// 10 is deliberately NOT used (engine-mechanics.md:303,513). Only the compounds the boost
/// economy needs are tabled here (the base pairs + the boost chains); an unlisted product panics
/// loudly at the [`compound_tag`] boundary rather than silently guessing a cooldown.
pub fn reaction_time(compound: crate::state::SimResource) -> u32 {
    use crate::state::SimResource::*;
    match compound {
        // Base pairs (engine-mechanics.md:307, constants.js:734-737).
        Hydroxide => 20, // OH
        ZynthiumKeanite => 5,
        UtriumLemergite => 5,
        Ghodium => 5, // G (the compound; the base mineral G shares the tag — see state docs)
        // Upgrade chain GH/GH2O/XGH2O (constants.js:762-764): 10 / 15 / 80.
        GH => 10,
        GH2O => 15,
        XGH2O => 80,
        // Everything below is not produced in the M6 boost economy (only the upgrade chain is
        // brewed on-sim); tabled for completeness of the reaction the fence exercises.
        _ => panic!("REACTION_TIME not tabled for {compound:?} — add its constants.js:733-768 row"),
    }
}

// ── Boost effect multipliers on WORK actions (M6; `BOOSTS[WORK]`, constants.js:618-657) ─────────────
//
// The WORK boosts do NOT follow the ×1/2/3/4 action ladder (`BoostTier::action_mult`, which is
// correct for attack/heal/dismantle/move/carry). They are per-effect tables (engine-mechanics.md:
// 136 references the `BOOSTS` table at :617-731). Returned as (numerator, denominator) exact
// rationals so the resolver's `power × num / den` stays integer.

/// WORK **upgradeController** boost multiplier as an exact rational (`BOOSTS[WORK][GH|GH2O|XGH2O]
/// .upgradeController` = 1.5 / 1.8 / 2.0; constants.js:650-656). T0 = ×1.
pub fn work_upgrade_mult(tier: screeps_sim_core::BoostTier) -> (u32, u32) {
    use screeps_sim_core::BoostTier::*;
    match tier {
        None => (1, 1),
        T1 => (3, 2),  // GH2O... GH is the T1 upgrade boost → 1.5
        T2 => (9, 5),  // GH2O → 1.8
        T3 => (2, 1),  // XGH2O → 2.0
    }
}

/// WORK **build**/**repair** boost multiplier (`BOOSTS[WORK][LH|LH2O|XLH2O].build/.repair` =
/// 1.5 / 1.8 / 2.0; constants.js:628-639). Same ladder as upgrade by coincidence of the table.
pub fn work_build_mult(tier: screeps_sim_core::BoostTier) -> (u32, u32) {
    work_upgrade_mult(tier)
}

/// WORK **harvest** boost multiplier (`BOOSTS[WORK][UO|UHO2|XUHO2].harvest` = 3 / 5 / 7;
/// constants.js:618-627). Integer multipliers; T0 = ×1.
pub fn work_harvest_mult(tier: screeps_sim_core::BoostTier) -> u32 {
    use screeps_sim_core::BoostTier::*;
    match tier {
        None => 1,
        T1 => 3,
        T2 => 5,
        T3 => 7,
    }
}

/// Which `(body part, boost tier)` a mineral/compound applies as a boost — the `BOOSTS` table
/// keyed by mineral (`common/constants.js:617-731`), inverted to the sim's `(Part, BoostTier)`.
/// Only the compounds the M6 economy handles are mapped (the WORK-upgrade chain GH/GH2O/XGH2O);
/// an unmapped mineral is not a boost (`None` — `boostCreep` finds no boostable part and no-ops).
/// The tier is which rung of the ×1/2/3-tier chain the compound sits on.
pub fn boost_effect(compound: crate::state::SimResource) -> Option<(Part, screeps_sim_core::BoostTier)> {
    use crate::state::SimResource::*;
    use screeps_sim_core::BoostTier;
    Some(match compound {
        // WORK upgradeController chain (`BOOSTS[WORK]`, constants.js:649-656): GH=T1, GH2O=T2, XGH2O=T3.
        GH => (Part::Work, BoostTier::T1),
        GH2O => (Part::Work, BoostTier::T2),
        XGH2O => (Part::Work, BoostTier::T3),
        _ => return None,
    })
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
        // engine-mechanics.md:118 — repair 100 hits/WORK, range ≤ 3; REPAIR_COST 0.01 e/hit
        // (`creeps/repair.js:24`) inverted to 100 hits/energy for exact-integer arithmetic.
        assert_eq!(REPAIR_POWER, 100);
        assert_eq!(REPAIR_HITS_PER_ENERGY, 100);
        assert_eq!(REPAIR_RANGE, 3);
        // engine-mechanics.md:430 — road decay 100 hits / 1000 ticks, swamp ×5 (hitsMax 5000
        // plain / 25000 swamp), per-step wear pulls nextDecayTime by 1 × body.length.
        assert_eq!(ROAD_DECAY_AMOUNT, 100);
        assert_eq!(ROAD_DECAY_TIME, 1000);
        assert_eq!(ROAD_HITS, 5000);
        assert_eq!(ROAD_SWAMP_RATIO, 5);
        assert_eq!(ROAD_WEAROUT, 1);
        assert_eq!(road_hits_max(false), 5000);
        assert_eq!(road_hits_max(true), 25_000);
        // engine-mechanics.md:429 — container −5000 hits per 100 ticks (RCL 0) / 500 (RCL ≥ 1),
        // 250K hitsMax.
        assert_eq!(CONTAINER_DECAY, 5_000);
        assert_eq!(CONTAINER_DECAY_TIME, 100);
        assert_eq!(CONTAINER_DECAY_TIME_OWNED, 500);
        assert_eq!(CONTAINER_HITS, 250_000);
        // common/constants.js:341 / storage row — the M2 store capacities.
        assert_eq!(CONTAINER_CAPACITY, 2_000);
        assert_eq!(STORAGE_CAPACITY, 1_000_000);
    }

    /// M2 controller citation pins: UPGRADE_CONTROLLER_POWER 1 / RCL8 cap 15
    /// (engine-mechanics.md:458); CONTROLLER_DOWNGRADE_RESTORE 100 (constants.js:233,
    /// engine-mechanics.md:228 "+100+1/tick capped at full" — the +1 offsets the timestamp
    /// model, see tick.rs step 3d); CONTROLLER_LEVELS / CONTROLLER_DOWNGRADE tables
    /// (engine-mechanics.md:467, constants.js:213/:232).
    #[test]
    fn m2_controller_citation_pins() {
        assert_eq!(UPGRADE_CONTROLLER_POWER, 1);
        assert_eq!(CONTROLLER_MAX_UPGRADE_PER_TICK, 15);
        assert_eq!(CONTROLLER_DOWNGRADE_RESTORE, 100);
        let levels = [(1u8, 200u32), (2, 45_000), (3, 135_000), (4, 405_000), (5, 1_215_000), (6, 3_645_000), (7, 10_935_000)];
        for (l, v) in levels {
            assert_eq!(controller_levels(l), Some(v), "CONTROLLER_LEVELS[{l}]");
            // The downgrade ×0.9 progress refund (controllers/tick.js:66 `Math.round(× 0.9)`)
            // is exact-integer for every table value (all divisible by 10) — pinned so the
            // resolver's `× 9 / 10` arithmetic can never round differently from the engine.
            assert_eq!(v % 10, 0, "CONTROLLER_LEVELS[{l}] × 0.9 stays exact");
        }
        assert_eq!(controller_levels(8), None, "level 8 is max");
        assert_eq!(controller_levels(0), None);
        let clocks = [(1u8, 20_000u32), (2, 10_000), (3, 20_000), (4, 40_000), (5, 80_000), (6, 120_000), (7, 150_000), (8, 200_000)];
        for (l, v) in clocks {
            assert_eq!(controller_downgrade(l), v, "CONTROLLER_DOWNGRADE[{l}]");
            assert_eq!(v % 2, 0, "half-max clock (upgradeController.js:72 / tick.js:65) stays exact");
        }
        assert_eq!(controller_downgrade(0), 0);
    }

    /// M2 build citation pins: BUILD_POWER 5 / range 3 (`creeps/build.js:23,67`); the
    /// CONSTRUCTION_COST table (constants.js:192-209) + the swamp road ratio 5
    /// (constants.js:210, engine-mechanics.md:430); CONTROLLER_STRUCTURES allowances
    /// (constants.js:214-231) — extensions per the ADR table {2:5,3:10,4:20,5:30,6:40,7:50,8:60}.
    #[test]
    fn m2_build_citation_pins() {
        use crate::state::StructureKind::*;
        assert_eq!(BUILD_POWER, 5);
        assert_eq!(BUILD_RANGE, 3);
        assert_eq!(CONSTRUCTION_COST_ROAD_SWAMP_RATIO, 5);
        for (kind, cost) in [(Spawn, 15_000), (Extension, 3_000), (Road, 300), (Storage, 30_000), (Tower, 5_000), (Container, 5_000)] {
            assert_eq!(construction_cost(kind), cost, "{kind:?}");
        }
        for (rcl, n) in [(1u8, 0u32), (2, 5), (3, 10), (4, 20), (5, 30), (6, 40), (7, 50), (8, 60)] {
            assert_eq!(controller_structures(Extension, rcl), n, "extension allowance at RCL {rcl}");
        }
        assert_eq!(controller_structures(Spawn, 1), 1);
        assert_eq!(controller_structures(Spawn, 7), 2);
        assert_eq!(controller_structures(Spawn, 8), 3);
        assert_eq!(controller_structures(Storage, 3), 0);
        assert_eq!(controller_structures(Storage, 4), 1);
        assert_eq!(controller_structures(Tower, 3), 1);
        assert_eq!(controller_structures(Tower, 8), 6);
        assert_eq!(controller_structures(Container, 0), 5, "containers are RCL-free");
        assert_eq!(controller_structures(Road, 4), 2500);
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

    /// M6 mineral + extractor citation pins (engine-mechanics.md:446,457,466;
    /// `common/constants.js:298-327,271-272`): HARVEST_MINERAL_POWER 1, MINERAL_REGEN_TIME 50k,
    /// the density tiers {1:15K,2:35K,3:70K,4:100K}, the cumulative selection table
    /// {1:0.1,2:0.5,3:0.9,4:1.0}, MINERAL_DENSITY_CHANGE 0.05, EXTRACTOR_COOLDOWN 5.
    #[test]
    fn m6_mineral_citation_pins() {
        assert_eq!(HARVEST_MINERAL_POWER, 1, "engine-mechanics.md:446,457 (≠ source HARVEST_POWER 2)");
        assert_eq!(MINERAL_REGEN_TIME, 50_000, "engine-mechanics.md:446");
        assert_eq!(
            [mineral_density_amount(1), mineral_density_amount(2), mineral_density_amount(3), mineral_density_amount(4)],
            [15_000, 35_000, 70_000, 100_000],
            "MINERAL_DENSITY tiers (constants.js:310)"
        );
        assert_eq!(mineral_density_amount(0), 0, "out-of-vocabulary density is 0");
        assert_eq!(
            MINERAL_DENSITY_PROBABILITY_Q,
            [(1, 100), (2, 500), (3, 900), (4, 1000)],
            "cumulative selection per-mille (constants.js:316)"
        );
        assert_eq!(MINERAL_DENSITY_CHANGE_Q, 50, "0.05 as per-mille (constants.js:322)");
        assert_eq!((DENSITY_LOW, DENSITY_MODERATE, DENSITY_HIGH, DENSITY_ULTRA), (1, 2, 3, 4));
        assert_eq!(EXTRACTOR_COOLDOWN, 5, "constants.js:272");
    }

    /// M6 lab citation pins (engine-mechanics.md:301-303,317,460,468;
    /// `common/constants.js:275-280,733-768,617-731`): store caps 3000/2000, LAB_REACTION_AMOUNT 5,
    /// boost 30 mineral + 20 energy, the REACTION_TIME rows for the brewed chain, and the
    /// WORK-boost effect multipliers (harvest 3/5/7, build & upgrade 1.5/1.8/2.0 — NOT the action
    /// ladder).
    #[test]
    fn m6_lab_citation_pins() {
        use crate::state::SimResource::*;
        assert_eq!((LAB_MINERAL_CAPACITY, LAB_ENERGY_CAPACITY), (3_000, 2_000), "constants.js:275-276");
        assert_eq!(LAB_REACTION_AMOUNT, 5, "constants.js:280");
        assert_eq!((LAB_BOOST_MINERAL, LAB_BOOST_ENERGY), (30, 20), "constants.js:277-278");
        // REACTION_TIME for the brewed upgrade chain + the shared pairs (constants.js:733-768).
        assert_eq!(reaction_time(Hydroxide), 20, "OH 20t");
        assert_eq!(reaction_time(ZynthiumKeanite), 5, "ZK 5t");
        assert_eq!(reaction_time(UtriumLemergite), 5, "UL 5t");
        assert_eq!(reaction_time(Ghodium), 5, "G 5t (the compound)");
        assert_eq!((reaction_time(GH), reaction_time(GH2O), reaction_time(XGH2O)), (10, 15, 80), "GH/GH2O/XGH2O");

        // WORK-effect boost multipliers (BOOSTS[WORK], constants.js:618-657) — the exact table.
        use screeps_sim_core::BoostTier::*;
        assert_eq!(
            [work_harvest_mult(None), work_harvest_mult(T1), work_harvest_mult(T2), work_harvest_mult(T3)],
            [1, 3, 5, 7],
            "harvest UO/UHO2/XUHO2 = 3/5/7"
        );
        assert_eq!(work_upgrade_mult(None), (1, 1));
        assert_eq!(work_upgrade_mult(T1), (3, 2), "GH → 1.5");
        assert_eq!(work_upgrade_mult(T2), (9, 5), "GH2O → 1.8");
        assert_eq!(work_upgrade_mult(T3), (2, 1), "XGH2O → 2.0");
        assert_eq!(work_build_mult(T3), (2, 1), "XLH2O → 2.0 (same table as upgrade)");

        // The boost-effect inverse: the upgrade chain compounds → (WORK, tier).
        assert_eq!(boost_effect(GH), Some((Part::Work, T1)));
        assert_eq!(boost_effect(GH2O), Some((Part::Work, T2)));
        assert_eq!(boost_effect(XGH2O), Some((Part::Work, T3)));
        assert_eq!(boost_effect(Energy), Option::None, "energy is not a boost");
    }

    /// The reaction recipe table is symmetric and covers the brewed chain (engine
    /// `REACTIONS[a][b]`, constants.js:484-615).
    #[test]
    fn m6_reaction_recipe_table() {
        use crate::state::{reaction_product, SimResource::*};
        // Order-independence: REACTIONS[a][b] == REACTIONS[b][a].
        assert_eq!(reaction_product(Hydrogen, Oxygen), Some(Hydroxide));
        assert_eq!(reaction_product(Oxygen, Hydrogen), Some(Hydroxide));
        assert_eq!(reaction_product(Keanium, Zynthium), Some(ZynthiumKeanite));
        assert_eq!(reaction_product(Utrium, Lemergium), Some(UtriumLemergite));
        assert_eq!(reaction_product(ZynthiumKeanite, UtriumLemergite), Some(Ghodium));
        // The upgrade boost chain: G+H→GH, GH+OH→GH2O, GH2O+X→XGH2O.
        assert_eq!(reaction_product(Ghodium, Hydrogen), Some(GH));
        assert_eq!(reaction_product(GH, Hydroxide), Some(GH2O));
        assert_eq!(reaction_product(GH2O, Catalyst), Some(XGH2O));
        // An untabled pair has no product (a no-op).
        assert_eq!(reaction_product(Hydrogen, Hydrogen), None);
    }

    /// The terminal recovery lever's fixed exchange rate (ADR 0040 §D4 abstraction): 1:1 par.
    #[test]
    fn m6_terminal_recovery_lever_pin() {
        assert_eq!((TERMINAL_SELL_ENERGY_NUM, TERMINAL_SELL_ENERGY_DEN), (1, 1), "par: 1 mineral = 1 energy");
        assert_eq!(terminal_sale_energy(500), 500);
        assert_eq!(terminal_sale_energy(0), 0);
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
