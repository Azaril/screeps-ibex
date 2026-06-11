# Review: global/static state & `findnearest.rs` → a single pathfinding instance

> **Date:** 2026-06-11. **Trigger:** operator — "I don't love that we've had to add globals as they're not very Rust friendly… review all usage of static and see if it should just be data passed through to systems or made available as a component on the system" + "Review all usage of `findnearest.rs` — having a single pathfinding system instance would let us remove these types of functions and also allow the budget to be attached to that."
>
> **Method:** multi-agent sweep — full statics inventory (two independent passes with different search patterns), every `findnearest.rs` call site, the context-threading map, the ibex↔rover pathfinding seam; two independent migration designs (minimal-churn vs end-state-first) reconciled here; adversarial feasibility verification of every "this can be threaded" claim (call stacks traced to their dispatch origin).
>
> **Rule this encodes:** [`../guides/engineering-practices.md`](../guides/engineering-practices.md) EP-1.1–1.6. **Consumes into:** Phase 2 planning.
>
> **Status (2026-06-11, operator pulled the work forward): M0–M6 LANDED** — commits `93a0e35` (M0), `5f9fe07` (M1+M4, paired to avoid double churn on the route-cache call sites), `8dbfacb` (M2), `6a3e54e` (M3), `7a05441` (M5, incl. deleting the two dead `visualize` methods), `4552fb9`+`bbe86e0` (M6). The bot crate's mutable statics are now exactly the EP-1.1 sanctioned set (ENVIRONMENT composition root, platform trio, WARNED log-once) — zero Mutexes/atomics. 52 host tests, both lanes warning-free per slice; full-diff adversarial review confirmed behavioral parity on every load-bearing path (ordering, first-tick defaults, pool/route/selection semantics, intent pipeline, identity). **Live battery on `bbe86e0`** (all under `runs/`): smoke PASS (916 ticks, zero panics/deser, creeps 1→16, health 0.4407; seg-57 block schema-identical); pressure PASS (governor normal→conserve ON TREND at −6.6 with bucket ~9k, **pool tier-scaled in lockstep 20000→10000→20000 through the Resource path**, recovery at tick 550); panic-containment exact (vm_starts 1→2, panics_caught=1, serialize_skipped_aborted=1 through the `MetricsState` path, colony alive at 16 creeps — the survival-gate zero IS the pass). **M7 remains open** (PathEngine injection + cache persistence — Inc 6/7 gated). **Accepted residuals from the diff review:** (1) the two lazily-flushed transfer-generator refresh paths pass `None` and bypass the pool (bounded: per-search cap, ≥10-tick cadence, fires only when no mission refreshed the room's cache first — missions run before jobs; documented at `structure_data.rs`); (2) BotIdentity loses keep-last across environment rebuilds (harmless respawn-window edge, documented in `identity.rs`); (3) fault counters are now per-environment rather than per-VM lifetime (documented in `metrics.rs`; harness restart-segmentation unaffected).

## 1. Verdict summary

Every mutable static in the bot crate **can** be migrated — the feasibility pass found **no call site without a path to `World`**; nothing fires during deserialization (deser runs as a System against a live world), panic handling (handled entirely in JS), or pre-world (only the `Memory._features` boundary reads, which stay). The one site needing design care (the lazily-flushed structure-data closures) has two workable threadings — see §3. The recommendation is the **end-state direction with minimal-churn sequencing**: migrate to Resources in the order below, keeping only the composition root + platform trio + JS-boundary state (plus two minor latches: the `WARNED` log-once and the `profile`-feature `TRACE`).

