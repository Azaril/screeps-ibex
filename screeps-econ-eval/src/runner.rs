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
    select_delivery_tiered, select_fill_pickup, select_pickup_and_delivery, spawn_requests,
    upgrade_about_to_run_dry, upgrader_pickup_anchor, upgrader_should_allow_harvest, Bookings,
    PolicyConfig, RepairPriority, RoleSpec, SinkKey, SrcKey, Tier, MASK_ALL, MASK_HIGH, MASK_LOW,
    MASK_MEDIUM, MASK_NONE,
};
use crate::layout::LayoutInfo;
use crate::market::{self, CarrierDto, GapStats, MarketRuntime, MarketTask};
use crate::metrics::{
    DeadlockSentinel, Diagnostics, LeakTotals, RecoverConsts, RecoveryTracker,
};
use crate::movement::Mover;
use crate::scenario::{instantiate, EconScenario};
use crate::workers::{Activity, Role, Worker};
use screeps::{Part, Position};
use screeps_econ_engine::spawn_queue::{spawn_step, HomeLanes, QueuedSpawn};
use screeps_econ_engine::{
    resolve_econ_tick, EconAction, EconIntents, EconTickReport, EconWorld, SimResource, StructRef,
    StructureKind,
};
use std::collections::BTreeMap;

/// What ends a run (M2 — the per-family stop conditions; every goal also stops on the deadlock
/// sentinel and the tick cap).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunGoal {
    /// Family C: stop at the recovered state (RecoveryTracker — lane-only, the #7 decision).
    Recover,
    /// Family D (review B5): stop only when recovered AND the downgrade clock is back at/above
    /// half-max (the upkeep mission's own safe threshold, missions/upgrade.rs:94) — ending at
    /// recovery alone made "no downgrade" VACUOUS for every scenario whose 10% clock outlived
    /// the recovery horizon (7-9/10 of the M2 catalog). T_recover still reads from the tracker;
    /// the run continues until the clock question resolves (safe, downgraded-then-safe, or cap).
    RecoverThenClockSafe,
    /// Family G: stop when the controller reaches `target` (T_RCL(N)).
    Rcl { target: u8 },
    /// Family S: run the full tick cap (the guard-rail horizon).
    Horizon,
}

/// Run options: the policy arm + recovery constants + caps (+ the fence's permutation hook).
#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    pub cfg: PolicyConfig,
    pub consts: RecoverConsts,
    pub tick_cap: u32,
    /// Reverse each tick's intent list before resolution (the det_reorder fence arm — insertion
    /// order must be non-semantic).
    pub permute_intents: bool,
    /// The stop condition (M2). `Recover` = the M1 behavior.
    pub goal: RunGoal,
    /// The live per-50-tick construction pass's PHASE (`game::time().is_multiple_of(50)`,
    /// construction.rs:452 — live rooms sit at arbitrary phase; scenarios seed it 0..49).
    pub construction_phase: u32,
}

impl RunOptions {
    pub fn new(cfg: PolicyConfig, consts: RecoverConsts, tick_cap: u32) -> Self {
        RunOptions {
            cfg,
            consts,
            tick_cap,
            permute_intents: false,
            goal: RunGoal::Recover,
            construction_phase: 0,
        }
    }

    pub fn with_goal(mut self, goal: RunGoal) -> Self {
        self.goal = goal;
        self
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
    /// η = T*/T_recover, 0 at the cap, CLAMPED ≤ 1 (the H-semantics value — H lives in (0,1]).
    /// For `RunGoal::Rcl` this is T*_RCL(target)/T_RCL(target) instead.
    pub eta: f64,
    /// The UNCLAMPED T*/T ratio (review A1 — the oracle-sanity gate's instrument: `eta_raw >
    /// 1 + ε` means the "lower bound" exceeded an achieved T, an oracle bug the clamped `eta`
    /// can never show).
    pub eta_raw: f64,
    pub ticks_run: u32,
    pub deadlocked: bool,
    pub diagnostics: Diagnostics,
    pub state_digest: u64,
    pub report_digest: u64,
    // ── M2 per-family extras ────────────────────────────────────────────────────────────────────
    /// Level → the first tick the controller REACHED it (Family G's T_RCL curve; levels regained
    /// after a downgrade do not overwrite the first crossing).
    pub t_rcl: BTreeMap<u8, u32>,
    /// Downgrade events during the run (Family D's levels_lost).
    pub levels_lost: u32,
    /// The controller's final (level, progress).
    pub final_controller: Option<(u8, u32)>,
    /// Sampled road-stock health: (tick, Σ hits, Σ hits_max) every 100 ticks (Family S — the
    /// must-not-collapse trajectory; exact integers, ratio computed at report time).
    pub road_stock: Vec<(u32, u64, u64)>,
    /// Task assignments (Idle-chain selections of a non-Wait activity) — flap_rate =
    /// assignments per kilotick.
    pub assignments: u64,
    /// Economy actions emitted across the run (intent-count diagnostics).
    pub intents_emitted: u64,
    /// Completed refill-deficit episode lengths, ticks (Family S refill-latency distribution).
    pub deficit_episodes: Vec<u32>,
    /// Review A3 — the length of a deficit episode still OPEN at run end (a terminal permanent
    /// deficit must not vanish from the latency statistics by censoring).
    pub deficit_open_at_end: Option<u32>,
    // ── M4 market diagnostics (zero on non-market arms) ─────────────────────────────────────────
    /// Σ matching ops (the §D3 CPU-gate proxy: edge intake + sort + scan).
    pub match_ops: u64,
    /// Σ edges generated across all passes.
    pub match_edges: u64,
    /// Assignment passes run (ticks with ≥ 1 idle carrier and ≥ 1 deposit).
    pub match_passes: u64,
    /// The most candidate edges any single pass generated (the contended worst case, review #2).
    pub match_max_edges: u64,
    /// The sampled greedy-vs-exact gap (market arms with `measure_gap` only).
    pub match_gap: Option<GapStats>,
    /// The #7 diagnostic: first tick trailing income met the demoted 0.9 threshold
    /// (`RecoveryTracker::self_sufficient_at` — reported, never gating).
    pub self_sufficient_at: Option<u32>,
    /// Construction sites completed during the run, by kind (road rebuilds show here).
    pub sites_built: BTreeMap<&'static str, u32>,
    /// ADR 0044 P2 remote-haul instruments (Family R; zero on single-room families).
    pub remote: crate::metrics::RemoteInstruments,
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
            .map(|(&id, w)| (id, role_spec(&w.role)))
            .collect();
        // In-flight spawns count toward the desired-counts (the live mission tracks creeps from
        // the spawn callback, i.e. from spawn START).
        for (&id, r) in &self.pending_roles {
            all.insert(id, r.clone());
        }
        all
    }
}

fn role_spec(role: &Role) -> RoleSpec {
    match role {
        Role::Harvester { source_idx } => RoleSpec::Harvester { source_idx: *source_idx },
        Role::Hauler => RoleSpec::Hauler,
        Role::Upgrader => RoleSpec::Upgrader,
        Role::Builder { allow_harvest } => RoleSpec::Builder { allow_harvest: *allow_harvest },
    }
}

