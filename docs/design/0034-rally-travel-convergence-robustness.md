# ADR 0034 — Rally / Travel / Convergence Robustness (far-home convergence, rally-room selection, renew-in-transit)

Status: **Proposed** (2026-06-29)

> Filename note: the originating task named `0033-rally-travel-convergence-robustness.md`, but `0033` is
> already taken by [ADR 0033](0033-rover-pathing-sim-and-benchmark.md) (rover-pathing sim + benchmark). This
> ADR is therefore numbered **0034**.

Supersedes/extends the **K0 solo-travel + gather-quorum model** introduced in
[ADR 0028](0028-lifecycle-harness.md) (the engine-backed offline lifecycle harness — the "decouple TRAVEL
from FORMATION, each member paths SOLO to ONE shared rally" decision). Builds on
[ADR 0027](0027-objective-squad-lifecycle.md) (the objective/squad lifecycle: rally gate, forming/travel
lease refresh, reconcile kernel). Companion to [ADR 0030](0030-squad-composition-size-tuning.md) (sizing) and
[ADR 0032](0032-ev-optimal-squad-assignment.md) (assignment). Cross-refs the cohesion instrument
([`screeps-combat-decision/src/cohesion.rs`](../../screeps-combat-decision/src/cohesion.rs)) and the rally
kernel ([`screeps-combat-decision/src/rally.rs`](../../screeps-combat-decision/src/rally.rs)).

---

## 0. Problem statement (operator-flagged)

A squad fielded from **multiple home rooms** against a **far** target (e.g. homes W3N2 + W4N7, target W9N8)
**never converges and never reaches/engages**. It mills near home, fatigue=0, the squad's reported
distance-to-target `d` does not decrease ("stalled"), the gather quorum never fires, and after the
travel-budget elapses the squad gives up — re-field churn, no engage. The K0 solo-travel fix (ADR 0028)
**decoupled travel from formation** and the spatial harness driver proves the *idealised* convergence works,
but three production-only failure surfaces remain unproven offline and are the live root cause:

1. **The rally room is mis-computed for far/cross-quadrant scatter** — the centroid that feeds
   `shared_rally_point` is in the *wrong room*, so the rally lands off the approach path or behind the squad.
2. **There is no lifetime/renew awareness in transit** — early members age out while the roster forms or
   while a far member crawls to the rally; the rally is not guaranteed renewable.
3. **The production sim never exercises the real travel path** — `run_lifecycle_churn` / `run_v1_flow` /
   `run_offense_flow` model travel as a *pure tick counter*; only the isolated `run_lifecycle_churn_spatial`
   driver positions members, and even it uses an abstract `WPos` grid rather than the real `centroid()` /
   `shared_rally_point()` geometry. So the far-home stall is **not reproducible in the production drivers**.

The operator's standing requirement (load-bearing): *"fully simulated so we know the root causes and can be
sure we have fixed it."* This ADR's spine is therefore **SIM-FIRST**: extend the harness to reproduce each
failure RED *before* changing production code, then drive the fix to GREEN.

---

## 1. Consolidated, prioritized ROOT-CAUSE MAP

Each failure mode → root cause → `file:line`. Ordered by how directly it blocks far-home convergence.
Verified against the current tree (2026-06-29). Note one correction to the source investigations: the
production *proceed* and *gather→assault* gates are **P(win)-driven** (`present_force_wins_or_stalls`), not
purely composition-completeness — see RC-9.

### RC-1 — Centroid ignores room names (the rally is computed in the wrong room) — PRIMARY
- **Cause:** `centroid()` averages only the in-room `x`/`y` (0–49) and stamps the result into
  `positions[0].room_name()` — **no world-coordinate conversion**. For members at W3N2(25,25) + W4N7(25,25)
  it returns (25,25,**W3N2**), ~5 rooms from the true spatial centre.
- `screeps-combat-decision/src/cohesion.rs:24-39` (`centroid`)

### RC-2 — Rally uses the broken centroid as the approach, with no validation — PRIMARY
- **Cause:** the manager passes `decision.center` (derived from the broken centroid) to
  `shared_rally_point(approach, target, uncontested)` as the approach. For scattered members the approach
  room is meaningless, so the staging-room delta `approach.room_name() - target.room_name()` points the wrong
  way → the rally lands off the approach path (possibly *behind* the squad or on the far side of the target).
  There is no check that the rally is (a) between the scatter and the target, (b) reachable by *all* members,
  (c) strictly closer to the target than the furthest member.
- `screeps-ibex/src/military/squad_manager.rs:2092-2101` (centroid → `shared_rally_point`)
- `screeps-combat-decision/src/rally.rs:209-231` (`shared_rally_point` — no reachability/side validation)

### RC-3 — No member-side movement-failure feedback (silent NO_PATH retry loop) — PRIMARY
- **Cause:** `MoveToRoom::tick` issues `move_to(rally).range(1)` and `return None` with no inspection of the
  movement result. A blocked/NO_PATH/timeout path is silent; the manager re-issues the same `MoveTo(rally)`
  every tick. A member stuck behind impassable terrain / a hostile room never moves and never reports it.
- `screeps-ibex/src/jobs/squad_combat.rs:169-178` (`MoveTo(rally)` → no failure poll)
- `screeps-ibex/src/military/squad_manager.rs:2152-2164` (re-issues `MoveTo(rally)` every tick, no feedback)

### RC-4 — `target_dist` is the MIN over members (one stuck member hides progress) — HIGH
- **Cause:** the squad's travel-progress signal is `min(room_distance(member, target))`. A single stuck
  member (or a stuck *lead*) pins the minimum flat even while others advance; conversely a moving lead masks
  a stuck bulk. `travel_progress` then mis-reads, and the trace logs "stalled" regardless of per-member
  motion.
- `screeps-ibex/src/military/squad_manager.rs:2233-2236` (`target_dist = …min()`)
- `screeps-ibex/src/military/squad_manager.rs:1127-1135` (`travel_progress` = min strictly decreased)
- `screeps-ibex/src/military/squad_manager.rs:2282-2293` ("stalled" log on `!closing`)

### RC-5 — Renew gate is FORMING-ONLY; no renew in transit or at the rally — HIGH
- **Cause:** Phase B-renew `continue`s the moment `filled_slot_count() >= requested` — a *full* squad gets no
  renew, no matter its phase. Members holding at the rally (HOLD orders) or crawling to it (MoveTo orders) are
  not renewed. A far member burns a large fraction of its TTL in transit and arrives near death; a slow form
  loses early members to age.
- `screeps-ibex/src/military/squad_manager.rs:1361-1362` (`>= requested { continue }`)
- `screeps-ibex/src/military/squad_manager.rs:2066-2069` (RALLY/HOLD orders — no renew pass)

### RC-6 — Rally point is not guaranteed renewable — HIGH
- **Cause:** `shared_rally_point` returns a *room centre* (25,25) — target-room centre (uncontested) or the
  approach-side neighbour centre (contested). Neither is spawn-aware. The Phase B-renew comment claims "the
  rally point is a home spawn", but the kernel does not place it at one. A member holding at a non-renewable
  rally just ages.
- `screeps-combat-decision/src/rally.rs:209-231` (room-centre staging, spawn-blind)
- `screeps-ibex/src/military/squad_manager.rs:1348-1349` (stale "rally is a home spawn" comment)

### RC-7 — No pre-departure lifetime gate — MEDIUM
- **Cause:** members are committed to `MoveTo(rally)` with no check of `ticks_to_live()` vs
  (distance-to-rally + distance-to-target + fight buffer). A member that cannot survive the journey is sent
  anyway, then dies in transit → roster drops → quorum oscillates (RC-9 interplay).
- `screeps-ibex/src/military/squad_manager.rs:2159-2163` (orders issued, no TTL check)

### RC-8 — Travel-budget exhaustion masquerades as give-up — MEDIUM
- **Cause:** the travel lease refreshes only while `travel_progress` (min distance strictly decreasing,
  RC-4). When far members crawl but the *min* is pinned by a stuck/holding member, `travel_progress=false`,
  the lease lapses at `departed_at + MAX_TRAVEL_BUDGET (1000)`, and the squad gives up while members are still
  pathing.
- `screeps-ibex/src/military/squad_manager.rs:172` (`MAX_TRAVEL_BUDGET = 1000`)
- `screeps-ibex/src/military/squad_manager.rs:1118-1135` (`departed_at` + `travel_progress` bound)
- `screeps-combat-decision/src/lifecycle.rs:177` (`traveling_progressing` gate)

### RC-9 — Gather→assault quorum can oscillate without the latch (CORRECTED framing) — MEDIUM
- **Cause / correction:** the production proceed gate is `present_wins_or_stalls || ready_to_depart_gate(...)`
  and the gather→assault gate is `present_wins_or_stalls || gather_quorum_met(...)` — **P(win)-driven**, not
  pure composition count (the source investigations under-stated this). The latch (`assault_latched`) holds
  the assault once fired so in-transit deaths don't un-commit it. BUT a wrong rally (RC-1/RC-2) means the
  quorum never fires *at all*, so the latch never engages and the win-or-stall path is the only escape — and
  a scattered, never-massed force does not win-or-stall. The oscillation BUG A is real but secondary to the
  rally being wrong in the first place.
- `screeps-ibex/src/military/squad_manager.rs:2054` (proceed gate, P(win)-driven)
- `screeps-ibex/src/military/squad_manager.rs:2129-2146` (gather→assault gate + `assault_latched`)
- `screeps-combat-decision/src/rally.rs:171-194` (`gather_quorum_met` kernel)

### RC-10 — Quorum can never fire when no member reaches the rally — DERIVED
- **Cause:** `members_gathered_at` counts only members within `RALLY_GATHER_RADIUS` of the rally. If the rally
  is unreachable/wrong (RC-1/2/3), `gathered = 0` forever → `gather_quorum_met` false → assault never advances
  → the `MoveTo(rally)` orders re-issue indefinitely (until RC-8 retires the squad). This is the *symptom*
  that surfaces all of the above.
- `screeps-combat-decision/src/rally.rs:155-157` (`members_gathered_at`)
- `screeps-ibex/src/military/squad_manager.rs:2148-2165` (assault vs solo-travel branch)

### RC-11 — Premature assault latch on a VACUOUS win → cross-room formation freeze — **THE TRUE LIVE BLOCKER** (found 2026-06-29 re-soak)
- **Cause:** an **unscouted** target room → `build_room_combat_dtos` returns EMPTY hostiles+structures →
  `assess_engage` sees `killable_dps=0, tower_dps=0` → `unwinnable=false`, enemy strength ≈ 0, our strength > 0
  → balance clamps to +1000 → **`present_force_wins_or_stalls` returns TRUE against zero visible enemies (a
  *vacuous* win).** That makes `ready_to_depart` **and** `quorum_now` true *regardless of co-location* → the
  squad latches the assault → builds a formation anchor at the **first living member's room** → stamps
  `TickMovement::Formation` on every member → a member in a *third* room hits `cross_room_formation_target`'s
  edge/own-tile hold → `move_to(own_tile)` → rover `Ok(None)` → `Arrived` (**not** a failure) → **frozen, with
  no `MOVE-BLOCKED`.**
- **Conditional on co-location at the latch:** a co-located squad takes the same-room formation branch and
  reaches (Entity 100 → W4N7); a scattered squad (Entity 414: W9N8/W7N4/W2N5) freezes. This is why some
  squads reached and most didn't — and why RC-1/RC-2/RC-3 fixes (the decision layer) couldn't help: the
  members were never moving. **The eval solo-stepper cannot see this** — it assumes moves execute and feeds
  non-empty DTOs.
- `screeps-ibex/src/military/squad_manager.rs` (~2135 `present_wins_or_stalls`, ~2286 `quorum_now`, ~2301
  latch), `screeps-combat-decision/src/lib.rs:~1430` (`present_force_wins_or_stalls`, vacuous-win), `military/formation.rs:~235`
  (`init_squad_path_if_needed` arbitrary-first-member anchor), `jobs/squad_combat.rs:~1043` (`cross_room_formation_target` edge-hold).
- **FIX (D9, ✅ DONE — decision + super, no WFV bump):** gate the win-or-stall fast-path on **real target
  intel**: `winnable_fast_path_allowed(present_wins_or_stalls, have_target_intel)` where
  `have_target_intel = !hostiles.is_empty() || !structures.is_empty() || intel_source == LiveVisible`, applied
  at **both** `ready_to_depart` and `quorum_now` (via `squad_is_gathered`). An unscouted "win vs zero enemies"
  falls back to the **count-quorum** (members *mass* at the rally via solo-travel) before any formation
  assault; the fast-path re-enables the moment real DTOs arrive (on arrival the DTO is `LiveVisible`, so no
  deadlock). **Keyed on `== LiveVisible`, NOT `is_reliable()`** — an empty-*Cached* room is itself the vacuous
  case (scouted-empty earlier, could now hold a fresh core). Defense-in-depth: `init_squad_path_if_needed`
  anchors on the destination-nearest (lead) member when scattered, not the arbitrary first. **Refines (not
  contradicts) the P(win)-driven-gating directive — a vacuous no-intel win isn't a real Lanchester P(win).**
  Proven by pure unit tests (trigger: empty-DTO → vacuous-win=true + gated-predicate=false; branch:
  scattered+no-intel → solo-travel, co-located+intel → assault).

**Priority chain:** **RC-11 (premature latch on a vacuous win → formation freeze) is the TRUE live blocker** —
it kept squads from moving at all, *upstream* of everything else. RC-1 → RC-2 (wrong rally) is the headline of
the *decision* layer (real bugs, but downstream of RC-11). RC-3 (no feedback) + RC-4/RC-8 (min-distance
progress + budget) turn a wrong rally into a *silent permanent stall* rather than a loud, recoverable one.
RC-5/RC-6/RC-7 (renew/lifetime) are the slow-form / far-travel attrition axis. RC-9/RC-10 are downstream
symptoms.

---

## 2. SIM-COVERAGE GAP — what the sim reproduces vs MISSES (load-bearing)

### 2.1 What IS proven today
- **Pure kernels** (`rally.rs` unit tests): rally geometry one-room-short, gather-quorum math, intel-reliability
  (the oscillation fix), the `scattered_members_converge_at_shared_rally_then_assault_advances` test
  (`rally.rs:389-454`) — but it hand-rolls a local `step_toward` over **adjacent** rooms (W2N9/W3N2→W4N2) and
  asserts the *idealised* convergence; it does NOT call the production `centroid()` to derive the approach.
- **`reconcile` kernel** (`lifecycle.rs` tests): forming/travel/holding/declaim lease refresh + give-up bounds.
- **`run_lifecycle_churn_spatial`** (`harness/lifecycle.rs:363-643`): THE only driver with per-member
  positions (`WPos` grid), solo travel to a shared rally, per-tick `gather_quorum_met`, FIX-A latch toggle,
  enemy-held attrition, BUG-A oscillation counter. It proves the latch fixes contested oscillation.
- **`run_lifecycle_churn`** (`:702`): the REAL `reconcile` kernel + forming + age-out, but **travel is a pure
  tick counter** (`Phase::Traveling { arrives_at }`).
- **`run_v1_flow`/`run_offense_flow`** (`:1110`/`:1437`): multi-objective + reassign, **travel = tick counter**.

### 2.2 What the sim MISSES (the gaps that hide the far-home stall)
| Gap | Where | Consequence |
|---|---|---|
| **G1 — real centroid/rally geometry** is never exercised. The spatial driver uses `WPos` grid + `travel.rally` *given*; the rally unit test hand-rolls a local stepper. Neither calls `cohesion::centroid()` then `rally::shared_rally_point()` over real cross-quadrant rooms. | `cohesion.rs:24`, `rally.rs:209`, harness `WPos` model | **RC-1/RC-2 (the headline) are completely invisible offline.** The wrong-room rally is never computed in a test. |
| **G2 — far/cross-quadrant scatter** (5+ rooms apart, asymmetric: one member near target, one far) | rally tests cover only 1–2-room clusters | The very geometry that breaks the centroid is untested. |
| **G3 — production drivers don't position members.** `run_lifecycle_churn`/`_v1_flow`/`_offense_flow` use a tick counter for travel. | harness `:770-821`, `:1110`, `:1437` | The far-home stall is **not reproducible in the production path**; only the isolated spatial driver positions members. |
| **G4 — TTL burn in transit + renew-at-rally**. The spatial driver tracks positions but NOT TTL/renew; `run_lifecycle_churn` tracks TTL age-out but renews only at home in `run_forming`, never at the rally. | harness has no rally-spawn model | **RC-5/RC-6/RC-7 unprovable** — no test shows a far member arriving near-death or a renew topping it up. |
| **G5 — member-side movement failure**. No driver models a blocked/NO_PATH member; all assume a clean `step_toward`. | harness steppers always advance | **RC-3 unprovable** — the silent retry loop is never reproduced. |
| **G6 — per-member progress** vs the min-distance signal. The sim measures `min(room_dist)` like production; no test shows one stuck member pinning the squad's progress flag. | harness `cur_dist = …min()` (`:493`) | **RC-4/RC-8 interplay** (a moving bulk reported "stalled") unprovable. |
| **G7 — engine cross-room movement.** `screeps-combat-engine` resolves moves in a SINGLE room; room-edge crossing is "Not modelled yet". | `screeps-combat-engine/src/movement.rs`, `resolve.rs` header | Full engine-backed multi-room travel impossible (acceptable — the harness can stay pure-kernel for travel; engine stays for in-room combat). |

### 2.3 EXACTLY what must be ADDED to make everything provable
The minimum additions to reproduce **far-home convergence stall + rally-room selection + renew-in-transit**:

1. **A real-geometry rally test (closes G1/G2).** In `rally.rs` tests, add a driver that, given member
   `Position`s in real cross-quadrant rooms, computes the approach via `cohesion::centroid()` and the rally
   via `rally::shared_rally_point()`, then asserts:
   (a) the rally room is **not** behind the scatter, (b) is strictly **closer to the target** than the
   furthest member, (c) is **on the approach line** (delta sign matches member→target), (d) both members
   converge within a bounded tick count. This is RED today (centroid returns the wrong room) and GREEN after
   the world-coordinate centroid + scatter-robust approach (§3).

2. **`run_lifecycle_churn_extended` — fold spatial positioning into the production driver (closes G3/G6).**
   Give `run_lifecycle_churn` per-member `Position`s (real room names, not `WPos`), seed each at a distinct
   home, derive the rally via the **production** `centroid`+`shared_rally_point`, step each member SOLO toward
   the rally, re-eval `gather_quorum_met` every tick with the FIX-A latch, and gate `Phase::Arrived` on
   `gathered` rather than a bare `arrives_at`. Track BOTH `min` and per-member distances so RC-4/RC-8 can be
   asserted. This makes the far-home stall reproducible on the production path.

3. **Renew/TTL-in-transit model (closes G4/RC-5/RC-6/RC-7).** Add `member_ttl` decay per travel tick, a
   `rally_spawn` flag + energy on the scenario, and a renew pass that fires when a holding/near-rally member's
   TTL < `RENEW_WHILE_FORMING_TTL` and a rally/home spawn is reachable. Add a **pre-departure lifetime gate**
   model: a member whose `ttl < dist_to_rally + dist_to_target + FIGHT_BUFFER` either holds for a renew or is
   recycled. RED pre-fix (far member arrives dead / slow form churns); GREEN post-fix (renew tops it up).

4. **Blocked-path model (closes G5/RC-3).** Add an `impassable`/`enemy_held` set the solo stepper checks: a
   member whose only path to the rally is blocked does NOT advance and surfaces a `Blocked` signal the driver
   reads to escalate (fallback rally / direct assault / abort). RED pre-fix (silent infinite retry → budget
   lapse → GaveUp); GREEN post-fix (detected + escalated within a bounded stall window).

5. **Three named scenario families** (wired into `param_sweep` gates so a knob set is graded on convergence,
   not just oracle calibration):
   - **S1 FAR-HOME CONVERGENCE STALL** — homes far/cross-quadrant, target 5+ rooms away; expect pre-fix
     `OscillatedNeverGathered`/`LapsedInTravel`, post-fix `DeployedAndEngaged`.
   - **S2 CONTESTED OSCILLATION** — multi-home, enemy-held neighbours, latch off→on; pre-fix oscillation,
     post-fix engaged (the latch already lands this in the spatial driver; promote it to the production path).
   - **S3 RENEW-IN-TRANSIT** — far homes, slow form, finite TTL; pre-fix `ChurnedNeverDeployed`, post-fix
     `DeployedAndEngaged` (renew keeps the early roster alive).

Engine cross-room movement (G7) is **explicitly out of scope** — the harness stays pure-kernel for travel and
engine-backed only for in-room combat. This is the cheapest path to full provability and matches ADR 0028.

---

## 3. ROBUST DESIGN DIRECTION

Principles: **good rally selection (reachable, on the approach, min convergence travel, renewable)**;
**far-home convergence that cannot silently stall**; **lifetime-aware deployment**; **bounded leashes**.

### D1 — World-coordinate centroid (fixes RC-1)
Convert each member position to world coords (`room → (wx,wy)*50 + (x,y)`), average in world space, convert
back to room + in-room offset. The true spatial centroid regardless of room boundaries. `cohesion.rs:centroid`
becomes correct for the cohesion *measure* AND for the rally approach.

### D2 — Scatter-robust approach selection (fixes RC-2)
Add a scatter check: when members span multiple rooms (`max_pairwise` high or centroid room ≠ furthest
member's room), derive the rally approach from the **furthest member's room toward the target**, not the
centroid. This biases the rally onto the actual approach corridor and guarantees the staging room is on the
direct path for the laggard (the member that actually gates convergence).

### D3 — Rally placement validation (fixes RC-2/RC-6/RC-10)
After computing `staging_room`, validate it: (a) on the line between the furthest member and the target (not
behind), (b) strictly closer to the target than the furthest member's approach, (c) not a wall/inaccessible
room, (d) **prefer a room within range of a friendly spawn** when one exists near the corridor (renewable
staging). Fail any → fall back to a conservative on-corridor neighbour. The placement becomes
reachability-and-renew aware, not pure geometry.

### D4 — Member-level path-failure detection + escalation (fixes RC-3)
In `MoveToRoom::tick`, after `move_to(rally)`, poll the movement result (`check_movement_failure`). On
`Blocked`/`NoPath`/`StuckTimeout`, surface a greppable per-member `[SquadTrace] MOVE-BLOCKED` signal — no
more *silent* retry loop (the job still re-issues the move, harmlessly; the escalation decision is the
manager's, single owner). The manager tracks each present member's room-distance to the rally; a member whose
distance stops decreasing increments a per-member stall counter, and past the bounded `SOLO_TRAVEL_STALL_WINDOW`
(D8) the manager **re-assesses that member OUT of the gather quorum** — it is dropped from the quorum
denominator (`effective_slots`) and the `gather_positions`, so the **reachable subset masses and the contested
quorum fires** rather than waiting forever on a member that cannot path.

> **AS-BUILT (decision corrected during implementation):** the escalation is **quorum exclusion of the blocked
> member, not a rally recompute.** The original sketch above ("recompute a fallback rally / go direct") was
> *not* built — the verifier confirmed the production rally is geometry-stable (`shared_rally_point_for_members`
> returns the same room with or without the blocked laggard in the set), so moving it would not help; dropping
> the blocked member from the quorum is the mechanism in **both** the sim and the live bot. Driven by
> independent position-stagnation (not the job's failure signal), self-correcting (a member that makes any
> progress re-enters next tick), contested-only, never strands the last present member.
>
> **KNOWN GAP → follow-up (Phase 1.5 / objective layer):** exclusion cannot help a **single-member or
> fully-blocked** squad — by design it never strands the last member, so a lone frozen member (or a squad
> where *every* member is genuinely unreachable, e.g. no route to the target at all) is **detected** (the
> `MOVE-BLOCKED` signal + the stall counter) but **not resolved**; it waits out `MAX_TRAVEL_BUDGET` and gives
> up. The robust handling of a genuinely-unreachable target is an **objective-level** decision (abort /
> mark-unreachable / reassign / reroute), tracked as a Phase 1.5 follow-up — *not* a rally/convergence concern.
> The live soak's frozen `1/1` squad (member stuck in W2N5) is this case.

### D5 — Per-member + majority travel progress (fixes RC-4/RC-8)
Replace the single `min`-distance progress signal with per-member tracking. Refresh the travel lease while a
**majority** of present members are closing distance (`closing * 2 > counted`, not while the single closest
is), so one straggler neither pins the squad "stalled" nor (conversely) one moving lead masks a stuck bulk.
In-target (`d = 0`) and a member's first reading count as closing; the signal is empty pre-departure so it
cannot misfire while forming. The trace reports per-member `d` and progress.

> **Sim/live signal nuance (as-built):** the live D5 majority signal keys on per-member distance-to-**target**
> (`squad_manager.rs`, spanning both the travel-to-rally and assault legs); the sim's solo-travel D5 signal
> keys on distance-to-**rally** (`harness/lifecycle.rs`). Same intent (is the present bulk progressing?) and
> both kill the min-pinning — but they are not byte-identical during the rally-approach leg. Acceptable: the
> sim proves the *mechanism* (majority-not-min), not an exact per-tick replica of the live distance metric.

### D6 — Lifetime-aware staging + renew-in-transit (fixes RC-5/RC-6/RC-7)
- **Pre-departure lifetime gate:** before committing a member to travel, check
  `ttl > dist_to_rally + dist_to_target + FIGHT_BUFFER`. If not: defer (hold for a fresh respawn of that slot)
  for low-urgency squads, renew-at-home for time-critical ones, or recycle if hopeless.
- **Renew while holding/rallying:** extend Phase B-renew past the `filled >= requested` gate — renew present
  members with low TTL that are holding at the rally OR at home next to a free spawn, gated on the existing
  free-spawn + room-energy checks. Self-healing, no new infra.
- **Renewable rally (D3 (d)):** bias the staging room toward a friendly spawn so the renew has somewhere to
  fire.

### D7 — Keep the per-tick fresh re-derivation + the assault latch (preserve RC-9 fix)
Keep the rally re-derived fresh each tick (no stored field → no `WORLD_FORMAT_VERSION` bump — preserves the
ADR-0028 K0 property) and keep `present_force_wins_or_stalls` as the primary proceed/assault gate. Keep the
`assault_latched` latch so an in-transit death never un-commits an assault. D1–D3 make the quorum *able* to
fire; the latch keeps it fired.

### D8 — Bounded leashes (preserve give-up safety)
Keep `MAX_TRAVEL_BUDGET`/`MAX_FORMING_BUDGET`/`COMMITMENT_BUDGET` as the absolute bounds. Add a tighter
**per-member solo-travel stall window** (e.g. ~50–150 ticks with zero members ever gathered) that escalates
(D4) before the coarse 1000-tick travel budget — so a wrong/blocked rally is caught and recovered fast, not
1000 ticks later.

---

## 4. SIM-FIRST PHASED PLAN (RED → GREEN)

The plan proves the root cause *before* the fix: each phase first extends the sim to reproduce the failure
RED (demonstrating we understand the cause), then lands the robust fix to GREEN. Production code is touched
only after the corresponding RED sim exists.

### Phase 0 — Real-geometry rally repro (RC-1/RC-2, sim gap G1/G2) — ✅ DONE (decision `5bb7666`, super `36bb340`)
- **RED:** add the real-geometry rally test (§2.3.1) over far cross-quadrant + asymmetric scatter, calling the
  production `cohesion::centroid` + `rally::shared_rally_point`. Assert it FAILS today (wrong room / behind
  the squad). This is the cheapest, most surgical RED and proves RC-1/RC-2 directly.
- **GREEN:** land D1 (world-coord centroid) + D2 (scatter-robust approach) + D3 (placement validation). The
  test goes GREEN.
- **AS-BUILT:** RC-1 (centroid) and D2/D3 (scatter-robust rally) split into independent tests + two
  discriminating geometries (legacy-vs-new genuinely differ), each RED-able by reverting the respective fix.
  **D1 (the centroid) alone fixes the headline far-home stall** — for the W3N2+W4N7→W9N8 geometry the legacy
  rally is insensitive to the centroid error (one room out by sign), so D2/D3 is *separate* robustness for
  scatter geometries where the centroid bearing ≠ the laggard bearing. No WFV bump. Live-deployed; soak showed
  reach jump from 0 → 20 `in_room=true`, confirming the geometry fix.

### Phase 1 — Production-path far-home stall repro (RC-3/RC-4/RC-8/RC-10, sim gap G3/G6) — ✅ DONE (eval + super, no WFV bump)
- **RED:** build `run_lifecycle_churn_extended` (§2.3.2) — per-member real `Position`s, production rally
  geometry, solo step, per-tick gather + latch, `Arrived` gated on `gathered`, per-member + min distance.
  Reproduce S1 (far-home stall) as `OscillatedNeverGathered`/`LapsedInTravel`. Add the blocked-path model
  (§2.3.4) to reproduce RC-3 as a silent budget-lapse give-up.
- **GREEN:** land D4 (member-side failure detection + escalation) + D5 (majority progress) + D8 (tighter stall
  window). S1 → `DeployedAndEngaged`.
- **AS-BUILT:** a differential toggle matrix proved orthogonality — **S1-clean is a D5-only bug** (held lead
  pins the min, masking the closing bulk; lapses on the commitment lease, not budget exhaustion) and
  **S1-blocked is closed *only* by D4+D8 together** (D5/D4/D8 alone each still lapse), pinned by
  `far_home_s1_blocked_d5_alone_still_lapses`. D4 escalation = quorum **exclusion** of the blocked member (see
  §3 D4 AS-BUILT, *not* a rally recompute). Trackers are transient in `SquadFormingProgress` (Default resource,
  self-healing on reload) → no WFV bump. **Known gap:** single-member / fully-blocked unreachable target →
  Phase 1.5 (objective-level abort), see §3 D4.

### Phase 2 — Renew-in-transit repro (RC-5/RC-6/RC-7, sim gap G4) — ✅ DONE (decision `4f72088`, eval `ec23ee2`, super, no WFV bump)
- **RED:** add the TTL/renew-at-rally model (§2.3.3). Reproduce S3 (slow far-home form) as
  `ChurnedNeverDeployed` (early members age out) and a far member arriving below the fight buffer.
- **GREEN:** land D6 (lifetime gate + renew while holding/rallying + renewable-rally bias from D3). S3 →
  `DeployedAndEngaged`.
- **AS-BUILT:** one shared pure kernel `rally::lifetime_sufficient_for_deployment(...) → CommitDecision`
  (Commit / RenewThenCommit / Recycle) drives both the bot and the sim. D6a (RC-7): solo-travel reads the
  live `ticks_to_live` and `Hold`s a would-die-en-route member at its home spawn. D6b (RC-5): the Phase-B
  renew drops the forming-only gate and renews any present member at a home room with `ttl<300`, reusing the
  existing energy-gated `request_renew` (non-griefing — verified bounded, no spawn monopolization, departed
  members structurally excluded). Both legs independently load-bearing (gate-alone and renew-alone each still
  lapse S3; `far_home_s3_gate_without_renew_still_fails` pins it). D6c renewable-rally bias is **sim-only** on
  the bot (home-renew carries S3) — documented follow-up. **Follow-ups (in-code):** F1 the `Recycle` verdict
  currently `Hold`s (torn down by MAX_TRAVEL_BUDGET, not an explicit recycle job); F2 `RALLY_TRAVEL_PER_ROOM`
  is a flat 50t (swamp/fatigue could arrive a member slightly short — graceful, conservativeness-tuning). No
  WFV bump (live ttl read per-tick; Hold/renew verdict ephemeral in `tick_orders`/`request_renew`).
- **⚠ NOTE (sim-world caveat):** Phase 2 (like Phase 1) is proven in the harness's "moves execute" world. The
  live re-soak after Phase 1 surfaced **RC-11** (see §3) — spawned scattered members *freeze* before any of
  this matters — which the solo-stepper harness cannot see. RC-11 is the true live blocker; Phase 2's renew
  only matters once members actually move.

### Phase 3 — Contested oscillation on the production path (RC-9, sim gap)
- **RED:** promote S2 (contested oscillation) into `run_lifecycle_churn_extended` with the latch toggled off;
  assert oscillation (the spatial driver already proves this in isolation — this proves it on the production
  path).
- **GREEN:** confirm D7 (latch + win-or-stall gate) lands S2 as `DeployedAndEngaged` with zero oscillations.

### Phase 4 — Wire into the param-sweep gates
- Add S1/S2/S3 to `ParamScore` (`param_sweep.rs`): `gates_held = false` if any family misses its expected
  outcome. The knob sweep is then graded on **full lifecycle convergence** (multi-home form → rally → travel →
  engage), not just oracle FP/FN calibration. This makes the fix permanently regression-fenced offline.

### Out of scope
Engine cross-room movement (G7) — the harness stays pure-kernel for travel. Revisit only if combat-aware
travel through hostile territory needs engine-proof fidelity (a NICE-TO-HAVE, not a blocker).

---

## 5. Consequences
- **No `WORLD_FORMAT_VERSION` bump** expected: the rally is re-derived fresh each tick (D7), the centroid fix
  is pure math, and the renew/lifetime gates add no serialized state (ephemeral per-objective trackers, like
  `assault_latched`). Confirm at implementation time per the standing WFV discipline.
- The fix is provable offline end-to-end (the operator's load-bearing requirement) before any live deploy.
- Risk: D2's "furthest-member approach" can over-bias the rally toward a single far outlier — bounded by D3's
  validation (must stay closer to the target than the furthest member) and D8's escalation.

## 6. Cross-references
- [ADR 0028](0028-lifecycle-harness.md) — K0 solo-travel + gather-quorum model (extended here).
- [ADR 0027](0027-objective-squad-lifecycle.md) — rally gate, forming/travel lease, `reconcile` kernel.
- [ADR 0030](0030-squad-composition-size-tuning.md) / [ADR 0032](0032-ev-optimal-squad-assignment.md) — sizing/assignment.
- Code: `cohesion.rs:24-39`, `rally.rs:155-231`, `squad_manager.rs:1345-1381` + `2054-2197` + `2233-2236`,
  `squad_combat.rs:114-225`, `lifecycle.rs:158-236`, `harness/lifecycle.rs:363-961`.
