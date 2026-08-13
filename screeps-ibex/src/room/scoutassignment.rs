//! ADR 0046 — the scout assignment post-pass: multi-room tours, room-centric
//! unreachable evidence, and EV-driven fleet sizing.
//!
//! One post-process system owns ALL scout fulfillment. It runs AFTER every
//! producer (operations, missions, squad manager — so it sees all same-tick
//! demand) and BEFORE `RunJobSystem` (so scouts walk fresh tours the same
//! tick). Scout jobs are pure tour-walkers ([`crate::jobs::scout::ScoutJob`]);
//! the per-room `ScoutMission` and the per-creep claim layer are gone.
//!
//! Design-review resolutions (2026-08-12) implemented here:
//! - **#1 insertion metric**: every cheapest-insertion delta is priced with
//!   Chebyshev room distance × 50 ticks/hop
//!   ([`crate::room::visibilitysystem::room_distance`]) — NEVER
//!   `find_route`. The route cache is consulted only for the ≤fleet-size
//!   chosen tour HEAD legs per tick (bounded, pool-friendly), and only as a
//!   map-disconnect bonus signal. Each entry's best insertion is memoized
//!   between greedy iterations ([`build_tours`]).
//! - **#2 unreachable evidence**: ROOM-centric — per demand room, count
//!   consecutive passes during which SOME assigned scout sat adjacent but
//!   outside while the room was its tour head; [`SCOUT_ENTRY_FAIL_TICKS`]
//!   (~100) marks it unreachable via the existing `VisibilityQueue` backoff.
//!   The counter resets when the room leaves demand or any scout enters.
//!   Rover `MovementFailure::PathNotFound` is NOT evidence (it is overloaded
//!   with CPU/budget exhaustion).
//! - **#4 stability**: the staleness value-multiplier is quantized into 0.25
//!   buckets so the greedy's primary key is piecewise-constant, and the
//!   empty-tour fallback leg targets the NEAREST qualifying unserviced entry
//!   (never the globally largest — argmax ping-pong).
//! - **#6 spawn EV**: computed and bid HERE (same tick: this system runs
//!   after all producers, `SpawnQueueSystem` consumes later in the tick).
//!   Closed form in e/t; only externally-produced, NON-opportunistic demand
//!   counts; gate at `CpuBar::MediumPriority`.
//! - **#7 observer throughput**: before tours and EV, projected observer
//!   coverage is subtracted — per observer, the top-N in-range demand entries
//!   it can freshen by rotation are dropped from the scout demand set.
//! - **#8 shed class**: the system is `StageClass::SkipUnderCritical`, but
//!   [`ScoutAssignments`] is NEVER cleared at tick start — the pass overwrites
//!   only when it runs; on skipped ticks scouts keep walking persisted tours.

use crate::cpugovernor::GovernorSnapshot;
use crate::creep::{CreepOwner, CreepSpawning};
use crate::entitymappingsystem::EntityMappingData;
use crate::jobs::data::JobData;
use crate::missions::constants::CpuBar;
use crate::operations::data::OperationData;
use crate::operations::scout::ScoutOperation;
use crate::pathing::pathfinderservice::PathfinderService;
use crate::room::data::RoomData;
use crate::room::visibilitysystem::*;
use crate::spawnsystem::{SpawnQueue, SpawnRequest};
use log::*;
use screeps::*;
use specs::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Consecutive assignment passes a scout may sit adjacent-but-outside a tour
/// head before the room is marked unreachable (design-review resolution #2).
pub const SCOUT_ENTRY_FAIL_TICKS: u32 = 100;

/// Travel-tick estimate per room hop (one room crossing ≈ 50 tiles).
const TICKS_PER_ROOM_HOP: u32 = 50;

/// A scout body is a single MOVE part.
const SCOUT_BODY_COST_E: f32 = 50.0;

/// Chebyshev reach (rooms) from a spawn-capable home within which unserviced
/// demand counts as reachable for the spawn-EV `reachable_share` term. Matches
/// the war/offense consumer reach (BFS hops <= 10; Chebyshev is its
/// over-approximation — see the pre-ADR-0046 `scout_reach_for_priority`).
const SCOUT_STRATEGIC_REACH: u32 = 10;

/// Demand entries one scout is projected to service across its lifetime — the
/// ADR 0046 D5 "entries the extra scout would service within its 1500-tick
/// life" horizon (1500 ticks ÷ ~2 hops × 50 ticks per stop ≈ 15, rounded).
/// Used both to discount pending (still-spawning) scouts from the unserviced
/// set and to cap the marginal scout's own serviceable value.
const SCOUT_EV_PROJECTION_ENTRIES: usize = 16;

/// BFS radius (rooms) for the never-seen-frontier fallback leg (absorbed from
/// the deleted job-side `pick_adjacent_explore_target`).
const FRONTIER_SEARCH_RADIUS: u32 = 5;

/// Intel age at or below which an unreachable-backoff record is cleared (the
/// "fresh sighting clears the give-up" rule, previously in `ScoutMission`).
const UNREACHABLE_FRESH_CLEAR_TICKS: u32 = 10;

/// Value floor (ticks) for the `value_e = rate × want_fresh_within` entry
/// value convention: an imperative `want_fresh_within = 0` entry (operator
/// scout flag) must not price at zero, so the convention floors the window at
/// one default TTL.
const ENTRY_VALUE_FLOOR_TICKS: u32 = DEFAULT_VISIBILITY_TTL;

// ─── Persistent (cross-tick, NOT serialized) assignment state ────────────────

