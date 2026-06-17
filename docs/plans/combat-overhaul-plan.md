# Combat Overhaul — Integrated Plan (harness-first → squad behavior)

- **Created:** 2026-06-16
- **Owner:** William Archbell
- **Drives:** ADR [0006](../design/0006-eval-and-iteration-harness.md) (combat micro-sim), ADR [0008](../design/0008-combat-and-squad-architecture.md) (combat & squad architecture); cross-cuts ADR [0003](../design/0003-behavior-modeling.md), [0011](../design/0011-spawn-orchestration.md), [0015](../design/0015-testing-and-validation-strategy.md), [0014](../design/0014-empire-strategy-and-posture.md).
- **Sequencing decision (operator, 2026-06-16):** **harness first**, then combat behavior. Rationale: combat is currently *uninstrumented* (`military: None` in the score; no cohesion/orphan metrics), so every behavior change today is blind. The combat micro-sim makes each change measurable + introspectable + replay-diffable before we touch tactics.
- **Status this session:** design + ADRs done; **no production code yet** (operator-chosen scope). This plan is the actionable backlog for the implementation follow-ups.
- **Execution tracker:** [`../execution/phase-2.md`](../execution/phase-2.md) is the authoritative living execution document (workstreams H/I/M/G/W/S, task IDs `P2.*`, checkpoints, status log). This plan is the higher-level backlog/overview; phase-2.md tracks what/how/why as code lands.

## The problem, in one paragraph

