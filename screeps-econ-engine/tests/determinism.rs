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
//!   evaporate if the script's timing drifts.
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
    for (res, v) in &r.ledger.dropped_decay {
        eat(*res as u64);
        eat(*v);
    }
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
        intents.act(WORKER, EconAction::Harvest { source_idx: 0 });
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
        if w.creep(PULL_TARGET).map(|c| c.pos) != t_before {
            pull_target_moves += 1;
        }
    }
    assert!(births > 5, "the spawn loop kept producing (got {births})");
    assert!(deaths >= 1, "the shuttler's TTL death fired");
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
    match rng.range(0, 9) {
        0..=6 => SimResource::Energy,
        7 => SimResource::Hydrogen,
        8 => SimResource::Oxygen,
        _ => SimResource::Ghodium,
    }
}

fn random_target(rng: &mut Rng) -> StructRef {
    match rng.range(0, 4) {
        0 => StructRef::Spawn(rng.range(0, 2) as usize), // idx 2 is out of bounds — rejection path
        1 => StructRef::Extension(rng.range(0, 3) as usize),
        2 => StructRef::Container(rng.range(0, 2) as usize),
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
                4 => {
                    // Duplicate-pipeline spam: both a second A and a second D must mask cleanly.
                    intents.act(id, EconAction::Harvest { source_idx: 0 });
                    intents.act(id, EconAction::Harvest { source_idx: 1 });
                    intents.act(id, EconAction::Pickup { dropped_idx: 0 });
                    intents.act(id, EconAction::Pickup { dropped_idx: 1 });
                }
                _ => {}
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
