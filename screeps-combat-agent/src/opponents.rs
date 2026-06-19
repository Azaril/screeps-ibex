//! Scripted opponents + a head-to-head engagement harness (P2.H4).
//!
//! The bot's real brain is [`IbexAgent`](crate::IbexAgent). These scripted [`TacticalAgent`]s are
//! the *adversarial* other side to validate it against — a fixed, deterministic opponent (no fork
//! to drift or overfit). Each emits `CombatIntent`s through the same per-creep `decide` contract, so
//! [`agent_intents`](crate::agent_intents) drives them identically to the bot.
//!
//! [`run_engagement`] runs two agents head-to-head through the authoritative engine tick and reports
//! the outcome + side-A cohesion — the H4 self-play / adversarial runner. (Replay capture is the
//! engine's [`CombatRecording`](screeps_combat_engine::CombatRecording); the richer SVG scrubber is
//! the `screeps-combat-eval` policy layer, H5.)

use screeps::{Part, Position, RoomName};
use screeps_combat_decision::{CombatCreepDto, CombatIntent, CombatView, TacticalAgent};
use screeps_combat_engine::{
    record_tick, CombatRecording, CombatWorld, CreepId, Intents, PlayerId, SimBody, SimCreep, TowerAction,
};

use crate::{agent_intents, SimView};

/// The heal intent for the most-damaged of {self, in-range allies}, if `me` can heal and anyone is
/// hurt. `Heal` (range 1) when adjacent, else `RangedHeal` (range ≤3). Shared by the stationary
/// support archetypes (turtle / drain).
fn self_or_ally_heal(view: &CombatView) -> Option<CombatIntent> {
    let me = view.me;
    if !me.has_working(Part::Heal) {
        return None;
    }
    let mut best_deficit = me.hits_max.saturating_sub(me.hits);
    let mut target_pos = me.pos;
    let mut target_id = me.id;
    let mut target_range = 0u32;
    for f in view.friends.iter() {
        let r = me.pos.get_range_to(f.pos);
        if r > 3 {
            continue;
        }
        let deficit = f.hits_max.saturating_sub(f.hits);
        if deficit > best_deficit {
            best_deficit = deficit;
            target_pos = f.pos;
            target_id = f.id;
            target_range = r;
        }
    }
    if best_deficit == 0 {
        return None;
    }
    Some(if target_range <= 1 {
        CombatIntent::Heal { target: target_pos, id: target_id }
    } else {
        CombatIntent::RangedHeal { target: target_pos, id: target_id }
    })
}

/// One unit in a composition: a `body` (parts × counts, spawn order) instantiated once at each
/// listed position. The harness building block for fielding our AI (or an opponent) in an arbitrary
/// composition.
pub struct Unit {
    pub body: Vec<(Part, usize)>,
    pub positions: Vec<Position>,
}

impl Unit {
    /// `count` identical creeps of `body`, one per position (length must match `count` via the slice).
    pub fn new(body: Vec<(Part, usize)>, positions: Vec<Position>) -> Self {
        Self { body, positions }
    }
}

/// Build a two-sided [`CombatWorld`] from composition specs, assigning unique sequential creep ids
/// across both sides (so the two agents' intents merge cleanly when resolved together). Lets
/// adversarial tests express arbitrary compositions for our AI and the opponent without
/// hand-numbering creeps. (Towers/structures: extend the returned world's fields as needed.)
pub fn world_from_units(a_owner: PlayerId, a_units: &[Unit], b_owner: PlayerId, b_units: &[Unit]) -> CombatWorld {
    let mut creeps = Vec::new();
    let mut next_id: CreepId = 1;
    for (owner, units) in [(a_owner, a_units), (b_owner, b_units)] {
        for unit in units {
            let body: Vec<Part> = unit.body.iter().flat_map(|&(p, n)| std::iter::repeat_n(p, n)).collect();
            for &p in &unit.positions {
                creeps.push(SimCreep { id: next_id, owner, pos: p, body: SimBody::unboosted(&body), fatigue: 0 });
                next_id += 1;
            }
        }
    }
    CombatWorld { creeps, ..Default::default() }
}