Squads are very ineffective in three named ways, all confirmed in code:
1. **Null tactics** — they "just stand and ranged-mass-attack": no kiting in the ordered path, best-effort (not authoritative) focus-fire, oscillating retreat, flat-HP heal math.
2. **Orphan → idle** — combat creeps aren't mission children; on objective completion the `SquadContext` is deleted but creeps keep a dangling `squad_entity: u32` and idle until TTL (~1500t).
3. **Scatter** — cohesion is N independent solo pathfinds against a virtual anchor (the rover's `Follow`/`pull` group API is fully built but 100% dead code); members also trickle-spawn rooms/ticks apart.

The fix copies the **scout** subsystem (the operator's reference): a global `CombatObjectiveQueue` (request→claim→complete→release→retire) + a `SquadManager` lifecycle/tactics owner + lead-follower movement + synchronized spawning. See ADR 0008.

## Dependency graph (what gates what)

```
ADR 0006 Inc A (combat-engine port + conformance)   ── trust foundation, no bot deps
        │
ADR 0006 Inc B (tactical seam: CombatView/CombatIntent + `tactical` feature, parity-first)
        │            (this is also ADR 0003's combat-FSM extraction)
        ├── ADR 0006 Inc C (cohesion metrics + military score term)  ◄── unblocks measurement
        │
        ▼
ADR 0008 Step 0 (quick wins) ── can land in PARALLEL with Inc A/B (no seam dep), but
        │                        VALIDATE on the sim once Inc C lands
ADR 0008 Step 1 (wire defense onto SquadContext)
ADR 0008 Step 2 (anchor mover: footprint pathfind + orientation; relax for corridors)  gated: ADR 0003 movement
ADR 0001 Inc 3 (SquadStore/SquadId)  ──►  ADR 0008 Step 3 (SquadId key)   gated: ADR 0001
        │
        ▼
ADR 0008 Step 4 (CombatObjectiveQueue + SquadManager + tactics)   gated: Steps 1–3 + ADR 0011 demand seam
ADR 0008 Step 5 (defense+escort on the queue; WarOperation supervisor; backoff)
ADR 0008 Step 6 (synchronized spawning: GroupId/align-finish/pre-spawn)   gated: ADR 0011 Step 2
        │
ADR 0006 Inc D (scenarios/opponents/self-play/replay viz)  ── rides alongside Steps 4–6
ADR 0006 Inc E (server parity harness + nightly acceptance) ── closes the anti-overfit loop
```

## Phase 1 — Harness (ADR 0006, Inc A–E) — FIRST

| # | Deliverable | Gate (definition of done) |
|---|---|---|
| **A** | `screeps-combat-engine` crate: `CombatWorld` (JS-free), two-phase resolve, per-part 100-hit pools + front-to-back destruction, all damage/heal/dismantle/tower formulas, TOUGH/boost rounding, fatigue + same-tile movement conflict, ramparts/safe-mode, single 50×50 room. ~12 golden vectors captured from the live private server. | Sim reproduces every conformance vector **byte-exact** under `cargo test-host`. |
| **B** | **Trait-first seam (no cargo feature unless required, operator pref):** make the first combat decision (target selection + formation advance) generic over a JS-free `CombatView` trait emitting `CombatIntent`s; live adapter reads `game::*` in an isolated leaf, sim adapter reads `CombatWorld`. `screeps-combat-agent::IbexAgent` wraps it. (Cargo feature is the fallback only if host-compiling the bot crate is too heavy.) | Live-vs-extracted **intent byte-diff parity** (reuse the `IntentRecorder` digest) on a recorded tick. |
| **C** | `cohesion.rs` metrics; extend the seg-57 schema (`screeps-ibex-metrics`) additively with the `CohesionMetrics` block; replace `score.rs:100`'s `military: None` with a real term (win-rate + cohesion-rate + targets-killed-per-energy + own-losses). | Metrics round-trip pin; military-term unit tests; a combat change now moves `colony_health`. |
| **D** | `CombatScenario` data; scripted opponents (rush/kite/turtle/drain); self-play runner (`IbexAgent` vs `IbexAgent`); SVG/ASCII replay scrubber with reason tags. | Full fast introspectable loop runs; scenario scores report-only, earning gate status per the flake policy. |
| **E** | `parity.rs` sim-vs-server divergence report (nightly); wire named combat scenarios into the server acceptance gate (N-seed). | Parity within the tracked budget; nightly N-seed server gate live. |

By **Inc C** the operator has fast iteration + introspection + a moving combat score; by **D**, self-play + visual "why"; by **E**, the sim-to-real gap is bounded.

## Phase 2 — Combat behavior (ADR 0008, Steps 0–6) — validated on Phase 1

| # | Deliverable | Breaking | Validate on |
|---|---|---|---|
| **0** | **Quick wins (can start immediately, in parallel with Inc A/B):** whole-squad-centroid focus target; recompute `heal_power` each tick + boost-aware heal math; wire dead `check_movement_failure` (IBEX-015) into move states; add a `Recall` terminal state so orphaned creeps recover *today*. | Behavioral | sim cohesion-rate ↑ once Inc C lands; immediate console sanity on the live bot |
| **1** | Wire `SquadDefenseMission` onto `SquadContext` / `new_with_squad` (ADR 0003 §B.1 dominant fix) — defense gains coordination/focus-fire/cohesion. | Behavioral | sim: defense quad forms up; cohesion-rate ↑ |
| **2** | **Anchor-primary mover** (corrected from "lead-follower", see ADR 0003 §B.2): replace straight-line `advance_virtual_pos` with a **cached** footprint-aware ("moving-maximum") anchor path (pathfound once, cached on `SquadPath`, followed; re-path on invalidation/stuck) + lockstep block advance; wire dormant orientation (`threat_direction`/`orient_toward`/`reassign_slots`/`mirror_y`); corridors/edges handled by **relaxing the same mover** (width-1 pathfind + travel-oriented line/loose — no separate follower mode); **loose-centroid** for N>4; hard cohesion gate + MOVE-balanced bodies. `Follow`/`pull` reserved for no-MOVE/under-MOVE compositions only. | Behavioral | sim: cohesion-rate ↑, member-spread ↓, no permanent-Loose ratchet; rotates to face threat; threads a corridor then re-forms |
| **3** | `SquadId` key replaces `squad_entity: Option<u32>` (gated on ADR 0001 SquadStore, Inc 3). | Memory/format (one loud reset; bump `WORLD_FORMAT_VERSION`) | host: recycle/stale-ref fixtures resolve to None, never a foreign squad |
| **4** | `CombatObjectiveQueue` + `SquadManager` behind the live offense path (parity), then migrate `AttackMission` → objective producer; manager computes tactics (authoritative focus-fire, kiting, heal, hysteresis); generalize `handle_wave_wipe`; **delete combat `request_renew`**. | Memory/format + Behavioral | sim replay intent-diff parity; kill member → successor pre-spawned; unreachable room → torn down within deadline |
| **5** | Migrate defense + `Escort` onto the queue; `WarOperation` becomes a supervisor (withdraw/trim — IBEX-026/028); `UnwinnableTarget` give-up backoff. | Behavioral | sim/server: threat clears → defense squad retired within deadline |
| **6** | Synchronized spawning via ADR 0011 `GroupId`/align-finish/pre-spawn; boost handoff (or kill-switch off). | Behavioral | sim: quad members emerge within window W and rally cohesively |

## Immediately actionable (no new infra, can start now)

These are the ADR 0008 Step 0 quick-wins — pure-ish fixes that improve the *current* squads before any new system, and that the sim (once it lands) will measure:
- **Whole-squad-centroid focus target** (fixes the "anchor in another room → `None` for everyone" bug).
- **Recompute `heal_power` each tick + boost-aware heal math** (replace the flat 12 HP/part).
- **Wire `check_movement_failure`** (IBEX-015) into the job move states (stuck recovery).
- **Add a `Recall` terminal state** to `SquadCombatJob` (orphaned creeps return home / volunteer-defend instead of idling to TTL).
- **Revive dead code audit:** the dormant *anchor orientation* system — `FormationLayout::{orient_toward, mirror_y, rotate_cw}`, `threat_direction`, `reassign_slots`/`threat_facing_slots`/`safe_slots` (`update_formation_for_living_count` already calls `orient_toward` when `threat_direction` is set), and the footprint/tower overlays `apply_quad_cost_overlay`/`apply_formation_cost_overlay`/`apply_tower_avoidance_costs` — all built, all unused. The rover `Follow`/`pull` API (also unused) is **reserved for no-MOVE / under-MOVE'd compositions** (pulled attackers / dedicated puller) — *not* the corridor mechanism (corridors are handled by relaxing the anchor mover to a width-1 pathfind + line/loose).

## Open decisions (operator) — defaults baked into the ADRs, override any

**Decided 2026-06-16:** harness = hybrid combat micro-sim, **trait-first seam** (no cargo feature unless required); **Q1 = retask-if-viable-else-recycle**; **Q3 = wire boosts** behind the kill-switch; first implementation push = **Step 0 quick-wins + harness Inc A in parallel**. Q2/Q4/Q5/Q6/Q7 stand at their recommended defaults below (override anytime).

| # | Decision | Recommended default (in ADR) |
|---|---|---|
| Q1 ✓ | Squad disposition when an objective completes while the squad is healthy | **DECIDED: Retask** to next viable objective (residual-TTL/travel gated, ≥40%), else recall-and-recycle |
| Q2 | Offensive spawn priority while a wave is committed | **MEDIUM→HIGH on group admission** (uncommitted stays MEDIUM; defense always HIGH) |
| Q3 ✓ | Boost pipeline (IBEX-027): wire vs delete | **DECIDED: Wire** behind the existing kill-switch (heavy-siege viability) |
| Q4 | Orphaned-creep escape valve behavior | **Recall to nearest owned room + volunteer-defend**; recycle only if no need & low TTL |
| Q5 | Mid-campaign force re-sizing | **Re-evaluate at wave boundaries + always-on economy-collapse abort** (no per-tick thrash) |
| Q6 | Movement-conflict fidelity in the sim | **Full `rate1..rate4` + pull/swap from Inc A** (the cohesion bug class lives here) |
| Q7 | First opponent emphasis for the sim | **Self-play + scripted** for iteration; recorded-from-MMO next; second account for acceptance only |

## Anti-overfitting commitments (carried from ADR 0006/0008)

No opponent-specific constants. Threat measured at runtime; force sized from it; kiting is range/fatigue math; cohesion is geometric. The sim runs the bot's **real** decision code (no tactics fork); scenarios perturb terrain/positions/bodies across N seeds; opponents are a roster (scripted + self-play + recorded); the live seg-57 cohesion canary is the final arbiter and tightens the parity budget when sim and MMO disagree.
