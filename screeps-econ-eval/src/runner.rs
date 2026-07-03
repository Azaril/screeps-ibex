//! Drive one Family-C scenario end-to-end: the transcribed baseline policy ([`crate::baseline`] +
//! [`crate::workers`]) over the econ-engine world, positions advanced by the analytic movement
//! tier ([`crate::movement`]), metrics streamed by [`crate::metrics`].
//!
//! Per-tick order (deterministic; workers processed in creep-id order — the stand-in for the
//! live ECS iteration, same convention as the engine's resolver):
//! 1. movement-field invalidation (road deaths re-price the memoized traces);
//! 2. demand + bookings (rebuilt per tick — the live re-register-every-tick contract);
//! 3. worker FSM steps (teleports + wear + K3 en-route repairs + intents);
//! 4. K4 spawn requests through the SHARED `spawn_step` queue kernel (head-of-line banking);
//! 5. `resolve_econ_tick`;
//! 6. births→roles, deaths→cleanup, metrics, the deadlock sentinel, early stop on recovery.
//!
//! **Conservation audit failure fails the RUN loudly** (panic — a sim bug must halt everything;
//! the bench's non-zero exit rides on it).

use crate::baseline::{
    self, allowance_for, deposits, effective_min_repair_priority, matched_unfulfilled_hauling,
    opportunistic_repair_target, pickups, resolve_repair_ref, select_delivery_flat_active,
    select_delivery_tiered, select_pickup_and_delivery, spawn_requests, Bookings, PolicyConfig,
    RepairPriority, RoleSpec, SinkKey, SrcKey, Tier, MASK_ALL, MASK_HIGH, MASK_LOW, MASK_MEDIUM,
    MASK_NONE,
};
use crate::layout::LayoutInfo;
use crate::metrics::{
    DeadlockSentinel, Diagnostics, LeakTotals, RecoverConsts, RecoveryTracker,
};
use crate::movement::Mover;
use crate::scenario::{instantiate, EconScenario};
use crate::workers::{Activity, Role, Worker};
use screeps::Position;
use screeps_econ_engine::spawn_queue::{spawn_step, HomeLanes, QueuedSpawn};
use screeps_econ_engine::{
    resolve_econ_tick, EconAction, EconIntents, EconTickReport, EconWorld, SimResource, StructRef,
};
use std::collections::BTreeMap;

/// Run options: the policy arm + recovery constants + caps (+ the fence's permutation hook).
#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    pub cfg: PolicyConfig,
    pub consts: RecoverConsts,
    pub tick_cap: u32,
    /// Reverse each tick's intent list before resolution (the det_reorder fence arm — insertion
    /// order must be non-semantic).
    pub permute_intents: bool,
}

impl RunOptions {
    pub fn new(cfg: PolicyConfig, consts: RecoverConsts, tick_cap: u32) -> Self {
        RunOptions { cfg, consts, tick_cap, permute_intents: false }
    }
}

/// One scenario run's outcome — the per-scenario bench line.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub scenario: String,
    pub seed: u32,
    pub t_star: u32,
    pub recovered_at: Option<u32>,
    /// `recovered_at` or the tick cap (the paired-diff quantity — non-recovery saturates).
    pub effective_t: u32,
    /// η = T*/T_recover, 0 at the cap (ADR §D7; clamped ≤ 1 — the oracle is a lower bound).
    pub eta: f64,
    pub ticks_run: u32,
    pub deadlocked: bool,
    pub diagnostics: Diagnostics,
    pub state_digest: u64,
    pub report_digest: u64,
}

impl RunOutcome {
    pub fn leak(&self) -> LeakTotals {
        self.diagnostics.leak
    }
}

/// The per-run policy driver state.
struct Driver {
    workers: BTreeMap<u32, Worker>,
    /// creep id (minted at spawn START) → role, until birth.
    pending_roles: BTreeMap<u32, RoleSpec>,
}

impl Driver {
    fn roles(&self) -> BTreeMap<u32, RoleSpec> {
        let mut all: BTreeMap<u32, RoleSpec> = self
            .workers
            .iter()
            .map(|(&id, w)| {
                (
                    id,
                    match &w.role {
                        Role::Harvester { source_idx } => RoleSpec::Harvester { source_idx: *source_idx },
                        Role::Hauler => RoleSpec::Hauler,
                    },
                )
            })
            .collect();
        // In-flight spawns count toward the desired-counts (the live mission tracks creeps from
        // the spawn callback, i.e. from spawn START).
        for (&id, r) in &self.pending_roles {
            all.insert(id, r.clone());
        }
        all
    }
}

