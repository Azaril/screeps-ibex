//! The deterministic economy tick — [`resolve_econ_tick`] resolves one tick of economy actions
//! over an [`EconWorld`], calling the shared kernel's `resolve_movement` at the movement point of
//! its pipeline (the ADR 0033 `Simulation` layering contract; [`EconSim`] is the trait binding).
//!
//! ## Pipeline order (and every documented deviation from the real engine)
//!
//! 0. **Extension re-cap** — the engine recomputes extension capacity from the CURRENT controller
//!    level every tick (`extensions/tick.js:11`; the 50/100/200 table at engine-mechanics.md:456):
//!    when the world has a controller, every extension's capacity is re-derived from its level
//!    here. A controller-less world keeps its builder-set capacities (a scenario convenience —
//!    exact while the level is static, which M0 guarantees; M2's upgrade mechanics ride this same
//!    step). A level DROP never deletes energy: an over-capacity store just reads `free() == 0`.
//! 1. **Mask** — one Pipeline-A work intent (Harvest OR Repair OR Build, M2) + one Pipeline-D
//!    transfer-class intent (Transfer/Withdraw/Pickup) + one Pipeline-E UpgradeController per
//!    creep per tick, the bot's own pipeline model (`jobs/actions.rs:27-56`) — a repairing creep
//!    therefore SKIPS that tick's harvest, exactly the S1 leak mechanic, while an UPGRADE
//!    coexists with a same-tick withdraw (the live parallel-refill idiom,
//!    `controllerbehavior.rs:107-124`). *Deviation:* the engine resolves per-intent-name with a
//!    conflict matrix (engine-mechanics.md:59-76; harvest conflicts with build and repair there,
//!    build conflicts with repair, and upgradeController conflicts with NOTHING —
//!    `creeps/intents.js:3-13`); the bot never emits more than one per pipeline, so the mask
//!    models the DECISION layer's contract. Masking is deterministic: actions are stable-sorted
//!    by creep id; within a creep, first submission wins and duplicates are counted in the
//!    report.
//! 2. **Transfer/Withdraw/Pickup** (creep-id order) — adjacency-1 (Chebyshev), atomic per action;
//!    runs BEFORE harvest, matching the engine's within-creep intent order (drop/transfer/
//!    withdraw/pickup precede harvest — `creeps/intents.js:15`). **Transfer** mirrors the engine:
//!    an over-ask (`amount` greater than the creep holds of that resource) is REJECTED WHOLE
//!    (`ERR_NOT_ENOUGH_RESOURCES`) and counted; what moves is clamped to the target's free
//!    capacity. **Withdraw/Pickup** clamp to `min(requested, available, free)`. *Deviations:* the
//!    engine also rejects a withdraw over-ask whole — the withdraw clamp is kept for M0 driver
//!    convenience (revisit at M1 kernel transcription); the engine's cross-creep ordering is JS
//!    hash order (engine-mechanics.md:33) — explicitly unordered — so creep-id order is the
//!    deterministic stand-in.
//! 3. **Work lane: Harvest + Repair + Build (M2)** (creep-id order), then **site
//!    materialization**, then **source regen**. Harvest: gain `min(2×WORK, source.energy)`
//!    (engine-mechanics.md:457), store overflow drops to the creep's tile; the 300-tick timer
//!    starts at the first harvest below capacity and the pool refills when `tick >= regen_at − 1`
//!    (engine-mechanics.md:445-446, :466). Regen runs after harvest, as the engine's source tick
//!    runs after the intent stage (engine-mechanics.md §1.2).
//!    Repair (M1; engine `creeps/repair.js`, engine-mechanics.md:118): range ≤ 3 (Chebyshev),
//!    requires carried energy > 0; `effect = min(WORK × 100, energy × 100, hits_max − hits)`,
//!    `cost = ceil(effect / 100)` — exact integers, the engine's `REPAIR_POWER`/`REPAIR_COST`
//!    arithmetic. Targets: roads + containers (the M1 hit-bearing structures; spawns/extensions/
//!    storage carry no hits model in-sim and are rejected — documented deviation, `repair_other`
//!    stays declared). Repair energy is ledgered by structure class, and booked to the report's
//!    `repair_leak` when the room had a refill deficit at tick start (the live `repair_leak_e`
//!    mirror — see [`RepairLeak`]).
//!    **Build (M2; engine `creeps/build.js`):** range ≤ 3 (`:23`), energy > 0 (`:14`); an
//!    obstacle-kind site is rejected while an obstacle object or ANY creep stands on its tile
//!    (`:50-60` — the engine's safe-mode ally carve-out is moot, one owner in-sim);
//!    `effect = min(5 × WORK, total − progress, energy)` at 1 energy/progress (`:67-69,83`).
//!    Completed sites (progress ≥ total) MATERIALIZE after the work-lane loop in ascending site
//!    index (the engine inserts inline; deferring to the loop end keeps mid-loop Vec pushes out
//!    of the id/index space — same-tick observable state is identical because nothing later in
//!    the lane reads the new structure), then compact: **a built spawn starts EMPTY**
//!    (`build.js:123` — deliberately unlike the scenario builder's born-full `add_spawn`),
//!    extensions cap from the CURRENT controller level (`:130-137` + the per-tick re-cap),
//!    containers arm a 100-tick first decay window (`:261` — `CONTAINER_DECAY_TIME` flat, the
//!    ownership-aware window only applies from the first decay event on, `containers/tick.js:26`),
//!    roads get swamp-scaled hitsMax + a full decay window + the SAFE terrain registration
//!    (`:171-186`), towers materialize as stubs.
//!
//! 3c. **Upgrade lane (M2; Pipeline E, creep-id order; engine `creeps/upgradeController.js`)**:
//!    requires energy > 0 (`:9`), an owned controller at level ≥ 1 (`:24`), range ≤ 3 (`:21`).
//!    `effect = min(WORK × 1, energy)` (`:33-34`); at RCL 8 the room-wide 15 e/t cap applies via
//!    the per-tick accumulator shared across upgraders (`:42-52` — a capped-out intent is a
//!    no-op counted as rejected). Below 8: progress += effect, and the LEVEL-UP fires only when
//!    `progress + effect ≥ CONTROLLER_LEVELS[level]` AND the downgrade clock is within
//!    `CONTROLLER_DOWNGRADE_RESTORE` of full (`:67-68` — the near-full-clock gate; progress
//!    accumulates PAST the threshold while the clock recovers): progress carries the remainder
//!    (`:70`), level += 1, the clock resets to HALF the new level's max (`:72`), and a level-8
//!    arrival zeroes progress (`:74-76`). Energy is debited `effect` (`:92`) and ledgered as
//!    `upgrade`. (GCL and safeModeAvailable are not modeled — no GCL/safemode in the sim.)
//!
//! 3d. **Controller step (M2; engine `controllers/tick.js`, countdown translation pinned in
//!    [`crate::state::SimController`])**: on a tick with ≥ 1 successful upgrade (the `_upgraded`
//!    truthy gate, `:38` — note a 0-WORK upgrade contributes 0 and does NOT count), the clock
//!    restores `min(remaining + 100, FULL[level])` and NO downgrade check runs (`:38-43`).
//!    Otherwise the clock expires at remaining ≤ 1 (`gameTime >= downgradeTime − 1`, `:49`):
//!    level −= 1; at level 0 progress zeroes and the room is unowned (`:52-62`); else
//!    progress += `round(0.9 × CONTROLLER_LEVELS[new])` (`:66` — exact ×9/10 by the pinned
//!    table) and the clock re-arms at remaining + FULL[new]/2 (`:65`). Ordinary ticks decrement
//!    by 1. *Deviation:* structure DEACTIVATION above the lowered RCL allowance
//!    (engine-mechanics.md:231) is not modeled — the per-tick extension re-cap adjusts
//!    capacities, but no structure turns off (documented; Family D's triage doesn't depend on
//!    it).
//! 4. **Spawns** — completions first (a spawn finishing at tick T can accept a new request at T),
//!    then new `SpawnCreep` intents in **(spawn index, submission order)**: same-tick requests to
//!    DIFFERENT spawns resolve independently of emission order (spawn-index order is the
//!    deterministic cross-actor stand-in, exactly as creep-id order is for step 2); duplicate
//!    requests to ONE spawn keep first-submitted-wins (the per-creep mask's documented contract).
//!    Each request: body legal (≤50 parts, engine-mechanics.md:453), cost = Σ`BODYPART_COST`
//!    (:452), **atomic debit at intent time** drawing all spawns first then extensions
//!    closest-first to the spawning spawn (:257), fail-whole if room-wide energy < cost (:257),
//!    busy for `3 × parts` ticks (:242); requests debit sequentially against mutated stores,
//!    matching the engine's same-tick multi-spawn behavior (:257). The newborn materializes on
//!    the free adjacent tile with the lowest packed (y,x); if all 8 are blocked the completion
//!    slips +1 tick — a coarse model of the engine's blocked-exit slip (:242). *Deviations:* the
//!    engine emerges the creep at `spawnTime−1` and re-tries placement in the object-tick stage;
//!    our busy window is exactly `3 × parts` ticks with a deterministic tile choice (no
//!    `directions`, no spawnstomp).
//! 5. **Spawn self-charge** — every spawn below its 300 cap gains +1/tick while room spawn+
//!    extension energy < 300 (engine-mechanics.md:279); the room total is computed once at step
//!    start (the engine's per-spawn read order is hash-order-adjacent; one precomputed read is
//!    the deterministic stand-in).
//! 6. **Creep TTL** — [`crate::EconWorld::creep_ttl`] holds the engine's `ageTime`; death fires
//!    on the first tick where `tick + 1 >= ageTime` — the engine's `gameTime >= ageTime − 1`
//!    boundary (engine-mechanics.md:57). The dying creep drops its whole store to the ground at
//!    its CURRENT position and leaves the movement state. *Deviations:* direct drop — no
//!    tombstone (the engine's tombstone spills to ground after `5×parts` ticks,
//!    engine-mechanics.md:432; end state identical, timing differs); no body-part corpse energy
//!    (`CREEP_CORPSE_RATE`, :455) — spawn energy is a pure sink in M0; the engine moves a creep
//!    before its same-tick age-death (engine-mechanics.md:57) — we die before the movement step,
//!    so the drop lands one tile earlier.
//!
//! 6b. **Structure decay (M1)** — roads then containers, index order. A structure's decay event
//!    fires when `tick >= next_decay_at − 1` (the engine's `gameTime >= nextDecayTime − 1`,
//!    `roads/tick.js:10` / `containers/tick.js:10`). Roads lose `100 × terrain ratio` (swamp ×5;
//!    wall-tunnel ×150 not modeled — no tunnels in scope) per event (engine-mechanics.md:430);
//!    containers lose 5000 per event, window 500 at RCL ≥ 1 / 100 otherwise
//!    (engine-mechanics.md:429). At 0 hits the structure is REMOVED: a road's tile reverts to its
//!    natural terrain (de-registered from the movement terrain), a container's store drops to the
//!    ground (relocation — the same-tick pile decay in step 7 books any loss). Removal COMPACTS
//!    the Vec — structure indices are only valid within the tick they were read (intent steps ran
//!    earlier this tick, so in-tick intents are safe; drivers re-derive indices each tick).
//!    Hit decay costs no energy and is never ledgered (ledger module docs).
//! 7. **Dropped decay** — every pile loses `ceil(amount/1000)` (engine-mechanics.md:431),
//!    including piles created this same tick (deviation: engine object-tick timing makes
//!    same-tick decay of a fresh drop unobservable; ours is one tick earlier, exactly booked).
//! 8. **Movement** — `resolve_movement` over the embedded `MovementState` (tick advances here).
//!    After it, **traffic wear (M1)**: every creep that STEPPED onto a road tile pulls that road's
//!    `next_decay_at` forward by `ROAD_WEAROUT × body parts` (engine `movement.js:215-219`,
//!    engine-mechanics.md:430). *Deviations:* the engine wears the road while processing the move
//!    intent (before the road's same-tick object tick); ours lands after this tick's decay step,
//!    so a pull that would have fired the decay event this tick fires next tick instead — a
//!    one-tick skew, exactly booked. Positions changed OUTSIDE `resolve_movement` (the analytic
//!    movement tier's teleports) are not seen here — that tier books its own wear through
//!    [`crate::EconWorld::apply_road_wear`], the same public helper this step uses.
//! 9. **Ledger + conservation audit** — exact per-resource integer balance
//!    (`prev + minted − burned == now`), `debug_assert!`ed AND surfaced on the report
//!    ([`EconTickReport::conservation`]) so a harness gates on it rather than learning via a
//!    panic (EP-6.12).
//!
//! *Signature deviation from the M0 spec sketch:* no `rng` parameter — M0 resolution is fully
//! deterministic (no stochastic mechanic exists until M6's mineral density re-roll), and the
//! `Simulation` trait's `step(world, intents)` cannot thread one. The conservation fuzz uses
//! sim-core's seeded RNG at intent-GENERATION time instead.

use crate::constants::{
    body_cost, controller_downgrade, controller_levels, BUILD_POWER, BUILD_RANGE, CONTAINER_DECAY,
    CONTAINER_DECAY_TIME, CONTROLLER_DOWNGRADE_RESTORE, CONTROLLER_MAX_UPGRADE_PER_TICK,
    CREEP_LIFE_TIME, CREEP_SPAWN_TIME, DROPPED_DECAY_DIVISOR, ENERGY_REGEN_TIME, MAX_CREEP_SIZE,
    REPAIR_HITS_PER_ENERGY, REPAIR_POWER, REPAIR_RANGE, ROAD_DECAY_AMOUNT, ROAD_DECAY_TIME,
    ROAD_SWAMP_RATIO, SPAWN_ENERGY_CAPACITY, UPGRADE_CONTROLLER_POWER,
};
use crate::intents::{EconAction, EconIntents, StructRef};
use crate::ledger::{audit_conservation, ConservationViolation, TickLedger};
use crate::state::{
    creep_store_capacity, EconWorld, PendingCreep, SimResource, SimStore, StructureKind,
};
use screeps::{Part, Position};
use screeps_sim_core::{resolve_movement, CreepId, MovementReport, SimBody, SimCreep, Simulation};
use std::collections::{BTreeMap, BTreeSet};

/// Repair energy spent while the room had a spawn/extension refill deficit, by structure class —
/// the sim mirror of the live `repair_leak_e` counter (ADR 0040 §D6; `energy_stress.rs`
/// `record_repair_leak`). The deficit condition is ANY deficit (`spawn+extension energy <
/// capacity` — energy_stress.rs:134, `spawn_energy < spawn_energy_capacity`), deliberately
/// DIFFERENT from the S1 gate's 10%/10k condition: the counter measures the symptom wherever it
/// occurs. Evaluated ONCE at tick start (after the extension re-cap), mirroring the live pre-pass
/// `EconomySnapshot` the counter reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairLeak {
    pub roads: u64,
    pub containers: u64,
    pub other: u64,
}

impl RepairLeak {
    pub fn total(&self) -> u64 {
        self.roads + self.containers + self.other
    }
}