/// The scout fleet's assignment state. A specs Resource that is NEVER cleared
/// at tick start (design-review resolution #8): the assignment pass overwrites
/// it only when it runs, so under a Critical shed scouts keep walking their
/// persisted tours and jobs tolerate tour entries whose demand has vanished —
/// freshness self-heals on the next pass. Ephemeral across VM resets (tours
/// are derived state; nothing here is serialized).
#[derive(Default)]
pub struct ScoutAssignments {
    /// Ordered multi-room tour per scout creep entity. The job walks the
    /// first entry that is not its current room.
    pub tours: HashMap<Entity, Vec<RoomName>>,
    /// Room-centric entry-failure evidence: consecutive passes some assigned
    /// scout sat adjacent-but-outside the room while it was a tour head.
    pub entry_fail: HashMap<RoomName, u32>,
    /// Last tick each room was serviced by an observer — drives the
    /// least-recently-observed-first rotation in `ObserverSystem` (F6 fix).
    pub last_observed: HashMap<RoomName, u32>,
}

// ─── Pure kernels (host-testable policy) ─────────────────────────────────────

/// Priority tier → intel value rate in energy-equivalent per tick (ADR 0046
/// §4a seeds, ratified by the design review; tune in soak).
pub fn tier_rate_et(priority: f32) -> f32 {
    if priority >= VISIBILITY_PRIORITY_CRITICAL {
        5.0
    } else if priority >= VISIBILITY_PRIORITY_HIGH {
        2.0
    } else if priority >= VISIBILITY_PRIORITY_MEDIUM {
        0.75
    } else {
        0.1
    }
}

/// Staleness value-multiplier: `age / want_fresh_within`, clamped to [1, 3],
/// then quantized DOWN into 0.25 buckets so the greedy's primary key is
/// piecewise-constant across ticks (design-review resolution #4 — a smoothly
/// rising multiplier would re-order the greedy every tick and thrash tours).
pub fn quantized_staleness_multiplier(age: u32, want_fresh_within: u32) -> f32 {
    let window = want_fresh_within.max(1) as f32;
    let ratio = (age as f32 / window).clamp(1.0, 3.0);
    (ratio * 4.0).floor() / 4.0
}

/// One demand entry as the tour builder sees it.
#[derive(Debug, Clone)]
pub struct TourDemand {
    pub room: RoomName,
    /// Quantized entry value (energy): `rate × max(want_fresh_within, floor)
    /// × quantized staleness multiplier`.
    pub value_e: f32,
}

/// One live scout as the tour builder sees it.
#[derive(Debug, Clone)]
pub struct TourScout {
    /// Deterministic tie-break key (the ECS entity index).
    pub key: u32,
    pub room: RoomName,
    pub ticks_to_live: u32,
}

/// The tour builder's output.
#[derive(Debug)]
pub struct TourBuild {
    /// One ordered tour per input scout (parallel to the input slice).
    pub tours: Vec<Vec<RoomName>>,
    /// Insertion-delta evaluations performed — memoization telemetry, pinned
    /// by `memoized_insertion_matches_naive_and_bounds_evaluations`.
    pub delta_evaluations: usize,
}

/// Best cheapest-insertion of `room` into `tour` for a scout starting at
/// `scout_room`: returns `(delta_ticks, position)` minimizing the added
/// travel, or `None` if no position fits within the remaining budget.
/// All legs are priced Chebyshev × [`TICKS_PER_ROOM_HOP`] (resolution #1).
fn best_insertion(scout_room: RoomName, tour: &[RoomName], room: RoomName, budget_ticks: u32) -> Option<(u32, usize)> {
    let leg = |a: RoomName, b: RoomName| room_distance(a, b) * TICKS_PER_ROOM_HOP;

    let mut best: Option<(u32, usize)> = None;
    for pos in 0..=tour.len() {
        let prev = if pos == 0 { scout_room } else { tour[pos - 1] };
        let delta = if pos == tour.len() {
            // Append: one new leg, no return.
            leg(prev, room)
        } else {
            leg(prev, room) + leg(room, tour[pos]) - leg(prev, tour[pos])
        };
        if delta > budget_ticks {
            continue;
        }
        // Strict < keeps the earliest position on ties (deterministic).
        if best.map(|(d, _)| delta < d).unwrap_or(true) {
            best = Some((delta, pos));
        }
    }
    best
}

/// Total travel ticks of a tour from the scout's current room (Chebyshev legs).
fn tour_ticks(scout_room: RoomName, tour: &[RoomName]) -> u32 {
    let mut prev = scout_room;
    let mut total = 0u32;
    for stop in tour {
        total += room_distance(prev, *stop) * TICKS_PER_ROOM_HOP;
        prev = *stop;
    }
    total
}

