# 0046 — Scout assignment post-pass, multi-room tours, and EV-driven fleet sizing

Status: **Accepted + implemented** (operator-directed 2026-08-11; adversarial 4-reviewer design
review 2026-08-12 — its resolutions are BINDING and recorded in §6; implemented as one batch
P1+P2+P3, WFV 27→28).
Origin: the expansion-stall diagnosis ([expansion-stall-2026-08-11.md](../reviews/expansion-stall-2026-08-11.md))
identified the scout fulfillment layer as the root of the MMO claim lockdown (finding cluster M1),
and the operator directed a redesign: *"Using a similar pre-pass and scout queue TTL + persistence
like other queue systems may be beneficial to allow a post-process system to make optimal
assignments. We also should remove the scout idle in room behavior and just have it continuously
try to move to the best scout mission it has been assigned. We may also want to allow a single
scout to claim multiple missions. Where a single scout can efficiently path to many rooms, we
should prefer that. It may be worth computing the expected value of a scout to drive spawning."*

Supersedes the fulfillment half of [ADR 0021](0021-strategic-visibility.md) (its register→broker
layer is kept verbatim; its open follow-ups #1/#2 — a re-scout scheduler and observer-preference —
are absorbed here). The producer/consumer contract (`VisibilityRequest` upsert, TTL, priority,
`OBSERVE|SCOUT` flags) is unchanged for callers.

## 1. What is wrong with the current fulfillment layer

The broker (`VisibilityQueue`) is already the right shape: TTL'd (100 ticks), persistent
(serialized component), idempotent upserts, priority-coalesced. Everything downstream of it is
greedy and per-creep, and that is where the live failures come from
(file references = master at diagnosis time, pre-implementation — F1-F5's code is now deleted):

| # | Defect | Where |
|---|---|---|
| F1 | Scout target-pick is greedy per-creep, priority-then-closest, **no freshness filter** — scouts re-claim rooms already resolved this window because producers keep re-upserting them (claim.rs re-asserts CRITICAL for its whole `unknown_rooms` cache every tick of a scouting window) | `best_unclaimed_for` [visibilitysystem.rs:301](../../screeps-ibex/src/room/visibilitysystem.rs); [claim.rs:362](../../screeps-ibex/src/operations/claim.rs) |
| F2 | One claim per scout; on arrival the scout releases and greedily re-picks by global priority — it bounces to the far top-priority room instead of sweeping the cluster it is standing in | [jobs/scout.rs:55–101](../../screeps-ibex/src/jobs/scout.rs) |
| F3 | Idle-in-room: with everything claimed/observed the scout parks (`mark_idle`) and only after 10 ticks starts a LOW-priority adjacent explore | [jobs/scout.rs:107–122,178–206](../../screeps-ibex/src/jobs/scout.rs) |
| F4 | Fleet size is a constant: `MAX_SCOUT_MISSIONS = 3` per-room missions empire-wide, regardless of frontier size (live: 124 rooms wanting intel, 3 slots) | [operations/scout.rs:260](../../screeps-ibex/src/operations/scout.rs) |
| F5 | The per-room `ScoutMission` counts *spawns*, but its scouts pick targets globally — after 4 "defecting" spawns the mission falsely marks its room scout-unreachable with 2k→20k-tick backoff (live: 103 poisoned rooms, several 1 hop from colonies) | [missions/scout.rs:195–206](../../screeps-ibex/src/missions/scout.rs) |
| F6 | Observers are assigned greedily by the same priority sort with no rotation and no already-fresh filter — k observers re-observe the same top-k entries every tick | `ObserverSystem` [visibilitysystem.rs:583](../../screeps-ibex/src/room/visibilitysystem.rs) |
| F7 | Scout spawning is fixed-priority (MEDIUM, HIGH iff the entry is CRITICAL) and CPU-gated at the *lowest* bar in the empire (`CpuBar::LowPriority`), so the intel lane that everything else gates on is the first thing to silently stop | [missions/scout.rs:209–251](../../screeps-ibex/src/missions/scout.rs) |

## 2. Design

### D1 — The queue stays; entries gain a freshness target

`VisibilityQueue`/`VisibilityQueueData` remain the single TTL'd, persistent request store (the
"pre-pass … TTL + persistence like other queue systems" already exists here — this ADR does not
add a second queue). One extension: `VisibilityEntry` gains

