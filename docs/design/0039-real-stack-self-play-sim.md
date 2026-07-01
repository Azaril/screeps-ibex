# ADR 0039 — Full real-stack self-play combat sim (the single trustworthy proof/debug tool)

- **Status:** Proposed (2026-07-01). Operator directive: make the offline combat sim drive the **real
  decision + rover + formation stack**, AI vs AI, multi-room, with varied terrain + compositions, as the
  single trustworthy proof/debug tool — "even if that means more self-play with continuous improvement."
- **One line:** The tactical sim is already real (engine + decision + rover + terrain, both sides); the
  gaps are (1) the render driver underuses the real terrain/composition inputs, (2) the self-play path is
  *anchorless* so it never exercises the real **formation cohesion**, and (3) the **strategic lead-up**
  (decide→size→spawn→rally→travel→commit) is split across three disjoint sims that never run end-to-end
  through one engine. Fix in phases: wire the real inputs (quick win) → extract the formation cohesion into
  a shared kernel → unify the lead-up into one engine-backed self-play loop → (optional) drive a render
  corpus from the tournament.

## 0. As-is fidelity (corrected — the sim is NOT a toy)

Self-play data flow: `validate.rs run_self_play` → both sides `ManagedSimSquad::step` → engine
`resolve_tick`. Per subsystem:

| Subsystem | State | Where |
|---|---|---|
| Engine (damage/heal/traffic/tower falloff/rampart/deaths) | **REAL** | `screeps-combat-engine/src/resolve.rs`, `movement.rs`, `damage.rs` |
| Combat decision (focus/heal/Lanchester engage/kite), **both sides** | **REAL** | `screeps-combat-agent/src/squad.rs:306` `decide_squad_with_pathing` |
| Movement/pathing/cross-room/shove | **REAL rover** | `screeps-combat-agent/src/pathing.rs:477` `resolve_moves_via_system` → `MovementSystem`+resolver+`LocalPathfinder` over multi-room `CombatWorld` |
| Terrain (walls/swamps → movement + pathing + combat) | **REAL** | `state.rs`, `movement.rs:252`, `pathing.rs:66` |
| `lifecycle.rs` `step_toward` Chebyshev stepper | **TOY, but ISOLATED** — a war-lifecycle FSM repro (ADR 0028 K0); **self-play never calls it** | `lifecycle.rs:338` |

The earlier "toy sim" read was a **layer confusion** (mistook the isolated lifecycle FSM stepper for the
self-play mover). The tactical stack is faithful.

## 1. The real gaps

**G1 — Driver underuses the real inputs (why the renders were samey).** `examples/multi_room_combat_renders.rs`
feeds *synthetic corridor* terrain and *hard-coded* compositions. The eval already has **13 real
mmo:shard3 terrain fixtures** + **13 real foreman-planned bases** (`terrain_import.rs fixtures()`,
`generate.rs captured_bases()`) and a free-form composition sampler (`roster.rs random_squad`). The variety
exists; it is simply not wired into the multi-room self-play lens. **Low effort.**

**G2 — Self-play is anchorless → the formation cohesion is not exercised.** `run_self_play` uses
`ManagedSimSquad` (real rover moves, but no `virtual_pos` anchor / `cross_room_formation_target` /
`advance_squad_virtual_position`). The bot's cross-room **formation cohesion** — including the transit-room
hold fix (ADR-less commit `db4ad3c`) — is therefore **never run in-sim**, so movement bugs at that layer
cannot be reproduced or proven offline. (The pure-rover repro in `screeps-rover/tests/border_oscillation.rs`
confirms the *rover* layer is clean; the bug lived one layer up, in the bot formation code the sim omits.)

**G3 — The strategic lead-up is split across three disjoint sims.** `run_self_play` hand-places a fixed
quad on the objective at t0. It never runs, through one engine-backed loop: **decide to attack → size a
force (oracle/doctrine) → spawn → rally → travel across rooms (with formation cohesion) → arrive → commit →
fight**. Force-sizing lives only in the movement-free `OracleCalibration`/siege path; spawn→rally→travel
lives only in the abstract `lifecycle.rs` stepper (no engine); the tactical fight is the pre-placed
self-play. "AI behavior across rooms" is exactly the seam none of the three cover end-to-end.

## 2. The formation extraction seam (the sign-off item)

The cohesion logic is **~90% pure** and extractable following the existing `rally::` shared-kernel
precedent (`formation.rs` already `pub use`s `screeps_combat_decision::rally::{...}`).

