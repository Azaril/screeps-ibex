//! **The M6 mineral-economy eval** (ADR 0040 §D7 / migration M6): a focused, self-contained
//! harness over the [`screeps_econ_engine`] M6 mechanics (extractor + labs + boosts). It exists to
//! produce the M6 metrics the ADR names:
//!
//! - **Compound time-to-X** ([`compound_time_to`]): ticks for a realized RCL6 room (extractor +
//!   the reaction lab cluster) to brew a target quantity of the WORK-upgrade boost compound
//!   `XGH2O` from raw minerals — the mineral pipeline's headline throughput number.
//! - **Boost e/t-equivalent diagnostic** ([`boost_e_t_equivalent`]): the extra controller
//!   progress-per-tick a boosted upgrader buys, expressed as an energy-equivalent rate — **the
//!   input the ADR 0033 §D5.4 military-`w` arm consumes** (its consumption is combat's; M6 reports
//!   the number).
//! - **The boosted-upgrader T_RCL(6+) probe** ([`boosted_upgrader_probe`]): ONE G6-probe pair
//!   (boosted vs unboosted) measuring the T_RCL delta a T3-WORK upgrader buys — the tick cost is
//!   the M2 N=6 exclusion reason, so the probe reaches a FIXED progress target (not a full level)
//!   to stay affordable, and reports the measured cost.
//!
//! Unlike the Family-C/G/S/D runner (the full transcribed baseline FSM), this harness drives the
//! mineral/lab/boost mechanics directly with state-derived scripts (the engine-test idiom) —
//! M6's scope is the mechanics + their metrics, not a re-plumbing of the civilian policy FSM.
//! Determinism: the seeded mineral re-roll + the deterministic scripts keep every number
//! reproducible (the `mineral_family_is_deterministic` fence pins spread 0).

use crate::layout::{mineral_reroll_seed, mineral_type_and_density, realize, LayoutInfo, RealizeParams};
use screeps::{Part, Position};
use screeps_econ_engine::{
    resolve_econ_tick, EconAction, EconIntents, EconWorld, SimResource,
};
use screeps_rover_eval::base_traffic::{captured_layouts, CapturedLayout};
use screeps_sim_core::SimTerrain;

/// The M6 lab cluster layout the harness stamps into a realized world: three reaction labs (two
/// inputs + one output within range 2) that brew the G-compound chain, and one boost lab holding
/// the finished compound. Positions are chosen adjacent to the controller so the boost/upgrade
/// dance is short. Stamped by [`realize_mineral_economy`].
#[derive(Clone, Copy, Debug)]
pub struct LabCluster {
    /// The two input labs + the output lab (reaction cluster) and the boost lab, by world index.
    pub in_a: usize,
    pub in_b: usize,
    pub out: usize,
    pub boost: usize,
    /// The extractor's mineral index (0 — the single deposit).
    pub mineral: usize,
}

/// One mineral-economy scenario: a captured RCL6 layout realized with an extractor on its mineral
/// + the reaction/boost lab cluster.
#[derive(Clone, Debug)]
pub struct MineralScenario {
    pub name: String,
    pub layout_room: String,
    pub seed: u32,
    pub tick_cap: u32,
}

/// Default mineral-family tick cap (compound brewing + a bounded upgrade window fit well inside).
pub const DEFAULT_MINERAL_TICK_CAP: u32 = 40_000;

impl MineralScenario {
    pub fn new(room: &str, seed: u32) -> Self {
        MineralScenario {
            name: format!("MIN-{room}#s{seed}"),
            layout_room: room.to_string(),
            seed,
            tick_cap: DEFAULT_MINERAL_TICK_CAP,
        }
    }

    /// Instantiate: realize the layout at RCL 6 (extractor + labs REALIZED), returning the world,
    /// terrain, layout facts, and the lab-cluster handle.
    pub fn instantiate(&self) -> (EconWorld, SimTerrain, LayoutInfo, LabCluster) {
        let layouts = captured_layouts();
        let layout: &CapturedLayout = layouts
            .iter()
            .find(|l| l.room == self.layout_room)
            .unwrap_or_else(|| panic!("mineral layout `{}` not in the captured cache", self.layout_room));
        let realized = realize(layout, &RealizeParams { rcl: 6, road_health_pct: 100, seed: self.seed });
        let (world, terrain, info, cluster) = realize_mineral_economy(realized.world, realized.terrain, realized.info, layout, self.seed);
        (world, terrain, info, cluster)
    }
}

