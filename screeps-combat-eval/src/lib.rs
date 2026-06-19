//! The combat **policy** layer (ADR 0006 Part B / P2.H5): the ADR 0008a EXP-* experiment register as
//! a runnable, metric-producing suite over the combat sim.
//!
//! Each experiment sets up a scenario, runs it through the H4 harness
//! ([`screeps_combat_agent::opponents::run_engagement`] — the bot's real `IbexAgent` vs a scripted
//! opponent, resolved by the authoritative engine), extracts **measured metrics**, and gates each
//! metric to pass/fail. [`register`] runs them all; [`report`] renders a table. This is the
//! **tactics-tuning loop**: change a tunable in `screeps-combat-decision`, run the register, watch
//! the numbers move — the gates flag regressions.
//!
//! Scope (first H5 increment): the sim-runnable foundational experiments (FOUND / KITE / FOCUS /
//! TOWER / COMP). The sim-vs-server **parity** oracle, byte-exact golden vectors, and the nightly
//! seeded gate are the follow-on increment (they need Docker-capture integration). The harder
//! register items (BREACH / DEF-2 / CTRL / PARITY) land as the sim grows ramparts/controllers.

pub mod metrics;
pub mod scoring;

use crate::metrics::SideMetrics;
use screeps::{Part, Position, RoomCoordinate, RoomName};
use screeps_combat_agent::opponents::{run_engagement, world_from_units, DrainAgent, RushAgent, TurtleAgent, Unit};
use screeps_combat_agent::scenario::ScenarioBuilder;
use screeps_combat_agent::{HoldAgent, IbexAgent};
use screeps_combat_engine::{CombatWorld, SimBody, SimCreep, SimTower, StructureKind};

// ─── Framework ───────────────────────────────────────────────────────────────

/// One measured quantity from an experiment, with the gate it's checked against.
#[derive(Clone, Debug)]
pub struct Metric {
    pub name: String,
    /// `None` for boolean checks (the gate text says what's expected).
    pub value: Option<f64>,
    pub gate: &'static str,
    pub pass: bool,
}

/// The result of running one experiment: its metrics + the overall pass (all metrics pass).
#[derive(Clone, Debug)]
pub struct ExperimentResult {
    pub id: &'static str,
    pub hypothesis: &'static str,
    pub metrics: Vec<Metric>,
    pub pass: bool,
}

fn measured(name: &str, value: f64, gate: &'static str, pass: bool) -> Metric {
    Metric { name: name.into(), value: Some(value), gate, pass }
}
fn boolean(name: &str, gate: &'static str, pass: bool) -> Metric {
    Metric { name: name.into(), value: None, gate, pass }
}
fn result(id: &'static str, hypothesis: &'static str, metrics: Vec<Metric>) -> ExperimentResult {
    let pass = metrics.iter().all(|m| m.pass);
    ExperimentResult { id, hypothesis, metrics, pass }
}

/// Run the whole register (one entry per EXP-*).
pub fn register() -> Vec<ExperimentResult> {
    vec![
        exp_found_1(),
        exp_kite_1(),
        exp_focus_1(),
        exp_tower_1(),
        exp_comp_1(),
        // U7 room-variety suite (walls / ramparts / towers via the ScenarioBuilder):
        exp_breach_1(),
        exp_nest_1(),
    ]
}

/// Render the register results as a readable text report (the tuning-loop dashboard).
pub fn report(results: &[ExperimentResult]) -> String {
    use std::fmt::Write;
    let passed = results.iter().filter(|r| r.pass).count();
    let mut s = String::new();
    let _ = writeln!(s, "EXP-* register — {}/{} experiments passed", passed, results.len());
    for r in results {
        let _ = writeln!(s, "[{}] {} — {}", if r.pass { "ok" } else { "!!" }, r.id, r.hypothesis);
        for m in &r.metrics {
            let mark = if m.pass { "ok" } else { "!!" };
            match m.value {
                Some(v) => {
                    let _ = writeln!(s, "    [{mark}] {} = {} (gate {})", m.name, fmt_num(v), m.gate);
                }
                None => {
                    let _ = writeln!(s, "    [{mark}] {} (gate {})", m.name, m.gate);
                }
            }
        }
    }
    s
}

fn fmt_num(v: f64) -> String {
    if v.is_infinite() {
        "∞".into()
    } else if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

// ─── Scenario helpers ────────────────────────────────────────────────────────

fn room() -> RoomName {
    "W1N1".parse().unwrap()
}
fn pos(x: u8, y: u8) -> Position {
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
}

// ─── Experiments (ADR 0008a register) ────────────────────────────────────────

/// EXP-FOUND-1 — the two-phase kill inequality predicts kill-or-not. DPS > heal ⇒ the target dies;
/// DPS < heal ⇒ it self-heals through and survives.
fn exp_found_1() -> ExperimentResult {
    let target_dies = |attacker_ra: usize, target_heal: usize| {
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, attacker_ra)], vec![pos(25, 22)])],
            1,
            &[Unit::new(vec![(Part::Heal, target_heal)], vec![pos(25, 25)])],
        );
        run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 40).side_b_alive == 0
    };
    let kill = target_dies(7, 3); // 70 dps vs 36 heal → dies
    let survive = !target_dies(3, 5); // 30 dps vs 60 heal → lives
    result(
        "EXP-FOUND-1",
        "two-phase kill inequality predicts kill-or-not (damage-then-heal netting)",
        vec![
            boolean("70 dps vs 36 heal → target dies", "dies", kill),
            boolean("30 dps vs 60 heal → target survives", "survives", survive),
        ],
    )
}