/// Outcome of one economy tick.
#[derive(Clone, Debug, Default)]
pub struct EconTickReport {
    /// The tick this report is for (pre-increment).
    pub tick: u32,
    /// This tick's exact flow accounting.
    pub ledger: TickLedger,
    /// The conservation audit's verdict: empty = exact balance. SURFACED here (not panic-only)
    /// so the eval harness can gate on imbalance (EP-6.12); also `debug_assert!`ed.
    pub conservation: Vec<ConservationViolation>,
    /// The kernel's movement outcome.
    pub movement: MovementReport,
    /// `(spawn_idx, creep_id)` for spawns STARTED this tick.
    pub spawns_started: Vec<(usize, CreepId)>,
    /// Creeps that materialized (finished spawning) this tick.
    pub births: Vec<CreepId>,
    /// Creeps that died of TTL this tick.
    pub deaths: Vec<CreepId>,
    /// Actions dropped by validation (bad index / not adjacent / busy spawn / unaffordable /
    /// illegal body / unknown creep / zero-effect repair or build / RCL8 upgrade cap) or by the
    /// Pipeline-A/D/E mask.
    pub rejected_actions: u32,
    /// The per-tick `repair_leak_e` mirror ([`RepairLeak`] — repair energy spent under refill
    /// deficit, by class).
    pub repair_leak: RepairLeak,
    /// Construction sites COMPLETED this tick, as `(kind, pos)` (M2).
    pub sites_completed: Vec<(StructureKind, screeps::Position)>,
    /// Controller level-ups this tick: the NEW level (M2; multiple upgraders can chain at most
    /// one level-up per tick in practice — the half-max clock gate blocks immediate repeats).
    pub level_ups: Vec<u8>,
    /// Controller downgrade events this tick: the NEW level (M2; 0 = ownership lost).
    pub downgrades: Vec<u8>,
    /// The controller's `(level, progress, downgrade_ticks)` AFTER this tick (M2) — the
    /// level/progress series the eval samples.
    pub controller: Option<(u8, u32, u32)>,
}

/// The economy layer's `Simulation` binding: drive [`EconWorld`] with [`EconIntents`] through
/// [`resolve_econ_tick`] — the same contract `MovementSim` and the combat layer satisfy.
pub struct EconSim;

impl Simulation for EconSim {
    type World = EconWorld;
    type Intents = EconIntents;
    type Report = EconTickReport;

    fn step(world: &mut EconWorld, intents: &EconIntents) -> EconTickReport {
        resolve_econ_tick(world, intents)
    }
}