/// The mineral-economy corpus: captured rooms that HAVE a mineral, realized at RCL 6 (extractor +
/// labs). `full` widens the room spread; `fast` takes the first pair.
pub fn mineral_catalog(seed: u32, full: bool) -> Vec<MineralScenario> {
    let rooms: Vec<String> = captured_layouts().iter().filter(|l| l.mineral.is_some()).map(|l| l.room.clone()).collect();
    let take = if full { rooms.len().min(6) } else { 2 };
    rooms.into_iter().take(take).map(|r| MineralScenario::new(&r, seed)).collect()
}

/// Augment a realized world with the M6 mineral economy: an EXTRACTOR on the mineral tile + a
/// 4-lab cluster (two input labs, one output lab within range 2, one boost lab) stamped on free
/// tiles near the controller. The captured plans have no in-vocabulary lab structures (labs are
/// out-of-vocab furniture, `layout.rs`), so the cluster is placed deterministically here. Returns
/// the augmented world + the cluster handle.
pub fn realize_mineral_economy(
    mut world: EconWorld,
    terrain: SimTerrain,
    info: LayoutInfo,
    layout: &CapturedLayout,
    seed: u32,
) -> (EconWorld, SimTerrain, LayoutInfo, LabCluster) {
    // Ensure the mineral is a well-typed MODERATE pool with the extractor on it.
    assert!(!world.minerals.is_empty(), "{}: the mineral-economy family needs a mineral", layout.room);
    let (res, density) = mineral_type_and_density(&layout.room);
    world.minerals[0].resource = res;
    world.minerals[0].density = density;
    world.minerals[0].amount = screeps_econ_engine::constants::mineral_density_amount(density);
    world.minerals[0].reroll_seed = mineral_reroll_seed(&layout.room, seed);
    let mineral_pos = world.minerals[0].pos;
    world.add_extractor(mineral_pos);

    // Stamp a 4-lab cluster near the controller on free walkable tiles (deterministic scan). The
    // three reaction labs MUST be mutually range ≤ 2 (the runReaction geometry constraint), so the
    // placement seeks a tight 4-tile block whose first three satisfy that — not just any 4 free
    // tiles (which can straddle the controller and break the reaction).
    let anchor = info.controller_pos;
    // `center` is range ≤ 2 of every tile in `ring` (the output lab goes on it, guaranteeing the
    // reaction geometry); the inputs + boost lab take the surrounding tiles.
    let (center, mut ring) = free_cluster_tiles(&world, anchor, 3);
    let out = world.add_lab(center, 0);
    let in_a = world.add_lab(ring.remove(0), 0);
    let in_b = world.add_lab(ring.remove(0), 0);
    let boost = world.add_lab(ring.remove(0), 0);
    // Labs block movement — register them as pathing walls so the mover/oracle price the same world.
    let mut terrain = terrain;
    for l in [in_a, in_b, out, boost] {
        let p = world.labs[l].pos;
        terrain.walls.insert((p.x().u8(), p.y().u8()));
        world.movement.terrain.walls.insert((p.x().u8(), p.y().u8()));
    }
    let cluster = LabCluster { in_a, in_b, out, boost, mineral: 0 };
    (world, terrain, info, cluster)
}

/// Find a free CENTER tile near `anchor` plus `n` free surrounding tiles ALL within Chebyshev 2 of
/// the center (the reaction geometry: the output lab on the center is range ≤ 2 of every input on
/// the ring). Scans candidate centers outward from `anchor`; returns `(center, ring[0..n])`
/// row-major. This guarantees `reaction_product`'s "output range 2 of both inputs" for any layout.
fn free_cluster_tiles(world: &EconWorld, anchor: Position, n: usize) -> (Position, Vec<Position>) {
    for radius in 0..=6i32 {
        for center in ring_tiles(anchor, radius) {
            if !world.is_walkable(center) {
                continue;
            }
            // The center's free 5×5 neighborhood, EXCLUDING the center itself — every such tile is
            // within Chebyshev 2 of the center.
            let mut ring: Vec<Position> = Vec::new();
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    if let Ok(p) = center.checked_add((dx, dy)) {
                        if world.is_walkable(p) {
                            ring.push(p);
                        }
                    }
                }
            }
            if ring.len() >= n {
                ring.sort_by_key(|p| (p.y().u8(), p.x().u8()));
                ring.truncate(n);
                return (center, ring);
            }
        }
    }
    panic!("no free lab-cluster (a center with {n} free range-2 neighbors) within radius 6 of the controller");
}

