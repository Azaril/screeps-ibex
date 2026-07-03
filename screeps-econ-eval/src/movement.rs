//! The **ANALYTIC fast movement tier** (ADR 0040 §D7 reconciliation: the long-horizon default —
//! M1 spec Part C.2). Creeps do NOT path through the sim resolver each tick; each assignment gets
//! a **fatigue-exact single-creep trace** between its endpoints, computed once from rover-eval's
//! oracle machinery and memoized:
//!
//! - The ROUTE is [`screeps_rover_eval::oracle::optimal_path`] — rover's own
//!   `room_grid_dijkstra` over the engine fatigue field (road 1 / plain 2 / swamp 10). REUSE, no
//!   new pathfinding algorithms (the standing rule).
//! - The TICKS come from driving the shared kernel's `resolve_movement` along that route with the
//!   real body + carry weight — the [`screeps_rover_eval::traverse::traverse`] loop, mirrored
//!   here because we need the per-tick POSITION TRACE it doesn't expose (each tick's tile, so the
//!   runner can advance positions, book `ROAD_WEAROUT` per step entered, and fire the K3
//!   en-route repair events at real range-3 geometry). Kernel physics only — no re-ported math.
//!
//! Positions are advanced by TELEPORT along the trace (the runner sets `SimCreep::pos` directly);
//! **contention is ignored BY DESIGN** — creeps may transiently co-occupy tiles (congestion-
//! sensitive families use the rover tier later; [`Mover`] is the seam an M2+ `rover_driver` tier
//! swaps in behind). The route is the FATIGUE-optimal one walked by the actual body — the same
//! `r_ticks` approximation rover-eval's `t_star_rtt` documents.
//!
//! **Memoization**: per `(from, to, range, body-class, carry)`, where body-class =
//! `(move, carry, other)` alive part counts — exactly the inputs of the kernel's fatigue formula
//! (weight = other + loaded-carry; regen = 2×MOVE). Road DEATH changes the fatigue field, so the
//! runner calls [`AnalyticMover::invalidate`] whenever the world's road set shrinks (memo
//! generation bump; repairs/decay without death never move the field).

use screeps::{Part, Position};
use screeps_rover_eval::oracle::optimal_path;
use screeps_sim_core::{resolve_movement, MoveIntents, MovementState, SimBody, SimCreep, SimTerrain};
use std::collections::BTreeMap;
use std::rc::Rc;

/// Generous per-trace walk cap: the worst body (1 MOVE, all-swamp) pays ≤ ~50 ticks/step on a
/// ≤ ~150-tile in-room route. Hitting it means a mis-built scenario, surfaced as `None`.
const TRACE_TICK_CAP: u32 = 10_000;

/// The fatigue-relevant body class: (MOVE, CARRY, other) alive part counts — two bodies with the
/// same class and carry load walk identical traces (kernel fatigue formula).
pub type BodyClass = (u8, u8, u8);

pub fn body_class(body: &SimBody) -> BodyClass {
    let moves = body.alive_part_count(Part::Move) as u8;
    let carries = body.alive_part_count(Part::Carry) as u8;
    let other = (body.parts.len() as u8).saturating_sub(moves).saturating_sub(carries);
    (moves, carries, other)
}

type TraceKey = ((u8, u8), (u8, u8), u8, BodyClass, u32);

/// The movement-tier seam (M1 spec Part C.2): the runner consumes THIS trait, so an M2+ rover
/// tier (`rover_driver`, per-tick pathing with contention) swaps in behind it without touching
/// the policy or the runner.
pub trait Mover {
    /// The per-tick position trace from `from` to within `range` of `to` (positions AFTER each
    /// tick; empty = already in range). `None` = unreachable (a mis-built scenario, not policy).
    fn trace(&mut self, from: Position, to: Position, range: u8, body: &SimBody, carry: u32) -> Option<Rc<Vec<Position>>>;

    /// The movement field changed (road death; M2: construction) — re-price everything.
    fn invalidate_from(&mut self, terrain: &SimTerrain);

    /// Travel time in ticks (= trace length).
    fn travel_ticks(&mut self, from: Position, to: Position, range: u8, body: &SimBody, carry: u32) -> Option<u32> {
        self.trace(from, to, range, body, carry).map(|t| t.len() as u32)
    }
}