/// Greedy cheapest-insertion tour construction (ADR 0046 D2.3).
///
/// Repeatedly takes the unassigned entry with the best
/// `value / marginal_travel_ticks` over any tour, respecting each scout's
/// remaining lifetime. Deterministic tie-breaks: higher value, then room name,
/// then scout key. Memoization (resolution #1): each entry caches its
/// per-scout best insertion; after an insertion into scout S only S's column
/// is re-evaluated (S's tour is the only one that changed), and entries whose
/// cached overall best was S re-derive it from the cached columns — no
/// quadratic full recompute per iteration.
pub fn build_tours(scouts: &[TourScout], demand: &[TourDemand]) -> TourBuild {
    let mut tours: Vec<Vec<RoomName>> = vec![Vec::new(); scouts.len()];
    let mut used_ticks: Vec<u32> = vec![0; scouts.len()];
    let mut evaluations = 0usize;

    if scouts.is_empty() || demand.is_empty() {
        return TourBuild {
            tours,
            delta_evaluations: 0,
        };
    }

    // Per remaining entry: per-scout best (delta, pos), plus the overall best
    // scout index. `None` = infeasible for that scout right now.
    struct EntryState {
        demand_idx: usize,
        per_scout: Vec<Option<(u32, usize)>>,
        best_scout: Option<usize>,
    }

    let eval_column = |entry_room: RoomName, s: usize, tours: &[Vec<RoomName>], used: &[u32], evals: &mut usize| {
        *evals += 1;
        let scout = &scouts[s];
        let budget = scout.ticks_to_live.saturating_sub(used[s]);
        best_insertion(scout.room, &tours[s], entry_room, budget)
    };

    let rederive_best = |st: &mut EntryState| {
        st.best_scout = st
            .per_scout
            .iter()
            .enumerate()
            .filter_map(|(s, d)| d.map(|(delta, _)| (s, delta)))
            .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
            .map(|(s, _)| s);
    };

    let mut remaining: Vec<EntryState> = demand
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let per_scout: Vec<Option<(u32, usize)>> = (0..scouts.len())
                .map(|s| eval_column(d.room, s, &tours, &used_ticks, &mut evaluations))
                .collect();
            let mut st = EntryState {
                demand_idx: i,
                per_scout,
                best_scout: None,
            };
            rederive_best(&mut st);
            st
        })
        .collect();

    loop {
        // Pick the feasible entry with the best value/delta score.
        // Tie-breaks: higher (quantized) value, then room name, then scout key.
        let mut best: Option<(usize, f32)> = None; // (remaining idx, score)
        for (ri, st) in remaining.iter().enumerate() {
            let Some(s) = st.best_scout else { continue };
            let (delta, _) = st.per_scout[s].expect("best_scout implies a cached delta");
            let d = &demand[st.demand_idx];
            let score = d.value_e / (delta.max(1) as f32);
            let better = match best {
                None => true,
                Some((bi, bscore)) => {
                    let b_st = &remaining[bi];
                    let b_d = &demand[b_st.demand_idx];
                    score > bscore
                        || (score == bscore
                            && (d.value_e > b_d.value_e
                                || (d.value_e == b_d.value_e
                                    && (d.room < b_d.room
                                        || (d.room == b_d.room && scouts[s].key < scouts[b_st.best_scout.unwrap()].key)))))
                }
            };
            if better {
                best = Some((ri, score));
            }
        }

        let Some((ri, _)) = best else { break };
        let st = remaining.swap_remove(ri);
        let s = st.best_scout.expect("picked entry has a best scout");
        let (delta, pos) = st.per_scout[s].expect("picked entry has a cached insertion");
        let room = demand[st.demand_idx].room;
        tours[s].insert(pos, room);
        used_ticks[s] += delta;

        // Memo maintenance: only scout S's tour changed — re-evaluate S's
        // column for every remaining entry; re-derive the overall best from
        // the cached columns (cheap, no distance work) where it could have
        // moved (previous best was S, or the fresh S column now wins).
        for entry in remaining.iter_mut() {
            entry.per_scout[s] = eval_column(demand[entry.demand_idx].room, s, &tours, &used_ticks, &mut evaluations);
            rederive_best(entry);
        }
    }

    TourBuild {
        tours,
        delta_evaluations: evaluations,
    }
}

/// Room-centric unreachable-entry evidence (design-review resolution #2).
///
/// `scouts` is `(current_room, tour_head)` per live scout. For each demand
/// room: any scout INSIDE it resets the counter; otherwise a scout tasked
/// with it (tour head) sitting Chebyshev-adjacent increments; no tasked-and-
/// adjacent scout resets (the count is CONSECUTIVE). Rooms that left demand
/// are dropped. Returns the rooms that just crossed
/// [`SCOUT_ENTRY_FAIL_TICKS`] (sorted, deterministic), with their counters
/// reset so the backoff owns the follow-up.
pub fn update_entry_fail_counters(
    counters: &mut HashMap<RoomName, u32>,
    demand_rooms: &HashSet<RoomName>,
    scouts: &[(RoomName, Option<RoomName>)],
) -> Vec<RoomName> {
    // Rooms no longer in demand lose their evidence (resolution #2: reset
    // when the room leaves demand).
    counters.retain(|room, _| demand_rooms.contains(room));

    let mut crossed = Vec::new();
    for room in demand_rooms {
        let entered = scouts.iter().any(|(current, _)| current == room);
        let adjacent_tasked = scouts
            .iter()
            .any(|(current, head)| *head == Some(*room) && room_distance(*current, *room) == 1);

        if entered {
            counters.remove(room);
        } else if adjacent_tasked {
            let c = counters.entry(*room).or_insert(0);
            *c += 1;
            if *c >= SCOUT_ENTRY_FAIL_TICKS {
                crossed.push(*room);
                counters.remove(room);
            }
        } else {
            // Not tasked / not adjacent this pass — the streak breaks.
            counters.remove(room);
        }
    }
    crossed.sort();
    crossed
}

/// Projected coverage of ONE observer over its in-range demand entries
/// (design-review resolution #7): entries the observer can keep fresh by
/// rotation, so scouts must not tour them. `in_range` is
/// `(room, value_e, want_fresh_within)` for the observer's in-range demand.
///
/// An observer services ~1 room per tick; rotating over k rooms re-freshens
/// each every k ticks, which sustains a room only while
/// `k <= want_fresh_within` (a `want_fresh_within ≈ 0-1` room needs dedicated
/// per-tick coverage and cannot share). Greedy: take entries by value (desc,
/// name-asc tie-break) while the rotation period stays within each taken
/// entry's freshness window.
pub fn observer_covered_rooms(in_range: &[(RoomName, f32, u32)]) -> Vec<RoomName> {
    let mut sorted: Vec<&(RoomName, f32, u32)> = in_range.iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

    // Growing the rotation to k rooms re-freshens each every k ticks, so every
    // taken room's window (and the candidate's) must be >= k.
    let mut covered = Vec::new();
    let mut min_window = u32::MAX;
    for (room, _value, wfw) in sorted {
        let window = (*wfw).max(1);
        let new_len = covered.len() as u32 + 1;
        if new_len <= min_window.min(window) {
            covered.push(*room);
            min_window = min_window.min(window);
        } else {
            break;
        }
    }
    covered
}