/// Resolve one economy tick. See the module docs for the pipeline order + deviations.
pub fn resolve_econ_tick(world: &mut EconWorld, intents: &EconIntents) -> EconTickReport {
    let tick = world.movement.tick;
    let prev_stocks = world.stocks();
    let mut ledger = TickLedger::default();
    let mut report = EconTickReport { tick, ..Default::default() };

    // ── 0. Extension re-cap from the CURRENT controller level (engine `extensions/tick.js:11`;
    // engine-mechanics.md:456). Controller-less worlds keep builder capacities (module docs). ────
    if let Some(level) = world.controller.as_ref().map(|c| c.level) {
        for e in &mut world.extensions {
            e.capacity = crate::constants::extension_capacity(level);
        }
    }

    // The refill-deficit snapshot for the repair_leak_e mirror: ANY spawn/extension deficit at
    // tick start, after the re-cap (the live counter reads the pre-pass EconomySnapshot —
    // energy_stress.rs:132-135; see `RepairLeak`).
    let refill_deficit = {
        let capacity = world.spawns.len() as u64 * SPAWN_ENERGY_CAPACITY as u64
            + world.extensions.iter().map(|e| e.capacity as u64).sum::<u64>();
        (world.room_spawn_energy() as u64) < capacity
    };

    // ── 1. Mask: stable-sort by creep id; one Pipeline-A + one Pipeline-D + one Pipeline-E
    // action per creep (module docs — Build joins A, UpgradeController is its OWN lane E). ──────
    let mut order: Vec<usize> = (0..intents.actions.len()).collect();
    order.sort_by_key(|&i| intents.actions[i].0); // stable → (creep id, submission order)

    let mut pipeline_used: BTreeMap<CreepId, (bool, bool, bool)> = BTreeMap::new(); // (A, D, E)
    // The Pipeline-A work lane: Harvest OR Repair OR Build, one per creep per tick (a repair
    // masks the harvest — the S1 leak mechanic, `jobs/actions.rs:27-34`).
    let mut work_actions: Vec<(CreepId, &EconAction)> = Vec::new();
    let mut transfer_class: Vec<(CreepId, &EconAction)> = Vec::new();
    // The Pipeline-E upgrade lane (`jobs/actions.rs:50-51`; conflict-free in the engine matrix).
    let mut upgrade_actions: Vec<CreepId> = Vec::new();
    // (spawn_idx, submission index, body): sorted before step 4b so the paired creep id and the
    // cross-spawn emission order are genuinely non-semantic (module docs, step 4).
    let mut spawn_reqs: Vec<(usize, usize, &Vec<Part>)> = Vec::new();

    for &i in &order {
        let (creep_id, action) = &intents.actions[i];
        match action {
            EconAction::SpawnCreep { spawn_idx, body } => {
                // A STRUCTURE intent: the paired creep id is ignored; not pipeline-masked. `i` is
                // the submission index — the within-spawn first-wins key.
                spawn_reqs.push((*spawn_idx, i, body));
            }
            EconAction::Harvest { .. } | EconAction::Repair { .. } | EconAction::Build { .. } => {
                let used = pipeline_used.entry(*creep_id).or_insert((false, false, false));
                if used.0 {
                    report.rejected_actions += 1; // second Pipeline-A action this tick
                } else {
                    used.0 = true;
                    work_actions.push((*creep_id, action));
                }
            }
            EconAction::Transfer { .. } | EconAction::Withdraw { .. } | EconAction::Pickup { .. } => {
                let used = pipeline_used.entry(*creep_id).or_insert((false, false, false));
                if used.1 {
                    report.rejected_actions += 1; // second Pipeline-D action this tick
                } else {
                    used.1 = true;
                    transfer_class.push((*creep_id, action));
                }
            }
            EconAction::UpgradeController => {
                let used = pipeline_used.entry(*creep_id).or_insert((false, false, false));
                if used.2 {
                    report.rejected_actions += 1; // second Pipeline-E action this tick
                } else {
                    used.2 = true;
                    upgrade_actions.push(*creep_id);
                }
            }
        }
    }

    // ── 2. Transfer / Withdraw / Pickup (creep-id order; adjacency-1; atomic) — BEFORE harvest,
    // the engine's within-creep intent order (`creeps/intents.js:15`; module docs step 2). ──────
    for (creep_id, action) in transfer_class {
        let Some(creep_pos) = world.creep(creep_id).filter(|c| c.is_alive()).map(|c| c.pos) else {
            report.rejected_actions += 1;
            continue;
        };
        match action {
            EconAction::Transfer { target, resource, amount } => {
                let Some(target_pos) = target_pos(world, *target) else {
                    report.rejected_actions += 1;
                    continue;
                };
                if creep_pos.get_range_to(target_pos) > 1 || !target_takes(*target, *resource) {
                    report.rejected_actions += 1;
                    continue;
                }
                let have = world.creep_stores.get(&creep_id).map(|s| s.amount(*resource)).unwrap_or(0);
                if have < *amount {
                    // Engine: a transfer over-ask is rejected WHOLE (ERR_NOT_ENOUGH_RESOURCES).
                    report.rejected_actions += 1;
                    continue;
                }
                let moved = (*amount).min(target_free(world, *target));
                if moved > 0 {
                    if let Some(store) = world.creep_stores.get_mut(&creep_id) {
                        store.remove(*resource, moved);
                    }
                    target_add(world, *target, *resource, moved);
                    world.sync_carry_used(creep_id);
                }
            }
            EconAction::Withdraw { target, resource, amount } => {
                let Some(target_pos) = target_pos(world, *target) else {
                    report.rejected_actions += 1;
                    continue;
                };
                if creep_pos.get_range_to(target_pos) > 1 || !target_takes(*target, *resource) {
                    report.rejected_actions += 1;
                    continue;
                }
                let free = world.creep_stores.get(&creep_id).map(SimStore::free).unwrap_or(0);
                let moved = (*amount).min(target_available(world, *target, *resource)).min(free);
                if moved > 0 {
                    target_remove(world, *target, *resource, moved);
                    if let Some(store) = world.creep_stores.get_mut(&creep_id) {
                        store.add(*resource, moved);
                    }
                    world.sync_carry_used(creep_id);
                }
            }
            EconAction::Pickup { dropped_idx } => {
                let Some((pile_pos, pile_resource, pile_amount)) =
                    world.dropped.get(*dropped_idx).map(|p| (p.pos, p.resource, p.amount))
                else {
                    report.rejected_actions += 1;
                    continue;
                };
                if creep_pos.get_range_to(pile_pos) > 1 {
                    report.rejected_actions += 1;
                    continue;
                }
                let free = world.creep_stores.get(&creep_id).map(SimStore::free).unwrap_or(0);
                let moved = pile_amount.min(free);
                if moved > 0 {
                    world.dropped[*dropped_idx].amount -= moved;
                    if let Some(store) = world.creep_stores.get_mut(&creep_id) {
                        store.add(pile_resource, moved);
                    }
                    world.sync_carry_used(creep_id);
                }
            }
            _ => unreachable!("only Pipeline-D actions reach the transfer lane"),
        }
    }

    // ── 3. Work lane: Harvest + Repair (creep-id order), then source regen. ────────────────────
    for (creep_id, action) in work_actions {
        match action {
            EconAction::Harvest { source_idx } => {
                let Some((creep_pos, work_power)) = world
                    .creep(creep_id)
                    .filter(|c| c.is_alive())
                    .map(|c| (c.pos, c.body.effective_power(Part::Work, crate::constants::HARVEST_POWER)))
                else {
                    report.rejected_actions += 1;
                    continue;
                };
                let Some(source) = world.sources.get_mut(*source_idx) else {
                    report.rejected_actions += 1;
                    continue;
                };
                if creep_pos.get_range_to(source.pos) > 1 {
                    report.rejected_actions += 1;
                    continue;
                }
                // gain = min(2 × WORK, source.energy) — HARVEST_POWER, engine-mechanics.md:457.
                let gain = work_power.min(source.energy);
                source.energy -= gain;
                ledger.harvested += gain as u64;
                let accepted = match world.creep_stores.get_mut(&creep_id) {
                    Some(store) => store.add(SimResource::Energy, gain),
                    None => 0,
                };
                // Store overflow spills to the creep's tile (the engine's drop-overflow step).
                let overflow = gain - accepted;
                if overflow > 0 {
                    world.drop_resource(creep_pos, SimResource::Energy, overflow);
                }
                world.sync_carry_used(creep_id);
            }
            EconAction::Repair { target } => {
                // Engine `creeps/repair.js` (engine-mechanics.md:118): range ≤ 3, energy > 0,
                // effect = min(WORK × REPAIR_POWER, energy × 100, missing hits), cost =
                // ceil(effect / 100) — all exact integers (`REPAIR_HITS_PER_ENERGY` docs).
                // ALIVE-work is moot in-sim: no partial body damage is modeled, every WORK part
                // is always alive (M1 spec note).
                let Some((creep_pos, work_power)) = world
                    .creep(creep_id)
                    .filter(|c| c.is_alive())
                    .map(|c| (c.pos, c.body.effective_power(Part::Work, REPAIR_POWER)))
                else {
                    report.rejected_actions += 1;
                    continue;
                };
                // The M1 hit-bearing targets: roads + containers. Store-only structures
                // (spawn/extension/storage) carry no hits model in-sim — rejected (deviation:
                // the engine would repair them; M2 extends the model, `repair_other` waits).
                let target_state = match target {
                    StructRef::Road(i) => world.roads.get(*i).map(|r| (r.pos, r.hits, r.hits_max)),
                    StructRef::Container(i) => {
                        world.containers.get(*i).map(|c| (c.pos, c.hits, crate::constants::CONTAINER_HITS))
                    }
                    _ => None,
                };
                let Some((target_pos, hits, hits_max)) = target_state else {
                    report.rejected_actions += 1;
                    continue;
                };
                let energy = world.creep_stores.get(&creep_id).map(|s| s.amount(SimResource::Energy)).unwrap_or(0);
                if creep_pos.get_range_to(target_pos) > REPAIR_RANGE || energy == 0 || hits >= hits_max {
                    report.rejected_actions += 1; // out of range / no energy / full target
                    continue;
                }
                let effect = work_power
                    .min(energy.saturating_mul(REPAIR_HITS_PER_ENERGY))
                    .min(hits_max - hits);
                if effect == 0 {
                    report.rejected_actions += 1; // no WORK parts
                    continue;
                }
                let cost = effect.div_ceil(REPAIR_HITS_PER_ENERGY);
                match target {
                    StructRef::Road(i) => {
                        world.roads[*i].hits += effect;
                        ledger.repair_roads += cost as u64;
                        if refill_deficit {
                            report.repair_leak.roads += cost as u64;
                        }
                    }
                    StructRef::Container(i) => {
                        world.containers[*i].hits += effect;
                        ledger.repair_containers += cost as u64;
                        if refill_deficit {
                            report.repair_leak.containers += cost as u64;
                        }
                    }
                    _ => unreachable!("validated above"),
                }
                if let Some(store) = world.creep_stores.get_mut(&creep_id) {
                    store.remove(SimResource::Energy, cost);
                }
                world.sync_carry_used(creep_id);
            }
            EconAction::Build { site_idx } => {
                // Engine `creeps/build.js` (M2): range ≤ 3 (:23), energy > 0 (:14); an
                // obstacle-kind site rejects while an obstacle object or ANY creep occupies the
                // tile (:50-60); effect = min(5 × WORK, remaining, energy) at 1 energy/progress
                // (:67-69,83). Completed sites materialize after the loop (module docs step 3).
                let Some((creep_pos, work_parts)) = world
                    .creep(creep_id)
                    .filter(|c| c.is_alive())
                    .map(|c| (c.pos, c.body.alive_part_count(Part::Work)))
                else {
                    report.rejected_actions += 1;
                    continue;
                };
                let Some((site_pos, site_kind, remaining)) =
                    world.sites.get(*site_idx).map(|s| (s.pos, s.kind, s.total.saturating_sub(s.progress)))
                else {
                    report.rejected_actions += 1;
                    continue;
                };
                if creep_pos.get_range_to(site_pos) > BUILD_RANGE {
                    report.rejected_actions += 1;
                    continue;
                }
                if site_kind.blocks_movement()
                    && (world.obstacle_object_at(site_pos)
                        || world.movement.creeps.iter().any(|c| c.is_alive() && c.pos == site_pos))
                {
                    // build.js:50-60 — an obstacle-type structure cannot complete under an
                    // obstacle object or a creep (the builder standing ON its own site blocks it).
                    report.rejected_actions += 1;
                    continue;
                }
                let energy = world.creep_stores.get(&creep_id).map(|s| s.amount(SimResource::Energy)).unwrap_or(0);
                // effect = min(5 × WORK, remaining, energy) — BUILD_POWER, 1 energy/progress.
                let effect = (work_parts * BUILD_POWER).min(remaining).min(energy);
                if effect == 0 {
                    // No energy / no WORK / already-complete site: the engine no-ops; counted.
                    report.rejected_actions += 1;
                    continue;
                }
                world.sites[*site_idx].progress += effect;
                ledger.build += effect as u64;
                if let Some(store) = world.creep_stores.get_mut(&creep_id) {
                    store.remove(SimResource::Energy, effect);
                }
                world.sync_carry_used(creep_id);
            }
            _ => unreachable!("only Pipeline-A actions reach the work lane"),
        }
    }

    // ── 3b. Materialize completed sites (ascending index; engine `build.js:108-293`), then
    // compact. Structure-specific birth state per the engine's insert blocks (module docs). ─────
    let completed: Vec<usize> =
        (0..world.sites.len()).filter(|&i| world.sites[i].progress >= world.sites[i].total).collect();
    for &i in &completed {
        let (pos, kind) = (world.sites[i].pos, world.sites[i].kind);
        match kind {
            StructureKind::Spawn => {
                // build.js:119-128: a BUILT spawn starts with store {energy: 0} — deliberately
                // unlike the scenario builder add_spawn's born-full convention.
                let id = world.mint_structure_id();
                world.spawns.push(crate::state::SimSpawn { id, pos, store_energy: 0, spawning: None });
            }
            StructureKind::Extension => {
                // build.js:130-137 inserts capacity 0 and the extension tick recomputes from the
                // CURRENT controller level (`extensions/tick.js:11`); materializing at the
                // current level is identical one step earlier (step 0 re-caps every tick).
                let level = world.controller.as_ref().map(|c| c.level).unwrap_or(0);
                let id = world.mint_structure_id();
                world.extensions.push(crate::state::SimExtension {
                    id,
                    pos,
                    store_energy: 0,
                    capacity: crate::constants::extension_capacity(level),
                });
            }
            StructureKind::Storage => {
                // build.js:151-158 (placement allowance keeps this to one per room).
                if world.storage.is_none() {
                    world.set_storage(pos, crate::constants::STORAGE_CAPACITY);
                }
            }
            StructureKind::Container => {
                // build.js:255-263: 250K hits, nextDecayTime = gameTime + CONTAINER_DECAY_TIME
                // (100 FLAT — the ownership-aware 500 window only applies from the first decay
                // event on, containers/tick.js:26).
                let id = world.mint_structure_id();
                world.containers.push(crate::state::SimContainer {
                    id,
                    pos,
                    store: SimStore::with_capacity(crate::constants::CONTAINER_CAPACITY),
                    hits: crate::constants::CONTAINER_HITS,
                    next_decay_at: tick + CONTAINER_DECAY_TIME,
                });
            }
            StructureKind::Road => {
                // build.js:171-186: hits = hitsMax = ROAD_HITS × swamp ratio; nextDecayTime a
                // full window out; the movement-terrain effect registers via the SAFE path.
                let swamp = {
                    let key = (pos.x().u8(), pos.y().u8());
                    world.movement.terrain_for(pos.room_name()).swamps.contains(&key)
                };
                let max = crate::constants::road_hits_max(swamp);
                world.register_road_tile(pos);
                world.roads.push(crate::state::SimRoad {
                    pos,
                    hits: max,
                    hits_max: max,
                    next_decay_at: tick + ROAD_DECAY_TIME,
                });
            }
            StructureKind::Tower => {
                world.add_tower(pos); // stub furniture (module docs)
            }
        }
        report.sites_completed.push((kind, pos));
    }
    for &i in completed.iter().rev() {
        world.sites.remove(i);
    }

    // Source regen (after harvest, as the engine's source tick follows the intent stage):
    // refill when `tick >= regen_at − 1` (engine-mechanics.md:445); THEN start the timer on any
    // source below capacity with no timer running (the first-harvest-below-cap start, same line).
    for source in &mut world.sources {
        if let Some(regen_at) = source.regen_at {
            if tick + 1 >= regen_at {
                source.energy = source.capacity;
                source.regen_at = None;
            }
        }
        if source.energy < source.capacity && source.regen_at.is_none() {
            source.regen_at = Some(tick + ENERGY_REGEN_TIME);
        }
    }

    // ── 3c. Upgrade lane (Pipeline E, creep-id order; engine `creeps/upgradeController.js` —
    // module docs). `upgraded_this_tick` is the controller's per-tick `_upgraded` accumulator:
    // it shares the RCL8 15 e/t cap across upgraders (:42-52) AND gates the clock restore in
    // step 3d (:38 — truthy means ≥ 1 energy actually converted). ───────────────────────────────
    let mut upgraded_this_tick: u32 = 0;
    for creep_id in upgrade_actions {
        let Some((creep_pos, work_parts)) = world
            .creep(creep_id)
            .filter(|c| c.is_alive())
            .map(|c| (c.pos, c.body.alive_part_count(Part::Work)))
        else {
            report.rejected_actions += 1;
            continue;
        };
        // Controller present, owned (level ≥ 1 — a level-0 controller rejects, :24), in range 3
        // (:21-23 — Chebyshev).
        let Some((level, cpos)) = world.controller.as_ref().map(|c| (c.level, c.pos)) else {
            report.rejected_actions += 1;
            continue;
        };
        if level == 0 || creep_pos.get_range_to(cpos) > 3 {
            report.rejected_actions += 1;
            continue;
        }
        let energy = world.creep_stores.get(&creep_id).map(|s| s.amount(SimResource::Energy)).unwrap_or(0);
        if energy == 0 {
            report.rejected_actions += 1; // :9 — store.energy <= 0 rejects
            continue;
        }
        // effect = min(WORK × UPGRADE_CONTROLLER_POWER, energy) (:33-34). Unboosted alive-WORK
        // count (boost multipliers for upgradeController differ from the combat action table —
        // an M6 concern; every M2 body is unboosted).
        let mut effect = (work_parts * UPGRADE_CONTROLLER_POWER).min(energy);
        if level == 8 {
            // The room-wide 15 e/t cap, shared via the accumulator (:42-52).
            if upgraded_this_tick >= CONTROLLER_MAX_UPGRADE_PER_TICK {
                report.rejected_actions += 1; // capped out: full no-op, no energy spent (:48-50)
                continue;
            }
            effect = effect.min(CONTROLLER_MAX_UPGRADE_PER_TICK - upgraded_this_tick);
        }
        // NOTE (review B10): NO zero-effect guard before the level-up check — the engine has
        // none between the :9 energy gate and the :67 check, so a 0-WORK creep CARRYING energy
        // can trigger the level-up in the surplus window (progress already past the threshold
        // while the clock gate was low: `progress + 0 >= next`, :67). Its `_upgraded += 0`
        // stays FALSY — no clock restore that tick (tick.js:38's truthy gate).
        let mut leveled = false;
        if level < 8 {
            let threshold = controller_levels(level).expect("level 1..=7 has a next-level cost");
            let full = controller_downgrade(level);
            let c = world.controller.as_mut().expect("checked above");
            // The level-up gate (:67-68): progress crosses the threshold AND the clock is within
            // CONTROLLER_DOWNGRADE_RESTORE of full (countdown translation:
            // `downgradeTime + 100 >= gameTime + FULL` ⟺ `remaining + 100 >= FULL`).
            if c.progress + effect >= threshold && c.downgrade_ticks + CONTROLLER_DOWNGRADE_RESTORE >= full {
                c.progress = c.progress + effect - threshold; // remainder carries (:70)
                c.level += 1;
                // Clock resets to HALF the NEW level's max (:72); the same tick's step-3d
                // restore then adds its +100 (the engine's tick.js:39 runs after the intent) —
                // unless effect == 0 (falsy accumulator: the new clock DECAYS this tick instead).
                c.downgrade_ticks = controller_downgrade(c.level) / 2;
                if c.level == 8 {
                    c.progress = 0; // :74-76
                }
                report.level_ups.push(c.level);
                leveled = true;
            } else if effect > 0 {
                c.progress += effect; // :80 — accumulates PAST the threshold while the clock is low
            }
        }
        if effect == 0 && !leveled {
            report.rejected_actions += 1; // zero conversion AND no level-up: a counted no-op
            continue;
        }
        // Level 8: no progress change (:59 guards the whole block); energy still spent (:92).
        upgraded_this_tick += effect;
        ledger.upgrade += effect as u64;
        if effect > 0 {
            if let Some(store) = world.creep_stores.get_mut(&creep_id) {
                store.remove(SimResource::Energy, effect);
            }
            world.sync_carry_used(creep_id);
        }
    }

    // ── 3d. Controller step (engine `controllers/tick.js`; countdown translation pinned at
    // `SimController`). Skipped for unowned/level-0 controllers (:14). ──────────────────────────
    if let Some(c) = world.controller.as_mut() {
        if c.level > 0 {
            if upgraded_this_tick > 0 {
                // :38-43 — restore ONCE per tick-with-upgrade, capped at the full clock
                // (`min(D + 101, g + FULL + 1)` at gameTime g ⟹ next-tick remaining
                // `min(R + 100, FULL)`), and NO downgrade check this tick (:43 returns).
                c.downgrade_ticks =
                    (c.downgrade_ticks + CONTROLLER_DOWNGRADE_RESTORE).min(controller_downgrade(c.level));
            } else if c.downgrade_ticks <= 1 {
                // :49 — expiry at `gameTime >= downgradeTime − 1` ⟺ remaining ≤ 1.
                c.level -= 1;
                if c.level == 0 {
                    c.progress = 0; // :52-62 — ownership lost, progress zeroed
                } else {
                    // :65-66 — clock re-arms at +FULL[new]/2 (net of this tick), progress gains
                    // round(0.9 × CONTROLLER_LEVELS[new]) — exact ×9/10 by the pinned table.
                    c.downgrade_ticks += controller_downgrade(c.level) / 2;
                    c.progress += controller_levels(c.level).expect("level 1..=7") / 10 * 9;
                }
                report.downgrades.push(c.level);
            } else {
                c.downgrade_ticks -= 1;
            }
        }
    }
    report.controller = world.controller.as_ref().map(|c| (c.level, c.progress, c.downgrade_ticks));

    // ── 4a. Spawn completions (index order) — BEFORE new intents, so a spawn is busy exactly
    // 3×parts ticks and can accept a new request the tick its creep walks out. ─────────────────
    for i in 0..world.spawns.len() {
        let Some(done_at) = world.spawns[i].spawning.as_ref().map(|p| p.done_at) else { continue };
        if tick < done_at {
            continue;
        }
        let spawn_pos = world.spawns[i].pos;
        match birth_tile(world, spawn_pos) {
            Some(tile) => {
                let pending: PendingCreep = world.spawns[i].spawning.take().expect("checked above");
                let body = SimBody::unboosted(&pending.body);
                let capacity = creep_store_capacity(&body);
                world.movement.creeps.push(SimCreep {
                    id: pending.id,
                    owner: 0,
                    pos: tile,
                    body,
                    fatigue: 0,
                    carry_used: 0,
                });
                world.creep_stores.insert(pending.id, SimStore::with_capacity(capacity));
                world.creep_ttl.insert(pending.id, tick + CREEP_LIFE_TIME);
                report.births.push(pending.id);
            }
            None => {
                // Blocked exit: completion slips +1 tick (coarse model — engine-mechanics.md:242).
                world.spawns[i].spawning.as_mut().expect("checked above").done_at += 1;
            }
        }
    }

    // ── 4b. New SpawnCreep intents in (spawn index, submission order) — cross-spawn contention is
    // emission-order-independent, same-spawn duplicates are first-submitted-wins (module docs,
    // step 4); requests debit sequentially against mutated stores, matching the engine's
    // same-tick multi-spawn behavior (engine-mechanics.md:257). ─────────────────────────────────
    spawn_reqs.sort_by_key(|&(spawn_idx, submission_idx, _)| (spawn_idx, submission_idx));
    for (spawn_idx, _, body) in spawn_reqs {
        let Some(spawn) = world.spawns.get(spawn_idx) else {
            report.rejected_actions += 1;
            continue;
        };
        if spawn.spawning.is_some() || body.is_empty() || body.len() > MAX_CREEP_SIZE {
            report.rejected_actions += 1;
            continue;
        }
        let cost = body_cost(body);
        if cost > world.room_spawn_energy() {
            // Fail WHOLE: nothing debited (the atomic charge — engine-mechanics.md:257).
            report.rejected_actions += 1;
            continue;
        }
        debit_spawn_cost(world, spawn_idx, cost);
        ledger.spawn_bodies += cost as u64;
        let id = world.mint_creep_id();
        world.spawns[spawn_idx].spawning = Some(PendingCreep {
            id,
            body: body.clone(),
            done_at: tick + CREEP_SPAWN_TIME * body.len() as u32,
        });
        report.spawns_started.push((spawn_idx, id));
    }

    // ── 5. Spawn self-charge: +1/tick per spawn while room spawn+extension energy < 300
    // (engine-mechanics.md:279); the room total is read once, before any charging. ──────────────
    if world.room_spawn_energy() < SPAWN_ENERGY_CAPACITY {
        for spawn in &mut world.spawns {
            if spawn.store_energy < SPAWN_ENERGY_CAPACITY {
                spawn.store_energy += 1;
                ledger.spawn_self_charge += 1;
            }
        }
    }

    // ── 6. Creep TTL: `creep_ttl` is the engine's ageTime — death fires on the first tick where
    // tick + 1 >= ageTime (the engine's `gameTime >= ageTime − 1`, engine-mechanics.md:57); the
    // whole store drops to ground at the creep's position (tombstone-less — documented deviation,
    // module docs). ─────────────────────────────────────────────────────────────────────────────
    let dead: Vec<CreepId> =
        world.creep_ttl.iter().filter(|&(_, &d)| tick + 1 >= d).map(|(&id, _)| id).collect();
    for id in dead {
        world.creep_ttl.remove(&id);
        if let Some(pos) = world.creep(id).map(|c| c.pos) {
            if let Some(store) = world.creep_stores.remove(&id) {
                for (r, v) in store.contents {
                    world.drop_resource(pos, r, v);
                }
            }
        }
        world.movement.creeps.retain(|c| c.id != id);
        report.deaths.push(id);
    }

    // ── 6b. Structure decay (M1; module docs): roads then containers, index order; events fire
    // at tick >= next_decay_at − 1 (`roads/tick.js:10` / `containers/tick.js:10`); dead
    // structures are removed (compaction — indices were only read pre-compaction this tick). ────
    let mut dead_roads: Vec<usize> = Vec::new();
    for i in 0..world.roads.len() {
        if tick + 1 < world.roads[i].next_decay_at {
            continue;
        }
        let pos = world.roads[i].pos;
        let key = (pos.x().u8(), pos.y().u8());
        // Terrain ratio: swamp ×5 (engine-mechanics.md:430; wall-tunnel ×150 not modeled).
        let swamp = world.movement.terrain_for(pos.room_name()).swamps.contains(&key);
        let amount = ROAD_DECAY_AMOUNT * if swamp { ROAD_SWAMP_RATIO } else { 1 };
        let road = &mut world.roads[i];
        road.hits = road.hits.saturating_sub(amount);
        if road.hits == 0 {
            dead_roads.push(i);
        } else {
            road.next_decay_at = tick + ROAD_DECAY_TIME;
        }
    }
    for &i in dead_roads.iter().rev() {
        // A dead road leaves its natural terrain behind: de-register the movement-tile effect
        // (`roads/tick.js:19-21` removes the object; fatigue reverts to plain/swamp).
        let pos = world.roads[i].pos;
        world.deregister_road_tile(pos);
        world.roads.remove(i);
    }

    let container_window = world.container_decay_window();
    let mut dead_containers: Vec<usize> = Vec::new();
    for i in 0..world.containers.len() {
        if tick + 1 < world.containers[i].next_decay_at {
            continue;
        }
        let container = &mut world.containers[i];
        container.hits = container.hits.saturating_sub(CONTAINER_DECAY);
        if container.hits == 0 {
            dead_containers.push(i);
        } else {
            container.next_decay_at = tick + container_window;
        }
    }
    for &i in dead_containers.iter().rev() {
        // Death drops the WHOLE store to the ground (`containers/tick.js:13-22` via
        // _create-energy) — stock relocation, no ledger entry; step 7 books the pile decay.
        let pos = world.containers[i].pos;
        let store = std::mem::take(&mut world.containers[i].store);
        for (r, v) in store.contents {
            world.drop_resource(pos, r, v);
        }
        world.containers.remove(i);
    }

    // ── 7. Dropped decay: ceil(amount/1000) per pile per tick (engine-mechanics.md:431);
    // exhausted piles are compacted away (indices are only ever read pre-compaction). ───────────
    for pile in &mut world.dropped {
        if pile.amount == 0 {
            continue;
        }
        let dec = pile.amount.div_ceil(DROPPED_DECAY_DIVISOR);
        pile.amount -= dec;
        *ledger.dropped_decay.entry(pile.resource).or_insert(0) += dec as u64;
    }
    world.dropped.retain(|p| p.amount > 0);

    // ── 8. Movement (the kernel call; the tick advances inside). Moves referencing creeps that
    // died this tick are stripped first. ────────────────────────────────────────────────────────
    let alive: BTreeSet<CreepId> =
        world.movement.creeps.iter().filter(|c| c.is_alive()).map(|c| c.id).collect();
    let mut moves = intents.moves.clone();
    moves.moves.retain(|id, _| alive.contains(id));
    moves.pulls.retain(|puller, target| alive.contains(puller) && alive.contains(target));
    let before_positions: BTreeMap<CreepId, Position> =
        world.movement.creeps.iter().map(|c| (c.id, c.pos)).collect();
    report.movement = resolve_movement(&mut world.movement, &moves);

    // Traffic wear (M1; module docs step 8): each creep that STEPPED onto a road tile pulls the
    // road's decay clock forward ROAD_WEAROUT × body parts (engine `movement.js:215-219`).
    let steps: Vec<(Position, u32)> = world
        .movement
        .creeps
        .iter()
        .filter(|c| before_positions.get(&c.id).is_some_and(|&old| old != c.pos))
        .map(|c| (c.pos, c.body.parts.len() as u32))
        .collect();
    for (pos, parts) in steps {
        world.apply_road_wear(pos, parts);
    }

    // ── 9. Conservation audit: exact per-resource integer balance, surfaced (EP-6.12). ─────────
    let now_stocks = world.stocks();
    report.ledger = ledger;
    report.conservation = audit_conservation(&prev_stocks, &report.ledger, &now_stocks);
    debug_assert!(
        report.conservation.is_empty(),
        "energy conservation violated at tick {tick}: {:?}",
        report.conservation
    );
    report
}

