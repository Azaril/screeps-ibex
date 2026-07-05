//! The in-crate determinism fence (ADR 0040 §D7 — SHIP-BLOCKER, green from M0's first commit) +
//! the randomized-intent conservation fuzz.
//!
//! - `econ_engine_is_deterministic`: a fixture colony (2 big + 1 recurrently-exhausted source,
//!   2 spawns, 4 extensions, 2 collision containers, 13 scripted creeps) runs harvest/haul/spawn
//!   loops for 600 ticks, 5×; the end-state digest AND the per-tick report digest must be
//!   identical across runs (spread 0). The fixture DELIBERATELY keeps order-sensitive contention
//!   binding on many ticks — same-store read/write collisions (the pumper pair), source
//!   exhaustion under two harvesters, cross-spawn energy contention, and a two-pullers-one-target
//!   conflict — and the instrumented arm asserts floors on each so the coverage cannot silently
//!   evaporate if the script's timing drifts. **M1 coverage:** a road corridor under the hauler's
//!   outbound legs (ROAD_WEAROUT clock pulls), a decayed road the WORKER drive-by repairs (the
//!   Pipeline-A harvest-masking repair), a short-fuse plain road (a decay event), a short-fuse
//!   swamp road that DIES mid-run (road removal + terrain reversion), and a doomed cargo-bearing
//!   container (container death + store drop) — each with its own anti-vacuity floor.
//! - `det_reorder`: the same script with every tick's action list permuted (reversed) digests
//!   identically. Insertion order is non-semantic for creep actions and cross-spawn requests (the
//!   resolver re-orders by creep id / spawn index); the two documented within-actor
//!   first-submitted-wins lanes (per-creep duplicate pipeline actions, same-spawn duplicate
//!   requests) are not emitted by the script — they are pinned by unit tests instead
//!   (`pipeline_a_d_one_intent_each`, `spawn_request_order_is_deterministic_under_contention`).
//! - `conservation_fuzz_500_ticks`: seeded RNG (sim-core `rng::Rng` — no ambient entropy)
//!   generates random-but-well-formed intents (including deliberately invalid ones) for 500
//!   ticks; the exact conservation audit must hold EVERY tick, and every creep's movement weight
//!   must equal its store total.

use screeps::{Direction, Part, Position, RoomCoordinate, RoomName};
use screeps_econ_engine::{
    resolve_econ_tick, EconAction, EconIntents, EconTickReport, EconWorld, SimResource, StructRef,
    StructureKind,
};
use screeps_sim_core::rng::Rng;
use screeps_sim_core::CreepId;

fn pos(x: u8, y: u8) -> Position {
    let room: RoomName = "W1N1".parse().unwrap();
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
}

fn dir_toward(from: Position, to: Position) -> Option<Direction> {
    let dx = (to.x().u8() as i32 - from.x().u8() as i32).signum();
    let dy = (to.y().u8() as i32 - from.y().u8() as i32).signum();
    Some(match (dx, dy) {
        (0, -1) => Direction::Top,
        (1, -1) => Direction::TopRight,
        (1, 0) => Direction::Right,
        (1, 1) => Direction::BottomRight,
        (0, 1) => Direction::Bottom,
        (-1, 1) => Direction::BottomLeft,
        (-1, 0) => Direction::Left,
        (-1, -1) => Direction::TopLeft,
        _ => return None,
    })
}

/// FNV-1a over the report's decision-relevant fields (the fence covers reports, not just state).
fn fold_report(mut h: u64, r: &EconTickReport) -> u64 {
    let mut eat = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    eat(r.tick as u64);
    eat(r.ledger.harvested);
    eat(r.ledger.spawn_self_charge);
    eat(r.ledger.spawn_bodies);
    eat(r.ledger.repair_roads);
    eat(r.ledger.repair_containers);
    eat(r.ledger.repair_other);
    eat(r.repair_leak.roads);
    eat(r.repair_leak.containers);
    eat(r.repair_leak.other);
    for (res, v) in &r.ledger.dropped_decay {
        eat(*res as u64);
        eat(*v);
    }
    // M6 mineral/lab ledger lanes (the fence pins them once the M6 fixture exercises them).
    for m in [&r.ledger.harvested_mineral, &r.ledger.reaction_produced, &r.ledger.reaction_consumed, &r.ledger.boost_mineral, &r.ledger.sold_mineral] {
        for (res, v) in m {
            eat(*res as u64);
            eat(*v);
        }
    }
    eat(r.ledger.boost_energy);
    eat(r.ledger.sold_energy_credit);
    for (idx, id) in &r.spawns_started {
        eat(*idx as u64);
        eat(*id as u64);
    }
    for id in &r.births {
        eat(*id as u64);
    }
    for id in &r.deaths {
        eat(*id as u64);
    }
    eat(r.rejected_actions as u64);
    eat(r.conservation.len() as u64);
    // M2: the controller/build lanes are part of the pinned history.
    eat(r.ledger.upgrade);
    eat(r.ledger.build);
    for l in &r.level_ups {
        eat(*l as u64);
    }
    for l in &r.downgrades {
        eat(*l as u64);
    }
    for (kind, p) in &r.sites_completed {
        eat(*kind as u64);
        eat(p.x().u8() as u64);
        eat(p.y().u8() as u64);
    }
    if let Some((level, progress, clock)) = r.controller {
        eat(level as u64);
        eat(progress as u64);
        eat(clock as u64);
    }
    h
}

// ── The fixture colony + its scripted (state-derived, insertion-order-free) drivers ─────────────

const HARVESTER_A: CreepId = 1;
const HARVESTER_B: CreepId = 2;
const HAULER_A: CreepId = 3;
const HAULER_B: CreepId = 4;
const WORKER: CreepId = 5;
const SHUTTLER: CreepId = 6;
/// The collision pair: X pumps container 0 → container 1; Y pumps container 1 → container 0.
/// Their same-tick read+write on ONE container binds nearly every tick — the recurring
/// same-store mixed collision the fence must cover.
const PUMP_X: CreepId = 7;
const PUMP_Y: CreepId = 8;
/// Two harvesters on the small (cap-300) source: combined demand outstrips the pool every regen
/// window, so who gets the last energy is a recurring cross-creep ordering decision.
const SHORT_HARV_A: CreepId = 9;
const SHORT_HARV_B: CreepId = 10;
/// The pull-conflict trio: two pullers drag ONE no-MOVE target every tick (the sim-core
/// lowest-puller-id rule under sustained conflict).
const PULLER_LO: CreepId = 11;
const PULLER_HI: CreepId = 12;
const PULL_TARGET: CreepId = 13;
/// Ids at and above this are spawned newborns (dispersing to id-derived posts).
const FIRST_NEWBORN: CreepId = 14;