/// The scout-fleet marginal spawn EV in e/t (ADR 0046 D5, design-review
/// resolution #6 — THE closed form):
///
/// `EV = unserviced_demand_value × min(1, reachable_share) − body_amortization`
///
/// where `unserviced_demand_value` is the summed staleness-scaled rates (e/t)
/// of externally-produced, non-opportunistic demand left over after tours and
/// observer projection — discounted by the projected coverage of scouts still
/// in the spawn tube, and capped at what the marginal scout itself can service
/// within its life (the D5 "entries the extra scout would service" horizon,
/// [`SCOUT_EV_PROJECTION_ENTRIES`]); `reachable_share` is the value-weighted
/// share of that demand within [`SCOUT_STRATEGIC_REACH`] of a spawn-capable
/// home; body amortization is 50e / 1500t ≈ 0.033 e/t.
///
/// `unserviced` is `(rate_et, reachable)` per unserviced entry.
pub fn scout_spawn_ev_et(unserviced: &[(f32, bool)], pending_scouts: usize) -> f32 {
    let mut rates: Vec<(f32, bool)> = unserviced.to_vec();
    // Highest rates first — pending scouts are assumed to take the top of the
    // book, and the marginal scout services the best of what remains.
    rates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let skip = pending_scouts.saturating_mul(SCOUT_EV_PROJECTION_ENTRIES);
    let serviceable: Vec<&(f32, bool)> = rates.iter().skip(skip).take(SCOUT_EV_PROJECTION_ENTRIES).collect();

    let total: f32 = serviceable.iter().map(|(r, _)| r).sum();
    if total <= 0.0 {
        return -SCOUT_BODY_COST_E / CREEP_LIFE_TIME as f32;
    }
    let reachable: f32 = serviceable.iter().filter(|(_, ok)| *ok).map(|(r, _)| r).sum();
    let share = (reachable / total).min(1.0);

    total * share.min(1.0) - SCOUT_BODY_COST_E / CREEP_LIFE_TIME as f32
}

// ─── The assignment system ───────────────────────────────────────────────────

#[derive(SystemData)]
pub struct ScoutAssignmentSystemData<'a> {
    visibility_queue: Write<'a, VisibilityQueue>,
    assignments: Write<'a, ScoutAssignments>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    creep_owners: ReadStorage<'a, CreepOwner>,
    creep_spawnings: ReadStorage<'a, CreepSpawning>,
    jobs: ReadStorage<'a, JobData>,
    operations: ReadStorage<'a, OperationData>,
    pathfinder: Write<'a, PathfinderService>,
    spawn_queue: Write<'a, SpawnQueue>,
    governor: Read<'a, GovernorSnapshot>,
}

/// One demand entry as gathered from the visibility queue this pass.
struct DemandEntry {
    room: RoomName,
    rate_et: f32,
    value_e: f32,
    want_fresh_within: u32,
    opportunistic: bool,
    scoutable: bool,
    observable: bool,
}

pub struct ScoutAssignmentSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for ScoutAssignmentSystem {
    type SystemData = ScoutAssignmentSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let now = game::time();

        let intel_age = |room: RoomName| -> u32 {
            data.mapping
                .get_room(&room)
                .and_then(|e| data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|dvd| dvd.age())
                .unwrap_or(u32::MAX)
        };

        // ── Fresh sightings clear the unreachable backoff (the old
        //    ScoutMission rule, now owned here) ─────────────────────────────
        let fresh_again: Vec<RoomName> = data
            .visibility_queue
            .unreachable
            .iter()
            .map(|u| u.room_name)
            .filter(|r| intel_age(*r) <= UNREACHABLE_FRESH_CLEAR_TICKS)
            .collect();
        for room in fresh_again {
            debug!("ScoutAssignment: {} seen again — clearing unreachable backoff", room);
            data.visibility_queue.clear_unreachable(room);
        }

        // ── The live fleet (deterministic order) ──────────────────────────
        let mut fleet: Vec<(Entity, RoomName, u32)> = (&data.entities, &data.creep_owners, &data.jobs)
            .join()
            .filter(|(_, _, job)| matches!(job, JobData::Scout(_)))
            .filter_map(|(entity, owner, _)| {
                let creep = owner.owner.resolve()?;
                let ttl = creep.ticks_to_live().unwrap_or(CREEP_LIFE_TIME);
                Some((entity, creep.pos().room_name(), ttl))
            })
            .collect();
        fleet.sort_by_key(|(entity, _, _)| entity.id());

        let pending_scouts = (&data.creep_spawnings, &data.jobs)
            .join()
            .filter(|(_, job)| matches!(job, JobData::Scout(_)))
            .count();

        // ── Demand set: stale (per want_fresh_within), not in backoff ─────
        let mut demand: Vec<DemandEntry> = data
            .visibility_queue
            .entries
            .iter()
            .filter(|e| intel_age(e.room_name) > e.want_fresh_within)
            .map(|e| {
                let age = intel_age(e.room_name);
                let rate = tier_rate_et(e.priority) * quantized_staleness_multiplier(age, e.want_fresh_within);
                let value_e = rate * e.want_fresh_within.max(ENTRY_VALUE_FLOOR_TICKS) as f32;
                DemandEntry {
                    room: e.room_name,
                    rate_et: rate,
                    value_e,
                    want_fresh_within: e.want_fresh_within,
                    opportunistic: e.opportunistic,
                    scoutable: e.allowed_types.contains(VisibilityRequestFlags::SCOUT)
                        && !data.visibility_queue.is_unreachable_now(e.room_name, now),
                    observable: e.allowed_types.contains(VisibilityRequestFlags::OBSERVE),
                }
            })
            .filter(|d| d.scoutable || d.observable)
            .collect();
        demand.sort_by(|a, b| a.room.cmp(&b.room));

