# External References

Reference material for the review and the rewrite. **Inspiration, not copying** — respect licenses and language differences; study *what* and *why*, then design Ibex's own approach.

## Reference bots (prior art)
- **Overmind** — https://github.com/bencbartlett/Overmind — top-tier open-source Screeps AI (TypeScript). Strongest inspiration for:
  - **Overlord / Directive / Colony** structure (≈ Ibex operations / missions / room).
  - **Logistics network** (request/provider matching ≈ Ibex transfer system).
  - **`CombatOverlords` / swarm cohesion** — directly relevant to **Field Report A** (squads scatter instead of forming quads).
  - Overall macro strategy, expansion, and CPU management.
  - Do **not** copy code; extract ideas and pitfalls.
- **rustyscreeps ecosystem** — `screeps-game-api` (the bindings, fork at `C:\code\screeps-game-api`), `screeps-starter-rust` — Rust/WASM patterns and idioms.

## Game ground-truth
- **Docs:** https://docs.screeps.com/ and https://docs.screeps.com/api/ — CPU/bucket, RawMemory segments, intents, structures, market.
- **Engine source (authoritative):** https://github.com/screeps/engine (org: https://github.com/screeps ; also `screeps/driver`, `screeps/common`). Use for exact **CPU/intent costs**, pathfinder internals, structure/market mechanics, and the **visual payload format** (Field Report H — renderer corruption).

## Telemetry / stats
- **Console stream** and **RawMemory segments** via the Screeps API — out-of-band metrics/error extraction (basis for self-improvement; review §11).
- **`screeps-plus-stats`** (in this repo), **screepspl.us** / **Grafana** dashboards.

## This repo
- `AGENTS.md`, `todo.md` (repo root); `docs/reviews/ibex-review-prompt.md`.
