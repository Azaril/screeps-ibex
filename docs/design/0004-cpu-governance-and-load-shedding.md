# ADR 0004 — CPU Governance & Load-Shedding

- **Status:** Proposed
- **Date:** 2026-06-09
- **Related:** Field Report C (CPU pathfinding death-spiral — **extinction-level**); IBEX-003, IBEX-016 (no global governor; `find_route` bypasses local budgets), IBEX-021 (war cadences=1), IBEX-035 (uncapped `compute_nearest_spawn_distances`), IBEX-036 (unbudgeted gather BFS), IBEX-038 (cost-matrix ephemeral rebuild), IBEX-039 (`find_route` not ops-budgeted). Review report §3 (no global CPU governance; observability coupled to the debug flag), §5 (extinction risk row), §8 (CPU pillar + Sequencing Increment 1), §9 (death-spiral early-warning signals). Cross-refs [0002](0002-serialization.md) (seg-55 wipe / persist+warm the route+cost-matrix cache — IBEX-013), [0005](0005-runtime-and-scheduling-model.md) (the priority-scheduler seam the governor reads at, and the tick-level panic containment that keeps `serialize_world` running), [0006](0006-eval-and-iteration-harness.md) (always-on metrics segment + induced-CPU-pressure scenario as the validation substrate).

## Context
Observed: long pathfinding spikes CPU → bucket exhaustion → constant tick restarts → no progress → **colony collapse**. Likely **not reliably reproducible** (emergent across systems), so the goal is **robustness, not a repro**. Today there is **no global CPU governor** — only local budgets; `find_route` can bypass the `pathfinding_ops` cap; cost matrices rebuild every tick. Remember **CPU = execution + intents**.

The review made the mechanism concrete and authoritative across three composing gaps (Field Report C, confidence M; IBEX-003/IBEX-016 confirmed, confidence H):
- **(1) No tick-level governor.** `tick()` (game_loop.rs:633–814) never reads `bucket()`/`get_used()` to gate the main pass; the **sole** early-return is segment-readiness (game_loop.rs:703–709). ~60+ systems run unconditionally. The piecemeal local budgets (`can_execute_cpu`, constants.rs:19–21, ~6–8 sites) are opt-in, and the heaviest consumers bypass them: transfer matching (IBEX-030), war recompute at cadence=1 (IBEX-021, war.rs:139–141), gather BFS + uncached `describe_exits` (IBEX-036), and per-candidate `pathfinder::search`.
- **(2) Pathfinding leaks outside the movement budget.** The rover's per-tick ops budget (movementsystem.rs:228–301) is the mature reference, but `RoomRouteCache::compute_route` calls `game::map::find_route` with **no CPU guard** (economy.rs:237 — confirmed: `find_route(from, to, Some(options))` with a per-room cost callback and no headroom gate); `findnearest.rs:61–133` runs the caller's full path per candidate with no cap; `compute_nearest_spawn_distances` runs `pathfinder::search` per spawn×target with no `max_ops` (structure_data.rs:174–206, IBEX-035); `find_route` is only headroom-gated, never charged to the ops budget (IBEX-039). `repath_count` is **written** (`saturating_add`, movementsystem.rs:935) but **never read** as a cap (IBEX-016).
- **(3) Ephemeral caches re-arm the storm on reset.** Route/cost-matrix caches are wiped on a VM reset, so the first post-reset tick (already WASM-reinstantiation-heavy) re-runs the full route storm. The cost-matrix segment-55 wipe (IBEX-013, owned by [0002](0002-serialization.md)) destroys the persisted matrix end-of-tick so the rebuild lands precisely on the most CPU-starved tick; cost-matrix ephemeral data is also rebuilt every tick (IBEX-017/IBEX-038). `env.tick` is set *before* `run_systems` (game_loop.rs:746), so a mid-tick abort does not force a clean reset.

Hard constraints: **single-threaded WASM** (no threads/locks/atomics-for-parallelism); **CPU is execution + intents** — intents charge CPU when logged, so a shed decision must account for both, not just `get_used()` execution time; **VM-reset resilience** is the whole point of persist/warm; the rewrite is **incremental and confidence-driven** (a stable, verifiable seam per step). Back-compat is **not** required but the running bot must never break mid-increment; this pillar is designed to be **None-breaking**.