        // ── Observer-throughput projection (resolution #7) ────────────────
        // Per observer, drop the top in-range OBSERVE-able entries it can
        // keep fresh by rotation — scouts must not tour what observers
        // freshen for free.
        let observer_homes: Vec<(RoomName, usize)> = (&data.entities, &data.room_data)
            .join()
            .filter_map(|(_, rd)| {
                let dvd = rd.get_dynamic_visibility_data()?;
                if !dvd.owner().mine() {
                    return None;
                }
                let structures = rd.get_structures()?;
                if structures.spawns().is_empty() {
                    return None;
                }
                let count = structures.observers().len();
                if count == 0 {
                    None
                } else {
                    Some((rd.name, count))
                }
            })
            .collect();

        let mut observer_covered: HashSet<RoomName> = HashSet::new();
        for (home, count) in &observer_homes {
            for _ in 0..*count {
                let in_range: Vec<(RoomName, f32, u32)> = demand
                    .iter()
                    .filter(|d| d.observable && !observer_covered.contains(&d.room))
                    .filter(|d| room_distance(d.room, *home) <= OBSERVER_RANGE)
                    .map(|d| (d.room, d.value_e, d.want_fresh_within))
                    .collect();
                for room in observer_covered_rooms(&in_range) {
                    observer_covered.insert(room);
                }
            }
        }

        // ── Tour construction (resolution #1) ─────────────────────────────
        let tour_demand: Vec<TourDemand> = demand
            .iter()
            .filter(|d| d.scoutable && !observer_covered.contains(&d.room))
            .map(|d| TourDemand {
                room: d.room,
                value_e: d.value_e,
            })
            .collect();

        let scouts_kernel: Vec<TourScout> = fleet
            .iter()
            .map(|(entity, room, ttl)| TourScout {
                key: entity.id(),
                room: *room,
                ticks_to_live: *ttl,
            })
            .collect();

        let mut tours = build_tours(&scouts_kernel, &tour_demand).tours;

        // ── Head-leg route validation (bounded: ≤ fleet size find_route
        //    consultations per tick; resolution #1/#2 bonus signal) ─────────
        for (i, (_, scout_room, _)) in fleet.iter().enumerate() {
            if let Some(head) = tours[i].first().copied() {
                let route = data.pathfinder.route_distance(*scout_room, head, now);
                if !route.reachable {
                    info!("ScoutAssignment: {} is map-disconnected from {} — marking unreachable", head, scout_room);
                    data.visibility_queue.mark_unreachable(head, now);
                    tours[i].retain(|r| *r != head);
                }
            }
        }

        // ── Fallback legs (D3 + resolution #4: NEAREST, never argmax) ─────
        let assigned: HashSet<RoomName> = tours.iter().flatten().copied().collect();
        for (i, (_, scout_room, _)) in fleet.iter().enumerate() {
            if !tours[i].is_empty() {
                continue;
            }
            // Nearest qualifying unserviced entry first…
            let nearest = tour_demand
                .iter()
                .filter(|d| !assigned.contains(&d.room) && d.room != *scout_room)
                .min_by(|a, b| {
                    room_distance(*scout_room, a.room)
                        .cmp(&room_distance(*scout_room, b.room))
                        .then_with(|| a.room.cmp(&b.room))
                })
                .map(|d| d.room);

            // …else pre-position toward the nearest never-seen frontier room,
            // registered as assigner-generated opportunistic demand (never
            // counts toward spawn EV).
            let target = nearest.or_else(|| self::nearest_unseen_frontier(*scout_room, &data.mapping, &data.room_data));
            if let Some(room) = target {
                if !data.visibility_queue.has_entry(room) {
                    data.visibility_queue
                        .request(VisibilityRequest::new_opportunistic(room, VISIBILITY_PRIORITY_LOW, VisibilityRequestFlags::SCOUT));
                }
                tours[i].push(room);
            }
        }

        // ── Room-centric unreachable evidence (resolution #2) ─────────────
        let demand_rooms: HashSet<RoomName> = tour_demand.iter().map(|d| d.room).collect();
        let scout_state: Vec<(RoomName, Option<RoomName>)> = fleet
            .iter()
            .enumerate()
            .map(|(i, (_, room, _))| (*room, tours[i].first().copied()))
            .collect();
        let crossed = update_entry_fail_counters(&mut data.assignments.entry_fail, &demand_rooms, &scout_state);
        for room in crossed {
            warn!(
                "ScoutAssignment: {} unreachable — a scout sat adjacent for {} passes without entering",
                room, SCOUT_ENTRY_FAIL_TICKS
            );
            data.visibility_queue.mark_unreachable(room, now);
            for tour in tours.iter_mut() {
                tour.retain(|r| *r != room);
            }
        }

        // ── Publish tours (overwrite-only; never cleared at tick start) ───
        data.assignments.tours = fleet
            .iter()
            .zip(tours.iter())
            .map(|((entity, _, _), tour)| (*entity, tour.clone()))
            .collect();

        // ── Spawn EV (D5, resolution #6) ──────────────────────────────────
        if !data.governor.can_execute_cpu(CpuBar::MediumPriority) {
            return;
        }

