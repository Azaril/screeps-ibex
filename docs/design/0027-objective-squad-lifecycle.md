# ADR 0027 — Combat Objective/Squad Lifecycle Rework (P-OBJ #23)

Status: IMPLEMENTED 2026-06-27 (super `e4bbf0f`; WFV 17→18); **EXTENDED 2026-06-28 —
reach/engage hardened end-to-end across a live soak (see "Update 2026-06-28" below).**
Companion to ADR 0008 (squad lifecycle) and ADR 0026 §9 (doctrine selection/sizing).
Task #23 / #25.

## Problem (Docker soak, 2026-06-27)

A force-sized offense squad "did nothing / scattered" against level-0 invader cores.
Root cause was a **three-system fatal coupling**, not a tactics bug:

1. Remote-core intel goes **stale ~every 200t** (no creep keeps eyes on the room) →
   the `war.rs` producer hits its stale gate (`war.rs:720`) and **stops re-pushing**
   the objective.
2. The objective **TTL-lapses** (`OFFENSE_OBJECTIVE_TTL=100`) — *shorter than the
   ~150–250t a squad needs to form + travel*.
3. The `SquadManager` retires the **still-forming** squad on `objective_gone`
   (`squad_manager.rs:241`), reading nothing about squad `state` → members orphan and
   **scatter**, often stranded mid-cross-edge (the "stuck on a room edge" reports).

Squad survival was coupled 1:1 to intel freshness. The offense was a spawn → orphan →
die conveyor that cleared nothing.

## Design

Reuse the **already-serialized but dormant** `CombatObjective.deadline: Option<u32>`
(was written by `request`, never read) as a manager-owned commitment lease.

### (a) Commitment — `objective_queue.rs`, `squad_manager.rs`
- `expire()` keeps an objective past its TTL while it is **CLAIMED** (`claimed_by`,
  a within-session resource) OR its **`deadline`** lease is in the future (serialized,
  bridges a VM reset / the cross-system ordering gap). Dies only on explicit
  `withdraw()` or once both lapse. (+`set_deadline`.)
- `field_new_squad` stamps `deadline = now + COMMITMENT_BUDGET (400)`; Phase A
  **refreshes** it every tick the squad has a `focus` (actively closing/fighting), so
  a long clear or a brief vision gap never drops the objective.

### (b) Resolve vs give-up — `squad_manager.rs` Phase A (new `SquadContext.engaged_once`)
`engaged_once` latches on the first `Engaged` tick — the signal that distinguishes a
squad that *fought and cleared* from one *just arriving* (Phase A runs before Phase B2
sets the focus) or *stuck en route*.
- **RESOLVE**: `engaged_once && in-target-room && no-focus` → target cleared →
  `withdraw()` the objective (clean win, no backoff) and retire.
- **GIVE-UP**: `deadline` lapsed with no focus and no clean clear → stuck/abandoned →
  `mark_unwinnable` (non-Defend) so we don't immediately re-field into a dead end.

### (c) Intel coverage — `squad_manager.rs`
For every live objective the manager pins **OBSERVE-only, HIGH** visibility on its
room, so an in-range RCL8 observer keeps `last_seen` fresh for free (no scout burned on
a walled target). Commitment + the lease cover rooms with no in-range observer.

### (d) Zero-orphan recall — `jobs/squad_combat.rs`
A retired squad's surviving members **recall themselves**: in the orphan fallback
(squad gone — `get_squad_state == None` — and nothing to fight) a combat creep moves to
the nearest home spawn and **recycles** instead of idling/scattering.

## As-built vs the 6-step plan

| Plan step | Outcome |
|-----------|---------|
| 1 expire immunity | **Built** (+2 unit tests) |
| 2 deadline heartbeat | **Built** (+ resolve/give-up via `engaged_once`) |
| 3 producer re-assert from last-known | **Subsumed** by (1): a claimed objective can't lapse underneath its squad, so producer silence on stale intel is already harmless. No dead code added. |
| 4 intel coverage pin | **Built** (OBSERVE/HIGH for live objectives) |
| 5 manager-side member cleanup + integrity sweep | **Revised**: `EntityCleanupQueue::delete_creep` only deletes the ECS entity (a live creep is re-discovered next tick), so disposing a live member must be an **in-game** action by its own job → moved to (d). `retire_squad`'s raw squad-entity delete stays (generation-safe `SquadRef` → an orphan resolves to `None`, never aliases); the existing `repair_entity_integrity` member-scrub + Phase-A `objective_gone` retire already prevent leaks. |
| 6 recall terminal state | **Built** as the orphan-fallback recall (d). No new job-FSM variant needed. |