// ── Structure-store plumbing (spawns/extensions are energy-only; containers/storage general) ────

fn target_pos(world: &EconWorld, target: StructRef) -> Option<Position> {
    match target {
        StructRef::Spawn(i) => world.spawns.get(i).map(|s| s.pos),
        StructRef::Extension(i) => world.extensions.get(i).map(|e| e.pos),
        StructRef::Container(i) => world.containers.get(i).map(|c| c.pos),
        StructRef::Storage => world.storage.as_ref().map(|s| s.pos),
        StructRef::Road(i) => world.roads.get(i).map(|r| r.pos),
    }
}

/// Whether the target's store can hold `resource` at all (spawns/extensions are energy-only;
/// roads have no store — a transfer/withdraw naming one is rejected).
fn target_takes(target: StructRef, resource: SimResource) -> bool {
    match target {
        StructRef::Spawn(_) | StructRef::Extension(_) => resource == SimResource::Energy,
        StructRef::Container(_) | StructRef::Storage => true,
        StructRef::Road(_) => false,
    }
}

fn target_free(world: &EconWorld, target: StructRef) -> u32 {
    match target {
        StructRef::Spawn(i) => SPAWN_ENERGY_CAPACITY.saturating_sub(world.spawns[i].store_energy),
        StructRef::Extension(i) => {
            let e = &world.extensions[i];
            e.capacity.saturating_sub(e.store_energy)
        }
        StructRef::Container(i) => world.containers[i].store.free(),
        StructRef::Storage => world.storage.as_ref().map(|s| s.store.free()).unwrap_or(0),
        StructRef::Road(_) => 0, // storeless (gated off by `target_takes` before any use)
    }
}

fn target_available(world: &EconWorld, target: StructRef, resource: SimResource) -> u32 {
    match target {
        StructRef::Spawn(i) => world.spawns[i].store_energy,
        StructRef::Extension(i) => world.extensions[i].store_energy,
        StructRef::Container(i) => world.containers[i].store.amount(resource),
        StructRef::Storage => world.storage.as_ref().map(|s| s.store.amount(resource)).unwrap_or(0),
        StructRef::Road(_) => 0, // storeless (gated off by `target_takes` before any use)
    }
}

fn target_add(world: &mut EconWorld, target: StructRef, resource: SimResource, amount: u32) {
    match target {
        StructRef::Spawn(i) => world.spawns[i].store_energy += amount,
        StructRef::Extension(i) => world.extensions[i].store_energy += amount,
        StructRef::Container(i) => {
            world.containers[i].store.add(resource, amount);
        }
        StructRef::Storage => {
            if let Some(s) = world.storage.as_mut() {
                s.store.add(resource, amount);
            }
        }
        StructRef::Road(_) => unreachable!("roads are storeless — target_takes gates them off"),
    }
}

fn target_remove(world: &mut EconWorld, target: StructRef, resource: SimResource, amount: u32) {
    match target {
        StructRef::Spawn(i) => world.spawns[i].store_energy -= amount,
        StructRef::Extension(i) => world.extensions[i].store_energy -= amount,
        StructRef::Container(i) => {
            world.containers[i].store.remove(resource, amount);
        }
        StructRef::Storage => {
            if let Some(s) = world.storage.as_mut() {
                s.store.remove(resource, amount);
            }
        }
        StructRef::Road(_) => unreachable!("roads are storeless — target_takes gates them off"),
    }
}

/// The deterministic birth tile: among the 8 neighbors of `spawn_pos`, the walkable one with the
/// lowest packed (y, x) — row-major, matching the reading order of a room. `None` = fully blocked
/// (the caller slips the completion +1 tick).
fn birth_tile(world: &EconWorld, spawn_pos: Position) -> Option<Position> {
    let mut candidates: Vec<Position> = Vec::with_capacity(8);
    for dy in -1i8..=1 {
        for dx in -1i8..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            if let Ok(p) = spawn_pos.checked_add((dx as i32, dy as i32)) {
                candidates.push(p);
            }
        }
    }
    // Row-major packed order (y*50+x); room name first for the (unrealistic) edge-adjacent spawn.
    candidates.sort_by_key(|p| (p.room_name().to_string(), p.y().u8(), p.x().u8()));
    candidates.into_iter().find(|&p| world.is_walkable(p))
}

