# Why the MMO bot stays locked in a 5-room cluster (2026-08-11)

Investigation of the operator question: *"why isn't the bot more aggressive in moving into other
rooms and increasing sprawl? It tends to become locked down in a cluster rarely making far in the
map."* Method: live MMO introspection (REST API: account, map-stats, memory segments, an offline
decode of the serialized world, console tail) cross-referenced with a 55-agent adversarially-verified
review of the expansion pipeline (42 confirmed findings, 6 refuted). Live payload snapshots and the
world decoder are described in §6.

## 0. TL;DR

The bot is **not** capacity-limited. At the time of investigation: GCL **11** with only **5** owned
rooms (six free claim slots), CPU cap allowing 9 rooms, governor tier `normal`, bucket ~9.5k, and
~20 unclaimed normal-status rooms inside the home sector. The claim pipeline runs every cycle and
creates **zero missions essentially every cycle**, because its input side — scout intel — is
structurally starved, and every commit gate keys off that starved intel. The historical CPU-bucket
drain (fixed 2026-07-28) explains the long frozen era; the intel-starvation squeeze explains why
expansion has not resumed since.

The squeeze in one sentence: **near rooms are deferred behind a "scouting coverage complete" flag
that can never become true, far rooms die on a 250-tick intel-freshness re-check that scouts can
never satisfy, and the two candidates that survive both die on missing room plans or denied
corridors.**

## 1. Live evidence (shardX, ticks ~4,392,187–4,392,700)

| Fact | Value | Source |
|---|---|---|
| GCL | 11 (99.4% to 12) | `/api/auth/me`, seg 57 |
| Owned rooms | 5: W11N52(7), W13N52(7), W12N51(6), W13N51(6), W16N51(4) | rooms API, map-stats |
| Reservations | 4: W12N52, W16N52, W11N51, W17N51 — all 1-hop | map-stats |
| Governor / bucket | `normal`, 9482–9527, trend ≈ 0 | seg 57 |
| `compute_maximum_rooms` | 9 (ema 57.2 CPU / 5 rooms ≈ 11.4/room) | seg 57 + code |
| Active claim missions | **0** | world decode |
| Claim phase | Idle, age 792 of interval-eff 840 | world decode |
| Candidates (scored) | 13 — see §1.1 | world decode |
| Unknown rooms | 21 | world decode |
| Scout give-up list | **103 rooms** in `unreachable` backoff, attempts up to 12 | world decode |
| Panics | 2687 cumulative, **flat** over 397 ticks (historical, drain-era) | seg 57 ×2 |
| Stale feature overrides | see §1.2 | `Memory._features` |

Neighborhood (map-stats, W4–W24 × N44–N60): the home sector still contains ~20 unclaimed
normal-status rooms, many adjacent to colonies (W14N52, W15N52, W17N52, W18N52, W15N51, W18N51,
W13–W17 N53/N54, …). The sector borders are novice/respawn-closed to the west (W4–W9), east
(W21–W24) and directly south (W11–W18 × N45–N49), so *long*-range sprawl is constrained by the
map — but in-sector infill is wide open and not happening. Neighbors: Robalian (24 rooms incl. a
large reservation ring), Jasper_185, Totalschaden (scattered RCL-8s), invader cores at W16N56 (L5).
W17N51 — our reserved room — had a live Invader presence (160 est. dps) during the snapshot.

### 1.1 The candidate list, annotated (from the live world decode)