| Static | Verdict | Mechanism | Slice |
|---|---|---|---|
| `cpugovernor.rs:97` `SNAPSHOT: Mutex<Option<Snapshot>>` | **MIGRATE** → `GovernorSnapshot` Resource (`Copy`; existing `Default` maps verbatim) | `tick_start(&mut World)` writes; `Read<GovernorSnapshot>` in 4 systems; a `tier: Tier` copy field on Mission/Operation/Job execution data; `can_execute_cpu` becomes a method. ~13 mechanical edits (all call sites traced — every one originates inside a system or mission/operation execution with SystemData in hand) | **M1 — first** |
| `pathbudget.rs:34-35` `POOL`/`REMAINING: AtomicU32` | **MIGRATE** → fields of `PathfinderService` (plain `u32`; CAS deleted) | see §3 | **M4 — one PR with the service** |
| `intents.rs:44,52` `COUNTS`/`DIGEST` | **MIGRATE** → `IntentRecorder` Resource | `Write<IntentRecorder>` in `JobSystemData` → `&mut` field on `JobExecutionRuntimeData` (the `transfer_queue` precedent — the "world is already borrowed" objection is **false**: jobs run against pre-fetched SystemData, not a borrowed World); 5 sink signatures + 23 squad_combat sites mechanically updated | **M2** |
| `metrics.rs:42-50` nine fault/movement counters | **MIGRATE** → fields on the existing `MetricsState` Resource | `Write<MetricsState>` added to the game_loop-local Serialize/Deserialize systems + `MovementUpdateSystem`; caveat: `run_now` doesn't auto-`setup`, so insert the resource in `create_environment` before `deserialize_world`. `Memory._metrics` seeding stays in `tick_start` | **M3** |
| `features.rs:478` `CACHED` thread_local | **MIGRATE (cache only)** → `Features` Resource; `load_reset`/`prepare` stay stateless pre-world JS reads | 31 sites: `Read<Features>` for systems, a field for execution-data; delivers the test setter ADR 0015 §4(d) names for free | M5 — opportunistic |
| `globals.rs:3` `USERNAME` thread_local | **MIGRATE** → `BotIdentity` Resource + `&str` params on the two `room/data.rs` methods | decision-bearing (friendly/hostile disposition), not cosmetic — a host test today reads `""` and misclassifies | M6 — rides any room/data touch |
| `game_loop.rs:626` `ENVIRONMENT` thread_local | **KEEP — the sanctioned exception** (EP-1.1a): the single top-level game instance. Composition root owning the `World` across JS→wasm calls; never read below `tick()` — ownership, not shared state | optional later: a `#[wasm_bindgen]`-owned `Bot` instance held by the loader (which already owns the catch→halt lifecycle) | — |
| `transfersystem.rs:181` `WARNED` once-latch | KEEP (EP-1.1c) or fold into a `MetricsState` counter when touched | log-only, no control flow | opportunistic |
| `lib.rs:7` allocator; `panic.rs:20` hook; `logging.rs` logger | **KEEP** — platform-mandated trio | — | — |
| `Memory._metrics.vm_starts` / `aborted_ticks`; `Memory._features.*` | **KEEP — boundary state** (EP-1.4): must survive VM halts (heap dies, Memory doesn't; the loader catch writes `aborted_ticks` after Rust has already thrown) or be readable pre-world (reset flags; eval injection contract) | — | — |
| `screeps-timing` `TRACE` thread_local | KEEP — `profile`-feature dev tooling, host-side analysis only | — | — |
| Dead surface: `bucket_trend()`, 3 `record_*` fns (`metrics.rs:74-86`, zero callers — superseded by the JS-boundary containment decision), 4 of 5 `PathFinderHelpers` fns, 6 of 8 `FindNearestItertools` methods | **DELETE** (~−130 LOC) | pure deletion, zero callers verified crate-wide | rides any commit |

`screeps-rover` contains **zero statics** — already at end state. Inert immutable data (`ORDERED_REPAIR_PRIORITIES` — an immutable `static` slice — and the `segments.rs` const registry) is fine.

### Why migrate at all (the cases that decide it)

1. **Host-test isolation.** Statics are process-wide; `cargo test` is multi-threaded. The exposure is real but currently contained by a one-mutating-test-per-module discipline: `can_execute_cpu_matches_legacy_formula` (`cpugovernor.rs:181`) refreshes the global snapshot and `take_drains_and_clamps` (`pathbudget.rs:95`) drains the pool — any *second* mutating test in those modules, or any test exercising a code path that draws the pool, cross-talks nondeterministically. ADR 0015 forbids CI retries at L0–L4 — statics manufacture exactly the flakes that rule outlaws, and the discipline doesn't scale with Inc-3's test-mass growth.
2. **The intents static defeats its own purpose.** Shadow-dispatch parity (P1.C5, the C3 digest's reason to exist) diffs two dispatches over the same tick — one global `DIGEST` forces snapshot/reset choreography between dispatches; as a Resource it's simply `recorder_a.digest == recorder_b.digest`.
3. **Inc-6 record/replay** needs every tick input injectable per instance; replayed ticks sharing process statics diverge from recordings with zero code change.
4. **The ceremony is dead weight.** wasm is single-threaded: the Mutex and CAS loops exist only because Rust statics demand `Sync`. Resources are plain fields.

## 2. `findnearest.rs` findings

The file is **almost entirely dead surface around one real client**:

- **Only 5 production call sites exist; only ONE pathfinds.** `jobs/utility/dismantlebehavior.rs:45` (`find_nearest_from` + `same_room_ignore_creeps_range_1`) is the sole budget-drawing caller. The other four (`haulbehavior.rs:47/116/166`, `harvestbehavior.rs:19`) are `find_nearest_linear_by` — pure Chebyshev math, no pathfinding, no budget needed.
- **Dead:** 4 of 5 `PathFinderHelpers` variants and 6 of 8 `FindNearestItertools` trait methods have zero callers crate-wide.
- All call sites are free helpers **one hop below `JobTickContext`** — threading a service handle is mechanical (callers in `jobs/dismantle.rs`, `build.rs`, `upgrade.rs`, `harvest.rs`, `haul.rs` all hold `tick_context`).
- The same-room search pattern the helpers implement also exists ad hoc: `compute_nearest_spawn_distances` (`structure_data.rs:203-246`, raw `pathfinder::search` per spawn×target) and linear-nearest duplicates (`squad_combat.rs:558/573/598` `min_by_key` hostiles, `visibilitysystem.rs:520`, transfer pairing) — the set a unified instance absorbs.
- `RoomRouteCache` (`military/economy.rs`) is **the in-tree proof of the target pattern**: already a specs Resource threaded via `Write<RoomRouteCache>` through Mission/Operation system data; its `find_route` admission charge and tier read are the pieces that fold into the service.

## 3. `PathfinderService` — the single pathfinding instance

**A Resource, not a dispatched System.** Pathfinding queries are synchronous calls made *inside* other systems' execution (jobs, missions, operations), so the single instance must be fetchable mid-run — a scheduled System would force materializing request/response across stages. "Single pathfinding system instance" lands as *single owning Resource*; the budget attaches to it, satisfying both halves of the operator's intent.

```rust
/// THE single mission-side pathfinding instance: budget + caches + queries.
/// Movement (screeps-rover) deliberately stays outside — never-shed, independent
/// reserve, runs after missions/jobs; the shared input is GovernorSnapshot.tier,
/// not a shared pool.
pub struct PathfinderService {
    tier: Tier,       // cached at begin_tick — the one governor read the service makes
    pool: u32,        // tier-scaled tick cap (pool_for_tier stays a pure kernel)
    remaining: u32,   // plain fields — wasm is single-threaded, CAS deleted
    denied: u32,      // refused grants (saturation telemetry, ADR 0004 step 2)
    routes: RoomRouteCache,  // absorbed (field move — same SystemData slots)
}

impl PathfinderService {
    pub fn begin_tick(&mut self, tier: Tier);            // from tick_start
    pub fn take_ops(&mut self, want: u32) -> u32;        // partial grant; 0 ⇒ degrade
    /// Budgeted same-room search (absorbs the live PathFinderHelpers variant).
    pub fn search_same_room(&mut self, from: Position, to: Position, range: u32) -> Path;
    /// One budgeted search per candidate, shortest wins (absorbs find_nearest_from;
    /// sole client: dismantlebehavior). Exhausted pool ⇒ None ("no path" semantics).
    pub fn nearest_by_path<T: HasPosition>(&mut self, from: Position,
        candidates: impl IntoIterator<Item = T>, range: u32) -> Option<T>;
    /// Free Chebyshev nearest (no budget) — keeps the 4 linear call sites + folds
    /// the ad-hoc min_by_key duplicates.
    pub fn nearest_linear<T>(&self, from: Position, items: impl IntoIterator<Item = T>,
        pos: impl Fn(&T) -> Position) -> Option<T>;
    /// Inter-room route distance through the owned cache: FIND_ROUTE_NOMINAL_OPS
    /// admission, TTL, Critical serves stale (reads the tier cached at begin_tick —
    /// kills the cpugovernor::tier() read at economy.rs:204).
    pub fn route_distance(&mut self, from: RoomName, to: RoomName) -> Option<u32>;
    pub fn telemetry(&self) -> PathTelemetry;            // seg-57: pool/consumed/denied
}
```

- **Reachability:** `Write<PathfinderService>` joins Mission/Operation system data (where `RoomRouteCache` already sits — absorbing it is a field move) and `JobSystemData` → `&mut` field on `JobExecutionRuntimeData` (4 lines, reaches all 11 jobs with zero per-impl edits). The handle MUST live on the `&mut` runtime-data side — the shared `&` system-data side is a compile-time dead end.
- **Rover relationship: parameterize, never wrap or fork.** `MovementUpdateSystem` keeps building the ephemeral rover instance and pushing budgets through the existing setters; the movement-budget derivation becomes a pure kernel fed from `Read<GovernorSnapshot>`. Longer term (Inc 6/7): rover gains setters for its four internal knobs (flee ops, per-search formula, max rooms, stuck thresholds), and a `PathEngine` trait injected per call makes every search recordable/replayable; the two governor-bypassing raw `game::cpu::bucket()` reads (`movementsystem.rs:231`, `roomplansystem.rs:279`) retire onto the snapshot so it is the only CPU truth.
- **The one site needing design care:** the spawn-distance precompute fires inside lazily-flushed transfer-generator closures (`structure_data.rs:224` reached from `TransferQueueGenerator` boxed closures flushed mid-job/mission). Options: (a) extend the `TransferRequestSystemData` trait with a budget accessor (wide: ~19 generator-construction sites in 11 files); (b) hoist `let pathfinder = &mut *system_data.pathfinder;` before the 6 `create_structure_data` closures (narrow; the `Rc` clone ends the conflicting borrow, so hoisting works). Decide in the implementing PR; (b) is the default.

## 4. Migration plan (Phase-2 slices; all Breaking: **None** — in-heap state only)

| # | Slice | Validation |
|---|---|---|
| M0 | Dead-surface deletion (findnearest dead fns, `bucket_trend`, 3 dead `record_*`) | builds green; rides any commit |
| M1 | `GovernorSnapshot` Resource (~13 sites) | pressure scenario: identical tier-transition series in seg-57 vs `runs/pressure-…`; governor fixtures become per-instance (flake class closed) |
| M2 | `IntentRecorder` Resource (5 sinks + 23 sites, mechanical) | seg-57 intents counts+digest byte-identical across a smoke pre/post — the parity instrument validates its own migration |
| M3 | Fault counters → `MetricsState` | panic-containment scenario reproduces `panics_caught=1, serialize_skipped_aborted=1, vm_starts 1→2` exactly |
| M4 | `PathfinderService` v1 — **one PR** (pool must have exactly one owner; a half-migrated pool double-counts): service + pathbudget deletion + findnearest absorption + RoomRouteCache field move + 3 draw-site threadings | pressure scenario seg-57 `pathing.pool/consumed` series matches; dismantle smoke at parity; pool kernel tests ported as instance tests |
| M5/M6 | `Features` / `BotIdentity` Resources | smoke parity; per-instance feature-flag fixture proves isolation |
| M7 | `PathEngine` injection + cache absorption/persistence (the deferred P1.B5 remainder) + rover setters | Inc 6 (replay gate) / Inc 7 (cache formats) |

Dependency shape: M1 → {M2, M3, M4} independent; M5/M6 anytime; M7 gated on GameView. M1+M4 are the natural first pairing — and should land **before** the route-cache-warm work so it inherits the threaded handle. After M1–M4 the crate's mutable statics are 3 thread_locals (environment, features, username) plus the `WARNED` log-once latch, and zero atomics/Mutexes; after M6, the sanctioned set only.

**Risks/pins:** first-tick parity (`Default` = full Normal pool, matching today's static init — or the first VM tick sheds spuriously); `take` partial-grant semantics and seg-57 field meanings byte-identical (port the kernel tests); the 6 structure-data closures need the borrow hoist; **M3 changes counter lifetime semantics** — today's fault counters are cumulative per *VM* (process statics survive a `reset.environment` env-drop within one VM; `MetricsState` fields would reset with the environment), and the panic-scenario validation restarts the whole VM so it cannot catch the difference — either carry the fields across env recreate / seed from `Memory._metrics`, or document the per-environment semantics in the seg-57 schema; until migration lands, the KEEP-set statics keep one mutating test per module (host-parallelism watch item).