/// The atomic spawn energy debit: ALL spawns first — the spawning spawn itself first, then the
/// rest closest-first — then extensions closest-first to the spawning spawn; ties break on
/// structure id (the engine's tie order is hash-order — engine-mechanics.md:257, :33).
fn debit_spawn_cost(world: &mut EconWorld, spawn_idx: usize, cost: u32) {
    let origin = world.spawns[spawn_idx].pos;
    let mut remaining = cost;

    let mut spawn_order: Vec<usize> = (0..world.spawns.len()).collect();
    spawn_order.sort_by_key(|&i| (origin.get_range_to(world.spawns[i].pos), world.spawns[i].id));
    for i in spawn_order {
        if remaining == 0 {
            return;
        }
        let take = world.spawns[i].store_energy.min(remaining);
        world.spawns[i].store_energy -= take;
        remaining -= take;
    }

    let mut ext_order: Vec<usize> = (0..world.extensions.len()).collect();
    ext_order.sort_by_key(|&i| (origin.get_range_to(world.extensions[i].pos), world.extensions[i].id));
    for i in ext_order {
        if remaining == 0 {
            return;
        }
        let take = world.extensions[i].store_energy.min(remaining);
        world.extensions[i].store_energy -= take;
        remaining -= take;
    }
    debug_assert_eq!(remaining, 0, "atomic spawn debit was pre-checked against room energy");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{SOURCE_CAPACITY_NEUTRAL, SOURCE_CAPACITY_OWNED};
    use screeps::{RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    /// Resolve one tick and assert the conservation audit passed — every mechanics test therefore
    /// doubles as a conservation test.
    fn step(world: &mut EconWorld, intents: &EconIntents) -> EconTickReport {
        let report = resolve_econ_tick(world, intents);
        assert!(report.conservation.is_empty(), "conservation violated: {:?}", report.conservation);
        report
    }

    fn steps(world: &mut EconWorld, n: u32) {
        for _ in 0..n {
            step(world, &EconIntents::new());
        }
    }

    /// HARVEST_POWER 2 e/WORK/t (engine-mechanics.md:457); neutral pool 1500 / owned 3000
    /// (engine-mechanics.md:466); the 300-tick regen timer starts at the FIRST harvest below
    /// capacity — a second harvest does not restart it — and the pool refills when
    /// `tick >= regen_at − 1` (engine-mechanics.md:445-446).
    #[test]
    fn harvest_power_and_regen_timer() {
        assert_eq!((SOURCE_CAPACITY_NEUTRAL, SOURCE_CAPACITY_OWNED), (1500, 3000));
        let mut w = EconWorld::default();
        let s = w.add_source(pos(10, 10), SOURCE_CAPACITY_NEUTRAL);
        let c = w.add_creep(pos(11, 10), &[Part::Work, Part::Work, Part::Work, Part::Carry, Part::Move], 100_000);

        // Tick 0: gain = 2 × 3 WORK = 6; the timer starts (regen_at = 0 + 300).
        let mut i = EconIntents::new();
        i.act(c, EconAction::Harvest { source_idx: s });
        let r = step(&mut w, &i);
        assert_eq!(r.ledger.harvested, 6);
        assert_eq!(w.sources[s].energy, 1494);
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 6);
        assert_eq!(w.sources[s].regen_at, Some(300), "timer starts at the FIRST harvest below cap");

        // Tick 1: harvesting again does NOT restart the timer.
        let mut i = EconIntents::new();
        i.act(c, EconAction::Harvest { source_idx: s });
        step(&mut w, &i);
        assert_eq!(w.sources[s].energy, 1488);
        assert_eq!(w.sources[s].regen_at, Some(300), "second harvest leaves the running timer alone");

        // Idle until the refill tick: refills during the tick where tick >= regen_at − 1 = 299.
        while w.tick() < 299 {
            steps(&mut w, 1);
            if w.tick() <= 299 {
                assert_eq!(w.sources[s].energy, 1488, "no refill before tick 299 (tick {})", w.tick());
            }
        }
        steps(&mut w, 1); // resolves tick 299
        assert_eq!(w.sources[s].energy, 1500, "refilled to capacity at tick 299 (= regen_at − 1)");
        assert_eq!(w.sources[s].regen_at, None, "timer cleared on refill");
    }

    /// Harvest gain beyond the creep's store capacity spills to the ground at the creep's tile
    /// (the engine's drop-overflow step) — and the ledger mints the FULL gain (conservation holds
    /// through the spill; the same-tick pile decay is booked).
    #[test]
    fn harvest_overflow_drops_to_ground() {
        let mut w = EconWorld::default();
        let s = w.add_source(pos(10, 10), SOURCE_CAPACITY_OWNED);
        // 5 WORK (gain 10/tick), ONE CARRY (cap 50): the 6th harvest overflows.
        let mut body = vec![Part::Work; 5];
        body.extend([Part::Carry, Part::Move]);
        let c = w.add_creep(pos(11, 10), &body, 100_000);
        for _ in 0..5 {
            let mut i = EconIntents::new();
            i.act(c, EconAction::Harvest { source_idx: s });
            step(&mut w, &i);
        }
        assert_eq!(w.creep_stores[&c].total(), 50, "store full");
        let mut i = EconIntents::new();
        i.act(c, EconAction::Harvest { source_idx: s });
        let r = step(&mut w, &i);
        assert_eq!(r.ledger.harvested, 10, "full gain minted");
        // Overflow 10 dropped at the creep's tile, minus the same-tick decay ceil(10/1000) = 1.
        assert_eq!(w.dropped.len(), 1);
        assert_eq!((w.dropped[0].pos, w.dropped[0].amount), (pos(11, 10), 9));
        assert_eq!(r.ledger.dropped_decay[&SimResource::Energy], 1);
    }

    /// The atomic spawn debit's draw order: ALL spawns first (the spawning spawn itself first),
    /// THEN extensions closest-first to the spawning spawn (engine-mechanics.md:257).
    #[test]
    fn spawn_atomic_debit_and_draw_order() {
        let mut w = EconWorld::default();
        let a = w.add_spawn(pos(25, 25));
        let b = w.add_spawn(pos(30, 25));
        w.spawns[a].store_energy = 100;
        w.spawns[b].store_energy = 100;
        let near = w.add_extension(pos(26, 26), 8); // range 1 from A
        let far = w.add_extension(pos(35, 25), 8); // range 10 from A
        let farther = w.add_extension(pos(36, 25), 8); // range 11 from A
        w.extensions[near].store_energy = 50;
        w.extensions[far].store_energy = 200;
        w.extensions[farther].store_energy = 150;
        // (Post-debit room energy stays ≥ 300 so the self-charge step doesn't muddy the arithmetic.)

        // Cost 260 = 100(work) + 100(work) + 50(move) + 10(tough) — engine-mechanics.md:452.
        let body = vec![Part::Work, Part::Work, Part::Move, Part::Tough];
        let mut i = EconIntents::new();
        i.spawn(a, body);
        let r = step(&mut w, &i);
        assert_eq!(r.spawns_started.len(), 1);
        assert_eq!(r.ledger.spawn_bodies, 260);
        assert_eq!(w.spawns[a].store_energy, 0, "the spawning spawn drains first");
        assert_eq!(w.spawns[b].store_energy, 0, "then the other spawn — ALL spawns before extensions");
        assert_eq!(w.extensions[near].store_energy, 0, "then the closest extension");
        assert_eq!(w.extensions[far].store_energy, 190, "the next-closest pays only the remainder (10)");
        assert_eq!(w.extensions[farther].store_energy, 150, "the farthest is never touched");
    }

    /// A spawn is busy exactly `CREEP_SPAWN_TIME × parts` = 3/part ticks (engine-mechanics.md:242,
    /// :454): a 3-part body started at tick 0 rejects new requests through tick 8 and its creep
    /// materializes (with a fresh CREEP_LIFE_TIME=1500 TTL — engine-mechanics.md:453) at tick 9,
    /// on the lowest-packed free adjacent tile.
    #[test]
    fn spawn_busy_3_ticks_per_part() {
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(25, 25));
        let e = w.add_extension(pos(26, 25), 8); // energy for the second body
        w.extensions[e].store_energy = 200;
        let body = vec![Part::Move, Part::Carry, Part::Move]; // 3 parts → 9 ticks, cost 150

        let mut i = EconIntents::new();
        i.spawn(s, body.clone());
        let r = step(&mut w, &i);
        assert_eq!(r.spawns_started.len(), 1, "tick 0: accepted");
        let firstborn = r.spawns_started[0].1;

        for t in 1..=8 {
            let mut i = EconIntents::new();
            i.spawn(s, body.clone());
            let r = step(&mut w, &i);
            assert_eq!(r.rejected_actions, 1, "tick {t}: spawn is busy");
            assert!(r.births.is_empty(), "tick {t}: nothing born yet");
        }

        // Tick 9 (= 3 × 3 parts): the creep materializes AND the spawn accepts a new request.
        let mut i = EconIntents::new();
        i.spawn(s, body.clone());
        let r = step(&mut w, &i);
        assert_eq!(r.births, vec![firstborn], "born exactly 3×parts ticks after the start");
        assert_eq!(r.spawns_started.len(), 1, "the freed spawn accepts a new request the same tick");
        let born = w.creep(firstborn).expect("materialized");
        assert_eq!(born.pos, pos(24, 24), "lowest packed (y,x) free neighbor of (25,25)");
        // creep_ttl stores the engine's ageTime = birth + 1500; death fires at ageTime − 1.
        assert_eq!(w.creep_ttl[&firstborn], 9 + CREEP_LIFE_TIME);
    }

    /// Every spawn self-charges +1/tick while room spawn+extension energy < 300, capped at its
    /// own 300 store (engine-mechanics.md:279) — a drained room always recovers spawn ability.
    #[test]
    fn spawn_self_charge_below_300() {
        let mut w = EconWorld::default();
        let a = w.add_spawn(pos(25, 25));
        let b = w.add_spawn(pos(30, 25));
        w.spawns[a].store_energy = 0;
        w.spawns[b].store_energy = 0;
        let r = step(&mut w, &EconIntents::new());
        assert_eq!(r.ledger.spawn_self_charge, 2, "both spawns charge while the room is < 300");
        assert_eq!((w.spawns[a].store_energy, w.spawns[b].store_energy), (1, 1));

        // At room total ≥ 300 the charge stops.
        w.spawns[a].store_energy = 299;
        w.spawns[b].store_energy = 1;
        let r = step(&mut w, &EconIntents::new());
        assert_eq!(r.ledger.spawn_self_charge, 0, "room total 300 → no charge");
        assert_eq!(w.spawns[a].store_energy, 299);
    }

    /// Extension capacity follows the controller level — 50 (≤6) / 100 (7) / 200 (8),
    /// engine-mechanics.md:456 — and the engine RECOMPUTES it from the CURRENT level every tick
    /// (`extensions/tick.js:11`): with a controller present, the pipeline re-caps every extension
    /// each tick; without one, the builder's capacities hold. A transfer clamps to that capacity.
    #[test]
    fn extension_capacity_by_rcl() {
        // Controller-less world: builder capacities hold, transfers clamp to them.
        let mut w = EconWorld::default();
        let e6 = w.add_extension(pos(10, 10), 6);
        let e7 = w.add_extension(pos(11, 10), 7);
        let e8 = w.add_extension(pos(12, 10), 8);
        assert_eq!(w.extensions[e6].capacity, 50);
        assert_eq!(w.extensions[e7].capacity, 100);
        assert_eq!(w.extensions[e8].capacity, 200);

        let c = w.add_creep(pos(11, 11), &[Part::Carry, Part::Carry, Part::Carry, Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 200);
        w.sync_carry_used(c);
        let mut i = EconIntents::new();
        i.act(c, EconAction::Transfer { target: StructRef::Extension(e6), resource: SimResource::Energy, amount: 200 });
        step(&mut w, &i);
        assert_eq!(w.extensions[e6].store_energy, 50, "clamped to the RCL-6 capacity");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 150);

        // With a controller, capacity is re-derived from the CURRENT level every tick — the
        // builder's rcl argument stops mattering, and a level change re-caps existing extensions.
        let mut w = EconWorld::default();
        let e = w.add_extension(pos(10, 10), 8); // built "at RCL 8"...
        w.controller = Some(crate::state::SimController { pos: pos(40, 40), level: 6, progress: 0, downgrade_ticks: 20_000 });
        step(&mut w, &EconIntents::new());
        assert_eq!(w.extensions[e].capacity, 50, "re-capped from the CURRENT level (6), not the build-time 8");
        w.controller.as_mut().unwrap().level = 7;
        step(&mut w, &EconIntents::new());
        assert_eq!(w.extensions[e].capacity, 100, "a level change re-caps on the next tick");
    }

    /// The spawn charge is atomic: if room-wide spawn+extension energy < cost the request fails
    /// WHOLE and no store is touched (engine-mechanics.md:257). Oversize bodies (> 50 parts —
    /// engine-mechanics.md:453) are rejected the same way.
    #[test]
    fn atomic_spawn_failure_leaves_stores_untouched() {
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(25, 25)); // full: 300
        let e = w.add_extension(pos(26, 25), 8);
        w.extensions[e].store_energy = 50;
        // (Room 350 ≥ 300, so the self-charge step stays quiet and the stores must be BIT-untouched.)

        let mut i = EconIntents::new();
        i.spawn(s, vec![Part::Work, Part::Work, Part::Work, Part::Work]); // cost 400 > 350 room-wide
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1);
        assert!(r.spawns_started.is_empty());
        assert_eq!(w.spawns[s].store_energy, 300, "nothing debited on failure");
        assert_eq!(w.extensions[e].store_energy, 50, "nothing debited on failure");
        assert!(w.spawns[s].spawning.is_none());

        // An oversize body (51 parts) is rejected even though the room could pay for it.
        w.spawns[s].store_energy = 300;
        w.extensions[e].store_energy = 200; // room 500
        let mut i = EconIntents::new();
        i.spawn(s, vec![Part::Move; 51]); // 51 × 50 = 2550, but rejected on size first
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "oversize body rejected (MAX_CREEP_SIZE 50)");
        assert_eq!(w.spawns[s].store_energy, 300);
    }

    /// TTL death (CREEP_LIFE_TIME clock — engine-mechanics.md:453) fires at the engine's
    /// `gameTime >= ageTime − 1` boundary (engine-mechanics.md:57): a ttl-5 creep registered at
    /// tick 0 lives ticks 0..3 and dies during tick 4, dropping its ENTIRE store to the ground at
    /// its position. Deviation (module docs): direct drop, no tombstone — the engine's tombstone
    /// spills to ground later (engine-mechanics.md:432); end state identical.
    #[test]
    fn ttl_death_drops_store() {
        let mut w = EconWorld::default();
        let c = w.add_creep(pos(20, 20), &[Part::Carry, Part::Carry, Part::Move], 5);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 40);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Ghodium, 10);
        w.sync_carry_used(c);

        steps(&mut w, 4); // ticks 0..3: alive
        assert!(w.creep(c).is_some(), "alive through tick 3");
        let r = step(&mut w, &EconIntents::new()); // tick 4 = ageTime − 1: the death tick
        assert_eq!(r.deaths, vec![c]);
        assert!(w.creep(c).is_none(), "gone from the movement state");
        assert!(!w.creep_stores.contains_key(&c) && !w.creep_ttl.contains_key(&c));
        // The whole store hit the ground at (20,20), then the same-tick pile decay took 1 each.
        let energy = w.dropped.iter().find(|p| p.resource == SimResource::Energy).unwrap();
        let ghodium = w.dropped.iter().find(|p| p.resource == SimResource::Ghodium).unwrap();
        assert_eq!((energy.pos, energy.amount), (pos(20, 20), 39));
        assert_eq!((ghodium.pos, ghodium.amount), (pos(20, 20), 9));
    }

    /// Dropped piles decay `ceil(amount/1000)` per tick (engine-mechanics.md:431); an exhausted
    /// pile is removed.
    #[test]
    fn dropped_decay_rate() {
        let mut w = EconWorld::default();
        w.drop_resource(pos(10, 10), SimResource::Energy, 2500);
        w.drop_resource(pos(11, 10), SimResource::Energy, 1001);
        w.drop_resource(pos(12, 10), SimResource::Energy, 1);
        let r = step(&mut w, &EconIntents::new());
        assert_eq!(w.dropped[0].amount, 2497, "ceil(2500/1000) = 3");
        assert_eq!(w.dropped[1].amount, 999, "ceil(1001/1000) = 2");
        assert_eq!(w.dropped.len(), 2, "the 1-unit pile decayed away and was compacted");
        assert_eq!(r.ledger.dropped_decay[&SimResource::Energy], 3 + 2 + 1);
    }

    /// Transfer/Withdraw/Pickup are adjacency-1 (Chebyshev) and atomic. A transfer OVER-ASK
    /// (amount > held) is rejected WHOLE (the engine's ERR_NOT_ENOUGH_RESOURCES); what moves
    /// clamps to the target's free capacity. Withdraw/pickup clamp to
    /// `min(requested, available, free)` (documented deviation — module docs step 2).
    /// Spawns/extensions take energy only.
    #[test]
    fn transfer_withdraw_pickup_adjacency_and_caps() {
        let mut w = EconWorld::default();
        // 2×CARRY hauler (cap 100) with 80 energy aboard.
        let c = w.add_creep(pos(10, 10), &[Part::Carry, Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 80);
        w.sync_carry_used(c);
        let ct = w.add_container(pos(11, 11), 100, 250_000); // adjacent (diag), 90/100 full
        w.containers[ct].store.add(SimResource::Energy, 90);
        w.set_storage(pos(10, 11), 1_000_000);
        w.storage.as_mut().unwrap().store.add(SimResource::Hydrogen, 30);
        let far = w.add_container(pos(13, 13), 2000, 250_000); // range 3: NOT adjacent
        w.drop_resource(pos(9, 9), SimResource::Energy, 500);

        // An over-ask (81 > the held 80) is rejected WHOLE — nothing moves, nothing partial.
        let mut i = EconIntents::new();
        i.act(c, EconAction::Transfer { target: StructRef::Container(ct), resource: SimResource::Energy, amount: 81 });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "transfer over-ask rejects whole (engine)");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 80, "nothing moved");
        assert_eq!(w.containers[ct].store.amount(SimResource::Energy), 90, "nothing received");

        // Transfer 50 (≤ held) → what moves clamps to the container's free 10.
        let mut i = EconIntents::new();
        i.act(c, EconAction::Transfer { target: StructRef::Container(ct), resource: SimResource::Energy, amount: 50 });
        step(&mut w, &i);
        assert_eq!(w.containers[ct].store.amount(SimResource::Energy), 100);
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 70);
        assert_eq!(w.creep(c).unwrap().carry_used, 70, "weight invariant maintained through the move");

        // Withdraw 50 hydrogen → clamped to the creep's free 30.
        let mut i = EconIntents::new();
        i.act(c, EconAction::Withdraw { target: StructRef::Storage, resource: SimResource::Hydrogen, amount: 50 });
        step(&mut w, &i);
        assert_eq!(w.creep_stores[&c].amount(SimResource::Hydrogen), 30);
        assert_eq!(w.storage.as_ref().unwrap().store.amount(SimResource::Hydrogen), 0);

        // Pickup with a FULL store moves nothing (min with free = 0); the pile only decays.
        let mut i = EconIntents::new();
        i.act(c, EconAction::Pickup { dropped_idx: 0 });
        step(&mut w, &i);
        assert_eq!(w.creep_stores[&c].total(), 100);
        assert_eq!(w.dropped[0].amount, 500 - 4, "untouched by the full creep (four ticks of decay)");

        // Out-of-range transfer is rejected; non-energy into a spawn is rejected.
        let s = w.add_spawn(pos(10, 9));
        let mut i = EconIntents::new();
        i.act(c, EconAction::Transfer { target: StructRef::Container(far), resource: SimResource::Energy, amount: 10 });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "range 3 target rejected");
        let mut i = EconIntents::new();
        i.act(c, EconAction::Transfer { target: StructRef::Spawn(s), resource: SimResource::Hydrogen, amount: 10 });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "spawns are energy-only stores");
    }

    /// The bot's Pipeline A/D intent model (`jobs/actions.rs:27-31`): ONE work intent (A: Harvest)
    /// and ONE transfer-class intent (D: Transfer/Withdraw/Pickup) per creep per tick — duplicates
    /// mask deterministically first-wins; an A and a D coexist in the same tick.
    #[test]
    fn pipeline_a_d_one_intent_each() {
        let mut w = EconWorld::default();
        let s = w.add_source(pos(10, 10), 3000);
        let ct = w.add_container(pos(12, 10), 2000, 250_000);
        w.containers[ct].store.add(SimResource::Energy, 100);
        let c = w.add_creep(pos(11, 10), &[Part::Work, Part::Carry, Part::Carry, Part::Move], 100_000);

        let mut i = EconIntents::new();
        i.act(c, EconAction::Harvest { source_idx: s }); // A — executes
        i.act(c, EconAction::Withdraw { target: StructRef::Container(ct), resource: SimResource::Energy, amount: 20 }); // D — executes
        i.act(c, EconAction::Harvest { source_idx: s }); // duplicate A — masked
        i.act(c, EconAction::Pickup { dropped_idx: 0 }); // duplicate D — masked
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 2, "one duplicate per pipeline masked");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 2 + 20, "harvest (1 WORK × 2) AND withdraw both landed");
        assert_eq!(w.sources[s].energy, 2998, "harvested exactly once");
        assert_eq!(w.creep(c).unwrap().carry_used, 22, "weight invariant after a two-pipeline tick");
    }

    /// Contending SpawnCreep requests resolve in (spawn index, submission order): under scarce
    /// room energy, which spawn wins is independent of emission order (spawn 0 outranks spawn 1);
    /// duplicate same-tick requests to ONE spawn keep first-submitted-wins (the documented
    /// within-actor contract, mirroring the per-creep pipeline mask).
    #[test]
    fn spawn_request_order_is_deterministic_under_contention() {
        // Cross-spawn contention: room holds 300 total; each request costs 250 ([W,W,M]); only
        // one can spawn. Both submission orders must produce the identical world.
        let run = |first_spawn: usize, second_spawn: usize| {
            let mut w = EconWorld::default();
            let a = w.add_spawn(pos(20, 20));
            let b = w.add_spawn(pos(30, 30));
            w.spawns[a].store_energy = 150;
            w.spawns[b].store_energy = 150;
            let body = vec![Part::Work, Part::Work, Part::Move]; // 250
            let mut i = EconIntents::new();
            i.spawn(first_spawn, body.clone());
            i.spawn(second_spawn, body.clone());
            let r = step(&mut w, &i);
            (w.state_digest(), r.spawns_started.clone(), r.rejected_actions)
        };
        let (d1, started1, rejected1) = run(0, 1);
        let (d2, started2, rejected2) = run(1, 0);
        assert_eq!(d1, d2, "cross-spawn contention is emission-order-independent");
        assert_eq!(started1, started2);
        assert_eq!(started1.len(), 1, "the room could afford exactly one");
        assert_eq!(started1[0].0, 0, "spawn 0 wins — the deterministic spawn-index order");
        assert_eq!((rejected1, rejected2), (1, 1), "the loser fails whole");

        // Same-spawn duplicates: first-submitted-wins, the second rejects on the now-busy spawn.
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(20, 20));
        let mut i = EconIntents::new();
        i.spawn(s, vec![Part::Move]);
        i.spawn(s, vec![Part::Carry]);
        let r = step(&mut w, &i);
        assert_eq!(r.spawns_started.len(), 1);
        assert_eq!(r.rejected_actions, 1, "the second request finds the spawn busy");
        assert_eq!(
            w.spawns[s].spawning.as_ref().unwrap().body,
            vec![Part::Move],
            "the FIRST-submitted body is the one spawning"
        );
    }

    /// `add_road` must not shadow the room's default terrain: registering a road never mints an
    /// empty per-room override, so default-terrain walls keep blocking (here: a sealed spawn
    /// still cannot birth after a road is added elsewhere in the room).
    #[test]
    fn add_road_preserves_default_terrain_walls() {
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(25, 25));
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx != 0 || dy != 0 {
                    w.movement.terrain.walls.insert(((25 + dx) as u8, (25 + dy) as u8));
                }
            }
        }
        w.add_road(pos(10, 10), 5000, 5000); // same room, elsewhere — must not clear the seal
        assert!(w.movement.terrain.roads.contains(&(10, 10)), "road registered in the DEFAULT terrain");
        assert!(w.movement.rooms.is_empty(), "no shadowing per-room override was minted");
        let mut i = EconIntents::new();
        i.spawn(s, vec![Part::Move]);
        step(&mut w, &i);
        steps(&mut w, 6);
        assert!(w.spawns[s].spawning.is_some(), "the sealed spawn still slips — walls intact");
    }

    /// Birth placement is deterministic (lowest packed (y,x) free tile) and a FULLY blocked exit
    /// slips the completion +1 tick until a tile frees (engine-mechanics.md:242's exit slip,
    /// coarsely modeled — module docs).
    #[test]
    fn spawn_birth_tile_determinism_and_blocked_exit_slip() {
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(25, 25));
        // Wall off the whole top row + (24,25): the lowest free packed tile becomes (26,25).
        for x in 24..=26 {
            w.movement.terrain.walls.insert((x, 25 - 1));
        }
        w.movement.terrain.walls.insert((24, 25));
        let mut i = EconIntents::new();
        i.spawn(s, vec![Part::Move]); // 1 part → done at tick 3
        let r = step(&mut w, &i);
        let newborn = r.spawns_started[0].1;
        steps(&mut w, 2); // ticks 1, 2
        let r = step(&mut w, &EconIntents::new()); // tick 3: birth
        assert_eq!(r.births, vec![newborn]);
        assert_eq!(w.creep(newborn).unwrap().pos, pos(26, 25), "skips walled tiles, lowest packed wins");

        // Fully seal a second spawn's neighborhood: the completion slips until a tile frees.
        let mut w = EconWorld::default();
        let s = w.add_spawn(pos(25, 25));
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx != 0 || dy != 0 {
                    w.movement.terrain.walls.insert(((25 + dx) as u8, (25 + dy) as u8));
                }
            }
        }
        let mut i = EconIntents::new();
        i.spawn(s, vec![Part::Move]);
        step(&mut w, &i);
        steps(&mut w, 5); // ticks 1..5 — would have been born at 3, slips instead
        assert!(w.spawns[s].spawning.is_some(), "still slipping: every exit tile is sealed");
        w.movement.terrain.walls.remove(&(26, 26));
        let r = step(&mut w, &EconIntents::new());
        assert_eq!(r.births.len(), 1, "born the first tick a tile is free");
        assert_eq!(w.creep(r.births[0]).unwrap().pos, pos(26, 26));
    }

    // ── M1: repair + decay + wearout (each test doubles as a conservation test via `step`) ──────

    /// Repair pricing (engine `creeps/repair.js`, engine-mechanics.md:118): 100 hits/WORK/tick,
    /// cost = ceil(hits/100) energy, clamped by carried energy and missing hits; range ≤ 3;
    /// zero-energy and full-target repairs are rejected. The ledger books the energy by class and
    /// the report mirrors it into `repair_leak` iff a refill deficit existed at tick start.
    #[test]
    fn repair_power_cost_and_clamps() {
        let mut w = EconWorld::default();
        let road = w.add_road(pos(10, 10), 1000, 5000);
        // 3 WORK + carry, holding 50 energy: power = 300 hits, energy clamp = 5000, missing = 4000.
        let body = [Part::Work, Part::Work, Part::Work, Part::Carry, Part::Move];
        let c = w.add_creep(pos(12, 12), &body, 100_000); // range 2 ≤ 3 (never steps — no wear)
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 50);
        w.sync_carry_used(c);
        // NO spawns/extensions in this world ⇒ zero refill capacity ⇒ no deficit ⇒ no leak.
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(road) });
        let r = step(&mut w, &i);
        assert_eq!(w.roads[road].hits, 1300, "300 hits repaired (3 WORK × 100)");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 47, "cost = ceil(300/100) = 3");
        assert_eq!(r.ledger.repair_roads, 3, "ledgered by class");
        assert_eq!(r.repair_leak.total(), 0, "no spawn/extension capacity ⇒ no deficit ⇒ no leak");
        assert_eq!(w.creep(c).unwrap().carry_used, 47, "weight invariant through the repair");

        // Energy clamp: 1 energy left repairs at most 100 hits (costing exactly that 1).
        w.creep_stores.get_mut(&c).unwrap().remove(SimResource::Energy, 46);
        w.sync_carry_used(c);
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(road) });
        step(&mut w, &i);
        assert_eq!(w.roads[road].hits, 1400, "clamped to energy × 100 = 100 hits");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 0);

        // Zero energy ⇒ rejected (engine repair.js:11 early-returns; the sim counts it).
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(road) });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "no energy aboard");
        assert_eq!(w.roads[road].hits, 1400);

        // Missing-hits clamp + cost ceil: 50 hits missing cost ceil(50/100) = 1 energy.
        let near_full = w.add_road(pos(11, 11), 4950, 5000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 10);
        w.sync_carry_used(c);
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(near_full) });
        step(&mut w, &i);
        assert_eq!(w.roads[near_full].hits, 5000, "clamped to missing hits");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 9, "ceil(50/100) = 1 energy");

        // Full target ⇒ rejected; out of range (4 > 3) ⇒ rejected.
        let far = w.add_road(pos(16, 12), 100, 5000); // range 4 from (12,12)
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(near_full) });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "full target rejected");
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(far) });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "range 4 > REPAIR_RANGE rejected");
    }

    /// Repair shares Pipeline A with Harvest (`jobs/actions.rs:27-31`): a repairing creep SKIPS
    /// that tick's harvest — the S1 leak mechanic, pinned at the resolver level.
    #[test]
    fn repair_masks_same_tick_harvest() {
        let mut w = EconWorld::default();
        let s = w.add_source(pos(10, 10), 3000);
        let road = w.add_road(pos(11, 11), 1000, 5000);
        let c = w.add_creep(pos(11, 10), &[Part::Work, Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 20);
        w.sync_carry_used(c);
        let mut i = EconIntents::new();
        i.act(c, EconAction::Repair { target: StructRef::Road(road) }); // first-submitted wins A
        i.act(c, EconAction::Harvest { source_idx: s }); // masked
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "the harvest was masked by the repair");
        assert_eq!(w.sources[s].energy, 3000, "no harvest happened — the repair ATE the work tick");
        assert_eq!(w.roads[road].hits, 1100, "1 WORK × 100 hits repaired");
        assert_eq!(r.ledger.repair_roads, 1);
    }

    /// The `repair_leak_e` mirror: identical repairs book into `repair_leak` iff ANY
    /// spawn/extension deficit existed at tick start (energy_stress.rs:132-135 semantics — ANY
    /// deficit, deliberately not the S1 gate's 10%/10k condition).
    #[test]
    fn repair_leak_requires_refill_deficit() {
        let run = |spawn_energy: u32| {
            let mut w = EconWorld::default();
            let sp = w.add_spawn(pos(20, 20));
            w.spawns[sp].store_energy = spawn_energy;
            let road = w.add_road(pos(10, 10), 1000, 5000);
            let ct = w.add_container(pos(11, 10), 2000, 100_000);
            let c = w.add_creep(pos(10, 11), &[Part::Work, Part::Carry, Part::Move], 100_000);
            w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 40);
            w.sync_carry_used(c);
            let mut i = EconIntents::new();
            i.act(c, EconAction::Repair { target: StructRef::Road(road) });
            let r1 = step(&mut w, &i);
            let mut i = EconIntents::new();
            i.act(c, EconAction::Repair { target: StructRef::Container(ct) });
            let r2 = step(&mut w, &i);
            (r1.repair_leak, r2.repair_leak, r1.ledger.repair_roads, r2.ledger.repair_containers)
        };
        // Full spawn (300/300): repairs happen, ledger books them, leak stays 0.
        let (leak_road, leak_container, road_e, container_e) = run(300);
        assert_eq!((road_e, container_e), (1, 1), "repairs run either way");
        assert_eq!(leak_road.total() + leak_container.total(), 0, "no deficit ⇒ no leak");
        // Deficient spawn (0/300 — stays deficient across both ticks despite the +1/tick
        // self-charge): the same repairs are leaks, by class.
        let (leak_road, leak_container, _, _) = run(0);
        assert_eq!(leak_road.roads, 1, "road repair energy leaked under deficit");
        assert_eq!(leak_container.containers, 1, "container repair energy leaked under deficit");
        assert_eq!(leak_road.containers + leak_container.roads, 0, "classes don't cross");
    }

    /// Road decay (engine-mechanics.md:430): −100 hits per 1000-tick window on plain (fires at
    /// `tick >= next_decay_at − 1`), ×5 on swamp; a road at 0 hits is REMOVED and its tile
    /// reverts to natural terrain (the movement-terrain road entry is de-registered).
    #[test]
    fn road_decay_plain_swamp_and_death() {
        let mut w = EconWorld::default();
        w.movement.terrain.swamps.insert((30, 30));
        let plain = w.add_road(pos(10, 10), 5000, 5000);
        let swamp = w.add_road(pos(30, 30), 25_000, 25_000);
        let _dying = w.add_road(pos(40, 40), 100, 5000); // exactly one event from death
        assert_eq!(w.roads[plain].next_decay_at, ROAD_DECAY_TIME, "a full window out at build");
        // Nothing decays before the boundary tick.
        steps(&mut w, ROAD_DECAY_TIME - 1); // ticks 0..=998
        assert_eq!(w.roads[plain].hits, 5000);
        // The boundary tick (tick 999 = next_decay_at − 1) fires the event.
        steps(&mut w, 1);
        assert_eq!(w.roads[plain].hits, 4900, "plain: −100");
        assert_eq!(w.roads[swamp].hits, 24_500, "swamp: −500 (×5 ratio)");
        assert_eq!(w.roads[plain].next_decay_at, 999 + ROAD_DECAY_TIME, "window re-arms from the event tick");
        // The 100-hit road died: removed from the world AND from the movement terrain.
        assert_eq!(w.roads.len(), 2, "the dead road was compacted away");
        assert!(!w.movement.terrain.roads.contains(&(40, 40)), "tile reverted to natural terrain");
        assert!(w.movement.terrain.roads.contains(&(10, 10)), "living roads keep their tiles");
    }

    /// Container decay (engine-mechanics.md:429): −5000 per 500-tick window at RCL ≥ 1, per
    /// 100-tick window with no (or level-0) controller; death drops the WHOLE store to ground.
    #[test]
    fn container_decay_windows_and_death_drop() {
        // Unowned world (no controller): the fast 100-tick window.
        let mut w = EconWorld::default();
        let ct = w.add_container(pos(10, 10), 2000, 250_000);
        assert_eq!(w.containers[ct].next_decay_at, 100, "unowned window = 100");
        steps(&mut w, 100); // the event fires at tick 99 (= next_decay_at − 1)
        assert_eq!(w.containers[ct].hits, 245_000, "−5000 at the boundary tick");
        assert_eq!(w.containers[ct].next_decay_at, 99 + 100, "unowned window re-arms at 100");

        // Owned world (controller level ≥ 1): the slow 500-tick window.
        let mut w = EconWorld::default();
        w.controller = Some(crate::state::SimController { pos: pos(40, 40), level: 3, progress: 0, downgrade_ticks: 20_000 });
        let ct = w.add_container(pos(10, 10), 2000, 250_000);
        assert_eq!(w.containers[ct].next_decay_at, 500, "owned window = 500");
        steps(&mut w, 499);
        assert_eq!(w.containers[ct].hits, 250_000, "no decay before the boundary");
        steps(&mut w, 1);
        assert_eq!(w.containers[ct].hits, 245_000);

        // Death: a 5000-hit container with cargo dies at its event and drops the store.
        let mut w = EconWorld::default();
        w.controller = Some(crate::state::SimController { pos: pos(40, 40), level: 3, progress: 0, downgrade_ticks: 20_000 });
        let ct = w.add_container(pos(15, 15), 2000, CONTAINER_DECAY);
        w.containers[ct].store.add(SimResource::Energy, 700);
        w.containers[ct].store.add(SimResource::Oxygen, 30);
        steps(&mut w, 500);
        assert!(w.containers.is_empty(), "dead container removed");
        let energy = w.dropped.iter().find(|p| p.resource == SimResource::Energy).unwrap();
        let oxygen = w.dropped.iter().find(|p| p.resource == SimResource::Oxygen).unwrap();
        // Dropped at the container's tile, minus the same-tick pile decay (ceil/1000 each).
        assert_eq!((energy.pos, energy.amount), (pos(15, 15), 699));
        assert_eq!((oxygen.pos, oxygen.amount), (pos(15, 15), 29));
    }

    /// ROAD_WEAROUT traffic wear (engine `movement.js:215-219`, engine-mechanics.md:430): a creep
    /// STEP onto a road tile pulls the road's `next_decay_at` FORWARD by 1 × body parts — the
    /// clock accelerates, hits are untouched. Standing still wears nothing.
    #[test]
    fn road_wearout_pulls_the_decay_clock() {
        let mut w = EconWorld::default();
        let road = w.add_road(pos(11, 10), 5000, 5000);
        let c = w.add_creep(pos(10, 10), &[Part::Move, Part::Move, Part::Carry], 100_000);
        let before = w.roads[road].next_decay_at;

        // Step ONTO the road: −3 (body parts).
        let mut i = EconIntents::new();
        i.moves.set_move(c, screeps::Direction::Right);
        step(&mut w, &i);
        assert_eq!(w.creep(c).unwrap().pos, pos(11, 10), "stepped onto the road");
        assert_eq!(w.roads[road].next_decay_at, before - 3, "clock pulled 1 × 3 parts");
        assert_eq!(w.roads[road].hits, 5000, "wear never damages hits directly");

        // Idle ON the road: no wear (wear is per STEP, not per occupancy).
        step(&mut w, &EconIntents::new());
        assert_eq!(w.roads[road].next_decay_at, before - 3);

        // Step OFF the road (onto plain): no wear either.
        let mut i = EconIntents::new();
        i.moves.set_move(c, screeps::Direction::Right);
        step(&mut w, &i);
        assert_eq!(w.roads[road].next_decay_at, before - 3);
    }

    /// Roads are storeless: a Transfer naming a road is rejected whole (target_takes gates it).
    #[test]
    fn transfer_to_road_is_rejected() {
        let mut w = EconWorld::default();
        let road = w.add_road(pos(11, 10), 1000, 5000);
        let c = w.add_creep(pos(10, 10), &[Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 50);
        w.sync_carry_used(c);
        let mut i = EconIntents::new();
        i.act(c, EconAction::Transfer { target: StructRef::Road(road), resource: SimResource::Energy, amount: 10 });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1);
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 50, "nothing moved");
    }

    // ── M2: controller + build (each test doubles as a conservation test via `step`) ────────────

    /// A worker world for the upgrade tests: controller at (40,40), a WORK-heavy upgrader in
    /// range 3 (at (38,40) — range 2) with `energy` aboard.
    fn upgrade_world(level: u8, progress: u32, clock: u32, work: usize, energy: u32) -> (EconWorld, CreepId) {
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), level);
        {
            let c = w.controller.as_mut().unwrap();
            c.progress = progress;
            c.downgrade_ticks = clock;
        }
        let mut body = vec![Part::Work; work];
        body.extend([Part::Carry, Part::Carry, Part::Move]);
        let id = w.add_creep(pos(38, 40), &body, 100_000);
        if energy > 0 {
            w.creep_stores.get_mut(&id).unwrap().add(SimResource::Energy, energy);
            w.sync_carry_used(id);
        }
        (w, id)
    }

    /// UPGRADE_CONTROLLER_POWER 1 e/WORK/t clamped by carried energy
    /// (`creeps/upgradeController.js:33-34`), 1 energy per progress (:92), Chebyshev range ≤ 3
    /// (:21-23), empty store rejected (:9), level-0 controller rejected (:24).
    #[test]
    fn upgrade_power_range_and_energy_clamp() {
        let (mut w, id) = upgrade_world(2, 0, 10_000, 3, 10);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(w.controller.as_ref().unwrap().progress, 3, "3 WORK × 1 progress");
        assert_eq!(w.creep_stores[&id].amount(SimResource::Energy), 7, "1 energy per progress");
        assert_eq!(r.ledger.upgrade, 3, "ledgered as the upgrade sink");
        assert_eq!(r.controller, Some((2, 3, 10_000)), "clock restored +100 net, capped at FULL(2)");

        // Energy clamp: 2 energy left with 3 WORK converts only 2.
        w.creep_stores.get_mut(&id).unwrap().remove(SimResource::Energy, 5);
        w.sync_carry_used(id);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        step(&mut w, &i);
        assert_eq!(w.controller.as_ref().unwrap().progress, 5, "clamped to the 2 energy aboard");
        assert_eq!(w.creep_stores[&id].amount(SimResource::Energy), 0);

        // Empty store: rejected (:9).
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "empty upgrader rejected");

        // Range 4 > 3: rejected (:21-23).
        let (mut w, id) = upgrade_world(2, 0, 10_000, 1, 10);
        w.creep_mut(id).unwrap().pos = pos(36, 40); // range 4
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "range 4 rejected");
        assert_eq!(w.controller.as_ref().unwrap().progress, 0);

        // Level-0 controller: rejected (:24).
        let (mut w, id) = upgrade_world(0, 0, 0, 1, 10);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "level-0 controller rejects upgrades");
    }

    /// The level-up (`upgradeController.js:67-78`): progress crosses CONTROLLER_LEVELS[level]
    /// with a NEAR-FULL clock → level += 1 carrying the remainder (:70), clock = half the NEW
    /// level's max (:72) + the same tick's +100 restore (`controllers/tick.js:39` runs after).
    #[test]
    fn level_up_carries_remainder_and_resets_clock_to_half_max() {
        // Level 1 → 2: threshold 200 (CONTROLLER_LEVELS[1]); progress 195 + 10 effect = 205.
        let (mut w, id) = upgrade_world(1, 195, 20_000, 10, 50);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        let c = w.controller.as_ref().unwrap();
        assert_eq!(c.level, 2, "leveled up");
        assert_eq!(c.progress, 5, "remainder carries over (:70)");
        assert_eq!(
            c.downgrade_ticks,
            10_000 / 2 + 100,
            "half the NEW level's max (:72) + the same-tick +100 restore (tick.js:39)"
        );
        assert_eq!(r.level_ups, vec![2]);
        assert_eq!(r.ledger.upgrade, 10, "full effect spent");
    }

    /// The near-full-clock LEVEL-UP GATE (`upgradeController.js:67-68`): with the clock more than
    /// CONTROLLER_DOWNGRADE_RESTORE below full, progress accumulates PAST the threshold and the
    /// level-up only fires once upgrade ticks have restored the clock to within 100 of full.
    #[test]
    fn level_up_blocked_until_clock_near_full() {
        // Level 1, clock 19_600 < 20_000 − 100: the gate blocks.
        let (mut w, id) = upgrade_world(1, 195, 19_600, 10, 500);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        {
            let c = w.controller.as_ref().unwrap();
            assert_eq!(c.level, 1, "gate held: clock too low (:67-68)");
            assert_eq!(c.progress, 205, "progress accumulates PAST the threshold (:80)");
        }
        assert!(r.level_ups.is_empty());
        // Each upgrade tick nets +100 clock; two more blocked ticks bring it to 19_900 =
        // FULL − 100 — the exact gate boundary (`R + RESTORE >= FULL`) — so the NEXT upgrade
        // levels up with the whole surplus carrying.
        for _ in 0..2 {
            let mut i = EconIntents::new();
            i.act(id, EconAction::UpgradeController);
            let r = step(&mut w, &i);
            assert!(r.level_ups.is_empty(), "still below the gate boundary");
        }
        assert_eq!(w.controller.as_ref().unwrap().downgrade_ticks, 19_900);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        let c = w.controller.as_ref().unwrap();
        assert_eq!(c.level, 2, "gate opens at clock = FULL − 100 exactly (:67-68 ≥)");
        assert_eq!(c.progress, 205 + 3 * 10 - 200, "the accumulated surplus carries");
        assert_eq!(r.level_ups, vec![2]);
    }

    /// The RCL8 cap (`upgradeController.js:42-52`): 15 energy/tick ROOM-WIDE via the per-tick
    /// accumulator shared across upgraders — the second upgrader converts only the remainder,
    /// the third is a full no-op (nothing spent); progress never moves at 8 (:59 guards).
    #[test]
    fn rcl8_cap_is_shared_across_upgraders() {
        let (mut w, a) = upgrade_world(8, 0, 200_000, 10, 100);
        let mut extra = vec![Part::Work; 10];
        extra.extend([Part::Carry, Part::Carry, Part::Move]);
        let b = w.add_creep(pos(39, 41), &extra, 100_000);
        w.creep_stores.get_mut(&b).unwrap().add(SimResource::Energy, 100);
        w.sync_carry_used(b);
        let c = w.add_creep(pos(41, 39), &extra, 100_000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 100);
        w.sync_carry_used(c);

        let mut i = EconIntents::new();
        i.act(a, EconAction::UpgradeController);
        i.act(b, EconAction::UpgradeController);
        i.act(c, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.ledger.upgrade, 15, "room-wide 15 e/t at RCL 8");
        assert_eq!(w.creep_stores[&a].amount(SimResource::Energy), 90, "first: full 10");
        assert_eq!(w.creep_stores[&b].amount(SimResource::Energy), 95, "second: the remaining 5");
        assert_eq!(w.creep_stores[&c].amount(SimResource::Energy), 100, "third: capped out, no spend (:48-50)");
        assert_eq!(r.rejected_actions, 1, "the capped-out intent is counted");
        assert_eq!(w.controller.as_ref().unwrap().progress, 0, "no progress at max level");
        assert_eq!(
            w.controller.as_ref().unwrap().downgrade_ticks,
            200_000,
            "the restore still applies (capped at FULL(8))"
        );
    }

    /// The downgrade clock (`controllers/tick.js`): −1/tick idle (:49 boundary at remaining ≤ 1);
    /// the expiry drops a level, refunds `round(0.9 × CONTROLLER_LEVELS[new])` progress (:66) and
    /// re-arms at +FULL[new]/2 (:65); an upgrade tick restores instead and skips the downgrade
    /// check (:38-43); level 0 zeroes progress and unowns the room (:52-62).
    #[test]
    fn downgrade_clock_expiry_and_progress_refund() {
        // Idle decrement.
        let (mut w, _id) = upgrade_world(3, 100, 5_000, 1, 0);
        step(&mut w, &EconIntents::new());
        assert_eq!(w.controller.as_ref().unwrap().downgrade_ticks, 4_999, "−1/tick idle");

        // Expiry: clock 3 → fires on the third tick (remaining ≤ 1 at the step).
        let (mut w, _id) = upgrade_world(3, 100, 3, 1, 0);
        step(&mut w, &EconIntents::new()); // 3 → 2
        step(&mut w, &EconIntents::new()); // 2 → 1
        let r = step(&mut w, &EconIntents::new()); // 1 ≤ 1: the downgrade tick
        {
            let c = w.controller.as_ref().unwrap();
            assert_eq!(c.level, 2, "level dropped");
            assert_eq!(c.progress, 100 + 45_000 / 10 * 9, "kept progress + 0.9 × CONTROLLER_LEVELS[2] (:66)");
            assert_eq!(c.downgrade_ticks, 1 + 10_000 / 2, "re-armed at remaining + FULL[2]/2 (:65)");
        }
        assert_eq!(r.downgrades, vec![2]);

        // An upgrade tick RESTORES instead (and skips the downgrade check entirely, :43).
        let (mut w, id) = upgrade_world(3, 0, 1, 2, 10);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert!(r.downgrades.is_empty(), "no downgrade on a tick with an upgrade (:38-43)");
        assert_eq!(w.controller.as_ref().unwrap().downgrade_ticks, 101, "restored +100");
        assert_eq!(w.controller.as_ref().unwrap().level, 3);

        // Level 1 → 0: ownership lost, progress zeroed; further upgrades reject.
        let (mut w, id) = upgrade_world(1, 150, 1, 1, 10);
        let r = step(&mut w, &EconIntents::new());
        {
            let c = w.controller.as_ref().unwrap();
            assert_eq!((c.level, c.progress), (0, 0), "level 0 zeroes progress (:52-62)");
        }
        assert_eq!(r.downgrades, vec![0]);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "a level-0 controller rejects upgrades");
    }

    /// Review B10 — the engine has NO zero-effect guard before the level-up check
    /// (`upgradeController.js:9` gates energy only; `:67` compares `progress + 0`): a 0-WORK
    /// creep CARRYING energy triggers the level-up in the surplus window (progress already past
    /// the threshold from the blocked-gate accumulation). No energy is spent (:92 −0), and the
    /// falsy `_upgraded` means NO clock restore — the freshly-halved clock decays that tick.
    #[test]
    fn zero_work_upgrade_still_fires_the_surplus_level_up() {
        // Level 1, progress 250 ≥ 200 (surplus), clock full: a [C,C,M] hauler with 10 energy.
        let (mut w, id) = upgrade_world(1, 250, 20_000, 0, 10);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        let c = w.controller.as_ref().unwrap();
        assert_eq!(c.level, 2, "the zero-effect intent fired the surplus level-up (:67)");
        assert_eq!(c.progress, 50, "remainder carries (250 + 0 − 200)");
        assert_eq!(r.level_ups, vec![2]);
        assert_eq!(w.creep_stores[&id].amount(SimResource::Energy), 10, "no energy spent (:92 −0)");
        assert_eq!(r.ledger.upgrade, 0, "nothing ledgered");
        assert_eq!(
            c.downgrade_ticks,
            10_000 / 2 - 1,
            "half-max from the level-up (:72) MINUS the tick's decay — the falsy accumulator skips the restore (tick.js:38)"
        );
        assert_eq!(r.rejected_actions, 0, "a level-up is not a no-op");

        // Without the surplus, the same zero-effect intent is a counted no-op (unchanged).
        let (mut w, id) = upgrade_world(1, 100, 20_000, 0, 10);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1);
        assert_eq!(w.controller.as_ref().unwrap().level, 1);
    }

    /// The restore caps at the full clock and requires an ACTUAL conversion: a 0-WORK "upgrader"
    /// converts nothing, so the `_upgraded` accumulator stays falsy and the clock still decays
    /// (`upgradeController.js:33` power 0 → effect 0; `controllers/tick.js:38` truthy gate).
    #[test]
    fn zero_work_upgrade_does_not_restore_the_clock() {
        let (mut w, id) = upgrade_world(2, 0, 5_000, 0, 10); // no WORK parts
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "zero-conversion intent counted");
        assert_eq!(w.controller.as_ref().unwrap().downgrade_ticks, 4_999, "clock still decays");

        // And the +100 restore caps at FULL: clock 19_950 + 100 → 20_000, not 20_050.
        let (mut w, id) = upgrade_world(1, 0, 19_950, 1, 10);
        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController);
        step(&mut w, &i);
        assert_eq!(w.controller.as_ref().unwrap().downgrade_ticks, 20_000, "capped at FULL[1] (:39-42)");
    }

    /// The parallel-refill idiom (Pipeline D + E, one tick): the engine processes withdraw BEFORE
    /// upgradeController (`creeps/intents.js:15-17` creepActions order), so a draining upgrader's
    /// same-tick withdraw lands first and the upgrade spends from the topped-up store — the live
    /// `controllerbehavior.rs:107-124` behavior the upgrader body math depends on.
    #[test]
    fn withdraw_and_upgrade_coexist_in_one_tick() {
        let (mut w, id) = upgrade_world(2, 0, 10_000, 3, 2); // 3 WORK, only 2 energy aboard
        let ct = w.add_container(pos(38, 41), 2000, 250_000); // adjacent to the creep at (38,40)
        w.containers[ct].store.add(SimResource::Energy, 500);

        let mut i = EconIntents::new();
        i.act(id, EconAction::UpgradeController); // Pipeline E
        i.act(id, EconAction::Withdraw { target: StructRef::Container(ct), resource: SimResource::Energy, amount: 90 }); // Pipeline D
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 0, "D and E coexist — no mask collision");
        assert_eq!(w.controller.as_ref().unwrap().progress, 3, "upgrade spent the FULL 3 (withdraw landed first)");
        assert_eq!(w.creep_stores[&id].amount(SimResource::Energy), 2 + 90 - 3);
        assert_eq!(w.containers[ct].store.amount(SimResource::Energy), 410);
    }

    /// BUILD_POWER 5 progress/WORK/t at 1 energy/progress clamped by remaining + carried energy
    /// (`creeps/build.js:67-69,83`); range ≤ 3 (:23); a Build masks the same-tick harvest
    /// (Pipeline A — `jobs/actions.rs:27-34`, engine conflict `intents.js:10`).
    #[test]
    fn build_power_cost_range_and_pipeline() {
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 2);
        let s = w.add_source(pos(9, 10), 3000);
        let site = w.add_construction_site(pos(12, 10), StructureKind::Road).unwrap();
        let b = w.add_creep(pos(10, 10), &[Part::Work, Part::Work, Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&b).unwrap().add(SimResource::Energy, 13);
        w.sync_carry_used(b);

        // Build (first-submitted) masks the harvest; effect = min(2×5, 300, 13) = 10.
        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: site });
        i.act(b, EconAction::Harvest { source_idx: s });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "the harvest was masked by the build (Pipeline A)");
        assert_eq!(w.sites[site].progress, 10, "2 WORK × 5 progress");
        assert_eq!(w.creep_stores[&b].amount(SimResource::Energy), 3, "1 energy per progress");
        assert_eq!(r.ledger.build, 10);
        assert_eq!(w.sources[s].energy, 3000, "no harvest happened");

        // Energy clamp: 3 energy left builds 3 progress.
        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: site });
        step(&mut w, &i);
        assert_eq!(w.sites[site].progress, 13, "clamped to carried energy");

        // Empty: rejected (build.js:14).
        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: site });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "no energy aboard");

        // Range 4: rejected (:23).
        w.creep_stores.get_mut(&b).unwrap().add(SimResource::Energy, 10);
        w.sync_carry_used(b);
        w.creep_mut(b).unwrap().pos = pos(16, 10); // range 4 from (12,10)
        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: site });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "range 4 > 3 rejected");
        assert_eq!(w.sites[site].progress, 13);
    }

    /// Completion materializes the structure with the engine's birth state (`build.js:108-293`):
    /// a road gets swamp-scaled hitsMax + the SAFE terrain registration; a SPAWN starts EMPTY
    /// (:123 — unlike the scenario builder's born-full convention); an extension caps from the
    /// CURRENT controller level; a container arms the FLAT 100-tick first window (:261); a tower
    /// stub blocks movement. Completed sites compact at the end of the work lane.
    #[test]
    fn build_completion_materializes_structures() {
        let heavy = {
            let mut b = vec![Part::Work; 10]; // 50 progress/t
            b.extend([Part::Carry; 10]);
            b.push(Part::Move);
            b
        };
        let fill = |w: &mut EconWorld, id: CreepId, amount: u32| {
            w.creep_stores.get_mut(&id).unwrap().add(SimResource::Energy, amount);
            w.sync_carry_used(id);
        };

        // Swamp road: placement cost ×5 (1500); completion: hits = hitsMax = 25_000, terrain
        // registered via the SAFE path, no shadowing room override.
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 7);
        w.movement.terrain.swamps.insert((12, 11));
        let road_site = w.add_construction_site(pos(12, 11), StructureKind::Road).unwrap();
        assert_eq!(w.sites[road_site].total, 1500, "swamp road costs 300 × 5 (create-construction-site.js:37-41)");
        let b = w.add_creep(pos(10, 10), &heavy, 100_000);
        while !w.sites.is_empty() {
            if w.creep_stores[&b].amount(SimResource::Energy) == 0 {
                fill(&mut w, b, 500);
            }
            let mut i = EconIntents::new();
            i.act(b, EconAction::Build { site_idx: 0 });
            step(&mut w, &i);
        }
        assert_eq!(w.roads.len(), 1, "the road materialized");
        assert_eq!((w.roads[0].hits, w.roads[0].hits_max), (25_000, 25_000), "swamp hitsMax ×5 (build.js:171-186)");
        assert_eq!(w.roads[0].next_decay_at, (w.tick() - 1) + ROAD_DECAY_TIME, "decay a full window out");
        assert!(w.movement.terrain.roads.contains(&(12, 11)), "terrain effect registered (SAFE path)");
        assert!(w.movement.rooms.is_empty(), "no empty room override minted");

        // Spawn / extension / container / tower birth state, one at a time.
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 7); // RCL 7: extension capacity 100
        let b = w.add_creep(pos(10, 10), &heavy, 100_000);
        for (kind, p) in [
            (StructureKind::Spawn, pos(11, 10)),
            (StructureKind::Extension, pos(12, 10)),
            (StructureKind::Container, pos(10, 11)),
            (StructureKind::Tower, pos(11, 11)),
        ] {
            let idx = w.add_construction_site(p, kind).unwrap();
            w.sites[idx].progress = w.sites[idx].total - 1; // one build from done
            fill(&mut w, b, 1);
            let mut i = EconIntents::new();
            i.act(b, EconAction::Build { site_idx: 0 });
            let r = step(&mut w, &i);
            assert_eq!(r.sites_completed, vec![(kind, p)], "{kind:?} completed");
            assert!(w.sites.is_empty(), "completed site compacted");
        }
        // The spawn was born EMPTY (build.js:123 — no 300 birth grant) and then self-charged
        // +1/tick for the 4 ticks since (the room is < 300 — engine-mechanics.md:279; the
        // ledger's conservation audit books each unit).
        assert_eq!(w.spawns[0].store_energy, 4, "born empty + 4 ticks of self-charge, NOT born-full");
        assert_eq!(w.extensions[0].capacity, 100, "extension caps from the CURRENT level (RCL 7)");
        assert_eq!(w.extensions[0].store_energy, 0);
        assert_eq!(w.containers[0].hits, crate::constants::CONTAINER_HITS);
        assert_eq!(
            w.containers[0].next_decay_at,
            (w.tick() - 2) + CONTAINER_DECAY_TIME,
            "built container arms the FLAT 100 window (build.js:261), owned room or not"
        );
        assert_eq!(w.towers.len(), 1, "tower stub materialized");
        assert!(!w.is_walkable(pos(11, 11)), "the tower blocks movement");
    }

    /// An obstacle-kind site cannot complete under a creep or obstacle object (`build.js:50-60`)
    /// — the build intent is REJECTED whole (no progress, no energy); a walkable-kind site (road)
    /// builds under a creep fine.
    #[test]
    fn obstacle_site_blocked_by_tile_occupant() {
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        let ext = w.add_construction_site(pos(11, 10), StructureKind::Extension).unwrap();
        let road = w.add_construction_site(pos(12, 10), StructureKind::Road).unwrap();
        let b = w.add_creep(pos(10, 10), &[Part::Work, Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&b).unwrap().add(SimResource::Energy, 50);
        w.sync_carry_used(b);
        let squatter = w.add_creep(pos(11, 10), &[Part::Move], 100_000); // ON the extension site

        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: ext });
        let r = step(&mut w, &i);
        assert_eq!(r.rejected_actions, 1, "obstacle site under a creep rejects the build");
        assert_eq!(w.sites[ext].progress, 0);
        assert_eq!(w.creep_stores[&b].amount(SimResource::Energy), 50, "nothing spent");

        // The road site under the squatter builds fine (roads are walkable).
        w.creep_mut(squatter).unwrap().pos = pos(12, 10);
        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: road });
        step(&mut w, &i);
        assert_eq!(w.sites[road].progress, 5, "walkable-kind site builds under a creep");

        // Squatter gone: the extension builds.
        w.creep_mut(squatter).unwrap().pos = pos(20, 20);
        let mut i = EconIntents::new();
        i.act(b, EconAction::Build { site_idx: ext });
        step(&mut w, &i);
        assert_eq!(w.sites[ext].progress, 5);
    }

    /// Site placement (`utils.js:128-190` + `:338-354`): the RCL extension allowance counts
    /// BUILT + PENDING (5 at RCL 2 — the {2:5, 3:10, …, 8:60} ladder); one site per tile; no
    /// obstacle-kind on an occupied tile (roads exempt); interior bounds; storage gated to
    /// RCL ≥ 4; natural walls reject every kind (road tunnels not modeled — documented deviation).
    #[test]
    fn site_placement_enforces_engine_rules() {
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 2);
        // 5 extension sites at RCL 2 — the 6th rejects on the allowance.
        for i in 0..5 {
            w.add_construction_site(pos(10 + i, 10), StructureKind::Extension).unwrap();
        }
        assert_eq!(
            w.add_construction_site(pos(20, 10), StructureKind::Extension),
            Err(crate::state::SitePlacementError::RclAllowance),
            "extension allowance at RCL 2 is 5 (constants.js:216)"
        );
        // A BUILT extension counts too: at RCL 3 (allowance 10), 5 sites + 5 built = full.
        w.controller.as_mut().unwrap().level = 3;
        for i in 0..5 {
            w.add_extension(pos(10 + i, 12), 3);
        }
        assert_eq!(
            w.add_construction_site(pos(20, 10), StructureKind::Extension),
            Err(crate::state::SitePlacementError::RclAllowance),
            "built + pending both count (checkControllerAvailability, utils.js:350)"
        );

        // One site per tile; obstacle collision; the road exemption.
        assert_eq!(
            w.add_construction_site(pos(10, 10), StructureKind::Road),
            Err(crate::state::SitePlacementError::SiteOccupied),
            "one site per tile (utils.js:174-176)"
        );
        assert_eq!(
            w.add_construction_site(pos(10, 12), StructureKind::Container),
            Err(crate::state::SitePlacementError::StructureCollision),
            "a non-road kind cannot share an extension's tile (utils.js:181-185)"
        );
        assert!(
            w.add_construction_site(pos(10, 12), StructureKind::Road).is_ok(),
            "a ROAD may share an obstacle structure's tile (utils.js:181 exempts roads)"
        );
        assert_eq!(
            w.add_construction_site(pos(11, 12), StructureKind::Extension),
            Err(crate::state::SitePlacementError::SameKindStructure),
            "a same-kind structure blocks placement (utils.js:171-173)"
        );

        // Review B1 — a CONTAINER blocks non-road placement (utils.js:181-185: containers have a
        // CONSTRUCTION_COST; walkability is irrelevant to the placement collision set).
        w.add_container(pos(15, 15), 2000, 250_000);
        assert_eq!(
            w.add_construction_site(pos(15, 15), StructureKind::Extension),
            Err(crate::state::SitePlacementError::StructureCollision),
            "a container tile rejects a non-road site (B1)"
        );
        assert!(
            w.add_construction_site(pos(15, 15), StructureKind::Road).is_ok(),
            "…while a road still shares the container's tile"
        );

        // Bounds + walls + storage RCL gate.
        assert_eq!(
            w.add_construction_site(pos(0, 10), StructureKind::Road),
            Err(crate::state::SitePlacementError::OutOfBounds),
            "x 0 < 1 (create-construction-site.js:11)"
        );
        w.movement.terrain.walls.insert((30, 30));
        assert_eq!(
            w.add_construction_site(pos(30, 30), StructureKind::Extension),
            Err(crate::state::SitePlacementError::TerrainWall)
        );
        assert_eq!(
            w.add_construction_site(pos(30, 30), StructureKind::Road),
            Err(crate::state::SitePlacementError::TerrainWall),
            "road-on-wall = tunnel, deliberately unmodeled (deviation)"
        );
        assert_eq!(
            w.add_construction_site(pos(31, 30), StructureKind::Storage),
            Err(crate::state::SitePlacementError::RclAllowance),
            "storage needs RCL 4 (CONTROLLER_STRUCTURES.storage)"
        );
        w.controller.as_mut().unwrap().level = 4;
        assert!(w.add_construction_site(pos(31, 30), StructureKind::Storage).is_ok());
    }

    /// Review A2 — the exit-adjacency border rule (`utils.js:130-145`): a non-road/non-container
    /// kind at x/y ∈ {1, 48} places only if ALL THREE adjacent border tiles are natural walls;
    /// roads and containers are exempt.
    #[test]
    fn site_placement_exit_adjacency_border_rule() {
        // Open exits (default terrain: no walls): the near-edge extension is rejected.
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        assert_eq!(
            w.add_construction_site(pos(1, 25), StructureKind::Extension),
            Err(crate::state::SitePlacementError::ExitAdjacent),
            "x = 1 with an open exit column rejects (utils.js:130-145)"
        );
        assert_eq!(
            w.add_construction_site(pos(25, 48), StructureKind::Tower),
            Err(crate::state::SitePlacementError::ExitAdjacent),
            "y = 48 with an open exit row rejects"
        );
        // Roads and containers are exempt (the :131 kind filter).
        assert!(w.add_construction_site(pos(1, 25), StructureKind::Road).is_ok(), "road exempt");
        assert!(w.add_construction_site(pos(1, 26), StructureKind::Container).is_ok(), "container exempt");

        // All three adjacent border tiles walled: the extension places.
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        for by in 24..=26u8 {
            w.movement.terrain.walls.insert((0, by));
        }
        assert!(
            w.add_construction_site(pos(1, 25), StructureKind::Extension).is_ok(),
            "walled border column admits the near-edge extension"
        );
        // …but a single open border tile among the three still rejects.
        let mut w = EconWorld::default();
        w.set_controller(pos(40, 40), 3);
        w.movement.terrain.walls.insert((0, 24));
        w.movement.terrain.walls.insert((0, 26)); // (0,25) stays open
        assert_eq!(
            w.add_construction_site(pos(1, 25), StructureKind::Extension),
            Err(crate::state::SitePlacementError::ExitAdjacent),
            "one open border tile of the three rejects"
        );
    }

    /// Review B2 — the controller is an OBSTACLE OBJECT (`common/constants.js:85`): its tile
    /// blocks creep birth (is_walkable) and blocks an obstacle-kind site from COMPLETING
    /// (`build.js:50-60`), but does NOT block site placement (no CONSTRUCTION_COST —
    /// `utils.js:181-185`).
    #[test]
    fn controller_tile_is_an_obstacle_object() {
        let mut w = EconWorld::default();
        w.set_controller(pos(26, 25), 3);
        assert!(!w.is_walkable(pos(26, 25)), "controller tile blocks birth/standing");
        assert!(w.obstacle_object_at(pos(26, 25)));
        assert!(
            !w.construction_blocking_structure_at(pos(26, 25)),
            "…but it is NOT in the placement collision set (no CONSTRUCTION_COST)"
        );
    }

    /// Spawned-then-living creeps keep the movement weight invariant: `carry_used` equals the
    /// store total for EVERY creep after a mixed tick (the resolver-level companion to
    /// `state::tests::carry_used_equals_store_total`).
    #[test]
    fn carry_used_matches_store_total_after_resolution() {
        let mut w = EconWorld::default();
        let s = w.add_source(pos(10, 10), 3000);
        let c1 = w.add_creep(pos(11, 10), &[Part::Work, Part::Carry, Part::Move], 100_000);
        let c2 = w.add_creep(pos(11, 11), &[Part::Carry, Part::Carry, Part::Move], 100_000);
        w.creep_stores.get_mut(&c2).unwrap().add(SimResource::Energy, 60);
        w.sync_carry_used(c2);
        let ct = w.add_container(pos(12, 11), 2000, 250_000);

        let mut i = EconIntents::new();
        i.act(c1, EconAction::Harvest { source_idx: s });
        i.act(c2, EconAction::Transfer { target: StructRef::Container(ct), resource: SimResource::Energy, amount: 25 });
        step(&mut w, &i);
        for creep in &w.movement.creeps {
            assert_eq!(
                creep.carry_used,
                w.creep_stores[&creep.id].total(),
                "creep {} carry weight == store total",
                creep.id
            );
        }
    }
}