fn spec_role(spec: &RoleSpec) -> Role {
    match spec {
        RoleSpec::Harvester { source_idx } => Role::Harvester { source_idx: *source_idx },
        RoleSpec::Hauler => Role::Hauler,
        RoleSpec::Upgrader => Role::Upgrader,
        RoleSpec::Builder { allow_harvest } => Role::Builder { allow_harvest: *allow_harvest },
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
        // M2: an in-flight refill trip holds its pickup booking (the live register_pickup on the
        // ticket every tick — jobs/upgrade.rs:143-145 / jobs/build.rs:115-117).
        Activity::FillFrom { src, take } => {
            *bookings.pickups.entry(*src).or_insert(0) += take;
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

/// Run a Family R (remote-mining) scenario through the MULTI-ROOM [`crate::movement::RoverMover`]
/// (ADR 0044 P2) — the mover routes remote-container→home hauls across rooms at true distance.
pub fn run_family_r(sc: &crate::scenario::FamilyRScenario, opts: &RunOptions) -> RunOutcome {
    let (mut world, _terrain, info) = sc.instantiate();
    let mut mover = crate::movement::RoverMover::new(&world.movement);
    run_world(&sc.shell(), &mut world, &mut mover, &info, opts)
}

/// The inner loop over an already-instantiated world (the oracle/test seam).
pub fn run_world(
    sc: &EconScenario,
    world: &mut EconWorld,
    mover: &mut dyn Mover,
    info: &LayoutInfo,
    opts: &RunOptions,
) -> RunOutcome {
    let t_star = match opts.goal {
        RunGoal::Recover | RunGoal::RecoverThenClockSafe => {
            crate::oracle::t_star(world, mover, info, &opts.consts)
        }
        RunGoal::Rcl { target } => crate::oracle::t_star_rcl(world, mover, info, target),
        RunGoal::Horizon => 0,
    };
    let mut driver = Driver { workers: BTreeMap::new(), pending_roles: BTreeMap::new() };

    // Pre-existing creeps: roles inferred from BODY (M2 — Family S seeds full fleets):
    // WORK+CARRY ≥ 2 MOVE-heavy shuttles → harvesters on source 0 (the M1 skeleton convention);
    // CARRY-only → haulers; the runner reassigns nothing after this.
    let initial: Vec<(u32, u32, u32)> = world
        .movement
        .creeps
        .iter()
        .map(|c| (c.id, c.body.alive_part_count(Part::Work), c.body.alive_part_count(Part::Carry)))
        .collect();
    // Single-room families keep the M1 skeleton convention (all harvesters on source 0). When the
    // world has REMOTE sources (Family R — sources outside the spawn room), round-robin the initial
    // harvesters across ALL sources so remotes are mined from t0 (the K4 spawn kernel already covers
    // every source over time; this just warm-starts the remote lanes).
    let home_room = world.spawns.first().map(|s| s.pos.room_name());
    let has_remotes = home_room.map_or(false, |hr| world.sources.iter().any(|s| s.pos.room_name() != hr));
    let n_sources = world.sources.len().max(1);
    let mut next_harvester = 0usize;
    for (id, work, _carry) in initial {
        let role = if work == 0 {
            Role::Hauler
        } else if has_remotes {
            let s = next_harvester % n_sources;
            next_harvester += 1;
            Role::Harvester { source_idx: s }
        } else {
            Role::Harvester { source_idx: 0 }
        };
        driver.workers.insert(id, Worker::new(role));
    }

    let mut tracker = RecoveryTracker::new(opts.consts, world.sources.len() as u32);
    let mut sentinel = DeadlockSentinel::default();
    let mut diagnostics = Diagnostics::default();
    let mut remote_instr = crate::metrics::RemoteInstruments::default();
    let mut report_digest: u64 = 0xcbf2_9ce4_8422_2325;
    let mut ticks_run = 0u32;
    let mut roads_before = world.roads.len();

    // M2 per-family trackers.
    let mut t_rcl: BTreeMap<u8, u32> = BTreeMap::new();
    if let Some(c) = &world.controller {
        t_rcl.insert(c.level, 0); // the starting level counts as reached at t0
    }
    let mut levels_lost = 0u32;
    let mut road_stock: Vec<(u32, u64, u64)> = Vec::new();
    // Review B8: the ever-seen road-tile denominator (tile → hits_max), seeded from t0's roads.
    let mut road_denominator: BTreeMap<(u8, u8), u32> = world
        .roads
        .iter()
        .map(|r| ((r.pos.x().u8(), r.pos.y().u8()), r.hits_max))
        .collect();
    let mut assignments = 0u64;
    let mut intents_total = 0u64;
    let mut deficit_episodes: Vec<u32> = Vec::new();
    let mut deficit_started: Option<u32> = None;
    let mut sites_built: BTreeMap<&'static str, u32> = BTreeMap::new();

    // M4: the market arms carry per-run market state (wear observation, pass products, the
    // matching/gap diagnostics). None on the tier arms — every baseline path is untouched.
    let mut market_rt: Option<MarketRuntime> = opts.cfg.market.map(|cfg| MarketRuntime::new(cfg, world.tick()));

    for _ in 0..opts.tick_cap {
        // ── 0. The construction pass (M2; live ConstructionMission every 50 ticks at the room's
        // phase — construction.rs:452): place plan sites the current RCL allows. ────────────────
        if world.tick() % 50 == opts.construction_phase % 50 {
            construction_pass(world, info);
        }

        // ── 1. Re-price the movement field if the road SET changed (death OR construction). ────
        if world.roads.len() != roads_before {
            mover.invalidate_from(&world.movement);
            roads_before = world.roads.len();
        }

        // ── 2. Demand + bookings (rebuilt per tick). ────────────────────────────────────────────
        let mut bookings = Bookings::default();
        for w in driver.workers.values() {
            book_activity(&mut bookings, &w.activity);
        }

        // ── 2.5 The MARKET pass (market arms only — ADR §D1/§D3): plan preview → refill bid →
        // per-deposit bids + opportunity floor + downgrade veto → the per-room greedy assignment
        // over this tick's idle carriers (assigned flows booked; tasks consumed by step 3). ──────
        if let Some(rt) = market_rt.as_mut() {
            let deposit_set = deposits(world, info, &bookings);
            let pickup_set = pickups(world, info, &bookings);
            let unfulfilled = matched_unfulfilled_hauling(&deposit_set, &pickup_set);
            let plans_preview = market::spawn_requests_market(world, &driver.roles(), unfulfilled, rt);
            let dep_bids = rt.begin_tick(world, info, &plans_preview, &deposit_set);
            let carriers = collect_market_carriers(world, &driver);
            rt.market_pass(world, &deposit_set, &dep_bids, &pickup_set, &carriers, &mut bookings, mover);
        }

        // ── 3. Worker FSM steps (creep-id order). ───────────────────────────────────────────────
        let mut intents = EconIntents::new();
        let mut moved_any = false;
        let allowance = allowance_for(&opts.cfg, world);
        let ids: Vec<u32> = driver.workers.keys().copied().collect();
        let mut new_assignments = 0u64;
        for id in ids {
            let step = step_worker(
                world,
                mover,
                info,
                allowance,
                opts.cfg.tiered_delivery,
                market_rt.as_mut(),
                &mut bookings,
                &mut driver,
                id,
                &mut intents,
                &mut new_assignments,
            );
            moved_any |= step;
        }
        assignments += new_assignments;
        let actions_emitted = intents.actions.len() as u32;
        intents_total += actions_emitted as u64;

        // ── 4. K4 spawning through the shared queue kernel. The hauling stat is the live
        // supply↔demand MIN-MATCH (`matched_unfulfilled_hauling` — the transfersystem.rs
        // :2249-2337 three-stage match, not demand alone), so a drained S0=0 world spawns no
        // hauler for unhaulable demand. ─────────────────────────────────────────────────────────
        let deposit_set = deposits(world, info, &bookings);
        let pickup_set = pickups(world, info, &bookings);
        let unfulfilled_hauling = matched_unfulfilled_hauling(&deposit_set, &pickup_set);
        let plans = if let Some(rt) = market_rt.as_ref() {
            // Market arms: the K4 request set (bodies deficit-priced when `k4_bodies`; the
            // repairer arm bid-admitted). Priorities keep the f32 band interface (M5b owns
            // bid-ordering of the queue).
            market::spawn_requests_market(world, &driver.roles(), unfulfilled_hauling, rt)
                .into_iter()
                .map(|(p, _)| p)
                .collect()
        } else {
            spawn_requests(world, &driver.roles(), unfulfilled_hauling, allowance)
        };
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
                driver.workers.insert(id, Worker::new(spec_role(&role)));
            }
        }
        for &id in &report.deaths {
            driver.workers.remove(&id);
        }

        // M2: completed OBSTACLE structures enter the pathing field as walls (the realize()
        // convention, applied mid-run) and any completion re-prices the memoized traces.
        if !report.sites_completed.is_empty() {
            let mut new_walls: Vec<Position> = Vec::new();
            for &(kind, p) in &report.sites_completed {
                *sites_built.entry(kind_name(kind)).or_insert(0) += 1;
                if kind.blocks_movement() {
                    let key = (p.x().u8(), p.y().u8());
                    match world.movement.rooms.get_mut(&p.room_name()) {
                        Some(t) => {
                            t.walls.insert(key);
                        }
                        None => {
                            world.movement.terrain.walls.insert(key);
                        }
                    }
                    new_walls.push(p);
                }
            }
            mover.invalidate_from(&world.movement);
            roads_before = world.roads.len();
            // Review B9 — the memo purge alone leaves IN-FLIGHT `Rc` traces walking through the
            // new wall: force any worker whose REMAINING trace crosses a fresh obstacle back to
            // Idle (bookings rebuild per tick, so the dropped task releases cleanly — the
            // analytic-tier analog of a live repath on blocked).
            if !new_walls.is_empty() {
                for worker in driver.workers.values_mut() {
                    if let Activity::Travel { trace, idx, .. } = &worker.activity {
                        if trace[*idx..].iter().any(|p| new_walls.contains(p)) {
                            worker.activity = Activity::Idle;
                        }
                    }
                }
            }
        }

        // ADR 0044 P2 remote-haul instruments: in-flight energy (Σ creep carry), carrier
        // utilization (creeps with energy aboard), and dropped/wasted energy present this tick.
        let in_flight: u32 = world.movement.creeps.iter().map(|c| c.carry_used).sum();
        let carrying = world.movement.creeps.iter().filter(|c| c.carry_used > 0).count() as u32;
        let dropped: u32 = world.dropped.iter().map(|d| d.amount).sum();
        // Instrument B — energy waiting in containers (mined but not yet consumed/hauled).
        let buffer: u32 = world.containers.iter().map(|c| c.store.amount(SimResource::Energy)).sum();
        remote_instr.sample(in_flight, carrying, dropped, buffer);

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

        // M2 trackers: RCL first-crossings, levels lost, deficit episodes, road stock samples.
        for &new_level in &report.level_ups {
            t_rcl.entry(new_level).or_insert(report.tick);
        }
        levels_lost += report.downgrades.len() as u32;
        let deficit_now = capacity > 0 && lane_energy < capacity;
        match (deficit_started, deficit_now) {
            (None, true) => deficit_started = Some(report.tick),
            (Some(start), false) => {
                deficit_episodes.push(report.tick.saturating_sub(start));
                deficit_started = None;
            }
            _ => {}
        }
        if report.tick.is_multiple_of(100) {
            // Review B8 — a FIXED (monotone) denominator kills the survivorship bias: every road
            // tile EVER seen keeps its hits_max in the denominator, so a dead road drags the
            // ratio down (numerator 0) instead of leaving the metric (denominator shrink made
            // stock JUMP on road death); a rebuild re-enters the same tile at full.
            for r in &world.roads {
                road_denominator.entry((r.pos.x().u8(), r.pos.y().u8())).or_insert(r.hits_max);
            }
            let hits: u64 = world.roads.iter().map(|r| r.hits as u64).sum();
            let max: u64 = road_denominator.values().map(|&m| m as u64).sum();
            road_stock.push((report.tick, hits, max));
        }

        let progressed = report.ledger.harvested > 0
            || report.ledger.spawn_self_charge > 0
            || report.ledger.repair_total() > 0
            || report.ledger.upgrade > 0
            || report.ledger.build > 0
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

        let goal_met = match opts.goal {
            RunGoal::Recover => tracker.recovered_at.is_some(),
            // Review B5: the clock question must RESOLVE — recovered AND the clock back at/above
            // half-max (a downgrade resets to half-max, so a lost level also resolves it).
            RunGoal::RecoverThenClockSafe => {
                tracker.recovered_at.is_some()
                    && world.controller.as_ref().is_none_or(|c| {
                        c.level == 0
                            || c.downgrade_ticks
                                >= screeps_econ_engine::constants::controller_downgrade(c.level) / 2
                    })
            }
            RunGoal::Rcl { target } => t_rcl.contains_key(&target),
            RunGoal::Horizon => false,
        };
        if goal_met || sentinel.fired_at.is_some() {
            break;
        }
    }

    let deadlocked = sentinel.fired_at.is_some();
    let (recovered_at, effective_t, eta, eta_raw) = match opts.goal {
        RunGoal::Recover | RunGoal::RecoverThenClockSafe => {
            let recovered_at = if deadlocked { None } else { tracker.recovered_at };
            let effective_t = recovered_at.unwrap_or(opts.tick_cap);
            // η = 0 at the cap AND on the deadlock sentinel (ADR hard gate); eta_raw unclamped
            // (review A1 — the oracle-sanity instrument).
            let (eta, eta_raw) = crate::metrics::etas(t_star, recovered_at);
            (recovered_at, effective_t, eta, eta_raw)
        }
        RunGoal::Rcl { target } => {
            let reached = if deadlocked { None } else { t_rcl.get(&target).copied() };
            let effective_t = reached.unwrap_or(opts.tick_cap);
            let (eta, eta_raw) = crate::metrics::etas(t_star, reached);
            (reached, effective_t, eta, eta_raw)
        }
        RunGoal::Horizon => (None, ticks_run, 0.0, 0.0),
    };
    // Review A3: a deficit episode still open at run end must not be censored out of the
    // latency statistics.
    let deficit_open_at_end =
        deficit_started.map(|start| ticks_run.saturating_sub(1).saturating_sub(start));
    let (match_ops, match_edges, match_passes, match_max_edges, match_gap) = match &market_rt {
        Some(rt) => (rt.match_ops, rt.match_edges, rt.match_passes, rt.match_max_edges, rt.cfg.measure_gap.then_some(rt.gap)),
        None => (0, 0, 0, 0, None),
    };
    // Instrument D — pull the realized haul-cost / delivered-value integrals off the market runtime.
    if let Some(rt) = market_rt.as_ref() {
        remote_instr.realized_haul_cost = rt.haul_cost_integral;
        remote_instr.delivered_value = rt.delivered_value_integral;
        remote_instr.admission_declines = rt.admission_declines;
    }
    RunOutcome {
        scenario: sc.name.clone(),
        seed: sc.seed,
        t_star,
        recovered_at,
        effective_t,
        eta,
        eta_raw,
        ticks_run,
        deadlocked,
        diagnostics,
        state_digest: world.state_digest(),
        report_digest,
        t_rcl,
        levels_lost,
        final_controller: world.controller.as_ref().map(|c| (c.level, c.progress)),
        road_stock,
        assignments,
        intents_emitted: intents_total,
        deficit_episodes,
        deficit_open_at_end,
        self_sufficient_at: tracker.self_sufficient_at,
        sites_built,
        match_ops,
        match_edges,
        match_passes,
        match_max_edges,
        match_gap,
        remote: remote_instr,
    }
}

/// The market pass's carrier pool: this tick's IDLE haul-capable creeps (haulers always;
/// harvesters with their §D5.4 opportunity rate — a live assigned source prices
/// `min(2·WORK, 10) e/t`, a drained/absent source or a full store prices 0). Upgraders and
/// builders are never carriers (their withdraw side is the Use-lane admission).
fn collect_market_carriers(world: &EconWorld, driver: &Driver) -> Vec<CarrierDto> {
    let mut out = Vec::new();
    for (&id, w) in &driver.workers {
        if !matches!(w.activity, Activity::Idle) {
            continue;
        }
        let Some(creep) = world.creep(id) else { continue };
        let (free, held) = (creep_free(world, id), creep_energy(world, id));
        match w.role {
            Role::Hauler => out.push(CarrierDto { id, pos: creep.pos, free, held, opportunity_milli: 0 }),
            Role::Harvester { source_idx } => {
                let live_source = world.sources.get(source_idx).map(|s| s.energy > 0).unwrap_or(false);
                let opportunity = if free > 0 && live_source {
                    let work = creep.body.alive_part_count(Part::Work);
                    (2_000 * work).min(10_000)
                } else {
                    0
                };
                out.push(CarrierDto { id, pos: creep.pos, free, held, opportunity_milli: opportunity });
            }
            _ => {}
        }
    }
    out
}

/// Convert a consumed market task into the worker's next activity (the same travel-then shapes
/// the baseline selections use).
fn market_task_activity(
    world: &EconWorld,
    mover: &mut dyn Mover,
    id: u32,
    pos: Position,
    task: &MarketTask,
    tick: u32,
) -> Activity {
    match *task {
        MarketTask::PickupDeliver { src, src_pos, take, sink, sink_pos, give } => travel_then(
            world,
            mover,
            id,
            pos,
            src_pos,
            1,
            Activity::PickupFor { src, take, sink, sink_pos, give },
            tick,
        ),
        MarketTask::Deliver { sink, sink_pos, amount } => {
            travel_then(world, mover, id, pos, sink_pos, 1, Activity::Deliver { sink, amount }, tick)
        }
    }
}

fn kind_name(kind: StructureKind) -> &'static str {
    match kind {
        StructureKind::Spawn => "spawn",
        StructureKind::Extension => "extension",
        StructureKind::Road => "road",
        StructureKind::Container => "container",
        StructureKind::Storage => "storage",
        StructureKind::Tower => "tower",
    }
}