/// The WORKER's drive-by repair target: a half-dead road within its repair range 3.
const REPAIR_ROAD_TILE: (u8, u8) = (12, 22);
/// A road under HAULER_A's outbound corridor — the wearout floor's probe tile.
const WEAR_ROAD_TILE: (u8, u8) = (20, 25);
/// The short-fuse swamp road that must DIE mid-run (decay 500/event on swamp).
const DOOMED_ROAD_TILE: (u8, u8) = (35, 35);
/// The doomed cargo container (5000 hits = one decay event; unowned window 100).
const DOOMED_CONTAINER_TILE: (u8, u8) = (36, 36);

fn fixture() -> EconWorld {
    let mut w = EconWorld::default();
    w.add_source(pos(10, 20), 3000); // idx 0
    w.add_source(pos(40, 20), 3000); // idx 1
    w.add_source(pos(15, 35), 300); // idx 2 — the small, recurrently exhausted pool
    w.add_spawn(pos(25, 25)); // idx 0, full 300
    w.add_spawn(pos(40, 40)); // idx 1 — outside the newborn post field
    for p in [pos(24, 24), pos(26, 24), pos(24, 26), pos(26, 26)] {
        w.add_extension(p, 8);
    }
    let c0 = w.add_container(pos(12, 12), 60, 250_000); // pumper loop container 0
    w.containers[c0].store.add(SimResource::Energy, 40);
    let c1 = w.add_container(pos(13, 13), 60, 250_000); // pumper loop container 1
    w.containers[c1].store.add(SimResource::Energy, 20);

    // ── M1 furniture ────────────────────────────────────────────────────────────────────────────
    // The hauler-A outbound corridor (x 18..=23 on y 25): stepped on every trip → wearout pulls.
    for x in 18..=23u8 {
        w.add_road(pos(x, 25), 5000, 5000);
    }
    // The WORKER's drive-by repair target: half-dead, range 1 from its post at (11,21).
    let repair_road = w.add_road(pos(REPAIR_ROAD_TILE.0, REPAIR_ROAD_TILE.1), 500, 5000);
    debug_assert_eq!(w.roads[repair_road].pos, pos(12, 22));
    // A short-fuse PLAIN road: one decay event (−100) inside the 600-tick run, no death.
    let event_road = w.add_road(pos(33, 33), 5000, 5000);
    w.roads[event_road].next_decay_at = 200;
    // A short-fuse SWAMP road with 500 hits: its first event (−500, swamp ×5) KILLS it — road
    // removal + terrain reversion inside the run.
    w.movement.terrain.swamps.insert(DOOMED_ROAD_TILE);
    let doomed_road = w.add_road(pos(DOOMED_ROAD_TILE.0, DOOMED_ROAD_TILE.1), 500, 25_000);
    w.roads[doomed_road].next_decay_at = 150;
    // A doomed cargo container (5000 hits = exactly one unowned decay event at tick 99): death
    // drops its 40 energy to ground, which then decays as an ordinary pile.
    let doomed_container = w.add_container(pos(DOOMED_CONTAINER_TILE.0, DOOMED_CONTAINER_TILE.1), 200, 5_000);
    w.containers[doomed_container].store.add(SimResource::Energy, 40);

    // Ids 1..=13, in this order (the fence pins the whole history, ids included).
    w.add_creep(pos(11, 20), &[Part::Work, Part::Work, Part::Carry, Part::Move], 100_000); // 1
    w.add_creep(pos(39, 20), &[Part::Work, Part::Work, Part::Carry, Part::Move], 100_000); // 2
    w.add_creep(pos(12, 20), &[Part::Carry, Part::Carry, Part::Move, Part::Move], 100_000); // 3
    w.add_creep(pos(38, 20), &[Part::Carry, Part::Carry, Part::Move, Part::Move], 100_000); // 4
    w.add_creep(pos(11, 21), &[Part::Work, Part::Carry, Part::Move], 100_000); // 5
    w.add_creep(pos(26, 25), &[Part::Carry, Part::Move], 300); // 6 — dies mid-run, cargo aboard
    w.add_creep(pos(12, 13), &[Part::Carry, Part::Move], 100_000); // 7 PUMP_X
    let y = w.add_creep(pos(13, 12), &[Part::Carry, Part::Move], 100_000); // 8 PUMP_Y
    w.creep_stores.get_mut(&y).unwrap().add(SimResource::Energy, 30); // primes the pump loop
    w.sync_carry_used(y);
    w.add_creep(pos(14, 35), &[Part::Work, Part::Work, Part::Move], 100_000); // 9
    w.add_creep(pos(16, 35), &[Part::Work, Part::Work, Part::Move], 100_000); // 10
    w.add_creep(pos(43, 25), &[Part::Move], 100_000); // 11 PULLER_LO
    w.add_creep(pos(44, 26), &[Part::Move], 100_000); // 12 PULLER_HI
    w.add_creep(pos(44, 25), &[Part::Carry], 100_000); // 13 PULL_TARGET (no MOVE — pull-only)
    w
}

/// One hauler's stateless script: fetch from its pile spot until half-full, deliver to the spawn
/// lane, repeat. Purely a function of world state — no memory, no insertion-order dependence.
/// The transfer asks for EXACTLY the energy held (a transfer over-ask is rejected whole,
/// mirroring the engine).
fn drive_hauler(
    w: &EconWorld,
    intents: &mut EconIntents,
    id: CreepId,
    fetch_spot: Position,
    pile_tile: Position,
    delivery_spot: Position,
) {
    let Some(creep) = w.creep(id) else { return };
    let store = &w.creep_stores[&id];
    let delivering = store.total() >= 50; // half-full commits the trip
    let target = if delivering { delivery_spot } else { fetch_spot };
    if creep.pos != target {
        if let Some(dir) = dir_toward(creep.pos, target) {
            intents.moves.set_move(id, dir);
        }
        return;
    }
    if delivering {
        let held = store.amount(SimResource::Energy);
        // First energy-hungry structure wins: spawn 0, then extensions in construction order.
        if w.spawns[0].store_energy < 300 {
            intents.act(id, EconAction::Transfer { target: StructRef::Spawn(0), resource: SimResource::Energy, amount: held });
        } else if let Some(e) = (0..w.extensions.len()).find(|&e| w.extensions[e].store_energy < w.extensions[e].capacity) {
            intents.act(id, EconAction::Transfer { target: StructRef::Extension(e), resource: SimResource::Energy, amount: held });
        }
    } else if let Some(idx) = w.dropped.iter().position(|p| p.pos == pile_tile && p.resource == SimResource::Energy) {
        intents.act(id, EconAction::Pickup { dropped_idx: idx });
    }
}

