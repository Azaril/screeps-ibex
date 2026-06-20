//! Richer combat metrics over a [`CombatRecording`] (P2.U5): the five families the operator named —
//! **healing, DPS, positioning, survivability, efficiency** — computed per side from the replay so
//! the EXP-* register can gate behavior on more than "did it die".
//!
//! Two review must-fixes are baked in:
//! 1. **Tower damage is attributed separately from creep DPS.** A creep's recorded `damage_taken`
//!    aggregates *all* incoming damage that tick (enemy creeps *and* enemy towers). So the damage a
//!    side's **creeps** dealt is the enemy's total damage-taken **minus** that side's tower output
//!    ([`SideMetrics::tower_damage_dealt`] is computed from the tower fire intents + falloff, then
//!    subtracted) — otherwise a tower-heavy defense would flatter its creeps' DPS.
//! 2. **Cohesion reuses [`screeps_combat_decision::cohesion::measure`]** (the shared live+sim measure),
//!    never a re-implementation, so the metric matches the seg-57 canary the live bot emits.
//!
//! Positioning is intentionally **body-free** (the recording carries no bodies): it reads geometry —
//! mean range to the nearest enemy and melee-exposure rate — which still separates a kiter (holds
//! ~3, ~0 melee exposure) from a brawler (range 1) without needing per-creep roles.

use screeps::{Position, RoomCoordinate, RoomName};
use screeps_combat_decision::cohesion::{measure, CohesionSample};
use screeps_combat_engine::damage::tower_attack_damage_at_range;
use screeps_combat_engine::record::{CreepFrame, TowerFrame};
use screeps_combat_engine::{CombatRecording, CreepId, PlayerId, TowerAction};
use std::collections::{HashMap, HashSet};

/// All metrics aren't actually in different rooms (the sim is single-room); range is translation-
/// invariant within a room, so a synthetic shared room reconstructs `Position`s for `get_range_to`
/// / `cohesion::measure` without the recording needing to store a room name.
fn synth_pos(x: u8, y: u8) -> Position {
    let room: RoomName = "W1N1".parse().expect("valid room");
    Position::new(
        RoomCoordinate::new(x.min(49)).expect("clamped"),
        RoomCoordinate::new(y.min(49)).expect("clamped"),
        room,
    )
}

/// One side's measured combat performance over a whole engagement, in the five families.
#[derive(Clone, Debug)]
pub struct SideMetrics {
    pub side: PlayerId,
    pub ticks: u32,

    // ── survivability ──
    /// Distinct creeps this side fielded (ever seen alive in the replay).
    pub fielded: u32,
    /// Alive at the last frame.
    pub survivors: u32,
    pub deaths: u32,
    /// `survivors / fielded` (1.0 if it fielded none).
    pub survival_rate: f64,
    /// Effective damage this side's creeps absorbed (what came off hits, post-TOUGH/boost).
    pub damage_taken: u32,
    /// `raw − effective` summed: TOUGH/boost soak + overkill that never landed.
    pub damage_absorbed: u32,

    // ── DPS (creep vs tower attributed separately — must-fix 1) ──
    /// Effective damage this side dealt to enemy **creeps** from all sources.
    pub damage_to_enemy_creeps: u32,
    /// The portion of `damage_to_enemy_creeps` attributable to this side's **towers** (raw falloff
    /// output of every tower Attack intent on an enemy creep) — reported on its own so creep DPS is
    /// uncontaminated.
    pub tower_damage_dealt: u32,
    /// `damage_to_enemy_creeps − tower_damage_dealt` (clamped): the side's **creep** DPS contribution.
    pub creep_damage_dealt: u32,
    /// Damage this side dealt to enemy **structures** (incl. towers) — breach/dismantle DPS, summed
    /// from per-tick enemy-structure HP loss.
    pub structure_damage_dealt: u32,

    // ── healing ──
    /// Total HP this side's creeps received from heals over the run.
    pub hp_healed: u32,

