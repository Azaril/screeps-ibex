# ADR 0006 — Local-Server Eval & Iteration Harness

- **Status:** Proposed
- **Date:** <YYYY-MM-DD>
- **Related:** review prompt §11 (observability/self-improvement), §12 (testing); rewrite plan **Increment 0**; Field Report C (needs fast, repeatable runs to validate load-shedding) and A (needs combat scenarios vs. an opponent).

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

Still TBD: scenario format and the exact colony-health score. *(A premature Node start was reverted; the harness will be Rust.)*

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
- **First scenarios** — economy bring-up, contested expansion, or war vs. an opponent bot (Field Report A)?
- **Metrics segment** — add the §11 metrics segment as part of this (Increment 0), or is there an existing stats path to read?
- **Layout** — where should harness scripts + run artifacts live (`tools/eval/`, `runs/`)?

## Incremental Build Path
- **Step 1 (smoke loop):** `docker compose up` screeps-launcher → scripted respawn → `deploy --server private-server` → read console + segment for K ticks → dump metrics JSON.
- **Step 2:** scenario parameterization + colony-health score + pass/fail gates.
- **Step 3:** run comparison / regression detection; wire into the self-improvement loop; later, CI.