/// The scripted per-tick intent set for the fixture colony.
fn scripted(w: &EconWorld) -> EconIntents {
    let mut intents = EconIntents::new();
    let even = w.tick().is_multiple_of(2);

    // Harvesters + the worker: harvest every tick (stores overflow to ground → the haul piles).
    if w.creep(HARVESTER_A).is_some() {
        intents.act(HARVESTER_A, EconAction::Harvest { source_idx: 0 });
    }
    if w.creep(HARVESTER_B).is_some() {
        intents.act(HARVESTER_B, EconAction::Harvest { source_idx: 1 });
    }
    if w.creep(WORKER).is_some() {
        // M1 drive-by repair (state-derived, insertion-order-free): with ≥ 10 energy aboard and
        // the half-dead road within range 3 still damaged, the WORKER spends its ONE Pipeline-A
        // work intent repairing INSTEAD of harvesting (the S1 leak mechanic the resolver models);
        // otherwise it harvests. Index re-derived from the world each tick (compaction contract).
        let repair_target = w
            .road_at(pos(REPAIR_ROAD_TILE.0, REPAIR_ROAD_TILE.1))
            .filter(|&i| w.roads[i].hits < w.roads[i].hits_max);
        let worker_energy = w.creep_stores[&WORKER].amount(SimResource::Energy);
        match repair_target {
            Some(road_idx) if worker_energy >= 10 => {
                intents.act(WORKER, EconAction::Repair { target: StructRef::Road(road_idx) });
            }
            _ => {
                intents.act(WORKER, EconAction::Harvest { source_idx: 0 });
            }
        }
    }
    // The short-source pair: combined 8 e/t against a 300 pool — recurring exhaustion contention.
    for id in [SHORT_HARV_A, SHORT_HARV_B] {
        if w.creep(id).is_some() {
            intents.act(id, EconAction::Harvest { source_idx: 2 });
        }
    }

    drive_hauler(w, &mut intents, HAULER_A, pos(12, 20), pos(11, 20), pos(24, 25));
    drive_hauler(w, &mut intents, HAULER_B, pos(38, 20), pos(39, 20), pos(25, 26));

    // The shuttler's withdraw/transfer dance (until its death drops its cargo).
    if w.creep(SHUTTLER).is_some() {
        if even {
            intents.act(SHUTTLER, EconAction::Withdraw { target: StructRef::Extension(1), resource: SimResource::Energy, amount: 10 });
        } else {
            let held = w.creep_stores[&SHUTTLER].amount(SimResource::Energy);
            if held > 0 {
                intents.act(SHUTTLER, EconAction::Transfer { target: StructRef::Spawn(0), resource: SimResource::Energy, amount: held.min(10) });
            }
        }
    }

    // The pumper collision loop: X pumps container 0 → 1, Y pumps 1 → 0, phase-offset so each
    // even tick both hit container 0 (X reads, Y writes) and each odd tick both hit container 1.
    if w.creep(PUMP_X).is_some() {
        if even {
            intents.act(PUMP_X, EconAction::Withdraw { target: StructRef::Container(0), resource: SimResource::Energy, amount: 30 });
        } else {
            let held = w.creep_stores[&PUMP_X].amount(SimResource::Energy);
            if held > 0 {
                intents.act(PUMP_X, EconAction::Transfer { target: StructRef::Container(1), resource: SimResource::Energy, amount: held.min(30) });
            }
        }
    }
    if w.creep(PUMP_Y).is_some() {
        if even {
            let held = w.creep_stores[&PUMP_Y].amount(SimResource::Energy);
            if held > 0 {
                intents.act(PUMP_Y, EconAction::Transfer { target: StructRef::Container(0), resource: SimResource::Energy, amount: held.min(30) });
            }
        } else {
            intents.act(PUMP_Y, EconAction::Withdraw { target: StructRef::Container(1), resource: SimResource::Energy, amount: 30 });
        }
    }

    // The pull-conflict trio: LO walks into the target's tile (a mutual swap) while BOTH pullers
    // pull it every tick — a sustained two-pullers-one-target conflict (LO must always win).
    if let (Some(lo), Some(t)) = (w.creep(PULLER_LO), w.creep(PULL_TARGET)) {
        if let Some(dir) = dir_toward(lo.pos, t.pos) {
            intents.moves.set_move(PULLER_LO, dir);
            intents.moves.set_pull(PULLER_LO, PULL_TARGET);
        }
    }
    if w.creep(PULLER_HI).is_some() && w.creep(PULL_TARGET).is_some() {
        intents.moves.set_move(PULLER_HI, if even { Direction::Left } else { Direction::Right });
        intents.moves.set_pull(PULLER_HI, PULL_TARGET);
    }

    // The spawn loops: both spawns keep producing whenever the room can fund them — under
    // scarcity the same tick often carries TWO requests the room can afford one of (the
    // cross-spawn contention the (spawn index, submission order) rule resolves).
    if w.spawns[0].spawning.is_none() && w.room_spawn_energy() >= 300 {
        intents.spawn(0, vec![Part::Work, Part::Carry, Part::Move]);
    }
    if w.spawns[1].spawning.is_none() && w.room_spawn_energy() >= 250 {
        intents.spawn(1, vec![Part::Carry, Part::Carry, Part::Move, Part::Move]);
    }

    // Newborns (ids ≥ FIRST_NEWBORN) disperse to id-derived posts and idle there.
    for c in &w.movement.creeps {
        if c.id >= FIRST_NEWBORN {
            let post = pos(30 + (c.id % 8) as u8, 30 + ((c.id / 8) % 8) as u8);
            if c.pos != post {
                if let Some(dir) = dir_toward(c.pos, post) {
                    intents.moves.set_move(c.id, dir);
                }
            }
        }
    }
    intents
}

/// Run the fixture for `ticks`, returning (state digest, report digest). `permute` reverses each
/// tick's action list + re-inserts moves/pulls in reverse — the det_reorder arm.
fn run_fixture(ticks: u32, permute: bool) -> (u64, u64) {
    let mut w = fixture();
    let mut report_digest: u64 = 0xcbf2_9ce4_8422_2325;
    for _ in 0..ticks {
        let mut intents = scripted(&w);
        if permute {
            intents.actions.reverse();
            let moves: Vec<_> = intents.moves.moves.drain().collect();
            for (id, dir) in moves.into_iter().rev() {
                intents.moves.set_move(id, dir);
            }
            let pulls: Vec<_> = intents.moves.pulls.drain().collect();
            for (puller, target) in pulls.into_iter().rev() {
                intents.moves.set_pull(puller, target);
            }
        }
        let report = resolve_econ_tick(&mut w, &intents);
        assert!(report.conservation.is_empty(), "conservation violated at tick {}: {:?}", report.tick, report.conservation);
        report_digest = fold_report(report_digest, &report);
    }
    (w.state_digest(), report_digest)
}

