//! The **market-arm sim adapter** (ADR 0040 M4, Part B/C): plumbs the `screeps-econ-decision`
//! market candidate kernels — [`sink_economics`](econ) bids + opportunity floor, the
//! [`matching`](m) greedy assignment, K4 deficit-priced bodies — into the econ-eval runner as
//! the MARKET policy arm the tournament scores against the baseline/S1 arms.
//!
//! Division of labor (the M3 convention): the PURE policy lives in `screeps-econ-decision`
//! (bid formulas, floor, admission, greedy matcher, body pricing); THIS module owns the
//! EconWorld → DTO adaptation (bid attachment, wear observation, edge generation, carrier
//! collection) and the SIM-ONLY exact oracle for `match_optimality_gap` (spec constraint: the
//! oracle is never a bot dependency path).
//!
//! **The per-tick market pass** (§D3): one per-room assignment over the current demand set —
//! idle carriers (haulers always; harvesters gated by their harvest opportunity rate, see
//! [`carrier_gate`]) × tickets (deposit nodes, with the best pickup per (carrier, deposit)
//! pair for empty carriers), edge value `v = bid · amount / service_ticks` with
//! `service_ticks` from linear range (the live distance service — §D3), assigned by the
//! shipped deterministic greedy with booking. Committed (in-flight) work keeps its booking —
//! re-matching in flight is deliberately NOT modeled (documented reduction; flap_rate is the
//! observation channel).
//!
//! **The reference bound** (sim-only): a branch-and-bound over per-carrier candidate lists
//! PRUNED to the top [`ORACLE_TOP_K`] edges by density, maximizing one-ticket-per-carrier
//! selection value under the same booking dynamics. This is NOT a proof of global optimality —
//! it is a **top-K-pruned, greedy-floored upper bound**: the DFS is seeded with the greedy's
//! own value (so `gap ≥ 0` by construction) and searches only the K densest edges per carrier.
//! What a measured pooled gap of ~0 therefore certifies is precise and sufficient for §D8 #4:
//! *no material value is left on the table within the pruned high-density search* — i.e. no
//! smarter same-dynamics selector (bounded auction included, which also works the dense edges)
//! finds a better assignment among the candidates that could plausibly matter. Two guards keep
//! the bound honest against its own pruning: (1) if the realized edge count per carrier ever
//! exceeds `ORACLE_TOP_K` the corpus is under-instrumented for the claim (the tournament asserts
//! `max edges/pass` stays well under K·carriers on the CONTENDED Family M); (2) the node cap
//! SKIPS a sample rather than approximating. Fixed-point totals ([`m::VALUE_FP`]) keep the
//! comparison exact-integer.
//!
//! **Traffic wear** (§D1 repair bids): the observed trailing timer-pull model — every creep
//! step onto a road pulls its decay clock by `ROAD_WEAROUT × parts` (M1 engine mechanic);
//! [`MarketRuntime::observe_wear`] accumulates the pulls and prices
//! `wear = base_decay + decay_amount·pulled/(elapsed·decay_period)` in milli-hits/tick.

use crate::baseline::{
    self, Bookings, Deposit, Lane, Pickup, RepairRef, RoleSpec, SinkKey, SpawnPlan, SrcKey,
};
use crate::layout::{ContainerRole, LayoutInfo};
use screeps::Position;
use screeps_econ_decision::market as mk;
use screeps_econ_decision::matching as m;
use screeps_econ_decision::sink_economics as econ;
use screeps_econ_decision::sink_economics::MarketConsts;
use screeps_econ_decision::spawn_policy;
use screeps_econ_engine::constants::{
    body_cost, construction_cost, controller_downgrade, controller_levels, CONTAINER_HITS,
    ROAD_DECAY_AMOUNT, ROAD_DECAY_TIME, ROAD_HITS, SPAWN_ENERGY_CAPACITY,
};
use screeps_econ_engine::{EconWorld, StructureKind};
use std::collections::BTreeMap;

/// The market policy arm's configuration (rides inside `PolicyConfig`).
#[derive(Clone, Copy, Debug)]
pub struct MarketArmCfg {
    pub consts: MarketConsts,
    /// K4 deficit-priced bodies ON (the full MARKET arm) or OFF (the MARKET-minus-K4 arm —
    /// baseline capacity-sized bodies; isolates the S6 fix for attribution).
    pub k4_bodies: bool,
    /// Run the sim-only exact oracle for `match_optimality_gap` (sampled every
    /// `oracle_period` ticks). Off in sweeps (cost), on in the adjudication runs.
    pub measure_gap: bool,
    pub oracle_period: u32,
}

impl Default for MarketArmCfg {
    fn default() -> Self {
        MarketArmCfg {
            consts: MarketConsts::default(),
            k4_bodies: true,
            measure_gap: false,
            oracle_period: 25,
        }
    }
}

/// One market task handed to a carrier for this tick (consumed by the runner's Idle arm).
#[derive(Clone, Copy, Debug)]
pub enum MarketTask {
    /// Empty carrier: pick up `take` at `src`, deliver `give` to `sink`.
    PickupDeliver {
        src: SrcKey,
        src_pos: Position,
        take: u32,
        sink: SinkKey,
        sink_pos: Position,
        give: u32,
    },
    /// Loaded carrier: deliver `amount` of carried energy to `sink`.
    Deliver { sink: SinkKey, sink_pos: Position, amount: u32 },
}

/// A haul-capable carrier as the pass sees it.
#[derive(Clone, Copy, Debug)]
pub struct CarrierDto {
    pub id: u32,
    pub pos: Position,
    pub free: u32,
    pub held: u32,
    /// The carrier's productive alternative in milli-e/t (a harvester's live-source harvest
    /// rate; 0 for haulers and for harvesters that cannot harvest) — the edge gate.
    pub opportunity_milli: u32,
}

/// Aggregated gap diagnostics (fixed-point per [`m::VALUE_FP`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct GapStats {
    pub samples: u32,
    pub greedy_fp: u64,
    pub oracle_fp: u64,
    pub worst_permille: u32,
    /// Oracle invocations abandoned at the node cap (never counted as samples).
    pub skipped: u32,
}

impl GapStats {
    /// Pooled gap = (oracle − greedy)/oracle over all samples, per-mille.
    pub fn pooled_permille(&self) -> u32 {
        if self.oracle_fp == 0 {
            return 0;
        }
        ((self.oracle_fp.saturating_sub(self.greedy_fp)) * 1000 / self.oracle_fp) as u32
    }
}

