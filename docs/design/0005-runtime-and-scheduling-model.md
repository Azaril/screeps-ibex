# ADR 0005 — Runtime & Scheduling Model

- **Status:** Proposed
- **Date:** 2026-06-09
- **Related:** IBEX-025 (no per-system panic isolation), IBEX-010 (Nuker-withdraw `panic!`), IBEX-008 (renderer per-target limit throw); review report §3, §5, §8 (Runtime pillar). Interacts with [0001](0001-entity-model.md) (a runtime-model change off specs rides on the identity decision) and [0004](0004-cpu-governance-and-load-shedding.md) (the scheduler seam is introduced *with* the CPU governor and is where 0004 plugs in).

## Context
Current: **specs** dispatcher runs ~60+ systems in a fixed per-tick order, with `world.maintain()` between. If the rewrite moves off ECS, the execution/scheduling model is an explicit choice. **Hard constraint:** single-threaded WASM — **no parallelism** (no OS threads, locks, or atomics-for-parallelism). The scheduler must support **load-shedding** (0004) and ideally **resumable work** across ticks (e.g. multi-tick pathfinding/planning).

## Decision

Adopt the **simplest model that supports load-shedding and resumable work: an explicit ordered / priority scheduler** over the current flat `for_each_system!` dispatch. **Do NOT adopt a cooperative `async` executor.** The report is authoritative on this (§8 Runtime pillar): async buys natural pause/resume, but the only concrete cross-tick-resume need today (room planning) already resumes via segment 60 without it, so the WASM/Rust async machinery is unjustified complexity against a plain priority pass. A runtime-MODEL change off `specs` dispatch is **deferred** — it rides on the ADR [0001](0001-entity-model.md) identity decision (whether `specs` remains the dispatch substrate is independent of, and downstream of, the stable-ID keying choice), not on this ADR.

Concretely:

1. **Scheduler seam (introduced with the CPU governor, ADR [0004](0004-cpu-governance-and-load-shedding.md), Increment 1).** Wrap the existing fixed-order system list in an explicit ordered scheduler that runs systems in **priority tiers** and consults the `CpuGovernor` (ADR 0004) between tiers. The governor decides which lower-priority tiers to shed under `Conserve`/`Critical` (war recompute, expansion, visuals shed first; the survival core — spawn, defense, serialize — never shed). This is **introduced at behavior parity** (same order, no shedding) and verified by replay before shedding is enabled; it is the seam the governor plugs into, not a new runtime.

2. **Panic containment under `panic="abort"` (IBEX-025), two complementary layers:**
   - **Narrow the known risky boundaries to `Result` / log-and-continue** at the source, so they never reach the abort path: the Nuker-withdraw `panic!` becomes `Err(ErrorCode::InvalidArgs)` + a one-shot log (IBEX-010, `transfersystem.rs:208` — the fn already returns `Result`); the visual flush guards per-target size and clamps coordinates to finite so `console.addVisual` cannot throw (IBEX-008). These are the reachable aborts the report identifies; fixing them removes the common cases without relying on the catch-all.
   - **PLUS a single tick-level containment boundary** — a `catch_unwind` (or the JS `try/catch` around the WASM call, which is the more robust choice under `panic="abort"`; see Consequences) wrapping `run_systems`, so that **`serialize_world` ALWAYS runs even if a system aborts mid-pass.** Today `run_systems` (game_loop.rs:135–151) calls each `.run_now(world)` with zero `catch_unwind` in the crate, and `cleanup_memory` / `repair_entity_integrity` / `serialize_world` (game_loop.rs:797/806/812) are plain calls *after* it returns — so any abort skips all three and freezes state at the prior good tick.
   - **Add a panic counter to the metrics segment** (ADR [0006](0006-eval-and-iteration-harness.md) always-on metrics, proposed seg 57) plus a `serialize-skipped` counter, so a recurring caught panic surfaces as data rather than a silent stall.

3. **Resumable work only where a concrete need exists.** Do not generalize a resumable-task abstraction. Room planning already resumes across ticks via seg-60; that pattern is retained. New cross-tick resume is added per-case only when a system demonstrably cannot complete inside one CPU-governed tier (none identified today). Load-shedding (governor-driven tier skipping), not async resumption, is the primary mechanism for staying under budget.

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep specs dispatcher (or a specs-free explicit ordered scheduler) | simple, debuggable, predictable | manual yield/resume for long work |
| **Single-threaded cooperative `async` executor** | natural pause/resume across ticks; ergonomic long tasks | real WASM/Rust complexity; easy to over-engineer; harder to reason about CPU |
| Coroutine/generator-based steps (without full async) | resumability with less machinery | Rust generator ergonomics |

> Guidance: pick the **simplest** model that supports load-shedding and resumable work. **Don't adopt `async` for its own sake** — justify it against a plain priority scheduler.

## Consequences

**Positive.**
- Choosing an explicit ordered/priority scheduler keeps CPU reasoning trivial (the pass is a deterministic, debuggable sequence) while giving ADR 0004 a clean place to shed tiers — the single biggest survival lever. No new concurrency model to reason about under single-threaded WASM.
- The tick-level containment boundary converts the current "one reachable panic anywhere aborts the whole tick and skips persistence" into "the offending system is skipped, the rest of the pass and `serialize_world` still run." This directly closes the structural amplifier behind IBEX-010/IBEX-008 and any future reachable panic — it is the highest-leverage robustness change in this ADR and is a quick-win (§4).
- Narrowing the two known reachable panics to `Result` means the catch-all is a backstop, not the primary defense, so the common cases degrade quietly with telemetry rather than as a caught abort.
- The panic + serialize-skipped counters give the death-spiral early-warning the rewrite needs (a rising serialize-skipped count is a direct signal that persistence is being lost), feeding both ADR 0004's shed trigger and ADR 0006's regression gates.

