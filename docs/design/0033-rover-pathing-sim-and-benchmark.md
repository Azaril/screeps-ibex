# ADR 0033 — Rover pathing & movement simulator + benchmark (the "validated separately" mover harness)

- **Status:** Proposed — **M0 (`screeps-sim-core` kernel extraction) + M2 (Tier-A `R_fatigue` benchmark) + M3-physics (roads + loaded-CARRY SHARED-FIXES) LANDED 2026-07-01** (byte-identical / default-inert, host + wasm green; §D1a + the §D3 AS-BUILT notes are reconciled to the as-built code). The Tier-A travel-time metrics (`R_ticks` / `movement_eff` / `fatigue_util` over a kernel-driven `traverse.rs`), the **scenario harness** (`scenario.rs`: catalog + seeded generator + validator, on a now-shared `sim-core::rng`), and **M1** (the rover driver now lives in `sim-core[rover]`, generic over an injected cost source) also landed 2026-07-01. M4/M5 (the Tier-B *contention* core — full `MovementSystem` resolver, congestion, C/D/E-families, CPU bench) pending.
- **Date:** 2026-06-29
- **Deciders:** William Archbell
- **Related:** [0006](0006-eval-and-iteration-harness.md) (the sibling combat micro-sim pattern — host-only, deterministic, drives the bot's own code behind a seam, fidelity = conformance/parity/canary; **this ADR is 0006 applied to the rover/movement subsystem**); [0023](0023-nroom-combat-sim.md) + [0023a](0023a-staged-combat-harness.md) (the N-room combat sim that **runs rover but never measures it** — since P-MOVE+ / task #30 it drives rover's `MovementSystem` + resolver + `LocalPathfinder` + `AnchorPath` to *produce* the move directions (`resolve_moves_via_system`, `pathing.rs:477`), then scores only *combat* outcomes; it never grades rover's path optimality / fatigue / congestion / ops. **0033 measures what 0023 only exercises.** NB: 0023's older "Architecture finding" prose — "it does NOT run the bot's rover/`MovementSystem`/`AnchorPath`" — is **stale**, superseded by 0023's own P-MOVE+ status line; trust the code); [0015](0015-testing-and-validation-strategy.md) (owns the L0–L6 taxonomy, the assertion-form rule, the flake policy, **seam S3** the pathfinding facade, the "no exact pathfinding routes" untested line `0015:130`, and the determinism prerequisite `0015:125`; **0015 owns taxonomy/policy/S3-shape, 0033 owns the rover sim substrate + movement metrics slotting into L0–L5**); [0009a](0009a-room-planner-performance.md) (prior pathing-perf precedent — corpus benchmark of a search-heavy subsystem); [0004](0004-cpu-governance-and-load-shedding.md) (owns S3 + ops-saturation telemetry; 0033's CPU/ops gates attach to S3, no new seam); `docs/references/engine-mechanics.md` §1.6 (the movement/fatigue ground truth 0033 must honor and cite); the **[Sim determinism fence]** (combat-eval `sim_is_deterministic_over_rounds`, spread-0; the rover `resolver.rs` seed-flaky HashMap iterations were the historical offender — 0033 generalizes that fence to movement); `screeps-foreman-bench` (the corpus-over-CLI structural template).

**Division of ownership (state it up front so a reviewer never has to derive it):** ADR 0023 validates the bot's *combat decisions and outcomes* over a world — it **drives rover to produce** the move directions (P-MOVE+ / `resolve_moves_via_system`) and the engine to apply them, but it scores only the fight, never the mover; ADR 0006 owns the *combat* micro-sim and the colony-health score; ADR 0015 owns the *taxonomy and policy* (which layer asserts what, flake budget, seam S3 contract); ADR 0004 owns the *ops pool* (seam S3 + saturation telemetry). **This ADR owns exactly one thing none of them measure: the quality and cost of the rover mover itself** — route optimality, fatigue efficiency / time-spent-moving, congestion behavior, algorithmic/ops CPU, and movement determinism — validated offline against engine ground truth. Subjects are disjoint; the seam is shared.

---

## Context

### §0 The pain, and why now

`screeps-rover` is the single most-leaned-on subsystem in the bot that has **no first-class validation of its own quality or cost.** Every hauler, upgrader, claimer, scout, and combat squad routes through it. The combat program now depends on it heavily — multi-room A* (`LocalPathfinder::search`), the traffic resolver wired in via `resolve_moves_via_system`, anchor footprint pathing (`AnchorPath`), the fatigue gate. Yet:

- The combat sim (ADR 0023) **runs rover but never measures it.** Since P-MOVE+ (task #30) the sim drives the *live* rover stack — `MovementSystem` + resolver + `LocalPathfinder` + `AnchorPath` — to produce each tick's move directions (`resolve_moves_via_system`, `pathing.rs:477`; the single-creep `resolve_move_direction`, `pathing.rs:198`), then hands them to the combat-engine's `resolve_tick` and scores the *fight* (DPS, healing, cohesion, survival). Rover is **exercised on every tick, measured on none** — no combat metric grades its path optimality, fatigue/time-moving, congestion, ops, or determinism. (0023's older "Architecture finding" line — "it does NOT run the bot's rover/`MovementSystem`/`AnchorPath` … validated by rover unit tests + live, **not** here" (`0023:11`) — predates that wiring and is **stale**; 0023's own P-MOVE+ status line is the current truth. This is exactly the "trust the code, not the combat-status docs" hazard this project has hit before.)
- Seam S3's gates (ADR 0004 / `0015:71`) are real but **thin** — "ops never exceed the per-tick pool; `MIN_PATHFIND_OPS` floor at Critical; unreachable → `Failed`, never a hang." There is no harness that drives them richly over a corpus.
- The only existing correctness evidence is **ad-hoc unit tests** in `resolver.rs`/`local_pathfinder.rs` plus the determinism fence that was discovered **accidentally** — a kiting combat test flaked ~50% of the time and the root cause turned out to be three seed-ordered `std::HashMap` iterations in `resolver.rs` (`topological_sort_follows`, `current_pos_to_entity`, swap discovery), fixed with `Handle`-sorted tie-breaks. A mover regression today silently corrupts every squad and every economy hauler, and we would learn about it the way we learned about the seed-flake: by something downstream breaking.

The operator's headline asks are concrete: *"time creeps spend moving, efficient usage of fatigue, optimal pathing"* and *"algorithmic/cpu benchmarking."* Neither is measurable today. This ADR designs the harness that makes them numbers.

### §1 The constraint litany (the forces every ADR in this corpus respects)

- **Single-threaded WASM, per-tick CPU budget including intents.** Movement is a large slice of that budget (`MAX_PATHFIND_OPS=20_000`, `DEFAULT_PATHFIND_OPS_BUDGET=20_000`, the live `MovementSystem` already meters `ops_consumed` and sheds load). The benchmark's primary CPU number must therefore be **op count** (deterministic, MMO-relevant), with host wall-clock as a *secondary* regression proxy only — host wall-clock ≠ MMO CPU and is blind to intents (`0015:20`, `0015:133`).
- **VM-reset resilience** — irrelevant to an offline host harness directly, but the persisted per-creep `StuckState`/`CreepPathData` cache that the sim must own across ticks mirrors the across-tick state the live bot persists.
- **Incremental migration with a stable seam.** The seam already exists: the rover crate's `screeps` cargo feature. WITHOUT it the crate compiles and runs on native targets using only pure data types from `screeps-game-api` (`Position`, `RoomName`, `LocalCostMatrix`, `Direction`) — no JS interop. WITH it, `screeps_impl.rs` provides the real-game trait implementations. An offline simulator is just a second consumer of that already-shipped seam.
- **Determinism is doctrine.** The project's determinism fence (`sim_is_deterministic_over_rounds`, spread 0) is non-negotiable, and `resolver.rs` is the *documented historical source* of nondeterminism. A rover benchmark is exactly where a directly-targeted rover determinism gate belongs.

### §2 The one architectural fact that frames the whole design: rover plans, it does not simulate physics

`MovementSystem::process()` (`movementsystem.rs:459-797`) turns intents into a per-creep `Direction` (or a pull pairing) and **stops**. It *reads* `creep.fatigue()` and `creep.spawning()` to gate a creep (`:510-514` — `if fatigue() > 0 || spawning()` → report `Moving`, skip), and it *writes* the move via `move_direction(dir)`. It never:

- computes fatigue accrual or decay,
- resolves the engine's authoritative tile-contention winner (engine `rate1`/pulled/pulling/moves-weight — `engine-mechanics.md §1.6`; rover's *own* resolver uses a different RTS tie-break: `(priority, stuck_ticks, Handle)`, `resolver.rs:424-443`),
- relocates a creep across a room edge, or
- moves a creep at all.

Those are the **server's** job. The clean reading: there are **two distinct movement resolvers** — rover's (navigation quality: which tile/path, cheaply, deterministically) and the engine's (the authoritative adjudicator of the moves rover requests, with fatigue and weight contention). This is confirmed by the only existing offline `process()` caller, `screeps-combat-agent/src/pathing.rs`: `CombatCreepHandle::move_direction` records the direction into a sink (`:333-336`); `fatigue()` returns a value the *engine* set (`:327-329`); `resolve_moves_via_system` (`:477-518`) runs rover, collects directions, and hands them to the combat-engine's `resolve_tick`, which alone applies movement + fatigue (`resolve.rs:494-518`) and resolves real contention (`movement.rs`).

**Consequence for this ADR:** the rover sim is a **two-halves tick loop** — (A) rover `process()` = the planner under test; (B) the authoritative "server" half — the combat engine's movement tick, **reused** (extracted to `sim-core` as `resolve_movement`, §D1/§D3), not re-built — that applies moves, accrues/regenerates fatigue, resolves engine-true contention, and relocates edge-crossers. The benchmark measures the quality of (A)'s decisions against a faithful (B). **A "fatigue efficiency / time-moving" metric therefore lives entirely in half (B); there is nothing to measure inside rover for fatigue** — rover's whole fatigue model is the single binary read at `:510`. Conflating the two halves (trusting rover's own resolved `final_pos` as if it were the executed move) would hide the exact failure class the sim exists to surface, because the two resolvers use different tie-break rules.

### §3 What is in scope vs out of scope

**IN scope** — offline, deterministic, host-only validation of rover for:

