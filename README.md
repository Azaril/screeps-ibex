# Screeps Ibex

**A fully autonomous [Screeps](https://screeps.com) AI written in Rust and compiled to WebAssembly.**

Screeps is a massively-multiplayer programming game: you don't play it directly, you write a program that the server executes once per game tick — forever, even while you're offline — to run an economy, expand an empire, and fight other players' programs. Ibex is one such program. It bootstraps a colony from a single spawn, plans and builds out its rooms, mines and hauls energy, expands across the map, exploits neutral and source-keeper territory, defends itself, and prosecutes wars against hostile players, all without human input.

The whole bot is Rust → WebAssembly. A thin JavaScript shim loads the module and calls into it each tick; everything else — strategy, planning, pathfinding, combat — is Rust. The codebase is one Cargo workspace, and the build-and-deploy path needs no npm, Node, or bundler.

---

## Table of contents

- [Highlights](#highlights)
- [Architecture](#architecture)
  - [Runtime model](#runtime-model)
  - [Behavior model: Operations, Missions, Jobs](#behavior-model-operations-missions-jobs)
  - [Economy & logistics](#economy--logistics)
  - [Room planning & base layout](#room-planning--base-layout)
  - [Empire strategy & expansion](#empire-strategy--expansion)
  - [Combat & military](#combat--military)
  - [Movement & pathfinding](#movement--pathfinding)
  - [Visualization & HUD](#visualization--hud)
- [Repository layout](#repository-layout)
- [Getting started](#getting-started)
  - [Prerequisites](#prerequisites)
  - [Clone the workspace](#clone-the-workspace)
  - [Build & verify](#build--verify)
  - [Configure a server](#configure-a-server)
  - [Deploy](#deploy)
  - [Running against a local private server](#running-against-a-local-private-server)
- [Runtime configuration](#runtime-configuration)
- [Tooling](#tooling)
- [Design documentation](#design-documentation)
- [Credits](#credits)

---

## Highlights

- **Pure Rust + WASM.** No game logic in JavaScript; the JS layer is only a loader.
- **ECS architecture.** All state lives in a [`specs`](https://crates.io/crates/specs) ECS `World` and is rebuilt deterministically each tick.
- **Reset-resilient persistence.** Long-lived state is serialized into RawMemory segments and survives VM resets behind a versioned, loud-on-failure seam.
- **CPU-governed.** A per-tick CPU-pressure governor sheds non-essential work under load while always running the survival core (defense, spawning, hauling, movement, persistence).
- **Algorithmic room planning.** A search-based planner (`screeps-foreman`) designs complete, defendable, fully-roaded base layouts and stages their construction across controller levels.
- **Threat-aware empire growth.** Expansion, remote mining, source-keeper farming, and abandoned-room salvage are each scored and gated on economics and safety.
- **Objective-driven combat.** A unified squad system sizes forces from measured threat, positions creeps via a single per-tile utility function, and validates engagements with a winnability model — with the tactical core shared, verbatim, by an offline combat simulator.
- **npm-free deploy.** `screeps-pack` takes the crate from `cargo build` to running code on any server in one command.

---

## Architecture

Ibex is organized as a stack of subsystems over a small, strict runtime. The sections below describe the design of each layer.

### Runtime model

The bot is invoked once per tick. A JavaScript loader (rendered at deploy time) lazily instantiates the WebAssembly module — deferring the multi-tick load until the in-game CPU bucket is healthy — calls a one-time `setup()`, then each tick calls `game_loop()`. The Rust entry point runs a single ordered pass of ECS systems over one `specs::World`.

- **Entity model.** There is one ECS entity per room, creep, operation, mission, and squad. Behavior is carried by enum components — `OperationData`, `MissionData`, `JobData` — that dispatch to concrete types implementing the corresponding trait. Rooms carry `RoomData`; creeps carry their owning reference, their job, and their movement state. Durable cross-references resolve through maps rebuilt every tick (e.g. room-name → entity) rather than through persisted entity indices, so a stale lookup is always a handled `None`, never a dangling pointer.

- **The tick.** A single macro defines the ordered system list, and both setup and execution expand from it, so registration and run order can never drift. Each tick: handle reset flags → load feature flags → get-or-create the ECS environment → request and gate on RawMemory segment readiness → deserialize the world once → run the system pass → clean up dead-creep memory → scrub any dangling entity references → serialize the world. Declaration order *is* execution order.

- **Persistence.** World state survives VM resets via RawMemory segments. The serialized payload is a 4-byte format-version fingerprint followed by a bincode-encoded component stream, gzip-compressed, base64-encoded, and chunked across a fixed set of segments. Loading reverses this and **rejects any payload whose fingerprint differs** — because bincode is positional, an old payload would otherwise decode as garbage. A compile-time segment registry asserts that no two subsystems share a segment id, and the engine's 10-active-segments-per-tick limit is respected by gating execution on the must-load set and lazily loading the rest. A dedicated, separately-versioned segment holds durable market history independent of the world format.

- **Loud failure.** Every reset is attributable. A version mismatch, a segment decode failure, or a mid-stream decode error each clears component storage and emits a logged cause plus a counted metric — a deliberate empty world, never a silent partial one. The crate is built `panic = "abort"`; if a tick panics or runs out of CPU, the JS loader destroys the isolate on the next tick (`Game.cpu.halt()`) to guarantee a fresh heap rather than continuing on corrupt state.

- **CPU governance.** At the start of each tick a single governor snapshot classifies CPU pressure into **Normal / Conserve / Critical** tiers from the bucket level and its least-squares trend. Every expensive system reads the same snapshot, so the whole tick makes one consistent decision. Each system carries a shed class: `Always` (never shed) or `SkipUnderCritical`. Under the Critical tier the scheduler skips the latter — intel gathering, summarization, room planning, rendering — while defense, spawning, hauling, movement, telemetry, and persistence always run.

### Behavior model: Operations, Missions, Jobs

All creep behavior is organized as a strict three-tier hierarchy, each tier an ECS component driven by its own pre-run and run system passes:

| Tier | Scope | Examples |
|---|---|---|
| **Operation** | Colony-wide, long-lived campaigns | run the whole colony, prosecute a war, claim/expand, scout, salvage, source-keeper farming |
| **Mission** | A single room or objective | supply a room, mine an outpost, build, upgrade, defend, run the labs, trade at the terminal |
| **Job** | One creep's work | harvest, haul, build, upgrade, mine, reserve, claim, dismantle, squad-combat |

Operations scan the world and create Missions; Missions request the creeps they need and attach a Job to each on birth; Jobs emit the actual game intents for a single creep. The set of live Operations, Missions, and Jobs *is* the bot's persistent behavioral state, reconstructed from components each tick. Operations are singletons-by-kind — a manager re-creates any missing kind every tick — so the campaign set self-heals after a reset.

Lifecycle flows parent-down and completion flows child-up: results (`Running` / `Success` / failure) bubble up through `child_complete` / `owner_complete` callbacks, and entity teardown is routed through a central cleanup queue so deletions can never strand a dangling reference at serialize time. Control flow *inside* a job or mission is an explicit state machine (generated by a `machine!` macro), while *selection* between targets and roles is utility-style scoring — explicit and testable where it needs to be, flexible where it pays off.

A creep gets its work through the spawn queue: a mission pushes a spawn request (body, priority, and a callback) into its room's queue; when the queue fulfills it, the callback constructs the concrete job and attaches it to the new creep. Every in-game action a creep takes flows through a single guarded intent sink modeling Screeps' simultaneous-action pipelines, so a creep fires at most one intent per pipeline per tick.

A key design rule keeps the tier traits **generic**: the executor systems only ever see the trait object obtained from a dispatch enum and never branch on the concrete variant. Adding a behavior is adding an enum arm and a trait impl — no change to the machinery, and no caller-specific logic leaks into the shared seam.

### Economy & logistics

The economy is the bot's circulatory system, and almost all of its coordination state is **ephemeral and rebuilt every tick**, which is what makes it reset-resilient by construction.

- **Hauling.** A per-tick supply/demand store is populated lazily: each room registers generator closures that, on first query, enumerate every structure and dropped resource into prioritized withdraw/deposit requests. Requests are keyed by (resource, priority, transfer-type), where the transfer-type partition (Haul / Link / Terminal / Use) ensures a hauler only ever sees requests it can serve. An idle hauler greedily matches one (pickup, delivery) pair ranked by `amount / (pickup_distance + delivery_distance)` using cheap linear distance — **no pathfinding in the matcher**. Per-key reservation accounting keeps greedy, per-creep matching mutually consistent within a tick regardless of order.

- **Spawning.** Missions push spawn requests into a per-room queue kept in descending priority order. Priorities are banded (critical miners > forming-squad slots > high economy > medium > low), so combat can win the energy-banking race without starving income, and miners are never preempted. A shared spawn token de-duplicates one logical creep submitted to several rooms — it spawns once, in whichever room has a free lane first. Bodies are sized to the room's economy; creep renewal runs only *after* the priority spawn pass and only on otherwise-idle spawns, so it can never preempt a needed creep.

- **Labs & boosts.** Each owned room runs a reaction state machine that picks a target compound by recursively expanding what it needs against what it has, runs the highest-volume feasible reaction (or decomposes excess via reverse reactions), and routes all lab input/output over the same transfer system.

- **Market & terminal.** Terminal contents are tiered per resource into reserve / transfer / passive-sale / active-sale bands. A trading pass runs on a throttled cadence under a CPU gate and prices every order off a **fair-value oracle** — a manipulation-resistant trailing-median with liquidity floors and anomaly detection — never a raw latest-day price. A persisted exposure ledger caps buy spend and per-resource traded volume over a rolling window, so even a fooled oracle is bounded. This market state is the one piece of economic data that persists, on its own versioned segment.

- **Power.** A dedicated mission keeps the power spawn supplied with energy and power over the transfer system and processes power whenever both are available, raising the empire's Global Power Level.

- **Room economics.** A pure, deterministic kernel computes the energy-equivalent net return of *controlling* a room (gross source yield minus hold, mining, haul, and CPU costs, projected over a horizon). The same kernel feeds both expansion scoring and combat target valuation, so "is this room worth it?" is answered the same way everywhere.

### Room planning & base layout

Owned-room layouts are designed by **`screeps-foreman`**, a standalone, pure-Rust planner. It runs a lazy depth-first search over an ordered stack of placement layers — each emitting candidate placements one at a time — and keeps the highest-scoring complete layout that passes hard feasibility gates.

- **Search.** The dominant search axis is the hub anchor position; an anchor layer ranks open tiles by a composite score and emits only the top-K (a beam), so the expensive downstream layers run on a handful of promising hubs rather than the whole room. Core structures are placed as compact stamps near the anchor, giving a **defendable-by-construction** footprint.
- **Scoring.** A weighted set of scoring layers contributes normalized sub-scores — source proximity, controller and mineral distance, terrain openness, defensibility, tower coverage, road transport cost, road connectivity, and upkeep. Because every sub-score is in `[0, 1]`, the search uses an admissible optimistic-completion bound that prunes provably-losing branches without ever discarding the eventual best.
- **Defense.** The wall/rampart perimeter is a true max-flow / min-cut (Dinic's algorithm) over a vertex-split flow network, computed last so it encloses the complete structure set. The cut is coverability-weighted — tiles a tower can reach are cheaper — yielding a compact, tower-coverable perimeter rather than one that merely seals.
- **Connectivity.** A dedicated layer floods the pruned road network from the hub and guarantees every interactable structure has an on-network road approach tile, recorded on the plan for deterministic, fully-roaded logistics pathing.
- **Construction.** Each structure is assigned the minimum controller level at which it is allowed, producing a staged build order. The bot executes it incrementally over the room's lifetime, diffing the plan against the live room and placing construction sites under a policy filter (deferring walls until they matter, roads until something uses them, and never sealing a spawn exit mid-build).

Planning is multi-tick and CPU-budgeted: in-progress search state is persisted to a memory segment and resumed across ticks and VM resets, the anchor beam escalates on failure so a plannable room is never lost, and rooms that fail back off exponentially. The planner compiles natively (game-API calls sit behind a feature flag), which is what makes the offline planner benchmark possible.

### Empire strategy & expansion

A small set of always-running singleton operations decide where and when to grow, and how to exploit neighboring rooms — all threat-aware.

- **Expansion.** Candidate rooms are discovered by breadth-first search from home rooms and scored on source count, walkability, distance band, and plan quality; the best within a score delta is claimed. Distance scoring deliberately favors the remote-mining ring (peaking a few rooms out) and penalizes immediate neighbors, which a new colony would cannibalize. Admission is governed by the hard GCL ceiling plus a **measured** CPU-capacity model (CPU-used per owned room, with headroom), not a fixed guess.
- **Claiming is abort-by-default.** A two-tier safety gate vetoes contested rooms — cheap dynamic checks during candidate gathering, then a rich threat veto at commit time (threat level, hostile DPS, towers, incoming nukes) plus an intel-freshness requirement. Absent fresh intel reads as *unsafe*, so the bot re-scouts rather than committing blind. A claimed room is continuously re-validated and instantly abandoned if it becomes an unwinnable contested claim.
- **Remote mining.** Neutral rooms are reserved and mined from nearby colonies with miners and long-haul haulers, each gated on a per-room safety predicate.
- **Source-keeper rooms** are exploited — never owned — via a pure ROI scorer that commits a persistent suppression farm with hysteresis, fielding a low-priority squad to keep the keepers down while miners work around them.
- **Salvage.** Abandoned and derelict rooms are scanned, gated on expected value, then looted and dismantled for the energy refund; strategically valuable ones are de-claimed to free them for a future mining outpost.
- **Visibility.** Everything above is fed by a register/broker/fulfill intel pipeline: consumers register room-visibility requests with a priority and observe/scout flags; a queue coalesces and expires them; room-8 observers fulfill what they can reach cheaply and creep scouts fulfill the rest, with an exponential backoff that suppresses scouts (not observers) for rooms a scout can't reach.

### Combat & military

Combat is **fully objective-driven** through a pull architecture. Producers — the war coordinator, defense scans, the claim pipeline, the source-keeper farm — upsert idempotent objectives (secure, defend, dismantle, harass, farm, escort, declaim) onto a single global, persistent priority/TTL queue. One perpetual `SquadManager` system claims objectives, sizes and fields squads, and computes per-tick tactical orders. Because work is queue-owned-and-pulled rather than mission-owned-and-pushed, a producer that completes or dies never strands a squad.

- **Capability-driven force composition.** A doctrine classifier maps an objective to a parts-level requirement vector (heal, dismantle, anti-structure, anti-creep, and tough parts as independent dimensions); a deterministic assembler turns that vector directly into a sized squad — no static templates or body catalogs. The composition is then refined by a bit-deterministic expected-value search over over-power and armor ladders.
- **Winnability oracle.** Before committing, a conservative Lanchester model decides whether one squad can take a defended target and *how* — direct **breach** (out-heal the towers and dismantle through the breach corridor) versus **drain** (a tank soaks tower fire at the damage-falloff standoff until the finite towers bleed dry, then the squad breaches). It accounts for tower energy (a drained tower deals zero damage), the engine's tower-damage curve, the cheapest breach corridor, and the squad's on-site time budget. A "yes" is safe to commit; an unwinnable target is marked with exponential give-up backoff (defense objectives are never abandoned).
- **One position-selection utility.** Every tactical move comes from a single signed per-tile score — a weighted sum over cached per-room fields (an integer threat field, a reachability flood-fill, wall-aware cohesion distance, focus damage, openness). Fleeing, holding, and closing all *emerge* as the argmax under objective-selected weight presets (kite, engage, breach, healer, drain), wrapped by a handful of hard safety guards (critical-HP flee, cohesion rejoin, survival-horizon veto).
- **Expected-value targeting.** Focus fire selects the hostile whose death removes the most enemy capability per tick, discards the unkillable (out-healed or rampart-shielded), and spills damage across ranked targets capped at each one's per-tick kill budget so the squad never over-commits to a single creep. Retreat uses coupled hysteresis on squad-average and per-member HP.
- **Squads as moving formations.** A squad follows a virtual, footprint-aware anchor along a cached pathfound route; members move in lockstep on rotated offsets, present armor toward the threat, and collapse to single-file in corridors. Small squads use rigid offsets; larger blobs use loose-centroid cohesion.
- **Defense, towers, safe-mode, nukes.** Defense emits objectives at a threat's *current* room (with an asset-priority boost and an over-extension leash) and follows roaming threats; a defender is committed on body composition, not just attack parts, so a controller-attacking claimer in a towerless room is still caught. Towers focus-fire net-positive targets and detect bait/drain attempts via the hitpoint sawtooth. Safe-mode is a guarded last resort; nuke defense fortifies ramparts ahead of impact.
- **No opponent-specific constants.** Threat is *measured* at runtime from each hostile body; force is sized from that measurement; kiting is a range/fatigue policy; cohesion is a geometric invariant. There is nothing tuned to a particular enemy to overfit against.

Crucially, the entire tactical core — target selection, focus fire, heal assignment, kiting, engagement, and the position utility — lives in a **JS-free `screeps-combat-decision` crate over plain value-type data**, so the live bot and an offline combat simulator run *the same code*. There is no second, diverging combat implementation.

### Movement & pathfinding

Creep movement is request-based, built on the standalone **`screeps-rover`** crate. Jobs never call the engine's `moveTo`; they record a semantic intent — move-to, follow, or flee — plus policy (priority, whether the creep may be shoved or swapped, an optional anchor that keeps a stationary worker near its work tile, and hostile-room behavior). One system then resolves the entire swarm at once.

Resolution runs in deterministic passes: topologically sort follow-dependencies (leaders before followers), compute each creep's desired tile (reusing cached paths, regenerating only on change/expiry/stuck), then arbitrate tile contention — head-to-head swaps first, then contested tiles in convoy order, with the highest-priority creep winning a tile and the occupant chain-shoved or side-stepped via local avoidance. Stuck recovery is an escalation ladder (avoid nearby friendlies → avoid all → more search ops → enable shoving → report failure). Every tie is broken by a stable key, so the resolver is **bit-reproducible** — which is what lets the offline combat sim replay deterministically.

Movement is never fully shed (creeps never freeze), but its CPU generosity scales with the governor tier and is bounded by a per-tick ops pool and a hard movement CPU cap. Mission-side "nearest by path" searches and an inter-room route cache live in a separate, tier-scaled pathfinder service that serves stale routes under Critical pressure rather than triggering recompute storms. Cost matrices are cached per room; the structure layer persists to a memory segment across resets while creep and construction-site layers are rebuilt each tick.

### Visualization & HUD

The bot renders an on-screen overlay that is **shed-first** — the first thing dropped under CPU pressure — and never on the critical path. All primitives batch into a buffer and flush once at end of tick, every coordinate is finiteness-checked, and an exact per-target byte budget guarantees a malformed visual is dropped (with telemetry) rather than aborting the tick.

The HUD design is exception- and anchor-oriented: rather than text rosters in solid panels, it surfaces anomalies (failed, stalled, or zero-creep missions) as compact glyph rows on translucent edge rails, attaches data to where it lives as world-anchored badges (sources, controller, spawn, squad members), and keeps the room center clear so the game stays visible. Detail is gated by an altitude model — Off / Ambient / Panels / Debug — with the effective level per domain (economy, military, intel, infrastructure, pathing) being the minimum of the master level and that domain's feature flag.

---

## Repository layout

The whole project is one Cargo workspace. The bot crate is WASM-only; pure support crates build for both host and WASM; host-only tool crates are excluded from the WASM build.

```
screeps-ibex/
├── screeps-ibex/            # The bot crate (cdylib) — all the AI logic
├── screeps-ibex-metrics/    # Metrics helpers
├── screeps-common/          # Shared types
├── screeps-cache/           # Caching utilities
├── screeps-machine/         # State-machine macro (machine!)
├── screeps-rover/           # Movement, pathfinding, traffic resolution
├── screeps-foreman/         # Room layout / base planner
├── screeps-visual/          # Visualization primitives
├── screeps-timing/          # Profiling (optional, behind the `profile` feature)
├── screeps-timing-annotate/ #   └─ profiling proc-macro
├── screeps-combat-engine/   # Deterministic JS-free Screeps combat-tick simulator
├── screeps-combat-decision/ # JS-free tactical decision layer (shared by bot + sim)
├── screeps-combat-agent/    # Sim adapter: runs the bot's real decisions headlessly
├── screeps-combat-eval/     # Combat policy / experiment register over the sim
│
├── screeps-pack/            # ★ Build + deploy pipeline (host-only)
├── screeps-server-kit/      # Private-server toolkit (host-only)
├── screeps-prospector/      # Room/world analysis & spawn-site selection (host-only)
├── screeps-rest-api/        # Screeps REST + websocket client (host-only)
├── screeps-foreman-bench/   # Offline room-planner benchmark (host-only)
├── screeps-ibex-eval/       # Evaluation harness (host-only)
│
├── js_src/                  # The in-game JS loader (reference copy)
├── docs/                    # Design documentation (architecture decision records)
├── Cargo.toml               # Workspace root; patches screeps-game-api + member crates
├── rust-toolchain.toml      # Pins nightly + the wasm32 target
├── .cargo/config.toml       # build-wasm / clippy-wasm / check-wasm / test-host aliases
└── .example-screeps.yaml    # Template server-credentials file
```

The main bot crate is organized by the behavior tiers and supporting systems:

```
screeps-ibex/src/
├── lib.rs              # WASM exports: setup(), game_loop()
├── game_loop.rs        # Tick orchestration, ECS setup, serialize/deserialize, scheduler
├── operations/         # High-level campaigns (colony, claim, war, scout, salvage, …)
├── missions/           # Room/objective tasks (supply, build, upgrade, defend, labs, …)
├── jobs/               # Per-creep work (harvest, haul, build, upgrade, squad-combat, …)
├── military/           # Combat objective queue, squad manager, formations, threat map
├── room/               # Room data, visibility, room-plan execution
├── pathing/            # Movement system, pathfinder service, cost-matrix store
├── transfer/           # Hauling supply/demand queue, market order queue, fair value
├── spawnsystem.rs      # Spawn queue & body design
├── room_economics.rs   # Pure net-ROI kernel
├── cpugovernor.rs      # CPU-pressure tiers
├── memorysystem.rs     # RawMemory segment arbiter
├── serialize.rs        # Segment encode/decode + entity-ref wrappers
├── features.rs         # Runtime feature flags
└── visualization.rs    # On-screen overlay / HUD
```

---

## Getting started

### Prerequisites

- **Rust nightly with the WASM target.** The toolchain is pinned by `rust-toolchain.toml`; nightly plus the `rust-src` component are required because the build uses `-Z build-std`.

  ```bash
  rustup target add wasm32-unknown-unknown
  rustup component add rust-src
  ```

- **A checkout of `screeps-game-api` next to the workspace.** The root `Cargo.toml` patches `screeps-game-api` to a sibling path `../screeps-game-api`, so it must be checked out alongside this repository:

  ```
  parent/
  ├── screeps-ibex/         # this repo
  └── screeps-game-api/     # https://github.com/rustyscreeps/screeps-game-api
  ```

- **An auth token (official servers) or username/password (private servers).** Get a token from your [Screeps account → Auth Tokens](https://screeps.com/a/#!/account/auth-tokens).

Nothing else — no Node, npm, webpack, or wasm-pack. The deploy tool handles the entire WASM toolchain itself.

### Clone the workspace

The support crates are git submodules, so clone recursively (and check out `screeps-game-api` as a sibling):

```bash
git clone --recurse-submodules https://github.com/Azaril/screeps-ibex.git
git clone https://github.com/rustyscreeps/screeps-game-api.git   # sibling of screeps-ibex
```

If you already cloned without `--recurse-submodules`:

```bash
cd screeps-ibex
git submodule update --init --recursive
```

### Build & verify

The workspace mixes WASM-only and host-only crates, so building everything for one target won't work. Use the cargo aliases (defined in `.cargo/config.toml`):

```bash
cargo build-wasm   -p screeps-ibex   # build the bot for wasm32-unknown-unknown
cargo clippy-wasm  -p screeps-ibex   # lint the bot (and other wasm crates)
cargo check-wasm                     # type-check the whole wasm side
cargo test-host                      # build & run all host-side tests natively
```

`build-wasm` / `clippy-wasm` / `check-wasm` build `--workspace --target wasm32-unknown-unknown` while excluding the six host-only tool crates (which depend on tokio/reqwest/etc. and can't target WASM). `test-host` is a plain `cargo test` that builds and tests everything natively. No global build target is set, so a bare `cargo build`/`cargo test` is host-native.

### Configure a server

Copy the example credentials file and fill in your server(s):

```bash
cp .example-screeps.yaml .screeps.yaml
```

`.screeps.yaml` follows the [screepers unified credentials format](https://github.com/screepers/screepers-standards/blob/master/SS3-Unified_Credentials_File.md). Each entry under `servers:` is named; official servers authenticate by `token`, private servers by `username` + `password`. Per-server build flags live under `configs.wasm-pack-options`, where the `'*'` key applies everywhere and per-server entries are appended:

```yaml
servers:
  mmo:
    host: screeps.com
    secure: true
    token: your-auth-token-here
    branch: default
  private-server:
    host: 127.0.0.1
    port: 21025
    secure: false
    username: you
    password: your-password
    branch: default

configs:
  wasm-pack-options:
    # Keep the emitted wasm inside the MVP feature set the Screeps Node version accepts.
    '*': ["--config", "build.rustflags=['-Ctarget-cpu=mvp']", "-Z", "build-std=std,panic_abort"]
    # Enable the `mmo` crate feature (intershard + pixel APIs) only where it applies.
    mmo: ["--features", "mmo"]
    ptr: ["--features", "mmo"]
```

Tokens and passwords are held in redacted secret strings end-to-end and are never logged or written into build artifacts.

### Deploy

Deployment is owned end-to-end by **`screeps-pack`**: it runs `cargo build` for WASM, fetches the exact `wasm-bindgen` your lockfile pins, patches the generated glue for the Screeps isolate, optionally runs `wasm-opt`, and uploads the resulting three-module map over the REST API. Run it from the workspace as a sibling crate:

```bash
# Preview the plan — resolve the server entry & cargo args, build nothing, upload nothing:
cargo run --manifest-path screeps-pack/Cargo.toml -- check  --server private-server

# Build the full module map locally, no upload (artifacts land under target/screeps-pack/dist/):
cargo run --manifest-path screeps-pack/Cargo.toml -- build  --server private-server

# Build and upload to the server entry's branch:
cargo run --manifest-path screeps-pack/Cargo.toml -- deploy --server private-server
cargo run --manifest-path screeps-pack/Cargo.toml -- deploy --server mmo
```

Useful flags: `--debug` makes a fast (much larger) unoptimized build; `--dryrun` builds everything and prints the module map without uploading. On the first run, `screeps-pack` downloads and caches the lockfile-matched `wasm-bindgen` CLI and a pinned `wasm-opt` under `target/screeps-pack/tools/`. A successful deploy prints the uploaded module names, sizes, SHA-256 hashes, and the fraction of the 5 MiB code-size limit used.

> The released code is uploaded against the **5 MiB** Screeps code-size limit. `screeps-pack` warns if a build is over budget but uploads it anyway (the server rejects it). If you hit this, drop `--debug` and make sure `wasm-opt` ran.

Once uploaded, an in-game JS loader handles boot: it waits until the CPU bucket is healthy, instantiates the WASM module in idempotent stages across several ticks, runs one-time setup, and then calls into the bot every tick — destroying and recreating the isolate if a tick ever errors.

You can also install the CLI standalone (`cargo install --git https://github.com/Azaril/screeps-pack`) and use the bare `screeps-pack deploy --server <entry>` form from anywhere.

### Running against a local private server

For local iteration, point a `private-server` entry at a [Screeps private server](https://github.com/screeps/screeps) (default `127.0.0.1:21025`) and `deploy --server private-server`. The `screeps-server-kit` crate provides a bot-agnostic toolkit for driving a Dockerized private-server stack — bringing the server up/down, bootstrapping a world, deploying, controlling ticks, and capturing console/metrics output — programmatically over the same pipeline.

---

## Runtime configuration

The bot is configured at runtime through feature flags stored in the game `Memory` tree under `Memory._features`, read fresh each tick. Every flag has a safe default, so an empty configuration runs the full bot. Flags are grouped by subsystem — `visualize`, `construction`, `market`, `transfer`, `remote_mine`, `pathing`, `room`, `military`, `claim`, `derelict`, `source_keeper`, `visibility` — plus top-level toggles such as `raid`, `dismantle`, and `system_timing` (per-system CPU logging). These act as kill-switches: turning one off retires the associated role or behavior cleanly without disrupting the rest of the colony.

One-shot **reset flags** live under `Memory._features.reset` and are consumed once: `environment` rebuilds the ECS world, `memory` clears all persistent segments, and `room_plans` discards cached room layouts. These are the supported way to force a clean restart.

The bot crate also exposes three compile-time Cargo features: `profile` (enables Chrome-trace CPU profiling via `screeps-timing`), `sim` (the Screeps simulation API), and `mmo` (intershard and pixel APIs, for the official servers).

---

## Tooling

Beyond the bot and its runtime support crates, the workspace ships a family of host-side tools that share the same REST client and credentials file:

- **`screeps-pack`** — the npm-free build + deploy pipeline described above; usable as a CLI or a library.
- **`screeps-server-kit`** — a bot-agnostic local private-server toolkit (lifecycle, world bootstrap, deploy, tick control, console/metrics capture) over a Dockerized stack.
- **`screeps-prospector`** — scans and scores rooms and recommends or places a first spawn, using the same `screeps-foreman` planner the bot runs.
- **`screeps-rest-api`** — the shared async Screeps HTTP + websocket client (token / username-password auth, rate-limit pacing, secret-wrapped credentials).
- **`screeps-foreman-bench`** — an offline benchmark and visualizer for the room planner, producing PNG/JSON output across many rooms.

The combat crates form a self-contained simulation stack: **`screeps-combat-engine`** is a deterministic, JS-free port of one Screeps combat tick; **`screeps-combat-decision`** is the tactical decision layer the *live bot* uses, over plain value-type data; and **`screeps-combat-agent`** runs the bot's real decision code over the engine for headless self-play. Because the live bot and the simulator share one decision implementation, combat behavior can be validated offline without a running server.

---

## Design documentation

The rationale behind each subsystem — the alternatives considered and the trade-offs made — is recorded as a set of architecture decision records under [`docs/design/`](docs/design/), covering the entity model, serialization, behavior modeling, CPU governance, hauling, room planning, spawn orchestration, the market, the power economy, empire strategy, combat, and visualization. Engine ground-truth mechanics are documented under [`docs/references/`](docs/references/).

---

## Credits

- Built on [`screeps-game-api`](https://github.com/rustyscreeps/screeps-game-api) and the broader [rustyscreeps](https://github.com/rustyscreeps) ecosystem; the in-game loader follows the [`screeps-starter-rust`](https://github.com/rustyscreeps/screeps-starter-rust) convention.
- The credentials file format is the [screepers SS3 unified credentials standard](https://github.com/screepers/screepers-standards).
- Game documentation: [docs.screeps.com](https://docs.screeps.com/) · [API reference](https://docs.screeps.com/api/).