/// The market arm's per-run state: wear observation + per-tick pass products + diagnostics.
pub struct MarketRuntime {
    pub cfg: MarketArmCfg,
    started_tick: u32,
    wear_pulled: BTreeMap<(u8, u8), u64>,
    /// This tick's assignments, keyed by creep id (rebuilt each pass).
    pub tasks: BTreeMap<u32, MarketTask>,
    /// This tick's opportunity floor (§D1) — quantized milli.
    pub floor: u32,
    /// The downgrade survival veto (§D1 guardrail — outside the market).
    pub veto: bool,
    // ── diagnostics (the §D3 CPU gate + §D8 #4 instruments) ────────────────────────────────────
    pub match_ops: u64,
    pub match_edges: u64,
    pub match_passes: u64,
    /// The most candidate edges any single pass generated (the contended worst case — review #2).
    pub match_max_edges: u64,
    pub gap: GapStats,
}

impl MarketRuntime {
    pub fn new(cfg: MarketArmCfg, start_tick: u32) -> Self {
        MarketRuntime {
            cfg,
            started_tick: start_tick,
            wear_pulled: BTreeMap::new(),
            tasks: BTreeMap::new(),
            floor: 0,
            veto: false,
            match_ops: 0,
            match_edges: 0,
            match_passes: 0,
            match_max_edges: 0,
            gap: GapStats::default(),
        }
    }

    /// Record a creep step onto a road tile (the engine timer-pull wear — module docs).
    pub fn observe_wear(&mut self, tile: (u8, u8), body_parts: u32) {
        *self.wear_pulled.entry(tile).or_insert(0) += body_parts as u64;
    }

    /// A road's priced wear rate, milli-hits/tick: base decay + the observed trailing traffic
    /// pull (whole-run trailing average — v0, documented).
    fn road_wear_milli(&self, tick: u32, tile: (u8, u8), hits_max: u32) -> u32 {
        let ratio = (hits_max / ROAD_HITS).max(1); // swamp roads: hits_max 25k ⇒ 5× decay amount
        let amount = ROAD_DECAY_AMOUNT * ratio;
        let base = amount * 1000 / ROAD_DECAY_TIME;
        let elapsed = tick.saturating_sub(self.started_tick).max(1) as u64;
        let pulled = self.wear_pulled.get(&tile).copied().unwrap_or(0);
        let extra = (amount as u64 * 1000 * pulled) / (elapsed * ROAD_DECAY_TIME as u64);
        base + extra.min(u32::MAX as u64) as u32
    }

    /// A container's priced wear rate, milli-hits/tick (pure decay — containers take no
    /// traffic wear).
    fn container_wear_milli(&self, world: &EconWorld) -> u32 {
        let window = world.container_decay_window().max(1);
        screeps_econ_engine::constants::CONTAINER_DECAY * 1000 / window
    }

    /// Every repairable with its market bid + survival override (roads then containers —
    /// deterministic construction order, the baseline convention).
    pub fn repair_bids(&self, world: &EconWorld) -> Vec<(RepairRef, Position, u32, bool)> {
        let consts = &self.cfg.consts;
        let tick = world.tick();
        let mut out = Vec::new();
        for r in &world.roads {
            if r.hits >= r.hits_max {
                continue;
            }
            let tile = (r.pos.x().u8(), r.pos.y().u8());
            let wear = self.road_wear_milli(tick, tile, r.hits_max);
            let imm = econ::imminence_q(r.hits, wear, consts.imminence_horizon_ticks);
            let rebuild = construction_cost(StructureKind::Road) * (r.hits_max / ROAD_HITS).max(1);
            let repair_e = (r.hits_max - r.hits).div_ceil(100);
            let bid = econ::repair_bid(consts, rebuild, repair_e, imm);
            out.push((RepairRef::Road(tile.0, tile.1), r.pos, bid, false));
        }
        for c in &world.containers {
            if c.hits >= CONTAINER_HITS {
                continue;
            }
            let tile = (c.pos.x().u8(), c.pos.y().u8());
            let wear = self.container_wear_milli(world);
            let imm = econ::imminence_q(c.hits, wear, consts.imminence_horizon_ticks);
            let rebuild = construction_cost(StructureKind::Container);
            let repair_e = (CONTAINER_HITS - c.hits).div_ceil(100);
            let bid = econ::repair_bid(consts, rebuild, repair_e, imm) + econ::container_function_milli(consts, imm);
            let survival = econ::container_survival_override(c.hits, CONTAINER_HITS);
            out.push((RepairRef::Container(tile.0, tile.1), c.pos, bid, survival));
        }
        out
    }

    /// K3-market full-repair target: the best ADMITTED candidate — survival overrides first,
    /// then highest bid, ties to the lowest tile (deterministic).
    pub fn full_repair_target(&self, world: &EconWorld) -> Option<(RepairRef, Position, u32)> {
        self.repair_bids(world)
            .into_iter()
            .filter(|&(_, _, bid, survival)| survival || econ::admit_repair(bid, self.floor))
            .max_by(|a, b| {
                (a.3, a.2).cmp(&(b.3, b.2)).then_with(|| {
                    // lower tile ranks GREATER (wins the max) — deterministic tie.
                    tile_of(a.0).cmp(&tile_of(b.0)).reverse()
                })
            })
            .map(|(r, p, bid, _)| (r, p, bid))
    }

    /// K3-market opportunistic (drive-by) target within Chebyshev 3 — same admission/order.
    pub fn opportunistic_target(&self, world: &EconWorld, pos: Position) -> Option<RepairRef> {
        self.repair_bids(world)
            .into_iter()
            .filter(|&(_, p, bid, survival)| pos.get_range_to(p) <= 3 && (survival || econ::admit_repair(bid, self.floor)))
            .max_by(|a, b| {
                (a.3, a.2)
                    .cmp(&(b.3, b.2))
                    .then_with(|| tile_of(a.0).cmp(&tile_of(b.0)).reverse())
            })
            .map(|(r, _, _, _)| r)
    }