/// The tiles at exactly Chebyshev `radius` from `p` (radius 0 = `p` itself), row-major.
fn ring_tiles(p: Position, radius: i32) -> Vec<Position> {
    let mut out = Vec::new();
    if radius == 0 {
        return vec![p];
    }
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx.abs().max(dy.abs()) != radius {
                continue;
            }
            if let Ok(q) = p.checked_add((dx, dy)) {
                out.push(q);
            }
        }
    }
    out.sort_by_key(|q| (q.y().u8(), q.x().u8()));
    out
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Metric 1 — compound time-to-X (the mineral pipeline's throughput).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The `XGH2O` upgrade-boost compound the M6 economy brews (the WORK-upgrade T3 boost).
pub const TARGET_COMPOUND: SimResource = SimResource::XGH2O;

/// **Compound time-to-X** (ADR §D7 M6): the tick at which the reaction lab cluster has produced
/// `target_units` of [`TARGET_COMPOUND`] (XGH2O) through its full chain. The input labs are
/// replenished each tick with base reagents (abstracting the mineral IMPORT/haul — the mechanic
/// under test is the reaction-chain THROUGHPUT, paced by the per-compound `REACTION_TIME`
/// cooldowns, not the haul FSM). Drives a staged reaction script that walks G→GH→GH2O→XGH2O,
/// producing intermediate compounds into the output lab and cycling them back as inputs.
///
/// Returns `Some(tick)` on reaching the target, `None` on the tick cap (the pipeline stalled).
/// This is the headline mineral-pipeline number the ADR names.
pub fn compound_time_to(world: &mut EconWorld, cluster: &LabCluster, target_units: u32, tick_cap: u32) -> Option<u32> {
    use SimResource::*;
    // The XGH2O chain, bottom-up. Each STAGE reacts two reagents (imported from storage / the
    // deposit) into the output lab, then the product is carried forward as the next stage's
    // reagent. With ONE output lab, we brew the chain stage-by-stage, clearing the output between
    // stages (abstracting the intra-cluster mineral shuffle a real 10-lab room does in parallel).
    // The pipeline throughput is paced by the per-stage REACTION_TIME cooldowns — the mechanic
    // under test. The chain for XGH2O: G+H→GH, GH+OH→GH2O, GH2O+X→XGH2O.
    let stages: [(SimResource, SimResource, SimResource); 3] =
        [(Ghodium, Hydrogen, GH), (Hydroxide, GH, GH2O), (Catalyst, GH2O, XGH2O)];
    let (a, b, o) = (cluster.in_a, cluster.in_b, cluster.out);
    let mut produced = 0u32;
    let mut stage = 0usize;
    for tick in 0..tick_cap {
        let (r1, r2, product) = stages[stage];
        // The output lab must be empty (or hold the product with room) to react. Clear any stray
        // content — the finished/intermediate product was carried forward by the stage advance.
        if world.labs[o].mineral.map(|(m, _)| m != product).unwrap_or(false) {
            world.labs[o].mineral = None;
        }
        fill_input(world, a, r1);
        fill_input(world, b, r2);
        let mut intents = EconIntents::new();
        intents.react(o, a, b);
        resolve_econ_tick(world, &intents);
        // If the reaction produced (or grew) the product, advance to the next stage / bank.
        if world.labs[o].mineral.map(|(m, _)| m == product).unwrap_or(false) {
            if stage + 1 < stages.len() {
                // Carry the intermediate forward: the output's product becomes the next stage's
                // second reagent (fill_input imports 5 of it), and the output is cleared to react.
                stage += 1;
                world.labs[o].mineral = None;
            } else {
                // Final stage: bank the XGH2O batch (5 units) and restart the chain.
                if let Some((XGH2O, n)) = world.labs[o].mineral {
                    produced += n;
                }
                world.labs[o].mineral = None;
                stage = 0;
            }
        }
        if produced >= target_units {
            return Some(tick + 1);
        }
    }
    None
}

/// Fill a lab's input to exactly the reaction amount of `reagent` (imported — the mineral IMPORT
/// abstraction; a real room hauls reagents in from storage / the extractor). Replaces any other
/// content (a real bot empties a mis-stocked input lab first).
fn fill_input(world: &mut EconWorld, lab: usize, reagent: SimResource) {
    let amount = screeps_econ_engine::constants::LAB_REACTION_AMOUNT;
    world.labs[lab].mineral = Some((reagent, amount));
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Metric 2 — boost e/t-equivalent (the ADR 0033 §D5.4 military-w input).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// **Boost e/t-equivalent diagnostic** (ADR §D7 M6; the ADR 0033 §D5.4 military-`w` arm's input):
/// the EXTRA controller PROGRESS-per-tick a T3-WORK-boosted upgrader buys over an unboosted one,
/// expressed as an energy-equivalent rate. Progress is worth 1:1 with energy (each progress point
/// is a point of controller value), so the extra progress IS the boost's e/t-equivalent. A T3
/// upgrade boost is ×2 and the engine charges the SAME (unboosted) energy for double the progress
/// (`upgradeController.js:70,92`), so a `W`-WORK upgrader gains `+W` progress/tick from the boost
/// = **+W e/t-equivalent**. Measured directly from the engine's controller progress delta (not
/// asserted).
///
/// The number this returns is what §D5.4's `w(creep)` military arm consumes when pricing a boosted
/// upgrader/worker's value; **M6 reports it, its consumption is combat's** (the ADR's division).
pub fn boost_e_t_equivalent(work_parts: u32) -> u32 {
    // Build a minimal controller world and measure the PROGRESS a boosted vs unboosted upgrader
    // gains in one tick (the extra progress = the boost's e/t-equivalent).
    let progress_gained = |boosted: bool| -> u32 {
        let mut w = EconWorld::default();
        w.set_controller(pos(30, 30), 5);
        let before = w.controller.as_ref().unwrap().progress;
        let mut body: Vec<Part> = vec![Part::Work; work_parts as usize];
        body.extend([Part::Carry, Part::Move]);
        let c = w.add_creep(pos(31, 30), &body, 100_000);
        w.creep_stores.get_mut(&c).unwrap().add(SimResource::Energy, 10_000);
        w.sync_carry_used(c);
        if boosted {
            for p in w.creep_mut(c).unwrap().body.parts.iter_mut() {
                if p.part == Part::Work {
                    p.boost = screeps_sim_core::BoostTier::T3;
                }
            }
        }
        let mut i = EconIntents::new();
        i.act(c, EconAction::UpgradeController);
        resolve_econ_tick(&mut w, &i);
        w.controller.as_ref().unwrap().progress - before
    };
    progress_gained(true).saturating_sub(progress_gained(false))
}

fn pos(x: u8, y: u8) -> Position {
    use screeps::{RoomCoordinate, RoomName};
    let room: RoomName = "W1N1".parse().unwrap();
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Metric 3 — the boosted-upgrader T_RCL(6+) probe (ONE pair; the M2 N=6 exclusion reason).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// One arm of the T_RCL probe: ticks for a fleet of `n_upgraders` upgraders (each `work_parts`
/// WORK), optionally T3-boosted, to accumulate `progress_target` controller progress on an RCL6
/// room — fed energy each tick (an infinite supply, isolating the UPGRADE throughput mechanic the
/// boost changes). Returns the tick reached.
pub fn t_rcl_probe_arm(world0: &EconWorld, cluster: &LabCluster, n_upgraders: u32, work_parts: u32, boosted: bool, progress_target: u32, tick_cap: u32) -> Option<u32> {
    let mut world = world0.clone();
    let cpos = world.controller.as_ref().expect("RCL6 world has a controller").pos;
    // Reset controller progress to 0 so both arms measure the same span; ensure it stays level 6
    // (the probe measures progress within the level, not a level-up — the tick cost of a full RCL6→7
    // is the M2 exclusion reason, so the probe targets a FIXED sub-level progress).
    if let Some(c) = world.controller.as_mut() {
        c.progress = 0;
        c.downgrade_ticks = screeps_econ_engine::constants::controller_downgrade(6); // full clock: no downgrade in the window
    }
    // Field the upgraders adjacent to the controller.
    let mut body: Vec<Part> = vec![Part::Work; work_parts as usize];
    body.extend([Part::Carry, Part::Carry, Part::Move]);
    let _ = cluster; // the probe isolates the UPGRADE effect; boost logistics are engine-fenced
    let mut creeps = Vec::new();
    for k in 0..n_upgraders {
        let tile = upgrader_tile(&world, cpos, k as u8);
        let id = world.add_creep(tile, &body, 100_000);
        if boosted {
            // Boost the WORK parts to T3 IN PLACE (the boost EFFECT is what the probe measures; the
            // lab boostCreep mechanic itself is fenced in the engine's determinism tests). The
            // creep stays controller-adjacent — teleporting it to a lab would break its upgrade
            // range and confound the measurement.
            for p in world.creep_mut(id).expect("just added").body.parts.iter_mut() {
                if p.part == Part::Work {
                    p.boost = screeps_sim_core::BoostTier::T3;
                }
            }
        }
        creeps.push(id);
    }
    for tick in 0..tick_cap {
        let mut intents = EconIntents::new();
        for &id in &creeps {
            // Keep each upgrader topped up (infinite supply — isolate the upgrade throughput).
            if let Some(store) = world.creep_stores.get_mut(&id) {
                if store.amount(SimResource::Energy) < work_parts * 2 {
                    store.add(SimResource::Energy, 2000);
                }
            }
            world.sync_carry_used(id);
            intents.act(id, EconAction::UpgradeController);
        }
        resolve_econ_tick(&mut world, &intents);
        if world.controller.as_ref().map(|c| c.progress).unwrap_or(0) >= progress_target {
            return Some(tick + 1);
        }
        // A level-up would reset progress — cap the probe at RCL6 by re-flooring the clock so no
        // level-up fires inside the window (the FIXED sub-level target is below the RCL6→7 cost).
        if let Some(c) = world.controller.as_mut() {
            c.downgrade_ticks = screeps_econ_engine::constants::controller_downgrade(c.level);
        }
    }
    None
}

/// The probe result: the two arms' ticks + the boost delta.
#[derive(Clone, Copy, Debug)]
pub struct ProbeResult {
    pub unboosted_ticks: Option<u32>,
    pub boosted_ticks: Option<u32>,
    /// Ticks SAVED by boosting (unboosted − boosted); `None` if either arm didn't finish.
    pub delta_ticks: Option<i64>,
    /// The measured single-arm tick cost (max of the two) — the affordability number the M2
    /// exclusion was about.
    pub max_arm_ticks: u32,
    pub progress_target: u32,
}

/// **The boosted-upgrader T_RCL(6+) probe** (ADR §D7 M6, the M2 N=6 exclusion): ONE pair —
/// boosted vs unboosted upgraders on a realized RCL6 room — to a FIXED progress target chosen so
/// the tick cost is affordable (measured first). A T3 upgrade boost is ×2, so the boosted arm
/// should reach the target in ~half the ticks.
pub fn boosted_upgrader_probe(world0: &EconWorld, cluster: &LabCluster, progress_target: u32, tick_cap: u32) -> ProbeResult {
    // A modest fleet: 4 upgraders × 10 WORK = 40 progress/tick unboosted, 80 boosted.
    let (n, work) = (4u32, 10u32);
    let unboosted = t_rcl_probe_arm(world0, cluster, n, work, false, progress_target, tick_cap);
    let boosted = t_rcl_probe_arm(world0, cluster, n, work, true, progress_target, tick_cap);
    let delta = match (unboosted, boosted) {
        (Some(u), Some(b)) => Some(u as i64 - b as i64),
        _ => None,
    };
    let max_arm = unboosted.unwrap_or(tick_cap).max(boosted.unwrap_or(tick_cap));
    ProbeResult { unboosted_ticks: unboosted, boosted_ticks: boosted, delta_ticks: delta, max_arm_ticks: max_arm, progress_target }
}

/// A free tile adjacent to `p` (row-major), if any.
fn adjacent_free(world: &EconWorld, p: Position) -> Option<Position> {
    let mut cands: Vec<Position> = Vec::new();
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            if let Ok(q) = p.checked_add((dx, dy)) {
                cands.push(q);
            }
        }
    }
    cands.sort_by_key(|q| (q.y().u8(), q.x().u8()));
    cands.into_iter().find(|&q| world.is_walkable(q))
}

/// A deterministic tile near the controller for the k-th upgrader (a ring scan).
fn upgrader_tile(world: &EconWorld, cpos: Position, k: u8) -> Position {
    let mut count = 0u8;
    for radius in 1..=3i32 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                if let Ok(p) = cpos.checked_add((dx, dy)) {
                    if world.is_walkable(p) {
                        if count == k {
                            return p;
                        }
                        count += 1;
                    }
                }
            }
        }
    }
    panic!("no free upgrader tile within radius 3 of the controller");
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Metric 4 — the terminal recovery-lever delta (the first market-ONLY capability delta).
// ═════════════════════════════════════════════════════════════════════════════════════════════
//
// ADR §D7 M6: "collapse WITH a stocked terminal mineral inventory — does selling-for-energy
// improve T_recover?" The baseline bot has NO sell behavior, so this is a MARKET-ARM capability
// only — the first delta attributable to a capability the current bot lacks. Both arms bootstrap
// the same collapse world (storage holds a large mineral stash + a little energy); the LEVER arm
// additionally sells the mineral for energy each tick (the recovery liquidity), the CONTROL arm
// leaves the stash as dead weight.