/// The fence: 5 runs of the 600-tick fixture — digest spread must be 0.
#[test]
fn econ_engine_is_deterministic() {
    let baseline = run_fixture(600, false);
    for run in 1..5 {
        assert_eq!(run_fixture(600, false), baseline, "run {run} diverged from run 0");
    }

    // The instrumented replay: the colony must actually EXERCISE the order-sensitive lanes the
    // fence exists to cover (anti-vacuity floors — if the script's timing drifts and a lane
    // stops binding, this fails loudly instead of the fence silently going blind).
    let mut w = fixture();
    let mut births = 0usize;
    let mut deaths = 0usize;
    let mut store_collision_ticks = 0u32; // pumper pair: same-container read+write, order-sensitive
    let mut short_source_ticks = 0u32; // the cap-300 pool below combined demand
    let mut source_binding_ticks = 0u32; // 0 < pool < demand: who gets the last energy
    let mut pull_conflict_ticks = 0u32; // both pullers validly pulling the one target
    let mut pull_target_moves = 0u32;
    let mut spawn_contention_ticks = 0u32; // two requests, room affords at most one
    let mut repair_energy = 0u64; // M1: the WORKER's drive-by road repairs actually burn energy
    for _ in 0..600 {
        let even = w.tick().is_multiple_of(2);
        if w.creep(PUMP_X).is_some() && w.creep(PUMP_Y).is_some() {
            let c = &w.containers[if even { 0 } else { 1 }];
            if c.store.free() < 30 || c.store.amount(SimResource::Energy) < 30 {
                store_collision_ticks += 1;
            }
        }
        if w.sources[2].energy < 8 {
            short_source_ticks += 1;
            if w.sources[2].energy > 0 {
                source_binding_ticks += 1;
            }
        }
        if let (Some(lo), Some(hi), Some(t)) =
            (w.creep(PULLER_LO), w.creep(PULLER_HI), w.creep(PULL_TARGET))
        {
            if lo.pos.get_range_to(t.pos) <= 1 && hi.pos.get_range_to(t.pos) <= 1 {
                pull_conflict_ticks += 1;
            }
        }
        let both_requests = w.spawns[0].spawning.is_none()
            && w.spawns[1].spawning.is_none()
            && w.room_spawn_energy() >= 300
            && w.room_spawn_energy() < 500; // can fund one 200-300 body, not two
        if both_requests {
            spawn_contention_ticks += 1;
        }
        let t_before = w.creep(PULL_TARGET).map(|c| c.pos);
        let intents = scripted(&w);
        let r = resolve_econ_tick(&mut w, &intents);
        births += r.births.len();
        deaths += r.deaths.len();
        repair_energy += r.ledger.repair_roads;
        if w.creep(PULL_TARGET).map(|c| c.pos) != t_before {
            pull_target_moves += 1;
        }
    }
    assert!(births > 5, "the spawn loop kept producing (got {births})");
    assert!(deaths >= 1, "the shuttler's TTL death fired");

    // ── M1 anti-vacuity floors ──────────────────────────────────────────────────────────────────
    assert!(repair_energy >= 20, "the WORKER's drive-by repairs must recur (burned {repair_energy}e)");
    let repair_road = w.road_at(pos(REPAIR_ROAD_TILE.0, REPAIR_ROAD_TILE.1)).expect("repair road alive");
    assert_eq!(
        w.roads[repair_road].hits, w.roads[repair_road].hits_max,
        "the WORKER healed its road to full inside the run"
    );
    let wear_road = w.road_at(pos(WEAR_ROAD_TILE.0, WEAR_ROAD_TILE.1)).expect("corridor road alive");
    assert!(
        w.roads[wear_road].next_decay_at <= 1000 - 40,
        "hauler traffic must pull the corridor road's decay clock ≥ 40 ticks (at {})",
        w.roads[wear_road].next_decay_at
    );
    let event_road = w.road_at(pos(33, 33)).expect("short-fuse plain road alive");
    assert_eq!(w.roads[event_road].hits, 4900, "exactly one −100 decay event fired on the plain road");
    assert!(w.road_at(pos(DOOMED_ROAD_TILE.0, DOOMED_ROAD_TILE.1)).is_none(), "the swamp road died");
    assert!(
        !w.movement.terrain.roads.contains(&DOOMED_ROAD_TILE),
        "the dead road's tile reverted to natural terrain"
    );
    assert!(
        w.movement.terrain.swamps.contains(&DOOMED_ROAD_TILE),
        "…which is still the swamp underneath"
    );
    assert_eq!(w.containers.len(), 2, "the doomed cargo container died (pumper pair survives)");
    assert!(
        !w.containers.iter().any(|c| c.pos == pos(DOOMED_CONTAINER_TILE.0, DOOMED_CONTAINER_TILE.1)),
        "no container remains on the doomed tile"
    );
    assert!(store_collision_ticks >= 20, "same-store collisions must recur (got {store_collision_ticks})");
    assert!(short_source_ticks >= 100, "the small source must run below demand (got {short_source_ticks})");
    assert!(source_binding_ticks >= 1, "the last-energy harvest split must bind (got {source_binding_ticks})");
    assert!(pull_conflict_ticks >= 20, "the two-pullers-one-target conflict must recur (got {pull_conflict_ticks})");
    assert!(pull_target_moves >= 100, "the pulled no-MOVE creep actually gets dragged (got {pull_target_moves})");
    assert!(spawn_contention_ticks >= 5, "cross-spawn energy contention must recur (got {spawn_contention_ticks})");
}

/// Reordered intent insertion must not change one bit of the history. (Creep actions and
/// cross-spawn requests are re-ordered by the resolver; the script emits no same-actor
/// duplicates — those first-submitted-wins contracts are pinned by unit tests instead.)
#[test]
fn det_reorder() {
    assert_eq!(run_fixture(600, false), run_fixture(600, true), "insertion order leaked into the outcome");
}

// ── The M2 fence: controller + build mechanics entered the state/digest — re-prove spread 0 ─────

const M2_UPGRADER: CreepId = 1;
const M2_BUILDER: CreepId = 2;