## Decision
Introduce a **single global `CpuGovernor` resource** plus **one budgeted pathfinding facade** that every pathfinding caller shares. This is the review's single biggest rewrite recommendation and the only change that closes the extinction-class failure mode (§3, §8 CPU pillar). It is **None-breaking** and lands **early (Increment 1)**.

**1. Global `CpuGovernor` resource — the tiering authority.**
A single resource inserted into the ECS `World` early in the tick, computing a **tier** from **bucket level + bucket trend** (the negative-delta-over-a-sliding-window leading indicator from §9, not just the instantaneous bucket):
- **Normal** — full bucket / non-negative trend: every system runs.
- **Conserve** — bucket sagging or trend negative: shed the *first* tier of optional work (see shed order).
- **Critical** — bucket low and/or steeply falling: shed everything non-essential; keep only survival-critical work + the `MIN_PATHFIND_OPS` floor.

Every **expensive** system reads the governor **at the top** of its `run` (the same call site as the existing `can_execute_cpu` checks, generalizing them) and early-returns or downgrades work for its tier. This turns the flat ~60-system pass into a **sheddable pass** without changing dispatch order. The governor is a pure decision function over `{bucket, bucket_trend, cpu_used, cpu_limit, tick_limit}` — host-target testable against in-memory fixtures (the §9 "CPU governor decision logic" test target), with no game runtime. It reads (does not own) the priority-scheduler seam defined in [0005](0005-runtime-and-scheduling-model.md): the scheduler supplies system priority/essentiality; the governor supplies the tier; the intersection decides what runs.

**Shed order — essential always runs, optional sheds first (authoritative per §8):**
- **Always-on (never shed):** defense, spawn, haul, the movement pass, and end-of-tick `serialize_world` (persistence must survive a starved tick).
- **Shed under Critical FIRST:** **war recompute** (raise the hardcoded `RECOMPUTE_CADENCE=1` toward the intended ~50, war.rs:139–141, IBEX-021; under Critical, defer it entirely), **expansion** (claim/colony/scout evaluation), and **visuals**. Raising war cadences off 1 (defense ~2, offense ~10–20, recompute ~50) *or* bucket-gating them through the governor is the IBEX-021 remediation and is part of this ADR.
- **Shed under Conserve:** the next tier down — non-essential planning, re-matching cadence (transfer re-decide only every N ticks, IBEX-030), and discovery bursts (gather BFS / `describe_exits`, IBEX-036).

**Shed-tier registry (authoritative, owned here).** This ADR owns the **single registry** of never-shed / Critical-tier entries; the current authoritative set is exactly the Always-on list above — **defense, spawn, haul, movement, `serialize_world`**. Any addition (including "keeps running at Critical" exceptions) **requires amending this ADR**: sibling ADRs may *propose* entries, but a proposal is pending until recorded here. Currently proposed, not yet accepted: reaction execution + Tier-A defense-boost-lab loading ([0010](0010-boost-lab-factory-pipeline.md) §7) and crisis-energy maker orders ([0012](0012-market-and-risk.md) §9). The registry exists to stop never-shed tier creep — every entry is CPU that *cannot* be shed in a death-spiral, so the bar for admission is survival-criticality, not importance.

**2. One budgeted pathfinding facade — a shared per-tick ops pool.**
A single facade owns a **shared per-tick ops budget** that **all** pathfinding draws from. **Generalize the mature rover movement budget (movementsystem.rs:228–301) as the inner layer — do not replace it.** All current bypassers route through the facade and charge the same pool:
- `RoomRouteCache::compute_route` / `find_route` (economy.rs:237) — currently unguarded;
- `findnearest.rs` (findnearest.rs:61–133) — per-candidate full path, uncapped;
- `compute_nearest_spawn_distances` (structure_data.rs:174–206, IBEX-035) — per spawn×target `pathfinder::search`, no `max_ops`;
- tile search and any other `pathfinder::search` / `find_route` caller.

