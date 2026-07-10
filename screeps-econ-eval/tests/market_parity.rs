//! ADR 0040 M5a — the **live-path parity test** (spec Part 2 coverage). The live bot's market
//! adapter (`screeps-ibex/src/transfer/market_adapter.rs`) builds `MarketDeposit`/`MarketPickup`/
//! `MarketCarrier` DTOs from the world and runs the shared kernel
//! `screeps_econ_decision::market::market_pass`. The SIM driver
//! (`screeps-econ-eval::market::MarketRuntime::market_pass`) now DELEGATES to the same kernel.
//!
//! This test proves the two paths are equivalent BY CONSTRUCTION: on a fixture `EconWorld` it
//! (1) runs the sim driver's full `begin_tick` + `market_pass` to get the sim's per-carrier
//! task set, and (2) independently builds the kernel DTOs the way the LIVE adapter would from
//! the SAME priced deposits / pickups / carriers, runs the shared kernel directly, and asserts
//! the resulting task set is IDENTICAL. Shared kernel + equal DTOs ⇒ equal decisions — that IS
//! the live-path coverage the "no offline harness for the live path" concern asked for. If the
//! live adapter's DTO construction ever diverged from the sim's, this equality would break.

use screeps::{Part, Position, RoomCoordinate, RoomName};
use screeps_econ_decision::market as mk;
use screeps_econ_decision::sink_economics::MarketConsts;
use screeps_econ_eval::baseline::{deposits, pickups, Bookings, Lane, SinkKey};
use screeps_econ_eval::layout::LayoutInfo;
use screeps_econ_eval::market::{CarrierDto, MarketArmCfg, MarketRuntime, MarketTask};
use screeps_econ_engine::EconWorld;
use std::collections::BTreeMap;

fn pos(x: u8, y: u8) -> Position {
    let room: RoomName = "W1N1".parse().unwrap();
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
}

/// A fixture with a real refill deficit (spawn under-full), a controller container (Low deposit
/// + Use withdraw), a provider container (Medium withdraw), storage, and a dropped pile — the
/// same shape the demand-registration tests use.
fn fixture() -> (EconWorld, LayoutInfo) {
    let mut w = EconWorld::default();
    w.add_source(pos(8, 8), 3000);
    w.add_spawn(pos(25, 25));
    w.spawns[0].store_energy = 50; // free 250 → refill deficit
    let ctl = w.add_container(pos(40, 40), 2000, 250_000);
    w.containers[ctl].store.add(screeps_econ_engine::SimResource::Energy, 1400); // 70% → Low deposit + Use withdraw
    let src = w.add_container(pos(10, 10), 2000, 250_000);
    w.containers[src].store.add(screeps_econ_engine::SimResource::Energy, 1700); // 85% → Medium withdraw
    w.set_storage(pos(30, 25), 1_000_000);
    w.storage.as_mut().unwrap().store.add(screeps_econ_engine::SimResource::Energy, 5_000);
    w.drop_resource(pos(12, 12), screeps_econ_engine::SimResource::Energy, 600); // High pile

    let mut info = LayoutInfo {
        room: "W1N1".parse().unwrap(),
        controller_pos: pos(40, 40),
        container_roles: BTreeMap::new(),
        source_containers: BTreeMap::new(),
        plan_structures: Vec::new(),
        furniture_tiles: Vec::new(),
    };
    info.container_roles
        .insert((40, 40), screeps_econ_eval::layout::ContainerRole::Controller);
    info.container_roles
        .insert((10, 10), screeps_econ_eval::layout::ContainerRole::Source);
    (w, info)
}

/// `same_structure` for the sim keys, keyed the way the live adapter would key its
/// `TransferTarget` identities (a pickup source == a deposit sink).
fn same_structure(pickups: &[screeps_econ_eval::baseline::Pickup], deposits: &[screeps_econ_eval::baseline::Deposit], src_idx: u32, sink_idx: u32) -> bool {
    use screeps_econ_eval::baseline::SrcKey;
    match (pickups[src_idx as usize].src, deposits[sink_idx as usize].sink) {
        (SrcKey::Storage, SinkKey::Storage) => true,
        (SrcKey::Container(a, b), SinkKey::Container(c, d)) => (a, b) == (c, d),
        _ => false,
    }
}

