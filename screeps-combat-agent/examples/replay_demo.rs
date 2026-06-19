//! Emit combat replay SVGs (P2.H4 demo): run our AI (`IbexAgent`) vs the scripted opponent roster
//! across the adversarial scenarios, capture each per-tick recording, and render it as an SVG
//! filmstrip (open the `.svg` and scrub left→right through ticks).
//!
//! `cargo run --example replay_demo -p screeps-combat-agent` → writes `target/replays/*.svg`.

use screeps::{Part, Position, RoomCoordinate};
use screeps_combat_agent::opponents::{run_engagement, world_from_units, DrainAgent, RushAgent, TurtleAgent, Unit};
use screeps_combat_agent::{replay, IbexAgent};
use screeps_combat_engine::{CombatWorld, SimBody, SimCreep, SimTower};

fn p(x: u8, y: u8) -> Position {
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), "W1N1".parse().unwrap())
}

fn main() {
    let room = "W1N1".parse().unwrap();
    let dir = std::path::Path::new("target/replays");
    std::fs::create_dir_all(dir).expect("mkdir");

    // 1) Ranged kiter vs a melee bruiser (MOVE parity): the kiter keeps its distance.
    {
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7), (Part::Move, 7)], vec![p(30, 25)])],
            1,
            &[Unit::new(vec![(Part::Attack, 10), (Part::Move, 10)], vec![p(27, 25)])],
        );
        let out = run_engagement(world, room, 0, p(30, 25), &mut IbexAgent, 1, p(27, 25), &mut RushAgent, 30);
        write(dir, "1_kiter_vs_rush", &out);
    }

    // 2) Three ranged attackers focus-fire a turtle healer (out-DPS the heal).
    {
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7)], vec![p(25, 22), p(24, 22), p(26, 22)])],
            1,
            &[Unit::new(vec![(Part::Heal, 5)], vec![p(25, 25)])],
        );
        let out = run_engagement(world, room, 0, p(25, 22), &mut IbexAgent, 1, p(25, 25), &mut TurtleAgent, 30);
        write(dir, "2_focus_fire_vs_turtle", &out);
    }

    // 3) A ranged QUAD vs a beefy (10-HEAL) turtle — a different composition.
    {
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7)], vec![p(24, 22), p(25, 22), p(26, 22), p(25, 21)])],
            1,
            &[Unit::new(vec![(Part::Heal, 10)], vec![p(25, 25)])],
        );
        let out = run_engagement(world, room, 0, p(25, 22), &mut IbexAgent, 1, p(25, 25), &mut TurtleAgent, 40);
        write(dir, "3_quad_vs_strong_turtle", &out);
    }

    // 4) A drain tank vs a tower: self-heals through falloff fire, bleeds the tower's energy.
    {
        let world = CombatWorld {
            creeps: vec![SimCreep { id: 1, owner: 0, pos: p(25, 1), body: SimBody::unboosted(&[Part::Heal; 13]), fatigue: 0 }],
            towers: vec![SimTower { id: 200, owner: 1, pos: p(25, 22), energy: 100, hits: 3000, hits_max: 3000 }],
            ..Default::default()
        };
        let out = run_engagement(world, room, 0, p(25, 1), &mut DrainAgent, 1, p(25, 22), &mut screeps_combat_agent::HoldAgent, 15);
        write(dir, "4_drain_vs_tower", &out);
    }
}

fn write(dir: &std::path::Path, name: &str, out: &screeps_combat_agent::opponents::EngagementOutcome) {
    let path = dir.join(format!("{name}.svg"));
    std::fs::write(&path, replay::to_svg(&out.recording)).expect("write svg");
    println!(
        "{:28} ticks={:2} a_alive={} b_alive={} cohesion_a={} tower_e={} -> {}",
        name, out.ticks, out.side_a_alive, out.side_b_alive, out.worst_cohesion_a, out.side_b_tower_energy,
        path.display()
    );
}