```rust
/// Intel age (ticks) at or below which this entry counts as SERVICED.
/// Producers declare how fresh they need the room; the assigner derives
/// service state instead of producers churning requests.
pub want_fresh_within: u32,   // default: DEFAULT_VISIBILITY_TTL
```

Producers keep re-asserting idempotently (that part of the contract is good — TTL + re-assert is
how every queue in this codebase works); the **assigner** now filters by
`dynamic_visibility.age() <= want_fresh_within` instead of trusting the entry's mere existence.
This single field kills F1's re-claim churn without changing any producer's cadence, and it lets
the claim pipeline declare its real need (`want_fresh_within = intel_freshness_ticks` on candidate
entries) so scouts arrive *because the commit gate needs them*.

Upsert merge (review resolution #3): `want_fresh_within` **MIN-merges** — the strictest freshness
demand wins — while `priority` stays MAX-merged and `expires_at` stays MAX-merged. Pinned by
`upsert_min_merges_want_fresh_within_and_max_merges_priority` next to the queue tests.

Serialization: adding a field to `VisibilityEntry` is a positional bincode change inside
`VisibilityQueueData` ⇒ **WFV 27→28** (operator has sanctioned resets; batch per
[0002](0002-serialization.md) conventions with the other shape changes in D4).

### D2 — `ScoutAssignmentSystem`: one post-process pass owns ALL fulfillment

A new system (`room/scoutassignment.rs`) runs after ALL producers — operations, missions, AND the
squad manager (dispatch slot: after `RunSquadUpdateSystem`, before `SpawnRefillPricingSystem` /
`RunJobSystem`, game_loop.rs; review resolution #8) — and before jobs:

1. **Build the demand set**: entries whose flags allow servicing, whose room is NOT fresh within
   `want_fresh_within`, and not in unreachable-backoff (for the scout leg only, as today).
2. **Observer throughput first** (F6 fix, review resolution #7): before tours and EV, the pass
   SUBTRACTS projected observer coverage — per observer, the top in-range OBSERVE-able demand
   entries it can keep fresh by rotation are dropped from the scout demand set (an observer
   services ~1 room/tick in range `OBSERVER_RANGE`; a rotation over k rooms sustains a room only
   while `k <= want_fresh_within`, so a `want_fresh_within ≈ 0-1` entry monopolizes its observer).
   Scouts must not tour what observers freshen for free. The actual observer *assignment* stays in
   `ObserverSystem` (same shed class as before) but gains the D1 freshness filter and
   least-recently-observed-first rotation among equal priorities (`last_observed:
   HashMap<RoomName, u32>` in the `ScoutAssignments` resource; deterministic room tie-break).
3. **Scout tours** (F2 fix): assign the remaining demand to the live scout fleet as **ordered
   multi-room tours** — each scout holds a route, not a single claim. Construction: greedy
   cheapest-insertion — repeatedly take the unassigned entry with the best
   `value / marginal_travel_ticks` where `marginal_travel_ticks` is the cheapest insertion delta
   over any current tour, respecting each scout's remaining lifetime.
   **Insertion metric (review resolution #1, BLOCKING fix)**: every insertion delta is priced
   with **Chebyshev room distance × 50** (`room_distance`, room/visibilitysystem.rs) — NEVER
   `route_distance_via` (a missing cache entry always calls the engine's `find_route`; thousands
   of cold calls on the first post-reset tick would crater the bucket, and even warm it costs
   ~50-100 CPU/tick). `PathfinderService::route_distance` is consulted ONLY for the ≤fleet-size
   chosen tour HEAD legs per tick (bounded, pool-friendly) as a map-disconnect check. Each
   entry's best-insertion is **memoized** between greedy iterations: after an insertion into
   scout S only S's column re-evaluates, and cached overall-bests re-derive from the columns
   (pinned by `memoized_insertion_matches_naive_and_bounds_evaluations`).
   Deterministic tie-breaks (quantized value, then room name, then scout entity id) per the
   [per-tick-optimal convention](../../docs/guides/engineering-practices.md) — the pass is
   **recomputed every tick from scratch**; stability comes from determinism, not latching.
   Tours live in the `ScoutAssignments` resource (assignment is derived state — nothing new
   serialized), which is **never cleared at tick start** (see the shed-class note below).
4. **Unreachable evidence** (F5 fix; review resolution #2, BLOCKING fix): evidence is
   **ROOM-centric**, not (scout, room): per demand room, count consecutive passes during which
   SOME assigned scout was adjacent-and-not-inside while the room was its tour head;
   `SCOUT_ENTRY_FAIL_TICKS` (~100) marks it unreachable via the existing `VisibilityQueue`
   backoff. The counter resets when the room leaves demand or any scout enters. Rover
   `MovementFailure::PathNotFound` is NOT evidence — it is overloaded with CPU/budget exhaustion
   and false-positives exactly under load. `find_route reachable:false` is only a bonus signal
   for map-disconnected rooms (the bounded head-leg check above). Fresh-sighting clearing stays
   (now owned by the assignment pass). **No poison-list migration code**: the WFV 28 reset wipes
   the persisted `unreachable` list outright; the empty-list thundering herd is bounded by tour
   budgeting (lifetime-capped tours ration how much frontier the fleet can chase at once).

Shed class (review resolution #8): the pass is `StageClass::SkipUnderCritical` — observer
assignment keeps parity with its pre-ADR shed class — while scout jobs/movement remain in the
never-shed set. The `ScoutAssignments` resource is NEVER cleared at tick start: the pass
overwrites it only when it runs, so on skipped ticks scouts keep walking persisted tours, and
jobs tolerate tour entries whose demand has vanished (freshness self-heals next pass).

### D3 — The scout job becomes "always move to the front of my tour" (F3 fix)

`ScoutState` collapses to a single behavior: read my tour from the assignment resource; move
toward its first room (existing `tick_move_to_room_with_bid`, `SCOUT_INTEL_BID` unchanged); the
assigner pops a tour stop when the room's intel becomes fresh (arrival is not the goal — fresh
intel is; an observer or another scout getting there first advances the tour too, because the
freshness filter re-runs every tick). A scout with an empty tour is not idle: the assigner always
gives it a fallback leg — toward the **NEAREST qualifying unserviced entry** (review resolution
#4: not the globally largest cluster — a global argmax ping-pongs the whole idle fleet between
frontiers as values drift), or, when the demand set is truly empty, the nearest never-seen
frontier room (absorbing today's `pick_adjacent_explore_target` BFS into the assigner as a
LOW-priority **opportunistic** demand producer instead of job-side special-casing — opportunistic
so it never counts toward spawn EV). The `Idle` state, `idle_since`, and the job-side
opportunistic-request path are deleted (their serialized shapes fold into WFV 28 — the whole
`ScoutState` machine collapses to a plain tour-walking struct).

Assignment stability (review resolution #4): the staleness value-multiplier is **quantized into
0.25 buckets** (`quantized_staleness_multiplier`: `age / want_fresh_within` clamped to [1, 3],
floored to quarter steps) so the greedy's primary key is piecewise-constant across ticks — a
smoothly rising multiplier would re-order the greedy every tick and thrash tours.

### D4 — Delete the per-room `ScoutMission`; the fleet is pooled

With assignment centralized, a mission whose identity is one room is meaningless (it was the
source of F5). `ScoutOperation` becomes the **fleet owner**: it holds the scout roster and
retires the operation's mission bookkeeping. `MissionData::Scout` is removed — a serialized-enum
shape change ⇒ folds into the same **WFV 28** bump as D1. Spawn callbacks attach the `ScoutJob`
and the roster entry directly on the operation.

Operation seams (review resolution #9): the `Operation` trait gains `remove_creep`/`get_creeps`
(default no-ops, mirroring `Mission`); `EntityCleanupSystem`'s creep phase notifies operations,
and the serialize-time dead-creep scrub in `repair_entity_integrity` gains an operations pass
(mirroring the mission paths at cleanup.rs / game_loop.rs) — so a roster entry can never dangle
into the specs `ConvertSaveload` serialize panic. `OperationSystemData` /
`OperationExecutionSystemData` gain `spawn_queue` (operations can request spawns without a
mission intermediary). The typed roster attach goes through a new `operation_type!` macro
(operations/data.rs) mirroring `mission_type!` (missions/data.rs:174-224). NOTE the spawn *bid*
itself is pushed by the `ScoutAssignmentSystem`, not the operation — see D5.

### D5 — EV-driven fleet sizing (F4/F7 fix)

Replace `MAX_SCOUT_MISSIONS = 3` with a marginal-value computation every assignment pass.
Review resolution #6 (BINDING): the EV is computed and bid **from the `ScoutAssignmentSystem`
itself** — same tick: it runs after all producers and `SpawnQueueSystem` consumes later that
tick — NOT from the operation's earlier dispatch slot (which would price against last tick's
demand). **The closed form is THE spec** (the hypothetical-extra-scout re-run is explicitly NOT
implemented):

- **Entry value convention**: `value_e = rate × want_fresh_within` (floored at one default TTL so
  an imperative `want_fresh_within = 0` flag does not price at zero), where `rate` is the
  priority-tier seed in e/t (§4a: CRITICAL 5.0, HIGH 2.0, MEDIUM 0.75, LOW 0.1) × the quantized
  staleness multiplier. The existing `SCOUT_INTEL_BID` (movement-market epsilon) stays the
  *movement* bid; this is the *spawn* bid.
- **Closed form**: `EV = unserviced_demand_value × min(1, reachable_share) − body_amortization`,
  all units e/t. `unserviced_demand_value` = summed staleness-scaled rates of demand left over
  after tours + observer projection, discounted by the projected coverage of scouts still in the
  spawn tube, and capped at what the marginal scout itself can service within its life (the
  "entries the extra scout would service within its 1500-tick life" horizon —
  `SCOUT_EV_PROJECTION_ENTRIES` top entries); `reachable_share` = value-weighted share within
  Chebyshev 10 of a spawn-capable home; body amortization = 50e/1500t ≈ 0.033 e/t.
- **ONLY externally-produced, NON-opportunistic demand counts toward EV** (the `opportunistic`
  flag survives and is honored): assigner-generated explore/frontier demand is serviced by
  surplus tour capacity only and NEVER counts toward spawning.
- **Spawn while EV > 0**, bidding the computed EV (milli-e/t) in the spawn queue (ADR 0040
  civilian bid lane) instead of a fixed MEDIUM band — a starving intel lane with a fat frontier
  now outbids steady-state economy, which is exactly the inversion the stall diagnosis asked
  for. The CPU-governor bar for scout spawns is `CpuBar::MediumPriority` — the same bar the
  economy's reserve/upgrade lanes use (intel is an input to everything; it must not be the first
  lane shed — finding M1/F7).

### D6 — Producer-side alignments (small, same change set)

Review resolution #5 enumerated EVERY `VisibilityRequest::new` call site and its declared
freshness. Anything not listed below carries the default (`DEFAULT_VISIBILITY_TTL = 100`):

| Producer (call site) | `want_fresh_within` | Why |
|---|---|---|
| claim Discover + refresh, candidate rooms (`operations/claim.rs`, 2 sites) | `intel_freshness_ticks` (250) | the commit gate's real freshness need |
| claim Discover + refresh, unknown rooms (`operations/claim.rs`, 2 sites) | `VISIBILITY_TIMEOUT` (20000) | ANY sighting resolves an unknown room |
| squad-manager committed-objective OBSERVE (`military/squad_manager.rs`) | 1 | continuous coverage is load-bearing mid-fight; monopolizes its observer's rotation slot |
| salvage sighting-confirmation (`operations/salvage.rs` `refresh_sightings`) | `derelict.confirm_ticks` (2000) | its real threshold: a sighting inside the confirm window is the freshest the pipeline can use |
| operator scout FLAGS (`operations/scout.rs`) | 0 | imperative force-visit — only a same-tick sighting satisfies it |
| war re-scouts ×4 + opportunistic neighbor (`operations/war.rs`) | default (100) | |
| mining outpost eval/derelict-wait (`operations/miningoutpost.rs`, `missions/miningoutpost.rs`) | default (100) | |
| SK ring discovery (`operations/sourcekeeper.rs`), stronghold re-scout (`missions/sourcekeeperfarm.rs`) | default (100) | |
| salvage mission eyes (`missions/salvage.rs`), local supply static/structure data (`missions/localsupply/mod.rs`, `source_mining.rs`) | default (100) | |
| assigner frontier fallback (`room/scoutassignment.rs`, opportunistic) | default (100) | never counts toward EV |

No cadence change needed anywhere — the freshness filter makes every-tick re-asserts harmless.
Priority policy stays tiered for now (CRITICAL unknowns / HIGH candidates), but the assigner's
`value/marginal_travel` objective already softens the strict-priority inversion (a cheap nearby
HIGH beats a far CRITICAL when the ratio says so). Full numeric-bid unification of visibility
priorities is deferred (§4c).

## 3. What this fixes, mapped to the stall diagnosis

- M1 (scout starvation + pinning + false unreachable + 3-slot cap): F1–F5 all addressed
  structurally; the poison list is re-validated on deploy.
- M2 (coverage/freshness squeeze): candidates now get serviced *because* they declare
  `want_fresh_within = 250`, so `scouting_coverage_complete` and the commit-time re-check become
  satisfiable. (The claim-side ordering/plan fixes remain separate work — stall report §4 items
  2–4.)
- F6/0021-followup: observers rotate and skip fresh rooms; OBSERVE-only entries keep never
  spawning scouts.

## 4. Open questions (RESOLVED by the 2026-08-12 design review)

a. **Entry value seeds** — RATIFIED: CRITICAL 5.0 e/t, HIGH 2.0, MEDIUM 0.75, LOW 0.1, scaled by
   staleness ratio (`age / want_fresh_within`, capped ×3) — with the multiplier QUANTIZED into
   0.25 buckets (resolution #4). Tune in soak.
b. **Tour horizon** — RATIFIED: lifetime-only; the memoized insertion (resolution #1) bounds the
   pass at initial `entries × scouts` deltas + one scout-column per insertion.
c. **Numeric-bid visibility priorities** — DEFERRED to a follow-up ADR (unchanged).
d. **WFV batching** — RESOLVED: ONE WFV 27→28 bump batching ALL shape changes
   (`VisibilityEntry.want_fresh_within`, `MissionData::Scout` deletion, the `ScoutState` machine
   collapse incl. `Idle`/`idle_since`), mirrored in `operations/claim.rs`
   `EXPECTED_WORLD_FORMAT_VERSION`. No other flags/config — always-on automatic behavior
   (operator directive).

## 5. Rollout

Implemented as ONE batch (P1+P2+P3 below collapsed — the operator sanctioned the reset, and
staged interim heuristics would have been dead code within the same deploy):

- ~~P1 (no serialization change)~~ / ~~P2 (WFV 28)~~ / ~~P3 (EV bidding)~~ — all landed together
  at WFV 28. There is no poison-list migration: the reset wipes the `unreachable` list (see D2.4).
- Verify on private soak first (offense-soak recipe), then MMO on explicit go-ahead.
- **Success criteria — scoped to what THIS change controls** (review resolution #11; the
  claim-side M2/M3 fixes — rolling commit, coverage simplification, plan prefetch — already
  landed in Wave 1, commit `09c36db`, and their criteria belong there):
  - the `unreachable` list SHRINKS from empty-after-reset steady state (no 1-hop-from-colony
    entries reappear; the old list peaked at 103 rooms);
  - coverage/freshness passes in the claim Select logs: candidates pass the commit-time
    freshness re-check (`intel_freshness_ticks`) because scouts keep them fresh — the
    "stale-intel skip" reason disappears from Select captures;
  - scouts visibly tour (multi-room routes in the summary HUD) instead of idling, and the fleet
    size tracks frontier demand (spawns while EV > 0) rather than pinning at 3.

## 6. Design review resolutions (2026-08-12) — BINDING

An adversarial 4-reviewer design review (28 findings) amended this ADR before implementation.
Numbered resolutions and where each landed:

1. **Insertion metric (BLOCKING)**: cheapest-insertion prices ALL deltas with Chebyshev × 50,
   never `route_distance_via`; route cache only for the ≤fleet-size tour HEAD legs per tick;
   per-entry best-insertion memoized between iterations → §2 D2.3;
   `room/scoutassignment.rs` `build_tours` + head-leg check.
2. **Unreachable evidence (BLOCKING)**: room-centric consecutive adjacent-not-inside counting
   (~100 ticks) via the existing backoff; `PathNotFound` is NOT evidence; `find_route
   reachable:false` only as a map-disconnect bonus signal; NO poison-list migration (the WFV
   reset wipes it) → §2 D2.4; `update_entry_fail_counters`.
3. **`want_fresh_within` MIN-merge** on upsert (priority MAX, expires MAX) + unit test → §2 D1;
   `VisibilityQueue::request`.
4. **Assignment stability**: staleness multiplier quantized to 0.25 buckets; empty-tour fallback
   targets the NEAREST qualifying unserviced entry, not the global argmax → §2 D3;
   `quantized_staleness_multiplier` + the fallback pass.
5. **Producer freshness declarations**: full call-site enumeration (claim 250 / unknown 20000,
   squads 1, salvage confirmation 2000, flags 0, rest default 100) → §2 D6 table.
6. **Spawn EV**: computed + bid from the `ScoutAssignmentSystem` itself (same tick), closed form
   IS the spec, e/t units, non-opportunistic external demand only, `CpuBar::MediumPriority`
   gate → §2 D5; `scout_spawn_ev_et`.
7. **Observer throughput**: projected observer coverage subtracted before tours + EV (top-N
   in-range entries per observer, N bounded by the rotation-period-vs-freshness-window rule)
   → §2 D2.2; `observer_covered_rooms`.
8. **Dispatcher + shed class**: pass runs after `RunSquadUpdateSystem`, before `RunJobSystem`
   (and before `SpawnRefillPricingSystem` so its bid is priced); `SkipUnderCritical`; the tour
   resource is never cleared at tick start → §2 D2 preamble + shed-class note; game_loop.rs.
9. **Operation seams**: `Operation::remove_creep`/`get_creeps` defaults; cleanup + serialize-time
   scrub notify operations; `spawn_queue` on the operation SystemData; `operation_type!` macro
   for the typed roster attach → §2 D4.
10. **Serialization**: ONE WFV 27→28 bump batching all shape changes + the
    `EXPECTED_WORLD_FORMAT_VERSION` mirror in operations/claim.rs; no new flags/config → §4d;
    game_loop.rs WFV history entry 28.
11. **Success criteria scoped** to this change (unreachable shrinkage + coverage/freshness in
    Select logs; claim-side M2/M3 already landed in Wave 1) → §5.

Implementation notes recorded for reviewers: the D5 closed form discounts scouts still in the
spawn tube and caps the marginal scout's serviceable value at its lifetime horizon
(`SCOUT_EV_PROJECTION_ENTRIES` — this is the ADR's own "entries the extra scout would service
within its 1500-tick life" bound, and it keeps a fat frontier from bidding unboundedly over the
CRITICAL economy lanes); the `value_e = rate × want_fresh_within` convention floors the window at
one default TTL so `want_fresh_within = 0` imperatives do not price at zero.