**Negative / new risks.**
- `panic="abort"` (`Cargo.toml:16`) means `std::panic::catch_unwind` does **not** unwind — an abort traps the WASM module and cannot be recovered *inside* the same WASM call. The robust containment boundary is therefore the **JS `try/catch` around the WASM entrypoint** (`lib.rs` export → `main_loop`), with a re-entry that still flushes segments; alternatively switch the build to `panic="unwind"` so `catch_unwind` works in-module. The panic=unwind switch is a **build-config change, not a Memory/format break** (the report labels it so under IBEX-025) and costs some code size / a little per-tick overhead; weigh it against the JS-boundary approach during Increment 1. Either way, "serialize always runs" must be validated by deliberately forcing a mid-pass panic and asserting persistence still occurred.
- **Mid-tick abort does not currently force a clean reset, and this must be addressed.** `env.tick = Some(current_time)` is set at **game_loop.rs:746, BEFORE `run_systems` (game_loop.rs:791)**. So if a system aborts mid-pass, the next tick's discontinuity check sees `env.tick == current_time` and does **not** rebuild the environment — leaving partial in-heap mutations from the aborted pass live, paired with a stale persisted world (serialize was skipped). The fix pairs with the containment boundary: either move the `env.tick` advance to **after** a successful `serialize_world` (so an abort leaves `env.tick` stale and the next tick force-rebuilds from the last good snapshot), and/or have the JS boundary mark the environment dirty on a caught abort. Do not advance `env.tick` until the pass has persisted.
- A system that is *shed* (skipped by the governor) is a different case from one that *aborts*: shedding is intentional and safe; the containment boundary must distinguish "deliberately skipped this tick" from "aborted" in telemetry so a shed tier does not read as a fault.

**CPU and tick-safety impact.**
- The priority scheduler itself adds negligible CPU (a tier ordering + a governor read between tiers; see ADR 0004 for the governor cost). The `Result`-narrowing changes are free. The tick-level boundary adds at most a guard frame per tick.
- Net tick-safety improves substantially: the worst-case outcome of a reachable panic goes from "colony-wide outage + frozen persistence" to "one system's work lost this tick, world still serialized, counter incremented."

**Validation.** Parity-validate the scheduler seam via **replay before any cutover** (record real GameView reads, assert identical intent stream with the ordered scheduler at no-shed parity — the §8/§9 record-replay substrate). Validate containment by forcing a mid-pass panic in a sim and asserting `serialize_world` ran and the panic counter incremented. Validate the `env.tick` fix by aborting a tick and asserting the next tick rebuilds from the last good snapshot rather than continuing on partial heap state.

**Breaking-change label.** Behavioral (panic boundary + scheduler-tier shedding change runtime behavior); a `panic="unwind"` build-config switch, if chosen, is **not** a Memory/format break. **None** for the `Result`-narrowing of the transfer/visual boundaries.

## Incremental Migration Path

Per the report's Sequencing paragraph (§8), this ADR's pieces land in **Increment 1** alongside ADR 0004, and the runtime-MODEL change is deferred:

- **Increment 0** (gate: none) — eval harness + always-on metrics segment (ADR 0006) land first, so the panic counter / serialize-skipped counter have a home and containment can be verified.
- **Increment 1** (gate: harness can induce CPU pressure) — **all of this ADR ships here, None-breaking and at parity:** (a) the explicit priority-scheduler seam introduced at behavior parity (same order, no shedding yet), which is where the ADR 0004 `CpuGovernor` plugs in; (b) the tick-level `catch_unwind` / JS try-catch containment boundary so `serialize_world` always runs; (c) narrowing the reachable boundaries to `Result` (IBEX-010 nuker withdraw, IBEX-008 visual flush); (d) the `env.tick`-advance fix (advance only after successful serialize, so a mid-tick abort force-rebuilds next tick). Validate ordering/behavior **parity via replay before enabling shedding**.
- **Deferred (rides on ADR 0001).** Any move *off* the `specs` dispatch substrate to a different runtime model rides on the ADR 0001 stable-ID identity decision and is **not** part of this increment. The priority scheduler wraps `specs` until then; whether `specs` stays as the dispatch substrate is decided downstream in 0001, independent of the identity keying.

**Stable seam.** The frozen boundary is the `run_systems` entrypoint and the post-pass (`cleanup_memory` → `repair_entity_integrity` → `serialize_world`): the scheduler is introduced behind `run_systems` and the containment boundary wraps it, so the post-pass contract (serialize always runs) is the verifiable invariant at each step.

**Breaking-change & state-drop notes.** This ADR introduces **no intentional state drop** (the two one-time resets in the report's sequencing belong to Increments 2 and 5, ADR 0002 / 0001). Behavioral changes (shedding, panic boundary) must never break the running bot mid-increment: ship the scheduler at no-shed parity first, enable shedding only after replay parity is green.