/// The analytic tier: memoized fatigue-exact traces over a terrain snapshot.
pub struct AnalyticMover {
    terrain: SimTerrain,
    traces: BTreeMap<TraceKey, Option<Rc<Vec<Position>>>>,
}

impl AnalyticMover {
    pub fn new(terrain: &SimTerrain) -> Self {
        AnalyticMover { terrain: interior_only(terrain), traces: BTreeMap::new() }
    }

    /// Refresh the terrain snapshot + drop every memo — call when the fatigue field changed
    /// (road death; M2: construction).
    pub fn invalidate(&mut self, terrain: &SimTerrain) {
        self.terrain = interior_only(terrain);
        self.traces.clear();
    }

    /// Walk `body` along `route` through the REAL kernel mover, one creep alone in the world,
    /// recording the position after every tick — the [`screeps_rover_eval::traverse::traverse`]
    /// loop with a trace (module docs). The kernel enforces fatigue; a fatigued tick records the
    /// unchanged position.
    fn walk_trace(&self, body: &SimBody, carry: u32, from: Position, route: &[Position]) -> Option<Vec<Position>> {
        let mut world = MovementState {
            terrain: self.terrain.clone(),
            creeps: vec![SimCreep { id: 1, owner: 0, pos: from, body: body.clone(), fatigue: 0, carry_used: carry }],
            ..Default::default()
        };
        let mut trace = Vec::new();
        let mut i = 0usize;
        let mut ticks = 0u32;
        while i < route.len() {
            if ticks >= TRACE_TICK_CAP {
                return None;
            }
            let before = world.creeps[0].pos;
            let mut intents = MoveIntents::new();
            if let Some(dir) = direction_between(before, route[i]) {
                intents.set_move(1, dir);
            }
            resolve_movement(&mut world, &intents);
            ticks += 1;
            let after = world.creeps[0].pos;
            trace.push(after);
            if after != before && after == route[i] {
                i += 1;
            }
        }
        Some(trace)
    }
}

impl Mover for AnalyticMover {
    fn invalidate_from(&mut self, terrain: &SimTerrain) {
        self.invalidate(terrain);
    }

    fn trace(&mut self, from: Position, to: Position, range: u8, body: &SimBody, carry: u32) -> Option<Rc<Vec<Position>>> {
        assert_eq!(from.room_name(), to.room_name(), "the analytic tier is single-room (Family C)");
        let key: TraceKey = (
            (from.x().u8(), from.y().u8()),
            (to.x().u8(), to.y().u8()),
            range,
            body_class(body),
            carry,
        );
        if let Some(hit) = self.traces.get(&key) {
            return hit.clone();
        }
        let computed = (|| {
            if from.get_range_to(to) <= range as u32 {
                return Some(Rc::new(Vec::new())); // already in range
            }
            let tiles = optimal_path(&self.terrain, from, to, range)?;
            let room = from.room_name();
            let route: Vec<Position> = tiles
                .iter()
                .map(|&(x, y)| {
                    Position::new(
                        screeps::RoomCoordinate::new(x).unwrap(),
                        screeps::RoomCoordinate::new(y).unwrap(),
                        room,
                    )
                })
                .collect();
            self.walk_trace(body, carry, from, &route).map(Rc::new)
        })();
        self.traces.insert(key, computed.clone());
        computed
    }
}

/// The analytic tier's pathing terrain: the room's INTERIOR only — every edge tile (x/y ∈
/// {0, 49}) prices impassable. Stepping onto an exit tile fires the kernel's cross-room
/// relocation (`resolve_movement`'s edge-exit rule), which a single-room Family-C trace must
/// never do; real captured layouts keep all endpoints interior, so this only forbids routes that
/// would GRAZE an exit (found by the full-catalog run on E13S29 — a border-hugging
/// fatigue-optimal route relocated the walker into E13S28).
fn interior_only(terrain: &SimTerrain) -> SimTerrain {
    let mut t = terrain.clone();
    for i in 0..=49u8 {
        t.walls.insert((i, 0));
        t.walls.insert((i, 49));
        t.walls.insert((0, i));
        t.walls.insert((49, i));
    }
    t
}