    // ── positioning (body-free geometry) ──
    /// Mean Chebyshev range from each living creep to its nearest living enemy, averaged over all
    /// creep-ticks (∞-safe: 0 if never co-present).
    pub mean_nearest_enemy_range: f64,
    /// Fraction of creep-ticks spent within range 1 of an enemy that can **actually deal melee
    /// damage** (a live ATTACK part) — high = brawling, ~0 = clean kiting. A disarmed adjacent enemy
    /// is no threat and doesn't count.
    pub melee_exposure_rate: f64,

    // ── efficiency ──
    /// Energy this side's towers spent firing (shots × `TOWER_ENERGY_COST`).
    pub tower_energy_spent: u32,
    /// `enemy damage taken / own damage taken` (the kill-exchange ratio; ∞ if untouched while
    /// dealing damage, 0 if it dealt nothing).
    pub damage_exchange_ratio: f64,
}

impl SideMetrics {
    /// Compute `side`'s metrics over `rec`, treating every other owner as the enemy.
    pub fn from_recording(rec: &CombatRecording, side: PlayerId) -> Self {
        let ticks = rec.frames.len() as u32;

        // Per-creep owner + alive tracking across frames.
        let mut owner_of: HashMap<CreepId, PlayerId> = HashMap::new();
        let mut fielded_side: HashSet<CreepId> = HashSet::new();
        for f in &rec.frames {
            for c in &f.creeps {
                owner_of.insert(c.id, c.owner);
                if c.owner == side {
                    fielded_side.insert(c.id);
                }
            }
        }

        // ── survivability + healing + own damage taken ──
        let mut damage_taken = 0u32;
        let mut damage_absorbed = 0u32;
        let mut hp_healed = 0u32;
        let mut enemy_damage_taken = 0u32;
        let mut died_side: HashSet<CreepId> = HashSet::new();
        for f in &rec.frames {
            for r in &f.results {
                match owner_of.get(&r.id) {
                    Some(&o) if o == side => {
                        damage_taken += r.damage_taken;
                        damage_absorbed += r.raw_damage.saturating_sub(r.damage_taken);
                        hp_healed += r.healed;
                        if r.died {
                            died_side.insert(r.id);
                        }
                    }
                    Some(_) => enemy_damage_taken += r.damage_taken,
                    None => {}
                }
            }
        }
        let deaths = died_side.len() as u32;
        let fielded = fielded_side.len() as u32;
        // A frame snapshots START-of-tick state, so a creep that dies *this* tick still appears in the
        // last frame's `creeps`; survivors must subtract the distinct creeps that ever died.
        let survivors = fielded - deaths;
        let survival_rate = if fielded == 0 { 1.0 } else { survivors as f64 / fielded as f64 };

        // ── tower damage attribution (must-fix 1) + tower energy spent ──
        let (tower_damage_dealt, tower_shots) = tower_attack_output(rec, side, &owner_of);
        let tower_energy_spent = tower_shots * screeps_combat_engine::constants::TOWER_ENERGY_COST;
        let damage_to_enemy_creeps = enemy_damage_taken;
        let creep_damage_dealt = damage_to_enemy_creeps.saturating_sub(tower_damage_dealt);

        // ── structure DPS (enemy structures + towers HP lost over the run) ──
        let structure_damage_dealt = enemy_structure_hp_lost(rec, side);

        // ── positioning (geometry) ──
        let (mean_nearest_enemy_range, melee_exposure_rate) = positioning(rec, side);

        let damage_exchange_ratio = if damage_taken == 0 {
            if damage_to_enemy_creeps + structure_damage_dealt > 0 {
                f64::INFINITY
            } else {
                0.0
            }
        } else {
            (damage_to_enemy_creeps + structure_damage_dealt) as f64 / damage_taken as f64
        };

        SideMetrics {
            side,
            ticks,
            fielded,
            survivors,
            deaths,
            survival_rate,
            damage_taken,
            damage_absorbed,
            damage_to_enemy_creeps,
            tower_damage_dealt,
            creep_damage_dealt,
            structure_damage_dealt,
            hp_healed,
            mean_nearest_enemy_range,
            melee_exposure_rate,
            tower_energy_spent,
            damage_exchange_ratio,
        }
    }
}