fn creep_energy(world: &EconWorld, id: u32) -> u32 {
    world.creep_stores.get(&id).map(|s| s.amount(SimResource::Energy)).unwrap_or(0)
}

fn creep_free(world: &EconWorld, id: u32) -> u32 {
    world.creep_stores.get(&id).map(|s| s.free()).unwrap_or(0)
}

fn sink_struct_ref(world: &EconWorld, sink: SinkKey) -> Option<StructRef> {
    match sink {
        SinkKey::Spawn(i) => (i < world.spawns.len()).then_some(StructRef::Spawn(i)),
        SinkKey::Extension(i) => (i < world.extensions.len()).then_some(StructRef::Extension(i)),
        SinkKey::Storage => world.storage.as_ref().map(|_| StructRef::Storage),
        SinkKey::Container(x, y) => world
            .containers
            .iter()
            .position(|c| c.pos.x().u8() == x && c.pos.y().u8() == y)
            .map(StructRef::Container),
    }
}

/// Book a worker's in-flight commitments (recursing through Travel's `then`).
fn book_activity(bookings: &mut Bookings, a: &Activity) {
    match a {
        Activity::Travel { then, .. } => book_activity(bookings, then),
        Activity::Deliver { sink, amount } => {
            *bookings.deposits.entry(*sink).or_insert(0) += amount;
        }
        Activity::PickupFor { src, take, sink, give, .. } => {
            *bookings.pickups.entry(*src).or_insert(0) += take;
            *bookings.deposits.entry(*sink).or_insert(0) += give;
        }
        _ => {}
    }
}

/// Reduce the delivery amount buried under travel legs by `cost` — the live
/// `consume_resource_from_deposits` (transfersystem.rs:1124-1134): en-route repair energy comes
/// OUT OF THE TICKET, the sink receives less than promised (the S1 disease's second half).
fn consume_from_delivery(a: &mut Activity, cost: u32) {
    match a {
        Activity::Travel { then, .. } => consume_from_delivery(then, cost),
        Activity::Deliver { amount, .. } => *amount = amount.saturating_sub(cost),
        Activity::PickupFor { give, .. } => *give = give.saturating_sub(cost),
        _ => {}
    }
}

/// Run one scenario. Panics on any conservation violation (loud failure — module docs).
pub fn run_scenario(sc: &EconScenario, opts: &RunOptions) -> RunOutcome {
    let (mut world, terrain, info) = instantiate(sc);
    let mut mover = crate::movement::AnalyticMover::new(&terrain);
    run_world(sc, &mut world, &mut mover, &info, opts)
}