/// The `Direction` from `a` to an ADJACENT tile `b` (the traverse.rs helper, mirrored).
fn direction_between(a: Position, b: Position) -> Option<screeps::Direction> {
    use screeps::Direction::*;
    if a.room_name() != b.room_name() {
        return None;
    }
    let dx = b.x().u8() as i32 - a.x().u8() as i32;
    let dy = b.y().u8() as i32 - a.y().u8() as i32;
    Some(match (dx, dy) {
        (0, -1) => Top,
        (1, -1) => TopRight,
        (1, 0) => Right,
        (1, 1) => BottomRight,
        (0, 1) => Bottom,
        (-1, 1) => BottomLeft,
        (-1, 0) => Left,
        (-1, -1) => TopLeft,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::RoomName;

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(screeps::RoomCoordinate::new(x).unwrap(), screeps::RoomCoordinate::new(y).unwrap(), room)
    }

    /// A balanced empty body walks 1 tile/tick on plain; a loaded under-MOVE'd hauler pays
    /// fatigue — the trace is fatigue-exact (matches the kernel arithmetic, not distance).
    #[test]
    fn traces_are_fatigue_exact() {
        let mut m = AnalyticMover::new(&SimTerrain::default());
        let balanced = SimBody::unboosted(&[Part::Carry, Part::Carry, Part::Move, Part::Move]);
        let t = m.travel_ticks(pos(10, 25), pos(20, 25), 0, &balanced, 0).unwrap();
        assert_eq!(t, 10, "empty balanced hauler: 1 tile/tick over 10 tiles");

        // Loaded 2×CARRY / 1×MOVE: weight 2, plain accrual 4, regen 2 ⇒ every other tick stalls.
        let slow = SimBody::unboosted(&[Part::Carry, Part::Carry, Part::Move]);
        let t_loaded = m.travel_ticks(pos(10, 25), pos(20, 25), 0, &slow, 100).unwrap();
        let t_empty = m.travel_ticks(pos(10, 25), pos(20, 25), 0, &slow, 0).unwrap();
        assert_eq!(t_empty, 10, "empty it flies");
        assert!(t_loaded >= 19, "loaded it stalls (~2 ticks/tile): {t_loaded}");

        // The trace's tiles are per-tick positions: stalls repeat the tile.
        let trace = m.trace(pos(10, 25), pos(12, 25), 0, &slow, 100).unwrap();
        assert_eq!(trace.last().copied(), Some(pos(12, 25)), "ends at the goal");
        assert!(trace.len() > 2, "loaded trace includes fatigue-stall ticks");
    }

    /// Range goals stop short; in-range starts give the empty trace; memoization returns the
    /// same Rc; invalidate() re-prices a changed field.
    #[test]
    fn range_memo_and_invalidation() {
        let mut terrain = SimTerrain::default();
        for x in 10..=20 {
            terrain.roads.insert((x, 25));
        }
        let mut m = AnalyticMover::new(&terrain);
        let body = SimBody::unboosted(&[Part::Work, Part::Carry, Part::Move]);

        let t = m.trace(pos(10, 25), pos(20, 25), 1, &body, 0).unwrap();
        assert_eq!(t.last().map(|p| p.get_range_to(pos(20, 25))), Some(1), "stops at range 1");
        assert!(m.trace(pos(20, 25), pos(20, 26), 1, &body, 0).unwrap().is_empty(), "already in range");

        let again = m.trace(pos(10, 25), pos(20, 25), 1, &body, 0).unwrap();
        assert!(Rc::ptr_eq(&t, &again), "memo hit");

        // Kill the road: an under-MOVE'd body now pays plain fatigue — longer travel.
        let heavy = SimBody::unboosted(&[Part::Work, Part::Work, Part::Work, Part::Move]);
        let on_road = m.travel_ticks(pos(10, 25), pos(20, 25), 1, &heavy, 0).unwrap();
        let mut bare = terrain.clone();
        bare.roads.clear();
        m.invalidate(&bare);
        let off_road = m.travel_ticks(pos(10, 25), pos(20, 25), 1, &heavy, 0).unwrap();
        assert!(off_road > on_road, "road death re-prices travel ({off_road} > {on_road})");
    }
}