/// The M2 fixture: a level-2 controller with a 40-tick fuse (one DOWNGRADE fires ~t39 while the
/// upgrader hasn't started), an upgrader that from t60 upgrades every tick with the live
/// parallel-refill withdraw (Pipeline D + E in one tick), and a builder chewing through an
/// extension site, a swamp road site, and one placed MID-RUN — level-up, downgrade, site
/// placement, completion/materialization, and the upgrade/build sinks all inside 600 ticks.
fn m2_fixture() -> EconWorld {
    let mut w = EconWorld::default();
    w.set_controller(pos(30, 30), 2);
    {
        let c = w.controller.as_mut().unwrap();
        c.progress = 100;
        c.downgrade_ticks = 40; // expires ≈ t39 → level 1, progress += 0.9 × 200
    }
    // The upgrader + its feeders (a 2000 container, then storage backup).
    w.add_creep(pos(32, 30), &[Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Carry, Part::Carry, Part::Move], 100_000); // 1
    let cu = w.add_container(pos(33, 30), 2000, 250_000);
    w.containers[cu].store.add(SimResource::Energy, 2000);
    w.set_storage(pos(33, 31), 1_000_000);
    w.storage.as_mut().unwrap().store.add(SimResource::Energy, 10_000);

    // The builder + its feeders + the pre-placed sites.
    w.add_creep(
        pos(10, 30),
        &[Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Carry, Part::Carry, Part::Carry, Part::Carry, Part::Move],
        100_000,
    ); // 2 — 50 progress/t, 200 carry
    for (i, p) in [pos(9, 30), pos(10, 29), pos(9, 29)].into_iter().enumerate() {
        let c = w.add_container(p, 2000, 250_000);
        w.containers[c].store.add(SimResource::Energy, 2000);
        let _ = i;
    }
    w.movement.terrain.swamps.insert((12, 31));
    w.add_construction_site(pos(11, 30), StructureKind::Extension).expect("extension site places");
    let road = w.add_construction_site(pos(12, 31), StructureKind::Road).expect("swamp road site places");
    assert_eq!(w.sites[road].total, 1500, "swamp road cost ×5 at placement");
    w
}

/// The scripted per-tick intents for the M2 fixture (purely state-derived).
fn m2_scripted(w: &EconWorld) -> EconIntents {
    let mut intents = EconIntents::new();
    let tick = w.tick();

    // Upgrader: from t60, upgrade every tick; when the store is about to run dry (≤ 5 = one
    // tick's conversion), ALSO withdraw this tick (the live parallel D+E refill idiom).
    if w.creep(M2_UPGRADER).is_some() {
        let energy = w.creep_stores[&M2_UPGRADER].amount(SimResource::Energy);
        if tick >= 60 {
            if energy > 0 {
                intents.act(M2_UPGRADER, EconAction::UpgradeController);
            }
            if energy <= 5 {
                let feeder = w
                    .containers
                    .iter()
                    .position(|c| c.pos == pos(33, 30) && c.store.amount(SimResource::Energy) > 0)
                    .map(StructRef::Container)
                    .unwrap_or(StructRef::Storage);
                intents.act(M2_UPGRADER, EconAction::Withdraw { target: feeder, resource: SimResource::Energy, amount: 100 });
            }
        } else if energy < 100 && tick < 3 {
            intents.act(M2_UPGRADER, EconAction::Withdraw { target: StructRef::Container(0), resource: SimResource::Energy, amount: 100 });
        }
    }

    // Builder: build the FIRST live site each tick (index re-derived — the compaction contract);
    // withdraw from the first non-empty feeder whenever the store dips below one build's worth.
    if w.creep(M2_BUILDER).is_some() {
        let energy = w.creep_stores[&M2_BUILDER].amount(SimResource::Energy);
        if !w.sites.is_empty() && energy > 0 {
            intents.act(M2_BUILDER, EconAction::Build { site_idx: 0 });
        }
        if energy <= 50 {
            if let Some(feeder) = w
                .containers
                .iter()
                .position(|c| c.pos != pos(33, 30) && c.store.amount(SimResource::Energy) > 0)
            {
                intents.act(M2_BUILDER, EconAction::Withdraw { target: StructRef::Container(feeder), resource: SimResource::Energy, amount: 200 });
            }
        }
    }
    intents
}

/// Run the M2 fixture (with the tick-300 mid-run site placement), returning the digest pair +
/// the instrumentation counters.
#[allow(clippy::type_complexity)]
fn run_m2_fixture(ticks: u32, permute: bool) -> ((u64, u64), (u32, u32, u32, u64, u64, u32)) {
    let mut w = m2_fixture();
    let mut report_digest: u64 = 0xcbf2_9ce4_8422_2325;
    let (mut level_ups, mut downgrades, mut completed) = (0u32, 0u32, 0u32);
    let (mut upgrade_e, mut build_e) = (0u64, 0u64);
    let mut parallel_de_ticks = 0u32;
    for _ in 0..ticks {
        if w.tick() == 300 {
            // Mid-run placement: the plain road the builder finishes last.
            w.add_construction_site(pos(12, 30), StructureKind::Road).expect("mid-run site places");
        }
        let mut intents = m2_scripted(&w);
        let upgrader_both = intents
            .actions
            .iter()
            .filter(|(id, _)| *id == M2_UPGRADER)
            .count()
            == 2;
        if permute {
            intents.actions.reverse();
        }
        let report = resolve_econ_tick(&mut w, &intents);
        assert!(report.conservation.is_empty(), "conservation violated at tick {}: {:?}", report.tick, report.conservation);
        report_digest = fold_report(report_digest, &report);
        level_ups += report.level_ups.len() as u32;
        downgrades += report.downgrades.len() as u32;
        completed += report.sites_completed.len() as u32;
        upgrade_e += report.ledger.upgrade;
        build_e += report.ledger.build;
        if upgrader_both && report.ledger.upgrade > 0 {
            parallel_de_ticks += 1;
        }
    }
    ((w.state_digest(), report_digest), (level_ups, downgrades, completed, upgrade_e, build_e, parallel_de_ticks))
}

/// The M2 fence: 5 runs spread 0, the reversed-insertion arm bit-identical, and anti-vacuity
/// floors proving the run actually EXERCISED a downgrade, a level-up, all three site
/// completions, both new sinks, and recurring parallel withdraw+upgrade ticks.
#[test]
fn econ_engine_m2_controller_build_determinism() {
    let (baseline, floors) = run_m2_fixture(600, false);
    for run in 1..5 {
        assert_eq!(run_m2_fixture(600, false).0, baseline, "run {run} diverged from run 0");
    }
    assert_eq!(run_m2_fixture(600, true).0, baseline, "insertion order leaked into the M2 lanes");

    let (level_ups, downgrades, completed, upgrade_e, build_e, parallel_de) = floors;
    assert!(downgrades >= 1, "the 40-tick fuse must fire a downgrade (got {downgrades})");
    assert!(level_ups >= 1, "the upgrader must earn a level back (got {level_ups})");
    assert_eq!(completed, 3, "extension + swamp road + mid-run road all complete");
    assert!(upgrade_e >= 1000, "the upgrade sink must run hot (got {upgrade_e}e)");
    assert_eq!(build_e, 3000 + 1500 + 300, "build energy = exactly the three sites' costs");
    assert!(parallel_de >= 15, "the parallel withdraw+upgrade tick must recur (got {parallel_de})");

    // End-state sanity: the structures materialized where the sites were.
    let w = {
        let mut w = m2_fixture();
        for _ in 0..600 {
            if w.tick() == 300 {
                w.add_construction_site(pos(12, 30), StructureKind::Road).unwrap();
            }
            let intents = m2_scripted(&w);
            resolve_econ_tick(&mut w, &intents);
        }
        w
    };
    assert_eq!(w.extensions.len(), 1, "the extension exists");
    assert!(w.movement.terrain.roads.contains(&(12, 31)), "the swamp road registered its tile");
    assert!(w.movement.terrain.roads.contains(&(12, 30)), "the mid-run road registered its tile");
    assert_eq!(w.controller.as_ref().unwrap().level, 2, "downgraded to 1, upgraded back to 2");
}