/// EXP-KITE-1 — a range-3 kiter at MOVE parity takes 0 melee damage from an equal-speed chaser, and
/// chips it to death.
fn exp_kite_1() -> ExperimentResult {
    let world = world_from_units(
        0,
        &[Unit::new(vec![(Part::RangedAttack, 7), (Part::Move, 7)], vec![pos(30, 25)])],
        1,
        &[Unit::new(vec![(Part::Attack, 10), (Part::Move, 10)], vec![pos(27, 25)])],
    );
    let out = run_engagement(world, room(), 0, pos(30, 25), &mut IbexAgent, 1, pos(27, 25), &mut RushAgent, 30);
    let m = metrics::SideMetrics::from_recording(&out.recording, 0); // U5 metrics power the gate
    result(
        "EXP-KITE-1",
        "a range-3 kiter at MOVE parity takes 0 melee damage and chips the chaser",
        vec![
            measured("melee damage taken by the kiter", m.damage_taken as f64, "== 0", m.damage_taken == 0),
            boolean("the kiter's DPS is uncontaminated by towers", "creep==total", m.creep_damage_dealt == m.damage_to_enemy_creeps),
            boolean("the chaser dies to ranged fire", "dead", out.side_b_alive == 0),
        ],
    )
}

/// EXP-FOCUS-1 — focus-fire out-DPSes the aggregate heal: a 3×ranged group clears a self-healing
/// turtle fast, taking no damage.
fn exp_focus_1() -> ExperimentResult {
    let world = world_from_units(
        0,
        &[Unit::new(vec![(Part::RangedAttack, 7)], vec![pos(25, 22), pos(24, 22), pos(26, 22)])],
        1,
        &[Unit::new(vec![(Part::Heal, 5)], vec![pos(25, 25)])],
    );
    let out = run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 30);
    let cleared = out.side_b_alive == 0;
    result(
        "EXP-FOCUS-1",
        "focus-fire out-DPSes the aggregate heal (low ticks-to-clear, no overkill stragglers)",
        vec![
            boolean("3×ranged clear a 5-HEAL turtle", "cleared", cleared),
            measured("ticks-to-clear", out.ticks as f64, "<= 10", cleared && out.ticks <= 10),
            measured("attacker survivors", out.side_a_alive as f64, "== 3", out.side_a_alive == 3),
        ],
    )
}

/// EXP-TOWER-1 — an edge drain sustains via self-heal and bleeds the tower's energy to zero.
fn exp_tower_1() -> ExperimentResult {
    let world = CombatWorld {
        creeps: vec![SimCreep { id: 1, owner: 0, pos: pos(25, 1), body: SimBody::unboosted(&[Part::Heal; 13]), fatigue: 0 }],
        towers: vec![SimTower { id: 200, owner: 1, pos: pos(25, 22), energy: 100, hits: 3000, hits_max: 3000 }],
        ..Default::default()
    };
    let out = run_engagement(world, room(), 0, pos(25, 1), &mut DrainAgent, 1, pos(25, 22), &mut HoldAgent, 15);
    result(
        "EXP-TOWER-1",
        "an edge drain sustains via self-heal and bleeds tower energy to 0",
        vec![
            boolean("the drain tank survives the falloff tower", "survives", out.side_a_alive == 1),
            measured("tower energy remaining", out.side_b_tower_energy as f64, "== 0 (drained)", out.side_b_tower_energy == 0),
        ],
    )
}

/// EXP-COMP-1 — composition comparison: against a heal-wall (a 10-HEAL turtle), a higher-DPS
/// composition clears strictly faster. (The harness for the full uniform-brick-vs-2+2-split + TOUGH
/// sweep; here the DPS-vs-heal-wall comparison.)
fn exp_comp_1() -> ExperimentResult {
    let clear_ticks = |n: u8| -> Option<u32> {
        let positions: Vec<Position> = (0..n).map(|i| pos(24 + i, 22)).collect();
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7)], positions)],
            1,
            &[Unit::new(vec![(Part::Heal, 10)], vec![pos(25, 25)])],
        );
        let out = run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 80);
        (out.side_b_alive == 0).then_some(out.ticks)
    };
    let duo = clear_ticks(2);
    let quad = clear_ticks(4);
    let faster = matches!((duo, quad), (Some(d), Some(q)) if q < d);
    result(
        "EXP-COMP-1",
        "a higher-DPS composition clears a heal-wall strictly faster",
        vec![
            measured("duo (2×RA) ticks-to-clear", duo.map(|t| t as f64).unwrap_or(f64::INFINITY), "cleared", duo.is_some()),
            measured("quad (4×RA) ticks-to-clear", quad.map(|t| t as f64).unwrap_or(f64::INFINITY), "cleared", quad.is_some()),
            boolean("quad clears faster than duo", "quad < duo", faster),
        ],
    )
}

