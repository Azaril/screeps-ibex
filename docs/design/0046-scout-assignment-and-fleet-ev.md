# 0046 — Scout assignment post-pass, multi-room tours, and EV-driven fleet sizing

Status: **Proposed** (operator-directed, 2026-08-11).
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
(file references = current master):

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

Serialization: adding a field to `VisibilityEntry` is a positional bincode change inside
`VisibilityQueueData` ⇒ **WFV 27→28** (operator has sanctioned resets; batch per
[0002](0002-serialization.md) conventions with the other shape changes in D4).

### D2 — `ScoutAssignmentSystem`: one post-process pass owns ALL fulfillment

A new system runs after producers (operations) and before jobs:

1. **Build the demand set**: entries whose flags allow servicing, whose room is NOT fresh within
   `want_fresh_within`, and not in unreachable-backoff (for the scout leg only, as today).
2. **Observers first** (F6 fix): assign each observer to the highest-value in-range demand entry
   it has not serviced recently — rotation via "least-recently-observed first" among equal
   priorities (a small `last_observed: HashMap<RoomName, u32>` in the ephemeral resource is
   enough; deterministic tie-break on room name). Entries taken by an observer leave the demand
   set (observation lands next tick; the freshness filter in (1) then holds them out naturally).
3. **Scout tours** (F2 fix): assign the remaining demand to the live scout fleet as **ordered
   multi-room tours** — each scout holds a route, not a single claim. Construction: greedy
   cheapest-insertion — repeatedly take the unassigned entry with the best
   `value / marginal_travel_ticks` where `marginal_travel_ticks` is the cheapest insertion delta
   over any current tour (rover route-cache estimates, `hops × 50`), respecting each scout's
   remaining lifetime. Deterministic tie-breaks (quantized value, then room name, then scout
   entity id) per the [per-tick-optimal convention](../../docs/guides/engineering-practices.md) —
   the pass is **recomputed every tick from scratch**; stability comes from determinism, not
   latching. Tours live in the ephemeral `VisibilityQueue` resource (assignment is derived state —
   nothing new serialized).
4. **Unreachable evidence** (F5 fix): `mark_unreachable` moves off "mission exhausted its spawns"
   onto actual failure evidence owned by the assigner/job: (a) the rover reports no route to the
   room under scout pricing, or (b) the same scout was continuously tasked to enter the room and
   failed for `SCOUT_ENTRY_FAIL_TICKS` (~100) while adjacent. Fresh-sighting clearing stays.
   Migration: on deploy, reset every existing `unreachable.retry_after` to `now + small stagger`
   so the 103-room poison list re-validates instead of persisting for weeks.

### D3 — The scout job becomes "always move to the front of my tour" (F3 fix)

`ScoutState` collapses to a single behavior: read my tour from the assignment resource; move
toward its first room (existing `tick_move_to_room_with_bid`, `SCOUT_INTEL_BID` unchanged); the
assigner pops a tour stop when the room's intel becomes fresh (arrival is not the goal — fresh
intel is; an observer or another scout getting there first advances the tour too, because the
freshness filter re-runs every tick). A scout with an empty tour is not idle: the assigner always
gives it a fallback leg — pre-position toward the centroid of the largest unserviced demand
cluster it could reach (or, when the demand set is truly empty, the nearest never-seen frontier
room — absorbing today's `pick_adjacent_explore_target` into the assigner as a LOW-priority
demand producer instead of job-side special-casing). The `Idle` state, `idle_since`, and the
job-side opportunistic-request path are deleted.

### D4 — Delete the per-room `ScoutMission`; the fleet is pooled

With assignment centralized, a mission whose identity is one room is meaningless (it was the
source of F5). `ScoutOperation` becomes the **fleet owner**: it holds the scout roster, requests
spawns (D5), and retires the operation's mission bookkeeping. `MissionData::Scout` is removed —
a serialized-enum shape change ⇒ folds into the same **WFV 28** bump as D1. Spawn callbacks
attach the `ScoutJob` directly to the fleet roster.

### D5 — EV-driven fleet sizing (F4/F7 fix)

Replace `MAX_SCOUT_MISSIONS = 3` with a marginal-value computation each spawn-consider cadence:

- **Entry value**: map priority tier → intel value in energy-equivalent-per-tick, consistent with
  the ADR 0040 numeric-bid convention (CRITICAL claim-frontier intel is worth more than LOW
  opportunistic explore; concrete seed numbers in §4 open question a). The existing
  `SCOUT_INTEL_BID` (movement-market epsilon) stays the *movement* bid; this is the *spawn* bid.
- **Marginal fleet EV**: run the D2 assignment once more with a hypothetical extra scout at the
  candidate home's spawn; `EV = Σ value(entries the extra scout would service within its 1500-tick
  life) − (50e body + spawn-slot opportunity cost via the spawn market's own bid mechanics)`.
  In practice a cheaper closed form is fine: `unserviced_demand_value × min(1, reachable_share)`.
- **Spawn while EV > 0**, bidding the computed EV in the spawn queue (ADR 0040 civilian bid lane)
  instead of a fixed MEDIUM band — a starving intel lane with a fat frontier now outbids
  steady-state economy, which is exactly the inversion the stall diagnosis asked for. The
  CPU-governor bar for scout spawns drops from `LowPriority` to the same bar as economy spawning
  (intel is an input to everything; it must not be the first lane shed — finding M1/F7).

### D6 — Producer-side alignments (small, same change set)

- `operations/claim.rs` `refresh_visibility_requests`: candidate entries assert
  `want_fresh_within = intel_freshness_ticks`; unknown-room entries assert
  `want_fresh_within = VISIBILITY_TIMEOUT` (any sighting). No cadence change needed — the
  freshness filter makes the every-tick re-assert harmless.
- Priority policy stays tiered for now (CRITICAL unknowns / HIGH candidates), but the assigner's
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

## 4. Open questions (operator input wanted)

a. **Entry value seeds** for D5 — proposal: CRITICAL 5.0 e/t, HIGH 2.0, MEDIUM 0.75, LOW 0.1,
   scaled by staleness ratio (`age / want_fresh_within`, capped ×3). Fine to tune in soak?
b. **Tour horizon** — cap tours at remaining-lifetime reach (natural) or also at a fixed stop
   count (e.g. 8) to bound the insertion search? Proposal: lifetime-only; the insertion loop is
   O(entries × scouts × tour-len) over ≤ ~150 entries and a handful of scouts.
c. **Numeric-bid visibility priorities** (full ADR 0040 unification of the priority tiers) — in
   scope now or a follow-up ADR?
d. **WFV batching** — D1+D4 want one WFV 27→28 reset. Batch with any other pending serialized
   work before the next MMO deploy, per the batch-if-ready convention?

## 5. Rollout

- **P1 (no serialization change)**: `ScoutAssignmentSystem` with per-source freshness heuristics
  (claim candidates 250, everything else TTL), observer rotation, tours, job simplification
  (job's serialized shape keeps `room_target` as a plain cache — no WFV), evidence-based
  unreachable NEW marks only.
- **P2 (WFV 28)**: `want_fresh_within` field, delete `MissionData::Scout`, poison-list
  re-validation migration.
- **P3**: EV spawn bidding + governor-bar change, then delete the interim heuristics.
- Verify on private soak first (offense-soak recipe), then MMO on explicit go-ahead. Success
  criteria: unreachable list shrinks below ~20 with no 1-hop entries; claim Select log shows
  candidates passing the commit freshness check within 2 cycles; a claim mission is created
  within 3 cycles of deploy at current GCL headroom.