- **Move verbatim** into `screeps-combat-decision::formation`: `virtual_anchor_target`,
  `cross_room_formation_target` (the transit-room fix), `anchor_start_pos`, `corridor_layout_transition`,
  `squad_footprint`, `standoff_one_tile`, `FormationLayout`. The bot re-`pub use`s them — call sites
  unchanged.
- **Port the quorum/boundary/mode body** of `advance_squad_virtual_position` to a pure
  `advance_cohesion(&mut CohesionState) -> AdvanceDecision`, where `CohesionState` is a POD of exactly the
  fields it reads (`members: Vec<(slot, Option<Position>)>`, `layout`, `formation_mode`,
  `strict_hold_ticks`, `virtual_pos`, `destination`). The bot's function becomes a thin shim:
  `SquadContext` → `CohesionState` → kernel → write back → (if advance) `advance_virtual_pos`.
- **Leave `advance_virtual_pos` split at the existing rover seam.** `AnchorPath::advance` (rover
  `anchor.rs:80`) is already pure over `PathfindingProvider` + a `room_callback`. The bot supplies the
  `game::*` cost-matrix/terrain callback; the sim supplies an **engine-terrain** callback + a simple
  pathfinder. Both step the SAME `AnchorPath`. No game types cross into the kernel.

**Target boundary:** `screeps-combat-decision::formation` owns the cohesion kernels; the bot keeps only ECS
plumbing (`SquadContext`↔`CohesionState`, `MovementData` issuance, the game-backed cost callback);
`AnchorPath` stays in `screeps-rover` (already sim-usable). Serialized shape unchanged → **no WFV bump**
(pure logic relocation; the bot's `SquadContext`/persisted fields keep their shapes).

## 3. Phased plan

- **P1 — Wire the real inputs into the self-play render driver (quick win, no code moves).** Feed the 13
  real terrain fixtures + `random_squad` compositions into `multi_room_combat_renders.rs`'s self-play lens;
  vary terrain × composition × room-span; regenerate the corpus + index. Immediately addresses "samey /
  varied terrain / varied composition / real AI." Deterministic (seeded).
- **P2 — Extract the formation cohesion kernel (§2).** Move the pure functions + `advance_cohesion` into
  `screeps-combat-decision::formation`; bot re-exports; bot behavior byte-identical (proven by the existing
  formation tests + the transit-room test moving with the code). Adds an offline `advance_cohesion` test
  feeding a transit-room `CohesionState` → asserts hold (the oscillation fix, now sim-provable).
- **P3 — Unify the strategic lead-up into one engine-backed self-play loop.** Replace the hand-placement
  with: oracle force-sizing → spawn schedule → rally → cross-room travel driving the extracted formation
  cohesion (`advance_cohesion` + `AnchorPath` over engine terrain) + real rover → arrive → commit → the
  existing real tactical fight. One `CombatWorld`, one engine, end-to-end, both sides. This is where a
  cross-room oscillation (or its fix) becomes visible in a render.
- **P4 — (optional) Tournament-driven render corpus + continuous improvement.** Couple `tournament.rs`
  (round-robin self-play, `meta_nash`, `exploitability` gate) to render emission so the "continuous
  improvement" loop also produces the per-match replay corpus, and multi-room symmetric self-play beds
  (two homes + travel + engage).

## 4. Consequences / risks
- P2 relocates code that the live bot runs. Mitigation: verbatim move + re-export + the existing formation
  tests (incl. `transit_room_member_ahead_of_anchor_holds_not_expelled`) travel with the code and must stay
  green; bot build + clippy + the full suite gate it. No serialized-shape change → no WFV bump.
- P3 is the largest phase; it must reuse the extracted kernels (no reimplementation) so the sim stays
  faithful — the whole point is that a bug reproduced in-sim is a bug in the real code.
- Determinism: the sim is already bit-deterministic (memory: sim-determinism fence). New drivers must keep
  seeds explicit; no wall-clock/RNG-by-default.

## 5. Cross-references
ADR 0028 (war lifecycle / rally kernels — the `step_toward` FSM repro), 0031/0020 (force-sizing oracle,
tournament/exploitability), 0034/0035 (rally/travel/convergence, engage cascade), 0025a (terrain import).
Formation fix commit `db4ad3c` (transit-room hold). Rover exoneration test
`screeps-rover/tests/border_oscillation.rs`.