/// The inner loop over an already-instantiated world (the oracle/test seam).
pub fn run_world(
    sc: &EconScenario,
    world: &mut EconWorld,
    mover: &mut dyn Mover,
    info: &LayoutInfo,
    opts: &RunOptions,
) -> RunOutcome {
    let t_star = crate::oracle::t_star(world, mover, info, &opts.consts);
    let mut driver = Driver { workers: BTreeMap::new(), pending_roles: BTreeMap::new() };

    // Pre-existing (skeleton) creeps: harvesters on the least-staffed source (deterministic).
    let initial: Vec<u32> = world.movement.creeps.iter().map(|c| c.id).collect();
    for id in initial {
        driver.workers.insert(id, Worker::new(Role::Harvester { source_idx: 0 }));
    }

    let mut tracker = RecoveryTracker::new(opts.consts, world.sources.len() as u32);
    let mut sentinel = DeadlockSentinel::default();
    let mut diagnostics = Diagnostics::default();
    let mut report_digest: u64 = 0xcbf2_9ce4_8422_2325;
    let mut ticks_run = 0u32;
    let mut roads_before = world.roads.len();

    for _ in 0..opts.tick_cap {
        // ── 1. Re-price the movement field if a road died last tick. ────────────────────────────
        if world.roads.len() != roads_before {
            mover.invalidate_from(&world.movement.terrain);
            roads_before = world.roads.len();
        }

        // ── 2. Demand + bookings (rebuilt per tick). ────────────────────────────────────────────
        let mut bookings = Bookings::default();
        for w in driver.workers.values() {
            book_activity(&mut bookings, &w.activity);
        }

        // ── 3. Worker FSM steps (creep-id order). ───────────────────────────────────────────────
        let mut intents = EconIntents::new();
        let mut moved_any = false;
        let allowance = allowance_for(&opts.cfg, world);
        let ids: Vec<u32> = driver.workers.keys().copied().collect();
        for id in ids {
            let step = step_worker(world, mover, info, allowance, &mut bookings, &mut driver, id, &mut intents);
            moved_any |= step;
        }
        let actions_emitted = intents.actions.len() as u32;

        // ── 4. K4 spawning through the shared queue kernel. The hauling stat is the live
        // supply↔demand MIN-MATCH (`matched_unfulfilled_hauling` — the transfersystem.rs
        // :2249-2337 three-stage match, not demand alone), so a drained S0=0 world spawns no
        // hauler for unhaulable demand. ─────────────────────────────────────────────────────────
        let deposit_set = deposits(world, info, &bookings);
        let pickup_set = pickups(world, info, &bookings);
        let unfulfilled_hauling = matched_unfulfilled_hauling(&deposit_set, &pickup_set);
        let plans = spawn_requests(world, &driver.roles(), unfulfilled_hauling);
        let idle_spawn_indices: Vec<usize> =
            (0..world.spawns.len()).filter(|&i| world.spawns[i].spawning.is_none()).collect();
        let mut pending_by_spawn: BTreeMap<usize, RoleSpec> = BTreeMap::new();
        if !plans.is_empty() {
            let mut lanes = HomeLanes {
                idle_spawns: idle_spawn_indices.len() as u32,
                available_energy: world.room_spawn_energy(),
                energy_capacity: baseline::spawn_lane_capacity(world),
            };
            let queue: Vec<QueuedSpawn> = plans
                .iter()
                .enumerate()
                .map(|(i, p)| QueuedSpawn {
                    priority: p.priority,
                    body_cost: screeps_econ_engine::constants::body_cost(&p.body),
                    part_count: p.body.len() as u32,
                    id: i as u64,
                })
                .collect();
            let started = spawn_step(&mut lanes, &queue);
            // Head-of-line blocked-tick diagnostic: an idle spawn + a capacity-affordable request
            // existed, but nothing started (the queue banked — the S6 window).
            if started.is_empty()
                && !idle_spawn_indices.is_empty()
                && queue.iter().any(|q| q.body_cost <= lanes.energy_capacity)
            {
                diagnostics.spawn_energy_blocked_ticks += 1;
            }
            for (slot, s) in started.iter().enumerate() {
                let spawn_idx = idle_spawn_indices[slot];
                let plan = &plans[s.id as usize];
                intents.spawn(spawn_idx, plan.body.clone());
                pending_by_spawn.insert(spawn_idx, plan.role.clone());
            }
        }

        // ── 5. Resolve (with the fence's permutation hook). ─────────────────────────────────────
        if opts.permute_intents {
            intents.actions.reverse();
        }
        let report = resolve_econ_tick(world, &intents);
        assert!(
            report.conservation.is_empty(),
            "{}: conservation violated at tick {}: {:?}",
            sc.name,
            report.tick,
            report.conservation
        );
        ticks_run = report.tick + 1;
        report_digest = fold_report(report_digest, &report);

        // ── 6. Post-resolve bookkeeping. ────────────────────────────────────────────────────────
        for &(spawn_idx, creep_id) in &report.spawns_started {
            if let Some(role) = pending_by_spawn.get(&spawn_idx) {
                driver.pending_roles.insert(creep_id, role.clone());
            }
        }
        for &id in &report.births {
            if let Some(role) = driver.pending_roles.remove(&id) {
                let role = match role {
                    RoleSpec::Harvester { source_idx } => Role::Harvester { source_idx },
                    RoleSpec::Hauler => Role::Hauler,
                };
                driver.workers.insert(id, Worker::new(role));
            }
        }
        for &id in &report.deaths {
            driver.workers.remove(&id);
        }

        // Diagnostics + recovery + the sentinel.
        let capacity = baseline::spawn_lane_capacity(world);
        let lane_energy = world.room_spawn_energy();
        diagnostics.extension_deficit_integral += capacity.saturating_sub(lane_energy) as u64;
        diagnostics.spawn_ticks += world.spawns.len() as u64;
        diagnostics.spawn_idle_ticks +=
            world.spawns.iter().filter(|s| s.spawning.is_none()).count() as u64;
        diagnostics.leak.roads += report.repair_leak.roads;
        diagnostics.leak.containers += report.repair_leak.containers;
        diagnostics.leak.other += report.repair_leak.other;
        tracker.observe(world, &report);

        let progressed = report.ledger.harvested > 0
            || report.ledger.spawn_self_charge > 0
            || report.ledger.repair_total() > 0
            || !report.births.is_empty()
            || !report.spawns_started.is_empty()
            || !report.deaths.is_empty()
            || moved_any
            || actions_emitted > report.rejected_actions
            // A running source-regen timer IS progress: the world is working toward income
            // (drained sources otherwise read as a false deadlock for up to 300 ticks).
            || world.sources.iter().any(|s| s.regen_at.is_some());
        let demand = lane_energy < capacity || !plans.is_empty();
        sentinel.observe(report.tick, progressed, demand);

        if tracker.recovered_at.is_some() || sentinel.fired_at.is_some() {
            break;
        }
    }

    let deadlocked = sentinel.fired_at.is_some();
    let recovered_at = if deadlocked { None } else { tracker.recovered_at };
    let effective_t = recovered_at.unwrap_or(opts.tick_cap);
    let eta = match recovered_at {
        Some(t) if t > 0 => (t_star as f64 / t as f64).min(1.0),
        _ => 0.0, // η = 0 at the cap AND on the deadlock sentinel (ADR hard gate)
    };
    RunOutcome {
        scenario: sc.name.clone(),
        seed: sc.seed,
        t_star,
        recovered_at,
        effective_t,
        eta,
        ticks_run,
        deadlocked,
        diagnostics,
        state_digest: world.state_digest(),
        report_digest,
    }
}