```
W21N49  d=7 score=0.360  NO PLAN DATA          <- top candidate, deferred every cycle (RequestPlan)
W12N56  d=9 score=0.336  NO PLAN DATA          <- same
W11N56  d=8 score=0.297  plan VALID            <- all corridors denied (Robalian reservation ring) -> no eligible home
W15N51  d=3 score=0.289  NO PLAN DATA          <- below ring 4: deferred until "coverage complete" (never)
W18N52  d=3 score=0.229  plan VALID            <- below ring: deferred
W15N53  d=3 score=0.215  plan VALID            <- below ring: deferred
W15N52  d=2 score=0.141  plan VALID            <- below ring: deferred
W18N51  d=2 score=0.133  plan VALID            <- below ring: deferred
W17N52  d=2 score=0.131  plan VALID            <- below ring: deferred
W17N51  d=1 score=0.037  plan VALID  INVADER   <- below ring + threat-rejected
W16N52  d=1 score=0.036  plan VALID            <- below ring: deferred
W12N52  d=1 score=0.023  NO PLAN DATA          <- below ring: deferred
W11N51  d=1 score=0.018  plan VALID            <- below ring: deferred
```

Every candidate is blocked by one of exactly three mechanisms (§2). The give-up list contains
in-sector, trivially-reachable rooms — **W14N51 (1 hop from W13N51) is marked scout-unreachable**,
as are W16N53, W12N53, W11N53, W12N54, W14N54, W15N55, W13N55, W16N54, W18N54… with backoffs from
2k to 20k+ ticks. This is why the 2-source rooms adjacent to the cluster are not even candidates.

### 1.2 Stale `Memory._features` overrides (silent posture downgrade)

`features::load()` writes the fully-resolved feature struct back to Memory every tick, so values
persisted by an **older build permanently shadow retuned code defaults**. Live vs intended
(ADR 0038 D9 rapid-spread posture):

| Knob | Live (Memory) | Code default |
|---|---|---|
| `claim.max_concurrent_missions` | **2** | 4 |
| `claim.max_score_delta` | **0.15** | 0.35 |
| `claim.rediscover_ticks_per_room` | **10** | 4 |
| `claim.max_discover_interval` | **5000** | 1500 |

Effect: the discover→select cadence can stretch to ~6.5h real time per single Select shot (5000
ticks × ~4.5s), instead of the intended ~2h — and each shot claims at most 2 rooms in the best
case. The rapid-spread retune **never took effect on MMO**. (`remote_mine.reserve` = true —
verified not to be the zero-reservation cause.)

**Addendum (same session, ~90 min later):** `Memory._features.claim` now reads the code defaults
(4 / 0.35 / 4 / 1500) — changed mid-cycle with **no VM restart, no panic, no deser failure** in the
window (seg-57 counters flat). Either the operator applied the values at the console, or a
transient `_features` parse failure made `load()` fall back to `Features::default()` and write the
defaults back (that path leaves no fault counter — a config-integrity gap worth closing either
way: overrides can be silently wiped, and, symmetrically, stale values silently shadow retunes).
Natural experiment: the live Select captured in §5 ran WITH the correct rapid-spread posture
(`mission_cap=4` in its header) and still created **zero** missions — posture was never the binding
constraint; mechanisms M1–M4 are.

## 2. The live blocking mechanisms (all adversarially verified)

### M1 — Scout starvation with self-reinforcing poisoning (critical; the root)

* The claim pipeline re-upserts a **CRITICAL(100)** visibility request for **every cached unknown
  room every tick** of the scouting window, with no "already resolved" check
  ([claim.rs:362](../../screeps-ibex/src/operations/claim.rs)). Candidate-refresh requests only
  ever get HIGH(75) ([claim.rs:388](../../screeps-ibex/src/operations/claim.rs)).
* The scout target-picker is strictly priority-then-closest with **no freshness filter**
  (`best_unclaimed_for`, [visibilitysystem.rs:315](../../screeps-ibex/src/room/visibilitysystem.rs)),
  so all scouts converge on the CRITICAL flood — including rooms **already resolved this window**
  (their entries persist): a scout that just resolved room X re-claims X every tick. Scouts are
  *pinned*; candidates never get refreshed.
* Observers are pinned identically: `ObserverSystem` has no rotation and no already-fresh filter —
  k observers re-observe the same top-k CRITICAL entries forever
  ([visibilitysystem.rs:583](../../screeps-ibex/src/room/visibilitysystem.rs)).
