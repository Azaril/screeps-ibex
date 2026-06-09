# Ibex Incremental Rewrite Plan — <YYYY-MM-DD>

> Built from [`../reviews/ibex-review-report.md`](../reviews/) and the [`../design/`](../design/) ADRs. **Incremental & confidence-driven**; back-compat not required (serialized state may be dropped per step). Don't break the running bot mid-increment.

## 1. North-Star & Objective Function
- **Win conditions:** survive · expand · score.
- **"Colony health" score** (review §11) — the function the rewrite optimizes and self-improvement tracks: *survival, CPU headroom, energy/GCL growth, military win-rate, …*
- **Non-negotiables:** never die to own CPU (load-shedding); never silently lose state without intent.

## 2. Architectural Pillars → ADRs
| Pillar | Current | Target (decision) | ADR |
|---|---|---|---|
| Entity model | specs/ECS + `repair_entity_integrity` | <TBD> | [0001](../design/0001-entity-model.md) |
| Serialization / persistence | bincode → gzip → base64 → segments | <TBD> | [0002](../design/0002-serialization.md) |
| Behavior modeling | `screeps-machine` FSM | <TBD> | [0003](../design/0003-behavior-modeling.md) |
| CPU governance / load-shedding | none (Field Report C) | <TBD> | [0004](../design/0004-cpu-governance-and-load-shedding.md) |
| Runtime / scheduling | specs dispatcher (fixed order) | <TBD> | [0005](../design/0005-runtime-and-scheduling-model.md) |

## 3. Increment Plan (strangler-fig)
Each increment: replaces one thing, behind a **stable seam**, validated before the next.

| # | Increment | Replaces / adds | Stable seam | Validation (test / replay / parallel-run) | Breaking? | Status |
|---|---|---|---|---|---|---|
| 0 | **Test & telemetry substrate** | unit-test harness, world-model abstraction, metrics segment, console event log | game-API boundary | pure-logic unit tests + replay smoke-run green | No | ☐ |
| 1 | **CPU governor + load-shedding** | global budget, bucket-aware scheduler, pathfinding cap | scheduler hook | sim under CPU pressure: progress continues, no restart-loop | No | ☐ |
| 2 | <e.g. entity model> | | | | | ☐ |
| 3 | <e.g. serialization> | | | | | ☐ |
| 4 | <e.g. behavior modeling / squad cohesion> | | | | | ☐ |
| … | | | | | | ☐ |

> Ordering rationale: **substrate first** (so every later step is verifiable), then the **survival-critical governor** (Field Report C), then the brittle pillars. Squad cohesion (Field Report A) and renderer (H) slot where their dependencies are ready.

## 4. Milestones & Sequencing
- M1: …
- M2: …

## 5. Per-Increment Validation Strategy
- Unit tests for the touched pure logic; serialization round-trip; replay corpus diff; optional **parallel-run/compare** (old vs new behind a flag) before cutover.

## 6. Risks & Rollback
| Risk | Increment | Mitigation | Rollback trigger |
|---|---|---|---|
| | | | |