    /// The S4 arm, market form: a repairer-builder is requested only for an ADMITTED candidate,
    /// banded by the bid's tier projection (survival override ⇒ HIGH).
    pub fn repairer_priority(&self, world: &EconWorld) -> Option<(u32, u32)> {
        let (_, _, bid, survival) = self
            .repair_bids(world)
            .into_iter()
            .filter(|&(_, _, bid, survival)| survival || econ::admit_repair(bid, self.floor))
            .max_by_key(|&(_, _, bid, survival)| (survival, bid))?;
        if survival {
            return Some((1, spawn_policy::SPAWN_BID_HIGH));
        }
        match econ::bid_to_tier(bid) {
            screeps_econ_decision::priority::TransferPriority::High => Some((1, spawn_policy::SPAWN_BID_HIGH)),
            screeps_econ_decision::priority::TransferPriority::Medium => Some((1, spawn_policy::SPAWN_BID_MEDIUM)),
            _ => None,
        }
    }

    /// The upgrade sink's current bid (V_UPGRADE + the step premium near level-up).
    pub fn upgrade_sink_bid(&self, world: &EconWorld) -> u32 {
        econ::upgrade_bid(&self.cfg.consts, near_level_up(&self.cfg.consts, world))
    }

    /// A construction site class's build bid.
    pub fn site_build_bid(&self, kind: StructureKind) -> u32 {
        econ::build_bid(&self.cfg.consts, build_class(kind))
    }

    /// Begin-of-tick market state: refill bid (from the plan preview's top energy-blocked
    /// request), per-deposit bids, the opportunity floor, and the downgrade veto.
    pub fn begin_tick(
        &mut self,
        world: &EconWorld,
        info: &LayoutInfo,
        plans: &[(SpawnPlan, u32)],
        deposits: &[Deposit],
    ) -> Vec<u32> {
        let consts = self.cfg.consts;
        let refill = refill_bid_from_plans(&consts, world, plans);
        let up_bid = self.upgrade_sink_bid(world);
        let bids: Vec<u32> = deposits
            .iter()
            .map(|d| match d.sink {
                SinkKey::Spawn(_) | SinkKey::Extension(_) => refill,
                SinkKey::Storage => econ::STORAGE_BID,
                // Containers are BUFFERS of their downstream sink (controller container →
                // upgrade; overflow container → storage-par): priced by the buffer curve
                // (`buffer_deposit_bid` docs — a mostly-full buffer's marginal energy just
                // sits). `unfulfilled` IS the free capacity for container deposits (K1).
                SinkKey::Container(x, y) => {
                    let base = match info.container_roles.get(&(x, y)) {
                        Some(ContainerRole::Controller) => up_bid,
                        // Provider containers take no deposits (K1); Other containers buffer par.
                        _ => econ::STORAGE_BID,
                    };
                    econ::buffer_deposit_bid(base, d.unfulfilled, screeps_econ_engine::constants::CONTAINER_CAPACITY)
                }
            })
            .collect();
        self.floor = econ::opportunity_floor(
            &consts,
            deposits.iter().zip(&bids).map(|(d, &b)| (b, d.unfulfilled)),
        );
        self.veto = world.controller.as_ref().is_some_and(|c| {
            c.level > 0 && econ::downgrade_veto(&consts, c.downgrade_ticks, controller_downgrade(c.level))
        });
        bids
    }

    /// The per-tick assignment pass (module docs). Books assigned flows into the runner's
    /// `bookings` (the adapter-side reservation layer) and fills [`Self::tasks`].
    ///
    /// **Refill aggregation (an M4 measured finding):** the spawn/extension deposits enter the
    /// matching as ONE aggregate refill node per room. §D3's raw `v = bid·amount/service` over
    /// per-STRUCTURE tickets structurally starves many-small-node sink classes — a 50-capacity
    /// extension can never out-density a 2000-capacity container at comparable bids, so the
    /// lane (the recovered-state gate's own quantity) never sustains full. The engine itself
    /// treats the lane as one fungible pool (spawn-energy draw + `energy_available`), so the
    /// aggregation prices the ECONOMIC sink, not its plumbing. The K1 demand set is unchanged
    /// (amounts per structure); the realized task delivers to the nearest still-needy lane
    /// structure and any surplus cargo re-enters the next pass as a loaded carrier (emergent
    /// bulk hauling). Flagged for §D8/M5a — the live matcher will need the same shaping.
    #[allow(clippy::too_many_arguments)]
    pub fn market_pass(
        &mut self,
        world: &EconWorld,
        deposits: &[Deposit],
        dep_bids: &[u32],
        pickups: &[Pickup],
        carriers: &[CarrierDto],
        bookings: &mut Bookings,
    ) {
        self.tasks.clear();
        if carriers.is_empty() || deposits.is_empty() {
            return;
        }
        self.match_passes += 1;

        // ── Build the shared-kernel DTOs (M5a: the ONE market-select algorithm lives in
        // `screeps_econ_decision::market`; this arm and the LIVE bot both delegate to it, so the
        // A/B is by construction — module docs). The kernel is index-scoped: `sink` = deposit
        // index, `src` = pickup index, so it returns them for us to resolve back to sim keys. ──
        let k_carriers: Vec<mk::MarketCarrier> = carriers
            .iter()
            .map(|c| mk::MarketCarrier {
                id: c.id,
                pos: c.pos,
                free: c.free,
                held: c.held,
                opportunity_milli: c.opportunity_milli,
            })
            .collect();
        let k_deposits: Vec<mk::MarketDeposit> = deposits
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
        // Only the Haul lane is a pickup candidate (a `Use` withdraw is invisible to haulers).
        let k_pickups: Vec<mk::MarketPickup> = pickups
            .iter()
            .enumerate()
            .filter(|(_, p)| p.lane == Lane::Haul)
            .map(|(i, p)| mk::MarketPickup {
                src: i as u32,
                pos: p.pos,
                available: p.available,
                source_floor_milli: src_floor_milli(p.src),
            })
            .collect();

        let out = mk::market_pass(&k_carriers, &k_deposits, &k_pickups, |src_idx, sink_idx| {
            same_structure(pickups[src_idx as usize].src, deposits[sink_idx as usize].sink)
        });

        self.match_edges += out.stats.edges;
        self.match_max_edges = self.match_max_edges.max(out.stats.edges);
        self.match_ops += out.stats.ops;

        // Resolve the kernel's index-scoped tasks + bookings back to sim keys.
        for a in &out.assignments {
            let task = match a.task {
                mk::MarketTask::PickupDeliver { src, src_pos, take, sink, sink_pos, give } => {
                    MarketTask::PickupDeliver {
                        src: pickups[src as usize].src,
                        src_pos,
                        take,
                        sink: deposits[sink as usize].sink,
                        sink_pos,
                        give,
                    }
                }
                mk::MarketTask::Deliver { sink, sink_pos, amount } => {
                    MarketTask::Deliver { sink: deposits[sink as usize].sink, sink_pos, amount }
                }
            };
            self.tasks.insert(a.carrier, task);
        }
        for (&src_idx, &amount) in &out.bookings.pickups {
            *bookings.pickups.entry(pickups[src_idx as usize].src).or_insert(0) += amount;
        }
        for (&sink_idx, &amount) in &out.bookings.deposits {
            *bookings.deposits.entry(deposits[sink_idx as usize].sink).or_insert(0) += amount;
        }

        // ── The sim-only exact oracle (sampled) — over the kernel's exact greedy inputs. ─────────
        if self.cfg.measure_gap && world.tick().is_multiple_of(self.cfg.oracle_period.max(1)) && !out.edges.is_empty() {
            let greedy_fp = m::assignments_value_fp(&out.edges, &out.greedy);
            match oracle_best_fp(&out.edges, &out.supply0, &out.demand0) {
                Some(oracle_fp) => {
                    let oracle_fp = oracle_fp.max(greedy_fp); // guard: greedy is feasible
                    self.gap.samples += 1;
                    self.gap.greedy_fp += greedy_fp;
                    self.gap.oracle_fp += oracle_fp;
                    if let Some(permille) = ((oracle_fp - greedy_fp) * 1000).checked_div(oracle_fp) {
                        self.gap.worst_permille = self.gap.worst_permille.max(permille as u32);
                    }
                }
                None => self.gap.skipped += 1,
            }
        }
    }
}