- single-room and N-room (≤ `MAX_PATHFIND_ROOMS`) **pathing** — route optimality, length, oscillation, incomplete/fail rate, ops-per-search;
- **movement execution / fatigue** — ticks-to-arrive, fatigue stalls/waste, terrain-rate (road 1 / plain 2 / swamp 10), sustained-speed inequality, edge-zeroing, power-creep no-fatigue, the idle decomposition (fatigued vs blocked);
- **multi-creep congestion** — shove/swap/local-avoidance, trains/pulls, deadlock/livelock detection, throughput/makespan;
- **flee/kite** correctness and **anchor** stability;
- **algorithmic/CPU** cost — ops-count gates (primary) + wall-clock regression (secondary), scaling curves;
- **determinism** as a first-class gate.

**OUT of scope** (owned elsewhere, do not duplicate):

- **combat resolution** — damage, healing, towers, focus-fire, kite *decisions*: owned by ADR 0023 / `screeps-combat-engine`. This sim consumes a body and a route; it never fires a ranged attack.
- **economy / colony / the full game tick** — owned by ADR 0006's colony-health sim.
- **the S3 seam contract shape** (ops pool semantics, `MIN_PATHFIND_OPS` floor) — owned by ADR 0004 / 0015; this sim *exercises and gates* S3 but does not redefine it.
- **exact pathfinding routes as golden snapshots** — explicitly forbidden by `0015:130` ("snapshot the *plan*, not the path; assert route *properties* at L2"). This sim asserts route **properties and distributions**, never "the path is exactly `[tiles]`."

This is the **rover-scoped realization of the L0–L6 taxonomy**, the movement analogue of ADR 0006's combat micro-sim.

---

## Decision

**Reuse the combat sim's mover; do not rebuild it.** The combat sim is already a working two-halves mover — rover *plans* (`resolve_moves_via_system` runs the real `MovementSystem` + resolver) and the engine *applies* (`resolve_tick` does fatigue / contention / edge-exit) — it just scores combat, never the mover (see §0). So this ADR makes **two moves**: (1) **extract `screeps-sim-core`** — the general, combat-agnostic Screeps movement mechanism (world, terrain, body, the same-tile contention resolver, the move/fatigue/edge-exit tick, the recording, the deterministic RNG, and the offline rover driver) lifted out of `screeps-combat-engine` into a shared lower crate **both** sims depend on; (2) **create `screeps-rover-eval`** — a host-only *measurement* crate that drives the *real* rover `MovementSystem` / `LocalPathfinder` / `resolver` over a `screeps-sim-core` world, applies the issued moves through sim-core's authoritative movement tick (**no re-ported physics, no new `RoverWorld`**), and emits a gated rover-quality metric set (path optimality, fatigue / time-moving, congestion, ops) with **determinism as the headline ship-blocker**. The only net-new *physics* is two fidelity fixes — **roads** and **loaded-CARRY** fatigue — that land in `screeps-sim-core` and improve **both** sims.