The facade enforces an **ops budget per tick** scaled by the governor tier (Normal → full, Conserve → reduced, Critical → floor), and a **`MIN_PATHFIND_OPS` floor** so creeps never fully freeze even at Critical — a starved colony must keep making *some* progress, which is exactly what averts the restart loop. `find_route` is **charged to the ops budget** (closing IBEX-039), not merely headroom-gated. `repath_count` is **capped** (it is already written at movementsystem.rs:935 but never read): when a creep exceeds the cap the facade surfaces `Failed` rather than re-searching, feeding the job-level stuck recovery (IBEX-015, owned by [0003](0003-behavior-modeling.md)) and the repath-storm telemetry below. The interim **quick-win** (None-breaking, can ship before the full facade): add `.max_ops` to `findnearest.rs` and `compute_nearest_spawn_distances`, and bucket-guard `economy.rs:237`.

**3. Persist / warm the route + cost-matrix cache — degrade gracefully across a reset.**
The `RoomRouteCache` and the cost matrix must **survive a VM reset** so the post-reset tick degrades gracefully instead of re-arming the storm on its most CPU-starved tick. The seg-55 wipe that destroys the persisted cost matrix (IBEX-013) is **closed in [0002](0002-serialization.md)** (dedicated cost-matrix segment + compile-time disjointness assert); this ADR depends on that fix and additionally persists/warms the route cache. Cost-matrix ephemeral rebuild (IBEX-017/IBEX-038) is reduced to change-event/TTL caching of the stable layers (structures/construction), rebuilding only the cheap creep layer per tick.

**4. Always-on death-spiral telemetry — feeds BOTH the runtime shed trigger AND diagnostics.**
The signals are **decoupled from the visualization/debug flag** (§3: today CPU/intent accounting lives only inside the viz overlay, so with viz off there is no trend signal — exactly the early-warning the rewrite needs). The governor's tier decision *consumes* these same signals, so the runtime trigger and the post-hoc diagnostic are one source of truth, emitted to the always-on metrics segment ([0006](0006-eval-and-iteration-harness.md), §9):
- **bucket trend** (negative delta over a sliding window — the leading indicator the tier reads);
- **ticks-since-progress** (GCL/RCL/stored-energy stall);
- **repath storms** (sum of `repath_count` increments, movementsystem.rs:935 — currently written, never read);
- **simultaneous-repath count** and **pathfinding-ops saturation** (facade budget exhausted);
- **env-reset / restart counter** (extend the `env.tick` discontinuity check at game_loop.rs:746);
- **serialize-skipped count** (a rising count is a direct death-spiral signal — `serialize_world` is skipped on a panic/abort, the IBEX-025 amplifier addressed by [0005](0005-runtime-and-scheduling-model.md)).

Per §8 Sequencing this is **Increment 1** (gate: the harness can induce CPU pressure), landing alongside the tick-level panic containment and the priority-scheduler seam ([0005](0005-runtime-and-scheduling-model.md)), after Increment 0 stands up the harness + metrics segment ([0006](0006-eval-and-iteration-harness.md)). It is **survival-critical and None-breaking → first** of the substantive increments.