        let homes: Vec<(Entity, RoomName)> = (&data.entities, &data.room_data)
            .join()
            .filter_map(|(entity, rd)| {
                let dvd = rd.get_dynamic_visibility_data()?;
                if !dvd.owner().mine() {
                    return None;
                }
                let structures = rd.get_structures()?;
                if structures.spawns().is_empty() {
                    return None;
                }
                Some((entity, rd.name))
            })
            .collect();
        if homes.is_empty() {
            return;
        }

        let final_assigned: HashSet<RoomName> = data.assignments.tours.values().flatten().copied().collect();
        let unserviced: Vec<(f32, bool, RoomName)> = demand
            .iter()
            .filter(|d| d.scoutable && !d.opportunistic)
            .filter(|d| !final_assigned.contains(&d.room) && !observer_covered.contains(&d.room))
            .map(|d| {
                let reachable = homes.iter().any(|(_, home)| room_distance(d.room, *home) <= SCOUT_STRATEGIC_REACH);
                (d.rate_et, reachable, d.room)
            })
            .collect();

        let rates: Vec<(f32, bool)> = unserviced.iter().map(|(r, ok, _)| (*r, *ok)).collect();
        let ev_et = scout_spawn_ev_et(&rates, pending_scouts);
        if ev_et <= 0.0 {
            return;
        }

        // Roster owner: the (singleton) ScoutOperation entity.
        let Some(op_entity) = (&data.entities, &data.operations)
            .join()
            .find(|(_, od)| matches!(od, OperationData::Scout(_)))
            .map(|(e, _)| e)
        else {
            return;
        };

        // Home nearest to the highest-value unserviced entry (tie: room name).
        let anchor = unserviced
            .iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.2.cmp(&a.2)))
            .map(|(_, _, room)| *room);
        let Some(anchor) = anchor else { return };
        let Some((home_entity, home_name)) = homes
            .iter()
            .min_by(|a, b| {
                room_distance(anchor, a.1)
                    .cmp(&room_distance(anchor, b.1))
                    .then_with(|| a.1.cmp(&b.1))
            })
            .copied()
        else {
            return;
        };

        let bid_milli = (ev_et * 1000.0).round().clamp(0.0, u32::MAX as f32) as u32;
        debug!(
            "ScoutAssignment: fleet EV {:.3} e/t (unserviced entries: {}, pending: {}) — bidding {} milli at {}",
            ev_et,
            unserviced.len(),
            pending_scouts,
            bid_milli,
            home_name
        );

        let spawn_request = SpawnRequest::new(
            format!("Scout - fleet EV {:.2} e/t", ev_et),
            &[Part::Move],
            bid_milli,
            None,
            ScoutOperation::create_spawn_callback(op_entity),
        );
        data.spawn_queue.request(home_entity, spawn_request);
    }
}

/// Nearest never-seen room via BFS over cached room exits (absorbed from the
/// deleted job-side `bfs_nearest_unknown`): walks the known exit graph from
/// `start` and returns the first neighboring room with no `RoomData` entity.
fn nearest_unseen_frontier(
    start: RoomName,
    mapping: &EntityMappingData,
    room_data: &ReadStorage<'_, RoomData>,
) -> Option<RoomName> {
    let mut visited: HashSet<RoomName> = HashSet::new();
    let mut queue: VecDeque<(RoomName, u32)> = VecDeque::new();

    visited.insert(start);
    queue.push_back((start, 0));

    while let Some((room, depth)) = queue.pop_front() {
        if depth >= FRONTIER_SEARCH_RADIUS {
            continue;
        }

        let mut exits: Vec<RoomName> = mapping
            .get_room(&room)
            .and_then(|entity| room_data.get(entity))
            .and_then(|rd| rd.get_static_visibility_data())
            .and_then(|svd| svd.exits())
            .map(|exits| exits.iter().map(|(_, name)| *name).collect())
            .unwrap_or_default();
        exits.sort();

        for neighbor in exits {
            if !visited.insert(neighbor) {
                continue;
            }
            if mapping.get_room(&neighbor).is_none() {
                return Some(neighbor);
            }
            queue.push_back((neighbor, depth + 1));
        }
    }

    None
}