/// A minimal deterministic collapse-recovery driver: a wiped RCL≥4 room bootstraps from spawn
/// self-charge. `use_lever` sells a slice of the storage mineral stash for energy each tick (the
/// M6 recovery lever); the energy proceeds go to storage. Returns the tick the spawn lane first
/// reaches capacity sustained for `RECOVER_FULL_WINDOW`, or `None` at the cap. Both arms share the
/// bootstrap logic — the ONLY difference is the sell lever, so the delta is the lever's alone.
///
/// The bootstrap is intentionally simple (one shuttle harvester ferrying source energy to the
/// spawn lane, plus the storage→lane refill) — the mechanic under test is the recovery LIQUIDITY
/// the sell lever adds, not the full civilian FSM.
pub fn recovery_lever_t_recover(world0: &EconWorld, mineral: SimResource, sell_per_tick: u32, use_lever: bool, tick_cap: u32) -> Option<u32> {
    let mut world = world0.clone();
    let full_window = crate::metrics::RecoverConsts::default().full_window;
    let capacity = crate::baseline::spawn_lane_capacity(&world);
    let mut full_streak = 0u32;
    // A single miner beside the nearest source, harvesting into its own store; its harvested energy
    // is deposited to STORAGE each tick (a reliable, pathing-free income model — the mechanic under
    // test is the recovery LIQUIDITY the sell lever adds, not haul pathing; both arms share this
    // identical income model, so the delta is the lever's alone). Income = 2 e/WORK/t, capped by
    // the source's 10 e/t regen ceiling.
    let src_pos = world.sources[0].pos;
    let harvester_tile = adjacent_free(&world, src_pos).expect("a source has a free neighbor");
    let hb = crate::baseline::harvester_body(600).expect("≥300");
    let harvester = world.add_creep(harvester_tile, &hb, 100_000);

    for tick in 0..tick_cap {
        let mut intents = EconIntents::new();
        // The lever: sell a slice of the mineral stash for energy into storage.
        if use_lever {
            intents.sell(mineral, sell_per_tick);
        }
        // The miner harvests the adjacent source into its store.
        if world.creep(harvester).is_some() {
            intents.act(harvester, EconAction::Harvest { source_idx: 0 });
        }
        resolve_econ_tick(&mut world, &intents);
        // Sweep the miner's harvested energy into storage (the pathing-free income model — both
        // arms identical), then refill the spawn lane from storage (the refill hauler abstraction).
        if let Some(store) = world.creep_stores.get_mut(&harvester) {
            let held = store.amount(SimResource::Energy);
            if held > 0 {
                store.remove(SimResource::Energy, held);
                if let Some(s) = world.storage.as_mut() {
                    s.store.add(SimResource::Energy, held);
                }
            }
        }
        world.sync_carry_used(harvester);
        refill_lane_from_storage(&mut world);

        let full = capacity > 0 && world.room_spawn_energy() >= capacity;
        full_streak = if full { full_streak + 1 } else { 0 };
        if full_streak >= full_window {
            return Some(tick + 1);
        }
    }
    None
}