## Components to design
| Component | Purpose |
|---|---|
| **Global CPU governor** | central authority that knows bucket level + budget remaining this tick |
| **Hard pathfinding budget** | cap `find_route`/PathFinder ops and cost-matrix rebuilds; cache/persist across ticks |
| **Bucket-aware scheduler** | run systems by priority; **defer/skip non-essential** systems when the bucket is low |
| **Load-shedding tiers** | essential (defense, spawn, haul) always run; expansion/visuals/planning shed first |
| **Graceful degradation + recovery** | shed → recover as bucket refills; never a hard restart loop |
| **Early-warning signals** (review §11) | bucket trend, ticks-since-progress, repath storms, restart counter → **runtime shed trigger** + telemetry |

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| **Single global `CpuGovernor` + one budgeted pathfinding facade** (chosen) | closes the extinction mode; one budget for all pathfinding; generalizes the mature rover budget; None-breaking; is the scheduler seam | touch-many (every expensive system reads the governor); needs a per-tick ops accounting discipline |
| Keep per-site local budgets, just add `.max_ops` everywhere | tiny, immediate, None-breaking | no global authority — the *sum* of many small searches still drains the bucket; no tiering; doesn't close C |
| Hard per-tick wall-clock cutoff (abort the pass when `get_used()` ≥ limit) | simple circuit-breaker | indiscriminate (cuts essential work too); ignores intent CPU; whichever systems run last starve; doesn't warm caches |
| Cooperative async executor that yields work across ticks | resumable heavy work | over-engineered for the need; complexity weighed and rejected per [0005](0005-runtime-and-scheduling-model.md) (explicit priority scheduler preferred; resumable only where a concrete need exists, e.g. planning's seg-60) |
| Replace the rover budget with a brand-new pathfinding subsystem | clean slate | discards the mature, well-defended router (Field Report G / IBEX-007: CPU-bounded by five layered governors, confirmed-OK); high risk; the report says *generalize, not replace* |

The decision **composes** the cheap row (add `.max_ops` / bucket-guard `economy.rs:237`) as the **interim quick-win** *inside* the facade rollout, then folds every caller into the shared pool under the global governor. The wall-clock cutoff is rejected as the sole mechanism but the governor still respects `cpu_limit`/`tick_limit` as a hard ceiling.

## Consequences
**Positive.**
- The extinction-class failure mode (Field Report C / IBEX-003/016) gains a **circuit-breaker**: under sustained pathfinding load the colony **sheds work and keeps making progress** (the `MIN_PATHFIND_OPS` floor guarantees creeps never fully freeze) rather than entering a restart loop. This is the single change the review calls table-stakes for competitive viability (§Executive Summary, §5 Extinction row).
- A **single budget** for all pathfinding closes the IBEX-016/IBEX-035/IBEX-039 leaks at once: the sum of many small searches can no longer drain the 10k bucket because they all draw from one pool. `repath_count` capping (movementsystem.rs:935) ends the repath-storm side of the spiral and surfaces `Failed` to job-level recovery (IBEX-015).
- Raising/gating war cadences (IBEX-021) and shedding expansion/visuals first means the heaviest uncoordinated consumers are now governed.
- Persist/warm of the route + cost-matrix cache (with [0002](0002-serialization.md)'s seg-55 fix) means the **post-reset tick degrades gracefully** instead of re-arming the storm on the most CPU-starved tick.
- The death-spiral telemetry is **always-on** (decoupled from the debug flag, §3), giving the operator a leading indicator (bucket trend) before collapse, and serving as the §9 colony-health **SURVIVAL** signal and a pre-deploy gate ("no death-spiral alarm").
- The governor is the **scheduler seam** the runtime model rides on ([0005](0005-runtime-and-scheduling-model.md)) — landing it early unblocks that pillar.

**Negative / costs.**
- **Touch-many:** every expensive system grows a governor read at the top of its `run`. Mitigated by generalizing the existing `can_execute_cpu` sites and by the priority-scheduler seam centralizing essentiality. Mis-tiering a system (marking essential work sheddable, or vice-versa) is a behavioral regression caught by the induced-CPU-pressure scenario.
- **Tuning surface:** tier thresholds, the per-tier ops budget, and `MIN_PATHFIND_OPS` are tunables. Set them in config so they are reproducible and diffable (§9), and validate against the colony-health score so a bad threshold shows as a regression, not a silent shed-too-much.
- Persist/warm adds a small per-tick serialize cost for the route cache; bounded by the cache size and gated behind the seg-55 fix in [0002](0002-serialization.md) (no new wipe risk — the disjointness assert prevents it).

**New risks / what becomes harder.**
- **Over-shedding** (a falsely-Critical tier starving essential work) is a new failure shape. The shed order pins defense/spawn/haul/serialize as never-shed and the `MIN_PATHFIND_OPS` floor keeps creeps moving; the harness scenario asserts *progress continues* under induced pressure, which would fail if the governor over-sheds.
- **Stale warmed caches** after a reset could route on out-of-date data; bounded because route/cost-matrix data is structural (slow-changing) and the creep layer is still rebuilt per tick (IBEX-038), so warming trades a small staleness for avoiding the storm — a deliberate, measurable trade.
- The governor must account for **intent CPU**, not just `get_used()` execution time (CPU = execution + intents); a governor that only watches execution time will under-shed when intent-heavy. The §9 intent counter at the side-effect choke point feeds this; validate intent costs against the open-source engine.

**CPU / tick-safety.** The governor itself is a cheap pure decision over a handful of scalars, read once per expensive system. The net effect is **strongly CPU-positive**: a shared budget caps total pathfinding ops, capped `repath_count` removes redundant re-searches, persist/warm removes the post-reset rebuild storm, and shed tiers remove whole systems' cost under pressure. **Tick-safety:** the always-shed-last guarantee on `serialize_world` keeps persistence running on a starved tick; combined with [0005](0005-runtime-and-scheduling-model.md)'s tick-level `catch_unwind`, a starved or aborting tick still persists, so the colony never silently stalls with no telemetry.

## Incremental Migration Path
The seam is the **`CpuGovernor` resource read + the pathfinding facade**; the inner rover budget (movementsystem.rs:228–301) is generalized, not replaced, so the well-defended router (Field Report G / IBEX-007) is preserved. Each step is validated by the eval harness (ADR 0006) under an **induced-CPU-pressure scenario** before the next; never break the running bot mid-increment. This is **Increment 1** (gate: harness can induce CPU pressure), after Increment 0's harness + metrics segment.

1. **Quick-win (None-breaking, can ship first):** add `.max_ops` to `findnearest.rs` (findnearest.rs:61–133) and `compute_nearest_spawn_distances` (structure_data.rs:174–206, IBEX-035); bucket-guard `RoomRouteCache::compute_route` (economy.rs:237, IBEX-016); raise war cadences off 1 *or* gate them by `can_execute_cpu` (war.rs:139–141, IBEX-021). **Validate:** profile under pressure (features.system_timing per-tier CPU); confirm recompute no longer runs every tick; assert no uncapped search remains via the ops-saturation telemetry.
2. **Always-on telemetry (None-breaking):** wire the death-spiral signal block (bucket trend, ticks-since-progress, repath storms reading `repath_count` at movementsystem.rs:935, simultaneous-repath count, restart counter extending game_loop.rs:746, serialize-skipped count) into the metrics segment ([0006](0006-eval-and-iteration-harness.md)/§9), decoupled from the debug flag. **Validate:** induce pressure in sim, assert the signals move and a spiral raises the alarm.
3. **Global `CpuGovernor` resource (Behavioral, None-breaking format):** insert the governor early in `tick()`; compute tier from bucket + trend; replace/generalize the `can_execute_cpu` sites with a governor read at the top of every expensive system; implement the shed order (Critical sheds war recompute + expansion + visuals first; defense/spawn/haul/serialize never shed). **Validate:** host-target unit tests on the pure tier decision; sim under induced pressure — **progress continues, no restart loop** (§5/§7 IBEX-003 validation).
4. **Budgeted pathfinding facade (Behavioral, None-breaking format):** route ALL pathfinding through one facade owning the shared per-tick ops pool (generalize the rover budget as the inner layer); scale the budget by tier; enforce the `MIN_PATHFIND_OPS` floor; charge `find_route` to the budget (IBEX-039); cap `repath_count` and surface `Failed` (IBEX-016). **Validate:** path to a far/unreachable room under low bucket and assert the budget caps ops and surfaces `Failed` without freezing other creeps; assert the floor keeps creeps moving at Critical.
5. **Persist / warm the route + cost-matrix cache (depends on [0002](0002-serialization.md) IBEX-013):** persist `RoomRouteCache`; rely on 0002's dedicated cost-matrix segment + disjointness assert so seg-55 survives a reset; reduce cost-matrix ephemeral rebuild to change-event/TTL on the stable layers (IBEX-017/IBEX-038). **Validate:** force a reset, assert `load_cost_matrix_cache` is non-empty and the post-reset tick does **not** re-run the full route storm (bucket does not collapse on the reinstantiation tick).

**Breaking-change labels:** Steps 1, 2 — **None**. Steps 3, 4 — **Behavioral** (load-shedding changes *when* optional work runs under pressure; no Memory/format change — no serialized field is added or reordered). Step 5 — **None** for this ADR (the persist/warm rides on [0002](0002-serialization.md)'s already-labelled cost-matrix-segment change; this ADR adds no new format break). No state drop is introduced by this pillar; it is non-format-breaking end-to-end, exactly so it can land early without a cutover.