/// Sum the raw falloff output of every `side`-owned tower's Attack on an enemy creep this run, and
/// count the shots (for energy spent). Towers are looked up by id in the same frame's `towers`
/// snapshot, the target creep in the same frame's `creeps` snapshot (start-of-tick positions, which
/// is what the engine fires from).
fn tower_attack_output(rec: &CombatRecording, side: PlayerId, owner_of: &HashMap<CreepId, PlayerId>) -> (u32, u32) {
    let mut damage = 0u32;
    let mut shots = 0u32;
    for f in &rec.frames {
        let tower_by_id: HashMap<_, &TowerFrame> = f.towers.iter().map(|t| (t.id, t)).collect();
        let creep_by_id: HashMap<_, &CreepFrame> = f.creeps.iter().map(|c| (c.id, c)).collect();
        for (tid, action) in &f.tower_intents {
            let Some(tower) = tower_by_id.get(tid) else { continue };
            if tower.owner != side {
                continue;
            }
            if let TowerAction::Attack(target) = action {
                // Only count fire at an enemy creep (heal/repair aren't DPS).
                if owner_of.get(target).is_some_and(|&o| o != side) {
                    if let Some(c) = creep_by_id.get(target) {
                        let range = synth_pos(tower.x, tower.y).get_range_to(synth_pos(c.x, c.y));
                        damage += tower_attack_damage_at_range(range);
                        shots += 1;
                    }
                }
            }
        }
    }
    (damage, shots)
}

/// Total HP lost by enemy structures (incl. towers) over the run — first-seen minus last-seen hits
/// (a destroyed structure's last-seen is just before it drops out, so this also counts the killing
/// blow up to that frame; the breach/dismantle DPS proxy).
fn enemy_structure_hp_lost(rec: &CombatRecording, side: PlayerId) -> u32 {
    let mut first: HashMap<u32, u32> = HashMap::new();
    let mut last: HashMap<u32, u32> = HashMap::new();
    for f in &rec.frames {
        for s in &f.structures {
            if s.owner != Some(side) {
                first.entry(s.id).or_insert(s.hits);
                last.insert(s.id, s.hits);
            }
        }
        for t in &f.towers {
            if t.owner != side {
                first.entry(t.id).or_insert(t.hits);
                last.insert(t.id, t.hits);
            }
        }
    }
    first.iter().map(|(id, &fh)| fh.saturating_sub(*last.get(id).unwrap_or(&fh))).sum()
}

/// Positioning: mean nearest-enemy range over living `side` creep-ticks, and the **melee-exposure
/// rate** — the fraction of those creep-ticks spent within range 1 of an enemy **that can actually
/// deal melee damage** (a live ATTACK part). A disarmed creep adjacent to you is no threat, so it
/// doesn't count as exposure (it inflated the metric when a kiter sat next to a chaser whose ATTACK
/// parts had been shot off). `mean_nearest_enemy_range` still measures all enemies (raw geometry).
fn positioning(rec: &CombatRecording, side: PlayerId) -> (f64, f64) {
    let mut range_sum = 0u64;
    let mut samples = 0u64;
    let mut melee_ticks = 0u64;
    for f in &rec.frames {
        let enemies: Vec<Position> = f.creeps.iter().filter(|c| c.owner != side).map(|c| synth_pos(c.x, c.y)).collect();
        if enemies.is_empty() {
            continue;
        }
        // Only enemies that can still deal melee damage count toward melee exposure.
        let armed: Vec<Position> =
            f.creeps.iter().filter(|c| c.owner != side && c.attack_power > 0).map(|c| synth_pos(c.x, c.y)).collect();
        for c in f.creeps.iter().filter(|c| c.owner == side) {
            let p = synth_pos(c.x, c.y);
            let nearest = enemies.iter().map(|e| p.get_range_to(*e)).min().unwrap_or(0);
            range_sum += nearest as u64;
            if armed.iter().any(|e| p.get_range_to(*e) <= 1) {
                melee_ticks += 1;
            }
            samples += 1;
        }
    }
    if samples == 0 {
        (0.0, 0.0)
    } else {
        (range_sum as f64 / samples as f64, melee_ticks as f64 / samples as f64)
    }
}