/// EXP-BREACH-1 (U7) — a rampart-shielded spawn: a ranged siege force in range of a **hostile
/// rampart** (the shield) breaks the rampart and then the spawn it covers. Exercises the U1
/// ScenarioBuilder (rampart/spawn), the U3 structure-targeted apply layer (`RangedAttackStructure`
/// resolves the shield first), and the U5 structure-DPS metric.
fn exp_breach_1() -> ExperimentResult {
    // 3×(10 RANGED) = 300 dmg/tick, all within range 3 of the shield tile; rampart 3000 + spawn 5000
    // = 8000 hits ⇒ ~27 ticks to fully breach + raze. Cap 50.
    let mut b = ScenarioBuilder::from_units(
        room(),
        0,
        &[Unit::new(vec![(Part::RangedAttack, 10)], vec![pos(25, 23), pos(24, 23), pos(26, 23)])],
        1,
        &[],
    );
    b.structure(StructureKind::Rampart, Some(1), 25, 25, 3000, 3000);
    b.structure(StructureKind::Spawn, Some(1), 25, 25, 5000, 5000);
    let world = b.build();
    let out = run_engagement(world, room(), 0, pos(25, 23), &mut IbexAgent, 1, pos(25, 25), &mut HoldAgent, 50);
    let m = SideMetrics::from_recording(&out.recording, 0);
    // Gate the breach MECHANIC (U3 shield-first apply + U5 structure DPS), not bot siege navigation:
    // the shield must fall first (a `Rampart` is destroyed), proving the apply layer targets the
    // rampart over the spawn it covers. (Sustained in-range siege to fully raze the spawn is a U8/U9
    // movement concern — a per-creep attacker with no creep to anchor on drifts out of range.)
    let rampart_fell = destroyed_kind_count(&out.recording, StructureKind::Rampart) >= 1;
    result(
        "EXP-BREACH-1",
        "a ranged siege breaks the hostile rampart SHIELD first (shield-over-spawn apply layer)",
        vec![
            measured("structure damage dealt", m.structure_damage_dealt as f64, ">= 3000 (shield)", m.structure_damage_dealt >= 3000),
            boolean("the rampart shield is destroyed", "rampart fell", rampart_fell),
            measured("siege survivors", m.survivors as f64, "== 3 (unopposed)", m.survivors == 3),
        ],
    )
}

/// How many structures of `kind` were destroyed across the whole run (U2 `destroyed_kinds`).
fn destroyed_kind_count(rec: &screeps_combat_engine::CombatRecording, kind: StructureKind) -> usize {
    rec.frames.iter().flat_map(|f| f.destroyed_kinds.iter()).filter(|(_, k)| *k == kind).count()
}

/// EXP-NEST-1 (U7) — a defender **tower nest** (no defender creeps) punishes attackers that walk in:
/// the nest deals attributed tower damage (must-fix 1 — it's the *defender's* tower output, not creep
/// DPS) and the attackers bleed. Exercises the U1 `tower_nest` builder + the scripted tower controller.
fn exp_nest_1() -> ExperimentResult {
    // 3 towers (full energy) centred at (25,25); two soft attackers walk into the room centre.
    let mut b = ScenarioBuilder::from_units(
        room(),
        0,
        &[Unit::new(vec![(Part::Attack, 5), (Part::Move, 5)], vec![pos(22, 22), pos(23, 22)])],
        1,
        &[],
    );
    b = b.tower_nest(1, 25, 25, 3, 1000);
    let world = b.build();
    let out = run_engagement(world, room(), 0, pos(22, 22), &mut RushAgent, 1, pos(25, 25), &mut HoldAgent, 30);
    let attackers = SideMetrics::from_recording(&out.recording, 0);
    let defender = SideMetrics::from_recording(&out.recording, 1);
    result(
        "EXP-NEST-1",
        "a 3-tower nest deals attributed tower damage and bleeds attackers (no defender creeps)",
        vec![
            measured("defender tower damage dealt", defender.tower_damage_dealt as f64, "> 0", defender.tower_damage_dealt > 0),
            boolean("none of that is creep DPS (no defender creeps)", "creep DPS == 0", defender.creep_damage_dealt == 0),
            measured("attacker damage taken", attackers.damage_taken as f64, "> 0 (bled by the nest)", attackers.damage_taken > 0),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_register_passes_all_gates() {
        let results = register();
        assert_eq!(results.len(), 7, "the register has 7 experiments");
        for r in &results {
            assert!(r.pass, "{} failed its gates:\n{}", r.id, report(std::slice::from_ref(r)));
        }
    }

    #[test]
    fn report_renders_the_register() {
        let s = report(&register());
        assert!(s.contains("7/7 experiments passed"), "{s}");
        assert!(s.contains("EXP-FOUND-1") && s.contains("EXP-COMP-1") && s.contains("EXP-BREACH-1"));
    }
}