> **Note (decision provenance):** an independent reuse-surface analysis recommended *against* the extraction — it found the shared primitives "already correctly layered" in `screeps-combat-engine`/`screeps-combat-agent` and judged a `screeps-sim-core` crate "churn for zero new sharing today." The operator chose the extraction anyway, for the **cleanest end state** (operator directive: *target the cleanest design, not incremental tech debt*) — **clean layering** (a movement bench should not depend on a crate named `combat-engine` or a world named `CombatWorld`), a full `Combat*`→`Sim*` rename of the shared kernel, and a **reusable mechanism foundation** for future non-combat sims (economy / hauling / lifecycle — ADR 0028, `screeps-ibex-eval`). The trade-off (extraction + rename churn vs. lower-churn reuse-in-place) is recorded in [Alternatives](#alternatives-considered); the component-level reuse map below is unchanged by the choice — only the *home crate and names* of the shared code differ.

The remainder of this section specifies: the two new crates and the extraction (§D1); the anti-duplication map of what is reused vs net-new (§D2); the reused server half + the two shared physics fixes (§D3); the ground-truth oracle (§D4); the metric set with formulas and L-layer mapping (§D5); the scenario catalog (§D6); the determinism fence (§D7); and where each gate sits in the 0015 taxonomy (§D8).

### §D1 The two new crates and the extraction

The design adds **one new lower crate** — `screeps-sim-core` (the shared mechanism; wasm + host, a submodule like the others) — and **one new host-only crate** — `screeps-rover-eval` (the measurement layer). There is **no `RoverWorld` and no re-ported physics**: the substrate is the combat sim's, factored out.

#### §D1a — `screeps-sim-core`: the pure movement kernel — **AS BUILT (M0 landed 2026-07-01)**

> This section is reconciled to the shipped code. Earlier drafts explored other shapes (`SimWorld` + a separate `CombatState`, a `resolve_combat_tick` rename, no `CombatWorld` type); the as-built decisions below supersede them, and the [Alternatives](#alternatives-considered) records why.

`screeps-combat-engine` bundled two separable concerns: (1) **general Screeps movement mechanics** — terrain, body-sizing arithmetic, the same-tile contention resolver, fatigue, edge-exit, the per-tick movement world — and (2) **combat** — damage, towers, ramparts, controllers, safe-mode, body-combat powers. M0 lifted (1) into `screeps-sim-core`; combat-engine keeps (2) and now depends on the kernel. The kernel carries **no combat concept**.

**What the kernel holds (as built):**

| kernel module | contents |
|---|---|
| `world.rs` | `MovementState { tick, terrain, rooms, creeps, npc_owners }` (the movement world) + `SimCreep` + `CreepId` / `PlayerId` / `StructureId` |
| `terrain.rs` | `SimTerrain { walls, swamps }` + `fatigue_rate` / `is_wall` |
| `body.rs` | `SimBody` — **movement / body-sizing arithmetic only** (`move_rate`, `fatigue_weight`, `can_move`, `fatigue_clear`, `part_hits`, `effective_power`, ctors); no combat powers |
| `movement.rs` | the same-tile contention resolver — `resolve_moves` / `resolve_moves_with_pulls`, `step`, `is_edge` (moved verbatim from the engine port) |
| `tick.rs` | `resolve_movement(&mut MovementState, &MoveIntents) -> MovementReport` — the movement tick: contention → apply move/fatigue → edge-exit → `tick += 1` |
| `intents.rs` | `MoveIntents { moves, pulls, reasons }` — the base action vocabulary |
| `sim.rs` | the `Simulation` layering trait + `MovementSim` (the movement-only layer) |
| `constants.rs` | `BODYPART_HITS` + `FATIGUE_*` (movement / body-sizing only) |

**The combat layer (`screeps-combat-engine`), as built** — a thin layer on the kernel, **byte-identical** (55 tests preserved: kernel 19 + engine 36):

- **`CombatWorld` = composition, not a rename.** `CombatWorld { movement: MovementState, towers, structures, controllers, safe_mode_owner }`. The name stays (it *is* a combat-layer type); its movement fields now live in the embedded `movement`. **No `Deref`** — combat reads `world.movement.creeps` explicitly and the mover takes `&mut MovementState` (combat passes `&mut world.movement`). Chosen over the two rejected alternatives: (a) separate `SimWorld` + `CombatState` params threaded through every helper (~187 edits, ~89 combat-field re-routes, no clarity gain), and (b) `Deref` composition (the operator ruled out the implicit IS-A coupling as shim-like).
- **`resolve_tick` keeps its name**; only its *movement phases* were extracted. It now reads: accumulate damage/heal from tick-start positions (Phase A/B) → `sim_core::resolve_movement(&mut world.movement, &moves)` (which owns contention + move/fatigue + edge-exit + `tick += 1`) → net damage → deaths → structures/towers/controllers. **Byte-identical** because per-creep *position* (moved by the kernel) and *hits* (netted here) are independent, edge-exit is occupancy-blind (relocating a soon-dead creep before it is retained changes nothing observable), and `report.tick` is captured before the mover increments it.
- **Body-combat lives in the layer, not the kernel.** The kernel's `SimBody` has only movement / body-sizing methods; the body-combat arithmetic (`attack_power`, `ranged_attack_power`, `heal_power`, `ranged_heal_power`, `dismantle_power`, `damage_after_tough`) + its constants (`ATTACK_POWER` … `CREEP_LIFE_TIME`) live in combat-engine as the **`SimBodyCombat` extension trait** (`impl SimBodyCombat for SimBody`), brought into scope where combat calls them. This applies the boundary rule literally: *the mover never needs body-combat, so it isn't in the mover's crate.*
- **Combat's `Intents` keeps its `moves` / `pulls` maps** and `resolve_tick` builds a `MoveIntents` from them to call the mover. (Having `Intents` *embed* a `MoveIntents` is a small clean-up follow-up; the type-level guarantee "a movement sim can't emit a combat action" already holds via `MovementSim::Intents = MoveIntents`.)

**The `Simulation` trait — per-layer action-typing (the operator's requirement).** A sim layer is a `(World, Intents, Report)` triple with a tick resolver:
```rust
pub trait Simulation { type World; type Intents; type Report;
    fn step(world: &mut Self::World, intents: &Self::Intents) -> Self::Report; }
```
`MovementSim` implements it with `Intents = MoveIntents`, so a decision routine driving the movement sim **cannot express a combat action at the type level**; a combat / economy layer's `Intents` embeds `MoveIntents` plus its own verbs. A new layer adds its own `(World containing MovementState, Intents embedding MoveIntents, Report)` with **zero** kernel changes — the generalization test the layering had to pass. (combat-engine currently exposes the concrete `resolve_tick` rather than an `impl Simulation for CombatSim`; adding that impl is a small follow-up.)

**Naming — as built:**

| concern | kernel (`screeps-sim-core`) | combat layer (`screeps-combat-engine`) |
|---|---|---|
| movement world | `MovementState` (ex-`CombatWorld` movement fields) | `CombatWorld { movement: MovementState, … }` (composition) |
| terrain | `SimTerrain` (ex-`CombatTerrain`) | — |
| creep / body | `SimCreep`, `SimBody` (movement methods) | `SimBodyCombat` extension trait (combat methods) |
| move intents | `MoveIntents` | `Intents` (combat; converts to `MoveIntents` for the mover) |
| tick | `resolve_movement` | `resolve_tick` (calls the mover) |
| layering | `Simulation` trait + `MovementSim` | `CombatSim` impl = follow-up |
| combat entities | — | `SimTower` / `SimStructure` / `SimController` / `StructureKind` (unchanged) |

Only genuinely combat-agnostic types were renamed *into* the kernel (`CombatTerrain`→`SimTerrain`; the `CombatWorld` movement fields → `MovementState`). **Combat-layer types kept their names** (`CombatWorld`, `resolve_tick`, `Intents`) — the kernel is clean, so there is no reason to churn them (or the bot). `screeps-combat-engine` keeps its name (it *is* the combat tick engine, now layered on the kernel); `screeps-sim-core` reads as the foundational layer both it and `rover-eval` build on; `screeps-rover-eval` follows the `-eval` convention. No back-compat aliases or re-export shims are kept.

**Deferred out of M0 (later slices, noted so the ADR isn't read as "all done"):**
- the offline **rover driver** (`resolve_moves_via_system` + `SimMoveRequest` / `SimMoveGoal` / `SimMoveCache`, today `screeps-combat-agent/src/pathing.rs:240-518`) → `screeps-sim-core` under an optional **`rover` feature** (the crate already declares it), so both the combat squads and the bench import one driver — **M1**;
- a kernel movement **recording**: combat-engine's `CombatRecording` is combat-coupled (records damage / deaths / tower intents), so it stayed in the layer; the kernel gets a lightweight movement-trajectory recording when rover-eval needs it;
- the deterministic **`Rng`** (rover-eval reuses combat-eval's for now);
- the **roads** + **loaded-CARRY** physics fixes — **M3** (§D3).

**No debt (M0 end state):** git submodule + git-dep + root `[patch]` (no path deps); explicit composition (no `Deref`); direct cross-crate imports (no re-export modules / no constant bridge); no combat concept in the kernel.

**Crate graph (post-extraction; `X ─▶ Y` = X depends on Y):**

```
screeps-sim-core   ── MovementState · SimTerrain · SimBody(movement) · contention resolver · resolve_movement · Simulation/MovementSim
   └─▶ screeps-rover            (feature "rover" — the offline driver module; M1, not yet moved down)

screeps-combat-engine  ─▶ screeps-sim-core      = kernel + the combat layer (CombatWorld{movement,…}, damage/towers/ramparts, SimBodyCombat, resolve_tick)
   └─▶ screeps-combat-agent ─▶ screeps-combat-decision (─▶ screeps-sim-core, for the moved body-sizing constants)
        └─▶ screeps-combat-eval                 (host-only harness)

screeps-rover-eval   (host-only, NET-NEW measurement layer — M2+, not yet built)
   ├─▶ screeps-sim-core[rover]   ── drive MovementSim over MovementState (+ the offline driver once M1 lands)
   ├─▶ screeps-combat-eval       ── reuse the host-only harness (Generator/Validator/evaluate/terrain_import/visualize)
   └─▶ screeps-rover             ── direct: the MovementSystem/LocalPathfinder UNDER TEST (custom op budgets + tick_stats)
```

`screeps-sim-core` is a normal **wasm + host** member (no `--exclude`) — it carries no host-only deps and is bot-adjacent mechanism code, exactly like `screeps-combat-engine`. The `rover` feature only adds `screeps-rover` (itself wasm-safe), so the feature-unified build stays wasm-clean.

#### §D1b — `screeps-rover-eval` (the host-only measurement layer)

`screeps-rover-eval` is a host-only workspace **member** — a **library** (`src/lib.rs`) with an `examples/run_register.rs` and an optional `[[bin]]` CLI, the `screeps-combat-eval` shape (register of gated experiments + CPU bench + tuning sweep in one crate) with the corpus-CLI ergonomics of `screeps-foreman-bench`. It **reuses** the world / physics / driver from `screeps-sim-core` and the host-only harness scaffolding from `screeps-combat-eval`; it writes only the net-new measurement layer (§D2).

```toml
# screeps-rover-eval/Cargo.toml
[package]
name = "screeps-rover-eval"
version = "0.1.0"
authors = ["William Archbell <william@archbell.com>"]
edition = "2021"
description = "Offline simulator + benchmark for screeps-rover: drive the REAL MovementSystem/\
LocalPathfinder/resolver over a screeps-sim-core world, measure pathing quality (ticks-moving, \
fatigue efficiency, path optimality), congestion behavior, and algorithmic/ops CPU — gated standing \
regression tests with a first-class determinism fence. Reuses the combat sim's mover (the ADR 0023 \
'validated separately' gap)."

[dependencies]
screeps-game-api = "0.23"
# The shared mechanism: world + server-half movement tick + recording + RNG + the offline rover driver
# (`rover` feature). Root [patch] redirects the git URL to the local member path.
screeps-sim-core = { git = "https://github.com/Azaril/screeps-sim-core", features = ["rover"] }
# Reuse the host-only harness scaffolding (Generator/Validator/run_suite, evaluate/_recorded,
# terrain_import, visualize, param_sweep helpers) — NOT re-derived. combat-eval -> combat-engine ->
# sim-core, so the world types unify; combat-eval never reaches wasm (already wasm-excluded).
screeps-combat-eval = { git = "https://github.com/Azaril/screeps-combat-eval" }
# The MovementSystem/LocalPathfinder UNDER TEST — a DIRECT dep (not only transitive via the driver):
# the CPU bench builds its own MovementSystem to set op budgets + read tick_stats(), which the
# production `resolve_moves_via_system` hard-codes away.
screeps-rover = { git = "https://github.com/Azaril/screeps-rover" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rayon = "1"                                          # parallel corpus sweep / CPU bench (host-only)
clap = { version = "4", features = ["derive"] }      # the corpus CLI (foreman-bench ergonomics)
```

No `[patch]`, no `[profile]` in either new crate — both inherit the root `Cargo.toml` (its `[patch]` table redirects every `Azaril/*` git URL to the local member path, so editing sim-core/rover and rebuilding the eval needs no push — the load-bearing in-workspace mechanism, identical to the other crates).

**Root edits to wire both crates in:**

1. `Cargo.toml` `members`: add `"screeps-sim-core"` (beside `screeps-combat-engine`) and `"screeps-rover-eval"` (in the host-side tool block, beside `screeps-foreman-bench` / `screeps-ibex-eval`).
2. `Cargo.toml` `[patch]`: add `[patch.'https://github.com/Azaril/screeps-sim-core'] screeps-sim-core = { path = "screeps-sim-core" }` and the equivalent for `screeps-rover-eval` (every git-dep'd crate needs a patch entry).
3. `.cargo/config.toml`: add `--exclude screeps-rover-eval` to **all three** wasm aliases (`clippy-wasm` / `check-wasm` / `build-wasm`) — host-only (rayon / clap). **Do NOT exclude `screeps-sim-core`** — it is a wasm + host mechanism crate (like `screeps-combat-engine`); the wasm build compiles it.
4. `screeps-combat-engine/Cargo.toml` gains a `screeps-sim-core` dep; `screeps-combat-agent/Cargo.toml` gains `screeps-sim-core` with the `rover` feature (the driver moved down). These are part of the §M0 extraction.

**Module layout** — `screeps-rover-eval` holds **only the net-new measurement layer**; everything else is imported from `screeps-sim-core` (world / tick / driver / recording / RNG) and `screeps-combat-eval` (the `harness/` traits + runners + `terrain_import` + `visualize` + `param_sweep` helpers):

```
screeps-rover-eval/
  Cargo.toml
  README.md                         # overview, usage, register table, module reference (combat-eval README shape)
  examples/run_register.rs          # `cargo run --example run_register -p screeps-rover-eval` -> the gated report
  src/
    lib.rs                          # the EXP-* register of gated pathing experiments + report()
    metrics.rs                      # PathMetrics/MoveMetrics over a sim-core recording (§D5); lifts `oscillation_rate`
                                    #   + `cohesion` from combat-eval metrics.rs (position-only, mis-filed there)
    oracle.rs                       # NET-NEW ground-truth optima (§D4): BFS / weighted Dijkstra / time-expanded SP
    scenarios.rs                    # NET-NEW `Generator` impls (§D6) — corridors, fan-ins, chokes, swamp/road fields
    validate.rs                     # NET-NEW `Validator` lenses (RouteOptimalityWins, NoOscillation, NoDeadlock, …)
    bench.rs                        # NET-NEW CPU bench (reuses combat-eval bench.rs SHAPE: compound worst case +
                                    #   Instant timing + loose death-spiral gate); op-counts via rover tick_stats()
    sweep.rs                        # NET-NEW rover param scorer over the reused combat-eval rayon sweep helpers
    cli.rs                          # NET-NEW host CLI (clap) over the reused corpus + report table
```

`screeps-combat-eval` is the **harness template**; `screeps-foreman-bench` the **corpus-CLI ergonomics**; the **membership / patch / wasm-exclude plumbing** is identical across the host crates.

### §D2 The anti-duplication map (reuse vs net-new)

Every simulation component the bench needs maps to exactly one disposition: **REUSE-AS-IS** (import it, no change), **REUSE-AFTER-MOVE** (relocated by the §D1a extraction, then imported), **SHARED-FIX** (a fidelity fix landing in `screeps-sim-core`, benefiting both sims), or **NET-NEW** (genuinely rover-specific — the bench writes it). Critically, the five rover abstraction traits (`CreepHandle` / `CostMatrixDataSource` / `MovementSystemExternal` / `PathfindingProvider` / `MovementVisualizer`) are **already implemented** for an offline world by the driver that moves into sim-core (renamed `SimCreepHandle` / `SimMovementExternal` / `SimWorldCostSource`, ex-`Combat*` at `pathing.rs:316-471`) — the bench does **not** re-implement them; it calls `resolve_moves_via_system`.

| Component | Disposition | Source / change |
|---|---|---|
| World substrate | REUSE-AFTER-MOVE | `SimWorld` / `SimTerrain` / `SimCreep` / `SimBody` (`sim-core`, ex-`engine state.rs/body.rs`). `SimWorld{ terrain, creeps, ..Default::default() }`. |
| Server-half physics (fatigue / contention / edge) | REUSE-AFTER-MOVE | `sim_core::resolve_movement` (the movement phases of ex-`resolve.rs:203`). **No re-port** — §D3. |
| Contention resolver (standalone) | REUSE-AFTER-MOVE | `resolve_moves` / `resolve_moves_with_pulls` (`sim-core movement.rs`) — read-only, isolated-contention tests. |
| Move-intent DTO | REUSE-AFTER-MOVE | `MoveIntents { moves, pulls, reasons }` (`sim-core`). `reasons` = a free per-creep introspection channel. |
| **Offline rover driver** (the whole `MovementSystem`+resolver run) | REUSE-AFTER-MOVE | `resolve_moves_via_system(world, owner, &[SimMoveRequest], &mut SimMoveCache)` (`sim-core` `rover` feature, ex-`agent/pathing.rs:477`). **The single biggest anti-duplication win.** |
| Per-creep request / goal / cache | REUSE-AFTER-MOVE | `SimMoveRequest` / `SimMoveGoal::{To,Flee}` / `SimMoveCache = HashMap<CreepId, CreepMovementData>` (`sim-core` `rover`). One cache held across ticks → path reuse + stuck escalation accumulate. |
| Cost-matrix source (for the CPU bench) | REUSE-AFTER-MOVE (made `pub`) | `SimWorldCostSource::from_world` (ex-`CombatWorldCostSource`) made `pub` in the move, so the bench can build its own `MovementSystem` with `set_pathfinding_ops_budget` + `tick_stats()`. |
| Terrain import (real-room corpus) | REUSE-AS-IS | `terrain_import::{decode_terrain, TerrainFixture}` (`combat-eval`, already `pub`, rover-agnostic). Walls + swamps; roads via SHARED-FIX-1. |
| Deterministic RNG | REUSE-AFTER-MOVE | the SplitMix64 `Rng` → `sim-core rng.rs` (was `pub(crate)` in combat-eval `generate.rs:54`; **no `rand`/`Date`**). |
| Scenario builder | REUSE-AS-IS | `ScenarioBuilder` (`agent/scenario.rs`) for terrain / creep / obstacle placement; extended to place roads (SHARED-FIX-1). |
| Generator / Validator / suite seams | REUSE-AS-IS (traits) + NET-NEW (impls) | `Generator` / `Validator` / `run_suite` (`combat-eval harness/`) reused; the rover *impls* are net-new. |
| Tick runner | REUSE-AS-IS | `evaluate` / `evaluate_recorded` (`combat-eval harness/evaluate.rs:105`): supply the rover system as the step closure. |
| Recording type | REUSE-AS-IS | `SimRecording` / `record_tick` (`sim-core record.rs`, ex-`CombatRecording`) already captures `(room,x,y)` + fatigue + `Direction` + reason — a rover trajectory+fatigue capture. (Ops come from the CPU-bench counter, not the recording.) |
| Replay rendering | REUSE-AS-IS | `visualize::{replay_to_html, write_replay}` (`combat-eval`) → a rover replay for free. |
| CPU-bench / param-sweep patterns | REUSE-AS-IS (shape) + NET-NEW (scorer) | the `bench.rs` compound-worst-case + `Instant` shape and the rayon `env_*_list` sweep helpers reused; the rover scorer is net-new. |
| Determinism fence | NET-NEW (pattern reused) | re-implement `sim_is_deterministic_over_rounds` summing a **rover** aggregate (Σ tiles-moved / Σ fatigue / Σ arrival-tick), spread 0, + `f64::to_bits` run-twice per float metric. Optionally a `sim_core::assert_spread_zero_over_rounds` helper both benches call. |
| **Roads physics** | **SHARED-FIX-1** | `sim-core terrain.rs` + `fatigue_rate` (§D3). Benefits both sims. |
| **Loaded-CARRY physics** | **SHARED-FIX-2** | `sim-core body.rs` `fatigue_weight` + `SimCreep` (§D3). Benefits both sims. |
| Rover-quality **metrics** | NET-NEW | `R_fatigue` / `R_ticks` / `movement_eff` / `fatigue_util` / congestion / ops over the recording (§D5). Combat's `SideMetrics` measures the wrong thing — but `oscillation_rate` + `cohesion` are lifted verbatim (position-only). |
| Ground-truth **oracle** | NET-NEW | ideal fatigue-weighted-cost baseline (§D4), via the project pathfinding primitives (`gridsearch`), **not** a one-off algorithm. |
| Rover **scenario catalog** | NET-NEW | `Generator` impls (§D6) — combat generators place towers/forces, the wrong shape. |
| Rover **CLI / report** | NET-NEW | host binary; the table shape is the combat-eval template, the columns are rover metrics. |

**Reuse `CreepId`; do not generalize the `Handle`.** The driver is nailed to `CreepId = u32`; rover's `MovementSystem` is already generic over `Handle: Hash+Eq+Copy+Ord`, but the bench owns the world and keys movers by `u32` happily. Generalizing touches three impls + a public signature for zero benefit. Three pure rover entry points the bench also drives directly (no trait): `AnchorPath::advance` (`anchor.rs:80-167`), `gridsearch::{room_grid_dijkstra, room_grid_dijkstra_to_edge, reaches_room_edge}` (the §D4 oracle primitives), `moving_maximum` (a prime CPU-bench target).

**The two tiers** (same cost/coverage split, expressed over the reused pieces):

- **Tier A — pure navigation bench (no server half).** Call `LocalPathfinder` / `gridsearch` / `moving_maximum` / `AnchorPath` directly over sim-core cost matrices; measure ops, length, cost-vs-optimum, incomplete-rate, determinism. Cheap, fast — the bulk of the corpus sweep.
- **Tier B — engine-resolved bench (the full tick loop).** Drive the whole `MovementSystem` via `resolve_moves_via_system`, apply the issued directions via `sim_core::resolve_movement`, step the loop; measure ticks-to-arrive, fatigue burned, contention losses, repath churn, deadlock/livelock, budget-binds. **This is where "time creeps spend moving / fatigue efficiency" actually lives.** Runner: the reused `evaluate_recorded`.

### §D3 The reused server half + the two shared physics fixes

The "server half" — the authoritative application of moves with fatigue, contention, and edge-exit — is **not new code**. It is `sim_core::resolve_movement` (the movement phases of the combat engine's `resolve_tick`, extracted in §D1a). The bench feeds it `MoveIntents` carrying **only** `moves` / `pulls`; with no combat actions the combat phases iterate empty maps and no-op (verified against the engine: combat-accumulate + tower loops read empty collections → zero effect). The per-tick loop (Tier B):

1. Build `[SimMoveRequest]` from each creep's goal — `SimMoveGoal::{To{target,range}, Flee{threats,range}}` with a `MovementPriority`, a `shove` toggle, and an optional `(center,range)` anchor (the real driver request type; `move_to` / `with_priority` / `with_shove` / `with_anchor` builders).
2. `resolve_moves_via_system(&world, owner, &reqs, &mut cache)` → per-creep `Direction`s. **This runs the real rover `MovementSystem` + `LocalPathfinder` + resolver — the thing under test** — and the persisted `cache` carries path reuse + stuck escalation across ticks.
3. `sim_core::resolve_movement(&mut world, &MoveIntents::from(dirs))` **applies** them with engine fidelity (REUSED, **not re-ported**):
   - **Tile contention** by the engine order — `rate1` (movers into the vacated tile; forced 100 for a swap), then pulled, then pulling, then moves/weight; heavier loses ties. Stationary creeps are **hard obstacles** (no engine pushing) — distinct from rover's *own* friendly-creep shove/swap, which already ran in step 2; the two resolvers are deliberately different and the bench measures the engine's adjudication of what rover requested. A vacating creep lets a follower in same-tick (trains); a failed leader cascades failure down the column.
   - **Fatigue accrual** on an executed step = `(otherParts + loadedCarry) × terrain_rate`, `otherParts` = #non-MOVE non-CARRY parts, `terrain_rate` = `1` road / `2` plain / `10` swamp (roads via **SHARED-FIX-1**, loaded-CARRY via **SHARED-FIX-2**, below).
   - **Fatigue regen** every tick = `2 × Σ(MOVE_boost_mult)`, `saturating_sub`, regardless of moving.
   - **Edge zeroing** — stepping onto a room-edge tile sets accrued fatigue to 0.
   - **Pull bypass** — a pulled creep moves even with nonzero fatigue / no MOVE. (The driver's `pull` / `move_pulled_by` are **no-ops today**, ex-`pathing.rs:337-342`, so the E-family pull-train scenarios require activating pull in the shared driver — a SHARED follow-up noted in §D6/E that benefits combat too.)
   - **Power creeps** — no fatigue, 1 tile/tick on any terrain.
   - **Edge relocation** — a creep that steps onto a room-edge tile crosses **same-tick** to the neighbour room: the **perpendicular** coordinate flips (`0↔49`), the **parallel** (along-edge) coordinate is **preserved** (engine `creeps/tick.js:52-78`; a creep at `(0,25)` exiting west becomes `(49,25)`, *not* `(49,24)` — NOT an `x→49−x` reflection). This is the **one** behavior a single-room bench inherits unconditionally: keep movers off `x/y ∈ {0,49}`, or register the owner in `world.npc_owners` (already `pub`) to suppress it. NPC creeps are exempt.
4. Record the frame via the reused `sim_core::record_tick` → `SimRecording` (positions + fatigue + issued `Direction` + reason); attach the tick's resolver-event counts + `MovementSystem::tick_stats()` ops for metrics + replay.
5. Advance tick.

> **The two shared physics fixes (both land in `screeps-sim-core`; both default-inert; both improve combat fidelity too).** The engine port omits two fatigue terms that are harmless to combat but fatal to a hauler benchmark; post-extraction they live in sim-core's `terrain.rs` / `body.rs` and benefit **both** sims.
>
> **SHARED-FIX-1 — Roads.** Today `fatigue_rate` (ex-`state.rs:22`) returns only swamp-or-plain, and `FATIGUE_RATE_ROAD` (`constants.rs:44`) is a **dead constant** (defined, never referenced — grep-confirmed). Add `roads: HashSet<(u8,u8)>` to `SimTerrain` and check it **first** in `fatigue_rate` (road `1` overrides swamp). `resolve_movement` already routes fatigue through `fatigue_rate` → **no tick change**; extend `ScenarioBuilder` + the cost sources to place roads and wire rover's already-present-but-empty `roads` `LinearCostMatrix` so the pathfinder *prefers* them. Roads are the single biggest base-ops fatigue lever; without this every fatigue/time-moving metric over a built base is fiction.
> **AS BUILT (M3 — 2026-07-01):** `roads: HashSet<(u8,u8)>` on `SimTerrain`; `fatigue_rate` = road 1 → swamp 10 → plain 2 (engine `movement.js:204-213` order, road-wins-over-swamp). `FATIGUE_RATE_ROAD` is now live. `is_wall` documents the one deliberate divergence (engine `checkObstacleAtXY` treats `wall && !road` as passable, but a road can't be built on a natural wall, so it's unreachable — walls stay unconditionally blocking). rover-eval's `cost.rs::fatigue_rate` **delegates to the kernel `SimTerrain::fatigue_rate`** (removed the duplicated swamp/plain branch), and its `TerrainCostSource` populates rover's `roads` sub-matrix (priced at `CostMatrixOptions::road_cost` = 1) *and excludes road tiles from the swamp overlay* so the apply-order (roads then `other.apply_to`) leaves road-on-swamp at 1. Tests: `terrain::road_overrides_swamp_and_plain`, `tick::road_step_accrues_less_fatigue_than_plain`, rover-eval `metrics::road_corridor_across_a_swamp_is_taken`.
>
> **SHARED-FIX-2 — Loaded CARRY.** `fatigue_weight` (ex-`body.rs:155`) counts only non-MOVE/non-CARRY alive parts (`body.rs:158`) — CARRY is **always free**, with no notion of carried resource. Engine truth: an *empty* CARRY is free, a *loaded* one counts. Add `loaded_carry_parts: u32` to `SimCreep` (default 0) and have `fatigue_weight` also count `min(loaded_carry_parts, alive CARRY parts)`. Because `fatigue_weight().max(1)` is also the `rate4` contention denominator (`movement.rs:156/174`), this *also* corrects loaded-hauler contention — a benefit. Loaded CARRY is the *dominant* fatigue term for haulers, the primary subject of this benchmark.
> **AS BUILT (M3 — 2026-07-01):** chose `carry_used: u32` (total resources aboard = engine `_.sum(creep.store)`) on `SimCreep`, *not* a pre-counted `loaded_carry_parts` — it is strictly more faithful (models the store, and the boost-aware part accounting is derived, not assumed). `SimBody::carry_weight(carry_used)` ports the engine's `calcResourcesWeight` (`movement.js:41`): walk the body back-to-front, each ALIVE CARRY part absorbs `CARRY_CAPACITY × BoostTier::carry_capacity_mult` (×1/2/3/4) of the load and adds 1 weight until the load is exhausted (naturally capped at the alive-CARRY count). The full weight lives on **`SimCreep::fatigue_weight()` = `body.fatigue_weight()` (structural) + `body.carry_weight(carry_used)`**, and both the mover's rate4 denominator (`movement.rs`) and the tick's accrual (`tick.rs`) now call it — so loaded-hauler contention is corrected in the same stroke. Tests: `body::carry_weight_counts_loaded_parts_only` / `carry_weight_is_boost_aware`, `tick::loaded_carry_accrues_fatigue_an_empty_hauler_does_not`.
>
> **Default-inert ⇒ combat byte-identical.** Empty `roads` + `carry_used = 0` make both fixes no-ops for every existing combat scenario (combat creeps carry nothing; combat rooms have no `roads` set), so the combat crates take only a mechanical `carry_used: 0` on their `SimCreep` literals with unchanged behavior — the M3 gate is the combat test suites re-passing at their M0 counts (proves inert).

**Models vs Omits** (the 0006:104 contrast, made explicit so reviewers don't expect what isn't there):

| **Models** | **Omits** |
|---|---|
| Fatigue accrual/regen, terrain-rate (1/2/10) incl. roads, sustained-speed | Combat damage / heal / towers (ADR 0023 owns) |
| Same-tile contention (engine `rate1`/pulled/pulling/weight), swaps, trains/pulls | Economy / resources / the full game tick (ADR 0006 owns) |
| Edge-zeroing + multi-room edge relocation (reuse 0023's corrected edge-exit model, `0023:13-18`) | Spawning logistics, lifecycle (ADR 0028 owns) |
| Stationary-creep-as-hard-obstacle, power-creep no-fatigue | Boost economy (boosts modeled as a body multiplier only) |
| Ops accounting (the `MovementSystem` ops budget) | Real CPU clock (op-count is the deterministic proxy; wall-clock is secondary) |

### §D4 The ground-truth oracle (`oracle.rs`)

Single-creep optima are **exactly computable and already half-shipped** (`gridsearch.rs`); multi-creep optima are **NP-hard** and fall back to relative/regression baselines. The ADR states this asymmetry plainly so no reviewer expects a multi-agent optimum that does not exist.

| Oracle | Method | Cost | Feeds |
|---|---|---|---|
| **Obstacle-aware min step count** | Unweighted 8-connected Chebyshev **BFS** to within `range` of goal | O(2500/room) | `R_len` (§D5.2); the **solvable/unsolvable label** (unsolvable ⇔ goal unreached) |
| **Terrain-weighted min cost** | **Dijkstra** with edge cost `w(dest)` using the *scenario's configured* `plains_cost`/`swamp_cost` (apples-to-apples with what rover's `search` was handed) — this is exactly `gridsearch::room_grid_dijkstra` already in the crate | O(2500 log) | `cost_fatigue(O*)` for `R_fatigue` (§D5.1a); `min_ops` lower bound for `ops_efficiency` |
| **Min arrival-time for a body** | **Time-expanded shortest path**: state `(tile, fatigue_bucket)`, edges *move* or *wait* at 1 tick each, edge-tile reset baked in; Dijkstra/Dial over `tiles × (max_fatigue+1)`. **Sub-tick ordering pinned to the engine** (`movement.js:11-14` / `creeps/tick.js:105-107`): a move at tick *T* requires `fatigue==0` as evaluated at *T*'s **start**; the destination tile's accrual lands **this** tick; the `−2·Σ(MOVE_boost)` drain applies once per tick **thereafter** — a stall is `fatigue>0 at tick start`, **never** "fatigue would reach 0 after applying this tick's drain" (the latter makes every `R_ticks` optimistic by one tick per stall). | tractable single-creep, 1–2 rooms | `T*` for `R_ticks` (§D5.1b) and `T_min` (§D5.fatigue) |
| **Closed-form min traversal ticks** | Per-tile stall = `ceil(accrual / drain)` clamped ≥1, summed; valid when the body sustains or stalls predictably with no path choices | O(len) | fast-path `T_min`; **cross-checked equal to the time-expanded oracle** on B-family scenarios (guards against a wrong oracle) |
| **Multi-room optimum** | `find_route` for the room sequence, then chain per-room `room_grid_dijkstra` through projected exit tiles | bounded | C-family `R_fatigue` (approximate); gate on single-room exactness + multi-room *monotonic improvement* |
| **Multi-creep makespan** | **NP-hard — no exact optimum claimed.** Lower bound = `max over creeps of single-creep T*` (loose). For N≤3 on small rooms, a CBS/exhaustive reference gives a true optimal makespan to anchor `flow_efficiency`. **Primary contract = committed regression baseline** (foreman-bench model): gate "no regression beyond X% vs the committed baseline"; absolute improvement is the optimization target the ADR exists to enable. | — | D-family quality indicators |
| **Determinism** | No computed optimum — the oracle is "the result equals itself across runs/reorders/seeds." | exact | §D7 fence |

**Oracle ↔ executor cross-validation (guards a shared off-by-one):** the closed-form and time-expanded oracles can agree with each other while *both* embed a wrong sub-tick convention. So M2 additionally asserts the oracle's per-tile stall equals the **server half's** observed stall on a single straight corridor — validating the oracle against the *engine ordering* (via the executor), not merely against the other oracle.

### §D5 The metric set (formulas + L-layer mapping)

Two foundational definitions: a tile's **engine fatigue-cost** `w(t)` = `road 1 / plain 2 / swamp 10`; a body's per-step accrual `= (otherParts + loadedCarry) × w(t)` with per-tick drain `2 × Σ(MOVE_boost_mult)`, sustaining 1 tile/tick iff `2·Σ(MOVE_boost) ≥ (otherParts + loadedCarry)·w(t)`.

**Provenance rule:** wherever possible the metric reads rover's *own* surfaced state (`MovementTickStats { ops_budget_cap, ops_consumed, repaths }`, `MovementResult`, `creep.fatigue()`, `StuckState { ticks_immobile, ticks_no_progress, repath_count, last_distance }`) rather than re-deriving — the eval measures the *real* system and owns only the rover metric layer + the oracle (the world and physics are the reused `sim-core`).

#### §D5.1 Pathing quality (computed on the returned path, before stepping)

- **(a) Fatigue-weighted optimality** `R_fatigue = cost_fatigue(P) / cost_fatigue(O*)`, `cost_fatigue(P) = Σ_{t∈P, t≠s} w(t)`. ≥1 always. **Gate (single-creep, A-family): `R_fatigue ≤ 1.0 + ε`, ε≈0.02.** [L2 property / L0 on fixed inputs]
- **(b) Tick-to-arrival optimality** `R_ticks = ticks(P, body) / T*(s,g,body)` where `T*` is the time-expanded arrival-time optimum. **The metric that actually matters in-game.** Divergence between (a) and (b) is itself a finding: the cost field is mis-tuned for that body (the single most actionable optimization the bench can surface — rover's cost field is **not body-aware** today; a heavy hauler and a 1:1 scout search the same `road=1/plain=2/swamp=10` field). **Gate: `R_ticks ≤ 1.0 + ε`, regression-tracked per body class.** [L2/L5]
- **(c) Length suboptimality** `R_len = len(P)/len(O*_chebyshev)`; `detour_tiles = len(P) − len(O*_chebyshev)`. Diagnostic except in uniform terrain. **Gate (uniform-terrain only): `R_len == 1.0`.** [L2]
- **(d) Oscillation/backtrack** `backtrack_steps` / `oscillation` (direction reversals) / `revisit_count`. Targets the operator-observed scatter. **Gate: `oscillation == 0` on static single-creep; `revisit_count` regression-tracked under congestion.** [L2/L5]
- **(e) Incomplete/fail rate** `solvable_fail = fails on oracle-proven-solvable scenarios`. **Gate: `solvable_fail == 0` (failing a genuinely-walled goal is correct).** [L2]
- **(f) Ops vs budget** `ops_consumed` (from `MovementTickStats`), `ops_per_pathfind`, `ops_efficiency = ops_consumed / min_ops`, `budget_pressure = #ticks at cap`. **Gate: single-creep ops ≤ base per-room budget (`rooms × 2000`, `movementsystem.rs:1310`) on a first, non-escalated search; ≤ `MAX_PATHFIND_OPS` (20 000) once stuck-escalation kicks in `ops_multiplier`; curve vs room size & route length.** [L4 invariant + L5 curve]
- **(g) Repath frequency** `repath_rate`, `expiry_repaths` (driven by `reuse_path_length=5`), `stuck_repaths`, `wasted_repaths` (repath returning an identical/worse path). **Gate: `wasted_repaths/repaths` regression-tracked; static-corridor `stuck_repaths == 0`.** [L5]

#### §D5.2 Movement execution / fatigue (require stepping the server half)

- **Movement efficiency** `movement_eff = ticks_displaced / ticks_trying`, with the wasted ticks partitioned into a disjoint set — **the single most important diagnostic**:
  - `idle_fatigued` = ticks `fatigue() > 0` (engine-forced; indicts the **body**, NOT rover) — **rover reports these as `Moving` at `:510`, so they are invisible in `MovementResult` and MUST be measured server-side by reading `creep.fatigue()`**;
  - `idle_blocked` = `fatigue==0` but position unchanged AND a contender held the tile (indicts the **resolver/pathfinder** — the true congestion-loss class the sim exists to surface);
  - `idle_no_path` = `Failed(PathNotFound)`/`Stuck`; `idle_spawning` = `spawning()` (excluded from `ticks_trying`).
  - **Gate: `idle_blocked / ticks_trying ≤ ε` in single-creep (should be 0).** [L4 invariant]
- **Fatigue utilization** `fatigue_util = T_min / actual_ticks`; `fatigue_waste = actual − T_min`; `fatigue_stalls`. Plus the route-independent **over/under-MOVE body diagnosis** `speed_ratio(terrain) = 2·Σ(MOVE_boost) / ((otherParts+loadedCarry)·w(terrain))` → `under_move ⇔ ratio < 1`, `over_move ⇔ min-over-traversed-terrains ratio > 1`. Lets the bench report "this hauler is under-MOVE on swamp (ratio 0.4) → 2.5× slowdown" and validate the pathfinder routes under-MOVE bodies *around* the terrain that punishes them (links D5.1b ↔ this). **Gate: `fatigue_util == 1.0` whenever the oracle says the body is fast enough for the route; `fatigue_stalls` accounted, never blamed on rover.** [L0 closed-form + L5]
- **Congestion** `shove_count`, `swap_count`, `stuck_events`, `train_follow_success`, `deadlock` (N≥2 creeps mutually position-stable, each desiring a tile held by another stable creep, ≥ `report_failure`=12 ticks), `livelock` (Σ|Δpos| over a 20-tick window < threshold while `shove+swap` over the same window > threshold), `throughput` (creeps/100t through a chokepoint), `makespan`, `flow_efficiency`. **Ship-blocker gates: `deadlock == 0` and `livelock == 0`; `train_follow_success ≥ 0.95`; `throughput`/`makespan` regression-tracked.** Targets the operator's "war/squad cohesion / lifecycle hang" failure class. [L4 hard invariants + L5 distributions]
- **Flee/anchor** `flee_min_range_achieved ≥ 0`, `ticks_to_safety`, `flee_into_corner` (bool), `anchor_violation` (ticks outside `AnchorConstraint.range`), `anchor_work_loss`. **Ship-blocker gates: `flee_min_range_achieved ≥ 0` whenever escape exists; `anchor_violation == 0` (hard invariant).** [L4]

#### §D5.3 CPU / algorithmic (op-count primary, wall-clock secondary)

`ops_per_search` (deterministic), `ops_per_resolve` (tile-walkability probes / shove-recursion depth), `ops_per_tick`; `ns_per_search`/`ns_per_resolve`/`ns_per_tick` (regression baseline only, **never a gate** — host wall-clock ≠ MMO CPU); `alloc_count` (a counting global allocator in the host bin — `LocalPathfinder::run` boxes two `[[_;50];50]` arrays ≈ 20 KB/call, a flagged pooling opportunity), `peak_path_len`.

**Scaling curves** (the deliverable that catches the "CPU pathfinding death-spiral" failure) — sweep one axis, fit & report the exponent, gate "slope ≤ baseline + X%":

- `ops_per_tick` vs **N creeps** (1,2,4,…,64) → expect ~linear; super-linear = congestion blowup;
- `ops_per_search` vs **route length** (10..600 tiles);
- `ops_per_search` vs **room obstacle %** (open .. maze) → A* degradation under walls (worst case = **open room**, which floods to the full `max_ops` with nothing to prune — the `bench.rs:84` worst-case construction);
- `resolve_work` vs **congestion density** → the `max_shove_depth` recursion is the prime super-linear suspect; instrument its realized depth distribution.

Benched the `bench.rs` way: **`Instant`-timed `#[test]` gates with LOOSE death-spiral bounds (not tight thresholds, not criterion), exact us/op printed** — carry forward the `bench.rs` disclaimer that native wall-clock is a *relative* proxy. (Criterion is used nowhere in this repo; the convention is hand-rolled `std::time::Instant` loops.)

### §D6 The scenario catalog (`scenarios.rs` — net-new `Generator` impls)

Each family is a parameterized `Generator` (the reused combat-eval trait, driven by the reused sim-core SplitMix64 `Rng` — **no `rand`/`Date`/`Math.random`**, the determinism requirement). Listed as *what it stresses → gate*; full table abbreviated here, enumerated in the crate.

- **A — Single-creep optimal path** (A1 open-plain, A2 all-road, A3 all-swamp, A4 mixed-terrain road-shortcut/swamp-avoidance, A5 maze single-solution, A6 spiral/comb adversarial, A7 unsolvable). Gates: `R_fatigue≈1`, `R_len==1` (uniform), `oscillation==0`, `solvable_fail==0`, A4 **`R_ticks==1`** (must dodge swamp for a heavy body), A7 `Failed(PathNotFound)` NOT a stuck-timeout.
- **B — Fatigue-bound bodies** (B1 under-MOVE on plain, B2 loaded-vs-empty CARRY, B3 swamp-punished under-MOVE with a plains detour available, B4 edge-reset, B5 power-creep). Gates: `fatigue_util==1` (body-bound, rover blameless), `idle_blocked==0`, **B2 accrual delta == `loadedCarry × w(t)`** (the loaded-CARRY term the combat-engine port drops — §D3), B3 `R_ticks==1` (chooses the detour), B4 post-edge `fatigue==0`, B5 `fatigue_stalls==0` on all terrain.
- **C — Cross-room routing** (C1 adjacent, C2 4–8 room chain, C3 room-cost detour via `get_room_cost`, C4 over-cap distance, C5 border re-entry — the `aaac0f7` "move-to-room border thrash" class). Gates: arrives, `R_fatigue≤1+ε`, bounded ops, **no edge oscillation**, C4 defined failure (no hang).
- **D — Multi-creep congestion** (D1 head-on 1-wide, D2 head-on with pocket, D3 chokepoint funnel, D4 perpendicular intersection, D5 priority preemption High-vs-Low, D6 16–64 dense field, D7 `Immovable` obstacle mid-corridor). Gates: `deadlock==0`, `livelock==0`, `flow_efficiency`/`throughput`/`makespan` ≥ baseline, D5 High advances & Low shoved (not failed), D7 `Immovable` never moves.
- **E — Follow/pull trains** (E1 simple follow, E2 long train + leader-fail cascade, E3 pull a fatigued/0-MOVE follower, E4 quad `desired_offset` 2×2, E5 broken-follow cycle fallback). Gates: `train_follow_success≥0.95`, full train advances 1/tick, on leader-fail the whole train holds (correct cascade), E3 follower moves despite fatigue, E5 no panic.
- **F — Flee/kite** (F1 single threat open, F2 multi-threat pincer, F3 flee into terrain, F4 flee ops-starved). Gates: `flee_min_range≥0`, `flee_into_corner==false`, defined failure under the 2000-op flee cap.
- **G — Anchored-worker shove** (G1 upgrader shoved, G2 anchor saturation). Gates: `anchor_violation==0`, worker stays in work range.
- **H — Stuck/repath recovery** (H1 transient block clears, H2 friendly-creep cluster tier escalation, H3 no-progress repath, H4 permanent block). Gates: recovers before `report_failure`(12), `stuck_repaths` bounded, H4 `Failed(StuckTimeout{≈12})` no infinite spin; escalated searches stay ≤ `MAX_PATHFIND_OPS` (the `ops_multiplier` ceiling, not the 2000 base).
- **I — Adversarial / regression corpus** (I1 real-layout import from `rover-terrain.json`, I2 determinism fuzz random-world random-Handle, I3 worst-case-ops maze). Gates: regression baseline, **I2 spread==0**, I3 ops < `MAX_PATHFIND_OPS` with defined cap behavior.

### §D7 The determinism fence (first-class ship-blocker)

The invariant: **identical inputs → bit-identical outputs across repeated AND reordered runs.** The historical offender was three seed-ordered `std::HashMap` iterations in `resolver.rs` (`topological_sort_follows` consuming the shared ops budget in seed order; `current_pos_to_entity` last-write-wins; swap discovery order), each now fixed with explicit `Handle`-sorted / `(room,x,y)`-sorted tie-breaks (`resolver.rs:146,225,239,297,379,424-443,511-521,574-580`). `MovementSystem` already requires `Handle: Ord`. The eval enforces three sub-gates, mirroring the combat-eval's `sim_is_deterministic_over_rounds` (spread 0):

- `det_repeat` — run a scenario R≥8 times from the same seed/world → all metric vectors bit-identical (spread 0);
- `det_reorder` — permute the creep-insertion order P ways → identical final trajectories & metrics;
- `det_hash_seed` — run under K different process-global HashMap seeds (a seeded `BuildHasher` injected into the harness, or run the binary K times with randomized SipHash keys and diff) → identical results.

The hash covers the **full per-tick recording** (positions + fatigue + issued directions + the built cost-matrix bytes — the `SparseCostMatrix` HashMap is the latent hazard to include). **Gate: spread == 0 on all three; any nonzero spread is a ship-blocker that localizes a leaked HashMap iteration.** Debugging method (the known one): thread-local capture of each resolver iteration + per-run diff. **`det_reorder` must be cheap to run on *every* scenario** so a leaked iteration is caught the instant it appears, not only in a dedicated test. (`param_sweep.rs` additionally pins `sweep_point_is_deterministic` via `f64::to_bits` run-twice, the combat-eval pattern.)

### §D8 Where every gate sits in the 0015 L0–L6 taxonomy

This sim is **not a single layer** — it is the substrate that populates L0–L5 for the movement subsystem (the way 0006's combat sim populates them for combat). Adopting the mapping lets 0033 inherit 0015's policy instead of re-litigating it:

| Layer | Rover-eval content | Assertion form (per `0015:28`) |
|---|---|---|
| **L0** kernel unit | fatigue accounting, sustained-speed predicate, single A* step cost, flee-cost eval, `resolver` contention winner on fixed inputs (the existing "rover resolver" tranche, `0015:206`) | exact |
| **L1** fixture-component | the rover movement/shove pipeline against the reused `sim-core` `SimWorld` (the `0015:33` "rover movement/shove pipeline vs FakePathfinder" row, enriched from a stub to a real shared world) | exact |
| **L2** property / golden vectors | **metamorphic relations** (`0015:130/161`): an added obstacle never shortens a path; more MOVE never slows arrival; a road never increases fatigue-steps; flee-distance monotone in threat range; route cost ≥ Chebyshev lower bound. Golden `(world,start,goal)→(path,ticks,fatigue,ops)` vectors reproduced byte-exact. **Routes asserted as properties/distributions, NEVER exact (`0015:130`).** | exact relations/snapshots |
| **L3** replay-parity / determinism | the §D7 fence — `0015:125` assigns determinism here ("a nondeterministic system fails parity against its own recording") | exact byte-diff |
| **L4** in-process composition | seeded movement-stress invariants: no two creeps on a tile; no fatigue>0 move without a pull; vacate-then-follow same-tick; **no deadlock/hang**; ops ≤ per-tick pool; `MIN_PATHFIND_OPS` floor at Critical; unreachable → `Failed` not a hang (the S3 invariants, `0015:71`) | hard invariants, no outcomes |
| **L5** scenario-behavioral | the **benchmark** half — distributional median/p95 ticks-to-arrival, fatigue-efficiency, wasted-move-rate, ops/search over the corpus; paired-vs-baseline, never exact | distributions / paired-vs-baseline |
| **L6** soak + canary | the **same numbers** the live seg-57 canary / ADR-0004 ops telemetry emit — wasted-move-rate (gap **G-13**), ops-saturation — so offline bench and live canary cross-vouch (the 0006:48/110 pattern) | live invariants only |

**One-artifact-per-gate (`0015:43`):** 0033 does **not** re-mint S3 (it exercises/gates the existing seam), does **not** assert exact routes (forbidden), does **not** treat host wall-clock as a CPU-budget validator (0015's standing caveat), and does **not** duplicate the combat sim's world/physics (it reuses `sim-core`). Its net-new contribution is the rover-quality metric set + the ground-truth oracle + the rover scenario catalog + the corpus benchmark, all over the *reused* `sim-core` substrate (`SimWorld` + `resolve_movement` + the offline rover driver) — making those already-promised-but-thin rover checks rich and gated, and emitting the same G-13 / ops-saturation numbers the canary watches.

---

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Extract `screeps-sim-core` + a host-only `screeps-rover-eval` reusing the combat sim's mover (CHOSEN)** | Reuses the bot's *own* rover code AND the combat sim's already-built world/physics/driver — one mover, **no re-ported physics**; clean layering (a movement bench depends on `sim-core`, not `combat-engine`/`CombatWorld`); a **reusable mechanism foundation** for future non-combat sims (economy/hauling/lifecycle); fast & offline → CI-gateable; bit-deterministic; single-creep optima exact via `gridsearch`; zero new live CPU | The `resolve_tick`→`resolve_movement`+`resolve_combat_tick` split, the `SimWorld`/`CombatState` recomposition, and the `Combat*`→`Sim*` rename are a **real refactor of a stable 40-test crate** (blast radius across engine/agent/eval), gated byte-identical by the combat fence; multi-creep optimum NP-hard → regression baselines only there |
| **Reuse `combat-engine`/`combat-agent` in place — NO extraction** (the independent analysis's recommendation) | Lowest churn; the shared primitives are already correctly layered; no refactor of the stable engine | A movement benchmark then depends on a crate named `combat-engine` + a world named `CombatWorld` and pulls unused combat code; no reusable mechanism crate for future sims. **Operator chose clean layering + a reusable foundation over minimal churn** (decision provenance, §Decision) |
| **Build a fresh `RoverWorld` + re-port the fatigue/contention/edge physics** | Zero coupling to the combat crates | **DUPLICATION** — the combat sim already runs rover via the same two-halves; a second physics half drifts from the first *and* from the engine, doubling the conformance-maintenance surface. Exactly what this ADR exists to avoid |
| **Extend the combat sim (ADR 0023) to also grade movement** | Reuses an existing engine + recording + harness; rover already runs inside it | **Welds two disjoint subjects** (combat decisions vs mover quality), violating 0023's clean boundary and `0015:43` one-artifact rule; the combat sim asserts engagement outcomes, not route optimality/fatigue/ops — wrong metric vocabulary; couples rover regressions to combat-scenario flakiness (exactly how the seed-flake hid for so long) |
| **criterion-only microbenchmarks of the hot functions** | Trivial to add; statistical wall-clock rigor on `run`/`resolve_conflicts`/`moving_maximum` | Measures *only* CPU, and only host wall-clock (≠ MMO CPU, blind to intents, `0015:133`); says nothing about route optimality, fatigue, congestion, or determinism; criterion is used nowhere else in this repo — bucks the established `Instant`-loop + loose-death-spiral convention |
| **Live/Docker private-server movement measurement** | Highest fidelity — the real engine resolves moves | Slow, blind (intermittent intel — the documented live-debugging unreliability), and **cannot be bit-fenced** (real server nondeterminism); the 0006:125 argument — a server harness is for conformance vectors, not the per-change quality/CPU gate |
| **Pure unit tests only, no sim substrate** | No new crate; lowest effort | Misses the emergent classes the operator actually hit — contention, trains, multi-room edge thrash, deadlock/livelock, congestion CPU blowup; cannot benchmark a corpus or track regressions distributionally; leaves "time-moving / fatigue efficiency" unmeasured |
| **`screeps-server-mockup` / a JS engine harness** | Closer to the real server than a hand-ported physics half | Node/JS dependency bucks the Rust-first stack (memory: prefer Rust over Node); async/non-deterministic; rejected for the same reasons 0006:127 / 0015:167 rejected it for combat |

---

## Consequences

### Positive

- **The operator's headline asks become numbers** — `R_ticks` / `fatigue_util` / `movement_eff` quantify "time creeps spend moving" and "efficient usage of fatigue"; `R_fatigue` / `R_len` / `oscillation` quantify "optimal pathing"; the scaling curves + ops gates quantify "algorithmic/cpu benchmarking."
- **The "validated separately" handoff from ADR 0023 is finally a real artifact**, not a promise. Rover regressions are caught directly and offline, not accidentally via a downstream combat flake.
- **The single most actionable optimization is surfaced and measurable**: rover's cost field is not body-aware, so `R_fatigue==1` can coexist with `R_ticks>1` for low-MOVE bodies. The A4/B3 scenarios turn that into a gate and a target.
- **Determinism becomes a directly-targeted first-class fence** on the documented historical offender, runnable on every scenario — the strongest "in-voice" gate for this codebase.
- **Cross-vouching with the live canary**: the offline bench emits the same G-13 wasted-move-rate and ADR-0004 ops-saturation numbers the seg-57 stream watches, so each validates the other (the 0006 pattern).
- **No duplicated mover, and a reusable mechanism foundation.** The bench runs the bot's *real* rover over the combat sim's *reused* world/physics/driver — one mover, one server half, validated once. The `screeps-sim-core` extraction gives every future offline sim (economy, hauling, lifecycle — ADR 0028) the same substrate, and turns the dead `FATIGUE_RATE_ROAD` constant live for **both** sims.

### Negative / costs

- **The `sim-core` extraction is real, one-time engineering** — splitting `resolve_tick` into `sim_core::resolve_movement` + `resolve_combat_tick`, recomposing the world as `SimWorld` + `CombatState`, and the `Combat*`→`Sim*` rename touch `combat-engine` / `agent` / `eval`. The hard guard: the recomposed combat tick must be **byte-identical** to today's — the existing combat determinism fence (spread 0) + the FP/FN calibration are the gate (§M0). *After* it, the server half is **reused, not re-ported**, so there is no second physics half to drift from the engine.
- **Overfit-to-sim risk** — the bot could be tuned to ace the sim and regress live. Mitigated by the 0006 fidelity triple (conformance golden vectors captured from the live server / parity report / MMO canary alignment) and by holding mechanism fixed while sweeping *policy* (the sweep), not the reverse.
- **Multi-creep has no exact optimum** — D-family gates are invariants (no deadlock) + committed regression baselines, not optimality. The ADR states this asymmetry so reviewers don't expect more.
- **Two new crates + an engine refactor**, not one — but `sim-core` is mechanism code factored *out* of an existing crate (net new lines roughly flat), `rover-eval` is plumbing-identical to three existing host crates, and the refactor is one-time. The SHARED-FIXES (roads, loaded-CARRY) are additive and default-inert.

### CPU & tick-safety impact

- **Zero new live CPU; no reset.** `screeps-rover-eval` is host-only (rayon/clap), excluded from all three wasm builds. The `sim-core` extraction is a host+wasm **refactor** with no serialized-shape change → **no `WORLD_FORMAT_VERSION` bump, no reset** (the bot's saved state is untouched; `combat-engine`/`sim-core` are sims, not bot state). The `rover` feature only adds the wasm-safe `screeps-rover` dep, already in the bot. This mirrors 0006:132's "the harness adds no MMO cost."
- The new `pub` surface (`SimWorldCostSource::from_world`, the moved `Rng`) is purely additive; the two SHARED-FIXES are **default-inert** (every existing combat scenario stays byte-identical, proven by the fence).

---

## Incremental Migration Path

Named, independently-testable slices, each with a gate (the 0023 S1–S5 / 0028 K0–K4 / 0006 Inc A–E idiom). **Bot serialized-shape / state-drop: None at any step** — host + sim code only, no `WORLD_FORMAT_VERSION` bump, no reset (`0015:204`). The one *internal* breaking change is `combat-engine`'s API in M0 (the `SimWorld`/`CombatState` recomposition + the `Combat*`→`Sim*` rename); all consumers are in-workspace and updated in lockstep.

- **M0 — Extract the `screeps-sim-core` kernel; make combat a thin layer. ✅ LANDED 2026-07-01 (byte-identical).** Moved `terrain` / `body`(movement methods) / `world`(`MovementState`) / `movement`(contention resolver) / the movement tick (`resolve_movement`) / `intents`(`MoveIntents`) / movement `constants` into `screeps-sim-core`, and added the `Simulation` trait + `MovementSim`. `combat-engine` became a thin layer: `CombatWorld { movement: MovementState, … }` (**composition, no `Deref`**), `resolve_tick` calls `sim_core::resolve_movement`, body-combat moved to the **`SimBodyCombat` extension trait**, combat constants restored to combat-engine, and **all re-export shims deleted**. New submodule (pushed) + git-dep + root `[patch]`; combat-engine/agent/eval migrated to the composed world + kernel imports. **Gate met:** 55 tests preserved (kernel 19 + engine 36); agent 52 / eval 106 / decision 311 unchanged; wasm-compat confirmed; clippy clean. As-built details in §D1a. *(Note: M1–M5 below retain the pre-implementation naming — `SimWorld`≈`MovementState`, `resolve_movement`≈`resolve_movement`, and the `resolve_combat_tick`/`CombatState` split was NOT taken (composition was) — they will settle to the as-built names as each lands.)*
- **M1 — Move the offline rover driver down (sim-core `rover` feature). ✅ LANDED 2026-07-01.** Relocated `resolve_moves_via_system` + `SimMoveRequest`/`SimMoveGoal`/`SimMoveCache` + `SimCreepHandle` + `SimMovementExternal` + `DEFAULT_SHOVE_DEPTH` into `screeps-sim-core::rover_driver` (feature `rover`). **Design departure from this sketch (cleaner):** the combat cost source was **NOT** moved/renamed to a kernel `SimWorldCostSource`. Instead the kernel driver is **generic over the routing policy** — `resolve_moves_via_system<S: CostMatrixDataSource>(movement: &MovementState, requests, cache, cost_source: S)` — and the `SimMovementExternal`/`SimCreepHandle` read **only `MovementState`** (never combat state), so the kernel has **zero** combat coupling. `CombatWorldCostSource` (towers / ADR-0024 threat / structures) **stays in `combat-agent`** as its pricing policy, injected via a thin same-named `combat-agent::pathing::resolve_moves_via_system` **wrapper**; `from_intent` (needs `CombatIntent`) became the free fn `move_request_from_intent`. `combat-agent`'s `sim-core` dep gains `features=["rover"]`; sim-core's optional rover dep switched **path→git** (convention). The SplitMix64 `Rng` was also promoted to the kernel — `sim-core::rng::Rng` (unconditional, no feature) — and `combat-eval` **de-duplicated** onto it (its private copy deleted, a direct `sim-core` dep added, byte-identical stream ⇒ eval 106 unchanged), so both harnesses share one deterministic RNG. **Gate met:** `combat-agent` squad + the 2 mover integration tests unchanged (52); engine 36 / eval 106(+19) / rover-eval 9 unchanged; sim-core 25 with `--features rover` (+driver smoke over a `PlainCostSource`); combat-agent + **full-workspace** wasm build green (rover driver is wasm-compatible).
- **M2 — Stand up `screeps-rover-eval` reusing everything (Tier A + oracle + A-family).** New host-only member; depends on `sim-core[rover]` + `combat-eval` (harness) + `rover`. First scenario: build a `SimWorld` via `ScenarioBuilder`, drive `resolve_moves_via_system`, apply via `resolve_movement`, capture via `record_tick`, render via `visualize`; reuse `terrain_import` for one real room; build `oracle.rs` (`gridsearch`-backed) + the A-family. **Proves the reuse wiring end-to-end before changing any physics. Gate:** a creep walks start→goal + replay renders; `R_fatigue ≤ 1+ε` / `R_len==1` (uniform) on A1–A6; A7 fails correctly; `det_repeat`/`det_reorder` spread==0 on the A corpus.
- **M3 — Land the two SHARED-FIXES in `sim-core` (roads + loaded-CARRY). ✅ PHYSICS LANDED 2026-07-01 (default-inert).** `roads: HashSet<(u8,u8)>` on `SimTerrain` + road-first `fatigue_rate` (road 1 → swamp 10 → plain 2); `carry_used: u32` on `SimCreep` + `SimBody::carry_weight` (engine `calcResourcesWeight`, boost-aware) + `SimCreep::fatigue_weight` (structural + carry), consumed by both the mover's rate4 denominator and the tick accrual; rover-eval's cost source made road-aware (+ its `fatigue_rate` delegated to the kernel). As-built detail in the two **AS BUILT** notes under §D3. **Gate met (inert):** combat suites re-pass at their M0 counts — engine 36 / agent 52 / eval 106(+19 ign) unchanged (the 44 combat `SimCreep` literals took a mechanical `carry_used: 0`); sim-core 19→24 (+road/carry/tick tests), rover-eval 4→5 (+road corridor); wasm-compat re-confirmed. **Follow-on (landed 2026-07-01, same session):** the Tier-A travel-time metrics this physics unblocks — `R_ticks` / `movement_eff` / `fatigue_util` + the single-creep `traverse.rs` kernel-driven route-walker — are now in `screeps-rover-eval` (see the M4 note); the remaining **B-family** items (the full B1–B5 scenario corpus gates + the true time-expanded `T_min` oracle) land with M4's Tier-B work.
- **M4 — Tier B contention + metrics + the determinism fence (D/E + the rover lens).** *(Tier-A metric forms ✅ landed early 2026-07-01: `R_ticks` / `movement_eff` / `fatigue_util` in `metrics.rs`, over the new single-creep `traverse.rs` — a kernel-driven route-walker that counts fatigue stalls tick-by-tick. `fatigue_util` is `1 − idle_free/ticks`: `==1` for a lone creep by construction — the "rover blameless" invariant — and only becomes a live signal under Tier-B contention, where `idle_free` = traffic idle. `R_ticks`'s denominator is today the fatigue-cost-optimal route's greedy traversal, not a true time-expanded optimum — the note in `r_ticks` flags the refinement. A **scenario harness** (`scenario.rs`, ADR §D4) also landed: a `Scenario` (terrain + body + route + metric gates), a curated `catalog()` (open/wall/swamp/road + under-MOVE/loaded/swamp-trap), a seeded procedural `generate()` (reusing `sim-core::rng`), and a `validate()` that runs the oracle + metrics and checks the gates — the systematic corpus that scales past ad-hoc fixtures. rover-eval 4→12 tests.)* **Remaining (the Tier-B core):** drive the full `MovementSystem` resolver through `evaluate_recorded` (needs M1); implement congestion + lift `oscillation_rate`/`cohesion`; add D-family + E-family (the latter needs pull activated in the shared driver, §D3) + the deadlock/livelock detectors; promote §D7 to a corpus-wide gate (rover aggregate spread==0, `det_hash_seed`, cost-matrix bytes hashed). **Gate:** `deadlock==0`, `livelock==0`, `train_follow_success≥0.95`, `idle_blocked` partition correct; **the full fence spread==0 on every scenario.**
- **M5 — Multi-room + CPU bench + sweep + CLI + canary alignment.** C-/F-/G-/H-families (reuse 0023's edge-exit fixtures, `0023:13-18`); the `bench.rs`-shape op-counted + `Instant`-timed CPU gates with scaling curves (via the now-`pub` `SimWorldCostSource` + `tick_stats()`); the rayon env-driven sweep over rover tunables (`pathfinding_ops_budget`/`max_shove_depth`/`reuse_path_length`/`StuckThresholds`) reusing `env_*_list`; the clap CLI over `terrain_import` corpora; commit the multi-creep regression baselines; align the L5 metrics with the live **G-13** wasted-move-rate + ADR-0004 ops telemetry. **Gate:** C5 no border thrash (the `aaac0f7` class); `anchor_violation==0`; scaling-curve slopes within baseline; `run_register` all-green; offline ↔ canary definitions aligned.

Each slice is shippable on master independently, gated by its own `#[test]`s in the host test pass, and adds zero live CPU.

---

## Cross-references

- ADRs: [0006](0006-eval-and-iteration-harness.md) (sibling micro-sim pattern + fidelity triple), [0023](0023-nroom-combat-sim.md) / [0023a](0023a-staged-combat-harness.md) (the "validated separately" rover handoff + the corrected edge-exit movement model + the staged Generation/Evaluation/Validation idiom; **the combat sim whose mover/world `sim-core` is extracted from**), [0015](0015-testing-and-validation-strategy.md) (L0–L6 taxonomy, assertion rule, seam S3, no-exact-routes `:130`, determinism prereq `:125`, G-13 / ops canary `:38`), [0004](0004-cpu-governance-and-load-shedding.md) (S3 ops pool + saturation telemetry), [0009a](0009a-room-planner-performance.md) (prior pathing-perf corpus-bench precedent), [0028](0028-lifecycle-harness.md) (a future consumer of the extracted `sim-core` substrate).
- Ground truth: `docs/references/engine-mechanics.md §1.6` (movement/fatigue, VERIFIED against engine source) and §6 (power-creep no-fatigue).
- Rover seam under benchmark: `screeps-rover/src/traits.rs` (`CreepHandle`/`PathfindingProvider`/`CostMatrixDataSource`/`MovementVisualizer`), `movementsystem.rs:229-250,459-797` (`MovementSystemExternal`, `process`), `local_pathfinder.rs:135-162,224-426,438-613` (`moving_maximum`, search core, headless impl), `gridsearch.rs:32-195` (the oracle primitives), `anchor.rs:80-167` (`AnchorPath`), `resolver.rs:128-251,266-558,561-643,656-821` (the determinism-fenced resolver), `movementrequest.rs` (intents/priority/anchor), `screeps_impl.rs` (the live adapter, the seam's other consumer).
- Reuse seams (the offline loop — all relocating into `screeps-sim-core`): the rover driver `screeps-combat-agent/src/pathing.rs:240-518` (`resolve_moves_via_system` + `SimMoveRequest`/`SimMoveGoal`/`SimMoveCache` + `CombatCreepHandle`/`CombatMovementExternal`/`CombatWorldCostSource` → renamed `Sim*` in the move); the server half `screeps-combat-engine/src/resolve.rs:203` (`resolve_tick` → split into `resolve_movement`) + `movement.rs:99` (contention) + `state.rs:11,73,131` (`SimTerrain`/`SimCreep`/`SimWorld`) + `body.rs:155` (`fatigue_weight`, SHARED-FIX-2) + `constants.rs:44` (dead `FATIGUE_RATE_ROAD`, SHARED-FIX-1) + `record.rs` (recording); the SplitMix64 `Rng` at `screeps-combat-eval/src/harness/generate.rs:54`.
- Harness templates: `screeps-combat-eval/src/{lib.rs,metrics.rs,bench.rs,tournament.rs,harness/*}` (the staged shape + the `sim_is_deterministic_over_rounds` fence at `tournament.rs:802-814`); `screeps-foreman-bench/src/main.rs` (the corpus-CLI + map-data format); `screeps-ibex-eval` (the simpler gate-runner). The [Sim determinism fence] memory (the rover resolver as the historical noise source + the thread-local-capture debugging method).