/// Move storage energy into the spawn lane (deterministic refill abstraction — spawns first, then
/// extensions, closest-ish by index). Both recovery-lever arms call this identically.
fn refill_lane_from_storage(world: &mut EconWorld) {
    let Some(available) = world.storage.as_ref().map(|s| s.store.amount(SimResource::Energy)) else { return };
    if available == 0 {
        return;
    }
    let mut budget = available.min(100); // a bounded per-tick refill rate (one hauler's worth)
    let spawn_free = screeps_econ_engine::constants::SPAWN_ENERGY_CAPACITY.saturating_sub(world.spawns[0].store_energy);
    let to_spawn = budget.min(spawn_free);
    if to_spawn > 0 {
        world.spawns[0].store_energy += to_spawn;
        world.storage.as_mut().unwrap().store.remove(SimResource::Energy, to_spawn);
        budget -= to_spawn;
    }
    for e in 0..world.extensions.len() {
        if budget == 0 {
            break;
        }
        let free = world.extensions[e].capacity.saturating_sub(world.extensions[e].store_energy);
        let take = budget.min(free);
        if take > 0 {
            world.extensions[e].store_energy += take;
            world.storage.as_mut().unwrap().store.remove(SimResource::Energy, take);
            budget -= take;
        }
    }
}