// ── The M6 fence: labs + minerals entered the state/digest — re-prove spread 0 ──────────────────

const M6_MINER: CreepId = 1;
const M6_BOOST_TARGET: CreepId = 2;

/// The M6 fixture: an ULTRA mineral (always re-rolls on regen) drained low so it exhausts + regens
/// (density re-roll) several times inside the run; an extractor pacing the harvest; a 3-lab reaction
/// cluster (G + H → GH, cooldown 10) fed from stocked input labs; a boost lab holding XGH2O; and a
/// WORK creep boosted mid-run whose subsequent upgrades run at ×2. Purely state-derived drivers.
fn m6_fixture() -> EconWorld {
    let mut w = EconWorld::default();
    // A small ULTRA pool so it drains + regens repeatedly (re-roll every regen).
    let mineral = w.add_mineral(pos(20, 20), SimResource::Utrium, 4 /*ULTRA*/, 7);
    w.minerals[mineral].amount = 30; // 5 harvests of a 6-WORK miner drain it
    w.add_extractor(pos(20, 20));
    // The reaction cluster: two input labs (range 1 of the output), stocked; the output lab empty.
    let in_g = w.add_lab(pos(30, 30), 0);
    w.labs[in_g].mineral = Some((SimResource::Ghodium, 3000));
    let in_h = w.add_lab(pos(31, 30), 0);
    w.labs[in_h].mineral = Some((SimResource::Hydrogen, 3000));
    w.add_lab(pos(30, 31), 0); // out, idx 2
    // The boost lab, holding XGH2O + energy (boosts the target's 4 WORK parts).
    let boost_lab = w.add_lab(pos(35, 35), 2000);
    w.labs[boost_lab].mineral = Some((SimResource::XGH2O, 3000));
    // A controller for the boosted upgrader to work (RCL 5 — below the RCL8 cap).
    w.set_controller(pos(36, 36), 5);

    // The miner (6 WORK): harvests the mineral through the extractor's 5-tick cooldown.
    w.add_creep(pos(21, 20), &[Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Carry, Part::Carry, Part::Carry, Part::Carry, Part::Move], 100_000); // 1
    // The boost target / upgrader (4 WORK), placed adjacent to the boost lab, fed energy.
    let up = w.add_creep(pos(35, 36), &[Part::Work, Part::Work, Part::Work, Part::Work, Part::Carry, Part::Carry, Part::Move], 100_000); // 2
    w.creep_stores.get_mut(&up).unwrap().add(SimResource::Energy, 100);
    w.sync_carry_used(up);
    w
}

/// The M6 fixture's per-tick scripted intents (state-derived, insertion-order-free).
fn m6_scripted(w: &EconWorld) -> EconIntents {
    let mut intents = EconIntents::new();

    // The miner harvests whenever the extractor is off cooldown AND the pool holds mineral.
    if w.creep(M6_MINER).is_some() {
        let ready = w.extractor_at(pos(20, 20)).is_some_and(|i| w.extractors[i].cooldown == 0);
        if ready && w.minerals[0].amount > 0 {
            intents.act(M6_MINER, EconAction::HarvestMineral { mineral_idx: 0 });
        }
    }
    // The reaction fires whenever the output lab is off cooldown and the inputs are stocked.
    if w.labs.len() >= 3 && w.tick() >= w.labs[2].cooldown_at && w.labs[0].mineral_amount(SimResource::Ghodium) >= 5 && w.labs[1].mineral_amount(SimResource::Hydrogen) >= 5 {
        intents.react(2, 0, 1);
    }
    // Boost the target ONCE (when it still has an unboosted WORK part).
    if let Some(c) = w.creep(M6_BOOST_TARGET) {
        let unboosted = c.body.parts.iter().filter(|p| p.part == Part::Work && p.boost == screeps_sim_core::BoostTier::None).count();
        if unboosted > 0 {
            intents.boost(M6_BOOST_TARGET, 3); // the XGH2O boost lab (idx 3)
        } else if w.creep_stores[&M6_BOOST_TARGET].amount(SimResource::Energy) > 0 {
            // Once boosted, upgrade (the ×2 effect); top up energy from... just keep upgrading.
            intents.act(M6_BOOST_TARGET, EconAction::UpgradeController);
        }
    }
    intents
}

#[allow(clippy::type_complexity)]
fn run_m6_fixture(ticks: u32, permute: bool) -> ((u64, u64), (u32, u64, u64, u32, bool)) {
    let mut w = m6_fixture();
    let mut report_digest: u64 = 0xcbf2_9ce4_8422_2325;
    let (mut mineral_harvests, mut reaction_products, mut boost_mineral) = (0u32, 0u64, 0u64);
    let mut regens = 0u32;
    let mut last_amount = w.minerals[0].amount;
    let mut ever_boosted = false;
    for _ in 0..ticks {
        // Refill the boost target's energy so the post-boost upgrade lane keeps running.
        if let Some(store) = w.creep_stores.get_mut(&M6_BOOST_TARGET) {
            if store.amount(SimResource::Energy) < 8 {
                store.add(SimResource::Energy, 100);
                w.sync_carry_used(M6_BOOST_TARGET);
            }
        }
        let mut intents = m6_scripted(&w);
        if permute {
            intents.actions.reverse();
        }
        let before_amount = w.minerals[0].amount;
        let report = resolve_econ_tick(&mut w, &intents);
        assert!(report.conservation.is_empty(), "conservation violated at tick {}: {:?}", report.tick, report.conservation);
        report_digest = fold_report(report_digest, &report);
        mineral_harvests += report.ledger.harvested_mineral.values().sum::<u64>() as u32;
        reaction_products += report.ledger.reaction_produced.values().sum::<u64>();
        boost_mineral += report.ledger.boost_mineral.values().sum::<u64>();
        // A regen event: the pool jumped from 0 back up. Cap the refilled pool low so it
        // re-drains fast (keeping the run short) — a deterministic test manipulation applied
        // identically every run, so the fence stays spread-0.
        if before_amount == 0 && w.minerals[0].amount > 0 {
            regens += 1;
            w.minerals[0].amount = 30;
        }
        let _ = last_amount;
        last_amount = w.minerals[0].amount;
        if w.creep(M6_BOOST_TARGET).map(|c| c.body.parts.iter().any(|p| p.part == Part::Work && p.boost == screeps_sim_core::BoostTier::T3)).unwrap_or(false) {
            ever_boosted = true;
        }
        // Fast-forward the 50k regen timer to keep the run short (deterministic — same each run).
        if w.minerals[0].amount == 0 {
            if let Some(_r) = w.minerals[0].regen_at {
                w.minerals[0].regen_at = Some(w.tick() + 1);
            }
        }
    }
    ((w.state_digest(), report_digest), (regens, reaction_products, boost_mineral, mineral_harvests, ever_boosted))
}

