# ADR 0027 — Combat Objective/Squad Lifecycle Rework (P-OBJ #23)

Status: IMPLEMENTED 2026-06-27 (super `e4bbf0f`; WFV 17→18). Companion to ADR 0008
(squad lifecycle) and ADR 0026 §9 (doctrine selection/sizing). Task #23.

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

## Deferred follow-ons (bounded — commitment makes retires rare)

- **Reassign** a resolve-clearer's survivors to a sibling core instead of recycling
  (reuse > recycle-refund > nothing).
- **Forming-progress lease refresh** for pathologically slow (energy-starved) big
  squads that can't form+travel inside 400t.
- **Candidate-discovery coverage**: keep last-known-core rooms warm (not just live
  objectives) so candidates re-appear without waiting for a roaming scout.