/// One worker's FSM step. Returns whether the creep MOVED (teleported) this tick.
#[allow(clippy::too_many_arguments)]
fn step_worker(
    world: &mut EconWorld,
    mover: &mut dyn Mover,
    info: &LayoutInfo,
    allowance: baseline::RepairAllowance,
    bookings: &mut Bookings,
    driver: &mut Driver,
    id: u32,
    intents: &mut EconIntents,
) -> bool {
    let Some(pos) = world.creep(id).map(|c| c.pos) else {
        return false;
    };
    let worker = driver.workers.get_mut(&id).expect("stepped worker exists");
    let is_harvester = matches!(worker.role, Role::Harvester { .. });
    let tick = world.tick();

    // The K3 opportunistic-repair admission for this worker (min priority by role):
    // harvesters ≥ Medium (harvest.rs:225, :264-266 — Harvest AND Delivery states);
    // haulers ≥ Low BUT gated by the live `allow_repair = max_distance > 0`
    // (missions/haul.rs:295) — max_distance == 0 for Family C's single-room missions, so the
    // hauler arm is transcribed and INERT here (the ADR §1 table's "REMOTE haulers"; documented).
    let drive_by_min = if is_harvester {
        Some(effective_min_repair_priority(RepairPriority::Medium, allowance))
    } else {
        None // local hauler: allow_repair = false
    };

    let mut moved = false;
    match std::mem::replace(&mut worker.activity, Activity::Idle) {
        Activity::Idle => {
            let deposit_set = deposits(world, info, bookings);
            let held = creep_energy(world, id);
            match worker.role {
                // The LIVE harvester Idle chain, in ORDER (jobs/harvest.rs:104-219; a
                // local-source harvester doubles as a home hauler — `allow_haul`,
                // harvest.rs:97-104): as-hauler HIGH|NONE → HARVEST-FIRST → as-hauler
                // MEDIUM|LOW|NONE → HIGH delivery → full-repair → M/L/None delivery → wait(5).
                // The delivery/repair chain is therefore only reachable with a FULL store or a
                // DRAINED source. (The upgrade/build arms are absent from the M1 intent
                // vocabulary — M2.)
                Role::Harvester { source_idx } => {
                    let free = creep_free(world, id);
                    let source_has_energy =
                        world.sources.get(source_idx).map(|s| s.energy > 0).unwrap_or(false);
                    let pickup_set = pickups(world, info, bookings);
                    // (1) As-hauler arm 1 (harvest.rs:104-125): pickup+delivery at HIGH|NONE,
                    // free-capacity sized (haulbehavior.rs:368-397) — a harvester with stocked
                    // storage and a spawn deficit HAULS before it harvests.
                    let arm1 = if free > 0 {
                        select_pickup_and_delivery(pos, free, &deposit_set, &pickup_set, MASK_HIGH | MASK_NONE)
                    } else {
                        None
                    };
                    // (3) As-hauler arm 2 (harvest.rs:137-158): MEDIUM|LOW|NONE — evaluated
                    // lazily only when the harvest-first arm did not fire.
                    let arm2 = |bookings: &Bookings| {
                        let pickup_set = pickups(world, info, bookings);
                        select_pickup_and_delivery(pos, free, &deposit_set, &pickup_set, MASK_MEDIUM | MASK_LOW | MASK_NONE)
                    };
                    if let Some((p, d, amount)) = arm1 {
                        *bookings.pickups.entry(p.src).or_insert(0) += amount;
                        *bookings.deposits.entry(d.sink).or_insert(0) += amount;
                        let act = Activity::PickupFor { src: p.src, take: amount, sink: d.sink, sink_pos: d.pos, give: amount };
                        worker.activity = travel_then(world, mover, id, pos, p.pos, 1, act, tick);
                    } else if free > 0 && source_has_energy {
                        // (2) HARVEST-FIRST (harvest.rs:127-129 / harvestbehavior.rs:40-44):
                        // free capacity + a non-drained source ⇒ back to harvesting.
                        let src_pos = world.sources[source_idx].pos;
                        worker.activity = travel_then(world, mover, id, pos, src_pos, 1, Activity::Harvest, tick);
                    } else if let Some((p, d, amount)) = if free > 0 { arm2(bookings) } else { None } {
                        *bookings.pickups.entry(p.src).or_insert(0) += amount;
                        *bookings.deposits.entry(d.sink).or_insert(0) += amount;
                        let act = Activity::PickupFor { src: p.src, take: amount, sink: d.sink, sink_pos: d.pos, give: amount };
                        worker.activity = travel_then(world, mover, id, pos, p.pos, 1, act, tick);
                    } else if held > 0 {
                        if let Some((sink, spos, amount)) =
                            select_delivery_tiered(pos, &deposit_set, held, &[Tier::High])
                        {
                            // (4) HIGH delivery (harvest.rs:166-175).
                            *bookings.deposits.entry(sink).or_insert(0) += amount;
                            worker.activity = travel_then(world, mover, id, pos, spos, 1, Activity::Deliver { sink, amount }, tick);
                        } else if let Some((target, tpos)) = baseline::full_repair_target(
                            world,
                            effective_min_repair_priority(RepairPriority::Medium, allowance),
                        ) {
                            // (5) idle full-repair ≥ Medium (harvest.rs:178-193, allowance-gated).
                            worker.activity =
                                travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                        } else if let Some((sink, spos, amount)) = select_delivery_tiered(
                            pos,
                            &deposit_set,
                            held,
                            &[Tier::Medium, Tier::Low, Tier::NonePri],
                        ) {
                            // (6) Medium → Low → None deliveries (harvest.rs:194-210).
                            *bookings.deposits.entry(sink).or_insert(0) += amount;
                            worker.activity = travel_then(world, mover, id, pos, spos, 1, Activity::Deliver { sink, amount }, tick);
                        } else {
                            // (7) wait(5) (harvest.rs:219).
                            worker.activity = Activity::Wait { until: tick + 5 };
                        }
                    } else {
                        // Empty + drained source + no haul work: wait(5) (harvest.rs:219).
                        worker.activity = Activity::Wait { until: tick + 5 };
                    }
                }
                Role::Hauler => {
                    if held > 0 {
                        // S3 verbatim: carried cargo → nearest FLAT ACTIVE; the None tier
                        // (storage dump) only when no ACTIVE demand exists.
                        if let Some((sink, spos, amount)) =
                            select_delivery_flat_active(pos, &deposit_set, held).or_else(|| {
                                select_delivery_tiered(pos, &deposit_set, held, &[Tier::NonePri])
                            })
                        {
                            *bookings.deposits.entry(sink).or_insert(0) += amount;
                            worker.activity =
                                travel_then(world, mover, id, pos, spos, 1, Activity::Deliver { sink, amount }, tick);
                        } else {
                            worker.activity = Activity::Wait { until: tick + 5 };
                        }
                    } else {
                        let pickup_set = pickups(world, info, bookings);
                        let capacity = creep_free(world, id);
                        if let Some((p, d, amount)) =
                            select_pickup_and_delivery(pos, capacity, &deposit_set, &pickup_set, MASK_ALL)
                        {
                            *bookings.pickups.entry(p.src).or_insert(0) += amount;
                            *bookings.deposits.entry(d.sink).or_insert(0) += amount;
                            let act = Activity::PickupFor {
                                src: p.src,
                                take: amount,
                                sink: d.sink,
                                sink_pos: d.pos,
                                give: amount,
                            };
                            worker.activity = travel_then(world, mover, id, pos, p.pos, 1, act, tick);
                        } else {
                            worker.activity = Activity::Wait { until: tick + 5 };
                        }
                    }
                }
            }
        }
        Activity::Travel { trace, idx, then } => {
            // Advance one trace tick: teleport, book wear per tile ENTERED, fire the K3 en-route
            // repair (harvesters; energy comes out of the delivery ticket — consume_from_deposits).
            let new_pos = trace.get(idx).copied().unwrap_or(pos);
            if new_pos != pos {
                let parts = world.creep(id).map(|c| c.body.parts.len() as u32).unwrap_or(0);
                if let Some(c) = world.creep_mut(id) {
                    c.pos = new_pos;
                }
                world.apply_road_wear(new_pos, parts);
                moved = true;
            }
            let mut then = then;
            if let Some(min) = drive_by_min {
                let held = creep_energy(world, id);
                if held > 0 {
                    if let Some(target) = opportunistic_repair_target(world, new_pos, min) {
                        if let Some(sref) = resolve_repair_ref(world, target) {
                            let (hits, hits_max) = repair_hits(world, target);
                            let work = world
                                .creep(id)
                                .map(|c| c.body.alive_part_count(screeps::Part::Work))
                                .unwrap_or(0);
                            let cost = baseline::repair_energy_consumed(work, held, hits, hits_max);
                            if cost > 0 {
                                intents.act(id, EconAction::Repair { target: sref });
                                consume_from_delivery(&mut then, cost);
                            }
                        }
                    }
                }
            }
            let next_idx = idx + 1;
            worker.activity = if next_idx >= trace.len() {
                *then
            } else {
                Activity::Travel { trace, idx: next_idx, then }
            };
        }
        Activity::Harvest => {
            let full = creep_free(world, id) == 0;
            // The live tick_harvest exits on a DRAINED source too (the harvest-Err arm → Idle;
            // regen brings the harvester back through the Idle chain's harvest-first arm) —
            // parking at an empty source until full is not live behavior.
            let drained = match worker.role {
                Role::Harvester { source_idx } => {
                    world.sources.get(source_idx).map(|s| s.energy == 0).unwrap_or(true)
                }
                Role::Hauler => true,
            };
            if full || drained {
                worker.activity = Activity::Idle; // the Idle chain runs next tick
            } else {
                // K3: the opportunistic repair MASKS the harvest (shared Pipeline A —
                // jobs/actions.rs:27-31; harvest.rs:225's tick order: repair, then harvest).
                let mut repaired = false;
                if let Some(min) = drive_by_min {
                    let held = creep_energy(world, id);
                    if held > 0 {
                        if let Some(target) = opportunistic_repair_target(world, pos, min) {
                            if let Some(sref) = resolve_repair_ref(world, target) {
                                intents.act(id, EconAction::Repair { target: sref });
                                repaired = true;
                            }
                        }
                    }
                }
                if !repaired {
                    if let Role::Harvester { source_idx } = worker.role {
                        intents.act(id, EconAction::Harvest { source_idx });
                    }
                }
                worker.activity = Activity::Harvest;
            }
        }
        Activity::Deliver { sink, amount } => {
            let held = creep_energy(world, id);
            let target = sink_struct_ref(world, sink);
            match target {
                Some(sref) if held > 0 && amount > 0 => {
                    // The drive-by repair DURING delivery (harvest.rs:271-276): Pipeline A repair
                    // + Pipeline D transfer the same tick, the repair cost consumed from the
                    // ticket so the Transfer never over-asks (baseline.rs exact-split contract).
                    let mut transfer_amount = amount.min(held);
                    if let Some(min) = drive_by_min {
                        if let Some(target) = opportunistic_repair_target(world, pos, min) {
                            if let Some(rref) = resolve_repair_ref(world, target) {
                                let (hits, hits_max) = repair_hits(world, target);
                                let work = world
                                    .creep(id)
                                    .map(|c| c.body.alive_part_count(screeps::Part::Work))
                                    .unwrap_or(0);
                                let cost = baseline::repair_energy_consumed(work, held, hits, hits_max);
                                if cost > 0 {
                                    intents.act(id, EconAction::Repair { target: rref });
                                    transfer_amount = transfer_amount.min(held.saturating_sub(cost));
                                }
                            }
                        }
                    }
                    if transfer_amount > 0 {
                        intents.act(
                            id,
                            EconAction::Transfer {
                                target: sref,
                                resource: SimResource::Energy,
                                amount: transfer_amount,
                            },
                        );
                    }
                    // Harvesters continue through FinishedDelivery (harvest.rs:279 →
                    // finished_delivery); haulers go Idle (haul.rs:255 → HaulState::idle).
                    worker.activity =
                        if is_harvester { Activity::PostDelivery } else { Activity::Idle };
                }
                _ => {
                    // Target died / nothing aboard: replan (harvesters through the same
                    // FinishedDelivery re-try, mirroring the invalid-ticket completion path).
                    worker.activity =
                        if is_harvester { Activity::PostDelivery } else { Activity::Idle };
                }
            }
        }
        Activity::PostDelivery => {
            // Live FinishedDelivery (harvest.rs:283-310): with leftover cargo, re-try deliveries
            // across ALL tiers in order (High→Medium→Low→None, nearest within each — the
            // ALL_TRANSFER_PRIORITIES iteration), NO repair arm; else fall to Idle.
            let held = creep_energy(world, id);
            if held > 0 {
                let deposit_set = deposits(world, info, bookings);
                if let Some((sink, spos, amount)) = select_delivery_tiered(
                    pos,
                    &deposit_set,
                    held,
                    &[Tier::High, Tier::Medium, Tier::Low, Tier::NonePri],
                ) {
                    *bookings.deposits.entry(sink).or_insert(0) += amount;
                    worker.activity =
                        travel_then(world, mover, id, pos, spos, 1, Activity::Deliver { sink, amount }, tick);
                } else {
                    worker.activity = Activity::Idle;
                }
            } else {
                worker.activity = Activity::Idle;
            }
        }
        Activity::PickupFor { src, take, sink, sink_pos: spos, give } => {
            let free = creep_free(world, id);
            let emitted = match src {
                SrcKey::Dropped(x, y) => world
                    .dropped
                    .iter()
                    .position(|d| {
                        d.pos.x().u8() == x && d.pos.y().u8() == y && d.resource == SimResource::Energy
                    })
                    .map(|i| {
                        intents.act(id, EconAction::Pickup { dropped_idx: i });
                    })
                    .is_some(),
                SrcKey::Storage => {
                    if world.storage.is_some() {
                        intents.act(
                            id,
                            EconAction::Withdraw {
                                target: StructRef::Storage,
                                resource: SimResource::Energy,
                                amount: take.min(free),
                            },
                        );
                        true
                    } else {
                        false
                    }
                }
                SrcKey::Container(x, y) => world
                    .containers
                    .iter()
                    .position(|c| c.pos.x().u8() == x && c.pos.y().u8() == y)
                    .map(|i| {
                        intents.act(
                            id,
                            EconAction::Withdraw {
                                target: StructRef::Container(i),
                                resource: SimResource::Energy,
                                amount: take.min(free),
                            },
                        );
                    })
                    .is_some(),
            };
            if emitted {
                // Withdraw resolves this tick; head to the delivery next tick.
                let body = world.creep(id).unwrap().body.clone();
                let carry = world
                    .creep_stores
                    .get(&id)
                    .map(|s| s.total() + take.min(free))
                    .unwrap_or(0);
                if let Some(trace) = mover.trace(pos, spos, 1, &body, carry) {
                    worker.activity = Activity::Travel {
                        trace,
                        idx: 0,
                        then: Box::new(Activity::Deliver { sink, amount: give }),
                    };
                } else {
                    worker.activity = Activity::Idle;
                }
            } else {
                worker.activity = Activity::Idle; // the source died: replan
            }
        }
        Activity::FullRepair { target } => {
            let held = creep_energy(world, id);
            // The live FinishedRepair chain (harvest.rs:335-357): a healed (or dead) target rolls
            // straight into the NEXT ≥Medium full-repair target while cargo remains — the
            // allowance re-applied each time; only an empty store (pragmatic guard: live's
            // energyless Repair intents no-op) or no target falls back to Idle.
            let chain = |worker: &mut Worker, world: &EconWorld, mover: &mut dyn Mover| {
                if held > 0 {
                    if let Some((next, tpos)) = baseline::full_repair_target(
                        world,
                        effective_min_repair_priority(RepairPriority::Medium, allowance),
                    ) {
                        worker.activity =
                            travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target: next }, tick);
                        return;
                    }
                }
                worker.activity = Activity::Idle;
            };
            match resolve_repair_ref(world, target) {
                Some(sref) if held > 0 => {
                    let (hits, hits_max) = repair_hits(world, target);
                    if hits >= hits_max {
                        chain(worker, world, mover); // healed: FinishedRepair → next target
                    } else {
                        intents.act(id, EconAction::Repair { target: sref });
                        worker.activity = Activity::FullRepair { target };
                    }
                }
                None if held > 0 => {
                    chain(worker, world, mover); // target died: FinishedRepair → next target
                }
                _ => {
                    worker.activity = Activity::Idle; // out of energy
                }
            }
        }
        Activity::Wait { until } => {
            worker.activity = if tick >= until { Activity::Idle } else { Activity::Wait { until } };
        }
    }
    moved
}

