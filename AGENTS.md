# Agent context: screeps-ibex

This file provides structured context for AI agents and developers working on the screeps-ibex codebase. Use it to understand the project’s domain, architecture, and conventions.

## 1. Domain: Screeps MMORTS

- **Screeps** is a massively multiplayer realtime strategy game. Code runs in the game’s V8 JavaScript environment; the server executes it every tick even when the player is offline.
- **Docs:** [Screeps documentation](https://docs.screeps.com/), [API reference](https://docs.screeps.com/api/).
- **Runtime:** Tick-based. Each tick, your code runs once; the game then applies all intents and advances state. Design for **safety and longevity**: no panics in hot paths, no reliance on heap state across VM reloads.
- **Persistence:** The VM can be reset between ticks. All long-lived state must go through **Memory** (or RawMemory segments). Assume a fresh start after each tick; do not rely on Rust heap/static state for correctness across ticks.

## 2. Tech stack

- **Language:** Rust, compiled to **WASM** and invoked from the game’s JS/TS layer.
- **Game API:** Typed bindings from **screeps-game-api** (Rust → Screeps JS API). The dependency is **patched** to a local path: `../screeps-game-api` (see root `Cargo.toml`). Changes there should be high quality and suitable to push upstream.
- **ECS:** **specs** (0.20). World state lives in a `specs::World`; systems run in a fixed order each tick. Components that must survive VM reload are serialized into RawMemory segments (see below).
- **Other:** serde/serde_json for serialization, bincode + base64 + compression for segment payloads, screeps-rover for movement/pathfinding, screeps-foreman for room planning, etc.

## 3. Repository layout

```
screeps-ibex/
├── Cargo.toml              # Workspace root; patches screeps-game-api and submodule crates
├── .gitmodules             # Git submodules (screeps-cache, screeps-machine, screeps-rover, etc.)
├── js_src/                 # JS entrypoint that loads and calls WASM
├── js_tools/               # Deploy/tooling (e.g. deploy.js)
├── screeps-ibex/           # Main bot crate (the AI logic)
│   ├── Cargo.toml         # Features: profile, sim, mmo
│   └── src/
│       ├── lib.rs         # WASM exports: setup(), game_loop_export() → main_loop() → game_loop::tick()
│       ├── game_loop.rs   # Tick orchestration, ECS setup, serialize/deserialize, dispatchers
│       ├── memorysystem.rs
│       ├── serialize.rs
│       ├── globals.rs
│       ├── jobs/          # Per-creep work: harvest, build, haul, upgrade, claim, etc.
│       ├── missions/      # Room-level goals (e.g. localbuild, miningoutpost, defend)
│       ├── operations/    # High-level campaigns (MiningOutpost, Claim, Colony)
│       ├── room/          # Room data, visibility, room plan, create/update systems
│       ├── pathing/       # Cost matrices, movement (uses screeps-rover)
│       ├── transfer/      # Orders and transfer queue
│       └── ...
├── screeps-cache/          # [submodule] Caching utilities
├── screeps-foreman/        # [submodule] Room layout/planning
├── screeps-machine/        # [submodule] State machine utilities
├── screeps-rover/          # [submodule] Movement/pathfinding
├── screeps-timing/         # [submodule] Profiling (optional)
└── screeps-timing-annotate/
```

- **Workspace members** (root `Cargo.toml`): screeps-ibex, screeps-cache, screeps-rover, screeps-timing, screeps-timing-annotate, screeps-foreman. screeps-foreman-bench is excluded.
- **Submodules** are developed in separate repos (e.g. Azaril/screeps-rover); this repo patches them via `[patch]` so local checkouts are used. Keep API boundaries clear so changes can be shared.

## 4. Tick flow and ECS

- **Entry:** JS calls `game_loop_export()` each tick → `game_loop::tick()`.
- **Reset handling:** If `reset.environment` or tick discontinuity is detected, the ECS `GameEnvironment` is dropped and recreated. If `reset.memory` is set, segment state is cleared.
- **Segment readiness:** Tick requests RawMemory segments (50, 51, 52 for component data; 55 for cost matrices). If segments are not yet active, the tick returns early after requesting segments; next tick they are available.
- **Pre-pass (dispatcher):** Cleanup dead creeps, create/update room data, then entity mapping (room name → ECS entity).
- **Main pass (dispatcher):**  
  Operations → Missions → Jobs (with pre_run then run), then movement, visibility queue, spawn queue, transfer queue, order queue; then room plan, visualizer, stats, cost matrix store, and finally MemoryArbiter (flush segment requests).
- **After main pass:** Creep memory cleanup (remove Memory.creeps entries for dead creeps), then **serialize** world into RawMemory segments (50, 51, 52).

Serialized components include: CreepSpawning, CreepOwner, CreepRoverData, RoomData, RoomPlanData, JobData, OperationData, MissionData. They use specs’ `SerializeComponents`/`DeserializeComponents` with a custom marker type and bincode.

## 5. Key abstractions

- **Operations** (e.g. MiningOutpost, Claim, Colony): High-level objectives; own missions and overall strategy.
- **Missions** (e.g. localbuild, miningoutpost, defend, haul): Room- or objective-scoped tasks; produce and consume jobs.
- **Jobs** (e.g. harvest, build, haul, upgrade): Assigned to creeps; implement `Job` (describe, pre_run_job, run_job). Jobs interact with the movement system (screeps-rover) and transfer queue.
- **Rooms:** One ECS entity per room; `RoomData` and `EntityMappingData` (room name → entity). Room systems create/update room state and run the room plan.
- **Creeps:** Mapped into ECS; have CreepOwner (screeps reference), JobData, and movement data. Creep memory is also cleaned from `Memory.creeps` when creeps die.

## 6. Memory and serialization

- **MemoryArbiter:** Requests which RawMemory segments to activate; reads/writes segment data. Segment size limit 50 KiB. Requested segments are activated on the *next* tick.
- **Persistence:** Critical state is stored in segments 50–52 (and 55 for cost matrices). Serialization is bincode → compress → base64 → chunked into segments. Deserialization runs when the environment is first loaded after a reset or when segments become ready.
- **Memory.creeps:** Used for per-creep script memory; cleaned each tick to remove entries for dead creeps. Do not assume any other Memory shape is stable across VM reloads unless it is explicitly documented and persisted.

### Serialization format and migration

- **Changing serialization format or serialized data across ticks requires careful attention.** Losing memory state is costly to recover from: the colony must rebuild from scratch (rooms, jobs, operations, missions, spawn queue, etc.). Treat serialized format as a contract.
- **There is no programmatic way to recover from a deserialization error.** The current code path does not handle deserialization failure gracefully; the only recovery is a **full reset of state** (e.g. via the reset flags that clear environment and/or memory). Do not introduce changes that make existing segment data fail to deserialize unless a reset is acceptable.
- **Newly added fields must have defaults.** When you add fields to types that are serialized (components or structs in the serialization list in `game_loop.rs`), old data stored in RawMemory was produced without those fields. You must ensure backward compatibility: use `Default` implementations, `#[serde(default)]`, or equivalent so that deserializing old payloads yields sensible values for the new fields rather than failing or leaving them uninitialized.
- **Breaking changes require user confirmation.** If a breaking change to memory layout or serialization format is necessary (e.g. removing or renaming serialized fields, changing format in a non–backward-compatible way), **you must prompt the user to confirm that this is acceptable** before implementing it. There are situations where the bot will be reset anyway and carrying forward technical debt for compatibility may be unacceptable; the user is the one to decide. Do not assume either “always preserve compatibility” or “breaking change is fine” without asking.

## 7. screeps-game-api (local fork)

- **Path:** Patched from `../screeps-game-api` (sibling directory to screeps-ibex repo).
- **Upstream:** [rustyscreeps/screeps-game-api](https://github.com/rustyscreeps/screeps-game-api). Changes in the fork should be minimal, well-tested, and suitable for upstream contribution.
- **Features:** `sim` (simulation mode) and `mmo` (live game) are passed through from screeps-ibex.

## 8. Conventions and safety

- **No panics in the hot path:** Avoid unwrap/expect in tick-critical code; handle errors and log instead. The game keeps running after your code returns.
- **Idempotent and restart-safe:** Assume any tick can be the first after a VM reload. Rely on serialized state and Memory/segments, not thread_local or static mutable state for correctness.
- **CPU:** Respect `Game.cpu` limits; avoid unbounded loops or heavy work per tick. Use the optional `profile` feature and screeps-timing to find hot spots.
- **Rust style:** The crate uses `#![warn(clippy::all)]`, clippy.toml, and rustfmt. New code should pass `cargo clippy` and `cargo fmt`. The project targets WASM: ensure the target is installed (`rustup target add wasm32-unknown-unknown`). The workspace `.cargo/config.toml` sets the default build target to `wasm32-unknown-unknown`, so `cargo clippy -p screeps-ibex` (or `npm run clippy`) runs Clippy for the WASM build.

## 9. Git and workflow

- **Feature branches:** Do changes on feature branches so they can be reviewed via pull requests when required. Avoid committing directly to the main integration branch (e.g. `master`) for changes that need review.
- **Commit messages:** Write commit messages that efficiently summarize the change and follow git commit best practices (e.g. imperative mood, clear scope, optional body for why when useful).
- **Submodules:** The project uses git submodules (screeps-cache, screeps-foreman, screeps-machine, screeps-rover, screeps-timing, etc.). When committing changes that touch both the main repo and submodules, **commit in the correct order**: leaf repos first (e.g. submodule repos), then update the superproject to point to the new submodule commits. This keeps history consistent and avoids broken references.
- **screeps-game-api:** The `screeps-game-api` directory is a **shared library** (game bindings used by other projects). It is patched from `../screeps-game-api` and is not a submodule of this repo. **Upstream changes must go through pull requests** (e.g. to rustyscreeps/screeps-game-api or the canonical host). Locally, work-in-progress changes in that folder are fine while developing; when landing changes, open or use a PR for the screeps-game-api repo rather than pushing directly to a shared branch.

## 10. Where to look for specific behavior

| Topic | Location |
|-------|----------|
| Tick entry, dispatchers, serialize/deserialize | `screeps-ibex/src/game_loop.rs` |
| Segment request/read/write | `screeps-ibex/src/memorysystem.rs` |
| Job trait and execution | `screeps-ibex/src/jobs/jobsystem.rs`, `jobs/data.rs`, `jobs/*.rs` |
| Mission system | `screeps-ibex/src/missions/missionsystem.rs`, `missions/*.rs` |
| Operation types and manager | `screeps-ibex/src/operations/` |
| Room entities and mapping | `screeps-ibex/src/room/`, `entitymappingsystem.rs` |
| Movement / pathfinding | `screeps-ibex/src/pathing/`, screeps-rover |
| Serialization format and markers | `screeps-ibex/src/serialize.rs` |
| WASM exports | `screeps-ibex/src/lib.rs` |
| Memory path access, creep cleanup | `screeps-ibex/src/memory_helper.rs` |

**Migration (0.23):** Entry is `lib.rs` (WASM exports `setup`, `game_loop_export`); RawMemory segment writes use JS `Reflect` in `memorysystem.rs`; hot-path unwraps were replaced with log-and-continue or optional handling. See in-code doc on `deserialize_world` in `game_loop.rs` for deserialization failure policy.

Use this document to ground edits in the correct layer (operation vs mission vs job vs room vs movement) and to respect persistence and tick semantics.