/// The construction pass (M2) — the live `ConstructionMission` execution cycle transcribed
/// (construction.rs:445-479 + the `ConstructionFilter` rules):
/// - candidates = plan structures with `required_rcl ≤ level`, in-vocabulary, nothing of the
///   kind built at the tile and no site there (structure_or_site_exists, :341-355);
/// - ROADS defer until an 8-neighbor holds a structure, a site, or a placement approved earlier
///   THIS batch (:247-254 + :308-334 — road chains grow outward from the hub in one cycle);
/// - order: (required_rcl, capture order) — the plan-order iteration approximated (documented);
/// - budget: up to `max_construction_sites (10, features.rs:130) − existing` NEW sites,
///   SUCCESS-charged (failures skip without burning budget, :455-458).
///
/// *Reductions (documented):* the spawn-seal deferral (REC-050) and the walls/ramparts RCL gate
/// are dropped — walls/ramparts are out of vocabulary and foreman plans keep spawn approaches
/// open by construction (ReachabilityLayer).
fn construction_pass(world: &mut EconWorld, info: &LayoutInfo) {
    const MAX_CONSTRUCTION_SITES: usize = 10; // features.rs:130
    let level = world.controller.as_ref().map(|c| c.level).unwrap_or(0);
    let mut budget = MAX_CONSTRUCTION_SITES.saturating_sub(world.sites.len());
    if budget == 0 {
        return;
    }
    let mut order: Vec<usize> = (0..info.plan_structures.len()).collect();
    order.sort_by_key(|&i| (info.plan_structures[i].required_rcl, i));
    let mut placed_this_batch: Vec<(u8, u8)> = Vec::new();
    for i in order {
        if budget == 0 {
            break;
        }
        let s = &info.plan_structures[i];
        if s.required_rcl > level {
            continue;
        }
        let p = Position::new(
            screeps::RoomCoordinate::new(s.x).unwrap(),
            screeps::RoomCoordinate::new(s.y).unwrap(),
            info.room,
        );
        if world.structure_of_kind_at(p, s.kind) || world.sites.iter().any(|site| site.pos == p) {
            continue;
        }
        // The road-adjacency deferral (construction.rs:247-254): any 8-neighbor with a
        // structure (non-natural-wall), a site, or a batch-approved placement justifies it.
        if s.kind == StructureKind::Road && !road_has_adjacent_anchor(world, info, s.x, s.y, &placed_this_batch) {
            continue;
        }
        if world.add_construction_site(p, s.kind).is_ok() {
            placed_this_batch.push((s.x, s.y));
            budget -= 1;
        }
        // Failures (allowance/collision) skip WITHOUT burning budget — success-charged.
    }
}