WFV **17→18** for the one serialized-shape change (`SquadContext.engaged_once`); the
reset also usefully clears the churned/orphaned squads. Bot-only — the sim/decision/eval
crates are untouched, so parity + the determinism fence are unaffected. bot 155 tests
green; wasm-clippy clean.

## Why one fresh look now suffices

Frontier-core intel is *chronically* intermittent (refreshed only by a passing scout).
Before: the squad had to **win a race** against the 100t TTL — usually lost → churn.
After: a single fresh look creates the candidate; the squad is then **committed** and
the objective survives the 400t form+travel window regardless of intel going stale, so
it arrives, clears, and resolves. Candidate *discovery* still rides the existing
intermittent scouting (a `requested re-scout` on the central visibility queue).

## Deferred follow-ons (from the original 0027 landing)

- **Reassign** a resolve-clearer's survivors to a sibling core instead of recycling
  (reuse > recycle-refund > nothing). *(still open — backlog #5 below)*
- ~~**Forming-progress lease refresh**~~ — **DONE 2026-06-28** (forming-in-flight + travel
  lease, both bounded; see Update below).
- ~~**Candidate-discovery coverage**~~ — **DONE 2026-06-28** via the scout-fix (priority-
  driven reach, no slot-truncation, HIGH offense re-scout; super `d3352f2`).

## Update 2026-06-28 — reach/engage hardened end-to-end + polish backlog

After 0027's commitment-lease landed, a live private-server soak peeled a further stack of
lifecycle/movement/sizing blockers *between* "objective committed" and "core dead." Each was
root-caused with the new `[SquadTrace]` introspection + offline-proven via the eval churn
harness (`run_lifecycle_churn[_spatial]`), then deployed. The offense pipeline now runs
**end-to-end** (scout → commit → size → multi-home spawn → solo-travel to a shared rally →
gather → formation assault → arrive → focus → engage → clear); **uncontested lvl0 cores are
cleared live** (squad 274 confirmed form→travel→arrive→focus→engage; the lvl0 inventory
collapsed from 5–13 to ~1).

### Built this session (layers on 0027 — all committed)
- **Scout/offense unblock** (`d3352f2`): priority-driven Chebyshev scout reach (was Manhattan-5,
  excluded BFS≤10 offense targets); no `.take(slots)` truncation before the range filter; HIGH
  offense re-scout priority; lvl0 cores exempt from the offense concurrency cap; softened the
  `total_free_spawns==0` blanket early-return.
- **Spawn-priority edge + efficient sizing** (`7f50a33`/`974184c`): forming combat members spawn at
  `SPAWN_PRIORITY_COMBAT_FORMING=85` (above economy bulk, below CRITICAL miners); the EV optimizer
  sizes undefended zero-attrition structures to the **minimal** effective force (binary p_kill — no
  over-power ladder where P(win)≈1; defended sizing + calibration gates byte-unchanged).
- **Quorum rally + forming/travel lease** (`c368283`/`8bfbd44`/`e770aa7`): uncontested targets deploy
  at a min-viable quorum; the lease refreshes through the forming-in-flight banking gap AND the travel
  phase (both bounded by `MAX_FORMING_BUDGET=3000`/`MAX_TRAVEL_BUDGET=1000`), closing the
  mid-spawn/mid-travel lapse + the Generation churn.
- **Focus-on-arrival** (`8bfbd44`): the arrival-tick empty-DTO hole is bridged by reading
  hostiles/structures directly from `game::rooms()` when `mapping.get_room` is None.
- **Fighter-first spawn order** (`7f50a33`/`8bfbd44`): a partial roster is combat-capable (bot Phase B
  order only — assembled force byte-identical; reordering the assembler regressed
  `assembler_kills_across_defended_regimes`).
- **Shared-rally traverse** (`69ee23a`/`e2fb30c`/`c399f6f`/`c8b211c`): the movement-stall fix —
  **multi-home spawn preserved**; members solo-travel to ONE shared rally (`rally::shared_rally_point`,
  derived fresh each tick → no WFV bump; uncontested→target centre, contested→one room short out of
  tower range), gather via the **unified `rally::gather_quorum_met` kernel both the bot AND the sim
  call** (kills the bot/sim cohesion drift that froze the box anchor), then box-formation assault
  rally→target (in-room formation/`formation.rs` 0-diff).
- **`[SquadTrace]` introspection** (`1b2413f`): debug-gated (`features.military.debug_log`) per-squad
  STATE/MEMBER/DEPLOY/TRAVEL/ARRIVED/FOCUS/ENGAGED/GIVEUP trace — the live diagnostic for all of this.
- **Defender-lifecycle fix** (in flight, `wn1f2fh90`): latch `engaged_once` only on real in-room
  presence (no en-route latch); latch the assault once gathered (no in_room↔travel oscillation); a
  Defend squad holds-station on a clear owned room instead of `GaveUp`+re-field churn.

### Polish / follow-up backlog (priority order)

**Lifecycle / movement (this ADR):**
1. **Defense targeting — intercept the threat, don't garrison empty owned rooms** *(highest value)*.
   The Defend producer (`war.rs:421-428`) targets the OWNED room, but the enemy roams the NEIGHBOR
   rooms, so a defender stands uselessly in its empty room (the root of the edge-oscillation + churn
   the lifecycle fix only *bounds*). Fix: target the threat's actual room (intercept at the border),
   OR only field a Defend objective when the threat is in/entering the owned room (don't field
   garrisons for roaming neighbors). This is what makes defense *useful*, not just non-churning.
2. **Engaged-en-route: fight-through / route-around.** Even with the in-room latch gate, a squad whose
   path crosses a hostile room should disengage + continue (or route around) rather than stall on
   incidental contact.
3. **Far-target stall (d≥7).** Distant targets can exhaust the travel lease before arriving (pathing
   through hostile rooms slows progress). Revisit `MAX_TRAVEL_BUDGET`/progress tracking for long hauls,
   or stage the approach in legs.
4. **Smarter garrison / hold-station.** B2 stops the churn; a garrisoning defender should hold at the
   room entrance / a defensible tile, and the hold must stay bounded by the objective lifecycle (no
   immortal idle squad).
5. **Reassign survivors** to a sibling target instead of recycling (carried from the original deferred
   list — reuse > recycle-refund > nothing).
6. **Contested multi-member forming tail.** Large defended-target squads form slowly under spawn
   contention; if income-sensitive, floor distance-0 miner priority ≥90 so a forming squad never
   out-prioritizes a marginal top-up miner (the `SPAWN_PRIORITY_COMBAT_FORMING=85` vs lerped-miner
   edge noted in `7f50a33`).
7. **eval spatial-repro fidelity.** Lift `gather_quorum_met` out of the `use_shared_rally` guard in
   `run_lifecycle_churn_spatial` so the buggy arm stays RED for the *freeze*, not the gate (the
   decision-crate spatial repro is already faithful).

**Sizing / composition (ADR 0031 §Tier-3 / task #39 — cross-referenced, not owned here):**
8. Defense right-sizing (a `dps=30` threat fields a 4-member 2-healer roster — calibration-gated).
9. Drain **P2** (optimizer fields drain comps) / **P3** (bot threads `assault_mode`; the runtime tactic
   P0/P1 landed inert).
10. `member_energy>3000` PREFERRED-clamp lift; budget-free `emit_requirement` (retire
    `optimizer_ceiling_budget`).

## Design — Squad reassignment (backlog #1 + #5) [PROPOSED 2026-06-28]

Subsumes backlog **#1 (defense targeting)** + **#5 (reassign survivors)**, and removes the
retire→re-field churn for non-loss terminals. Reviewed against the existing objective model
for cohesion; lands in the same pure-kernel + thin-adapter seam as the rest of 0027.

### Problem
A squad that **Resolves** (target cleared) or hits **ObjectiveGone** (target vanished)
retires → members recycle or a fresh squad re-fields next tick (Generation churn), wasting
the invested spawn energy. And a garrisoning defender holds its now-clear owned room while
the threat roams a neighbor (the `holding_station` fix *bounds* the waste but doesn't make
the squad useful).

### Decision — reassign-on-terminal, in-place rebind, composition-gated
- **Kernel** (`screeps-combat-decision/src/lifecycle.rs`): new `ReconcileAction::Reassign {
  withdraw_old: bool }`, returned in place of `Retire{Resolved}` / `Retire{ObjectiveGone}`
  **iff** a manager-computed `reassign_available: bool` (a new `ReconcileSnapshot` input, fed
  in exactly like `holding_station` so the kernel stays pure/deterministic) is true. Resolved
  → `withdraw_old=true` (record the clean win); ObjectiveGone → `withdraw_old=false`.
  **`Wiped`/`Duplicate`/`GaveUp` still retire** (no members / unwinnable-backoff — don't chain
  a tired squad straight into another fight).
- **Manager** (`squad_manager.rs` Phase A): compute `reassign_available` + the target via a new
  `best_reassignment` = `best_unclaimed_near_excluding(exclude=[current_id])` + a **capability
  gate** (v1: same broad class — defender→`Defend`/`Secure`, offense→offense; full ADR-0031
  capability match later). On `Reassign`, **rebind in place — no `retire_squad`/`field_new_squad`,
  no Generation churn, bodies reused**: release/withdraw old claim → `claim(new)` (and add it to
  the Phase-A `covered` set so a second reassigner can't double-claim) → rewrite
  `SquadContext.objective_id`+`target` → reset `engaged_once=false`/`focus_target=None`/
  `state`/`squad_path` → **clear + re-key the `SquadFormingProgress` clocks** under the new id
  (reuse the existing re-field cleanup block, then stamp fresh `forming_started_at`) →
  `set_deadline(new, now+COMMITMENT_BUDGET)`.
- **No `WORLD_FORMAT_VERSION` bump** — only the already-serialized `objective_id` is rewritten;
  `claimed_by` + `SquadFormingProgress` are ephemeral.

### "Defending the wrong room" — reassignment **+** an intercept objective (not reassignment alone)
Reassignment can only re-point a freed defender to objectives that **exist**; today `war.rs`
emits only `Defend{owned_room}`, so when the owned room clears there is **nothing at the
neighbor** to reassign to (the offense scan `continue`s on owned rooms — `war.rs:759`). So the
producer must **also emit an INTERCEPT objective at the THREAT's room**: in the defense scan
(`war.rs:201-431`), when an armed hostile roams a neighbor of an owned room (reuse
`hostile_warrants_defender` + the existing border-visibility refresh), emit
`ObjectiveKind::Secure { room: threat_room }` (or a dedicated `Intercept{room}`) at **HIGH**
(below owned-`Defend` CRITICAL, above farms). Flow: threat in owned room → `Defend{owned}`
fielded → owned cleared (`Resolved`) **or** threat steps to a neighbor (`Defend{owned}`
TTL-lapses → `ObjectiveGone`) → Phase A `Reassign` → defender re-points to `Secure{neighbor}`
→ intercepts. The bounded garrison `holding_station` remains the fallback when no intercept
objective exists. **Rejected: objective-follows-threat** (mutating a `Defend`'s `room` breaks
the queue's `kind == identity` upsert/claim invariant — `objective_queue.rs:225-260`).

### Deferred / rejected
- **Preemption** (reassign to a higher-EV target mid-flight) — deferred (thrash; the
  `assault_latched`/`engaged_once` latches exist precisely to stop un-committing). Behind a flag
  if ever pursued.
- **GaveUp-reassign** — excluded for v1 (the squad just `mark_unwinnable`'d its room).

### Cohesion risks → mitigations
claim race → reassign-claim immediately + add to `covered`; lease/Generation accounting →
reuse the re-field cleanup + re-key the per-id clocks; ping-pong → terminal-only + `exclude=
[old_id]`; composition mismatch (defender onto an uncrackable core → `IN_ROOM_NO_FOCUS` stall →
poisons the room unwinnable) → the capability gate; unwinnable poisoning → `best_unclaimed_near`
already skips backoff rooms; determinism → selection is `max_by` over a `Vec` + the capability
gate is a pure fn over sorted roles (no `HashMap`).

### Offline repro / tests (extend `screeps-combat-eval/src/harness/lifecycle.rs`)
New `ChurnOutcome::Reassigned { from_gen, to, reuse_tick }` + cases over the shared kernel:
(1) reassign-on-resolve (assert **same generation** = reuse, vs churn's climbing generations);
(2) reassign-on-expire (+ a no-sibling control that still falls back to retire — reassign is
strictly additive); (3) defender-reassigns-to-threat (a neighbor `Secure` appears on owned
`ObjectiveGone` → `Reassigned{neighbor}` not `Garrisoned`; + a capability-mismatch control that
holds/recycles). Plus pure-kernel unit tests: resolved/gone→`Reassign` with correct
`withdraw_old`; wiped/gaveup never reassign; `reassign_available=false`→existing retire.

### Seams
`lifecycle.rs` (new action + snapshot input + tests) · `squad_manager.rs` (Phase-A rebind +
re-key) · `objective_queue.rs` (capability-aware selection helper over
`best_unclaimed_near_excluding`) · `war.rs` (emit the intercept objective — the "wrong room"
half) · eval harness (the churn cases). Cross-ref ADR 0026 (the `war.rs` intercept targeting is
a doctrine/targeting change).
