# Ultracode Review Kickoff — Ibex (Screeps AI Bot): Design, Architecture, Correctness, Performance & Competitive Fitness — Pre-Rewrite Review

## 1. Mission Context

**Ibex** is an autonomous bot for **Screeps**, a persistent MMO-RTS in which player-authored code executes every tick (even while offline) inside the game's V8 JavaScript sandbox. The competitive objective is threefold: **survive** (never crash, never lose state), **expand** (claim and develop rooms), and **score** (accrue resources/GCL/points against rival players). Ibex is **Rust compiled to WASM**, invoked from a thin JS entry each tick. It uses **specs 0.20 ECS** (`specs::World`; ~60+ systems run in a fixed per-tick order), persists mutable state through **RawMemory segments** (bincode → gzip → base64 → 50 KiB chunks), and binds to the game via a **local fork of `screeps-game-api`** (`C:\code\screeps-game-api`).

The binding constraints of the domain:
- **CPU — and it is *not* just execution time.** Each tick has a `Game.cpu` limit plus a bucket. Most game **action intents** (move, transfer, attack, build, repair, …) are queued into the tick's intent database and **charge CPU when logged**, so any cost model that counts only Rust execution **under-measures** real CPU. Measure execution **+ intents** (the open-source engine, below, is authoritative for exact costs).
- **VM-reset resilience.** The sandbox may reset between any two ticks, so no Rust heap/static state may be relied on for correctness — only serialized segment state survives.
- **Single-threaded WASM (no *parallelism*).** Execution is strictly sequential within a tick (ticks run concurrently *across players*, but never within your code). OS threads, multi-threaded executors, locks, and atomics-for-parallelism don't apply. *Single-threaded cooperative `async` on a custom executor is possible* and worth considering **only** as part of a deliberate runtime-model change (e.g. off ECS) — weigh its complexity. Throughput comes from algorithmic efficiency and bucket-aware scheduling, not concurrency.

