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

use screeps::{LocalCostMatrix, Part, Position, RoomName, RoomXY};
use screeps_rover::{LocalPathfinder, PathfindingProvider};
use screeps_rover_eval::oracle::optimal_path;
use screeps_sim_core::{resolve_movement, MoveIntents, MovementState, SimBody, SimCreep, SimTerrain};
use std::collections::{BTreeMap, HashMap};
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

    /// The movement field changed (road death; M2: construction) — re-price everything. Takes the
    /// whole [`MovementState`] (default terrain + the per-room map) so a multi-room mover re-prices
    /// every room; the single-room `AnalyticMover` reads `.terrain` (ADR 0044 P2).
    fn invalidate_from(&mut self, movement: &MovementState);

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
    fn invalidate_from(&mut self, movement: &MovementState) {
        self.invalidate(&movement.terrain);
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

// ═════════════════════════════════════════════════════════════════════════════════════════════
// ADR 0044 P2 — the rover-backed MULTI-ROOM mover (Family R). Single-room families keep the
// analytic tier; this tier routes cross-room via the REAL `screeps_rover::LocalPathfinder`.
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Offline cost-matrix tile costs (headless has NO `Terrain` — the caller bakes walls/swamp/roads
/// into the matrix; `local_pathfinder.rs`). Mirrors the engine fatigue field (road 1 / plain 2 /
/// swamp 10); `search` ignores its `swamp_cost` arg, so swamp MUST be baked here.
const MATRIX_ROAD_COST: u8 = 1;
const MATRIX_PLAIN_COST: u8 = 2;
const MATRIX_SWAMP_COST: u8 = 10;
const MATRIX_IMPASSABLE: u8 = u8::MAX;
/// Per-search op cap for the multi-room A* — generous (this is the OFFLINE sim, not the live CPU
/// budget), so a long multi-room corridor route completes rather than best-efforting to `incomplete`.
const SEARCH_MAX_OPS: u32 = 2_000_000;

/// Bake a room's terrain into a `LocalCostMatrix` for `LocalPathfinder::search`: walls impassable,
/// swamp raised, roads lowered; plains left `0` (= `plain_cost`). Terrain-only — exactly the field
/// `optimal_path` prices, no structure blockers (parity with the analytic tier).
fn room_cost_matrix(terrain: &SimTerrain) -> LocalCostMatrix {
    let mut cm = LocalCostMatrix::new();
    let mut put = |x: u8, y: u8, v: u8| {
        if let Ok(xy) = RoomXY::checked_new(x, y) {
            cm.set(xy, v);
        }
    };
    // Order: swamp, then roads override swamp (a road on swamp walks at road speed), then walls.
    for &(x, y) in &terrain.swamps {
        put(x, y, MATRIX_SWAMP_COST);
    }
    for &(x, y) in &terrain.roads {
        put(x, y, MATRIX_ROAD_COST);
    }
    for &(x, y) in &terrain.walls {
        put(x, y, MATRIX_IMPASSABLE);
    }
    cm
}

/// The `Direction` stepping `a` → adjacent `b`, CROSS-ROOM aware (unlike [`direction_between`],
/// which is `None` across a border): tries the 8 offsets through `Position::checked_add`, which
/// crosses room boundaries in global space — so a step from an edge tile into the neighbour room
/// resolves to the border-crossing direction the kernel's edge-exit rule then applies.
fn step_direction(a: Position, b: Position) -> Option<screeps::Direction> {
    use screeps::Direction::*;
    const DIRS: [(screeps::Direction, (i32, i32)); 8] = [
        (Top, (0, -1)),
        (TopRight, (1, -1)),
        (Right, (1, 0)),
        (BottomRight, (1, 1)),
        (Bottom, (0, 1)),
        (BottomLeft, (-1, 1)),
        (Left, (-1, 0)),
        (TopLeft, (-1, -1)),
    ];
    DIRS.iter().find_map(|&(dir, off)| (a.checked_add(off).ok() == Some(b)).then_some(dir))
}

/// The multi-room memo key: full room-qualified `Position`s (the single-room `TraceKey` drops the
/// room).
type MultiTraceKey = (Position, Position, u8, BodyClass, u32);

/// The **rover-backed multi-room mover** (ADR 0044 P2): routes cross-room via the REAL
/// `screeps_rover::LocalPathfinder::search` (multi-room A*, offline/no-JS) over per-room cost
/// matrices baked from the world terrain, then walks the shared `resolve_movement` kernel along the
/// route (edges PASSABLE ⇒ edge-exit relocation carries the walker across borders) for the
/// fatigue-exact trace. Same [`Mover`] contract + memoization as [`AnalyticMover`]; used for Family
/// R (single-room families keep the analytic tier to avoid perturbing their baselines).
pub struct RoverMover {
    default_terrain: SimTerrain,
    rooms: HashMap<RoomName, SimTerrain>,
    matrices: HashMap<RoomName, LocalCostMatrix>,
    pathfinder: LocalPathfinder,
    traces: HashMap<MultiTraceKey, Option<Rc<Vec<Position>>>>,
}

impl RoverMover {
    pub fn new(movement: &MovementState) -> Self {
        RoverMover {
            default_terrain: movement.terrain.clone(),
            rooms: movement.rooms.clone(),
            matrices: HashMap::new(),
            pathfinder: LocalPathfinder,
            traces: HashMap::new(),
        }
    }

    /// Walk `body` along a (possibly cross-room) `route` through `resolve_movement`, recording the
    /// position after every tick — the multi-room analogue of [`AnalyticMover::walk_trace`]: the
    /// `MovementState` carries the full per-room terrain with edges PASSABLE, so the kernel's
    /// edge-exit relocation carries the walker across borders. A fatigued tick records the
    /// unchanged position.
    fn walk_trace_multi(&self, body: &SimBody, carry: u32, from: Position, route: &[Position]) -> Option<Vec<Position>> {
        let mut world = MovementState {
            terrain: self.default_terrain.clone(),
            rooms: self.rooms.clone(),
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
            if let Some(dir) = step_direction(before, route[i]) {
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

impl Mover for RoverMover {
    fn invalidate_from(&mut self, movement: &MovementState) {
        self.default_terrain = movement.terrain.clone();
        self.rooms = movement.rooms.clone();
        self.matrices.clear();
        self.traces.clear();
    }

    fn trace(&mut self, from: Position, to: Position, range: u8, body: &SimBody, carry: u32) -> Option<Rc<Vec<Position>>> {
        let key: MultiTraceKey = (from, to, range, body_class(body), carry);
        if let Some(hit) = self.traces.get(&key) {
            return hit.clone();
        }
        let computed = (|| {
            if from.get_range_to(to) <= range as u32 {
                return Some(Rc::new(Vec::new())); // already in range
            }
            // Multi-room A* over per-room baked matrices (rover's real search). Borrow-split so the
            // `room_callback` can lazily build+cache matrices while `pathfinder` is borrowed.
            let result = {
                let RoverMover { default_terrain, rooms, matrices, pathfinder, .. } = &mut *self;
                let mut callback = |room: RoomName| -> Option<LocalCostMatrix> {
                    // A room OUTSIDE the world (not in `rooms`) is IMPASSABLE (`None`) — otherwise the
                    // A* on open corridor rooms wanders into undefined neighbours (which the walk then
                    // can't follow across their corners). The single-room degenerate case (empty
                    // `rooms`) falls back to `default_terrain` for the one room the search stays in.
                    if rooms.is_empty() {
                        let m = matrices.entry(room).or_insert_with(|| room_cost_matrix(default_terrain));
                        return Some(m.clone());
                    }
                    let t = rooms.get(&room)?;
                    let m = matrices.entry(room).or_insert_with(|| room_cost_matrix(t));
                    Some(m.clone())
                };
                pathfinder.search(from, to, range as u32, &mut callback, SEARCH_MAX_OPS, MATRIX_PLAIN_COST, MATRIX_SWAMP_COST)
            };
            if result.incomplete {
                return None; // unreachable within the op budget
            }
            self.walk_trace_multi(body, carry, from, &result.path).map(Rc::new)
        })();
        self.traces.insert(key, computed.clone());
        computed
    }
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

    /// ADR 0044 P2: the rover mover on a SINGLE room (the degenerate multi-room case) reaches the
    /// goal at plains speed and memoizes — parity with the analytic tier's reachability.
    #[test]
    fn rover_mover_single_room_reaches_goal() {
        let movement = MovementState { terrain: SimTerrain::default(), ..Default::default() };
        let mut m = RoverMover::new(&movement);
        let balanced = SimBody::unboosted(&[Part::Carry, Part::Carry, Part::Move, Part::Move]);
        let t = m.travel_ticks(pos(10, 25), pos(20, 25), 0, &balanced, 0).unwrap();
        assert_eq!(t, 10, "single-room degenerate: 1 tile/tick over 10 open tiles");
        let trace = m.trace(pos(10, 25), pos(20, 25), 0, &balanced, 0).unwrap();
        assert_eq!(trace.last().copied(), Some(pos(20, 25)), "ends at the goal");
        let again = m.trace(pos(10, 25), pos(20, 25), 0, &balanced, 0).unwrap();
        assert!(Rc::ptr_eq(&trace, &again), "memo hit");
    }

    /// ADR 0044 P2: the rover mover routes ACROSS a room border — the whole point of the tier. Two
    /// adjacent open rooms; the trace crosses the east edge (kernel edge-exit relocation) and the
    /// TRUE distance is the routed tile count, not a single-room Chebyshev.
    #[test]
    fn rover_mover_crosses_room_border() {
        // The room one step east of home (W1N1), derived via `checked_add` (no hardcoded name).
        let east_room = pos(49, 25).checked_add((1, 0)).unwrap().room_name();
        let mut rooms = HashMap::new();
        rooms.insert(pos(0, 0).room_name(), SimTerrain::default()); // home must be a KNOWN world room
        rooms.insert(east_room, SimTerrain::default());
        let movement = MovementState { terrain: SimTerrain::default(), rooms, ..Default::default() };
        let mut m = RoverMover::new(&movement);
        let balanced = SimBody::unboosted(&[Part::Carry, Part::Carry, Part::Move, Part::Move]);
        let goal =
            Position::new(screeps::RoomCoordinate::new(10).unwrap(), screeps::RoomCoordinate::new(25).unwrap(), east_room);
        let trace = m.trace(pos(40, 25), goal, 0, &balanced, 0).expect("cross-room reachable");
        assert!(!trace.is_empty(), "cross-room trace is non-empty");
        assert_eq!(trace.last().map(|p| p.room_name()), Some(east_room), "ends in the east room");
        assert_eq!(trace.last().copied(), Some(goal), "reaches the goal tile");
        // ≈ (40→49 home = 9) + border + (0→10 east = 10) ≈ 20 tiles — the TRUE routed distance.
        let d = m.travel_ticks(pos(40, 25), goal, 0, &balanced, 0).unwrap();
        assert!((18..=24).contains(&d), "cross-room distance ~20 tiles: {d}");
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