/// Whether a structure/site/batch-placement sits in the 8-neighborhood of `(x, y)`
/// (construction.rs:308-334; "structure" = anything `look_for STRUCTURES` returns except
/// constructedWall — review B4: the CONTROLLER is a structure live and counts, as does the
/// realized out-of-vocab furniture (labs/links/terminal/ramparts) the sim keeps as
/// `info.furniture_tiles`).
fn road_has_adjacent_anchor(world: &EconWorld, info: &LayoutInfo, x: u8, y: u8, batch: &[(u8, u8)]) -> bool {
    for dy in -1i16..=1 {
        for dx in -1i16..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (x as i16 + dx, y as i16 + dy);
            if !(0..=49).contains(&nx) || !(0..=49).contains(&ny) {
                continue;
            }
            let (nx, ny) = (nx as u8, ny as u8);
            if batch.contains(&(nx, ny)) || info.furniture_tiles.contains(&(nx, ny)) {
                return true;
            }
            let at = |p: Position| (p.x().u8(), p.y().u8()) == (nx, ny);
            if world.spawns.iter().any(|s| at(s.pos))
                || world.extensions.iter().any(|e| at(e.pos))
                || world.containers.iter().any(|c| at(c.pos))
                || world.roads.iter().any(|r| at(r.pos))
                || world.towers.iter().any(|t| at(t.pos))
                || world.storage.as_ref().is_some_and(|s| at(s.pos))
                || world.controller.as_ref().is_some_and(|c| at(c.pos))
                || world.sites.iter().any(|s| at(s.pos))
            {
                return true;
            }
        }
    }
    false
}