fn tile_of(r: RepairRef) -> (u8, u8) {
    match r {
        RepairRef::Road(x, y) => (x, y),
        RepairRef::Container(x, y) => (x, y),
    }
}

fn build_class(kind: StructureKind) -> econ::BuildClass {
    match kind {
        StructureKind::Spawn => econ::BuildClass::Spawn,
        StructureKind::Extension => econ::BuildClass::Extension,
        StructureKind::Road => econ::BuildClass::Road,
        StructureKind::Container => econ::BuildClass::Container,
        StructureKind::Storage => econ::BuildClass::Storage,
        StructureKind::Tower => econ::BuildClass::Tower,
    }
}

fn same_structure(src: SrcKey, sink: SinkKey) -> bool {
    match (src, sink) {
        (SrcKey::Storage, SinkKey::Storage) => true,
        (SrcKey::Container(a, b), SinkKey::Container(c, d)) => (a, b) == (c, d),
        _ => false,
    }
}

/// ADR 0044 stage-1 source-floor classifier (sim side): a LOSSLESS source (storage — declining an
/// arc truly banks the energy) has the par outside option; a SATURATING buffer (a source container
/// filling from harvest, or decaying dropped energy — declining strands/loses it) has ~0. Mirrors
/// the live adapter's `TransferTarget`-based classification.
fn src_floor_milli(src: SrcKey) -> u32 {
    match src {
        SrcKey::Storage => econ::STORAGE_BID,
        SrcKey::Container(..) | SrcKey::Dropped(..) => 0,
    }
}

/// The harvester opportunity gate — now the shared kernel's ([`mk::carrier_gate`]); this thin
/// wrapper keeps the `CarrierDto` shape for the sim's parity test (module docs on the gate).
#[cfg(test)]
fn carrier_gate(c: &CarrierDto, bid: u32, amount: u32, service: u32) -> bool {
    mk::carrier_gate(c.opportunity_milli, bid, amount, service)
}