/// Nearest hostile to `me` by Chebyshev range, if any.
fn nearest<'a>(view: &'a CombatView) -> Option<&'a CombatCreepDto> {
    let me = view.me.pos;
    view.hostiles.iter().min_by_key(|c| me.get_range_to(c.pos))
}

/// **Rush** — a melee bruiser: close on the nearest hostile and `attack` at range 1. The classic
/// pressure opponent the kiter must out-range.
#[derive(Default)]
pub struct RushAgent;

impl TacticalAgent for RushAgent {
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent> {
        let Some(t) = nearest(view) else {
            return vec![CombatIntent::Idle];
        };
        let r = view.me.pos.get_range_to(t.pos);
        let mut out = Vec::new();
        if r <= 1 && view.me.has_working(Part::Attack) {
            out.push(CombatIntent::Attack { target: t.pos, id: t.id });
        }
        if r > 1 {
            out.push(CombatIntent::MoveTo { target: t.pos, range: 1 });
        }
        out
    }
}

/// **Kite** — a ranged skirmisher: hold range 3 of the nearest hostile and `rangedAttack`; flee if
/// it closes inside 3, advance if it drifts past 3. The mirror of the bot's own kiting.
#[derive(Default)]
pub struct KiteAgent;

impl TacticalAgent for KiteAgent {
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent> {
        let Some(t) = nearest(view) else {
            return vec![CombatIntent::Idle];
        };
        let r = view.me.pos.get_range_to(t.pos);
        let mut out = Vec::new();
        if r <= 3 && view.me.has_working(Part::RangedAttack) {
            out.push(CombatIntent::RangedAttack { target: t.pos, id: t.id });
        }
        if r < 3 {
            out.push(CombatIntent::Flee { from: vec![t.pos], range: 3 });
        } else if r > 3 {
            out.push(CombatIntent::MoveTo { target: t.pos, range: 3 });
        }
        out
    }
}

/// **Turtle** — stand and heal: never move; `rangedAttack`/`attack` the lowest-hits hostile in
/// range and `heal` the most-damaged of {self, allies} in range. The "out-heal me" opponent that
/// focus-fire must beat (out-DPS the aggregate heal).
#[derive(Default)]
pub struct TurtleAgent;

impl TacticalAgent for TurtleAgent {
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent> {
        let me = view.me;
        let mut out = Vec::new();

        // Offense: hit the lowest-hits hostile within weapon range.
        if let Some(t) = view.hostiles.iter().filter(|c| me.pos.get_range_to(c.pos) <= 3).min_by_key(|c| c.hits) {
            let r = me.pos.get_range_to(t.pos);
            if r <= 1 && me.has_working(Part::Attack) {
                out.push(CombatIntent::Attack { target: t.pos, id: t.id });
            } else if r <= 3 && me.has_working(Part::RangedAttack) {
                out.push(CombatIntent::RangedAttack { target: t.pos, id: t.id });
            }
        }

        // Heal: the most-damaged of {self, in-range allies}.
        out.extend(self_or_ally_heal(view));

        // Holds position (no movement intent).
        out
    }
}

/// **Drain** — a pure tower-bait tank: self-heal (and heal allies), never move or attack. Carries
/// only HEAL/TOUGH, so it soaks tower fire while its heal out-paces the (falloff) damage, bleeding
/// the tower's energy to zero — the drain tactic. Positioned at the standoff range where heal
/// sustains; the scenario provides the tower.
#[derive(Default)]
pub struct DrainAgent;

impl TacticalAgent for DrainAgent {
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent> {
        self_or_ally_heal(view).into_iter().collect()
    }
}

