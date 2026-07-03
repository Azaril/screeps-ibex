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
//! 1. **Mask** — one Pipeline-A work intent (Harvest) + one Pipeline-D transfer-class intent
//!    (Transfer/Withdraw/Pickup) per creep per tick, the bot's own Pipeline A/D model
//!    (`jobs/actions.rs:27-31`). *Deviation:* the engine resolves per-intent-name with a conflict
//!    matrix (engine-mechanics.md:59-76, none of these verbs conflict there); the bot never emits
//!    more than one per pipeline, so the mask models the DECISION layer's contract, not the
//!    engine's. Masking is deterministic: actions are stable-sorted by creep id; within a creep,
//!    first submission wins and duplicates are counted in the report.
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
//! 3. **Harvest** (creep-id order), then **source regen** — gain `min(2×WORK, source.energy)`
//!    (engine-mechanics.md:457), store overflow drops to the creep's tile; the 300-tick timer
//!    starts at the first harvest below capacity and the pool refills when
//!    `tick >= regen_at − 1` (engine-mechanics.md:445-446, :466). Regen runs after harvest, as the
//!    engine's source tick runs after the intent stage (engine-mechanics.md §1.2).
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
//! 7. **Dropped decay** — every pile loses `ceil(amount/1000)` (engine-mechanics.md:431),
//!    including piles created this same tick (deviation: engine object-tick timing makes
//!    same-tick decay of a fresh drop unobservable; ours is one tick earlier, exactly booked).
//! 8. **Movement** — `resolve_movement` over the embedded `MovementState` (tick advances here).
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
    body_cost, CREEP_LIFE_TIME, CREEP_SPAWN_TIME, DROPPED_DECAY_DIVISOR, ENERGY_REGEN_TIME,
    MAX_CREEP_SIZE, SPAWN_ENERGY_CAPACITY,
};
use crate::intents::{EconAction, EconIntents, StructRef};
use crate::ledger::{audit_conservation, ConservationViolation, TickLedger};
use crate::state::{creep_store_capacity, EconWorld, PendingCreep, SimResource, SimStore};
use screeps::{Part, Position};
use screeps_sim_core::{resolve_movement, CreepId, MovementReport, SimBody, SimCreep, Simulation};
use std::collections::{BTreeMap, BTreeSet};

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
    /// illegal body / unknown creep) or by the Pipeline-A/D mask.
    pub rejected_actions: u32,
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

    // ── 1. Mask: stable-sort by creep id; one Pipeline-A + one Pipeline-D action per creep. ────
    let mut order: Vec<usize> = (0..intents.actions.len()).collect();
    order.sort_by_key(|&i| intents.actions[i].0); // stable → (creep id, submission order)

    let mut pipeline_used: BTreeMap<CreepId, (bool, bool)> = BTreeMap::new(); // (A, D)
    let mut harvests: Vec<(CreepId, usize)> = Vec::new();
    let mut transfer_class: Vec<(CreepId, &EconAction)> = Vec::new();
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
            EconAction::Harvest { source_idx } => {
                let used = pipeline_used.entry(*creep_id).or_insert((false, false));
                if used.0 {
                    report.rejected_actions += 1; // second Pipeline-A action this tick
                } else {
                    used.0 = true;
                    harvests.push((*creep_id, *source_idx));
                }
            }
            EconAction::Transfer { .. } | EconAction::Withdraw { .. } | EconAction::Pickup { .. } => {
                let used = pipeline_used.entry(*creep_id).or_insert((false, false));
                if used.1 {
                    report.rejected_actions += 1; // second Pipeline-D action this tick
                } else {
                    used.1 = true;
                    transfer_class.push((*creep_id, action));
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
            EconAction::Harvest { .. } | EconAction::SpawnCreep { .. } => unreachable!(),
        }
    }

    // ── 3. Harvest (creep-id order), then source regen. ─────────────────────────────────────────
    for (creep_id, source_idx) in harvests {
        let Some((creep_pos, work_power)) = world
            .creep(creep_id)
            .filter(|c| c.is_alive())
            .map(|c| (c.pos, c.body.effective_power(Part::Work, crate::constants::HARVEST_POWER)))
        else {
            report.rejected_actions += 1;
            continue;
        };
        let Some(source) = world.sources.get_mut(source_idx) else {
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
    report.movement = resolve_movement(&mut world.movement, &moves);

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
    }
}

/// Whether the target's store can hold `resource` at all (spawns/extensions are energy-only).
fn target_takes(target: StructRef, resource: SimResource) -> bool {
    match target {
        StructRef::Spawn(_) | StructRef::Extension(_) => resource == SimResource::Energy,
        StructRef::Container(_) | StructRef::Storage => true,
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
    }
}

fn target_available(world: &EconWorld, target: StructRef, resource: SimResource) -> u32 {
    match target {
        StructRef::Spawn(i) => world.spawns[i].store_energy,
        StructRef::Extension(i) => world.extensions[i].store_energy,
        StructRef::Container(i) => world.containers[i].store.amount(resource),
        StructRef::Storage => world.storage.as_ref().map(|s| s.store.amount(resource)).unwrap_or(0),
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
        w.controller = Some(crate::state::SimController { level: 6, progress: 0, downgrade_ticks: 20_000 });
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