* Only **3 scout missions empire-wide** (`MAX_SCOUT_MISSIONS`,
  [scout.rs:260](../../screeps-ibex/src/operations/scout.rs)); each mission's scouts pick targets
  globally, so a mission for room X can burn all its spawns on "defectors" that go elsewhere; after
  4 spawns with none delivering fresh intel the mission **marks X unreachable with exponential
  backoff** ([missions/scout.rs:195](../../screeps-ibex/src/missions/scout.rs)) — give-up ≠
  unreachable, but it is recorded as unreachable. Live result: the 103-room poison list including
  1-hop rooms.
* The ratchet: rooms whose intel ages past `VISIBILITY_TIMEOUT` (20k ticks ≈ 1 day) drop from
  candidate back to **unknown** and become BFS walls
  ([claim.rs:171](../../screeps-ibex/src/operations/claim.rs),
  [gather.rs:216](../../screeps-ibex/src/room/gather.rs)) — starvation grows the CRITICAL flood and
  shrinks/blinds the frontier. The discovered world *contracts* toward the cluster.
* The scout lane is also the **hardest CPU-gated lane** (spawn gated at `CpuBar::LowPriority`,
  bucket ≥ ~2500; reservers ≥ 2000) — during the whole drain era and its recovery, intel silently
  froze while economy ran. That is the historical entry into today's poisoned state.

### M2 — The coverage/freshness squeeze at Select (critical)

`run_select` fires once per cycle (usually on window **timeout**, not coverage) and then:

* Every candidate at distance < 4 is deferred by `may_claim_below_ring(distance, ring, covered)`
  — and `covered` (`scouting_coverage_complete`,
  [claim.rs:532](../../screeps-ibex/src/operations/claim.rs)) requires **every** viable candidate
  simultaneously fresh within **250 ticks** AND every unknown resolved-or-backed-off. With M1
  starving refreshes and 21 unknowns churning in/out of backoff, coverage essentially never
  completes at MMO scale ⇒ **near infill is permanently vetoed**.
* Every candidate at distance ≥ 4 must pass the commit-time re-check `is_claim_target_safe(...,
  intel_freshness_ticks=250)` whose first clause rejects intel older than 250 ticks
  ([utility.rs:19](../../screeps-ibex/src/missions/utility.rs)). The scouting window is 750–2500
  ticks and scouts don't service candidates (M1) ⇒ far rooms are almost never fresh at the single
  Select instant ⇒ skipped.
* The freshness gate is ordered **before** the plan-request gate, so a stale candidate never even
  gets its plan *requested* — though planning needs only static data.

### M3 — Plan starvation for the winners (critical)

Candidates that would survive M2 hit the plan gate:

* Claim plan requests live **exactly one tick** and the planner starts at most **one** plan while
  idle; requests are silently dropped if the planner is busy that tick ⇒ plan starts are capped at
  ~1 per discover cycle.