**Screeps is a competition, not just a codebase** — evaluate whether the design *wins the game* with the same rigor as code quality. The **game engine is open-source** (https://github.com/screeps/engine) and is the ground-truth for CPU/intent costs and mechanics.

Scale: main crate `screeps-ibex/src/` ≈ 38,093 lines, 123 files.

---

## 2. Review Objectives & Scope

This review **precedes a planned large-scale rewrite** and feeds directly into a **project plan** built afterward. So it must do more than catalog bugs: judge what to **keep / refactor / replace**, propose **alternatives** where the current design is brittle, and lay groundwork for **testing** and **recursive self-improvement** (§11–§12). Deliver a rigorous, prioritized assessment across these lenses:

1. **Design soundness** — are the operation→mission→job and room/ECS models coherent; do abstractions earn their keep, or should they be replaced in the rewrite?
2. **Architectural integrity** — layering, coupling, dispatch ordering, persistence/entity model.
3. **Screeps-domain fitness** — tick-safety, CPU bounds (execution **+ intents**), single-thread constraints, reset-resilience.
4. **Correctness & robustness** — panic vectors, dangling-entity handling, serialization edge cases, state-machine convergence. (**Flag-and-track, don't root-cause now** — §10.)
5. **Performance / CPU** — per-tick algorithmic bounds, the **pathfinding CPU-exhaustion death-spiral** (Field Report C), cost-matrix and intent cost.
6. **Strategic competitiveness** — does the design actually win (economy, expansion, **military/squad cohesion**, defense, market)?
7. **Maintainability & evolvability** — oversized files, duplication, testability, and how fast the team can *safely* iterate.
8. **Rewrite readiness & alternatives** — for each brittle pillar (serialization, ECS/entity model, job/squad behavior modeling), propose concrete alternative designs with trade-offs.
9. **Observability & self-improvement** — what telemetry (console + segments) and offline validation would let the bot, its maintainers, and agentic tooling detect regressions and improve recursively.

**In scope (primary):** main crate `screeps-ibex/src/`.
**In scope (secondary/contextual):** support crates `screeps-foreman` (planning), `screeps-rover` (movement), `screeps-cache`, `screeps-machine`, `screeps-timing`, and the `screeps-game-api` fork — at their **integration boundary** with Ibex.
**In scope (forward-looking):** **architectural alternatives and a clean-slate redesign direction** — this review explicitly informs the rewrite.
**Out of scope:** writing/changing code in this pass (propose, don't edit); redesigning the *game*; deep-auditing upstream crate internals; fully root-causing every bug (track them for later deep-dives).

> ### ♻️ Rewrite Mandate — incremental & confidence-driven; backward compatibility NOT required
> An **incremental rewrite** is planned — **not a one-shot rebuild** — so each step must build **confidence in behavior** before the next. **Breaking changes are acceptable at any step, including dropping serialized state**; the "serialization is a contract / never break memory" rule (`AGENTS.md` §6) is **lifted**. Prefer the *right* long-term design even if it discards segment state, changes the format, or replaces ECS / the state-machine — but **favor designs that migrate in stages** (strangler-fig: replace one subsystem at a time behind a stable seam) and are **verifiable per increment** (tests, replay, parallel-run/compare — §11–§12). **Label** breaking changes for sequencing, and **don't break the currently-running bot** mid-increment. Don't weaken a recommendation to preserve compatibility — but *do* weigh how cleanly it can land in steps. Where the persistence/entity approach is brittle (Field Reports D & E), **propose alternatives** (§12).

> ### ⭐ Operator Field Reports — observed production failures (READ FIRST; weight above static analysis)
> First-hand failures from running Ibex on a **private server** — the primary motivation for this review and the rewrite, and higher-signal than any static finding. Investigate the **root mechanism** and **rewrite implication** of each (flag-and-track; deep-dive later).
>
> - **A. War / squad cohesion is broken.** Creeps **fail to group into "quads"** and scatter instead of holding formation; the operator **trusts the war system very little**. Because acting on teammates (heal/defend/focus) requires being **in range**, cohesion is a **correctness requirement**, not polish. → `military/formation.rs`, `squad.rs`, `jobs/squad_combat.rs`, squad missions (§6.5/§6.7).
> - **B. Operation/mission lifecycle gets stuck.** Operations & missions **behave wrongly or hang** (don't progress or tear down), especially war/attack. → operation/mission state lifecycle, child ownership, teardown (§6.1/§6.3/§6.5): find states with **no exit**.
> - **C. CPU death-spiral via pathfinding — design for load-shedding, not reproduction.** **Long pathfinding spikes CPU → bucket exhaustion → constant tick restarts → no progress → colony collapse** — a **survival-critical / extinction** failure mode. It is **probably not reliably reproducible** (emergent from many interacting systems), so **don't chase a repro** — instead make **load-shedding a first-class design property**: every expensive system must be able to **defer/shed work under CPU pressure and still make forward progress and recover**. A **global CPU governor + pathfinding budget + graceful degradation** is a top rewrite requirement (§6.9, §8, §11–§12).
> - **D. Serialization is consistently problematic** — repeated **breakage** and fragile **entity-mapping**; highly brittle. Primary driver to evaluate **alternatives** (§6.2, §12).
> - **E. ECS is a dangling-reference bug farm.** specs/ECS buys serialization + Rust-lifetime decoupling between systems, but **dangling `Entity` refs recur** (the per-tick `repair_entity_integrity` scan exists *because* of this). Does ECS earn its keep vs. alternatives (typed generational handles, an ID-keyed store)? (§6.1, §12).
> - **F. Job state machine friction.** The `screeps-machine` FSM is useful but **sometimes inflexible / hard to understand**. Evaluate alternatives (behavior trees, utility AI, data-driven FSMs) for jobs *and* squads (§6.6, §12).
> - **G. Pathfinding itself seems OK** (under-tested but acceptable) — the acute pain is **squad/group coordination**, not single-creep routing. Bound CPU here; invest correctness effort in cohesion, not routing.
> - **H. The world renderer corrupts all rendering.** Enabling the world/visualization renderer **breaks all subsequent rendering, including room visualization** — likely a **serialization/encoding fault** in the visual layer (ties to Field Report D). This is a **debugging-capability blocker**: it prevents operation-level visual debugging, compounding A & B. **Compare the emitted visual payload against the engine's expected format** (`screeps/engine` visual handling) to find the mismatch. → `visualization.rs`, `visualize.rs`, `screeps-visual`, RoomVisual/MapVisual encoding (§6.12).

---

## 3. Architecture Primer

Read `AGENTS.md` (repo root) and `screeps-ibex/src/game_loop.rs` first; they are authoritative.

**Layered strategy model (operation → mission → job):**
- **Operations** (`operations/`) — high-level campaigns (Colony, Claim, MiningOutpost, Scout, Attack, War). Own missions; set strategy. Dispatched via the `OperationData` enum (6 variants).
- **Missions** (`missions/`) — room/objective-scoped goals (LocalSupply, LocalBuild, Upgrade, Haul, Tower, Labs, Terminal, SquadAssault/Defense, etc.). Produce & consume jobs. Dispatched via `MissionData` (24 variants, each wrapped in `EntityRefCell`).
- **Jobs** (`jobs/`) — per-creep work implementing the `Job` trait (`describe`/`pre_run_job`/`run_job`). 11 variants via `JobData`; each is a state machine (via `screeps-machine`, `MAX_STATE_TRANSITIONS=20`/tick).
- **Rooms** — one ECS entity each (`RoomData` + entity mapping); room systems plan/update/track visibility.

**Tick flow** (`game_loop::tick()` → `for_each_system` macro, systems in fixed order, `world.maintain()` after each):
- **Pre-pass:** dead-creep cleanup → entity cleanup → room data create/update → entity mapping → threat & economy assessment.
- **Main pass:** Operations → Missions → Squads → Jobs (each `pre_run` then `run`) → movement → visibility → spawn queue → transfer queue → order queue → room planning → stats → cost-matrix store.
- **End of tick:** `cleanup_memory()` (purge `Memory.creeps`) → `repair_entity_integrity()` (5-phase scan that fixes dangling `Entity` refs before serialization — **required**, or `ConvertSaveload` can panic) → `serialize_world()` (segments 50–55).

**Persistence model** (`memorysystem.rs`, `serialize.rs`): `MemoryArbiter` gates execution until required segments are active (one-tick request latency); on first load after reset, `deserialize_world` runs. New serialized fields **must** carry `#[serde(default)]`/`Default` for backward-compat — enforced by convention only, with no compile-time check and no version header.

---

## 4. Subsystem Map

All 12 subsystems below have a matching focus block in §6. Line counts verified against survey FACTS.

| # | Subsystem | Key files (LOC) | Responsibility | Scale | Hotspots |
|---|---|---|---|---|---|
| 1 | **Tick orchestration & ECS core** | `lib.rs`, `game_loop.rs` (834), `memorysystem.rs`, `cleanup.rs`, `entitymappingsystem.rs`, `machine_tick.rs` | WASM entry, env lifecycle, system dispatcher, entity-integrity repair, (de)serialize | `game_loop.rs` 834 | `repair_entity_integrity` 5-phase scan (168–369); `EntityCleanupSystem` cascade (`MAX_CASCADE_ITERATIONS=200`); `for_each_system` macro opacity |
| 2 | **Memory & serialization** | `memorysystem.rs`, `serialize.rs`, `game_loop.rs`, `memory_helper.rs`, `pathing/costmatrixsystem.rs` | Segment I/O, encode pipeline, 50 KiB gating, entity-ref wrappers | — | `from_utf8_unchecked` (441); silent deser (508/533); seg-55 cost-matrix/ECS collision |
| 3 | **Operations (campaigns)** | `war.rs` (1444), `claim.rs` (921), `attack.rs` (822), `colony.rs`, `scout.rs`, `miningoutpost.rs`, `operationsystem.rs`, `data.rs` | High-level campaigns; own missions | `war.rs` 1444 | `war.rs` 3-tier cadences + parallel attack vectors; `attack.rs:615` `.unwrap().unwrap()`; greedy `reassign_home_rooms` O(A·H) |
| 4 | **Economy & infra missions** | `missions/colony.rs`, `localsupply/*` (2198), `localbuild.rs`, `upgrade.rs`, `haul.rs`, `tower.rs`, `terminal.rs`, `labs.rs` (794), `construction.rs`, `missionsystem.rs` | Mining, hauling, building, upgrading, labs, terminal | missions 10,661 | `SupplyStructureCache` 10-tick staleness; `compute_nearest_spawn_distances` pathfinding spikes; `terminal.rs`/`labs.rs` branching |
| 5 | **Combat & expansion missions** | `attack_mission.rs` (2040), `squad_assault.rs`, `squad_defense.rs`, `squad_harass.rs`, `defend.rs`, `dismantle.rs`, `raid.rs`, `claim.rs`, `scout.rs` | Multi-squad assault/defense, claim, scout, dismantle | `attack_mission.rs` 2040 | `attack_mission.rs:1917` `.expect`; `SquadMember` entity refs **not** repaired pre-serialize; duplicated `TTL=1200` |
| 6 | **Jobs layer** | `jobsystem.rs`, `data.rs`, `context.rs`, `actions.rs`, `haul.rs`, `squad_combat.rs` (1001), `utility/haulbehavior.rs`, `harvest.rs`, `build.rs`, `staticmine.rs`, `scout.rs` | Per-creep state machines; transfer/move/visibility integration | jobs 3,832 (+utility 1,859) | partial-haul abandon (`haulbehavior.rs:513–567`); `staticmine.rs:201` `.unwrap()`; squad-combat per-tick hostile scans |
| 7 | **Military core** | `military/squad.rs` (1021), `formation.rs`, `composition.rs`, `bodies.rs`, `damage.rs`, `threatmap.rs`, `economy.rs`, `boostqueue.rs` | Squad lifecycle, formations, threat classification, combat economics | military 3,590 | `advance_squad_virtual_position` (201–359); heal assignment O(H·T·H); `BoostQueue` ephemeral (lost on reset) |
| 8 | **Room data, visibility & planning** | `room/data.rs` (899), `visibilitysystem.rs`, `roomplansystem.rs`, `gather.rs`, `room_status_cache.rs` (new), `createroomsystem.rs`, `updateroomsystem.rs` | Room entity lifecycle, visibility queue, multi-tick planning (foreman) | room 2,324 | monolithic `RoomData`; `visibilitysystem.rs:362–363` double-unwrap; unbudgeted gather BFS; seg-60 planner state |
| 9 | **Pathing & movement** | `pathing/movementsystem.rs`, `pathing/costmatrixsystem.rs`, `screeps-rover/*`, `jobs/utility/movebehavior.rs`, `room_status_cache.rs` | Pathfinding, collision/shove/swap, stuck escalation, CPU budgeting | pathing 378 (delegates to rover) | no job-layer stuck recovery; cache-eviction TODO (clears ephemeral every tick); unbounded `find_route`; no-progress-repath deadlock |
| 10 | **Transfer & market logistics** | `transfer/transfersystem.rs` (2439), `ordersystem.rs`, `utility.rs` | Haul supply/demand matching; market buy/sell | transfer 3,012 | panic on invalid `TransferTarget` (208–289); O(rooms·targets·resources) matching; weak price-manipulation guards |
| 11 | **Spawning, stats & helpers** | `spawnsystem.rs`, `statssystem.rs`, `stats_history.rs`, `features.rs`, `structureidentifier.rs`, `repairqueue.rs`, `remoteobjectid.rs`, `findnearest.rs` | Spawn queue, stats/history, feature flags, shared utilities | top-level 6,118 | spawn energy-gating: break-on-first-unaffordable consume (`spawnsystem.rs:236–248`) + body-sizing vs min-cost (todo.md); stats-history tier corruption on reset; `RepairQueue` NaN on `max_hits=0` |
| 12 | **Visualization & support/API-fork** | `visualization.rs` (1485), `visualize.rs`, `ui.rs`, support crates, `screeps-game-api` fork visuals | Debug UI panels, map visuals, crate composition | `visualization.rs` 1485 | oversized single file; `visualization.rs:1016` `.unwrap()`; per-tick full relayout; MapVisual serialization limits |

---

## 5. Review Dimensions

Apply **both** axes to every subsystem touched.

### (a) Screeps-domain axis
- **Tick-safety / no-panic hot paths** — flag every `unwrap`/`expect`/`assert`/`panic!`/array-index/unchecked-arithmetic reachable from `pre_run`/`run`/serialize. A single panic halts **all remaining systems that tick** (the macro runs systems sequentially; the panic hook catches only the first). State is left partial.
- **CPU budget & per-tick bounds (execution + intents)** — identify unbudgeted or super-linear per-tick work (matching loops, **pathfinding**, BFS, cost-matrix rebuilds, `game::creeps()`/`getRoomStatus` calls). **Action intents charge CPU too** — a model counting only Rust execution under-measures. Is there a guard before expensive expansion (remote mining, reserving, claiming), and — critically — **any global CPU governor / circuit-breaker** to prevent the pathfinding death-spiral (Field Report C)? Flag anything that can drain the bucket into a restart-loop.
- **Single-threaded execution** — no parallelism within a tick. Flag any reliance on OS threads/locks/atomics-for-parallelism as inappropriate. (Single-threaded cooperative `async` on a custom executor is *allowed but optional* — only as part of a runtime-model change; see §10/§12.) CPU wins come from algorithms, caching, and bucket-aware scheduling.
- **Segment 50 KiB & migration safety** — does any write path silently drop data when a segment exceeds 50 KiB? Is backward-compat (`#[serde(default)]`) actually upheld for every serialized type? Is there any version marker or migration path?
- **VM-reset resilience / idempotency** — is every tick treated as a possible cold start? Is any correctness-critical state held only in `thread_local`/`static`/ephemeral resources (e.g. `BoostQueue`, `CpuHistory`, `EconomySnapshot`) where loss degrades behavior?

### (b) Software-engineering axis
- **Cohesion / coupling** — do shared `World` resources (`TransferQueue`, `SpawnQueue`, `cleanup_queue`, `ENVIRONMENT` thread_local) create hidden ordering dependencies?
- **Abstraction boundaries & layering** — does operation→mission→job hold, or do layers reach across (jobs mutating `SquadContext`; missions assuming generator flush order)?
- **Error handling** — is `Result`/`Option`/log-and-continue discipline consistent, or do paths swallow errors with no telemetry?
- **Duplication** — repeated constants (`TTL=1200`, RCL gates), copy-pasted system boilerplate (`PreRun*`/`Run*`), parallel-vector bookkeeping.
- **Oversized files / complexity** — files >800 lines (`transfersystem.rs` 2439, `attack_mission.rs` 2040, `visualization.rs` 1485, `war.rs` 1444, `squad.rs` 1021, `squad_combat.rs` 1001, `claim.rs` 921, `room/data.rs` 899) — assess decomposition.
- **Testability / coverage & evolvability** — **zero unit tests exist anywhere in the crate or support crates**, and the code changes *rapidly*, so any strategy must balance correctness with iteration speed. Assess what is most dangerous to leave untested, what is feasibly testable *today* (serialization round-trips, formation geometry, threat classification, transfer matching, spawn ordering, body calc), and **how to decouple game-API side-effects from pure decision logic** so logic is testable offline. Concrete strategy → §12.

---

## 6. Per-Subsystem Focus Areas

### 1. Tick orchestration & ECS core
*The foundation: dispatcher ordering, entity-integrity repair, and (de)serialize gate every other subsystem's correctness.*
- If segments are active when `gates_ready()` returns true but deactivate before `deserialize_world` reads them, what triggers the documented panic (`game_loop.rs:496`)? Is there a re-check before use?
- `repair_entity_integrity` scans all components in 5 phases (168–369). Can phase 1 (mutating `RoomData.missions` while iterating) double-remove or miss a ref that also appears in a `MissionData.children` list? Could the 5 passes be unified?
- Does `EntityCleanupSystem`'s topological sort (`cleanup.rs:228–247`) assume mission ownership is a DAG? Is circular ownership (A owns B, B owns A) possible, leaving deletion order undefined? What happens past `MAX_CASCADE_ITERATIONS=200`?
- `CleanupCreepsSystem` only checks `creep.hits()==0` — if the game despawns a creep that still has hits, does the stale entity linger? Should it also check `game::creeps()` membership?
- `cleanup_memory()` calls `game::creeps()` (O(creeps) network) every tick — CPU-budgeted, or a per-tick overrun source?
- If a system panics mid-`for_each_system`, subsequent systems don't run and state is partial — is there any containment (per-system panic isolation), or is the whole tick lost?
- **Field Report E (ECS = dangling-ref bug farm):** `repair_entity_integrity` exists *because* `Entity` refs routinely dangle. Step back: is specs/ECS the right backbone, or do the dangling-ref hazards outweigh its serialization/decoupling benefits? For the rewrite, evaluate alternatives — typed **generational handles** with validate-on-access, an **arena/store keyed by stable game IDs** (room name, creep id) instead of ECS `Entity`, or an owned-tree model — and what each costs (§12).

### 2. Memory & serialization
*Loss of segment state = full colony rebuild; survival-critical contract.*
- Does `repair_entity_integrity` cover **all** serializable components with entity refs, or only RoomData/MissionData/OperationData/SquadContext? Confirm JobData/CreepSpawning/CreepOwner/VisibilityQueueData/RoomThreatData truly carry no entity refs (make this explicit, not assumed).
- **Segment 55 collision:** the last ECS data chunk and the cost-matrix cache both target segment 55 (`COMPONENT_SEGMENTS` ends at 55; `COST_MATRIX_SEGMENT`=55). What prevents ECS growth from clobbering the cost matrix? Is there any assertion or reservation?
- When total serialized size exceeds 6×50 KiB, the error is logged but serialization proceeds (`game_loop.rs:445–449`) — confirm this is silent partial data loss; assess detection/telemetry (segment-fullness watermark).
- Is backward-compat enforced anywhere, or purely by `AGENTS.md` convention? Is there any test deserializing an old snapshot? What is the migration story if the format must change (no version header)?
- `from_utf8_unchecked` (441): base64 is ASCII so safe, but is there an invariant/test guarding the chunking boundary against splitting a multibyte sequence?
- Segment 60 (planner) is cleared on `room_plan_reset` but is **not** in `COMPONENT_SEGMENTS` — should it be formalized as a `SegmentRequirement`?
- **Field Report D (serialization brittle) + Rewrite Mandate:** this subsystem has **repeatedly broken** and the entity-ref mapping is fragile. Beyond fixing specifics, **evaluate replacing the approach**: explicit hand-written (de)serialization with **versioned schemas**; a schema-evolving binary format (flatbuffers/capnp/protobuf-style) vs. bincode's positional fragility; or persisting **stable game IDs** instead of ECS `Entity` indices to delete the repair pass entirely. With back-compat lifted, what is the most robust design, and what invariants/tests (round-trip, fuzz, old-snapshot corpus) catch breakage *before* deploy? (§12)

### 3. Operations (campaigns)
*Highest-churn area; `war.rs` is the largest, least-settled file.*
- `attack.rs:615` — `system_data.room_data.get(room_entity.unwrap()).unwrap()` in Recon. **Currently guarded** by the `have_live_intel` computation just above (`:608–612`, same tick / same `system_data`), so it is *latent*, not an active panic — but the guard is implicit and load-bearing. Harden with `if let`/`and_then` + skip, and confirm no path reaches `:615` without the guard.
- `war.rs` maintains `active_attack_entities[]` / `active_attack_rooms[]` in parallel with manual sync (`add_/remove_active_attack`, `cleanup_dead_attacks`). What guarantees alignment, and what happens on divergence beyond the logged warn?
- `war.rs` cadences `DEFENSE/OFFENSE/RECOMPUTE` are all hardcoded to 1 (every tick) despite comments suggesting 1–2 / 10–20 (`:1314–1316`). Intentional? Is per-tick threat scan + greedy `reassign_home_rooms` (O(A·H), `:1097–1241`) CPU-safe at scale? `is_attacking_room()` does an O(n) linear scan per scored candidate (`:1246`) — should it be a `HashSet`?
- `claim.rs` scores candidates by **linear** room distance, not route distance — can it claim rooms that are linearly close but route-unreachable or behind hostiles? Does `VISIBILITY_TIMEOUT` (20,000 ticks, `:93`) ever leave a room stuck in Scouting if the scout dies before servicing?
- If `max_concurrent_attacks` drops (room abandonment) below active attacks, are orphaned `AttackOperation`s ever cleaned up, or do they run forever? Does `should_abort()` poll route-reachability (rampart built mid-campaign → `u32::MAX`)?
- `operationsystem.rs`: `PreRunOperationSystem`/`RunOperationSystem` duplicate `system_data` construction (`:105–119`, `:140–154`) — extractable?

### 4. Economy & infrastructure missions
*Self-sufficiency engine; correctness here = sustained growth.*
- `LocalSupplyMission::ensure_children` may recreate a `SourceMiningMission` if the source still exists but the child returned Success — what prevents infinite recreation of a "stuck-completed" source? Is there a do-not-respawn flag?
- `SupplyStructureCache` (`Rc<RefCell<Option<…>>>`, refreshed every 10+ ticks): if a container/link is destroyed between refreshes, can missions spawn creeps for nonexistent targets? Where is source/target existence re-validated before spawning?
- `EconomySnapshot` reserve = `stored_energy/5` — in early game this over-reserves (floor of ~5k dominates low storage); is the threshold ever scaled by RCL/economy?
- Transfer generators are registered per-mission in `pre_run`; some register multiple (labs `unload` generator) — can generators conflict or double-count? Is flush ordering across jobs/missions defined?
- `compute_nearest_spawn_distances` runs `pathfinder::search` per source/mineral/spawn — is the result cache invalidated when the cost matrix changes? Worst-case CPU on roadless rooms?
- Wall-repair thresholds (`EMERGENCY_WALL_HITS`=100k, `MODERATE`=1M) are static — during a prolonged siege, can ramparts fall below emergency before tower-priority boost spawns repairers?

### 5. Combat & expansion missions
*Critical to offense/defense; least-settled. **Operator Field Reports A & B live here:** squads scatter instead of forming quads, and operation/mission lifecycles hang — the operator's least-trusted system. Prioritize cohesion and stuck-lifecycle root mechanisms over line-level nits.*
- **CRITICAL to assess:** `SquadMember.entity` refs are *not* repaired before serialization (unlike `Mission::repair_entity_refs`). After a creep dies and its entity is cleaned, does `creep_owner.get(member.entity)` silently return None and break heal/retreat logic? Locate and assess the fix (extend repair to `SquadContext.members`).
- `attack_mission.rs:1917` — confirmed `.expect("AttackMission must have at least one entity reference")` in `get_room()`. When (no home rooms + no owner + no squad entities) can this fire mid-tick? Should it return a fallback instead of panicking?
- TickOrders are ephemeral (skipped in serialization) but must be repopulated before jobs run — if `MissionExecutionSystem` skips/short-circuits a mission's `tick()`, do jobs run on stale/None orders? Is there a guard?
- `force_plan` is a `Vec` mutated during Exploiting (push haulers/guards) while `squads` hold `plan_index` — what guarantees index safety if `force_plan.len()` changes mid-access?
- Formation `squad_is_cohesive` skips the offset check entirely once `strict_hold_ticks ≥ STRICT_HOLD_MAX_TICKS` (15) — does this let a dispersed squad (2+ tiles apart) enter combat, masking pathfinding failures? Does any mission ever reset `desired_formation_mode` back to Strict?
- `TTL=1200` and `RoomCoordinate::new(25).unwrap()` (×7) are duplicated across `squad_*` missions and `attack_mission.rs` — extract to constants? (The coordinate unwraps are provably safe since 25∈[0,49], but are a copy-paste panic vector.)
- **Field Report A — squads don't form quads (cohesion):** trace desired-formation → per-creep move targets → actual movement. Where does cohesion break — formation-offset computation (`formation.rs`), move-intent generation, rover collision/shove undoing the formation, or a rally/grouping phase that releases too early? Does the squad **wait for all members in range** before advancing/engaging? Quantify how often members are out-of-range in the Loose state. Propose a model that holds (lead-follower with hard in-range wait-gates, or single-position group movement).
- **Field Report B — lifecycle hangs:** map the operation→mission→squad lifecycle for a war campaign end-to-end; enumerate every state and its exit condition; find states with **no exit** (target gone, all members dead, room unreachable, spawn starved) where the campaign neither progresses nor tears down. Is there a watchdog/timeout to force-abort a stuck operation?

### 6. Jobs layer
*Where creep autonomy lives; the partial-haul bug originates here.*
- **Partial-haul abandon:** in `haulbehavior.rs:513–567` (`tick_delivery`), if the TRANSFER action is consumed or the creep fills before all deposit tickets process, are remaining tickets orphaned in place (`tickets.remove(0)` + early break)? Confirm the exact trigger (push-out vs. target-invalid vs. creep-full) and whether the deposit list is pruned.
- `staticmine.rs:201` — confirmed `container_target.resolve().unwrap()`. If the container disappears after the transition to Harvest, this panics next tick. Should be `if let … else Wait`.
- Are `SimultaneousActionFlags` guaranteed reset to UNSET before every `RunJobSystem` invocation? If `run_state_machine` ticks twice in one loop (Idle→Pickup→Delivery), do both register-pickup and register-deposit fire, double-consuming flags/double-registering tickets?
- When `RemoteObjectId::resolve()` returns None mid-task, what is the contract — wait, retry, or self-deassign? Are there jobs that infinite-loop (None→None) or hang in Idle when a room is permanently hostile/invisible (e.g. `or_else(|| Some(State::wait(5)))` loops)?
- `STUCK_REPORT_THRESHOLD=10`: do all job states call `check_movement_failure`, or can Wait/Idle states never detect stuck and never recover?
- The `get_used_capacity` double-count workaround (manual store-type summation; lines 28/80/168/388/479) — if the underlying API bug is fixed, does this now *under*-report free capacity and misplan hauls?
- **Field Report F (FSM friction):** the `screeps-machine` job FSM is useful but **inflexible / hard to follow** (`MAX_STATE_TRANSITIONS=20`/tick; multi-transition-per-tick semantics). Is the multi-transition model the source of the double-fire hazards above? For the rewrite, weigh alternatives — **behavior trees**, **utility AI**, or a declarative/data-driven FSM — for jobs *and* squads, trading off debuggability and CPU (§12).

### 7. Military core
*Reusable squad/threat/economy toolkit underpinning all combat.*
- Once `advance_squad_virtual_position` times out of Strict and switches to Loose (`formation.rs ~314`), does any mission ever reset back to Strict, or is the squad permanently Loose?
- `compute_heal_assignments` (`squad.rs:515–661`) greedily assigns healers but never un-assigns — if the target list changes (member dies) between calls, can healers stay assigned to dead targets in `tick_orders`? Cost is O(H·T·H); is there a CPU guard for large (hauler) squads?
- Threat classification (`classify_threat`, `threatmap.rs:162–212`) triggers Siege on *any* boosted creep with a hardcoded 4.0× multiplier (assumes T3) — can a weakly-boosted raid be misclassified as Siege, causing over-investment? Two unboosted 150-DPS creeps (300 DPS, no heal) classify as Raid not Siege — intended? Is the 500-tick stale TTL adequate (threat leaves and returns)?
- `SquadContext` serializes all members — is there any bound on member count before the serialized size threatens segment limits (large hauler squads)?
- `BoostQueue` is ephemeral (not serialized) — boost requests in-flight are lost on VM reset; does this stall military ops, and is re-request automatic?
- Body definitions (`bodies.rs`, 27 hand-tuned variants) maintain 1:1 MOVE ratios — brittle to any fatigue/boost model change; is resulting creep speed validated anywhere?

### 8. Room data, visibility & planning
*Base layout drives defense and expansion; planning is multi-tick and CPU-gated.*
- `visibilitysystem.rs:362–363` double-`unwrap` on the singleton entity — can it be deleted between the `is_none` check (`~350`) and the second unwrap? Make defensive with `.get()`.
- `best_unclaimed_for()` uses `partial_cmp` on f32 priorities falling back to `Equal` — could a single NaN priority deadlock scout assignment? Where do priorities originate, and can they be NaN? (Also `VisibilityEntry::default()` hardcodes `RoomName::new("E0N0").unwrap()` — can default entries leak into persistence/comparison?)
- `gather_candidate_rooms` BFS (`gather.rs:164–204`) has **no CPU budget** and calls `Game.map.getRoomStatus` (via the new `room_status_cache`) per room — can a large `max_distance` or slow API spike CPU in one tick? Is `RoomStatusCache` inserted as a resource early enough that `gather.rs` sees it on first run?
- Segment-60 planner state: when `encode_to_string` fails (`roomplansystem.rs ~443`), state is silently dropped and `is_complete` flips true, restarting planning — intended recovery, or can a fingerprint mismatch (`~201`) cause indefinite restart-thrash? Add a restart-attempt counter?
- `RoomData` lazy caches expire on `game::time() != last_updated` — if `game::time()` stalls across a reset, can a cache become permanently stale? Should it also detect tick discontinuity?
- Is monolithic `RoomData` (899 lines, 5 RefCell caches + dual visibility datasets + 23 structure vecs) a decomposition candidate?

### 9. Pathing & movement
*CPU is the binding resource here; movement gates every creep action. **Field Report C lives here:** long pathfinding spikes CPU → bucket exhaustion → tick-restart death-spiral → colony collapse (extinction-level). **Field Report G:** single-creep routing seems acceptable (if under-tested) — bound CPU here rather than chasing routing correctness; the acute failure is squad coordination (§6.5/§6.7).*
- On a failed stuck-repath (`movementsystem.rs ~900`, screeps-rover), the creep keeps its old path — when stuck escalation has *changed* (now avoiding all creeps), is retaining the stale path correct? Should a failed repath increment the (currently uncapped) `repath_count` to avoid infinite repath?
- `should_repath_no_progress` keys on `ticks_no_progress ≥ 15`, but distance is sampled only when the creep *moves* — if it is wedged in place, is the distance stale such that no-progress repath never fires (deadlock)?
- The cache-eviction TODO (`costmatrixsystem.rs:67`): ephemeral creep/construction costs are cleared *every tick*. For 100 creeps across 10 rooms, is rebuilding per-room cost matrices every tick a real CPU sink vs. caching creep positions for N ticks?
- `build_local_cost_matrix` cross-room proximity (`~268–308`) does `Position::get_range_to` per creep even past the out-of-range fast path — does cost-matrix build scale O(creeps-in-room)?
- `find_route` (`screeps_impl.rs`) is unbounded — Ibex caps `pathfinding_ops` but `find_route` can bypass it; should it be time-budgeted, or is the game's internal limit trusted?
- **No job-layer stuck recovery exists** (todo.md confirms) — `check_movement_failure` only *reports*. How does a creep in a non-checking state recover? Can one stuck blocker cascade-freeze a corridor? Does tier-1b "avoid ALL friendly creeps" escalation cause a convoy-wide cost-matrix rebuild + deadlock when everyone escalates at once?
- **Field Report C — load-shedding by design (top rewrite requirement; reproduction likely infeasible):** pinpoint where pathfinding cost becomes unbounded (large `find_route`, repeated full-room cost-matrix rebuilds, many simultaneous repaths, cross-room searches). Since the spiral is emergent and won't reliably reproduce, the goal is **robustness, not a repro**: design a **hard pathfinding budget** + **bucket-aware governor** that defers/sheds expensive work as the bucket drains, plus **graceful degradation + recovery** so the colony keeps making progress instead of looping restarts. What runtime signal triggers shedding, and what early-warning telemetry surfaces it (§11)?

### 10. Transfer & market logistics
*Largest file in the crate; logistics correctness and market safety.*
- **Panic vectors:** `withdraw_resource_amount` / `link_transfer_energy_amount` (`208–289`) unconditionally `panic!` on invalid `TransferTarget` variants (Nuker withdraw; Ruin/Tombstone/Resource transfer). A mission-generator bug would halt all creeps — should these return `Result` or use type-split enums?
- Generators run **lazily on first room query**, not in a centralized collection phase — if Job A and Job B query the same room in one tick, execution order is undefined and the second sees the first's requests. Deliberate optimization or ordering hazard? Document or move to two-phase collect.
- Matching is O(rooms·targets·resources·priorities) with unbounded per-target key counts (`select_pickup` `586–675`, `select_delivery` `677–820`, `total_unfufilled_resources` `2148–2298`) — measured/bounded? At 20 rooms × 100 targets, can this blow the CPU budget?
- **Hauling is a hard assignment problem.** Provider↔consumer matching is effectively a transport/assignment optimization (NP-hard in general). Evaluate the current greedy match against established approaches — study **Overmind's logistics network** and the author's write-up **"Screeps #4: Hauling is NP-hard"** (§13) for *inspiration*. Is greedy good-enough at Ibex's scale, or is a smarter **yet CPU-bounded** assignment warranted? Cross-ref the partial-haul abandon (§6.6; `IBEX-011`) and hauler-count-not-path-derived (todo.md).
- **Market manipulation:** `can_trust_history` (`ordersystem.rs:349–351`) checks only transaction count (>100), volume (>1000), and `stddev ≤ avg×0.5` — no time-series/trend/spike detection. Can a rival spike price with a fake high-volume order, then sell into Ibex at inflation? (todo.md flags this.)
- `calc_transaction_cost_fractional` has no upper bound — across-map terminal deliveries can cost more energy than the resource is worth. Is there any prefer-local logic?
- `TransferTarget::is_valid()` (a game-API call) is never invoked inside the matching loops — can invalid targets persist in nodes after visibility changes?

### 11. Spawning, stats & cross-cutting helpers
*The spawn queue decides creep composition order and energy gating — a strategic chokepoint.*
- **Spawn ordering & energy gating — VERIFY (traced and found *correct*, contrary to first impression).** `SpawnQueue::request` (`spawnsystem.rs:85–94`) inserts with `binary_search_by(|probe| spawn_request.priority.partial_cmp(&probe.priority))`. The comparator is **non-idiomatic** — it compares the *new* request to each probe rather than probe-to-target — so it *looks* inverted, but applied consistently it keeps the vector in **descending** priority (`f32` constants `SPAWN_PRIORITY_CRITICAL=100.0 … NONE=0.0`); inserting 0, then 100, then 50 yields `[100, 50, 0]`. The forward consume loop (`:236`) therefore spawns **highest-priority first** — there is **no** inversion, and "fixing" the comparator would *introduce* one. The genuine open questions: (a) `partial_cmp(...).unwrap_or(Equal)` silently coalesces a NaN priority to Equal — can any computed priority ever be NaN? (b) the loop `break`s on the first request with `body_cost > available_energy` (`:247–248`, vs. `continue` at `:243–244` for over-capacity bodies) — intentional energy-reservation for the top request, but can it starve cheap essential creeps behind a temporarily-unaffordable high-priority one? (c) the todo.md *"body cost below min never enters queue"* item is a **body-sizing** bug in the body-calc path, **distinct** from ordering — locate and confirm.
- `SpawnRequest.cost()` is checked against `energy_capacity` (`:243`) but body length is never validated against the 50-part max at the queue boundary — can a malformed 50-part request silently fail at the API (`spawn_creep`) and be lost with no requeue?
- Renew TTL threshold derives from `next_spawn_duration_ticks`, which is 0 when all requests are satisfied → threshold=50; can this oscillate and starve renewals when a new request arrives mid-tick?
- `stats_history` tiers use `wrapping_sub` on tick counters — if the environment resets while history persists (`reset.environment` but not `reset.memory`), can the tick jump (e.g. 500→1) corrupt cascade state (spurious or skipped downsamples)?
- `RepairQueue::get_best_target` computes `current/max_hits` — NaN when `max_hits=0` (legitimate for some ramparts), making `partial_cmp` return None → undefined order. Confirm the `max_hits>0` filter fully guards this.
- Segment 99 (stats) is overwritten every tick with no version field — does a stats-schema change risk deserializing garbage on `on_load`?

### 12. Visualization & support/API-fork integration
*Runs in the tick loop and is the sole CPU sampler — and per **Field Report H** the world renderer currently **corrupts all rendering** (incl. room visuals), blocking visual debugging. Not merely cosmetic.*
- **Field Report H — renderer corrupts all rendering (high priority; blocks debugging):** enabling the world renderer breaks *all* later rendering, including room visuals — likely a **serialization/encoding fault** in the visual payload (RoomVisual/MapVisual JSON or `screeps-visual` encoding): a single malformed / oversized / NaN-bearing draw call may **poison the whole RawMemory visual buffer** for the rest of the tick/session. Reproduce by toggling the renderer; **bisect which draw call corrupts the stream**; and **compare the emitted format against the engine's** (`screeps/engine` visual handling, https://github.com/screeps/engine). Is the >16 KiB MapVisual limit, or an invalid coordinate, the trigger?
- `visualization.rs:1016` `snapshots.last().unwrap()` — guarded by a `len() ≥ 2` check in `RenderSystem` but not at the call site; assess whether the guard should move into `draw_stats_sparkline`.
- At >100 rooms, how many draw primitives / MapVisual calls are generated per tick? Is the ~16 KiB-per-MapVisual serialization limit at risk (esp. claim visuals, one call per candidate room)?
- `RenderSystem` always recomputes full layout even when `VisualizationData` is unchanged — acceptable for a debug tool, but quantify per-tick cost under heavy load (war/spam).
- Is the feature gate (`features.visualize.on` → `Option<>`-gated resources) airtight, so visualization has **zero** cost and **zero** panic surface when off? Note `CpuTrackingSystem` is the *sole* producer of CPU history — if visualization is off, is the CPU histogram simply blank, and is that acceptable?
- `visualization.rs` is 1485 lines mixing summary types, 6 systems, layout, and 4 render implementations — assess module decomposition.
- `screeps-game-api` fork: confirm divergence is minimal (visual additions only per `AGENTS.md` §7) and no fork-specific types leak into Ibex's public interfaces in a way that complicates upstreaming.

---

## 7. Known Issues & Hotspots to Validate

The **Operator Field Reports (§2, A–H) are the highest-priority items** and head the register. For each item below, reviewers must **confirm, locate (file:line), and log it as a trackable entry** (Bug & Issue Register, §9) — **flag-and-track, do not fully root-cause now** (deep-dives come later, §10). Distinguish *still-present* from *already-mitigated*.

1. **Spawn ordering & energy gating** — *investigated; ordering is CORRECT* (highest-priority-first; the `spawnsystem.rs:85–94` comparator is non-idiomatic but yields descending order — see §6.11). **Do not "fix" it into an actual inversion.** Real open items: NaN-priority coalescing, `break`-on-unaffordable starvation (`:247–248`), and the **separate** todo.md body-sizing bug ("body cost below min never enters queue"). Re-verify and assign severity.
2. **Partial hauls abandoned on damage / mid-delivery** (todo.md; `jobs/utility/haulbehavior.rs:513–567`) — haulers strand resources instead of finishing delivery.
3. **No creep stuck-detection/response at job layer** (todo.md; `pathing/movementsystem.rs`, `jobs/utility/movebehavior.rs`) — rover reports stuck; no job recovers.
4. **No lost-creep (memory-loss) recovery** (todo.md) — no rebuild path when `Memory.creeps` state is lost.
5. **No CPU guards before remote-mining/reserving/claiming** (todo.md) — expansion proceeds regardless of `Game.cpu` bucket; cross-check `war.rs` cadences and `gather.rs` BFS.
6. **Market lacks price-history analysis & hard manipulation guards** (todo.md; `transfer/ordersystem.rs:349–351`).
7. **Container miners don't anchor to container location** (todo.md) — re-acquire behavior on container build/destroy (`jobs/staticmine.rs`).
8. **Road/connectivity system absent** and **hauler/harvester part counts not derived from path distance** (todo.md).
9. **Factory & boost usage incomplete** (todo.md); **`BoostQueue` ephemeral**, lost on reset (`military/boostqueue.rs`).
10. **Deserialization failure unrecoverable** (`game_loop.rs:508/533`; `AGENTS.md` §6) — no telemetry, no graceful degradation; **segment-55 ECS/cost-matrix collision** risk.
11. **Panic vectors in hot paths** — `transfersystem.rs:208–289` (`panic!` on invalid `TransferTarget`); `staticmine.rs:201` (`resolve().unwrap()` — container can vanish mid-task, *reachable*); `attack_mission.rs:1917` (`.expect` last-resort); `visibilitysystem.rs:362–363`; `visualization.rs:1016`; `RepairQueue` NaN compare. (`attack.rs:615` double-`unwrap` is currently **guarded** — latent only; see §6.3.)
12. **Oversized files** — `transfersystem.rs` (2439), `attack_mission.rs` (2040), `visualization.rs` (1485), `war.rs` (1444), `squad.rs` (1021), `squad_combat.rs` (1001), `claim.rs` (921), `room/data.rs` (899).
13. **In-flight military/war churn** — recent commits (war system, attack missions, squad formation/renew/group-up, military economy) plus uncommitted edits to `operations/{war,claim,miningoutpost,operationsystem}.rs`, `pathing/movementsystem.rs`, `room/{gather,mod,roomplansystem}.rs`, new `room/room_status_cache.rs`, and `screeps-foreman`/`screeps-rover` submodules. **Least-settled** — weight findings toward design-level guidance over line-level nits.
14. **Zero automated test coverage** across the entire crate and support crates.

---

## 8. Strategic & Competitive Evaluation

Beyond correctness: **does the design win the game?** Ground each judgment in a real subsystem and deliver a verdict.

- **Economy efficiency** — Does the colony reach self-sufficiency and scale across rooms? Spawn priorities are *static* constants (CRITICAL/HIGH/MEDIUM/LOW/NONE) — is static prioritization expressive enough, or is demand-driven weighting needed? (The ordering itself is correct — see §6.11.) Hauler counts are *not* path-derived (todo.md). Is there meta-room resource allocation (a mineral-sink room importing energy), or only per-room optimization? `EconomySnapshot` reserve = `stored_energy/5` — does it cap growth in early game?
- **Expansion / claim logic** — Is `claim.rs` candidate scoring (linear distance + source/walkability/plan weights) good enough to pick *winning* rooms and avoid dangerous/unreachable ones (route-blind)? Is scouting throughput adequate (`MAX_SCOUT_MISSIONS=3`, global visibility queue) or does it starve under 100+ requests?
- **Military doctrine (war/squad)** — Is offense economically gated to avoid waves on unwinnable targets (towers/safemode/repair-rate), or does it spam up to `max_waves`? Is defense proactive or purely reactive (wait-for-hostiles)? Are operations parallel (multi-room) or serial (one attack/defense at a time)? Does the idealized 2×2 box formation survive real terrain/walls, or does the 15-tick Loose fallback mask pathfinding failure?
- **Defense** — Is there a survival fallback (retreat-and-rebuild) if walls fall, or does a breach cascade? Are tower energy, wall-repair escalation (100k/1M static thresholds), and safe-mode triggers correctly prioritized under siege? Can a per-room under-siege boost its own tower/spawn priority above peaceful peers?
- **Market / logistics** — Can the bot be out-traded by an opponent watching price trends? Does it prefer-local to avoid energy-negative terminal trades? Does it react to scarcity by paying premiums for critical resources, or treat all requests equally?
- **CPU as the binding resource (the existential one)** — Field Report C shows CPU exhaustion can **kill the colony** via a restart death-spiral. Where does per-tick CPU blow the budget first (pathfinding, cost-matrices, transfer matching, war threat scans, gather BFS, `game::creeps()`/`getRoomStatus`)? Remember **intents charge CPU** too. Is there **any** global CPU governor/circuit-breaker, or only local budgets? A bot that can permanently die to its own CPU use is **strategically unviable** however well it plays when healthy — treat a governor as table-stakes for the rewrite.

**Deliver a competitive verdict:** where Ibex is competitive, where a well-optimized rival out-farms/out-fights it, and the **3–5 highest-leverage strategic gaps** to close. **Benchmark doctrine against a top-tier open-source bot — Overmind** (TypeScript; §13) — for *inspiration on ideas* (Overlord/Directive structure, the logistics network, combat/swarm cohesion relevant to Field Report A), **not** for copying.

---

## 9. Required Deliverables & Output Format

The review must emit the following. **Every finding uses the exact block below** (uniform: severity + file:line + evidence + recommendation).

**1. Prioritized findings list** — each as:
```
ID:             <stable id, e.g. IBEX-NN — tracks into the §9 Bug & Issue Register and later deep-dives>
[SEVERITY] <one-line title>
Subsystem:      <name from §4, or "cross-cutting">
Location:       <file:line(s)>   (exact; if a line shifted, locate the construct by name)
Type:           Correctness | Tick-safety | CPU | Persistence/Migration | Architecture | Strategy | Maintainability | Test-gap
Status:         Observed-fact | Hypothesis (confidence: H/M/L)
Evidence:       <code reference / concrete reasoning — quote the load-bearing line>
Impact:         <gameplay/operational consequence>
Recommendation: <concrete fix, or the exact check to run if Hypothesis>
Breaking-change?:  None | Memory/format | Behavioral   (breaking is ACCEPTABLE per the Rewrite Mandate — just label it for sequencing; never break the running bot in an interim quick-win)
Rewrite-implication: keep-as-is | refactor | replace   (+ the alternative to consider, if any)
```

**Severity rubric:**
- **Critical** — panic reachable in normal play, data loss, or a strategy-breaking bug active in normal operation (e.g. a reachable hot-path `panic!`/`unwrap`, deserialization failure, or segment overflow that silently drops state).
- **High** — correctness/CPU issue likely to manifest under load or expansion.
- **Medium** — latent edge case, or significant maintainability/architecture debt.
- **Low** — nit, style, cosmetic.

**2. Per-subsystem health assessment** (all 12) — 2–4 sentences + maturity scores (see #6 below) + the single biggest risk.

**3. Cross-cutting architectural findings** — layering/coupling, dispatch ordering, persistence contract, panic-surface, CPU-governance, test strategy — issues spanning subsystems.

**4. Quick-wins vs. deep-refactors** — two lists.
- *Quick-wins* = localized, low-risk, high-value that **don't break the running bot** (replace a reachable hot-path `unwrap` like `staticmine.rs:201`; add a pathfinding CPU cap / governor stub; fix a NaN comparator; add a clarifying comment + unit test to spawn ordering so it isn't re-flagged). **Do not** "reverse the spawn comparator" — it is correct (§6.11).
- *Deep-refactors* = structural (decompose `transfersystem.rs`/`attack_mission.rs`/`visualization.rs`; two-phase transfer matching; global CPU governor; extend entity-ref repair to `SquadMember`).

**5. Risk register** — table: `Risk | Subsystem | Likelihood | Impact | Trigger condition | Mitigation`. Lead with **survival/extinction risks**: the **CPU pathfinding death-spiral (Field Report C)**, deserialization failure / state loss, segment overflow, and reachable hot-path panics.

**6. Maturity/score rubric** — score each subsystem 1–5 on **Correctness**, **Robustness (tick-safety/reset)**, **Performance/CPU**, **Maintainability**, **Strategic fitness**; present as one table for at-a-glance comparison.

**7. Bug & Issue Register** — a trackable table of all suspected bugs for **later individual deep-dives**: `ID | Title | Subsystem | Location | Symptom / observed impact | Status (suspected/confirmed) | Suggested validation (repro / test / log to add)`. **Do not root-cause here** — capture just enough to pick each up later. Operator Field Reports A–H seed the top rows.

**8. Rewrite direction & architectural alternatives** — for each brittle pillar (serialization, ECS/entity model, job/squad behavior modeling, CPU governance), a short ADR-style entry: *current approach → pain → 1–3 alternatives → trade-offs → recommendation* **+ an incremental migration path** (what to replace first, the stable seam to hide it behind, how to validate behavior before/after each step). The rewrite is **incremental** and back-compat is **not** a constraint (Rewrite Mandate). Feeds the project plan.

**9. Observability & self-improvement plan** — concrete telemetry to add (what to emit to console vs. RawMemory segments), how to measure CPU **including intents**, death-spiral early-warning signals, and an offline feedback loop maintainers/agents can use to detect regressions and drive recursive improvement (detail in §11).

**Lead the report with an executive summary:** a verdict on each **Operator Field Report (A–H)** with a root-mechanism hypothesis; the top 5 must-fix items (severity + one-liner); the 3 most fragile subsystems; the **single biggest rewrite recommendation**; and the overall competitive verdict — explicitly stating whether the **CPU death-spiral** makes the current bot non-viable as-is.

---

## 10. Constraints & Guardrails for Reviewers

- **Read-only.** Propose; do not modify code. No PRs, no edits.
- **Cite `file:line`** for every concrete claim. Prefer evidence over assertion; quote the load-bearing line.
- **Rewrite mandate, not a compatibility cage.** A full rewrite is planned and **backward compatibility is not required** — recommend the right design even if it breaks serialization/memory or replaces a subsystem wholesale. Still **label** breaking changes for sequencing, and **never break the currently-running bot** in an interim quick-win. Don't dilute a recommendation to preserve compat.
- **Flag-and-track, don't root-cause.** When you find a likely bug, log it in the Bug & Issue Register (§9) with a validation idea and move on — individual deep-dives happen later. Correct breadth and prioritization now beat exhaustive diagnosis.
- **Single-threaded, WASM — no *parallelism*.** Don't propose OS threads, multi-threaded executors, locks, or atomics-for-parallelism — there is no parallelism within a tick. **Single-threaded cooperative `async`/await on a custom executor is permissible** and may be worth considering **if** the rewrite moves off ECS to a different runtime model — but weigh the **complexity cost** (easy to over-engineer) against a simpler explicit scheduler. CPU wins still come from algorithms, caching, and bucket-aware scheduling.
- **Measure CPU as execution + intents.** Any performance claim must acknowledge that action intents charge CPU; verify against the open-source engine (§13) when exact costs matter.
- **Distinguish observed-fact from hypothesis.** The *existence* of the `staticmine.rs:201` `.unwrap()`, `attack_mission.rs:1917` `.expect`, and `attack.rs:615` double-`unwrap` is verified against code (Observed-fact); whether each can actually *fire* is the open question — `staticmine.rs:201` looks genuinely reachable (container destroyed mid-task), whereas `attack.rs:615` is currently *guarded* by the `have_live_intel` check above it (latent only). The earlier **"reversed spawn-priority" reading was investigated and found incorrect** — the ordering is correct (§6.11); treat any inversion claim as refuted unless a fresh trace proves otherwise. The partial-haul *trigger* and segment-55 *collision likelihood* remain unconfirmed — label Hypothesis with confidence + a concrete check until verified.
- **Mind the churn.** The military/war area (`war.rs`, `attack_mission.rs`, `squad*`, `military/*`), the uncommitted edits, and the submodule changes are **new and least-settled** — favor design-level guidance; avoid over-indexing on line-level nits that may already be in flux. Treat `screeps-foreman`/`screeps-rover` at their Ibex integration boundary only.
- **No invention.** Ground every finding in a real file/type/risk from the repo. If a referenced line has shifted, locate the construct by name rather than guessing.
- **CPU and tick-safety are first-class.** A maintainability nit and a hot-path panic are not equal — weight severity by gameplay/survival impact.

---

## 11. Recursive Self-Improvement & Observability

A core goal beyond bug-finding: make Ibex **measurable and self-improving**, so regressions are caught *before* and *after* deploy and the maintainers (and agentic tooling) can drive iterative gains. Assess the current state and design the target.

**Telemetry channels available**
- **Console output** — readable via the Screeps **API** (console stream). Use for structured **events, errors, and counters** (e.g. one JSON line per significant event). Cheap to emit but volume-limited — budget it.
- **RawMemory segments** — readable out-of-band via the API; ideal for **periodic metric snapshots**: CPU used **and intent count**, bucket level, GCL/RCL, energy throughput, creep counts by role, active operations/missions, threat levels, death-spiral signals. The bot already serializes to segments — add a dedicated **metrics segment**.
- **Existing stats** — `statssystem` / `stats_history` and the `screeps-plus-stats` integration are a starting point; evaluate a screepspl.us / Grafana dashboard.

**What to design / recommend**
- **CPU accounting that includes intents** (Field Report C is a CPU problem): per-system CPU + intent attribution so the worst offenders are visible. **Death-spiral early-warning**: bucket trend, ticks-since-progress, repath storms, restart counter — used both as a **runtime trigger for load-shedding** (shed/defer work when signals fire) and as post-hoc diagnostics.
- **Structured error/event log** (console and/or segment) with severity, so panics, deser failures, stuck operations (Field Report B), and out-of-range squads (Field Report A) surface as **data**, not silent degradation.
- **Offline feedback loop / harness**: pull console + segment telemetry → compute health metrics → diff vs. a baseline → flag regressions → prioritize → redeploy → re-measure. This is the substrate for **recursive self-improvement**, including agent-driven analysis (an LLM/agent reading telemetry + the Bug & Issue Register to propose the next change).
- **Fast local server (screeps-launcher) as the eval substrate**: a Dockerized private server runs **much faster than MMO** and is fully scriptable; the repo already targets `private-server` (`127.0.0.1:21025`) and deploys via `js_tools/deploy.js` + `screeps-api`. Automate **startup → deploy → telemetry capture → scoring → compare** to close the loop offline and run repeatable scenarios (economy / contested / war). Design: `docs/design/0006-eval-and-iteration-harness.md`.
- **Pre-deploy validation gates** (tie to §12): block deploy if serialization round-trip, key unit tests, or a sim smoke-run fail.
- **A reproducible "colony health" score** (survival, CPU headroom, energy/GCL growth, military win-rate) — the **objective function the rewrite optimizes** and self-improvement tracks over time.

---

## 12. Recommended Tooling, Techniques & Practices

Concrete recommendations to raise design quality, performance, and reliability — and to make the rewrite tractable. The review should evaluate, prioritize, and extend these.

**Testing & offline validation** (zero tests today; code changes fast — balance correctness with iteration speed)
- **Pure-logic unit tests first**: deterministic, high-value, stable modules — serialization round-trips, formation geometry, threat classification, transfer matching, spawn ordering, body/composition calc, room-plan scoring. High ROI, low churn.
- **Decouple side-effects from decisions**: a thin **world-model / game-API abstraction** so decision logic runs against in-memory fixtures without the live game. The single biggest enabler of both testing *and* a cleaner rewrite.
- **Simulation / replay harness + fast local server**: a Dockerized **screeps-launcher** private server (much faster than MMO; the repo already targets `private-server` via `js_tools/deploy.js` + `screeps-api`) for scripted integration/eval runs — automate startup → deploy → telemetry → scoring → compare (design: `docs/design/0006-eval-and-iteration-harness.md`). Also record real state from a run and replay offline to reproduce bugs deterministically; `screeps-server-mockup` for lighter in-process tests.
- **Property-based & fuzz tests** for the serializer (round-trip arbitrary state; fuzz old-snapshot decode); **golden-file tests** for the room planner.
- **Keep fast-iterating strategy code behind thin, tested cores**: test the stable kernel, leave the experimental shell flexible.

**Architecture alternatives to evaluate** (back-compat lifted — Rewrite Mandate)
- **Entity model (Field Report E):** typed **generational handles** with validate-on-access, or an **arena/store keyed by stable game IDs** (creep name/id, room name) instead of ECS `Entity` indices — to eliminate dangling refs and the per-tick repair pass. Decide whether specs/ECS stays at all.
- **Serialization (Field Report D):** versioned, explicit (de)serialization; schema-evolving formats (flatbuffers/capnp/protobuf) vs. positional bincode; persist stable IDs, not entity indices; a version header + migration path.
- **Behavior modeling (Field Report F):** behavior trees / utility AI / data-driven FSMs for jobs and squads vs. the current `screeps-machine` FSM — for clarity and flexibility.
- **Squad cohesion (Field Reports A/G):** explicit lead-follower with hard in-range wait-gates, or single-"fat-position" group movement, with cohesion as an invariant.
- **Runtime / scheduling model:** if moving off specs/ECS, choose the execution model deliberately — an **explicit ordered scheduler** (simple, debuggable) vs. a **single-threaded cooperative `async` executor** (enables pausing/resuming long work across ticks, but adds real WASM/Rust complexity). Pick the **simplest** model that supports **load-shedding** and **resumable work**; don't adopt `async` for its own sake.
- **Prior art — study, don't copy:** mine **Overmind** (TypeScript; §13) for how a top-tier bot structures Overlords/Directives, logistics, and combat cohesion (Field Report A), and the rustyscreeps ecosystem (`screeps-game-api`, `screeps-starter-rust`) for Rust/WASM patterns. Extract *ideas and pitfalls*, then design Ibex's own — licenses and language differ; do not lift code.

**Performance & reliability**
- **Global CPU governor / circuit-breaker** (Field Report C — table-stakes): hard pathfinding budget, bucket-aware scheduling (defer non-essential systems when the bucket is low), graceful degradation instead of restart-spiral.
- **Path & cost-matrix caching**: cache/share paths, persist cost matrices across ticks, incremental updates instead of per-tick rebuilds.
- **Profiling**: `screeps-timing` + the `profile` feature, flamegraphs, per-system budgets — and **account for intents**, not just execution.
- **Crash containment**: per-system panic isolation so one system's failure doesn't void the whole tick.

**Process & engineering practices**
- **ADRs** for the rewrite's pillar decisions (entity model, serialization, behavior modeling, CPU governance) — feeds the project plan.
- **Feature flags + canary** on a private server before MMO; **deterministic seeds** and a **replay corpus** from production for repeatable validation.
- **Ground-truth from the open-source engine** (https://github.com/screeps/engine) for exact intent/CPU costs, pathfinder internals, and mechanics — don't guess where the source is authoritative.
- **WASM/Rust hygiene**: mind `panic = "abort"` semantics, WASM binary size, and the bucket cost of large allocations; profile allocation and consider arena patterns if it shows up.

---

## 13. Reference Material

- **`AGENTS.md`** (repo root) — authoritative: domain, tick flow (§4), key abstractions (§5), **memory/serialization & migration contract (§6)**, screeps-game-api fork policy (§7), safety/CPU rules (§8), and the "where to look" table (§10). Read before judging.
- **`todo.md`** (repo root) — the maintainer's backlog; source for the §7 Known-Issues list.
- **Screeps docs** — https://docs.screeps.com/ and https://docs.screeps.com/api/ — for game-rule grounding (CPU/bucket, RawMemory segments, intents, structure mechanics, market).
- **Screeps engine source (open-source)** — org: https://github.com/screeps ; core engine: https://github.com/screeps/engine (also `screeps/driver`, `screeps/common`). **Authoritative** for exact **CPU/intent costs**, pathfinder internals, and structure/market mechanics — consult instead of guessing.
- **Overmind (top-tier reference bot, TypeScript)** — https://github.com/bencbartlett/Overmind — a mature open-source Screeps AI. Use for **inspiration on ideas, not copying** (license & language differ): Overlord/Directive/colony structure (≈ Ibex operations/missions), the logistics network, `CombatOverlords` / swarm cohesion (relevant to **Field Report A**), and overall strategy. Study *what* it does and *why*, then design Ibex's own approach. (Catalogued in `docs/references/external-references.md`.)
- **"Screeps #4: Hauling is NP-hard"** (Overmind author) — https://bencbartlett.com/blog/screeps-4-hauling-is-np-hard/ — why hauler/logistics assignment is a hard optimization and practical approaches. **Required reading for the transfer/logistics review** (§6.4, §6.6 haul, §6.10) and any logistics rewrite.
- **Telemetry endpoints** — the Screeps API exposes **console output** (stream) and **RawMemory segments** for out-of-band metric/error extraction (basis for §11). `screeps-plus-stats` (in this repo) and screepspl.us / Grafana are existing options.
- **`screeps-game-api`** — local working fork at `C:\code\screeps-game-api`; **upstream & latest docs:** https://github.com/rustyscreeps/screeps-game-api (API reference on docs.rs/screeps). Consult for the exact semantics of `resolve()`, `RawMemory`, pathfinder, store, and market APIs used in hot paths.
- **Key entry files** — `screeps-ibex/src/lib.rs` (WASM exports); `screeps-ibex/src/game_loop.rs` (tick orchestration, dispatcher order, serialize/deserialize, `repair_entity_integrity`); `screeps-ibex/src/memorysystem.rs` (segment I/O & gating); `screeps-ibex/src/serialize.rs` (entity-ref wrappers, encode/decode pipeline). Start here to orient before diving into any subsystem.