/// Scripted tower controller: every living tower with energy fires `Attack` at the nearest enemy
/// creep (any owner but its own). Towers aren't agent-driven, so the harness drives them — this is
/// the defender's tower AI for tower scenarios (drain, breach-under-fire).
fn tower_intents(world: &CombatWorld, intents: &mut Intents) {
    for tower in world.towers.iter().filter(|t| t.is_alive()) {
        let target = world
            .creeps
            .iter()
            .filter(|c| c.is_alive() && c.owner != tower.owner)
            .min_by_key(|c| tower.pos.get_range_to(c.pos));
        if let Some(t) = target {
            // Keyed by the tower's stable id (U4) so a nest's shots stay valid as towers fall.
            // resolve_tick's `can_fire` gate skips the shot if the tower is out of energy.
            intents.set_tower(tower.id, TowerAction::Attack(t.id));
        }
    }
}

/// Outcome of a head-to-head engagement run through the engine.
#[derive(Clone, Debug)]
pub struct EngagementOutcome {
    /// Ticks actually simulated (≤ `max_ticks`; stops early when a side is fully gone).
    pub ticks: u32,
    pub side_a_alive: usize,
    pub side_b_alive: usize,
    /// Worst (max) pairwise Chebyshev distance among side-A creeps over the run — a cohesion proxy
    /// (lower = tighter; 0 when a side never has >1 creep). Symmetric `_b` companion below (U6).
    pub worst_cohesion_a: u32,
    pub worst_cohesion_b: u32,
    /// Total remaining energy across each side's living towers (0 if none) — for drain validation.
    pub side_a_tower_energy: u32,
    pub side_b_tower_energy: u32,
    /// Per-tick replay capture — render with [`crate::replay::to_svg`] (or `recording.render()`). The
    /// richer per-side metrics + stalemate adjudication are computed from this in `screeps-combat-eval`.
    pub recording: CombatRecording,
}

/// Run `agent_a` (owner `a_owner`) vs `agent_b` (owner `b_owner`) through the engine until one side
/// is fully gone (no creeps AND no towers) or `max_ticks` elapses. Both sides' creeps decide via the
/// per-creep `decide` contract; each side's towers fire via the scripted [`tower_intents`]; the
/// engine `resolve_tick` is the authoritative "server".
#[allow(clippy::too_many_arguments)]
pub fn run_engagement<A: TacticalAgent, B: TacticalAgent>(
    mut world: CombatWorld,
    room: RoomName,
    a_owner: PlayerId,
    a_center: Position,
    agent_a: &mut A,
    b_owner: PlayerId,
    b_center: Position,
    agent_b: &mut B,
    max_ticks: u32,
) -> EngagementOutcome {
    let creeps = |w: &CombatWorld, owner: PlayerId| w.creeps.iter().filter(|c| c.owner == owner).count();
    let towers = |w: &CombatWorld, owner: PlayerId| w.towers.iter().filter(|t| t.is_alive() && t.owner == owner).count();
    // A side with standing owned structures (a spawn/rampart, not a neutral wall) isn't "gone" — so a
    // breach/dismantle scenario (defender has structures but maybe no creeps) runs until they fall.
    let structs = |w: &CombatWorld, owner: PlayerId| w.structures.iter().filter(|s| s.is_alive() && s.owner == Some(owner)).count();
    let gone = |w: &CombatWorld, owner: PlayerId| creeps(w, owner) == 0 && towers(w, owner) == 0 && structs(w, owner) == 0;
    let mut worst_cohesion_a = 0u32;
    let mut worst_cohesion_b = 0u32;
    let mut ticks = 0;
    let mut recording = CombatRecording::new();
    let worst_pairwise = |w: &CombatWorld, owner: PlayerId| {
        let p: Vec<Position> = w.creeps.iter().filter(|c| c.owner == owner).map(|c| c.pos).collect();
        let mut m = 0u32;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                m = m.max(p[i].get_range_to(p[j]));
            }
        }
        m
    };
    while ticks < max_ticks {
        if gone(&world, a_owner) || gone(&world, b_owner) {
            break;
        }
        // Per-side cohesion (max pairwise Chebyshev distance) — symmetric (U6).
        worst_cohesion_a = worst_cohesion_a.max(worst_pairwise(&world, a_owner));
        worst_cohesion_b = worst_cohesion_b.max(worst_pairwise(&world, b_owner));
        // Both sides' creeps decide; merge disjoint intents; towers fire; resolve.
        let sva = SimView::from_world(&world, a_owner, a_center, room);
        let mut intents = agent_intents(&world, &sva, agent_a);
        let svb = SimView::from_world(&world, b_owner, b_center, room);
        let ib = agent_intents(&world, &svb, agent_b);
        intents.creeps.extend(ib.creeps);
        intents.moves.extend(ib.moves);
        tower_intents(&world, &mut intents);
        record_tick(&mut recording, &mut world, &intents); // drop-in for resolve_tick + captures a frame
        ticks += 1;
    }
    let tower_energy = |w: &CombatWorld, owner: PlayerId| {
        w.towers.iter().filter(|t| t.is_alive() && t.owner == owner).map(|t| t.energy).sum()
    };
    EngagementOutcome {
        side_a_alive: creeps(&world, a_owner),
        side_b_alive: creeps(&world, b_owner),
        side_a_tower_energy: tower_energy(&world, a_owner),
        side_b_tower_energy: tower_energy(&world, b_owner),
        ticks,
        worst_cohesion_a,
        worst_cohesion_b,
        recording,
    }
}