fn repair_hits(world: &EconWorld, r: baseline::RepairRef) -> (u32, u32) {
    match r {
        baseline::RepairRef::Road(x, y) => world
            .roads
            .iter()
            .find(|rd| rd.pos.x().u8() == x && rd.pos.y().u8() == y)
            .map(|rd| (rd.hits, rd.hits_max))
            .unwrap_or((0, 0)),
        baseline::RepairRef::Container(x, y) => world
            .containers
            .iter()
            .find(|c| c.pos.x().u8() == x && c.pos.y().u8() == y)
            .map(|c| (c.hits, screeps_econ_engine::constants::CONTAINER_HITS))
            .unwrap_or((0, 0)),
    }
}

/// Travel to within `range` of `to`, then do `then` (empty trace ⇒ `then` immediately NEXT tick).
#[allow(clippy::too_many_arguments)] // the FSM transition's full context; a struct would hide it
fn travel_then(
    world: &EconWorld,
    mover: &mut dyn Mover,
    id: u32,
    from: Position,
    to: Position,
    range: u8,
    then: Activity,
    tick: u32,
) -> Activity {
    let body = world.creep(id).unwrap().body.clone();
    let carry = world.creep_stores.get(&id).map(|s| s.total()).unwrap_or(0);
    match mover.trace(from, to, range, &body, carry) {
        Some(trace) if trace.is_empty() => then,
        Some(trace) => Activity::Travel { trace, idx: 0, then: Box::new(then) },
        None => Activity::Wait { until: tick + 5 }, // unreachable target: back off + replan
    }
}