/// The M6 fence: 5 runs spread 0, the reversed-insertion arm bit-identical, and anti-vacuity floors
/// proving the run EXERCISED mineral harvest + at least one seeded density re-roll on regen + lab
/// reactions + a boostCreep + the boosted-upgrade effect.
#[test]
fn econ_engine_m6_labs_minerals_determinism() {
    let (baseline, floors) = run_m6_fixture(400, false);
    for run in 1..5 {
        assert_eq!(run_m6_fixture(400, false).0, baseline, "run {run} diverged from run 0");
    }
    assert_eq!(run_m6_fixture(400, true).0, baseline, "insertion order leaked into the M6 lanes");

    let (regens, reaction_products, boost_mineral, mineral_harvests, ever_boosted) = floors;
    assert!(mineral_harvests >= 12, "the extractor-paced mineral harvest must recur (got {mineral_harvests})");
    assert!(regens >= 2, "the mineral must exhaust + regen (seeded density re-roll) ≥ 2× (got {regens})");
    assert!(reaction_products >= 20, "the lab reaction cluster must produce GH repeatedly (got {reaction_products})");
    assert_eq!(boost_mineral, 4 * 30, "the target's 4 WORK parts boosted (4 × 30 XGH2O)");
    assert!(ever_boosted, "the boost target reached T3");

    // End-state sanity: the ULTRA mineral re-rolled OFF ultra at least once (density changed),
    // and the boosted upgrader accumulated controller progress at the ×2 rate.
    let mut w = m6_fixture();
    let mut densities_seen = std::collections::BTreeSet::new();
    for _ in 0..400 {
        let mut intents = m6_scripted(&w);
        let _ = &mut intents;
        if let Some(store) = w.creep_stores.get_mut(&M6_BOOST_TARGET) {
            if store.amount(SimResource::Energy) < 8 {
                store.add(SimResource::Energy, 100);
                w.sync_carry_used(M6_BOOST_TARGET);
            }
        }
        let intents = m6_scripted(&w);
        let before = w.minerals[0].amount;
        resolve_econ_tick(&mut w, &intents);
        densities_seen.insert(w.minerals[0].density);
        if before == 0 && w.minerals[0].amount > 0 {
            w.minerals[0].amount = 30; // same cap as run_m6_fixture — keeps the re-drain fast
        }
        if w.minerals[0].amount == 0 {
            if let Some(_r) = w.minerals[0].regen_at {
                w.minerals[0].regen_at = Some(w.tick() + 1);
            }
        }
    }
    assert!(densities_seen.len() >= 2, "the density re-roll actually VARIED the tier (saw {densities_seen:?})");
    assert!(w.controller.as_ref().unwrap().progress > 0, "the boosted upgrader made controller progress");
}

// ── Conservation fuzz ────────────────────────────────────────────────────────────────────────────

fn fuzz_world(rng: &mut Rng) -> EconWorld {
    let mut w = EconWorld::default();
    w.add_source(pos(10, 10), 3000);
    w.add_source(pos(40, 40), 1500);
    w.add_spawn(pos(25, 25));
    w.add_spawn(pos(20, 25));
    for (i, p) in [pos(24, 24), pos(26, 24), pos(24, 26)].into_iter().enumerate() {
        let e = w.add_extension(p, 6 + i as u8);
        w.extensions[e].store_energy = rng.range(0, 50);
    }
    let c0 = w.add_container(pos(11, 10), 2000, 250_000);
    w.containers[c0].store.add(SimResource::Energy, 800);
    w.containers[c0].store.add(SimResource::Ghodium, 120);
    let c1 = w.add_container(pos(26, 25), 2000, 250_000);
    w.containers[c1].store.add(SimResource::Oxygen, 300);
    w.set_storage(pos(24, 25), 1_000_000);
    w.storage.as_mut().unwrap().store.add(SimResource::Energy, 5_000);
    w.storage.as_mut().unwrap().store.add(SimResource::Hydrogen, 400);
    w.drop_resource(pos(12, 10), SimResource::Energy, 1_200);
    w.drop_resource(pos(25, 24), SimResource::Ghodium, 90);
    // M1: damaged roads scattered through the creep field (random Repair targets + wearout under
    // the random walks); one on swamp (×5 decay), one short-fuse (decay events mid-fuzz).
    w.movement.terrain.swamps.insert((20, 20));
    w.add_road(pos(15, 15), 900, 5000);
    w.add_road(pos(20, 20), 6_000, 25_000);
    let short_fuse = w.add_road(pos(25, 25), 2_500, 5000);
    w.roads[short_fuse].next_decay_at = 60;
    // M2: a controller with a mid-fuzz-expiring clock (downgrades under fire) + sites for the
    // random Build arm (one is pre-damaged toward completion so a materialization fires).
    w.set_controller(pos(18, 18), 2);
    w.controller.as_mut().unwrap().downgrade_ticks = 250;
    w.controller.as_mut().unwrap().progress = 150; // near the 45_000... no — near NOTHING; random upgrades may or may not level it
    w.add_construction_site(pos(17, 17), StructureKind::Road).expect("fuzz road site");
    let ext_site = w.add_construction_site(pos(16, 16), StructureKind::Extension).expect("fuzz extension site");
    w.sites[ext_site].progress = 2_900; // 100 from completion — materialization likely mid-fuzz

    // M6: a mineral + extractor (random HarvestMineral) that drains + regens under the seeded
    // re-roll; a lab cluster (random RunReaction + BoostCreep) stocked with a base pair + a boost
    // compound; storage stocked with minerals for the random SellMineral recovery lever.
    let mineral = w.add_mineral(pos(38, 38), SimResource::Utrium, 1 /*LOW: always re-rolls*/, 91);
    w.minerals[mineral].amount = 20; // drains fast → regens mid-fuzz
    w.add_extractor(pos(38, 38));
    let lg = w.add_lab(pos(5, 40), 500);
    w.labs[lg].mineral = Some((SimResource::Ghodium, 400));
    let lh = w.add_lab(pos(6, 40), 500);
    w.labs[lh].mineral = Some((SimResource::Hydrogen, 400));
    w.add_lab(pos(5, 41), 500); // out
    let boost_lab = w.add_lab(pos(7, 40), 500);
    w.labs[boost_lab].mineral = Some((SimResource::XGH2O, 400));
    w.storage.as_mut().unwrap().store.add(SimResource::Zynthium, 2_000); // sell fodder

    let bodies: [&[Part]; 4] = [
        &[Part::Work, Part::Work, Part::Carry, Part::Move],
        &[Part::Carry, Part::Carry, Part::Move, Part::Move],
        &[Part::Work, Part::Move], // no CARRY: every harvest overflows to ground
        &[Part::Work, Part::Carry, Part::Carry, Part::Move],
    ];
    for i in 0..8u32 {
        let p = pos(9 + (i * 4 % 30) as u8, 9 + (i * 7 % 30) as u8);
        w.add_creep(p, bodies[(i % 4) as usize], rng.range(100, 450)); // staggered mid-run deaths
    }
    w
}

