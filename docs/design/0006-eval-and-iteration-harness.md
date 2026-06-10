# ADR 0006 — Local-Server Eval & Iteration Harness

- **Status:** Proposed
- **Date:** 2026-06-09
- **Related:** **IBEX-023** (zero automated tests — Increment 0 stands up the harness + first pure-logic kernel tests); review prompt §11 (observability/self-improvement), §12 (testing); report §8 (Sequencing — **Increment 0**, gate: none — lands FIRST as the verification substrate) and §9 (metrics segment + colony-health score, the two specifics this ADR pins); Field Report C (needs fast, repeatable runs to validate load-shedding) and A (needs combat scenarios vs. an opponent — surfaced as the squad-cohesion-rate term of the score); ADR [0015](0015-testing-and-validation-strategy.md) + [`../plans/component-test-plans.md`](../plans/component-test-plans.md) §15 — **division of ownership: 0015 owns the overall test taxonomy and the L5 assertion-form/flake/earned-gating policy; this ADR keeps the harness machinery, the colony-health score, and the pre-deploy gates**; the companion doc's §15.2 enumerates the scenario library.

## Context
Iteration is currently **manual**. A **screeps-launcher** Dockerized private server (https://github.com/screepers/screeps-launcher) runs **much faster than MMO** and is fully controllable. The repo already has the pieces:
- A `private-server` target in `.screeps.yaml` (`127.0.0.1:21025`, user/pass, branch `default`).
- Deploy via `node js_tools/deploy.js --server private-server` → `wasm-pack` + `rollup` → `ScreepsAPI.fromConfig('private-server')` → `api.code.set(branch, modules)`.
- `screeps-api` (Node, already a devDependency) — programmatic API, **console stream**, and **memory/segment** access.

Automating **startup → deploy → data-gathering → evaluation → iteration** would replace the manual flow with fast, repeatable experiments, and is the foundation for the recursive self-improvement loop (§11) and (later) CI.

## Goals / Non-goals
- **Goals:** repeatable, fast, scriptable runs; automated metric extraction (console + segments); a **colony-health score** per run; run-to-run comparison for regression detection; substrate for self-improvement and pre-deploy gates.
- **Non-goals:** replacing MMO validation entirely; reproducing MMO CPU exactly (private-server CPU differs); a GUI.

## Harness components
1. **Server lifecycle** — screeps-launcher via Docker Compose (`C:\code\screeps-launcher\docker-compose.yml`: `screepers/screeps-launcher` + `mongo:8` + `redis:7`). The checkout has **no Dockerfile**, so **pull** the public image (don't build). Compose currently publishes **only `21025`** (game/API); the **server-CLI port (≈21026) is not exposed**, so bootstrap needs a compose override to publish it, or `docker compose exec`. Up/down; reset between experiments.
2. **World bootstrap / scenario** — automate the operator's manual flow (see *Bootstrap workflow* below) via the **server CLI**: reset data, set the user password, set tick duration, place the spawn, optionally add opponent bots (Field Report A / war). Parameterized scenarios: *economy bring-up*, *contested expansion*, *siege/defense*.
3. **Deploy** — reuse `js_tools/deploy.js --server private-server` (optionally a `debug` build for richer logs).
4. **Run control** — advance/fast-forward N ticks or until a condition.
5. **Data gathering** — via `screeps-api`: subscribe to the **console stream** (structured JSON events/errors), read the **RawMemory metrics segment** (§11) + stats; record a per-run time-series (CPU **+ intents**, bucket, GCL/RCL, energy throughput, creep counts, threat, deaths, restart counter).
6. **Evaluation / scoring** — compute the **colony-health score** (survival, CPU headroom, energy/GCL growth, military win-rate) and pass/fail **gates** (no death-spiral, no deser failure, CPU under budget).
7. **Iteration / comparison** — persist run artifacts (scenario + git SHA + metrics) under a `runs/` dir; diff vs. baseline; flag regressions; feed the self-improvement loop and the Bug & Issue Register.

### Bootstrap workflow (operator steps to automate)
1. Start a fresh server.
2. Connect and register the user (web client).
3. **Set a password** so `.screeps.yaml` username/password auth works for deploy + the API client — either `setPassword("<user>", "<pass>")` in the **server CLI**, or the **web form** `http://<host>:21025/authmod/password/`. (Provided by [screepsmod-auth](https://github.com/screepsmods/screepsmod-auth); users register via the Steam client first.)
4. `system.resetAllData()` — wipe the world.
5. Select / place the spawn.
6. Watch via the visualization UI client or console output.

Tick rate is tunable for fast eval: `system.setTickDuration(<ms>)` (default 1000 = 1s/tick) / `system.getTickDuration()`; `system.pauseSimulation()` / `system.resumeSimulation()`; bots via `help(bots)`. (Ref: screepspl.us *Private Server Common Tasks*; CLI access port/method to confirm — see Open Questions.)

## Decision
**Rust harness** (per *prefer Rust over Node*; single-language repo). Building blocks:
- **Docker:** the **`bollard`** crate (operator-suggested; pure Docker API, no `docker`-CLI subprocess) — pull/up/down the launcher compose, and publish the CLI port / `exec` for bootstrap.
- **Screeps API:** the Rust **`screeps-api`** crate (daboross/rust-screeps-api) covers **username/password auth (private servers) + websocket console streaming + live CPU/memory-usage** — but **not** **code upload (deploy)** or **RawMemory segment reads**. Implement those directly over the documented HTTP endpoints with `reqwest`: `POST /api/user/code` for deploy (the same endpoint `js_tools/deploy.js` hits via `api.code.set`) and the memory/segment endpoints (`/api/user/memory`, `/api/user/memory-segment`). *Interim:* shell out to the working `js_tools/deploy.js` for deploy until Rust code-upload lands.
- **Layout:** a Rust crate (e.g. a workspace member `screeps-eval`, or `tools/eval/` as a crate); run artifacts under `runs/`.

Both former TBDs are now pinned: the **scenario format and catalogue** live in [`component-test-plans.md`](../plans/component-test-plans.md) §15.2 (versioned config: map/terrain, spawn placement, opponent, fault-injection schedule, seeds, gate expressions over seg-57 + console events), and the **colony-health score** is fixed in §Reconciliation (2) below. *(A premature Node start was reverted; the harness will be Rust.)*

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Manual (today) | no build effort | slow, non-repeatable, no regression detection |
| **Scripts around screeps-launcher + `screeps-api`** | reuses existing deploy stack; fast; scriptable; real engine | upfront build; server CPU ≠ MMO |
| `screeps-server-mockup` (in-process) | no Docker; lighter; good for deterministic-ish tests | less faithful than a real server; separate from the deploy path |

## Open Questions (need operator input)
- **Rust API coverage** — the Rust `screeps-api` crate lacks **code upload** and **RawMemory segment reads**; confirm the private-server HTTP endpoints and build a thin `reqwest` client for them (or reuse `js_tools/deploy.js` for deploy as an interim).
- **Server-CLI access** — confirm the CLI port (≈21026) and how to reach it under Docker (publish via a compose override, or `docker compose exec`).
- **screeps-launcher config** — which mods/opponent bots; persistent world or fresh per run; desired tick rate?
- **First scenarios** — economy bring-up, contested expansion, or war vs. an opponent bot (Field Report A)? **Resolved:** the full catalogue (with increments and owning sections) is enumerated in [`component-test-plans.md`](../plans/component-test-plans.md) §15.2; the first is *economy bring-up* (Inc 0, also the parity-recording substrate).
- **Metrics segment** — add the §11 metrics segment as part of this (Increment 0), or is there an existing stats path to read? **Resolved:** yes — dedicated, versioned **seg 57** at Increment 0 (§Reconciliation (1) below).
- **Layout** — where should harness scripts + run artifacts live (`tools/eval/`, `runs/`)? **Resolved:** workspace member `screeps-eval`; artifacts under `runs/` keyed (scenario, git SHA) — pinned in component-test-plans §15/F14.

## Incremental Build Path
- **Step 1 (smoke loop):** `docker compose up` screeps-launcher → scripted respawn → `deploy --server private-server` → read console + segment for K ticks → dump metrics JSON.
- **Step 2:** scenario parameterization + colony-health score + pass/fail gates.
- **Step 3:** run comparison / regression detection; wire into the self-improvement loop; later, CI.

## Sequencing & cross-ADR ordering
Per report §8 *Sequencing*, this is **Increment 0** (gate: **none**) and **lands FIRST** as the verification substrate every later increment is validated against — never break the running bot mid-increment. It must be in place before Increment 1 (ADR 0004 global CpuGovernor + budgeted pathfinding facade + ADR 0005 tick-level panic containment), whose gate is *"harness can induce CPU pressure"*; the always-on metrics segment defined below is also the source for the **death-spiral telemetry** ADR 0004 §8 reads as its shed trigger (bucket trend, ticks-since-progress, repath storms, restart counter). Later gates likewise depend on this harness emitting the relevant metric: Increment 3 needs the **dangling-ref counter** (ADR 0001/0005), Increment 4 the **squad-cohesion-rate** metric (ADR 0003), and the **serialization round-trip / old-snapshot / fuzz** tests that gate Increment 2 (ADR 0002) are the first pure-logic kernel tests stood up here (IBEX-023, §9 *Test strategy*).

## Reconciliation with report §9 (now-pinned specifics)
The report fixes two details the *Open Questions* and *Harness components* above left open. These supersede the open question "*Metrics segment — add the §11 metrics segment as part of this (Increment 0)…*": **yes, add it here as part of Increment 0**, dedicated and versioned.

### (1) Dedicated, versioned metrics segment (report proposes **seg 57**)
Decoupled from the visualization flag and **always-on** (CPU is currently measured execution-only behind the debug timing flag, `game_loop.rs:139–143`, with ZERO intent accounting — report §9). Segment choice avoids the taken slots: **50–55** ECS + cost-matrix, **56** stats_history, **60** planner, **99** live stats — so **57** is free. One periodic metric snapshot per N ticks (a labelled **Memory/format** addition — a new segment, no change to existing serialized state):

```
seg 57 = { ver, tick,
  cpu_used, cpu_limit, tick_limit, bucket,
  intents_by_category,          // move/transfer/attack/build/repair/spawn (intents charge CPU; see MOVE_ACTION_CPU=0.2, movementsystem.rs:255)
  gcl, gpl, rcl_by_room,
  energy_throughput,
  creeps_by_role,
  active_ops, active_missions,
  threat_max,
  deaths, restart_counter,       // restart_counter from the env.tick discontinuity check, §9 death-spiral signals
  deser_failures, panics_caught, // IBEX-004/IBEX-014 silent deser-to-empty-world; IBEX-025 panic skips serialize_world
  death_spiral_signals,          // bucket trend, ticks-since-progress, repath storms, serialize-skipped count (§9)
  segment_fullness_watermark }   // per-segment high-water mark (IBEX-013/IBEX-014: seg-55 wipe + silent chunk-drop on overflow)
```

`ver` carries a version header (report §9 notes seg 99/56 are currently unversioned JSON and mis-decode silently on schema change — apply the header to ALL metric segments). The harness reads seg 57 over `reqwest` (the `screeps-api` crate lacks RawMemory-segment reads — see *Open Questions*) alongside the console stream.

### (2) Colony-health score — four weighted, survival-dominating terms
Replaces the TBD score (§Decision) with the report §9 *objective function*. Weights and per-scenario normalization are fixed in config so the score is reproducible and diffable; the term breakdown is recorded so regressions are attributable to a term:
1. **SURVIVAL (gate + score, dominates):** avoided extinction — no death-spiral, no unrecoverable deser failure (IBEX-004/014), spawn alive. A spiral run scores ~0 here regardless of the other terms.
2. **CPU HEADROOM:** mean/p95 of `(tick_limit − cpu_used)/tick_limit` **including intent cost**, plus long-tick rate (`used >= tick_limit`).
3. **ECONOMIC GROWTH:** slope of GCL + stored energy + RCL-progress, per-scenario normalized (Field Report C load-shedding validation).
4. **MILITARY WIN-RATE:** rooms held vs lost, **squad-cohesion rate = fraction of combat ticks all members in-range** (directly measures Field Report A / IBEX-001), targets killed vs waves spent.

These feed the **pre-deploy gates** (report §9): ZERO deser failures, ZERO caught panics, ZERO segment-overflow drops, CPU under tick_limit, no death-spiral alarm, and the economy-bringup colony-health score not regressed below baseline beyond threshold.

Assertion form, seed counts, and the flake/earned-gating policy for scenario gates are owned by ADR [0015](0015-testing-and-validation-strategy.md) §3 (distributional paired-seed-diff gates; N and thresholds from the companion doc's F19 file) — this ADR's score and gates are invoked there verbatim, not re-decided.