/// One worker's FSM step. Returns whether the creep MOVED (teleported) this tick.
/// `assignments` counts NEW task selections (the flap_rate numerator — every point where the
/// live state machine would pick a fresh target).
#[allow(clippy::too_many_arguments)]
fn step_worker(
    world: &mut EconWorld,
    mover: &mut dyn Mover,
    info: &LayoutInfo,
    allowance: baseline::RepairAllowance,
    tiered_delivery: bool,
    mut market: Option<&mut MarketRuntime>,
    bookings: &mut Bookings,
    driver: &mut Driver,
    id: u32,
    intents: &mut EconIntents,
    assignments: &mut u64,
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
    // Upgraders/builders run NO opportunistic repair (their repair is an explicit state —
    // jobs/upgrade.rs has no repair arm; jobs/build.rs repairs via the queue-read Idle arms).
    let drive_by_min = if is_harvester {
        Some(effective_min_repair_priority(Some(RepairPriority::Medium), allowance))
    } else {
        None // local hauler / upgrader / builder: no drive-by lane
    };

    let mut moved = false;
    match std::mem::replace(&mut worker.activity, Activity::Idle) {
        Activity::Idle => {
            // ── MARKET arms: the bid-driven Idle chain (task from the pass → productive
            // fallback → admitted repair → wait). The tier chains below never run. ───────────────
            if let Some(rt) = market.as_deref() {
                step_idle_market(world, mover, info, rt, bookings, worker, id, pos, tick);
                if !matches!(worker.activity, Activity::Wait { .. } | Activity::Idle) {
                    *assignments += 1;
                }
                return false; // Idle steps never move
            }
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
                            effective_min_repair_priority(Some(RepairPriority::Medium), allowance),
                        ) {
                            // (5) idle full-repair ≥ Medium (harvest.rs:178-193, allowance-gated).
                            worker.activity =
                                travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                        } else if let Some((sink, spos, amount)) = select_delivery_tiered(
                            pos,
                            &deposit_set,
                            held,
                            &[Tier::Medium, Tier::Low, Tier::None],
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
                        // (storage dump) only when no ACTIVE demand exists. The PTRP arm
                        // (`tiered_delivery`) walks the tiers High→Medium→Low instead — the
                        // tier-faithful S3 fix (M4 optional arm).
                        let selected = if tiered_delivery {
                            select_delivery_tiered(pos, &deposit_set, held, &[Tier::High, Tier::Medium, Tier::Low])
                        } else {
                            select_delivery_flat_active(pos, &deposit_set, held)
                        }
                        .or_else(|| select_delivery_tiered(pos, &deposit_set, held, &[Tier::None]));
                        if let Some((sink, spos, amount)) = selected {
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
                // The LIVE upgrader Idle chain, in ORDER (jobs/upgrade.rs:102-133): nearby
                // pickup (ALL priorities, HAUL|USE lanes, the slow-creep CONTROLLER anchor) →
                // harvest (fast always; slow only with zero storage AND containers) → [sign —
                // SKIPPED: no sign model in-sim, a one-shot live detour, documented reduction]
                // → upgrade (energy > 0, owned controller) → wait(5).
                Role::Upgrader => {
                    let body = world.creep(id).unwrap().body.clone();
                    let free = creep_free(world, id);
                    let pickup_set = pickups(world, info, bookings);
                    let anchor = upgrader_pickup_anchor(&body, info.controller_pos);
                    if let Some((src, spos, take)) = select_fill_pickup(pos, free, &pickup_set, anchor) {
                        *bookings.pickups.entry(src).or_insert(0) += take;
                        worker.activity =
                            travel_then(world, mover, id, pos, spos, 1, Activity::FillFrom { src, take }, tick);
                    } else if free > 0
                        && upgrader_should_allow_harvest(&body, world)
                        && !world.sources.is_empty()
                    {
                        // jobs/upgrade.rs:123-129 → get_new_harvest_state: NEAREST source,
                        // free capacity required (harvestbehavior.rs:14-16), no energy filter.
                        let si = nearest_source(world, pos);
                        worker.activity = travel_then(
                            world, mover, id, pos, world.sources[si].pos, 1,
                            Activity::HarvestSrc { source_idx: si }, tick,
                        );
                    } else if held > 0 && world.controller.as_ref().is_some_and(|c| c.level > 0) {
                        // :131 → get_new_upgrade_state (controllerbehavior.rs:10-29).
                        worker.activity =
                            travel_then(world, mover, id, pos, info.controller_pos, 3, Activity::Upgrade, tick);
                    } else {
                        worker.activity = Activity::Wait { until: tick + 5 }; // :132
                    }
                }
                // The LIVE builder Idle chain, in ORDER (jobs/build.rs:57-112): repair ≥ High
                // (allowance-raised) → build (site select: foreman priority, progress, nearest)
                // → repair at ANY priority (the S4 leak — VeryLow roads included; allowance-
                // raised) → pickup (ALL, HAUL|USE, no anchor) → harvest (if frozen-allowed) →
                // wait(5). Arms 1-3 need energy > 0 (repairbehavior.rs:26, buildbehavior.rs:14).
                Role::Builder { allow_harvest } => {
                    let free = creep_free(world, id);
                    let rcl = world.controller.as_ref().map(|c| c.level).unwrap_or(0);
                    let min_high = effective_min_repair_priority(Some(RepairPriority::High), allowance);
                    // The live "repair at ANY priority" arm has NO floor at all (the Option-min
                    // form: None) — the S1 allowance raises it to Critical. (The pre-M3 VeryLow
                    // stand-in was equivalent: every candidate is ≥ VeryLow.)
                    let min_any = effective_min_repair_priority(None, allowance);
                    let repair_high =
                        (held > 0).then(|| baseline::full_repair_target(world, min_high)).flatten();
                    let site = (held > 0)
                        .then(|| baseline::select_construction_site(pos, world, rcl))
                        .flatten();
                    if let Some((target, tpos)) = repair_high {
                        worker.activity =
                            travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                    } else if let Some(tile) = site {
                        let spos = site_pos(info.room, tile);
                        worker.activity =
                            travel_then(world, mover, id, pos, spos, 3, Activity::Build { tile }, tick);
                    } else if let Some((target, tpos)) =
                        (held > 0).then(|| baseline::full_repair_target(world, min_any)).flatten()
                    {
                        worker.activity =
                            travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                    } else if let Some((src, spos, take)) = {
                        let pickup_set = pickups(world, info, bookings);
                        select_fill_pickup(pos, free, &pickup_set, None)
                    } {
                        *bookings.pickups.entry(src).or_insert(0) += take;
                        worker.activity =
                            travel_then(world, mover, id, pos, spos, 1, Activity::FillFrom { src, take }, tick);
                    } else if allow_harvest && free > 0 && !world.sources.is_empty() {
                        let si = nearest_source(world, pos);
                        worker.activity = travel_then(
                            world, mover, id, pos, world.sources[si].pos, 1,
                            Activity::HarvestSrc { source_idx: si }, tick,
                        );
                    } else {
                        worker.activity = Activity::Wait { until: tick + 5 }; // :110
                    }
                }
            }
            // The flap counter: an Idle pass that SELECTED a task is one assignment.
            if !matches!(worker.activity, Activity::Wait { .. } | Activity::Idle) {
                *assignments += 1;
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
                // M4 market arms: the observed-traffic-wear term of the repair bid (§D1) —
                // the same timer-pull the engine just booked.
                if world.road_at(new_pos).is_some() {
                    if let Some(rt) = market.as_deref_mut() {
                        rt.observe_wear((new_pos.x().u8(), new_pos.y().u8()), parts);
                    }
                }
                moved = true;
            }
            let mut then = then;
            if is_harvester {
                let held = creep_energy(world, id);
                if held > 0 {
                    if let Some(target) = drive_by_target(world, new_pos, market.as_deref(), drive_by_min) {
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
                // Only harvesters enter the assigned-source Harvest state (upgraders/builders
                // use HarvestSrc); any other role here is a stale state — exit.
                _ => true,
            };
            if full || drained {
                worker.activity = Activity::Idle; // the Idle chain runs next tick
            } else {
                // K3: the opportunistic repair MASKS the harvest (shared Pipeline A —
                // jobs/actions.rs:27-31; harvest.rs:225's tick order: repair, then harvest).
                let mut repaired = false;
                if is_harvester {
                    let held = creep_energy(world, id);
                    if held > 0 {
                        if let Some(target) = drive_by_target(world, pos, market.as_deref(), drive_by_min) {
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
                    if is_harvester {
                        if let Some(target) = drive_by_target(world, pos, market.as_deref(), drive_by_min) {
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
            // MARKET arms: fall to Idle — leftover cargo re-enters the next tick's pass (the
            // uniform 1-tick-lag convention; documented reduction).
            let held = creep_energy(world, id);
            if market.is_some() {
                worker.activity = Activity::Idle;
            } else if held > 0 {
                let deposit_set = deposits(world, info, bookings);
                if let Some((sink, spos, amount)) = select_delivery_tiered(
                    pos,
                    &deposit_set,
                    held,
                    &[Tier::High, Tier::Medium, Tier::Low, Tier::None],
                ) {
                    *bookings.deposits.entry(sink).or_insert(0) += amount;
                    *assignments += 1;
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
            let harvester_chains = matches!(worker.role, Role::Harvester { .. });
            // HARVESTERS: the live FinishedRepair chain (harvest.rs:335-357) — a healed (or
            // dead) target rolls straight into the NEXT ≥Medium full-repair target while cargo
            // remains (allowance re-applied). BUILDERS: tick_repair → BuildState::idle
            // (jobs/build.rs:168-172) — completion falls to Idle, the next Idle pass re-selects.
            // The next chained target (market arms: bid-admitted; tier arms: ≥ Medium with the
            // allowance re-applied). Precomputed — the world does not change before the chain.
            let next_target: Option<(baseline::RepairRef, Position)> = if harvester_chains && held > 0 {
                match market.as_deref() {
                    Some(rt) => rt.full_repair_target(world).map(|(t, p, _)| (t, p)),
                    None => baseline::full_repair_target(
                        world,
                        effective_min_repair_priority(Some(RepairPriority::Medium), allowance),
                    ),
                }
            } else {
                None
            };
            let chain = |worker: &mut Worker, world: &EconWorld, mover: &mut dyn Mover, assignments: &mut u64| {
                if let Some((next, tpos)) = next_target {
                    *assignments += 1;
                    worker.activity =
                        travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target: next }, tick);
                    return;
                }
                worker.activity = Activity::Idle;
            };
            match resolve_repair_ref(world, target) {
                Some(sref) if held > 0 => {
                    let (hits, hits_max) = repair_hits(world, target);
                    if hits >= hits_max {
                        chain(worker, world, mover, assignments); // healed → next / Idle by role
                    } else {
                        intents.act(id, EconAction::Repair { target: sref });
                        worker.activity = Activity::FullRepair { target };
                    }
                }
                None if held > 0 => {
                    chain(worker, world, mover, assignments); // target died → next / Idle by role
                }
                _ => {
                    worker.activity = Activity::Idle; // out of energy
                }
            }
        }
        // ── M2: the upgrader work state (controllerbehavior.rs:69-133 `tick_upgrade`,
        // refill_when_draining = true — the parallel D+E refill the upgrader body math counts
        // on: the withdraw is issued THIS tick, adjacent sources costing no dedicated tick). ────
        Activity::Upgrade => {
            let held = creep_energy(world, id);
            if world.controller.as_ref().is_none_or(|c| c.level == 0) {
                worker.activity = Activity::Idle; // controller gone/unowned: Err → idle
            } else if pos.get_range_to(info.controller_pos) > 3 {
                worker.activity =
                    travel_then(world, mover, id, pos, info.controller_pos, 3, Activity::Upgrade, tick);
            } else if held == 0 {
                // The live upgrade Err(NotEnough) → Some(idle) — the Idle chain runs NEXT tick
                // (the uniform 1-tick-lag transcription convention, module docs).
                worker.activity = Activity::Idle;
            } else {
                intents.act(id, EconAction::UpgradeController);
                let body = world.creep(id).unwrap().body.clone();
                let work = body.alive_part_count(Part::Work);
                let free = creep_free(world, id);
                // MARKET arms: the Use-lane withdraw admission (§D1) — the upgrade sink must
                // meet the opportunity floor unless the downgrade veto is live.
                let refill_admitted = match market.as_deref() {
                    // ADR 0044 A3 — Arm A (`a3_live_control`) reverts Defect 2: bypass the Use-lane
                    // admission gate so the upgrader draws regardless of the floor (reproducing the
                    // live inversion where consumers never shed under a refill deficit).
                    Some(rt) => rt.cfg.a3_live_control
                        || rt.veto
                        || screeps_econ_decision::sink_economics::admit_use_withdraw(rt.upgrade_sink_bid(world), rt.floor),
                    None => true,
                };
                if refill_admitted && upgrade_about_to_run_dry(work, held, free) {
                    // controllerbehavior.rs:107-124: the state cascade runs the pickup NOW —
                    // an ADJACENT source withdraws in parallel (Pipeline D + E, same tick);
                    // a distant one starts the refill trip one tick early.
                    let pickup_set = pickups(world, info, bookings);
                    let anchor = upgrader_pickup_anchor(&body, info.controller_pos);
                    if let Some((src, spos, take)) = select_fill_pickup(pos, free, &pickup_set, anchor) {
                        *bookings.pickups.entry(src).or_insert(0) += take;
                        *assignments += 1;
                        if pos.get_range_to(spos) <= 1 {
                            emit_fill(world, intents, id, src, take);
                            worker.activity = Activity::Upgrade; // stays put — no dedicated tick
                        } else {
                            worker.activity =
                                travel_then(world, mover, id, pos, spos, 1, Activity::FillFrom { src, take }, tick);
                        }
                    } else {
                        worker.activity = Activity::Upgrade; // nothing to refill from: drain out
                    }
                } else {
                    worker.activity = Activity::Upgrade;
                }
            }
        }
        // ── M2: the builder work state (buildbehavior.rs:38-98 `tick_build`): site gone → Idle;
        // empty → (the live build Err) Idle; else emit Build on the CURRENT index (re-derived —
        // the compaction contract). Standing ON an obstacle-kind site wedges the build
        // (build.js:50-60): sidestep to the first walkable neighbor — the analytic-tier stand-in
        // for the live resolver's shove of a working creep (documented deviation). ──────────────
        Activity::Build { tile } => {
            let held = creep_energy(world, id);
            let site = world
                .sites
                .iter()
                .position(|s| (s.pos.x().u8(), s.pos.y().u8()) == tile);
            match site {
                Some(idx) if held > 0 => {
                    let (spos, kind) = (world.sites[idx].pos, world.sites[idx].kind);
                    if kind.blocks_movement() && pos == spos {
                        if let Some(n) = walkable_neighbor(world, pos) {
                            worker.activity =
                                travel_then(world, mover, id, pos, n, 0, Activity::Build { tile }, tick);
                        } else {
                            worker.activity = Activity::Wait { until: tick + 5 };
                        }
                    } else if pos.get_range_to(spos) > 3 {
                        worker.activity =
                            travel_then(world, mover, id, pos, spos, 3, Activity::Build { tile }, tick);
                    } else {
                        intents.act(id, EconAction::Build { site_idx: idx });
                        worker.activity = Activity::Build { tile };
                    }
                }
                _ => {
                    worker.activity = Activity::Idle; // completed/died or out of energy
                }
            }
        }
        // ── M2: the upgrader/builder refill (tick_pickup): withdraw/pick up into SELF, then
        // Idle (FinishedPickup's re-try collapses into the next Idle pass — 1-tick lag). ────────
        Activity::FillFrom { src, take } => {
            let free = creep_free(world, id);
            emit_fill(world, intents, id, src, take.min(free));
            worker.activity = Activity::Idle;
        }
        // ── M2: the upgrader/builder harvest arm (tick_harvest): full or drained → Idle. ───────
        Activity::HarvestSrc { source_idx } => {
            let full = creep_free(world, id) == 0;
            let drained = world.sources.get(source_idx).map(|s| s.energy == 0).unwrap_or(true);
            if full || drained {
                worker.activity = Activity::Idle;
            } else {
                intents.act(id, EconAction::Harvest { source_idx });
                worker.activity = Activity::HarvestSrc { source_idx };
            }
        }
        Activity::Wait { until } => {
            worker.activity = if tick >= until { Activity::Idle } else { Activity::Wait { until } };
        }
    }
    moved
}

/// The drive-by (opportunistic) repair target: market arms use the bid-admitted K3-market
/// selection ([`MarketRuntime::opportunistic_target`]); tier arms the priority-map queue read
/// with the caller's allowance-raised minimum.
fn drive_by_target(
    world: &EconWorld,
    pos: Position,
    market: Option<&MarketRuntime>,
    baseline_min: Option<Option<RepairPriority>>,
) -> Option<baseline::RepairRef> {
    match market {
        Some(rt) => rt.opportunistic_target(world, pos),
        None => baseline_min.and_then(|min| opportunistic_repair_target(world, pos, min)),
    }
}

/// The MARKET-arm Idle chain (ADR §D1/§D3, one per role):
/// - **Harvester**: pass task (the opportunity-gated haul edges) → harvest-first → admitted
///   full-repair with cargo → wait(5).
/// - **Hauler**: pass task → wait(5) (everything it can usefully do IS a market ticket).
/// - **Upgrader**: fill-pickup ADMITTED by the Use-lane rule (upgrade bid ≥ floor, or the
///   downgrade veto) → harvest (the live slow/fast rule) → upgrade → wait(5).
/// - **Builder**: with cargo, the higher-BID action of {admitted repair, best site's build} —
///   ties build; empty, fill-pickup admitted iff the intended sink's bid meets the floor →
///   harvest (if frozen-allowed) → wait(5).
#[allow(clippy::too_many_arguments)]
fn step_idle_market(
    world: &EconWorld,
    mover: &mut dyn Mover,
    info: &LayoutInfo,
    rt: &MarketRuntime,
    bookings: &mut Bookings,
    worker: &mut Worker,
    id: u32,
    pos: Position,
    tick: u32,
) {
    use screeps_econ_decision::sink_economics as se;
    let held = creep_energy(world, id);
    let free = creep_free(world, id);
    match worker.role {
        Role::Harvester { source_idx } => {
            let source_has_energy = world.sources.get(source_idx).map(|s| s.energy > 0).unwrap_or(false);
            if let Some(task) = rt.tasks.get(&id) {
                worker.activity = market_task_activity(world, mover, id, pos, task, tick);
            } else if free > 0 && source_has_energy {
                let src_pos = world.sources[source_idx].pos;
                worker.activity = travel_then(world, mover, id, pos, src_pos, 1, Activity::Harvest, tick);
            } else if held > 0 {
                if let Some((target, tpos, _bid)) = rt.full_repair_target(world) {
                    worker.activity = travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                } else {
                    worker.activity = Activity::Wait { until: tick + 5 };
                }
            } else {
                worker.activity = Activity::Wait { until: tick + 5 };
            }
        }
        Role::Hauler => {
            if let Some(task) = rt.tasks.get(&id) {
                worker.activity = market_task_activity(world, mover, id, pos, task, tick);
            } else {
                worker.activity = Activity::Wait { until: tick + 5 };
            }
        }
        Role::Upgrader => {
            let body = world.creep(id).unwrap().body.clone();
            // ADR 0044 A3 — Arm A bypasses the Use-lane admission gate (Defect 2 revert).
            let admitted = rt.cfg.a3_live_control || rt.veto || se::admit_use_withdraw(rt.upgrade_sink_bid(world), rt.floor);
            let fill = if admitted && free > 0 {
                let pickup_set = pickups(world, info, bookings);
                let anchor = upgrader_pickup_anchor(&body, info.controller_pos);
                select_fill_pickup(pos, free, &pickup_set, anchor)
            } else {
                None
            };
            if let Some((src, spos, take)) = fill {
                *bookings.pickups.entry(src).or_insert(0) += take;
                worker.activity = travel_then(world, mover, id, pos, spos, 1, Activity::FillFrom { src, take }, tick);
            } else if free > 0 && upgrader_should_allow_harvest(&body, world) && !world.sources.is_empty() {
                let si = nearest_source(world, pos);
                worker.activity = travel_then(
                    world, mover, id, pos, world.sources[si].pos, 1,
                    Activity::HarvestSrc { source_idx: si }, tick,
                );
            } else if held > 0 && world.controller.as_ref().is_some_and(|c| c.level > 0) {
                worker.activity = travel_then(world, mover, id, pos, info.controller_pos, 3, Activity::Upgrade, tick);
            } else {
                worker.activity = Activity::Wait { until: tick + 5 };
            }
        }
        Role::Builder { allow_harvest } => {
            let rcl = world.controller.as_ref().map(|c| c.level).unwrap_or(0);
            // The candidate sinks (computed regardless of cargo — the EMPTY builder's pickup
            // admission prices what it WOULD do with energy).
            let best_repair = rt.full_repair_target(world); // admission (or survival) included
            let best_site = baseline::select_construction_site(pos, world, rcl).and_then(|tile| {
                world
                    .sites
                    .iter()
                    .find(|s| (s.pos.x().u8(), s.pos.y().u8()) == tile)
                    .map(|s| (tile, rt.site_build_bid(s.kind)))
            });
            if held > 0 {
                match (best_repair, best_site) {
                    (Some((target, tpos, rbid)), Some((tile, bbid))) => {
                        // The per-tick optimum between the two sinks; exact ties BUILD (progress
                        // compounds; deterministic).
                        if rbid > bbid {
                            worker.activity =
                                travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                        } else {
                            let spos = site_pos(info.room, tile);
                            worker.activity = travel_then(world, mover, id, pos, spos, 3, Activity::Build { tile }, tick);
                        }
                    }
                    (Some((target, tpos, _)), None) => {
                        worker.activity =
                            travel_then(world, mover, id, pos, tpos, 3, Activity::FullRepair { target }, tick);
                    }
                    (None, Some((tile, _))) => {
                        let spos = site_pos(info.room, tile);
                        worker.activity = travel_then(world, mover, id, pos, spos, 3, Activity::Build { tile }, tick);
                    }
                    (None, None) => {
                        worker.activity = Activity::Wait { until: tick + 5 };
                    }
                }
            } else {
                // Empty: acquire energy only when the intended sink clears the floor (§D1
                // withdraw admission; a survival-override repair target always admits).
                let sink_bid = best_repair
                    .as_ref()
                    .map(|&(_, _, b)| b.max(rt.floor)) // an admitted repair target clears the floor by construction
                    .into_iter()
                    .chain(best_site.map(|(_, b)| b))
                    .max()
                    .unwrap_or(0);
                // ADR 0044 A3 — Arm A bypasses the builder self-fetch admission gate (Defect 2 revert).
                let fill = if (rt.cfg.a3_live_control || se::admit_use_withdraw(sink_bid, rt.floor)) && free > 0 {
                    let pickup_set = pickups(world, info, bookings);
                    select_fill_pickup(pos, free, &pickup_set, None)
                } else {
                    None
                };
                if let Some((src, spos, take)) = fill {
                    *bookings.pickups.entry(src).or_insert(0) += take;
                    worker.activity = travel_then(world, mover, id, pos, spos, 1, Activity::FillFrom { src, take }, tick);
                } else if allow_harvest && free > 0 && !world.sources.is_empty() {
                    let si = nearest_source(world, pos);
                    worker.activity = travel_then(
                        world, mover, id, pos, world.sources[si].pos, 1,
                        Activity::HarvestSrc { source_idx: si }, tick,
                    );
                } else {
                    worker.activity = Activity::Wait { until: tick + 5 };
                }
            }
        }
    }
}

/// Nearest source by linear range (get_new_harvest_state, harvestbehavior.rs:16-23 — no energy
/// filter; ties to the lowest index, the deterministic stand-in).
fn nearest_source(world: &EconWorld, pos: Position) -> usize {
    (0..world.sources.len())
        .min_by_key(|&i| (pos.get_range_to(world.sources[i].pos), i))
        .expect("caller checked sources non-empty")
}

fn site_pos(room: screeps::RoomName, tile: (u8, u8)) -> Position {
    Position::new(
        screeps::RoomCoordinate::new(tile.0).unwrap(),
        screeps::RoomCoordinate::new(tile.1).unwrap(),
        room,
    )
}

/// The first walkable neighbor (row-major — the birth-tile order), for the build sidestep.
fn walkable_neighbor(world: &EconWorld, pos: Position) -> Option<Position> {
    let mut candidates: Vec<Position> = Vec::new();
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            if let Ok(p) = pos.checked_add((dx, dy)) {
                candidates.push(p);
            }
        }
    }
    candidates.sort_by_key(|p| (p.y().u8(), p.x().u8()));
    candidates.into_iter().find(|&p| world.is_walkable(p))
}

/// Emit the withdraw/pickup intent for a fill source (shared by [`Activity::FillFrom`] and the
/// upgrader's same-tick parallel refill). Returns whether an intent was emitted (the source may
/// have died — the caller replans through Idle either way).
fn emit_fill(world: &EconWorld, intents: &mut EconIntents, id: u32, src: SrcKey, take: u32) -> bool {
    match src {
        SrcKey::Dropped(x, y) => world
            .dropped
            .iter()
            .position(|d| d.pos.x().u8() == x && d.pos.y().u8() == y && d.resource == SimResource::Energy)
            .map(|i| {
                intents.act(id, EconAction::Pickup { dropped_idx: i });
            })
            .is_some(),
        SrcKey::Storage => {
            if world.storage.is_some() {
                intents.act(id, EconAction::Withdraw { target: StructRef::Storage, resource: SimResource::Energy, amount: take });
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
                intents.act(id, EconAction::Withdraw { target: StructRef::Container(i), resource: SimResource::Energy, amount: take });
            })
            .is_some(),
    }
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
            plan_structures: Vec::new(),
            furniture_tiles: Vec::new(),
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

    /// ADR 0044 P2: a Family R remote-mining scenario RUNS end-to-end through the multi-room
    /// `RoverMover` — creeps traverse rooms (the mover drives cross-room movement for thousands of
    /// ticks without panicking/deadlocking) and at least one creep reaches a REMOTE room, proving
    /// the remote lanes are actually worked (the flow the ADR 0044 admission prices).
    #[test]
    fn family_r_runs_multiroom_and_reaches_remotes() {
        let sc = crate::scenario::FamilyRScenario::new("E11N1", 6, vec![40, 90], 3);
        let (mut world, _terrain, info) = sc.instantiate();
        let home_room = world.spawns[0].pos.room_name();
        let mut mover = crate::movement::RoverMover::new(&world.movement);
        let mut opts = RunOptions::new(PolicyConfig::default(), RecoverConsts::default(), 2_000);
        opts.goal = RunGoal::Horizon;
        let out = run_world(&sc.shell(), &mut world, &mut mover, &info, &opts);
        let creep_in_remote = world.movement.creeps.iter().any(|c| c.pos.room_name() != home_room);
        assert!(creep_in_remote, "a creep reached a remote room — cross-room movement is live");
        // ADR 0044 P2 instruments fire: energy IS in flight (carriers hauling remote energy) and
        // carriers spend real transit time (the remote-haul signature).
        assert!(out.remote.mean_in_flight() > 0, "in-flight energy > 0 (remote hauling active)");
        assert!(out.remote.in_flight_max > 0);
        assert!(out.remote.carrier_ticks > 0, "carriers are utilized");
    }

    /// ADR 0044 end state: the MARKET arm (admission + TRUE routed-distance haul pricing always on)
    /// runs over REALISTIC Family R terrain (generated caves, `routed ≫ Chebyshev`) — the mover-backed
    /// distance oracle drives the pass without panic, and the remotes are worked. The kernel-level
    /// decline of a beyond-break-even haul is pinned separately (`market::admission_declines_far_par_sink`).
    #[test]
    fn market_end_state_runs_on_realistic_family_r() {
        let sc = crate::scenario::FamilyRScenario::new("E11N1", 6, vec![0, 0], 5).realistic();
        let (mut world, _t, info) = sc.instantiate();
        let home_room = world.spawns[0].pos.room_name();
        let mut mover = crate::movement::RoverMover::new(&world.movement);
        let mut opts = RunOptions::new(PolicyConfig::market(crate::market::MarketArmCfg::default()), RecoverConsts::default(), 3_000);
        opts.goal = RunGoal::Horizon;
        let out = run_world(&sc.shell(), &mut world, &mut mover, &info, &opts);
        // Cross-room hauling on realistic terrain is live: energy in flight, a creep reaches a remote.
        assert!(out.remote.mean_in_flight() > 0, "remote hauling active on realistic terrain");
        assert!(world.movement.creeps.iter().any(|c| c.pos.room_name() != home_room), "a creep reached a realistic remote room");
        // ADR 0044 step 4 instruments populate: D (realized haul cost / delivered value) is measured
        // and its ratio is finite; B (mined-waiting buffer) accrues.
        assert!(out.remote.delivered_value > 0, "instrument D: value was delivered");
        assert!(out.remote.realized_haul_cost > 0, "instrument D: haul cost was priced on real routed distance");
        assert!(out.remote.haul_cost_permille() > 0 && out.remote.haul_cost_permille() < 1000, "instrument D ratio is sane (haul < delivered value)");
        assert!(out.remote.buffer_tick_integral > 0, "instrument B: source buffers hold mined energy");
    }

    /// ADR 0044 step 2 (the IN-SIM economic-decline proof, operator success gate): on a SATURATED
    /// home (remotes compete at storage PAR) over REALISTIC cave terrain (routed ≫ Chebyshev), the
    /// reduced-cost admission DECLINES beyond-break-even FAR remotes while serving near ones — so the
    /// un-hauled energy retained at a remote RISES with its routed distance. This is the full-run
    /// analogue of the kernel pins (`admission_declines_far_par_sink` / `unreachable_pickup...`).
    #[test]
    fn saturated_family_r_declines_far_remotes_in_sim() {
        use crate::movement::Mover;
        // 9 chained cave rooms: the farthest routes past the PAR break-even (`haul_milli(d)=1000` ⇒
        // d≈375), so it is genuinely rejected by admission (delivered ≤ 0), while nearer ones (under
        // break-even, generous haulers) are served.
        let n = 9usize;
        let sc = crate::scenario::FamilyRScenario::new("E11N1", 6, vec![0; n], 7).realistic().saturated();
        let (mut world, _t, info) = sc.instantiate();
        let home_spawn = world.spawns[0].pos;
        let mid = screeps_sim_core::terrain_gen::EXIT_MID;
        let remote_pos: Vec<Position> = (1..=n)
            .map(|k| home_spawn.checked_add(((k * 50) as i32, 0)).unwrap().room_name())
            .map(|rn| Position::new(RoomCoordinate::new(mid).unwrap(), RoomCoordinate::new(mid).unwrap(), rn))
            .collect();

        let mut mover = crate::movement::RoverMover::new(&world.movement);
        let body = screeps_sim_core::SimBody::unboosted(&[Part::Carry, Part::Move]);
        let routed: Vec<u32> = remote_pos.iter().map(|&r| mover.travel_ticks(home_spawn, r, 1, &body, 0).unwrap_or(u32::MAX)).collect();

        let mut opts = RunOptions::new(PolicyConfig::market(crate::market::MarketArmCfg::default()), RecoverConsts::default(), 1_200);
        opts.goal = RunGoal::Horizon;
        let out = run_world(&sc.shell(), &mut world, &mut mover, &info, &opts);

        // Un-hauled energy retained at each remote (its source + container): high = the admission
        // declined the haul, low = it was served.
        let retained: Vec<u32> = remote_pos
            .iter()
            .map(|&r| {
                let rn = r.room_name();
                let src: u32 = world.sources.iter().filter(|s| s.pos.room_name() == rn).map(|s| s.energy).sum();
                let cont: u32 = world.containers.iter().filter(|c| c.pos.room_name() == rn).map(|c| c.store.amount(SimResource::Energy)).sum();
                src + cont
            })
            .collect();
        for k in 0..n {
            eprintln!("[decline] remote {} routed={:<5} retained={}", k + 1, routed[k], retained[k]);
        }
        eprintln!(
            "[decline] admission_declines={} haul_cost_permille={} delivered={}",
            out.remote.admission_declines,
            out.remote.haul_cost_permille(),
            out.remote.delivered_value
        );

        // The corridor genuinely reaches past the par break-even (routed > ~375 ⇒ haul_milli > par).
        assert!(routed[n - 1] > routed[0], "routed distance grows down the cave corridor");
        assert!(routed[n - 1] >= 375, "the farthest remote is genuinely beyond the par break-even (routed {})", routed[n - 1]);

        // THE gate — the admission FIRED in an actual run: generated arcs were rejected as beyond
        // break-even (the full-run analogue of the kernel decline pins), not merely deprioritized.
        assert!(out.remote.admission_declines > 0, "the reduced-cost admission declined beyond-break-even arcs in-sim");
        // AND no realized haul was a net loss: aggregate realized haul cost stays below delivered
        // value (instrument D — the ADR "zero realized delivered<0" / no-over-hauling gate). The
        // greedy structurally refuses unadmitted edges, so this holds by construction; the instrument
        // makes it observable end-to-end.
        assert!(out.remote.delivered_value > 0 && out.remote.haul_cost_permille() < 1000, "every realized haul was profitable (no over-hauling)");
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

    /// Review A3 — a TERMINAL deficit must not be censored: a room whose lane can never fill
    /// (spawn self-charges to 300, the extension's 50 is unreachable — no creeps, no sources)
    /// ends with the episode OPEN, surfaced via `deficit_open_at_end` instead of vanishing from
    /// the closed-episode list.
    #[test]
    fn permanent_deficit_episode_is_not_censored() {
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(25, 25));
        w.spawns[s].store_energy = 0;
        w.add_extension(pos(26, 25), 3); // capacity 50 that nothing can ever fill
        let info = LayoutInfo {
            room: "W1N1".parse().unwrap(),
            controller_pos: pos(40, 40),
            container_roles: BTreeMap::new(),
            source_containers: BTreeMap::new(),
            plan_structures: Vec::new(),
            furniture_tiles: Vec::new(),
        };
        let terrain = w.movement.terrain.clone();
        let mut mover = AnalyticMover::new(&terrain);
        let opts = RunOptions::new(PolicyConfig::default(), RecoverConsts::default(), 1_000);
        let out = run_world(&sc(1_000), &mut w, &mut mover, &info, &opts);
        assert!(out.recovered_at.is_none(), "the lane never fills");
        assert!(out.deficit_episodes.is_empty(), "no episode ever CLOSED");
        let open = out.deficit_open_at_end.expect("the terminal deficit is surfaced");
        assert!(
            open as u64 + 2 >= out.ticks_run as u64,
            "the open episode spans the whole run (open {open}, ran {})",
            out.ticks_run
        );
    }
}