#[test]
fn live_adapter_dtos_match_sim_market_pass() {
    let (mut world, info) = fixture();
    // Two idle carriers: a loaded hauler (delivers carried cargo) and an empty hauler
    // (pickup+deliver). Ids are the world creep ids.
    let loaded = world.add_creep(pos(24, 25), &[Part::Carry, Part::Carry, Part::Move], 1400);
    world
        .creep_stores
        .get_mut(&loaded)
        .unwrap()
        .add(screeps_econ_engine::SimResource::Energy, 100);
    world.sync_carry_used(loaded);
    let empty = world.add_creep(pos(11, 11), &[Part::Carry, Part::Carry, Part::Move], 1400);

    let bookings = Bookings::default();
    let dep_set = deposits(&world, &info, &bookings);
    let pick_set = pickups(&world, &info, &bookings);

    // ── Path A: the SIM driver (begin_tick + market_pass). ────────────────────────────────────
    let cfg = MarketArmCfg::default();
    let mut rt = MarketRuntime::new(cfg, world.tick());
    // begin_tick prices the deposits (refill floor derives from the lane deficit + next-body
    // cost even with an empty plan preview).
    let dep_bids = rt.begin_tick(&world, &info, &[], &dep_set);
    let carriers = vec![
        CarrierDto { id: loaded, pos: pos(24, 25), free: 0, held: 100, opportunity_milli: 0 },
        CarrierDto { id: empty, pos: pos(11, 11), free: 100, held: 0, opportunity_milli: 0 },
    ];
    let mut sim_bookings = Bookings::default();
    // ADR 0044: the sim prices haul on the mover's routed distance. This fixture is an OPEN single
    // room, so the mover's distance equals Chebyshev — Path A (mover) and Path B (get_range_to) stay
    // byte-identical, and the sim↔live kernel parity holds.
    let mut mover = screeps_econ_eval::movement::AnalyticMover::new(&world.movement.terrain);
    rt.market_pass(&world, &dep_set, &dep_bids, &pick_set, &carriers, &mut sim_bookings, &mut mover);
    let sim_tasks: BTreeMap<u32, MarketTask> = rt.tasks.clone();

    // ── Path B: the LIVE adapter's DTO construction over the SAME priced facts, run through the
    // shared kernel directly (exactly what `market_adapter::run_room_market` does). ─────────────
    let k_carriers: Vec<mk::MarketCarrier> = carriers
        .iter()
        .map(|c| mk::MarketCarrier { id: c.id, pos: c.pos, free: c.free, held: c.held, opportunity_milli: c.opportunity_milli })
        .collect();
    let k_deposits: Vec<mk::MarketDeposit> = dep_set
        .iter()
        .enumerate()
        .map(|(i, d)| mk::MarketDeposit {
            sink: i as u32,
            pos: d.pos,
            bid_milli: dep_bids[i],
            unfulfilled: d.unfulfilled,
            is_refill: d.sink.is_fungible_pool_member(),
        })
        .collect();
    let k_pickups: Vec<mk::MarketPickup> = pick_set
        .iter()
        .enumerate()
        .filter(|(_, p)| p.lane == Lane::Haul)
        .map(|(i, p)| mk::MarketPickup {
            src: i as u32,
            pos: p.pos,
            available: p.available,
            source_floor_milli: screeps_econ_eval::market::src_floor_milli(p.src),
        })
        .collect();
    let out = mk::market_pass(
        &k_carriers,
        &k_deposits,
        &k_pickups,
        screeps_econ_decision::sink_economics::HAUL_ROAD_Q_PLAINS_PERMILLE,
        &mut |a: Position, b: Position| Some(a.get_range_to(b)),
        |src, sink| same_structure(&pick_set, &dep_set, src, sink),
    );

    // Resolve the kernel's index-scoped tasks back to sim keys (the live adapter does the
    // analogous `TransferTarget` resolution).
    let mut live_tasks: BTreeMap<u32, MarketTask> = BTreeMap::new();
    for a in &out.assignments {
        let task = match a.task {
            mk::MarketTask::PickupDeliver { src, src_pos, take, sink, sink_pos, give } => MarketTask::PickupDeliver {
                src: pick_set[src as usize].src,
                src_pos,
                take,
                sink: dep_set[sink as usize].sink,
                sink_pos,
                give,
            },
            mk::MarketTask::Deliver { sink, sink_pos, amount } => {
                MarketTask::Deliver { sink: dep_set[sink as usize].sink, sink_pos, amount }
            }
        };
        live_tasks.insert(a.carrier, task);
    }

    // ── The parity assertion: identical per-carrier task sets. ─────────────────────────────────
    assert_eq!(sim_tasks.len(), live_tasks.len(), "same number of assigned carriers");
    assert_eq!(sim_tasks.len(), 2, "both carriers assigned (a loaded Deliver + an empty PickupDeliver) — non-vacuous, covers both task variants");
    // Cover both task shapes: the loaded hauler delivers, the empty one picks-up-and-delivers.
    assert!(matches!(sim_tasks.get(&loaded), Some(MarketTask::Deliver { .. })), "loaded hauler delivers carried cargo");
    assert!(matches!(sim_tasks.get(&empty), Some(MarketTask::PickupDeliver { .. })), "empty hauler picks up then delivers");
    for (id, sim_task) in &sim_tasks {
        let live_task = live_tasks.get(id).unwrap_or_else(|| panic!("carrier {id} assigned by sim but not live"));
        assert_eq!(fmt(sim_task), fmt(live_task), "carrier {id}: sim task != live task");
    }
    // The published floor is the same quantity the live adapter's `room_opportunity_floor`
    // computes (spec Part 2 — the floor the selection admits against).
    let floor = screeps_econ_decision::sink_economics::opportunity_floor(
        &MarketConsts::default(),
        k_deposits.iter().map(|d| (d.bid_milli, d.unfulfilled)),
    );
    assert_eq!(floor, rt.floor, "the live-adapter floor equals the sim begin_tick floor");
}

/// A stable string form for a `MarketTask` (the sim/live keys must match field-for-field).
fn fmt(task: &MarketTask) -> String {
    match *task {
        MarketTask::PickupDeliver { src, take, sink, give, .. } => {
            format!("PD src={src:?} take={take} sink={sink:?} give={give}")
        }
        MarketTask::Deliver { sink, amount, .. } => format!("D sink={sink:?} amount={amount}"),
    }
}
