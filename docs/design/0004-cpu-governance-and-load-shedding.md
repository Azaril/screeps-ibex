# ADR 0004 — CPU Governance & Load-Shedding

- **Status:** Proposed
- **Date:** <YYYY-MM-DD>
- **Related:** Field Report C (CPU pathfinding death-spiral — **extinction-level**); review prompt §5(a), §6.9, §8, §11, §12.

## Context
Observed: long pathfinding spikes CPU → bucket exhaustion → constant tick restarts → no progress → **colony collapse**. Likely **not reliably reproducible** (emergent across systems), so the goal is **robustness, not a repro**. Today there is **no global CPU governor** — only local budgets; `find_route` can bypass the `pathfinding_ops` cap; cost matrices rebuild every tick. Remember **CPU = execution + intents**.

## Decision
<TBD after review.> Target: a colony that **sheds work and keeps making progress** under CPU pressure rather than entering a restart loop.

## Components to design
| Component | Purpose |
|---|---|
| **Global CPU governor** | central authority that knows bucket level + budget remaining this tick |
| **Hard pathfinding budget** | cap `find_route`/PathFinder ops and cost-matrix rebuilds; cache/persist across ticks |
| **Bucket-aware scheduler** | run systems by priority; **defer/skip non-essential** systems when the bucket is low |
| **Load-shedding tiers** | essential (defense, spawn, haul) always run; expansion/visuals/planning shed first |
| **Graceful degradation + recovery** | shed → recover as bucket refills; never a hard restart loop |
| **Early-warning signals** (review §11) | bucket trend, ticks-since-progress, repath storms, restart counter → **runtime shed trigger** + telemetry |

## Consequences
<TBD — interacts with the runtime/scheduling model, [0005](0005-runtime-and-scheduling-model.md).>

## Incremental Migration Path
<Land early (Increment 1): add the governor + pathfinding cap behind the scheduler hook; validate in sim under induced CPU pressure (progress continues, no restart-loop). No state-format change required.>