fn random_resource(rng: &mut Rng) -> SimResource {
    match rng.range(0, 12) {
        0..=6 => SimResource::Energy,
        7 => SimResource::Hydrogen,
        8 => SimResource::Oxygen,
        9 => SimResource::Zynthium,
        10 => SimResource::Ghodium,
        11 => SimResource::XGH2O,
        _ => SimResource::Ghodium,
    }
}

fn random_target(rng: &mut Rng) -> StructRef {
    match rng.range(0, 5) {
        0 => StructRef::Spawn(rng.range(0, 2) as usize), // idx 2 is out of bounds — rejection path
        1 => StructRef::Extension(rng.range(0, 3) as usize),
        2 => StructRef::Container(rng.range(0, 2) as usize),
        3 => StructRef::Road(rng.range(0, 4) as usize), // idx 3 is out of bounds — rejection path
        _ => StructRef::Storage,
    }
}

/// Random-but-well-formed intents for 500 ticks (invalid indices/oversize bodies/transfer
/// over-asks included on purpose — rejections must be as conservation-clean as successes). The
/// audit holds EVERY tick.
#[test]
fn conservation_fuzz_500_ticks() {
    let mut rng = Rng::seeded(40); // ADR 0040
    let mut w = fuzz_world(&mut rng);
    let spawn_bodies: [&[Part]; 4] = [
        &[Part::Move],
        &[Part::Work, Part::Carry, Part::Move],
        &[Part::Carry, Part::Carry, Part::Move, Part::Move],
        &[Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Work, Part::Move], // 1050e: usually unaffordable → atomic fail
    ];

    for _ in 0..500 {
        let mut intents = EconIntents::new();
        let ids: Vec<CreepId> = w.movement.creeps.iter().map(|c| c.id).collect();
        for id in ids {
            // M2 lanes ride ALONGSIDE the Pipeline-A/D roll (upgrade is its own pipeline; a
            // random build may mask the rolled work action — both must stay conservation-clean).
            if rng.chance(20) {
                intents.act(id, EconAction::UpgradeController);
            }
            if rng.chance(15) {
                intents.act(id, EconAction::Build { site_idx: rng.range(0, 3) as usize }); // idx OOB included
            }
            match rng.range(0, 6) {
                0 => {
                    intents.act(id, EconAction::Harvest { source_idx: rng.range(0, 2) as usize }); // idx 2 OOB
                }
                1 => {
                    let (target, resource, amount) = (random_target(&mut rng), random_resource(&mut rng), rng.range(0, 400));
                    intents.act(id, EconAction::Transfer { target, resource, amount });
                }
                2 => {
                    let (target, resource, amount) = (random_target(&mut rng), random_resource(&mut rng), rng.range(0, 400));
                    intents.act(id, EconAction::Withdraw { target, resource, amount });
                }
                3 => {
                    intents.act(id, EconAction::Pickup { dropped_idx: rng.range(0, 6) as usize });
                }
                5 => {
                    // M1: random repairs — storeless/store-only/OOB targets must reject cleanly,
                    // valid ones must burn exactly the ledgered energy (the audit is the judge).
                    intents.act(id, EconAction::Repair { target: random_target(&mut rng) });
                }
                4 => {
                    // Duplicate-pipeline spam: both a second A and a second D must mask cleanly.
                    intents.act(id, EconAction::Harvest { source_idx: 0 });
                    intents.act(id, EconAction::Harvest { source_idx: 1 });
                    intents.act(id, EconAction::Pickup { dropped_idx: 0 });
                    intents.act(id, EconAction::Pickup { dropped_idx: 1 });
                }
                _ => {}
            }
            // M6: random mineral harvest (idx OOB included) + random boost (the target creep is
            // this id; range/lab-state gates most, valid ones must ledger exactly — the audit is
            // the judge). BoostCreep is a lab intent naming this creep as the target.
            if rng.chance(20) {
                intents.act(id, EconAction::HarvestMineral { mineral_idx: rng.range(0, 2) as usize }); // idx 1 OOB
            }
            if rng.chance(15) {
                intents.boost(id, rng.range(0, 5) as usize); // lab idx (some OOB / non-boost)
            }
            if rng.chance(70) {
                let dir = match rng.range(1, 8) {
                    1 => Direction::Top,
                    2 => Direction::TopRight,
                    3 => Direction::Right,
                    4 => Direction::BottomRight,
                    5 => Direction::Bottom,
                    6 => Direction::BottomLeft,
                    7 => Direction::Left,
                    _ => Direction::TopLeft,
                };
                intents.moves.set_move(id, dir);
            }
        }
        if rng.chance(25) {
            let body = spawn_bodies[rng.range(0, 3) as usize].to_vec();
            intents.spawn(rng.range(0, 2) as usize, body);
        }
        if rng.chance(5) {
            intents.spawn(0, vec![Part::Move; 51]); // oversize: must reject, never debit
        }
        // M6 structure intents: random reactions (some OOB / distinct-lab-violating) + random
        // terminal sales (some of energy, rejected; some over-ask, clamped). The audit is judge.
        if rng.chance(30) {
            intents.react(rng.range(0, 5) as usize, rng.range(0, 5) as usize, rng.range(0, 5) as usize);
        }
        if rng.chance(20) {
            let res = random_resource(&mut rng); // energy included → the reject path
            intents.sell(res, rng.range(0, 500));
        }

        let report = resolve_econ_tick(&mut w, &intents);
        assert!(
            report.conservation.is_empty(),
            "conservation violated at tick {}: {:?}",
            report.tick,
            report.conservation
        );
        for c in &w.movement.creeps {
            assert_eq!(
                c.carry_used,
                w.creep_stores[&c.id].total(),
                "creep {} weight invariant broke at tick {}",
                c.id,
                report.tick
            );
        }
    }
}