/// FNV fold of a tick report's decision-relevant fields (the fence instrument — mirrors the
/// econ-engine fence's `fold_report`, extended with the M1 fields).
pub fn fold_report(mut h: u64, r: &EconTickReport) -> u64 {
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
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::RecoverConsts;
    use crate::movement::AnalyticMover;
    use crate::scenario::{CreepInit, EconScenario};
    use screeps::{Part, RoomCoordinate, RoomName};
    use screeps_sim_core::SimTerrain;

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    /// A synthetic collapse room: drained spawn, one source, one empty bootstrap harvester beside
    /// the spawn, and (optionally) stocked storage — the minimal world for the FSM-order pins.
    fn synthetic(storage_energy: u32) -> (EconWorld, SimTerrain, LayoutInfo) {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 25), 3000);
        let s = w.add_spawn(pos(25, 25));
        w.spawns[s].store_energy = 0;
        if storage_energy > 0 {
            w.set_storage(pos(30, 25), 1_000_000);
            w.storage.as_mut().unwrap().store.add(SimResource::Energy, storage_energy);
        }
        w.add_creep(pos(26, 25), &[Part::Move, Part::Move, Part::Carry, Part::Work], 100_000);
        let info = LayoutInfo {
            room: "W1N1".parse().unwrap(),
            controller_pos: pos(40, 40),
            container_roles: BTreeMap::new(),
            source_containers: BTreeMap::new(),
        };
        let terrain = w.movement.terrain.clone();
        (w, terrain, info)
    }

    fn sc(tick_cap: u32) -> EconScenario {
        EconScenario {
            name: "synthetic".into(),
            layout_room: "synthetic".into(),
            rcl: 3,
            storage_energy: 0,
            creeps: CreepInit::Wiped,
            road_health_pct: 100,
            downgrade_clock_pct: 100,
            tick_cap,
            seed: 1,
            bait: false,
        }
    }

    fn run(world: &mut EconWorld, terrain: &SimTerrain, info: &LayoutInfo, ticks: u32) {
        let mut mover = AnalyticMover::new(terrain);
        let opts = RunOptions::new(PolicyConfig::default(), RecoverConsts::default(), ticks);
        run_world(&sc(ticks), world, &mut mover, info, &opts);
    }

    /// THE as-hauler regression pin (harvest.rs:104-125 mirrored): an EMPTY harvester with
    /// stocked storage and a spawn deficit selects the storage→spawn haul BEFORE harvesting —
    /// the storage drains, the spawn refills beyond self-charge, and the SOURCE IS UNTOUCHED.
    #[test]
    fn empty_harvester_hauls_storage_to_spawn_before_harvesting() {
        let (mut w, terrain, info) = synthetic(5_000);
        run(&mut w, &terrain, &info, 30);
        assert_eq!(w.sources[0].energy, 3000, "the source was never harvested — hauling came first");
        let storage = w.storage.as_ref().unwrap().store.amount(SimResource::Energy);
        assert!(storage < 5_000, "storage was withdrawn from ({storage})");
        // Self-charge alone yields ≤ 30 energy in 30 ticks; the delivered 50-cargo shows on top.
        assert!(
            w.room_spawn_energy() >= 50,
            "the spawn lane got the hauled cargo (at {})",
            w.room_spawn_energy()
        );
    }

    /// The harvest-first pin (harvest.rs:127-129): with NO haulable supply (S0 = 0, no piles),
    /// the same empty harvester goes to its source and harvests — and does NOT idle at the spawn.
    #[test]
    fn empty_harvester_without_haul_work_harvests_first() {
        let (mut w, terrain, info) = synthetic(0);
        run(&mut w, &terrain, &info, 30);
        assert!(
            w.sources[0].energy < 3000,
            "the harvest-first arm fired (source at {})",
            w.sources[0].energy
        );
    }
}