/// The recovery-lever delta result (the first market-only capability delta).
#[derive(Clone, Copy, Debug)]
pub struct RecoveryLeverResult {
    pub with_lever: Option<u32>,
    pub without_lever: Option<u32>,
    /// Ticks SAVED by the lever (without − with); positive = the lever speeds recovery.
    pub delta_ticks: Option<i64>,
}

/// **The recovery-lever delta** (ADR §D7 M6, the first market-only capability delta): a collapse
/// world with a stocked mineral stash, measured with vs without the sell-mineral-for-energy lever.
pub fn recovery_lever_delta(world0: &EconWorld, mineral: SimResource, sell_per_tick: u32, tick_cap: u32) -> RecoveryLeverResult {
    let with_lever = recovery_lever_t_recover(world0, mineral, sell_per_tick, true, tick_cap);
    let without = recovery_lever_t_recover(world0, mineral, sell_per_tick, false, tick_cap);
    let delta = match (with_lever, without) {
        (Some(w), Some(wo)) => Some(wo as i64 - w as i64),
        _ => None,
    };
    RecoveryLeverResult { with_lever, without_lever: without, delta_ticks: delta }
}

/// Build a collapse-with-mineral-stash world for the recovery-lever measurement: a captured RCL≥4
/// layout, spawn lane drained to 0, storage holding a large mineral stash + a small energy seed.
pub fn recovery_lever_world(room: &str, mineral: SimResource, stash: u32, seed: u32) -> EconWorld {
    let layouts = captured_layouts();
    let layout = layouts.iter().find(|l| l.room == room).unwrap_or_else(|| panic!("recovery layout `{room}` not cached"));
    let realized = realize(layout, &RealizeParams { rcl: 4, road_health_pct: 100, seed });
    let mut world = realized.world;
    for s in &mut world.spawns {
        s.store_energy = 0;
    }
    let storage = world.storage.as_mut().expect("RCL4 has storage");
    storage.store.add(mineral, stash);
    storage.store.add(SimResource::Energy, 200); // a tiny seed — the room must rebootstrap
    world
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mineral-economy family instantiates: RCL6 world with a typed mineral pool + extractor +
    /// the 4-lab cluster, all pathing-blocked where they should be.
    #[test]
    fn mineral_family_instantiates() {
        // The FULL corpus (every mineral-bearing room) — the range-2 reaction geometry must hold
        // for every layout, not just the fast pair.
        for sc in mineral_catalog(1, true) {
            let (world, _, _, cluster) = sc.instantiate();
            assert!(!world.minerals.is_empty(), "{}: mineral present", sc.name);
            assert!(world.minerals[0].resource.is_mineral());
            assert_eq!(world.minerals[0].density, screeps_econ_engine::constants::DENSITY_MODERATE);
            assert_eq!(world.extractors.len(), 1, "{}: extractor on the mineral", sc.name);
            assert_eq!(world.extractors[0].pos, world.minerals[0].pos);
            assert_eq!(world.labs.len(), 4, "{}: the reaction + boost lab cluster", sc.name);
            // The three reaction labs are mutually range ≤ 2 (the reaction geometry constraint).
            let (a, b, o) = (world.labs[cluster.in_a].pos, world.labs[cluster.in_b].pos, world.labs[cluster.out].pos);
            assert!(o.get_range_to(a) <= 2 && o.get_range_to(b) <= 2, "{}: output lab in range 2 of inputs", sc.name);
        }
    }

    /// Compound time-to-X: the pipeline brews XGH2O and reaches a small target within the cap; the
    /// number is deterministic (same seed ⇒ same tick).
    #[test]
    fn compound_time_to_is_finite_and_deterministic() {
        let run = || {
            let sc = mineral_catalog(1, false)[0].clone();
            let (mut world, _, _, cluster) = sc.instantiate();
            compound_time_to(&mut world, &cluster, 20, 5_000)
        };
        let t = run();
        assert!(t.is_some(), "the pipeline brews 20 XGH2O within 5k ticks (got {t:?})");
        assert_eq!(t, run(), "compound time-to-X is deterministic");
    }

    /// The boost e/t-equivalent diagnostic: a T3 upgrade boost is ×2, so per WORK part the boost
    /// buys +1 progress/tick (= +1 e/t-equivalent, progress 1:1 energy). For W WORK, +W e/t.
    #[test]
    fn boost_e_t_equivalent_matches_the_double_effect() {
        assert_eq!(boost_e_t_equivalent(1), 1, "1 WORK: T3 upgrade ×2 buys +1 progress/tick");
        assert_eq!(boost_e_t_equivalent(10), 10, "10 WORK: +10 e/t-equivalent (the §D5.4 input)");
    }

    /// The recovery-lever delta (the first market-only capability delta): both arms recover; the
    /// lever arm is no slower (selling a stuck mineral stash for energy adds recovery liquidity),
    /// and the measurement is deterministic.
    #[test]
    fn recovery_lever_delta_is_measured_and_deterministic() {
        let room = mineral_catalog(1, false)[0].layout_room.clone();
        let w = recovery_lever_world(&room, SimResource::Ghodium, 50_000, 1);
        let r = recovery_lever_delta(&w, SimResource::Ghodium, 100, 20_000);
        // THE market-only capability delta: the lever arm recovers (the stuck mineral stash
        // becomes spendable energy that funds the refill), and it is never SLOWER than the control
        // (which has only the tiny energy seed + source trickle). The lever's whole point is that
        // it unlocks a recovery path the current bot has no behavior for.
        assert!(r.with_lever.is_some(), "the sell-lever arm recovers ({r:?})");
        if let (Some(w), Some(wo)) = (r.with_lever, r.without_lever) {
            assert!(w <= wo, "the lever never slows recovery ({r:?})");
        }
        // Deterministic across runs.
        let r2 = recovery_lever_delta(&w, SimResource::Ghodium, 100, 20_000);
        assert_eq!((r.with_lever, r.without_lever), (r2.with_lever, r2.without_lever));
    }

    /// The boosted-upgrader T_RCL(6+) probe: the boosted arm reaches the target in ~half the ticks
    /// (the ×2 effect), and the tick cost is measured (the M2 exclusion number).
    #[test]
    fn boosted_upgrader_probe_halves_the_time() {
        let sc = mineral_catalog(1, false)[0].clone();
        let (world, _, _, cluster) = sc.instantiate();
        // A small progress target so the test is fast; the bench uses a larger one.
        let r = boosted_upgrader_probe(&world, &cluster, 40_000, 20_000);
        assert!(r.unboosted_ticks.is_some() && r.boosted_ticks.is_some(), "both arms finish ({r:?})");
        let (u, b) = (r.unboosted_ticks.unwrap(), r.boosted_ticks.unwrap());
        assert!(b < u, "boosted reaches the target faster ({b} < {u})");
        // ×2 effect: boosted ≈ half. Allow a wide band (fielding/warmup ticks).
        assert!(b * 2 >= u && b * 2 <= u + 4, "boosted ~half the unboosted ticks (u={u} b={b})");
        assert!(r.delta_ticks.unwrap() > 0);
    }
}

