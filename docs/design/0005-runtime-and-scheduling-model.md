# ADR 0005 — Runtime & Scheduling Model

- **Status:** Proposed
- **Date:** <YYYY-MM-DD>
- **Related:** review prompt §1, §5(a), §10, §12. Interacts with [0001](0001-entity-model.md) and [0004](0004-cpu-governance-and-load-shedding.md).

## Context
Current: **specs** dispatcher runs ~60+ systems in a fixed per-tick order, with `world.maintain()` between. If the rewrite moves off ECS, the execution/scheduling model is an explicit choice. **Hard constraint:** single-threaded WASM — **no parallelism** (no OS threads, locks, or atomics-for-parallelism). The scheduler must support **load-shedding** (0004) and ideally **resumable work** across ticks (e.g. multi-tick pathfinding/planning).

## Decision
<TBD after review.>

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep specs dispatcher (or a specs-free explicit ordered scheduler) | simple, debuggable, predictable | manual yield/resume for long work |
| **Single-threaded cooperative `async` executor** | natural pause/resume across ticks; ergonomic long tasks | real WASM/Rust complexity; easy to over-engineer; harder to reason about CPU |
| Coroutine/generator-based steps (without full async) | resumability with less machinery | Rust generator ergonomics |

> Guidance: pick the **simplest** model that supports load-shedding and resumable work. **Don't adopt `async` for its own sake** — justify it against a plain priority scheduler.

## Consequences
<TBD.>

## Incremental Migration Path
<The scheduler seam is introduced with the CPU governor (Increment 1); a runtime-model change rides on the entity-model decision (0001). Validate ordering/behavior parity via replay before cutover.>
