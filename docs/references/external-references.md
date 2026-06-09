# External References

Reference material for the review and the rewrite. **Inspiration, not copying** — respect licenses and language differences; study *what* and *why*, then design Ibex's own approach.

## Reference bots (prior art)
- **Overmind** — https://github.com/bencbartlett/Overmind — top-tier open-source Screeps AI (TypeScript). Strongest inspiration for:
  - **Overlord / Directive / Colony** structure (≈ Ibex operations / missions / room).
  - **Logistics network** (request/provider matching ≈ Ibex transfer system).
  - **`CombatOverlords` / swarm cohesion** — directly relevant to **Field Report A** (squads scatter instead of forming quads).
  - Overall macro strategy, expansion, and CPU management.
  - Do **not** copy code; extract ideas and pitfalls.
- **rustyscreeps ecosystem** — **`screeps-game-api`** (the Rust↔Screeps bindings): upstream & latest docs at https://github.com/rustyscreeps/screeps-game-api (local working fork: `C:\code\screeps-game-api`; API reference on docs.rs/screeps). Also `screeps-starter-rust` — Rust/WASM project patterns and idioms.

## Hauling / logistics (deep-dive)
Provider↔consumer matching is a hard assignment/transport problem — relevant to `transfer/transfersystem.rs`, the partial-haul abandon (`IBEX-011`), and matching-cost (`IBEX-010`) findings.
- **"Screeps #4: Hauling is NP-hard"** — Ben Bartlett (Overmind author) — https://bencbartlett.com/blog/screeps-4-hauling-is-np-hard/ — why hauler assignment is NP-hard, and practical CPU-bounded approaches.
- **Overmind `LogisticsNetwork`** — in https://github.com/bencbartlett/Overmind — a worked implementation to study (not copy).

## Game ground-truth
- **Docs:** https://docs.screeps.com/ and https://docs.screeps.com/api/ — CPU/bucket, RawMemory segments, intents, structures, market.
- **Engine source (authoritative):** https://github.com/screeps/engine (org: https://github.com/screeps ; also `screeps/driver`, `screeps/common`). Use for exact **CPU/intent costs**, pathfinder internals, structure/market mechanics, and the **visual payload format** (Field Report H — renderer corruption).

## Telemetry / stats
- **Console stream** and **RawMemory segments** via the Screeps API — out-of-band metrics/error extraction (basis for self-improvement; review §11).
- **`screeps-plus-stats`** (in this repo), **screepspl.us** / **Grafana** dashboards.

## Local server & automation
- **screeps-launcher** — https://github.com/screepers/screeps-launcher — Dockerized private Screeps server; **much faster than MMO**, fully scriptable. Basis for the eval/iteration harness (`../design/0006-eval-and-iteration-harness.md`).
- **`screeps-api`** (Node; already a devDependency) — programmatic API + console stream + segment access; reused for deploy (`js_tools/deploy.js --server private-server`) and data-gathering. The repo's `.screeps.yaml` already defines a `private-server` target (`127.0.0.1:21025`).
- **screepers tools / bots** — https://github.com/screepers — server mods, opponent bots (for combat scenarios, Field Report A), and standards (SS3 unified credentials, which `.screeps.yaml` follows).
- **Private-server CLI tasks** — https://wiki.screepspl.us/Private_Server_Common_Tasks/ — `system.resetAllData()`, `system.setTickDuration(ms)` (default 1000) / `getTickDuration()`, `system.pauseSimulation()` / `resumeSimulation()`, `setPassword(user, pass)` (screepsmod-auth), `help(bots)`. CLI access port/method (≈21026) to confirm.
- **`bollard`** (Rust Docker API client) — https://crates.io/crates/bollard — **chosen** for the (Rust) harness's Docker control (pure API, no `docker` CLI subprocess).
- **Rust `screeps-api` crate** — https://github.com/daboross/rust-screeps-api — username/password auth + websocket **console** streaming + live CPU/memory-usage. **Gaps:** no code upload (deploy) and no RawMemory segment reads — do those via `reqwest` against `POST /api/user/code` and the memory/segment endpoints (or reuse `js_tools/deploy.js` for deploy interim).
- **screepsmod-auth** — https://github.com/screepsmods/screepsmod-auth — enables username/password auth on the private server. Set creds via `setPassword('User','Pass')` in the server CLI or the web form `/authmod/password/` (after Steam registration); required for the API client and `deploy.js` to authenticate.

## This repo
- `AGENTS.md`, `todo.md` (repo root); `docs/reviews/ibex-review-prompt.md`.