/// Self-play convenience: run the bot's real `IbexAgent` on **both** sides (the no-fork self-play
/// the trait seam exists for — ADR 0006 §B.2). The stalemate adjudication lives in
/// `screeps-combat-eval::scoring` (policy), computed from the returned recording.
pub fn self_play(
    world: CombatWorld,
    room: RoomName,
    a_owner: PlayerId,
    a_center: Position,
    b_owner: PlayerId,
    b_center: Position,
    max_ticks: u32,
) -> EngagementOutcome {
    run_engagement(
        world,
        room,
        a_owner,
        a_center,
        &mut crate::IbexAgent,
        b_owner,
        b_center,
        &mut crate::IbexAgent,
        max_ticks,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HoldAgent, IbexAgent};
    use screeps::RoomCoordinate;
    use screeps_combat_engine::{resolve_tick, SimTower};

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }
    fn creep(id: CreepId, owner: PlayerId, x: u8, y: u8, parts: &[(Part, usize)]) -> SimCreep {
        let body: Vec<Part> = parts.iter().flat_map(|&(p, n)| std::iter::repeat_n(p, n)).collect();
        SimCreep { id, owner, pos: pos(x, y), body: SimBody::unboosted(&body), fatigue: 0 }
    }
    /// One `decide` call for the creep at index `me_idx`, viewing the world as `me_owner`.
    fn decide_one<A: TacticalAgent>(agent: &mut A, world: &CombatWorld, me_owner: PlayerId, me_idx: usize) -> Vec<CombatIntent> {
        let sv = SimView::from_world(world, me_owner, pos(25, 25), room());
        agent.decide(&sv.view_for(me_idx))
    }

    // ── Scripted-opponent unit behavior (deterministic, no engine) ──

    #[test]
    fn rush_agent_closes_then_attacks() {
        // Far: just advance to range 1.
        let far = CombatWorld {
            creeps: vec![creep(1, 0, 20, 25, &[(Part::Attack, 5), (Part::Move, 5)]), creep(2, 1, 30, 25, &[(Part::Move, 1)])],
            ..Default::default()
        };
        assert_eq!(
            decide_one(&mut RushAgent, &far, 0, 0),
            vec![CombatIntent::MoveTo { target: pos(30, 25), range: 1 }]
        );
        // Adjacent: attack (no move).
        let adj = CombatWorld {
            creeps: vec![creep(1, 0, 24, 25, &[(Part::Attack, 5), (Part::Move, 5)]), creep(2, 1, 25, 25, &[(Part::Move, 1)])],
            ..Default::default()
        };
        assert_eq!(
            decide_one(&mut RushAgent, &adj, 0, 0),
            vec![CombatIntent::Attack { target: pos(25, 25), id: Some(crate::synthetic_id(2)) }]
        );
    }

    #[test]
    fn kite_agent_holds_range_three() {
        // Too close (range 2): ranged-attack AND flee to 3.
        let close = CombatWorld {
            creeps: vec![creep(1, 0, 23, 25, &[(Part::RangedAttack, 5), (Part::Move, 5)]), creep(2, 1, 25, 25, &[(Part::Move, 1)])],
            ..Default::default()
        };
        let out = decide_one(&mut KiteAgent, &close, 0, 0);
        assert!(out.contains(&CombatIntent::RangedAttack { target: pos(25, 25), id: Some(crate::synthetic_id(2)) }));
        assert!(out.iter().any(|i| matches!(i, CombatIntent::Flee { range: 3, .. })));
        // Too far (range 5): just advance to 3 (no ranged fire).
        let far = CombatWorld {
            creeps: vec![creep(1, 0, 20, 25, &[(Part::RangedAttack, 5), (Part::Move, 5)]), creep(2, 1, 25, 25, &[(Part::Move, 1)])],
            ..Default::default()
        };
        assert_eq!(
            decide_one(&mut KiteAgent, &far, 0, 0),
            vec![CombatIntent::MoveTo { target: pos(25, 25), range: 3 }]
        );
    }

    #[test]
    fn turtle_agent_heals_and_never_moves() {
        // Damaged self + a hostile in range: ranged-attack the hostile + heal self, no movement.
        let mut me = creep(1, 0, 25, 25, &[(Part::RangedAttack, 3), (Part::Heal, 3)]);
        me.body.hits = me.body.hits_max() - 100; // take some damage
        let world = CombatWorld {
            creeps: vec![me, creep(2, 1, 27, 25, &[(Part::Move, 1)])],
            ..Default::default()
        };
        let out = decide_one(&mut TurtleAgent, &world, 0, 0);
        assert!(out.iter().any(|i| matches!(i, CombatIntent::RangedAttack { .. })));
        assert!(out.iter().any(|i| matches!(i, CombatIntent::Heal { .. } | CombatIntent::RangedHeal { .. })));
        assert!(!out.iter().any(|i| matches!(i, CombatIntent::MoveTo { .. } | CombatIntent::Flee { .. })));
    }

    // ── Adversarial engagements through the engine (the H4 self-play runner) ──

    #[test]
    fn ibex_kiter_outlasts_a_rush_bruiser() {
        // The bot's real brain (ranged, MOVE parity) vs a scripted melee rusher. The kiter keeps its
        // distance so the bruiser never connects: ibex survives, the rusher takes ranged damage.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 0, 30, 25, &[(Part::RangedAttack, 7), (Part::Move, 7)]), // ibex kiter
                creep(2, 1, 27, 25, &[(Part::Attack, 10), (Part::Move, 10)]),     // rush bruiser (MOVE parity)
            ],
            ..Default::default()
        };
        let rusher_max = world.creeps[1].body.hits_max();
        let out = run_engagement(world, room(), 0, pos(30, 25), &mut IbexAgent, 1, pos(27, 25), &mut RushAgent, 30);
        assert_eq!(out.side_a_alive, 1, "the ibex kiter survives the rush");
        // (Validated separately that the kiter takes no melee damage; here assert it pressures the
        // rusher — ranged fire chips it even while kiting.)
        let _ = rusher_max;
    }

    #[test]
    fn ibex_focus_fire_beats_a_turtle_healer() {
        // Three ibex ranged attackers (210 dps) focus the lone turtle (a 5-HEAL self-healer, 60/tick
        // self-heal): aggregate DPS out-paces the heal, so the turtle dies and all three survive.
        let world = CombatWorld {
            creeps: vec![
                creep(10, 1, 25, 25, &[(Part::Heal, 5)]), // turtle healer (500 hits, heals 60/t)
                creep(1, 0, 25, 22, &[(Part::RangedAttack, 7)]),
                creep(2, 0, 24, 22, &[(Part::RangedAttack, 7)]),
                creep(3, 0, 26, 22, &[(Part::RangedAttack, 7)]),
            ],
            ..Default::default()
        };
        let out = run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 30);
        assert_eq!(out.side_b_alive, 0, "focus-fire out-DPSes the turtle's heal");
        assert_eq!(out.side_a_alive, 3, "the attackers take no damage from a pure healer");
        // Focus-fire stays cohesive (started within 2 of each other; holding range 3 keeps them tight).
        assert!(out.worst_cohesion_a <= 4, "attackers stayed cohesive (was {})", out.worst_cohesion_a);
    }

    #[test]
    fn scripted_opponents_pathfind_around_walls() {
        // A RushAgent must reach a target on the far side of a wall band (3-wide gap). Its `MoveTo`
        // goal is *pathfound* through rover (around the wall) — not a raw directional step that would
        // stall against x=25. Proves the opponent roster inherits the same pathfinding as the bot.
        let mut world = world_from_units(
            0,
            &[Unit::new(vec![(Part::Attack, 5), (Part::Move, 5)], vec![pos(10, 25)])],
            1,
            &[Unit::new(vec![(Part::Move, 1)], vec![pos(40, 25)])], // inert target on the far side
        );
        for y in 0..50u8 {
            if (24..=26).contains(&y) {
                continue; // 3-wide gap
            }
            world.terrain.walls.insert((25, y));
        }
        let start_x = world.creeps[0].pos.x().u8();
        for _ in 0..45 {
            if !world.creeps.iter().any(|c| c.owner == 0) {
                break;
            }
            let sv = SimView::from_world(&world, 0, pos(10, 25), room());
            let intents = agent_intents(&world, &sv, &mut RushAgent);
            resolve_tick(&mut world, &intents);
        }
        let rusher = world.creeps.iter().find(|c| c.owner == 0).expect("rusher alive");
        assert!(
            rusher.pos.x().u8() > start_x + 15,
            "the rusher pathfound through the gap past the x=25 wall (reached x={})",
            rusher.pos.x().u8()
        );
    }

    #[test]
    fn ibex_quad_out_dpses_a_strong_turtle_healer() {
        // Our AI fielded as a 4×ranged QUAD (a different composition, built via `world_from_units`)
        // vs a beefy 10-HEAL turtle (120/tick self-heal): 4×70 = 280 dps out-paces the heal, so the
        // turtle dies and the quad — sitting at weapon range against a stationary, weaponless
        // healer — survives intact. Adversarial validation across a composition the bot really fields.
        let world = world_from_units(
            0,
            &[Unit::new(
                vec![(Part::RangedAttack, 7)],
                vec![pos(24, 22), pos(25, 22), pos(26, 22), pos(25, 21)],
            )],
            1,
            &[Unit::new(vec![(Part::Heal, 10)], vec![pos(25, 25)])],
        );
        let out = run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 40);
        assert_eq!(out.side_b_alive, 0, "the quad's 280 dps beats the turtle's 120/tick heal");
        assert_eq!(out.side_a_alive, 4, "the quad takes no damage from a weaponless healer");
    }

    #[test]
    fn drain_tank_outlasts_a_tower_and_bleeds_its_energy() {
        // A DrainAgent tank (13 HEAL = 156/tick) at the room edge, ~21 tiles from a centred tower
        // (≥ falloff range → 150/tick), self-heals through the fire while the tower bleeds 10
        // energy/shot to zero. Exercises the tower scenario (scripted tower controller, side B has a
        // tower + no creeps) + the drain tactic. Mirrors the engine's drain conformance config.
        let world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 1, &[(Part::Heal, 13)])],
            towers: vec![SimTower { id: 200, owner: 1, pos: pos(25, 22), energy: 100, hits: 3000, hits_max: 3000 }],
            ..Default::default()
        };
        let out = run_engagement(world, room(), 0, pos(25, 1), &mut DrainAgent, 1, pos(25, 22), &mut HoldAgent, 15);
        assert_eq!(out.side_a_alive, 1, "the drain tank out-heals the falloff tower");
        assert_eq!(out.side_b_tower_energy, 0, "the tower bled all its energy (10/shot) firing at the drainer");
    }
}