/// Per-tick cohesion of `side`'s living creeps via the shared [`measure`] (must-fix 2). `tol` is the
/// in-formation tolerance; formation is `None` (loose-centroid cohesion — the metric we score on).
pub fn cohesion_series(rec: &CombatRecording, side: PlayerId, tol: u32) -> Vec<CohesionSample> {
    rec.frames
        .iter()
        .map(|f| {
            let positions: Vec<Position> = f.creeps.iter().filter(|c| c.owner == side).map(|c| synth_pos(c.x, c.y)).collect();
            measure(&positions, None, tol)
        })
        .collect()
}

/// The worst (largest) `max_pairwise` cohesion spread `side` ever reached — the scatter peak (lower
/// is tighter). 0 if the side never had ≥2 creeps co-present.
pub fn worst_cohesion(rec: &CombatRecording, side: PlayerId) -> u32 {
    cohesion_series(rec, side, 0).iter().map(|s| s.max_pairwise).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_combat_agent::opponents::{run_engagement, world_from_units, KiteAgent, RushAgent, TurtleAgent, Unit};
    use screeps_combat_agent::IbexAgent;
    use screeps::Part;

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }

    #[test]
    fn positioning_metric_discriminates_a_kiter_from_a_brawler() {
        // The positioning family must separate a kiter (holds distance, low melee exposure) from a
        // brawl (both sides closing to range 1). Deterministic scripted agents + a short pre-corner
        // run (a straight-line kiter eventually corners against the wall — itself a U8 finding — so
        // we measure before that). 1v1 nearest-range is symmetric, so we compare ACROSS scenarios.
        let kite = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7), (Part::Move, 7)], vec![pos(30, 25)])],
            1,
            &[Unit::new(vec![(Part::Attack, 10), (Part::Move, 10)], vec![pos(27, 25)])],
        );
        let ko = run_engagement(kite, room(), 0, pos(30, 25), &mut KiteAgent, 1, pos(27, 25), &mut RushAgent, 12);
        let km = SideMetrics::from_recording(&ko.recording, 0);

        // TOUGH-front so the ATTACK parts survive the fight (front-to-back degradation strips TOUGH
        // first) — keeps both brawlers *armed* and adjacent, so the refined melee-exposure (armed
        // enemies only) stays high through the run rather than collapsing as parts strip.
        let brawl = world_from_units(
            0,
            &[Unit::new(vec![(Part::Tough, 10), (Part::Attack, 10), (Part::Move, 10)], vec![pos(30, 25)])],
            1,
            &[Unit::new(vec![(Part::Tough, 10), (Part::Attack, 10), (Part::Move, 10)], vec![pos(27, 25)])],
        );
        let bo = run_engagement(brawl, room(), 0, pos(30, 25), &mut RushAgent, 1, pos(27, 25), &mut RushAgent, 16);
        let bm = SideMetrics::from_recording(&bo.recording, 0);

        // No towers in either → zero tower contamination, all DPS is creep DPS (must-fix 1).
        assert_eq!(km.tower_damage_dealt, 0);
        assert_eq!(km.creep_damage_dealt, km.damage_to_enemy_creeps);
        // The kiter holds more distance and spends far less time in melee than the brawl does.
        assert!(
            km.mean_nearest_enemy_range > bm.mean_nearest_enemy_range,
            "kiter held {} vs brawl {}",
            km.mean_nearest_enemy_range,
            bm.mean_nearest_enemy_range
        );
        assert!(
            km.melee_exposure_rate < bm.melee_exposure_rate,
            "kiter melee {} vs brawl {}",
            km.melee_exposure_rate,
            bm.melee_exposure_rate
        );
        assert!(km.melee_exposure_rate < 0.2, "a kiter barely touches melee pre-corner, got {}", km.melee_exposure_rate);
        assert!(bm.melee_exposure_rate > 0.3, "a brawl lives in (armed) melee, got {}", bm.melee_exposure_rate);
    }

    #[test]
    fn melee_exposure_ignores_a_disarmed_adjacent_enemy() {
        // The motivating case: a 7-RANGED kiter vs a 10-ATTACK chaser. The kiter chips the chaser's
        // front-loaded ATTACK parts off before/as it closes, then sits next to a DISARMED chaser. The
        // refined metric must NOT count that harmless adjacency — exposure stays low even though the
        // creeps are at range 1 for many ticks (raw adjacency would read ~0.45 here).
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7), (Part::Move, 7)], vec![pos(30, 25)])],
            1,
            &[Unit::new(vec![(Part::Attack, 10), (Part::Move, 10)], vec![pos(27, 25)])],
        );
        let out = run_engagement(world, room(), 0, pos(30, 25), &mut IbexAgent, 1, pos(27, 25), &mut RushAgent, 40);
        let m = SideMetrics::from_recording(&out.recording, 0);
        assert_eq!(m.damage_taken, 0, "the kiter is never actually hit");
        assert!(m.melee_exposure_rate < 0.1, "adjacency to a disarmed chaser isn't exposure, got {}", m.melee_exposure_rate);
    }

    #[test]
    fn tower_damage_is_attributed_separately_from_creep_dps() {
        use screeps_combat_agent::scenario::ScenarioBuilder;
        use screeps_combat_agent::{HoldAgent, IbexAgent};
        // A lone defender creep + a friendly tower fire on a single attacker. The attacker's total
        // damage-taken must split into tower output (>0) and creep DPS (>0), and they must sum back.
        let mut b = ScenarioBuilder::from_units(
            room(),
            0,
            &[Unit::new(vec![(Part::RangedAttack, 5)], vec![pos(25, 24)])], // our defender
            1,
            &[Unit::new(vec![(Part::Move, 5)], vec![pos(25, 27)])], // a soft attacker (no heal)
        );
        b.tower(0, 25, 25, 1000); // our tower
        let world = b.build();
        let out = run_engagement(world, room(), 0, pos(25, 24), &mut IbexAgent, 1, pos(25, 27), &mut HoldAgent, 12);
        let m = SideMetrics::from_recording(&out.recording, 0);
        assert!(m.tower_damage_dealt > 0, "the tower fired on the attacker");
        assert!(m.creep_damage_dealt > 0, "the defender also chipped it");
        assert_eq!(
            m.creep_damage_dealt + m.tower_damage_dealt,
            m.damage_to_enemy_creeps,
            "the split reconstructs the total (no double-count)"
        );
        assert!(m.tower_energy_spent >= 10, "at least one 10-energy shot");
    }

    #[test]
    fn turtle_metrics_show_healing_and_survival() {
        // A 5-HEAL turtle out-healed by 3×ranged should die (survival 0) but record heals received.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7)], vec![pos(25, 22), pos(24, 22), pos(26, 22)])],
            1,
            &[Unit::new(vec![(Part::Heal, 5)], vec![pos(25, 25)])],
        );
        let out = run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 30);
        let attacker = SideMetrics::from_recording(&out.recording, 0);
        let turtle = SideMetrics::from_recording(&out.recording, 1);
        assert_eq!(attacker.fielded, 3);
        assert_eq!(attacker.survivors, 3, "no losses focus-firing a lone turtle");
        assert_eq!(turtle.deaths, 1);
        assert_eq!(turtle.survival_rate, 0.0);
        assert!(turtle.hp_healed > 0, "the turtle self-healed before dying");
        assert!(attacker.creep_damage_dealt > 0);
    }

    #[test]
    fn cohesion_uses_the_shared_measure() {
        // Three friendly creeps standing together → worst_cohesion is small (tight).
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::Move, 1)], vec![pos(25, 25), pos(26, 25), pos(25, 26)])],
            1,
            &[],
        );
        let out = run_engagement(world, room(), 0, pos(25, 25), &mut IbexAgent, 1, pos(40, 40), &mut IbexAgent, 3);
        assert!(worst_cohesion(&out.recording, 0) <= 2, "a clustered trio stays tight");
    }
}