// ─── Tests (pure kernels) ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rn(s: &str) -> RoomName {
        RoomName::new(s).unwrap()
    }

    // ── Insertion metric + memoization (review resolution #1) ───────────────

    /// Naive reference: full recompute of every (entry × scout) insertion each
    /// iteration. The memoized builder must produce IDENTICAL tours with
    /// strictly fewer delta evaluations on a multi-scout instance.
    fn naive_tours(scouts: &[TourScout], demand: &[TourDemand]) -> (Vec<Vec<RoomName>>, usize) {
        let mut tours: Vec<Vec<RoomName>> = vec![Vec::new(); scouts.len()];
        let mut used: Vec<u32> = vec![0; scouts.len()];
        let mut remaining: Vec<usize> = (0..demand.len()).collect();
        let mut evals = 0usize;

        loop {
            let mut best: Option<(usize, usize, u32, usize, f32)> = None; // (rem idx, scout, delta, pos, score)
            for (ri, &di) in remaining.iter().enumerate() {
                for (s, scout) in scouts.iter().enumerate() {
                    evals += 1;
                    let budget = scout.ticks_to_live.saturating_sub(used[s]);
                    if let Some((delta, pos)) = best_insertion(scout.room, &tours[s], demand[di].room, budget) {
                        let score = demand[di].value_e / (delta.max(1) as f32);
                        let better = match best {
                            None => true,
                            Some((bri, bs, _, _, bscore)) => {
                                let d = &demand[di];
                                let bd = &demand[remaining[bri]];
                                score > bscore
                                    || (score == bscore
                                        && (d.value_e > bd.value_e
                                            || (d.value_e == bd.value_e
                                                && (d.room < bd.room || (d.room == bd.room && scouts[s].key < scouts[bs].key)))))
                            }
                        };
                        // Within one entry, keep the first (lowest-delta then
                        // lowest scout index) insertion — mirror the memoized
                        // builder's per-entry best_scout derivation.
                        let same_entry_better = match best {
                            Some((bri, bs, bdelta, _, _)) if remaining[bri] == di => {
                                delta < bdelta || (delta == bdelta && s < bs)
                            }
                            _ => false,
                        };
                        if better || same_entry_better {
                            best = Some((ri, s, delta, pos, score));
                        }
                    }
                }
            }
            let Some((ri, s, delta, pos, _)) = best else { break };
            let di = remaining.swap_remove(ri);
            tours[s].insert(pos, demand[di].room);
            used[s] += delta;
        }
        (tours, evals)
    }

    #[test]
    fn memoized_insertion_matches_naive_and_bounds_evaluations() {
        let scouts = vec![
            TourScout {
                key: 1,
                room: rn("W10N10"),
                ticks_to_live: 1_500,
            },
            TourScout {
                key: 2,
                room: rn("W20N10"),
                ticks_to_live: 1_500,
            },
            TourScout {
                key: 3,
                room: rn("W10N20"),
                ticks_to_live: 900,
            },
        ];
        let demand: Vec<TourDemand> = [
            ("W11N10", 500.0),
            ("W12N11", 500.0),
            ("W21N11", 400.0),
            ("W22N10", 400.0),
            ("W9N19", 300.0),
            ("W10N22", 300.0),
            ("W13N13", 200.0),
            ("W19N12", 200.0),
            ("W15N15", 100.0),
            ("W8N8", 100.0),
        ]
        .iter()
        .map(|(r, v)| TourDemand {
            room: rn(r),
            value_e: *v,
        })
        .collect();

        let memoized = build_tours(&scouts, &demand);
        let (naive, naive_evals) = naive_tours(&scouts, &demand);

        assert_eq!(memoized.tours, naive, "memoization must not change the greedy result");
        assert!(
            memoized.delta_evaluations < naive_evals,
            "memoized builder must evaluate fewer deltas ({} vs naive {})",
            memoized.delta_evaluations,
            naive_evals
        );
        // Structural bound: initial full table (E×S) + one scout-column per
        // insertion (≤ E entries each) — quadratic-in-entries worst case, NOT
        // entries × scouts × iterations.
        let e = demand.len();
        let s = scouts.len();
        assert!(
            memoized.delta_evaluations <= e * s + e * e,
            "evaluation bound exceeded: {} > {}",
            memoized.delta_evaluations,
            e * s + e * e
        );
        // Every demand room within lifetime reach was assigned exactly once.
        let mut all: Vec<RoomName> = memoized.tours.iter().flatten().copied().collect();
        all.sort();
        let mut expect: Vec<RoomName> = demand.iter().map(|d| d.room).collect();
        expect.sort();
        assert_eq!(all, expect, "all reachable demand assigned exactly once");
    }

    /// The insertion metric is Chebyshev × 50 — a diagonal neighbor costs one
    /// hop, and inserting between two stops prices the detour delta.
    #[test]
    fn insertion_deltas_are_chebyshev_hops() {
        // Empty tour: the delta is the direct leg from the scout.
        let (delta, pos) = best_insertion(rn("W10N10"), &[], rn("W12N11"), 10_000).unwrap();
        assert_eq!(delta, 2 * TICKS_PER_ROOM_HOP, "Chebyshev(W10N10,W12N11) = 2 hops");
        assert_eq!(pos, 0);

        // Insert W11N10 into [W12N10]: on the way — zero detour, position 0.
        let (delta, pos) = best_insertion(rn("W10N10"), &[rn("W12N10")], rn("W11N10"), 10_000).unwrap();
        assert_eq!(delta, 0, "an on-path stop costs no extra travel");
        assert_eq!(pos, 0);

        // Lifetime budget rejects an unaffordable stop.
        assert!(best_insertion(rn("W10N10"), &[], rn("W40N10"), 100).is_none());
    }

    // ── Room-centric evidence counter (review resolution #2) ────────────────

    #[test]
    fn entry_fail_counter_accumulates_resets_and_fires() {
        let target = rn("W5N5");
        let mut counters = HashMap::new();
        let demand: HashSet<RoomName> = [target].into_iter().collect();

        // Scout tasked with the room, sitting adjacent: accumulates.
        let adjacent = vec![(rn("W5N6"), Some(target))];
        for _ in 0..(SCOUT_ENTRY_FAIL_TICKS - 1) {
            let crossed = update_entry_fail_counters(&mut counters, &demand, &adjacent);
            assert!(crossed.is_empty());
        }
        assert_eq!(counters.get(&target).copied(), Some(SCOUT_ENTRY_FAIL_TICKS - 1));

        // Any scout ENTERING the room resets the streak.
        let entered = vec![(target, Some(target))];
        update_entry_fail_counters(&mut counters, &demand, &entered);
        assert!(!counters.contains_key(&target), "entering resets the evidence");

        // Re-accumulate to the threshold: fires exactly once.
        for _ in 0..(SCOUT_ENTRY_FAIL_TICKS - 1) {
            assert!(update_entry_fail_counters(&mut counters, &demand, &adjacent).is_empty());
        }
        let crossed = update_entry_fail_counters(&mut counters, &demand, &adjacent);
        assert_eq!(crossed, vec![target], "threshold crossing reports the room");
        assert!(!counters.contains_key(&target), "counter resets after firing");

        // The count is CONSECUTIVE: a pass without a tasked-adjacent scout breaks it.
        let away = vec![(rn("W9N9"), Some(target))];
        update_entry_fail_counters(&mut counters, &demand, &adjacent);
        assert_eq!(counters.get(&target).copied(), Some(1));
        update_entry_fail_counters(&mut counters, &demand, &away);
        assert!(!counters.contains_key(&target), "a non-adjacent pass breaks the streak");

        // A room that LEAVES demand loses its evidence.
        update_entry_fail_counters(&mut counters, &demand, &adjacent);
        assert!(counters.contains_key(&target));
        let empty: HashSet<RoomName> = HashSet::new();
        update_entry_fail_counters(&mut counters, &empty, &adjacent);
        assert!(!counters.contains_key(&target), "leaving demand clears the evidence");

        // A scout adjacent but NOT tasked with the room is not evidence.
        let untasked = vec![(rn("W5N6"), Some(rn("W1N1")))];
        let crossed = update_entry_fail_counters(&mut counters, &demand, &untasked);
        assert!(crossed.is_empty());
        assert!(!counters.contains_key(&target), "untasked adjacency is not evidence");
    }

    // ── Spawn EV closed form (review resolution #6) ─────────────────────────

    #[test]
    fn spawn_ev_closed_form() {
        let amortized = SCOUT_BODY_COST_E / CREEP_LIFE_TIME as f32; // ≈ 0.0333 e/t

        // No demand: EV is exactly the (negative) amortized body cost.
        assert!((scout_spawn_ev_et(&[], 0) + amortized).abs() < 1e-6);

        // All reachable: EV = Σrates − amortization.
        let all_reachable = vec![(5.0, true), (2.0, true), (0.75, true)];
        let ev = scout_spawn_ev_et(&all_reachable, 0);
        assert!((ev - (7.75 - amortized)).abs() < 1e-4, "EV = Σ − amortized: {ev}");

        // Half the value reachable: the share discounts the total.
        let half = vec![(4.0, true), (4.0, false)];
        let ev = scout_spawn_ev_et(&half, 0);
        assert!((ev - (8.0 * 0.5 - amortized)).abs() < 1e-4, "share-weighted: {ev}");

        // Nothing reachable: share 0 → pure cost → negative.
        let none = vec![(5.0, false)];
        assert!(scout_spawn_ev_et(&none, 0) < 0.0);

        // A pending scout consumes the projected top of the book.
        let mut many: Vec<(f32, bool)> = Vec::new();
        for _ in 0..SCOUT_EV_PROJECTION_ENTRIES {
            many.push((5.0, true));
        }
        many.push((1.0, true));
        let ev_no_pending = scout_spawn_ev_et(&many, 0);
        let ev_one_pending = scout_spawn_ev_et(&many, 1);
        assert!(ev_no_pending > ev_one_pending, "a pending scout absorbs the top entries");
        assert!(
            (ev_one_pending - (1.0 - amortized)).abs() < 1e-4,
            "only the leftover tail prices the next spawn: {ev_one_pending}"
        );

        // The marginal scout's own serviceability caps the total (D5 horizon):
        // a 100-entry book prices as its top window, not the whole book.
        let flood: Vec<(f32, bool)> = (0..100).map(|_| (5.0, true)).collect();
        let ev = scout_spawn_ev_et(&flood, 0);
        let cap = SCOUT_EV_PROJECTION_ENTRIES as f32 * 5.0 - amortized;
        assert!((ev - cap).abs() < 1e-3, "EV capped at the per-scout horizon: {ev} vs {cap}");
    }

    // ── Staleness quantization (review resolution #4) ───────────────────────

    #[test]
    fn staleness_multiplier_is_quantized_and_capped() {
        // Fresh-ish: ratio ≈ 1 → bucket 1.0.
        assert_eq!(quantized_staleness_multiplier(250, 250), 1.0);
        // ratio 1.3 → bucket 1.25 (floors, so small drifts do not reorder).
        assert_eq!(quantized_staleness_multiplier(325, 250), 1.25);
        assert_eq!(quantized_staleness_multiplier(330, 250), 1.25);
        // Cap at ×3, including the never-seen u32::MAX age.
        assert_eq!(quantized_staleness_multiplier(10_000, 250), 3.0);
        assert_eq!(quantized_staleness_multiplier(u32::MAX, 250), 3.0);
        // want_fresh_within = 0 (imperative) never divides by zero.
        assert_eq!(quantized_staleness_multiplier(5, 0), 3.0);
    }

    // ── Observer-throughput projection (review resolution #7) ───────────────

    #[test]
    fn observer_projection_respects_rotation_period() {
        // Three 100-tick rooms: one observer rotates all three.
        let relaxed = vec![
            (rn("W1N1"), 500.0, 100),
            (rn("W2N2"), 400.0, 100),
            (rn("W3N3"), 300.0, 100),
        ];
        assert_eq!(observer_covered_rooms(&relaxed).len(), 3);

        // A want_fresh_within ≈ 1 room needs dedicated coverage: the observer
        // takes it (highest value) and cannot rotate anything else in.
        let strict = vec![(rn("W1N1"), 500.0, 1), (rn("W2N2"), 400.0, 100)];
        assert_eq!(observer_covered_rooms(&strict), vec![rn("W1N1")]);

        // Value orders the take; equal values break ties on `RoomName`'s own
        // `Ord` (its packed representation — deterministic, input-order-free).
        let tie = vec![(rn("W2N2"), 400.0, 100), (rn("W1N1"), 400.0, 100)];
        let mut expected = vec![rn("W1N1"), rn("W2N2")];
        expected.sort();
        assert_eq!(observer_covered_rooms(&tie), expected);
        let flipped = vec![(rn("W1N1"), 400.0, 100), (rn("W2N2"), 400.0, 100)];
        assert_eq!(observer_covered_rooms(&flipped), expected, "input order must not matter");
    }
}