* Priority-tie inversion: all claim requests carry priority 0.5 and `max_by` returns the **last**
  max element ⇒ the planner plans the **worst-ranked** candidate first; the top candidate starves
  at `RequestPlan` for many cycles. (Live: top-2 candidates have no plan data while #6–#11 do.)
* A room whose plan ever **failed** is permanently excluded (the replan backoff is dead code for
  unowned rooms — no requester exists once the room is score-rejected).
* A single plan is hundreds of ticks of planner time (≈15 CPU/tick budget, beam-escalating DFS), so
  even a serviced request stretches a claim by whole cycles.

### M4 — Corridor-oracle asymmetry strands the rest (major)

The commit-time reach oracle (`economy_route_cost`,
[routepricing.rs:85](../../screeps-ibex/src/pathing/routepricing.rs)) **denies** any room with a
hostile reservation, an SK flag, or **any cached hostile-creep sighting — with no intel-age
bound** — while the discovery BFS happily traverses hostile-reserved rooms. Result: phantom
candidates (surfaced by BFS, uncommittable by oracle: live example W11N56 behind Robalian's
reservation ring) and corridors poisoned for up to ~20k ticks by one stale sighting that starved
scouting never refreshes.

### M5 — Secondary confirmed contributors

* **Claimer meat-grinder** on far targets: a claimer that ages out en route counts as a "death";
  2 deaths abort the mission and put the room in a 5000-tick avoid-cooldown; after cooldown the
  same doomed room can be re-selected (11-hop terrain-blind reach vs 600-tick CLAIM life).
* **Discovery freezes while ANY owned room is < RCL 2** — every successful claim serially pauses
  further expansion until the new colony reaches RCL 2; a spawnless-but-uncontested failed colony
  freezes it permanently (abandon only fires under sustained hostiles).
* **One claim mission consumes all its eligible home rooms** (`used_home_rooms`), collapsing
  mission concurrency to ~1/cycle in a tight cluster.
* **The tier veto is sampled on exactly one tick** per ~2000–4000-tick cycle
  (`cpu_healthy = tier == Normal` read inside `run_select` only) — one bad tick discards a whole
  scouted cycle.
* **Remote-mining/reservation sprawl is structurally capped at 1 hop** (hard-coded search radius),
  so "sprawl" beyond claims cannot come from outposts; invader reservations additionally veto
  outpost viability and nothing is tasked to clear them outside the 1-hop ring.
* **BFS truncates at 256 visited rooms** with a warn log — not binding today (34 tracked), but a
  ceiling on "spread across the map" scale once intel health is fixed.
* **Drain-era history**: below-Normal governor tiers hard-vetoed Select for months while GCL grew
  to 11 — the origin of the frozen cluster; confirmed recovered (tier `normal` live).

Refuted (do not chase): expired novice-zone walls via `RoomStatusCache` (rooms API refreshes),
the "scout bar > claim bar mis-ordering" framing (both gates are open at the live bucket), the
specific "MEDIUM outpost intel starved by CRITICAL flood" cross-corroboration as stated, the
freshness dead-zone no-re-request claim, and the 256-cap "can never spread" strong form.

## 3. Why this presents as "locked down in a cluster"

1. Drain era: all four CPU gates shut → zero expansion while GCL climbed to 11 and neighbors
   (Robalian, Jasper) expanded around us, hostile-reserving the corridors.
2. Post-fix: governor healthy, but M1's poisoned/starved intel means the pipeline sees a shrunken
   frontier (103 rooms "unreachable", 21 perpetual unknowns), M2 vetoes all near rooms and
   staleness-kills all far rooms, M3 starves the two survivors of plans, M4 denies the corridors of
   the third. Net: **0 claims per cycle at a ~2–6.5h cycle cadence**.
3. The stale Memory overrides (§1.2) halve concurrency, tighten the score-delta gate, and stretch
   cadence up to 3×.

## 4. Fix program (ranked by leverage)

1. **Un-starve scouting (M1)** — stop re-upserting CRITICAL for resolved unknown rooms (freshness
   check in `refresh_visibility_requests` and the Discover requests); add a freshness filter to the
   scout target-picker and observer selection (+ rotation); make scout give-up distinguish
   "defected/never-tried" from "tried and failed"; clear or rapidly decay the poisoned
   `unreachable` list; scale `MAX_SCOUT_MISSIONS` with empire size.
2. **Fix the Select squeeze (M2)** — align `intel_freshness_ticks` with the effective scouting
   window (or commit candidates event-driven as they *become* fresh instead of one sampled
   instant); request plans regardless of intel freshness (static data suffices); consider relaxing
   `covered` to per-candidate coverage rather than simultaneous-global.
3. **Fix plan starvation (M3)** — retain plan requests across ticks (queue, not one-shot); use the
   candidate score as the request priority (fixes the tie inversion too); prefetch plans for top-N
   candidates during Scouting, not at commit.
4. **Reconcile BFS and the reach oracle (M4)** — age-bound hostile-creep sightings; decide a single
   semantic for hostile-RESERVED corridors (passable-expensive for claim routing, or BFS-denied so
   phantom candidates die at Discover); at minimum make both sides agree.
5. **Config hygiene** — refresh the stale `Memory._features.claim` overrides to the rapid-spread
   defaults (console one-liner, no deploy); or add a versioned "posture" so retunes propagate.
6. **Secondary** — claimer TTL-expiry ≠ combat death; RCL-2 discovery freeze scoped to the claiming
   homes only; per-mission home consumption capped; sample `cpu_healthy` over a window rather than
   one tick; remote-mining radius configurable (>1 hop) once intel is healthy.

## 5. Live verification — a full Select captured (tick ≈ 4,393,450)

The predictions were confirmed verbatim by the next live Select cycle (console tail, `grep ClaimOp`):

```
ClaimOp [Select]: 16 candidates total, 0 unscored (pruned), 5 hostile (pruned), 11 remaining
  #1 W21N49 score=0.360 dist=7   #2 W12N56 score=0.336 dist=9   #3 W11N56 score=0.297 dist=8 plan=0.84
  #4 W15N51 0.289 d3  #5 W15N53 0.215 d3  #6 W15N52 0.141 d2  #7 W18N51 0.133 d2
  #8 W17N52 0.131 d2  #9 W16N52 0.036 d1  #10 W12N52 0.023 d1  #11 W11N51 0.018 d1
ClaimOp [Select]: owned=5 active_missions=0 max_rooms=10 available=5 mission_cap=4
                  at_capacity=false features.on=true cpu_healthy=true est_room_cpu=10.9
ClaimOp [Select]: candidate W21N49 failed commit-time safety re-check, skipping        <- M2-far (stale intel)
ClaimOp [Select]: candidate W12N56 failed commit-time safety re-check, skipping        <- M2-far (stale intel)
ClaimOp [Select]: top candidate W11N56 has no eligible home rooms (... not
                  claim-reachable through hostile-free corridors)                      <- M4 (corridor denial)
ClaimOp [Select]: candidate W15N51 at distance 3 < ring 4 deferred (waiting for
                  farther rooms; frontier not yet fully scouted)                       <- M2-near (x8, every
   ... same line for W15N53 W15N52 W18N51 W17N52 W16N52 W12N52 W11N51 ...                 below-ring candidate)
ClaimOp [Select]: had 11 scored candidates but created no missions
```

Notes: `cpu_healthy=true` and `available=5` on the header — capacity and governor gates open, zero
claims anyway. No `reachable ring covered` line anywhere in the ~2h tail (M1: coverage never
completes). The freshness skip on #1/#2 fires BEFORE the plan gate, so their plans were again not
even requested (finding M3/ordering). This cycle ran with the corrected rapid-spread posture
(`mission_cap=4`, §1.2 addendum) — posture is not the binding constraint.

## 6. Tooling added by this investigation

* **Offline world decoder** — `operations::claim::live_world_decode::decode_live_world`
  (`#[ignore]`d host test). Fetch segments 50–52 back-to-back (same tick — one curl process, else
  the gzip checksum fails), concatenate their `data` fields to a file, then:
  `IBEX_WORLD_PAYLOAD=<file> IBEX_NOW=<tick> cargo test -p screeps-ibex decode_live_world --
  --ignored --nocapture`. Prints the ClaimOperation phase/candidates/scores, claim/remote-build
  missions, and the visibility queue incl. the unreachable list, with plan-validity and threat
  annotations per candidate.
* Console tail: `screeps-rest-api/examples/tail.rs` (`--shard shardX`), log level is Info live —
  all ClaimOp Select gate lines are visible.