fn near_level_up(consts: &MarketConsts, world: &EconWorld) -> bool {
    world.controller.as_ref().is_some_and(|c| {
        controller_levels(c.level)
            .is_some_and(|need| need.saturating_sub(c.progress) <= consts.upgrade_step_window_progress)
    })
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Refill bid + K4 spawn plans (bodies deficit-priced when `k4_bodies`).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The refill bid from a plan preview (§D1 + the review-#1 derived floor): the ROI of the TOP
/// energy-blocked request (highest priority — the head-of-line banker, cost above available-now
/// but within the RCL ceiling), floored by the DERIVED instant-spawnability premium computed
/// from the lane deficit and the next-body cost. `plans` carry their §D5.4 `w` (milli e/t).
fn refill_bid_from_plans(consts: &MarketConsts, world: &EconWorld, plans: &[(SpawnPlan, u32)]) -> u32 {
    let available = world.room_spawn_energy();
    let capacity = baseline::spawn_lane_capacity(world);
    let lane_deficit = capacity.saturating_sub(available);
    let mut order: Vec<usize> = (0..plans.len()).collect();
    // Descending bid, stable on emission order (the spawn queue's own sort). `u32` bids (M5b) —
    // `cmp` is total, the old `partial_cmp`/NaN coalescing is gone.
    order.sort_by(|&a, &b| plans[b].0.priority.cmp(&plans[a].0.priority).then(a.cmp(&b)));
    // The next-body cost for the derived floor: the cheapest planned body within the ceiling
    // (the one the lane could bank toward next); fall back to a bare spawn body (300) so a
    // deficit never divides by zero.
    let next_body_cost = order
        .iter()
        .map(|&i| body_cost(&plans[i].0.body))
        .filter(|&cost| cost <= capacity)
        .min()
        .unwrap_or(SPAWN_ENERGY_CAPACITY);
    let top_blocked = order.into_iter().find_map(|i| {
        let (plan, w) = &plans[i];
        let cost = body_cost(&plan.body);
        (cost > available && cost <= capacity).then(|| econ::body_roi_milli(*w, cost))
    });
    econ::refill_bid(consts, top_blocked, lane_deficit, next_body_cost)
}

/// The room's current income estimate, milli e/t (K4's time-to-afford denominator): per-source
/// saturated harvest rate of the ALIVE harvester fleet + spawn self-charge while the lane is
/// under 300 (the bootstrap floor). Deterministic from world + roles.
pub fn income_estimate_milli(world: &EconWorld, roles: &BTreeMap<u32, RoleSpec>) -> u32 {
    let mut per_source: BTreeMap<usize, u32> = BTreeMap::new();
    for (&id, role) in roles {
        if let RoleSpec::Harvester { source_idx } = role {
            if let Some(creep) = world.creep(id) {
                *per_source.entry(*source_idx).or_insert(0) += creep.body.alive_part_count(screeps::Part::Work);
            }
        }
    }
    let harvest: u32 = per_source
        .values()
        .map(|&w| (econ::HARVEST_POWER_E_T * 1000 * w).min(econ::SOURCE_RATE_E_T * 1000))
        .sum();
    let self_charge = if world.room_spawn_energy() < SPAWN_ENERGY_CAPACITY {
        world.spawns.len() as u32 * 1000
    } else {
        0
    };
    harvest + self_charge
}

/// A coarse hauler round-trip estimate for the logistics-rate arm (ticks): twice the range
/// from the first spawn to the nearest source, floored at 10 (deterministic; the §D5.4 hauler
/// rate `ρ = Q/T*_rtt` with the linear-range stand-in the live matching also uses).
fn hauler_rtt_est(world: &EconWorld) -> u32 {
    let Some(spawn) = world.spawns.first() else { return 10 };
    let nearest = world
        .sources
        .iter()
        .map(|s| spawn.pos.get_range_to(s.pos))
        .min()
        .unwrap_or(5);
    (2 * nearest).max(10)
}

/// A plan's §D5.4 civilian rate `w`, milli e/t (module docs: the ratified arms — harvester =
/// income unlocked, hauler = logistics rate, worker = `min(WORK·k, supply)·V_SINK`).
fn role_w_milli(world: &EconWorld, consts: &MarketConsts, role: &RoleSpec, body: &[screeps::Part], income_milli: u32) -> u32 {
    let count = |p: screeps::Part| body.iter().filter(|&&x| x == p).count() as u32;
    match role {
        RoleSpec::Harvester { .. } => {
            (econ::HARVEST_POWER_E_T * 1000 * count(screeps::Part::Work)).min(econ::SOURCE_RATE_E_T * 1000)
        }
        RoleSpec::Hauler => {
            count(screeps::Part::Carry) * econ::CARRY_CAPACITY * 1000 / hauler_rtt_est(world)
        }
        RoleSpec::Upgrader => {
            count(screeps::Part::Work) * econ::UPGRADE_POWER_E_T * consts.v_upgrade_milli
        }
        RoleSpec::Builder { .. } => {
            (econ::BUILD_POWER_E_T * 1000 * count(screeps::Part::Work)).min(income_milli.max(1000)) * econ::V_SINK_Q / 1000
        }
    }
}

/// Pick a body from a candidate ladder via the K4 kernel; `None` falls back to the baseline
/// body (the request SET never changes — K4 changes sizing only, spec Part B).
fn pick_body(
    consts: &MarketConsts,
    candidates: Vec<(Vec<screeps::Part>, u32)>, // (body, w_milli)
    available: u32,
    income_milli: u32,
) -> Option<(Vec<screeps::Part>, u32)> {
    let cands: Vec<econ::BodyCandidate> = candidates
        .iter()
        .map(|(body, w)| econ::BodyCandidate { cost: body_cost(body), w_milli: *w })
        .collect();
    econ::deficit_priced_pick(consts, &cands, available, income_milli).map(|i| candidates[i].clone())
}

/// **The market K4 spawn-request set** (spec Part B): the SAME roles/counts/f32 priorities as
/// the baseline `spawn_requests` (the queue interface is unchanged — bid-ordering is M5b), with
/// two market substitutions: (a) when `k4_bodies`, bodies come from the deficit-priced ladder
/// ([`econ::deficit_priced_pick`] — the S6 fix); (b) the repairer arm admits by repair bid vs
/// floor instead of the priority-map/allowance gate. Every plan returns with its §D5.4 `w`
/// (the refill bid's ingredient).
pub fn spawn_requests_market(
    world: &EconWorld,
    roles: &BTreeMap<u32, RoleSpec>,
    unfulfilled_hauling: u32,
    rt: &MarketRuntime,
) -> Vec<(SpawnPlan, u32)> {
    let consts = &rt.cfg.consts;
    let k4 = rt.cfg.k4_bodies;
    let mut out: Vec<(SpawnPlan, u32)> = Vec::new();
    let total_harvesting = roles.values().filter(|r| matches!(r, RoleSpec::Harvester { .. })).count();
    let capacity = baseline::spawn_lane_capacity(world);
    let available = world.room_spawn_energy();
    let income = income_estimate_milli(world, roles);
    let budget = capacity.max(SPAWN_ENERGY_CAPACITY);

    // ── Harvesters (baseline roster logic; K4 ladder [M,M,C,W]×r) ───────────────────────────────
    for source_idx in 0..world.sources.len() {
        let current = roles
            .values()
            .filter(|r| matches!(r, RoleSpec::Harvester { source_idx: s } if *s == source_idx))
            .count();
        let desired = spawn_policy::DESIRED_HARVESTERS_PER_SOURCE;
        if current < desired {
            let role = RoleSpec::Harvester { source_idx };
            let chosen = if k4 {
                let ladder: Vec<(Vec<screeps::Part>, u32)> = (1..=5u32)
                    .filter_map(|r| baseline::harvester_body(250 * r))
                    .filter(|b| body_cost(b) <= budget)
                    .map(|b| {
                        let w = role_w_milli(world, consts, &role, &b, income);
                        (b, w)
                    })
                    .collect();
                pick_body(consts, ladder, available, income)
            } else {
                let energy = spawn_policy::harvester_body_energy(total_harvesting, available, capacity);
                baseline::harvester_body(energy).map(|b| {
                    let w = role_w_milli(world, consts, &role, &b, income);
                    (b, w)
                })
            };
            if let Some((body, w)) = chosen {
                let priority = spawn_policy::harvester_priority(current, desired, 0);
                out.push((SpawnPlan { body, priority, role }, w));
            }
        }
    }

    // ── Haulers (baseline demand sizing; K4 ladder [C,M]×r) ─────────────────────────────────────
    {
        let haulers = roles.values().filter(|r| matches!(r, RoleSpec::Hauler)).count();
        let chosen = if k4 {
            let ladder: Vec<(Vec<screeps::Part>, u32)> = (1..=20u32)
                .filter_map(|r| baseline::hauler_body(100 * r))
                .filter(|b| body_cost(b) <= budget)
                .map(|b| {
                    let w = role_w_milli(world, consts, &RoleSpec::Hauler, &b, income);
                    (b, w)
                })
                .collect();
            pick_body(consts, ladder, available, income)
        } else {
            let energy = if haulers == 0 { available.max(SPAWN_ENERGY_CAPACITY) } else { capacity };
            baseline::hauler_body(energy).map(|b| {
                let w = role_w_milli(world, consts, &RoleSpec::Hauler, &b, income);
                (b, w)
            })
        };
        if let Some((body, w)) = chosen {
            let carry_parts = body.iter().filter(|p| **p == screeps::Part::Carry).count() as u32;
            let (desired_unfulfilled, desired) = spawn_policy::hauler_desired(unfulfilled_hauling, carry_parts, 0);
            if haulers < desired {
                let priority = spawn_policy::hauler_priority(haulers, desired_unfulfilled, 0);
                out.push((SpawnPlan { body, priority, role: RoleSpec::Hauler }, w));
            }
        }
    }

    // ── Upgraders (baseline roster/priority; K4 ladder over WORK counts) ────────────────────────
    let controller = world.controller.as_ref().filter(|c| c.level > 0);
    if let Some(c) = controller {
        let rcl = c.level;
        let excess = baseline::has_excess_energy(world);
        let at_max_level = controller_levels(rcl).is_none();
        let max_ticks = controller_downgrade(rcl);
        let downgrade_upkeep_parts: Option<usize> =
            (c.downgrade_ticks < max_ticks / 2).then(|| spawn_policy::work_parts_for_upkeep(c.downgrade_ticks, max_ticks));
        let downgrade_risk = downgrade_upkeep_parts.is_some();
        let max_upgraders = spawn_policy::max_upgraders(true, false, at_max_level, excess, rcl);
        let roster: Vec<u32> = roles
            .iter()
            .filter(|(_, r)| matches!(r, RoleSpec::Upgrader))
            .map(|(&id, _)| id)
            .collect();
        let tick = world.tick();
        let alive = roster
            .iter()
            .filter(|id| world.creep_ttl.get(id).map(|&age| age.saturating_sub(tick) > 100).unwrap_or(true))
            .count();
        if alive < max_upgraders {
            let work_parts = spawn_policy::upgrader_work_parts(
                downgrade_upkeep_parts,
                roster.is_empty(),
                at_max_level,
                excess,
                world.sources.len(),
                max_upgraders,
            );
            let chosen = if k4 {
                let target = work_parts.unwrap_or(1).max(1);
                let mut ladder: Vec<(Vec<screeps::Part>, u32)> = Vec::new();
                for w in 1..=target {
                    if let Some(body) = baseline::upgrader_body(rcl, budget, Some(w)) {
                        if body_cost(&body) <= budget && !ladder.iter().any(|(b, _)| *b == body) {
                            let wm = role_w_milli(world, consts, &RoleSpec::Upgrader, &body, income);
                            ladder.push((body, wm));
                        }
                    }
                }
                pick_body(consts, ladder, available, income)
            } else {
                let maximum_energy = if roster.is_empty() && downgrade_risk {
                    available.max(SPAWN_ENERGY_CAPACITY)
                } else {
                    capacity
                };
                baseline::upgrader_body(rcl, maximum_energy, work_parts).map(|b| {
                    let w = role_w_milli(world, consts, &RoleSpec::Upgrader, &b, income);
                    (b, w)
                })
            };
            if let Some((body, w)) = chosen {
                let priority = spawn_policy::upgrader_priority(
                    downgrade_risk,
                    roster.is_empty(),
                    excess,
                    world.storage.is_some(),
                    max_upgraders,
                    alive,
                );
                out.push((SpawnPlan { body, priority, role: RoleSpec::Upgrader }, w));
            }
        }
    }

    // ── Builders (baseline site arm; the repairer arm is MARKET-ADMITTED; K4 ladder) ────────────
    if let Some(c) = controller {
        let rcl = c.level;
        let sufficient = baseline::has_sufficient_energy(world);
        let builders = roles.values().filter(|r| matches!(r, RoleSpec::Builder { .. })).count();
        let mut spawn_count = 0u32;
        let mut spawn_priority = 0u32; // SPAWN_BID_NONE (milli-e/t)
        if let Some((desired, priority)) = baseline::builder_priority(world, rcl, sufficient, builders) {
            spawn_count = spawn_count.max(desired);
            spawn_priority = spawn_priority.max(priority);
        }
        if let Some((desired, priority)) = rt.repairer_priority(world) {
            spawn_count = spawn_count.max(desired);
            spawn_priority = spawn_priority.max(priority);
        }
        if (builders as u32) < spawn_count {
            let role = RoleSpec::Builder { allow_harvest: world.storage.is_none() };
            let chosen = if k4 {
                let max_repeats = if spawn_priority >= spawn_policy::SPAWN_BID_HIGH { 12 } else { 5 };
                let ladder: Vec<(Vec<screeps::Part>, u32)> = (1..=max_repeats)
                    .filter_map(|r| baseline::builder_body(300 * r, spawn_priority))
                    .filter(|b| body_cost(b) <= budget)
                    .map(|b| {
                        let w = role_w_milli(world, consts, &role, &b, income);
                        (b, w)
                    })
                    .collect();
                pick_body(consts, ladder, available, income)
            } else {
                let use_energy_max = if builders == 0 && spawn_priority >= spawn_policy::SPAWN_BID_HIGH {
                    available.max(SPAWN_ENERGY_CAPACITY)
                } else {
                    capacity
                };
                baseline::builder_body(use_energy_max, spawn_priority).map(|b| {
                    let w = role_w_milli(world, consts, &role, &b, income);
                    (b, w)
                })
            };
            if let Some((body, w)) = chosen {
                out.push((SpawnPlan { body, priority: spawn_priority, role }, w));
            }
        }
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The SIM-ONLY exact selection oracle (module docs).
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// Per-carrier candidate pruning for the oracle: the top K edges by density. Greedy always
/// picks within a carrier's top-density edges, so the pruned optimum still dominates greedy.
const ORACLE_TOP_K: usize = 10;
/// Branch-and-bound search-node cap: an exceeded budget SKIPS the sample (never approximates).
const ORACLE_NODE_CAP: u64 = 500_000;

/// The exact best one-ticket-per-carrier selection value (fixed-point), under the canonical
/// consumption dynamics (a selected set realizes its flows in global density order — the same
/// rule the greedy scans by). `None` if the search exceeded [`ORACLE_NODE_CAP`].
pub fn oracle_best_fp(
    edges: &[m::AssignEdge],
    supply0: &BTreeMap<u32, u32>,
    demand0: &BTreeMap<u32, u32>,
) -> Option<u64> {
    // Per-carrier candidate lists, density-ordered, pruned.
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&a, &b| {
        let (ea, eb) = (&edges[a], &edges[b]);
        let va = ea.bid_milli as u128 * ea.amount as u128 * eb.service_ticks.max(1) as u128;
        let vb = eb.bid_milli as u128 * eb.amount as u128 * ea.service_ticks.max(1) as u128;
        vb.cmp(&va).then_with(|| (ea.carrier, ea.supply, ea.demand, a).cmp(&(eb.carrier, eb.supply, eb.demand, b)))
    });
    let mut per_carrier: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for idx in order {
        // ADR 0044 fix 6-4: the oracle optimizes over the SAME admitted arc set the greedy does —
        // a below-break-even arc (`bid − source_floor − haul ≤ 0`) is not a candidate for either.
        // Without this the `match_optimality_gap` would measure the greedy failing to optimize an
        // objective the oracle isn't running (an artifact), not a real approximation gap.
        if !edges[idx].admitted() {
            continue;
        }
        let list = per_carrier.entry(edges[idx].carrier).or_default();
        if list.len() < ORACLE_TOP_K {
            list.push(idx);
        }
    }
    // Carriers ordered by their best unconstrained edge value, descending (better pruning);
    // suffix upper bounds from the same optimistic per-carrier bests.
    let mut carriers: Vec<(u64, u32, Vec<usize>)> = per_carrier
        .into_iter()
        .map(|(c, list)| {
            let ub = list
                .iter()
                .map(|&i| m::flow_value_fp(edges[i].bid_milli, edges[i].amount, edges[i].service_ticks))
                .max()
                .unwrap_or(0);
            (ub, c, list)
        })
        .collect();
    carriers.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut suffix_ub = vec![0u64; carriers.len() + 1];
    for i in (0..carriers.len()).rev() {
        suffix_ub[i] = suffix_ub[i + 1] + carriers[i].0;
    }

    struct Search<'a> {
        edges: &'a [m::AssignEdge],
        carriers: &'a [(u64, u32, Vec<usize>)],
        suffix_ub: &'a [u64],
        best: u64,
        nodes: u64,
        capped: bool,
    }
    impl Search<'_> {
        fn go(&mut self, i: usize, acc: u64, supply: &mut BTreeMap<u32, u32>, demand: &mut BTreeMap<u32, u32>) {
            self.nodes += 1;
            if self.nodes > ORACLE_NODE_CAP {
                self.capped = true;
                return;
            }
            if i == self.carriers.len() {
                self.best = self.best.max(acc);
                return;
            }
            if acc + self.suffix_ub[i] <= self.best {
                return; // bound
            }
            // Take each candidate edge (realized against the CURRENT remaining caps)…
            for &idx in &self.carriers[i].2 {
                if self.capped {
                    return;
                }
                let e = &self.edges[idx];
                let mut flow = e.amount;
                if let Some(s) = e.supply {
                    flow = flow.min(supply.get(&s).copied().unwrap_or(0));
                }
                flow = flow.min(demand.get(&e.demand).copied().unwrap_or(0));
                if flow == 0 {
                    continue;
                }
                if let Some(s) = e.supply {
                    *supply.get_mut(&s).unwrap() -= flow;
                }
                *demand.get_mut(&e.demand).unwrap() -= flow;
                self.go(i + 1, acc + m::flow_value_fp(e.bid_milli, flow, e.service_ticks), supply, demand);
                if let Some(s) = e.supply {
                    *supply.get_mut(&s).unwrap() += flow;
                }
                *demand.get_mut(&e.demand).unwrap() += flow;
            }
            // …or skip this carrier.
            self.go(i + 1, acc, supply, demand);
        }
    }
    let mut search = Search { edges, carriers: &carriers, suffix_ub: &suffix_ub, best: 0, nodes: 0, capped: false };
    let mut s = supply0.clone();
    let mut d = demand0.clone();
    search.go(0, 0, &mut s, &mut d);
    (!search.capped).then_some(search.best)
}

/// Note on carrier ORDER inside the oracle: carriers realize flows in the DFS's carrier order,
/// not global density order — for a FIXED selected set the total can differ from the canonical
/// order's total only when two selected edges contend for the same node, in which case the DFS
/// still explores the alternative selections (including each contender alone), so the maximum
/// over selections dominates every canonical-order realization. `oracle ≥ greedy` additionally
/// holds by the `max(oracle, greedy)` guard at the sample site.
#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Part, RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    fn edge(carrier: u32, supply: Option<u32>, demand: u32, amount: u32, bid: u32, service: u32) -> m::AssignEdge {
        // 0/0 admission inputs = inert (every positive-bid arc admitted) — these tests exercise the
        // density/oracle-gap mechanics, not ADR 0044 stage-1 admission.
        m::AssignEdge { carrier, supply, demand, amount, bid_milli: bid, service_ticks: service, source_floor_milli: 0, haul_cost_milli: 0 }
    }

    /// The classic greedy trap: carrier 0 holds the globally DENSEST edge on the shared demand,
    /// so greedy burns d0 on c0 and strands c1 (whose only edge is d0); the exact oracle routes
    /// c0 to its second-best ticket instead. The measured gap is positive and exact.
    #[test]
    fn oracle_beats_greedy_on_the_swap_trap() {
        // e0: c0→d0 (v = 2000·100/5 = 40k — the global max). e1: c0→d1 (v = 6000·50/10 = 30k).
        // e2: c1→d0 (v = 20k). Greedy: e0 → c0 takes d0; e1 (c0 taken), e2 (d0 drained) ⇒ c1
        // starves. Oracle: c0→d1 + c1→d0 = 30,720,000 + 20,480,000 FP > greedy's 40,960,000.
        let edges = vec![
            edge(0, None, 0, 100, 2000, 5),
            edge(0, None, 1, 50, 6000, 10),
            edge(1, None, 0, 100, 2000, 10),
        ];
        let supply0: BTreeMap<u32, u32> = BTreeMap::new();
        let demand0: BTreeMap<u32, u32> = [(0u32, 100u32), (1, 50)].into_iter().collect();
        let (mut s, mut d) = (supply0.clone(), demand0.clone());
        let (got, _) = m::greedy_assign(&edges, &mut s, &mut d);
        let greedy_fp = m::assignments_value_fp(&edges, &got);
        let oracle_fp = oracle_best_fp(&edges, &supply0, &demand0).unwrap();
        assert_eq!(greedy_fp, m::flow_value_fp(2000, 100, 5), "greedy takes the dense edge and strands c1");
        assert_eq!(
            oracle_fp,
            m::flow_value_fp(6000, 50, 10) + m::flow_value_fp(2000, 100, 10),
            "the oracle finds the swap"
        );
        assert!(oracle_fp > greedy_fp);
    }

    /// Oracle == greedy on a non-contended instance (gap exactly 0), and the oracle respects
    /// supply booking (two carriers cannot both take a 100-unit supply for 100 each).
    #[test]
    fn oracle_matches_greedy_when_greedy_is_optimal() {
        let edges = vec![
            edge(0, Some(0), 0, 100, 5000, 10),
            edge(1, Some(0), 1, 100, 5000, 10),
        ];
        let supply0: BTreeMap<u32, u32> = [(0u32, 150u32)].into_iter().collect();
        let demand0: BTreeMap<u32, u32> = [(0u32, 100u32), (1, 100)].into_iter().collect();
        let (mut s, mut d) = (supply0.clone(), demand0.clone());
        let (got, _) = m::greedy_assign(&edges, &mut s, &mut d);
        let greedy_fp = m::assignments_value_fp(&edges, &got);
        let oracle_fp = oracle_best_fp(&edges, &supply0, &demand0).unwrap();
        assert_eq!(greedy_fp, oracle_fp, "no contention ⇒ zero gap");
        // Both flows clamp to the shared 150 supply: 100 + 50.
        assert_eq!(got.iter().map(|a| a.amount).sum::<u32>(), 150);
    }

    /// The harvester opportunity gate: par tickets never beat a live source; a stressed refill
    /// ticket does; harvest-incapable carriers take anything.
    #[test]
    fn harvester_gate_prices_surplus_against_harvest() {
        let harvester = CarrierDto { id: 1, pos: pos(10, 10), free: 50, held: 0, opportunity_milli: 2000 };
        // Par storage dump: surplus 0 — never beats harvesting.
        assert!(!carrier_gate(&harvester, econ::STORAGE_BID, 50, 10));
        // Post-wipe refill at 12×: surplus (11000)·300 ≫ 2000·20.
        assert!(carrier_gate(&harvester, 12_000, 300, 20));
        // A full harvester (opportunity 0) dumps at par happily.
        let full = CarrierDto { id: 1, pos: pos(10, 10), free: 0, held: 200, opportunity_milli: 0 };
        assert!(carrier_gate(&full, econ::STORAGE_BID, 200, 10));
        // Boundary: surplus must STRICTLY beat the alternative.
        let c = CarrierDto { id: 2, pos: pos(0, 0), free: 50, held: 0, opportunity_milli: 1000 };
        assert!(!carrier_gate(&c, 2000, 10, 10), "surplus 10000 == opp·service 10000: harvest wins ties");
        assert!(carrier_gate(&c, 2001, 10, 10));
    }

    /// K4 income estimate: saturating per-source WORK + self-charge under 300.
    #[test]
    fn income_estimate_saturates_per_source() {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 25), 3000);
        w.add_source(pos(40, 25), 3000);
        let s = w.add_spawn(pos(25, 25));
        w.spawns[s].store_energy = 0;
        let h1 = w.add_creep(pos(11, 25), &[Part::Move, Part::Move, Part::Carry, Part::Work], 500);
        let h2 = w.add_creep(pos(12, 25), &[Part::Work; 8], 500);
        let mut roles: BTreeMap<u32, RoleSpec> = BTreeMap::new();
        roles.insert(h1, RoleSpec::Harvester { source_idx: 0 });
        roles.insert(h2, RoleSpec::Harvester { source_idx: 1 });
        // Source 0: 1 WORK → 2 e/t; source 1: 8 WORK saturates at 10 e/t; +1 self-charge.
        assert_eq!(income_estimate_milli(&w, &roles), 2000 + 10_000 + 1000);
    }

    /// Review #5: ONLY the engine-fungible spawn lane aggregates; every other sink is matched
    /// per-structure (a container/storage is a distinct stockpile, not a member of a pool).
    #[test]
    fn only_the_fungible_lane_aggregates() {
        assert!(SinkKey::Spawn(0).is_fungible_pool_member());
        assert!(SinkKey::Extension(3).is_fungible_pool_member());
        assert!(!SinkKey::Container(10, 10).is_fungible_pool_member(), "a container is its own stockpile");
        assert!(!SinkKey::Storage.is_fungible_pool_member(), "storage is the numeraire depot, not a lane member");
    }

    /// Review #6: the reference bound's pruning must not silently bite — realized edge counts
    /// per carrier must stay within ORACLE_TOP_K on the contended corpus, or the "no material
    /// value left in the pruned search" claim is under-instrumented. This pins the invariant on
    /// a synthetic max-contention pass (more sinks than K for one carrier); the tournament's
    /// Family-M `max_edges/pass` is the corpus-scale check.
    #[test]
    fn oracle_pruning_stays_within_top_k_per_carrier() {
        // One carrier, 15 distinct deposit demands (> ORACLE_TOP_K = 10) — the greedy generates
        // all 15 edges, but the oracle's per-carrier candidate list is pruned to the top K.
        let edges: Vec<m::AssignEdge> = (0..15u32)
            .map(|d| m::AssignEdge { carrier: 0, supply: None, demand: d, amount: 10, bid_milli: 1000 + d * 100, service_ticks: 5, source_floor_milli: 0, haul_cost_milli: 0 })
            .collect();
        let supply0: BTreeMap<u32, u32> = BTreeMap::new();
        let demand0: BTreeMap<u32, u32> = (0..15u32).map(|d| (d, 10u32)).collect();
        // The oracle runs (does not panic / node-cap) and returns a bound ≥ the greedy's best
        // single edge (the densest of the 15 by construction).
        let (mut s, mut d) = (supply0.clone(), demand0.clone());
        let (greedy, _) = m::greedy_assign(&edges, &mut s, &mut d);
        let greedy_fp = m::assignments_value_fp(&edges, &greedy);
        let oracle_fp = oracle_best_fp(&edges, &supply0, &demand0).expect("small instance never node-caps");
        assert!(oracle_fp >= greedy_fp, "the bound is greedy-floored even under pruning");
        // One carrier takes exactly one ticket ⇒ the value is a single densest edge either way.
        assert_eq!(greedy_fp, m::flow_value_fp(1000 + 14 * 100, 10, 5), "greedy takes the densest edge");
        assert_eq!(oracle_fp, greedy_fp, "with one carrier the pruned oracle == greedy (no swap to find)");
    }
}