#[cfg(test)]
mod u8 {
    use super::*;
    use screeps::Part;
    use screeps_combat_agent::opponents::{run_engagement, world_from_units, RushAgent, Unit};
    use screeps_combat_agent::IbexAgent;

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }

    #[test]
    fn edge_aware_kiter_rounds_a_corner_instead_of_pinning() {
        // The U8 fix: a kiter started pinned in the SE corner with a slower-but-armed chaser between
        // it and open space. Raw "flee = max distance from the threat" would drive it into (49,49)
        // and pin it at range 1; the edge repulsors make it round the corner toward the interior.
        // Kiter: fast (10 MOVE), low DPS (5 RA) so the TOUGH chaser stays armed the whole run.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 5), (Part::Move, 10)], vec![pos(47, 47)])],
            1,
            &[Unit::new(vec![(Part::Attack, 10), (Part::Move, 8), (Part::Tough, 10)], vec![pos(45, 45)])],
        );
        let out = run_engagement(world, room(), 0, pos(47, 47), &mut IbexAgent, 1, pos(45, 45), &mut RushAgent, 30);
        let m = SideMetrics::from_recording(&out.recording, 0);
        assert_eq!(m.survivors, 1, "the kiter escapes the corner and lives");

        let last = out.recording.frames.last().unwrap();
        let kiter = last.creeps.iter().find(|c| c.owner == 0).expect("kiter alive");
        let corner = synth_pos(49, 49);
        assert!(
            synth_pos(kiter.x, kiter.y).get_range_to(corner) >= 10,
            "the kiter broke out toward open space, not into the corner (ended {} from (49,49))",
            synth_pos(kiter.x, kiter.y).get_range_to(corner)
        );

        // After the breakout it holds standoff distance: the second half averages > range 2.
        let half = out.recording.frames.len() / 2;
        let (mut sum, mut n) = (0u32, 0u32);
        for f in &out.recording.frames[half..] {
            if let (Some(k), Some(e)) = (f.creeps.iter().find(|c| c.owner == 0), f.creeps.iter().find(|c| c.owner == 1)) {
                sum += synth_pos(k.x, k.y).get_range_to(synth_pos(e.x, e.y));
                n += 1;
            }
        }
        assert!(n > 0 && (sum as f64 / n as f64) > 2.0, "holds standoff after breaking out");
    }

    #[test]
    fn solo_healer_kites_a_melee_chaser_instead_of_dying() {
        // U8-2: a pure support creep (HEAL + MOVE, no offense) used to walk up and get cut down by a
        // melee chaser. It now evades + self-heals — surviving the run and taking far less damage.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::Heal, 5), (Part::Move, 5)], vec![pos(30, 25)])],
            1,
            &[Unit::new(vec![(Part::Attack, 7), (Part::Move, 7)], vec![pos(27, 25)])],
        );
        let out = run_engagement(world, room(), 0, pos(30, 25), &mut IbexAgent, 1, pos(27, 25), &mut RushAgent, 40);
        let m = SideMetrics::from_recording(&out.recording, 0);
        assert_eq!(m.survivors, 1, "the healer evades + self-heals instead of standing to die");
        assert!(m.hp_healed > 0, "it self-heals while kiting");
    }
}
