# Component Test Plans — Per-Component Test Plans & Implementation Guidelines

- **Status:** Proposed
- **Date:** 2026-06-09
- **Deciders:** William Archbell
- **Related:** **ADR [0015](../design/0015-testing-and-validation-strategy.md)** (testing taxonomy & policy — this document is its per-component execution plan); ADR [0006](../design/0006-eval-and-iteration-harness.md) (owns the local-server harness, colony-health score, pre-deploy gates — extended here, never contradicted); [`rewrite-plan.md`](rewrite-plan.md) §3/§5/§6 (increment gates these plans absorb); [`proposed-fixes.md`](proposed-fixes.md) (Validation rows absorbed per component); [`../reviews/ibex-review-report.md`](../reviews/ibex-review-report.md) §9; ADRs [0001](../design/0001-entity-model.md)–[0014](../design/0014-empire-strategy-and-posture.md) (each ADR's validation commitments are absorbed into exactly one section below); [`../design/world-class-gap-analysis.md`](../design/world-class-gap-analysis.md) §4.

## Context

### How to read this document

One section per component (~16). Each section has five parts:

- **(a) Critical invariants** — the 3–7 properties whose violation causes survival/strategy failure.
- **(b) Test plan by layer** — concrete named cases per applicable layer. Layer names **L0–L6 are ADR 0015's** and are *not* redefined here; one-line mnemonics only: **L0** kernel (pure host unit) · **L1** fixture/contract (host vs in-memory doubles) · **L2** property/fuzz/golden · **L3** replay-parity (record→replay intent diff) · **L4** in-process composition (host-side multi-system: seam contracts + the seeded fault-injection sim — **never a server run**) · **L5** scenario-behavioral (private-server, distributional assertions — **every Docker/server run lives here**, including forced-reset, fault-injection-hook, and profiling runs) · **L6** soak (sustained-window invariants, nightly). Gate cadence (what runs per-change vs per-cutover vs nightly) is 0015 policy; this document only assigns cases to layers. (Adversarial-review correction: earlier drafts filed server-side forced-reset/fault-injection/profiling runs as L4, contradicting 0015's definition; every such row below is re-triaged to L5 — or L6 where it is a soak — and carries L5's cadence and assertion-form rules.)
- **(c) Fixtures & seams** — what exists today vs must be built, citing the testability inventory (purity classes: **a** = pure/testable now, **b** = DTO swap needed, **c** = needs a GameView-style seam).
- **(d) Implementation guidelines** — how to structure the Rust.
- **(e) Roadmap hooks** — increment placement, priority, **absorbed commitments** (every validation promise from the ADRs/plan/review is absorbed by exactly one section, marked ✓; cross-references where a commitment was promised in multiple docs), and the **iteration-tax note** (what is deliberately left untested per 0015's test-the-kernel/pin-the-shell rule).

**Priority legend:** **P0** survival-critical · **P1** composition-seam confidence (the value target) · **P2** quality/competitive.

### Build reality (verified by probe, 2026-06-09)

- The workspace default target is `wasm32-unknown-unknown` (`.cargo/config.toml:5-6`), so **bare `cargo test` is broken workspace-wide** — it cross-compiles the test binary to wasm and fails at execution (os error 193). `--target x86_64-pc-windows-msvc` (or the host triple) is mandatory.
- **Host builds compile, link, AND run today** — `cargo test -p screeps-ibex --target x86_64-pc-windows-msvc` passes with 0 tests. wasm-bindgen externs compile to panicking stubs on host; the boundary is **runtime**, not link time: any test invoking a JS-bound call (`game::rooms()`, `Creep::store()`, `RoomPosition` construction) panics. Since `panic="abort"` is **release-only** (`Cargo.toml:15-16`), a host dev-profile test that strays across the boundary is a clean test failure, not a process abort.
- Zero `#[test]`/`#[cfg(test)]`/`tests/` exist anywhere in the workspace (IBEX-023 confirmed). `screeps-foreman-bench` is the only verification artifact: a manual host harness that asserts nothing but proves the offline-planning seam and ships room-corpus fixtures (`screeps-foreman-bench/resources/map-mmo-shard0..3.json`).
- **One-time precondition (Inc 0, ~zero risk):** a cargo alias `test-host = "test --target x86_64-pc-windows-msvc"` in `.cargo/config.toml` (with a comment giving the non-Windows form), plus the CI dual-build (`wasm32` check + host test). This single flag is all that stands between the repo and working `cargo test`.

### Shared fixture infrastructure

Every piece below is consumed by ≥2 component plans. Each lands once, in the increment shown, owned by the section in **bold**. (This list resolves the commitments sweep's "distinct infra pieces" and assigns the orphaned ones an owner.)

| # | Infra piece | What it is | Lands | Owner / consumers |
|---|---|---|---|---|
| F1 | **Host test lane** | `test-host` alias + CI dual-build (wasm check + host test) + `cargo llvm-cov` on host | Inc 0 | **§15** / all |
| F2 | **GameView trait + in-memory double** | The room-snapshot ingestion seam (split of `room/data.rs` `update()`/lazy getters, data.rs:355-446/:465-516 — purity class **c**, the highest-fan-out seam in the tree) + a constructible double | trait skeleton Inc 0–1; full RoomData split rides rewrite increments (class L) | **§9-adjacent (room data)** — defined in §15.3 / §§2,5,8,9,14 |
| F3 | **MemoryArbiter double** | 3-method `SegmentStore` trait or `#[cfg(any(test, feature="testkit"))]` constructor pre-populating `active` (memorysystem.rs:54); gating logic (:128-188) is pure given `active` | Inc 0 (effort S) | **§1** / §15 |
| F4 | **Intent differ (Inc 1) + replay recorder (deferred to GameView-real)** | Two artifacts on two timelines (split per adversarial review — §15.3): the **intent-sink differ** lands Inc 1 and serves the scheduler gate via in-process **shadow-dispatch** over the same live tick (no replay, no recording, no GameView dependency); the **recorder proper** (GameView reads + emitted intents; replay old-vs-new behind a flag; byte-diff) rides the GameView-realization work — targeted Inc 4–5, hard-required before the Inc-6 HaulJob pilot. Storage: `runs/recordings/<scenario>/<git-sha>/`; **≥3 pinned reference recordings including a pressure run** (parity is necessary-not-sufficient — §15.3). Still the highest-fan-in piece in the corpus | differ Inc 1; recorder Inc 4–5 | **§15** / §3 (scheduler shadow parity), §8 (HaulJob ×3), §5, §6, §7, §12, §13 |
| F5 | **Old-snapshot corpus + corpus runner** | Real captured serialized payloads per schema version; rejection/decode oracle; carried forward as the Stage-2 migration oracle | capture starts with the first Inc 0 smoke run; runner Inc 2 | **§1** |
| F6 | **Decode fuzzer** | `cargo-fuzz` target on `decode_from_string` (random/truncated/bit-flipped); host-built by construction | Inc 2 | **§1** |
| F7 | **Snapshot fixtures** | Host specs Worlds: squad+members round-trip, recycled-index, deliberately-stale-ref | Inc 3 | **§2** |
| F8 | **Private-server harness core** | bollard lifecycle, CLI bootstrap, deploy (deploy.js interim), run control, console+segment reader — **owned by ADR 0006**, not re-decided here | Inc 0 | 0006 / all L4–L6 |
| F9 | **Scenario library + format + fault-injection hooks** | The ~17-scenario catalogue (§15.2 — resolves 0006's open question), parameterized; hooks: forced reset, mid-pass panic, one-tick visibility loss, fingerprint mismatch, member-kill | format Inc 0; scenarios land with their first consumer | **§15** / per-section |
| F10 | **CPU-pressure inducer** | The literal Inc-1 gate mechanism ("harness can induce CPU pressure") — unspecified anywhere; candidate designs in §15.4 | Inc 1 (blocking) | **§15** / §3 |
| F11 | **Opponent-bot infrastructure** | Second-account bootstrap + a combat opponent (`help(bots)` per 0006); later: wash-paint market adversary (0012 M4), hostile-operator driver (0013 P4) | Inc 4 (combat); later pieces evidence-gated | **§15** / §7, §12, §13 |
| F12 | **PowerFixture state seeder** | Direct mongo writes: GPL, pre-built operator docs, storage power, `isPowerEnabled` — mandated by 0013 D6, retrofitted onto 0006 | Inc 8 (design no later than Inc 7) | **§13** |
| F13 | **Seg-57 schema registry** | One versioned schema, eight contributing ADRs (0004/0005/0006/0011/0012/0013/0014/gap §4) — single registry module, version header on all metric segments | Inc 0 | **§15** |
| F14 | **Colony-health scorer + baseline store + differ** | 0006 §(2) four-term score; config-pinned weights/normalization (placeholder values pinned at Inc 0 — see §15.5); `runs/` keyed (scenario, git SHA); dual-scoring re-baseline at the Inc-8 boundary | Inc 0 | 0006 / **§15** |
| F15 | **Foreman bench corpus gate** | `screeps-foreman-bench` (exists) + pinned room corpus + score baseline wired as a pre-deploy plan-quality gate | any time after Inc 0 | **§10** |
| F16 | **Rover testkit fakes** | shared `testkit` module: ~50-line FakePathfinder + FakeCreep against the existing trait suite (screeps-rover/src/traits.rs:9-108) — zero production changes | Inc 0 | **§4** / §8 |
| F17 | **Market adversarial fixture set** | `MarketSnapshot` fixtures: T1 painted-day, T2 thin-book spike, T3 hollow-wall, T5 far-honeypot, T6 stale-quote drift, honest-volatility control | pre-Inc 7 (M0 is host-only, any time after Inc 0) | **§12** |
| F18 | **`features.rs` test setter** | `#[cfg(any(test, feature="testkit"))] pub fn set_features(Features)` — 3-line gap; `features()` is already host-safe (returns `Features::default()` without panicking, features.rs:457-535) | Inc 0 | **§15** / all |
| F19 | **Numeric-thresholds + seed-count config** | One `thresholds.toml` (or const module) for every scenario-gate number — cohesion threshold, boost/tower-drain latency bounds, emergence window W, incubation ≥30%, operator ≥95% uptime, score-regression threshold (values asserted across the ADRs but defined nowhere) — **plus per-gate-class seed counts N** (the single source; 0015 §3 defers here). Gates evaluate **paired-seed diffs vs the stored (scenario, seed, SHA) baseline** (F14); absolute values are reserved for physical bounds and stay flagged *provisional* until they survive a re-baseline — never silently pinned by the first run they gate (§15.5) | Inc 0 (file); baselines from F14 | **§15** |

### Two cross-cutting policies (0015's, applied here)

1. **Test the stable kernel; pin the experimental shell** (review §9). Every section's iteration-tax note names what is *deliberately* untested: per-mission `run_mission` bodies, utility weights, strategy constants — covered by harness gates and insta snapshots (one-keystroke re-baseline), never by hand-written unit assertions. Strategy iteration must stay near-zero-cost.
2. **Distributional assertions on emergent behavior.** L5 assertions are **paired-seed** quantile gates over N seeded runs — N from F19; the form is "matched seeds within tolerance of the same seed's stored baseline" (§15.5), not absolute thresholds — plus conditional polls ("storage reaches X within window"); never single-run exact values, fixed-tick asserts, or absolute thresholds born from the run they gate.

---

## Decision — per-component test plans

### §1 Serialization & persistence

Modules: `serialize.rs`, `memorysystem.rs` (MemoryArbiter), `game_loop.rs` serialize/deserialize path, segment map.

**(a) Critical invariants**
1. Round-trip identity: `decode(encode(w)) == w` for every serializable component graph.
2. Arbitrary/truncated/bit-flipped bytes **never panic, never silently half-decode** — reject-and-reset, loudly (ADR 0002 Stage 1).
3. Version mismatch → one deterministic loud reset, never a silently-empty world (IBEX-004/014).
4. Segment disjointness: `COST_MATRIX_SEGMENT ∉ COMPONENT_SEGMENTS` — **compile-time** (IBEX-013).
5. Overflow → loud watermark error, never a silent chunk drop (IBEX-014).
6. Forced reset reloads a **non-empty cost matrix** (the seg-55 wipe class).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `roundtrip_identity_per_component` | `encode_to_string`/`decode_from_string` (serialize.rs:310/:335 — pure bincode+gzip+base64, class **a**) |
| L0 | `convert_saveload_wrappers_roundtrip` | `EntityVec`/`EntityOption`/`EntityHashMap` (serialize.rs:67/:137/:251) against a host-constructed specs World |
| L0 | `version_mismatch_rejects_loudly`, `magic_garbage_rejects` | Stage-1 header check in `decode` |
| L0 | `arbiter_gating_logic` | `request_registered`/`gates_ready`/`pending_load_indices` (memorysystem.rs:128-188) via F3 double |
| L2 | proptest: **per-component DTO round-trips** (scoped deliberately — an `Arbitrary` over whole specs Worlds with the ConvertSaveload wrappers is real work, budgeted as its own M-sized task only if the per-component form proves insufficient); encode length monotone in entity count (watermark sanity) | serialize.rs |
| L2 | cargo-fuzz `fuzz_decode`: reject-and-reset, never panic, never half-decode (F6) | `decode_from_string` |
| L2 | golden: old-snapshot corpus replays — every prior-version payload either decodes identically or rejects loudly; **catches a forgotten version bump** (F5; the Stage-2 migration oracle per ADR 0002 step 3) | corpus runner |
| L2 | bench (criterion, host): WASM-size + CPU on the captured corpus — the ADR 0002 step-3 Stage-2 format decision input | corpus |
| L5 | `forced_reset_reloads_nonempty_cost_matrix` — **the single owner** of this four-times-promised test (IBEX-013 / 0002 step 1 / 0004 step 5 / plan §6 rollback row); also asserts the post-reset tick does not re-run the route storm (0004 step 5) | harness forced-reset hook (F9) |
| L5 | `ecs_inflation_trips_watermark` — drive entity count past the chunk budget (mechanism: scenario spawns a pathological creep/mission count via a debug console command); loud error, ≤5-chunk confirmation **before** the `COMPONENT_SEGMENTS` shrink (gating order pinned by IBEX-013) | harness |
| L5 | smoke gate: zero deser failures, zero overflow drops on every increment's smoke run (plan §5 gate 1) | seg-57 counters |
| L5 | `segment_bytes_before_after_serde_skip` at creep scale (IBEX-049, with §4) | harness |

**(c) Fixtures & seams** — encode/decode helpers pure **today** (class a, effort S). Build: F3 MemoryArbiter double (S — `active: Option<HashSet<u32>>` already exists as a field); F5 corpus (orphan resolved: **capture begins with the first Inc 0 smoke run** — the harness archives the raw segment payloads of every run under `runs/corpus/<format-version>/`); F6 fuzzer. The compile-time disjointness assert (IBEX-013) **is** its own regression test — no runtime test needed.

**(d) Implementation guidelines** — keep encode/decode free of JS forever (they are the frozen seam of ADR 0002); the fuzz crate lives at `fuzz/` (host-built by construction); corpus tests in `screeps-ibex/tests/serialization.rs` with `#![cfg(not(target_arch = "wasm32"))]`; every schema change requires a corpus entry + version bump in the same commit (checklist in the module doc-comment).

**(e) Roadmap hooks** — **P0.** Inc 0 (L0 kernels — the review's most-dangerous-untested #1), Inc 2 (full suite is the advance gate), Inc 5 (corpus as Stage-2 oracle).
Absorbed: ✓ review §9 round-trip; ✓ plan §5 universal gate 2; ✓ ADR 0002 Stage 1 (round-trip/corpus/fuzz via arbiter double), step 1 (forced reset + watermark + inflate), step 2 gate, step 3 (corpus bench + version-bump backstop); ✓ IBEX-013 (assert + seg-55 log + watermark-before-shrink ordering); ✓ IBEX-049 (segment-bytes measurement; rover half in §4); ✓ dedup #2 (one suite, ADR 0002 owns) and #3 (one forced-reset test, owned here).
Iteration tax: none — this component is the one place where exhaustive testing is the cheap option and the failure is unrecoverable.

---

### §2 Entity model & SquadStore

Modules: squad identity (`military/squad.rs` persistence side), `repair_entity_integrity` (game_loop.rs:168-369), the Inc-3 `SquadStore`/`SquadId`.

**(a) Critical invariants**
1. A recycled/stale slot resolves to **`None`, never a foreign squad** (IBEX-002b aliasing — ADR 0001 A1).
2. Serialize squad+members → deserialize → **same logical squad** (or clean `None`), across reset and index recycle.
3. A dangling ref is a handled lookup-miss, never a `ConvertSaveload` panic (review §9 #2).
4. Dangling-ref counter **zero across a sustained window** before `repair_entity_integrity` is deleted (ADR 0001 A3).
5. `id → Entity` rebuild is per-tick and deterministic.

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `generation_handle_stale_resolves_none` — the one validate-on-access helper (A1) | new handle type |
| L1 | F7 fixture: kill member + recycle slot → same logical squad or `None`, **never a different squad** (closes IBEX-002b; satisfies the IBEX-012 round-trip recommendation) | SquadStore round-trip |
| L1 | F7 fixture: full snapshot with deliberately stale refs → handled `None`s, no panic, **no repair pass** (A3) | post-deletion world |
| L3 | A2 cutover: old-vs-new parity before the store swap (shadow-dispatch form — the recorder is not yet live at Inc 3, §15.3); cutover on a low-stakes tick | F4 |
| L5 | smoke: dangling-ref counter == 0 every increment ≥3 | seg-57 |
| L6 | soak: counter zero across the sustained window — **the gate for deleting the repair pass** (Inc 5) | seg-57 nightly |

**(c) Fixtures & seams** — blocked on the Inc-3 store: recycled `specs::Entity` slots are **not host-constructible today**; stable IDs are the stated testability enabler (ADR 0001). The dangling-ref **counter emitter** is the orphan all of 0001/0005/0006 point at — resolved: it lives in the A1 validate-on-access helper (every `None`-from-stale increments it), schema slot in F13.

**(d) Implementation guidelines** — the handle type and `SquadStore` are plain Rust with zero game-API deps: keep them in a module with no `screeps` import so they are class-a by construction; fixtures construct Worlds via `specs::WorldExt` on host; the counter increments in exactly one helper (greppable single definition).

**(e) Roadmap hooks** — **P0.** A1 kernel + counter at Inc 3; round-trip fixtures at Inc 3; stale-ref fixture + soak gate at Inc 5.
Absorbed: ✓ ADR 0001 A1/A2/A3 validation rows; ✓ plan Inc 3 + Inc 5 validation columns; ✓ review §9 entity-ref repair invariant; ✓ plan §6 foreign-squad rollback trigger; ✓ dedup #5 (one fixture suite).
Iteration tax: the repair pass itself gets no new tests — it is scheduled for deletion; only its replacement invariant is tested.

---### §3 CPU governor, pathfinding facade & tick containment

Modules: the Inc-1 `CpuGovernor` (new), budgeted pathfinding facade (new, generalizing rover's budget), tick-level `catch_unwind` + scheduler seam (ADR 0005), `findnearest.rs`, `structure_data.rs`, `RoomRouteCache` (economy.rs:216-237). ADR 0005's runtime/scheduling commitments are absorbed **here** (no separate runtime section).

**(a) Critical invariants**
1. Tier decision is a **pure function** of `{bucket, trend, cpu_used, cpu_limit, tick_limit}` (ADR 0004 — pure by design when built).
2. Essential never sheds: defense, spawn, haul, movement pass, end-of-tick `serialize_world` (0004 shed order).
3. `MIN_PATHFIND_OPS` floor holds at Critical — creeps never fully freeze (IBEX-003/016).
4. **No uncapped search remains** — every pathfinding caller draws from the one shared ops pool.
5. A mid-pass panic never skips `serialize_world`; panic counter increments (IBEX-025 / ADR 0005).
6. Telemetry distinguishes **shed** from **aborted** (ADR 0005).
7. Cadence subtractions never underflow (`stored_tick > game::time()` — IBEX-044).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `tier_decision_table` — exhaustive boundary cases over the 5-tuple; hysteresis/trend window | `CpuGovernor` (build it pure; template: foreman `CpuBudget`, pipeline/mod.rs:11-34) |
| L0 | `ops_pool_accounting` — pool never over-drawn; floor reserved at Critical | facade budget arithmetic |
| L0 | `findnearest_combinators_with_fake_generator` — path generator is already injected (`find_nearest_from`, findnearest.rs:61-65); `find_nearest_linear*` Chebyshev (:135-169) | findnearest.rs (class a) |
| L0 | `saturating_sub_no_panic` on `stored_tick > game::time()` + grep all sites converted (IBEX-044, kernel + static) | cadence/timeout sites |
| L0 | `debug_assert_is_finite` fires under test; `length=0`/`cost=0` → finite (IBEX-046) | float priority/value sites |
| L3 | **scheduler-seam no-shed parity** — identical intent stream old-vs-new before cutover (dedup #7 owner: one gate for ADR 0005 / plan Inc 1 / review §8) — at Inc 1 via **in-process shadow-dispatch on live ticks** (§15.3; no recording exists yet); re-run as record/replay once the recorder lands | F4 differ (shadow-dispatch) |
| L4+L5 | `mid_pass_panic_serialize_ran` — **L4 owner: the host fault-injection sim** (injected mid-pass panic in-process, per 0015 §1) → `serialize_world` ran + `panics_caught` incremented; next tick rebuilds from last good snapshot (`env.tick` fix); the F9 panic-injection hook (a debug console command that panics a chosen system) re-runs it as the **L5 server variant** | host sim + harness fault injection |
| L5 | `war_cadence_raise_measured` — per-tier CPU via `features.system_timing` before/after; raise is measured, not blind (IBEX-021 = ADR 0004 step 1; dedup #13) | profiling run |
| L6 | `ops_saturation_telemetry_clean` — soak assertion: no caller saturates outside the facade (0004 step 1's "no uncapped search" proof) | seg-57 |
| L5 | **induced-pressure scenario** (dedup #4 owner — ONE scenario instance, F10 inducer, carrying five docs' layered assertions): (i) signals move + spiral alarm raises (0004 step 2); (ii) **progress continues, no restart loop**; over-shed also fails (0004 step 3 / plan Inc 1); (iii) far/unreachable path under low bucket → ops capped, `Failed` surfaced, others don't freeze; floor keeps creeps moving (0004 step 4); (iv) §5's haulers-keep-delivering, §10's multi-room-sheds-first, §14's claims-refused assertions ride this same scenario | F9 `pressure` scenario, N-seed quantile gates |
| L5 | `forced_reset_active_war` — progress continues, no restart loop, serialize never skipped (plan Inc 1 gate) | F9 |
| L6 | standing death-spiral alarm + nonzero-panic rollback triggers (plan §6) | seg-57 nightly |

**(c) Fixtures & seams** — governor/facade don't exist yet; **build them pure-by-design** (the test plan is an API constraint on Inc 1, not retrofit work). Existing: rover budget closures (movementsystem.rs:259-305) as the inner layer; foreman `CpuBudget` as the tier template. Must build: F10 pressure inducer (orphan #2 — candidate designs §15.4), F9 panic-injection hook, F4 recorder. `RoomRouteCache::compute_route` is class **c** (game::map calls, economy.rs:216-237) — inject rover's existing `PathfindingProvider::find_route` trait (effort M, already the right shape).

**(d) Implementation guidelines** — the governor is a plain struct + free function over the 5-tuple, in its own module with no `screeps` import; the facade exposes `&mut dyn PathfindingProvider`-shaped seams so L1 fixtures reuse F16; shed-vs-aborted is an enum on the seg-57 record, not two booleans; intent-cost constants get one `intent_cost(category)` table function (kernel-testable; cross-checked per §15.6).

**(e) Roadmap hooks** — **P0** (extinction-class). All of it lands Inc 1, except the L6 soak which is standing.
Absorbed: ✓ review §9 governor-decision-logic + reachable-panics rows; ✓ ADR 0004 steps 1–5 (step 5's forced-reset half is owned by §1, the no-route-storm clause asserted there); ✓ ADR 0005 (scheduler parity, panic injection, env.tick, shed-vs-aborted telemetry); ✓ plan Inc 1 validation column + §6 rollback triggers (death-spiral, nonzero-panic); ✓ IBEX-021 (dedup #13), IBEX-044, IBEX-046; ✓ dedup #4 (pressure scenario owner), #7 (scheduler parity owner), #15 (panic→serialize owner).
Iteration tax: shed-order *tuning* (which tier sheds what) is pinned by the pressure scenario's outcome gates only — no unit tests on the ordering table, it is expected to churn.

---

### §4 Movement & rover

Crate: `screeps-rover` (movementsystem, resolver, costmatrix) — **the best-seamed code in the tree**; class **a** throughout.

**(a) Critical invariants**
1. `resolve_conflicts` never leaves two creeps on one tile and never livelocks (shove/swap termination ≤ `max_shove_depth`).
2. Stuck handling repaths; an empty `path` after serde-skip-load triggers repath, never a frozen creep (IBEX-049).
3. Repath/CPU budgets honored (`RepathBudget`, `tick_limit`, `movement_cpu_cap` — movementsystem.rs:259-305).
4. Room-status traversal rules correct (`can_traverse_between_room_status`).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `resolve_conflicts_no_overlap`, `shove_depth_bounded`, `topological_sort_follows_acyclic` with `is_tile_walkable` closures | resolver.rs:256-261 |
| L0 | `room_status_traversal`, `room_linear_distance` | rover utility |
| L1 | full movement pipeline vs F16 fakes: pathing, shove/swap, stuck handling, budget exhaustion (deterministic counter closures), tick_limit behavior | `MovementSystem` (movementsystem.rs:276-309), `CostMatrixSystem::new(.., Box<dyn CostMatrixDataSource>)` (costmatrixsystem.rs:144-146) |
| L2 | proptest: random creep sets + walkability → no-overlap, termination, idle creeps shoved ≤ depth; metamorphic: adding an obstacle never shortens a returned path | resolver + fake pathfinder |
| L5 | `repath_on_empty_path_after_skip_load` (IBEX-049, with §1's byte measurement) | harness forced reset |

**(c) Fixtures & seams** — all seams exist (traits.rs:9-108; live impls quarantined in `screeps_impl.rs` behind `cfg(feature="screeps")`, lib.rs:13-14). Build only F16 (~50-line fakes, zero production change). The ibex-side `CostMatrixCache` shell (`pathing/costmatrixsystem.rs:12` reads segments) stays untested per the inventory — round-trip the cache type directly via §1.

**(d) Implementation guidelines** — F16 lives at `screeps-rover/src/testkit.rs` behind `feature = "testkit"` (so screeps-ibex tests can reuse it via a dev-dependency feature); tests run with `--no-default-features` (probe-verified to pass); keep new movement logic behind the existing trait suite — it is the in-repo template every other seam imitates.

**(e) Roadmap hooks** — **P1**, cheapest confidence in the tree: Tranche-1 work, Inc 0. IBEX-049's L4 case rides Inc 2's forced-reset machinery.
Absorbed: ✓ IBEX-049 (rover half); ✓ inventory tranche-1 rover rows; ✓ ADR 0004's reliance on the rover budget as the facade inner layer (tested here at L1, composed in §3).
Iteration tax: visualizer output and heuristic tuning (stuck thresholds, swap preferences) — pinned by §3's pressure scenario, not unit-tested.

---

### §5 Hauling & transfer matching

Modules: `transfersystem.rs` (TransferQueue, select_*), `transfer/utility.rs`, `jobs/utility/haulbehavior.rs`, hauler sizing.

**(a) Critical invariants**
1. **Never over-commit:** summed reservations ≤ available; capacity conservation across select/register (IBEX-011/030 churn class).
2. Priority monotonicity: a higher-priority request is never starved by a lower one in the same select pass.
3. Every `(TransferTarget variant, mode)` pair returns `Err`, **never panics** (superset rule, ADR 0007 step 1; absorbs IBEX-010's nuker-withdraw — dedup #8).
4. Committed delivery completes its target set; a rejected transfer never mis-accounts a ticket (ADR 0007 step 4).
5. Capacity helper: summed-used > capacity ⇒ 0, single definition (IBEX-045/050).
6. Hauler count reflects **route** distance, not linear distance (ADR 0007 step 5).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `select_pickup_accounting`, `select_best_delivery_ranking`, `never_overcommit`, `priority_monotone` — **after the Tranche-2 DTO swap** | transfersystem.rs:586/:1763 (class **b**: swap JS `RoomPosition` → local `screeps::Position` at :1771/:1963/:2029/:2120 + `TransferTarget::pos()` :148-165; data already local in `RemoteObjectId`, remoteobjectid.rs:7,31 — **S, highest value/cost ratio in the tree**) |
| L0 | `every_variant_mode_pair_errs` — exhaustive match matrix incl. (Nuker, withdraw) | TransferTarget arms (transfersystem.rs:208 class) |
| L0 | `capacity_helper_boundary` (summed-used > capacity → 0) + grep one definition (static) | IBEX-045/050 helper |
| L0 | `generate_active_priorities_order` | utility.rs:85-98 (pure today) |
| L0 | `transaction_cost_no_js` — after the one-line `game::map::get_room_linear_distance` → `RoomName::x_coord/y_coord` swap (cf. rover utility.rs:25-29) | utility.rs:5 |
| L0 | `pickup_state_capacity_math` — after the `(capacity, used, Position)` DTO replaces `&Creep` | haulbehavior.rs:13-51 (class b, S-M) |
| L1 | full request/select/register accounting vs a stub `&dyn TransferRequestSystemData` (transfersystem.rs:1310-1317; `RoomData::new(room_name)` is JS-free, data.rs:320). **Trap (inventory):** any generator path touching `RoomData::get_structures()` panics on host — DTO swaps first | matcher pipeline |
| L2 | proptest conservation: arbitrary request/offer sets → no over-commit, no negative balance; metamorphic: adding an offer never reduces total matched amount | select_* kernels |
| L3 | matcher cutover parity on the **recorded economy-bringup run** (dedup #12 — ONE recording artifact, owner §15, consumers: this gate, IBEX-050's "unchanged" check, §6's mission migrations) (ADR 0007 step 2); pure `(snapshot, creep) → tickets` tested at L0 first | F4 |
| L5 | `idle_hauler_generation_runs_once` profiling (0007 step 2) | harness profiling |
| L5 | pressure-scenario rider: matcher CPU drops under Conserve/Critical, **haulers keep delivering**, mid-siege tower-drain served within the F19 latency bound (0007 step 3) | §3's pressure scenario |
| L5 | `damaged_hauler_finishes_target_set`, `no_misaccount_on_rejected_transfer` (0007 step 4, with §8's HaulJob parity) | F9 + F4 |
| L5 | `hostile_wall_route_sizing` — hauler count reflects route not line; no idle tick on a known-full target; route lookups charged to the §3 shared pool (0007 step 5) | F9 |

**(c) Fixtures & seams** — the `&dyn TransferRequestSystemData` seam **exists** (the in-repo template ADR 0007 cites). Build: the two Tranche-2 DTO swaps (mechanical, no behavior change), the stub fixture, F4/F9 consumption.

**(d) Implementation guidelines** — after the swap, enforce "no JS types in `select_*` signatures" as a review rule (the seam is the contract); ticket accounting gets `debug_assert!` validators behind a `validate` feature (log-don't-panic in release per `panic="abort"`); matcher tests live in-module `#[cfg(test)]` (they need internal types).

**(e) Roadmap hooks** — **P0** for invariants 1/3 (Inc 1 quick-wins), **P1** for the matcher seam (Inc 1-vicinity), Inc 6 (committed-delivery via HaulJob), Inc 7 (route sizing).
Absorbed: ✓ ADR 0007 steps 1–5; ✓ IBEX-010 (via the step-1 superset, dedup #8), IBEX-011/030 (conservation/churn), IBEX-045/050 (kernel + static + parity); ✓ plan Inc 1/6/7 hauling rows; ✓ review §9 reachable-panic transfer arms.
Iteration tax: utility *weights* in delivery ranking — expected to churn; pinned by the L3 parity at cutover and the economy-bringup score gate only.

---

### §6 Spawn orchestration & body calculation

Modules: `spawnsystem.rs`, `creep.rs::create_body`, ADR 0011's orchestrator (demand model, group spawn, pre-spawn, renew policy).

**(a) Critical invariants**
1. **Descending** priority order in the queue — KNOWN-CORRECT today; lock in with a test + clarifying comment (the anti-re-flag guard; dedup #9 owner — one test for review §9 / plan Inc 0 / ADR 0011 step 0).
2. `create_body` respects `MAX_CREEP_SIZE`, energy floors, `min_repeat` failure (creep.rs:123).
3. A CRITICAL request is never delayed by a renew-eligible creep consuming the lane (the 0011 P4 inversion).
4. Group spawn align-finish: all quad members emerge within window W (F19).
5. Priorities finite — no NaN coalescing (IBEX-046; 0011 P7).
6. **Deterministic placement:** same snapshot ⇒ same fulfilling room (fixes the HashSet-iteration pick, spawnsystem.rs:284-289 — also a replay-parity prerequisite, orphan #11).
7. Starvation cure: all-haulers-dead + empty extensions recovers (review §6.11(b); 0011 D7).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `queue_order_descending` (+ comment), `next_spawn_duration_ticks` | spawnsystem.rs:82-95/:161-168 (pure today) |
| L0 | `create_body_boundaries` — energy floors, MAX_CREEP_SIZE, min_repeat → `Err` | creep.rs:123 (pure today) |
| L0 | 0011 pure kernels: budget model (throughput × energy), align-finish arithmetic, refill-stall rule, renew predicate (≤15-part adjacent utility at idle lanes) | orchestrator core (build pure) |
| L0 | `priority_is_finite` debug_asserts (IBEX-046) | priority sites |
| L1 | `process_room_spawns` behind a spawn-DTO + intent-out seam (class **c** today: game::rooms/spawn_creep/renew at :147/:178/:207/:213 — effort M) | spawn executor |
| L2 | property: **same snapshot ⇒ same placement** (determinism, invariant 6); metamorphic: more energy in never fewer spawns | placement function |
| L3 | orchestrator **parity mode** — same spawn stream as legacy before cutover (shadow-dispatch if the recorder isn't live yet at Inc 4 — §15.3; on the recorded engagement once it is); then seg-57 spawn-pressure metrics populate (0011 step 1) | F4 |
| L5 | `critical_preempts_renew` — large-creep renew + queued CRITICAL → CRITICAL spawns same tick (0011 step 0) | F9 |
| L5 | quad emergence within W; kill member mid-siege → successor emerges before `death − margin`; **zero combat renew intents** (0011 step 2; cohesion-rate half owned by §7 — dedup #14: kernel/scenario split, §7 owns the scenario assertion) | F9 engagement |
| L5 | `hauler_extinction_recovers` — all haulers die, extensions empty → minimal hauler within T+spawn-time, recovers to capacity sizing; CRITICAL big-body still pre-empts cheap LOW spam (0011 step 3 — the §6.11(b) regression test) | F9 starvation-wedge scenario |
| L3+L2 | per migrated mission (0011 step 4): replay parity on economy-bringup; zero-gap container-miner handover (energy-throughput non-regression golden); renew-predicate soak | F4 + seg-57 |
| L5+L2 | incubation (0011 step 5): incubated room self-sufficient ≥30% faster (F19), no donor spawn-uptime regression; defense spillover zero cancels in the normal case; placement-determinism property | F9 newborn-colony scenario |

**(c) Fixtures & seams** — comparator + `create_body` pure today. Build: spawn-DTO/intent-out seam (M); the orchestrator itself is new — **build its kernels pure** (0011 D1–D10 are mostly arithmetic + policy tables).

**(d) Implementation guidelines** — replace the HashSet room pick with sorted iteration *before* recording any parity run (determinism is upstream of F4); demand/budget kernels in a JS-free module; encode priority as the 0011 D10 class+deadline pair and test the f32 encoding's order-preservation at L0.

**(e) Roadmap hooks** — **P1.** L0 kernels Inc 0 (comparator, create_body); IBEX-046 + determinism fix Inc 1-vicinity; seam + parity Inc 4; starvation/group-spawn scenarios Inc 4/5; migrations Inc 5–6; incubation Inc 7.
Absorbed: ✓ ADR 0011 steps 0–5 + kernel list; ✓ review §9 comparator (KNOWN-CORRECT lock-in) + create_body kernel rows; ✓ plan Inc 0 kernel list (spawn-ordering descending invariant); ✓ IBEX-046 (spawn half), IBEX-047 (pre-spawn replaces reactive `remove_creep` — asserted via 0011 step 2); ✓ dedup #9 (one comparator test), #14 (kernel half).
Iteration tax: body-shape *templates* per role and priority constants — churn freely; pinned only by the smoke-run score and the starvation scenario.

---

### §7 Combat: formation, cohesion & Squad Manager

Modules: `military/squad.rs`, `military/formation.rs`, `military/damage.rs`, `military/bodies.rs`, `military/composition.rs`, ADR 0008's Squad Manager.

**(a) Critical invariants**
1. **Cohesion:** fraction of combat ticks with all members in-range stays above the F19 threshold AND does not regress vs baseline (dedup #6 — one scenario + one metric + one unified threshold phrasing, owned here; consumed by ADR 0003/0008/0011/0006's score term).
2. Non-cohesive squad **force-aborts within N ticks** (closes the lifecycle hang).
3. Kill member → pre-spawned successor (dedup #14 — scenario half owned here).
4. Tower fire decision correct under falloff (damage curves per [`../references/engine-mechanics.md`](../references/engine-mechanics.md)); anti-quad/edge policy (gap G-10).
5. **Zero combat renew intents** after 0011 lands (renew-as-glue is the IBEX-002 pathology).
6. Unreachable/impossible objective → squad retired within deadline — the **teardown-deadline scenario family** (dedup #10 owner: ONE family covering ADR 0003's unreachable attack, 0008 step 3's supervisor, 0010 L2's no-Forming-hang, plan §6's watchdog).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `formation_rotate_cw/orient_toward/mirror_y`, `compute_heal_assignments`, `reassign_slots`, `compute_retreat_centroid`, `should_retreat` | squad.rs:151/:162/:179/:515/:823/:727/:458 (pure today; only `resolve_creep` :236 is JS) |
| L0 | `tower_damage_falloff`, `total_tower_damage`, `should_towers_fire` **vs quad fixtures** (G-10: anti-quad/edge cases) | damage.rs:8/:76/:94 (pure today) |
| L0 | `solo_defender_body` etc.; `composition body_definition/estimated_cost/estimated_spawn_time/is_viable_from`; `estimated_travel_time` with tick param (one-line class-b swap, composition.rs:515) | bodies.rs:7, composition.rs:41-537 |
| L0 | `quad_cost_overlay` after terrain-DTO swap (pass foreman `FastRoomTerrain` instead of fetching, formation.rs:25-68 — class b, S); `virtual_anchor_target`, `advance_squad_virtual_position` (pure today, :145/:201) | formation.rs |
| L1 | Squad Manager reconciler (ADR 0008 seams): declare objective → kill member → successor pre-spawned; objective impossible → retired within deadline | `register_objective`/`objective_status` (new seam) |
| L1 | 0008 step 0: round-trip of a snapshot **without** the deleted variants (SquadAssault/Harass) — co-staged with the labelled reset (bincode discriminant shift) | F7-style fixture |
| L0 | `intent_no_double_fire` debug_assert at the guarded sink (IBEX-029 kernel half; sink owned by §8) | combat intent path |
| L3 | 0008 step 1: same engagement → equivalent spawn/intent stream old-vs-new manager; IBEX-029 flagged-vs-old parity | F4 (recorded engagement once the recorder lands; shadow-dispatch until then — §15.3) |
| L5 | **engagement scenario** (F11 opponent): cohesion metric (member spread, ticks-in-Loose) above F19 threshold + not regressed; non-cohesive force-abort within N ticks; no idle defense missions linger after threat clears (plan Inc 4 verbatim; 0008 step 2 defense form-up + retire-on-clear) | F9 + F11, N-seed quantile |
| L5 | **teardown-deadline family**: attack at unreachable room → teardown within deadline, no perpetual Running; drop `room_count` → active attacks trimmed (0008 step 3) | F9 |
| L6 | per-tick member spread + ticks-in-Loose logged; cohesion rate trend (ADR 0003 validation register) | seg-57 nightly |

**(c) Fixtures & seams** — geometry/heal/damage kernels pure **today** (Tranche 1). Build: quad fixtures (G-10, new artifacts); terrain-DTO swap (S); the Manager's reconciler seam (new — build pure); cohesion telemetry emitter (Inc 4, F13 slot); F11 opponent bot (orphan #3 — first design in §15.4).

**(d) Implementation guidelines** — `FormationLayout`/`SquadContext` stay JS-free (they nearly are); the Manager reconciler takes a roster DTO + objective and returns spawn-demand/abort decisions (no game reads inside); cohesion is computed from member `Position`s already in `SquadContext` and emitted once per tick to seg-57.

**(e) Roadmap hooks** — **P0** (Field Reports A/B). L0 kernels Inc 0; Manager kernels + parity Inc 4; engagement + teardown scenarios Inc 4; supervisor/variant-deletion Inc 5.
Absorbed: ✓ ADR 0003 §B (cohesion invariant, force-abort) + validation register (spread/ticks-in-Loose, unreachable teardown); ✓ ADR 0008 step 0 (build green, zero live call sites static check, no-deleted-variant round-trip), step 1 (parity + renew-loop gone + successor), step 2 (form-up + retire deadline), step 3 (supervisor trim/teardown) + kernel seams; ✓ plan Inc 4 validation + §6 cohesion/lifecycle rollback rows; ✓ IBEX-001/002/026/028; IBEX-029 (kernel here, sink in §8); ✓ gap G-10; ✓ dedup #6 (cohesion owner), #10 (teardown family owner), #14 (scenario half).
Iteration tax: target-selection utility scoring and formation *choice* — utility weights pinned by engagement-scenario win-rate only (0015 shell rule).

---

### §8 Jobs & behavior FSM

Modules: `machine_tick.rs`, `jobs/*` (screeps-machine FSMs), the Inc-6 data-driven FSM, the guarded intent sink (ADR 0003 §A).

**(a) Critical invariants**
1. **Byte-identical intent streams** on the HaulJob pilot before any other job migrates (ADR 0003 — the Inc-6 gate).
2. No creep fires the same intent category twice per tick — one guarded sink (IBEX-029 / ADR 0003 §A).
3. The 20-transition cap prevents livelock (machine_tick.rs:5-40).
4. Transient fault ⇒ `Wait`, not teardown: a one-tick `room_data == None` never tears down a long-running mission (ADR 0003 / plan Inc 6).
5. FSM state stays serializable (ticket-carrying states, haul.rs:28-51).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `run_state_machine_transition_cap`, label/tick_fn driver semantics | machine_tick.rs:5-40 (pure today — also the replay harness for any DTO-ized job state) |
| L0 | new-FSM transition tables + SELECTION/EXECUTION split as pure kernels (ADR 0003: utility for selection only) | data-driven FSM (build pure) |
| L0 | `sink_rejects_double_fire` — the debug_assert at the ONE sink (interim Inc 1 via IBEX-029; permanent at Inc 6) | intent sink |
| L1 | job states replayed under `machine_tick` against a `CreepState` DTO — **requires the `JobExecutionRuntimeData.owner: &'a Creep` → DTO refactor** (jobsystem.rs:38-40; template: rover `CreepHandle`, traits.rs:9-16) — the single biggest class-b refactor (M-L), highest-leverage next item per the inventory | all job FSMs |
| L3 | **HaulJob replay parity — ONE harness, reused three times** (dedup #1 owner): (i) old screeps-machine HaulJob vs new FSM, byte-identical intents (ADR 0003 pilot / plan Inc 6); (ii) §5's matcher cutover (0007 step 2); (iii) §5's committed-delivery guard (0007 step 4) | F4 on the recorded economy-bringup run |
| L4+L5 | `one_tick_visibility_loss_waits` — **L4 owner: the host fault-injection sim** (a snapshot with `room_data` = `None` for one tick, per 0015 §1) → mission `Wait`s, not deleted; the F9 hook re-runs it as the **L5 server variant** | host sim + harness fault injection |
| L5 | unreachable job target → leaves move state within N ticks (ADR 0003 validation register; rides §7's teardown family) | F9 |

**(c) Fixtures & seams** — `machine_tick` pure today. Build: `CreepState` DTO (M-L — schedule immediately after Tranche 2 per the inventory; it unlocks every job FSM under replay); the guarded sink; F4; the visibility-loss injection hook (orphan: designed as an F9 hook — a debug flag that masks one room's visibility for one tick).

**(d) Implementation guidelines** — the new FSM's transition tables are data (`const` tables / serde-loaded), so L0 tests enumerate them mechanically; the sink is the **only** module that touches creep intent methods (grep-enforced); job tests live next to each job module; per 0015's shell rule, *states* are tested, *tick glue* is pinned by parity.

**(e) Roadmap hooks** — **P1** (the composition-seam centerpiece). machine_tick kernel Inc 0; sink assert Inc 1 (interim); CreepState DTO after Tranche 2; pilot + parity + fault tolerance Inc 6.
Absorbed: ✓ ADR 0003 §A (sink + double-fire assert), FSM pilot (byte-identical gate), transition-table kernels, validation-register stuck-recovery + visibility-loss rows; ✓ plan Inc 6 validation column; ✓ IBEX-006/029/042; ✓ dedup #1 (one HaulJob harness).
Iteration tax: per-job behavior *content* beyond HaulJob — migrated job-by-job under the same parity harness; no hand-written per-state unit tests for jobs still on screeps-machine (they're scheduled for replacement).

---

### §9 Mission & operation lifecycle

Modules: `missionsystem.rs`, `operations/*`, mission dispatch/cleanup/abort machinery.

**(a) Critical invariants**
1. Every campaign reaches a terminator: progress or teardown by deadline (Field Report B; asserted via §7's teardown family).
2. Abort/cleanup bookkeeping never leaks entities or children (`queue_mission_abort`, missionsystem.rs:76-90; result handling :255-256).
3. Degraded room access (`get_room()` → None/sentinel) never panics; cleanup/repair callers no-op (IBEX-020).
4. The stuck-operation watchdog fires when a campaign neither progresses nor tears down (plan §6 rollback trigger).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L1 | **stub-Mission fixture** drives dispatch/cleanup/abort host-side: `Mission` trait bookkeeping (`get_owner`/`child_complete`/`remove_creep` are pure entity bookkeeping; `MissionExecutionSystemData` is all specs storages + in-crate queues, constructible in a host World — missionsystem.rs:46-160; effort M) | mission machinery |
| L1 | `degraded_get_room_no_panic` — None/sentinel mission fixture (IBEX-020) | attack_mission.rs:1917 class |
| L5 | watchdog: ticks-since-progress per operation emitted to seg-57 (orphan #10 resolved — the emitter is a per-operation `last_progress_tick` updated by the supervisor; the watchdog is a harness assertion over it) | seg-57 + harness |
| L5 | teardown-deadline family (owned §7); no idle defense missions linger (with §7) | F9 |
| L6 | standing watchdog assertion in nightly soak (plan §6) | seg-57 |

**(c) Fixtures & seams** — `Mission` trait is class **b**: the bookkeeping half is host-constructible today; concrete `run_mission` bodies are class **c** and are **explicitly not unit-tested** (inventory: cover via eval harness with distributional assertions).

**(d) Implementation guidelines** — keep the stub Mission in `missionsystem.rs` `#[cfg(test)]`; new lifecycle code (per-state wall-clock deadlines, 0008 supervisor) carries its deadline as data so the L1 fixture can fast-forward it.

**(e) Roadmap hooks** — **P1.** Stub fixture + IBEX-020 at Inc 1; watchdog emitter Inc 4 (with §7's lifecycle work); soak standing.
Absorbed: ✓ IBEX-002 (lifecycle hang — via deadlines + watchdog), IBEX-020 (fixture); ✓ plan §6 stuck-operation watchdog row (orphan #10 assigned); ✓ ADR 0003 lifecycle validation (with §7); ✓ review §9 console-event row (force-abort, stuck-watchdog events — schema in §15).
Iteration tax: **all `run_mission` bodies** — the deliberate shell. Emergent mission behavior is pinned exclusively by L5 scenarios + the colony-health score. **Known thin spot (flagged by adversarial review):** economy missions specifically are pinned almost solely by the score's economic term on the one economy-bringup scenario — a single distributional signal; accepted deliberately, but a regression that preserves the aggregate slope can slip through. Revisit when the contested-expansion scenario (Inc 7) gives the economy a second independent gate.

---

### §10 Room planning (foreman) + RoomGraph

Crates/modules: `screeps-foreman` (23 layer files, planner, pipeline), `screeps-foreman-bench`, `room/gather.rs`, the Inc-7 `RoomGraph` + `InterRoomRoadLayer`.

**(a) Critical invariants**
1. Plan quality never regresses on the pinned room corpus; no Failed/unreachable plans (ADR 0009 step 2 — the pre-deploy plan-quality gate).
2. Restart backoff: permanent fingerprint mismatch stops replanning after the cap (IBEX-037).
3. Discovery bursts budgeted (gather BFS, IBEX-036).
4. RoomGraph rejects routes through hostile/closed rooms (IBEX-032); reloads non-empty after reset, no room-graph storm.
5. Inter-room pass sheds **first** under pressure, no headroom regression (ADR 0009 step 4).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | layer snapshot tests (insta) per layer file against `screeps-foreman-bench/resources/map-mmo-shard*.json` terrain — the layers use only `screeps::constants::StructureType` (e.g. layers/extension.rs:26; **already proven host-side daily** by the bench) | 23 layer files |
| L0 | plan-scoring kernel (review §9 named kernel: foreman plan scoring) | scorer |
| L0 | gather BFS with injected exits-provider closure — `candidate_generator` is **already injected** (gather.rs:122-125); inline `describe_exits`/`RoomStatusCache` (:143/:181) swapped for the provider (class b, S-M; `RoomData` already stores `exits`, data.rs:105-110) | gather.rs:118-196 |
| L0 | `CpuBudget` exhaustion determinism (counter closure) — the in-repo budget template | pipeline/mod.rs:11-34 |
| L2 | **golden bench-corpus gate** (F15): regenerate plans for the fixed corpus → insta snapshot diff + score non-regression; PlanScore feeds the economic term; a weight change must improve/hold colony-health on bringup (ADR 0009 step 2) | bench-in-workspace |
| L5 | `fingerprint_mismatch_backoff` — F9 fault-injection forces a permanent mismatch → replanning stops after cap, backs off (0009 step 1 / IBEX-037); discover-burst profiling (IBEX-036) | harness |
| L5 | `room_graph_reloads_nonempty` post-reset, no storm (0009 step 3) | harness forced reset |
| L5 | `hostile_room_between_home_and_candidate_rejected` (0009 step 3 / IBEX-032) | F9 |
| L5 | remote trunk generated within budget, merges, mission builds it; **inter-room pass sheds first** under pressure with no headroom regression vs baseline (0009 step 4 — rides §3's pressure scenario) | F9 + golden |

**(c) Fixtures & seams** — the **strongest existing seams in the repo**: `PlannerRoomDataSource` (room_data.rs:7-12) with a copy-pasteable JSON impl (bench main.rs:83-105), `RoomVisualizer` as output collector (bench main.rs:360), `CpuBudget`. Build: move the bench into the workspace as `tests/`+`benches` (or add `#[test]` snapshot wrappers — note the bench is workspace-`exclude`d with its own patch table and inherits the parent wasm default via cargo's config walk, so it needs `--target` too); the `execute_operations` intent-collection double (currently feature-gated out rather than abstracted — small new seam); F15 baseline.

**(d) Implementation guidelines** — keep the `screeps` feature gating exactly three things (planner.rs:20-21, plan.rs:459/:512, visual.rs:11-107) — it is the pattern other crates should copy; insta snapshots under `screeps-foreman/tests/snapshots/`; corpus rooms are pinned by hash; weight-calibration loop documented next to the weights.

**(e) Roadmap hooks** — **P1** (gate) / **P2** (tuning). Layer snapshots + bench gate: any time after Inc 0 (host-only). Backoff/burst: Inc 1. RoomGraph/roads: Inc 7.
Absorbed: ✓ ADR 0009 step 1 (backoff + burst), step 2 (bench gate + weight calibration), step 3 (RoomGraph reject + reload), step 4 (trunk + shed-first); ✓ IBEX-032/036/037; ✓ plan Inc 7 validation; ✓ review §9 foreman-plan-scoring kernel; ✓ orphan #12 (bench↔harness tie-in — owner assigned: this section, wired at F15).
Iteration tax: layout aesthetics and stamp choice — snapshot-pinned only; a layout change is a one-keystroke `cargo insta review` re-baseline plus the score gate.

---

### §11 Boost, lab & factory pipeline

Modules: ADR 0010's ReagentPlanner, lab roles, BoostQueue wiring, FactoryMission.

**(a) Critical invariants**
1. **Defense spawn is never gated by boost availability** (dedup #17 owner — one invariant for 0010 L1 + 0013 D5.1): defenders spawn immediately, boost within the F19 latency bound when stocked, fight unboosted otherwise.
2. Tier A deficit ⇒ next lab assignment is Tier A (priority ordering — soak invariant).
3. Boosted objective deploys boosted OR falls back within deadline — **no Forming hang** (the Field Report B regression; rides §7's teardown family).
4. No factory intent without inputs; `OPERATE_FACTORY` never called (level-0-only doctrine — review + debug assert).
5. Bars sell only on price-check pass; compression follows the storage-energy thresholds.

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | reaction-selection + chain-math/targets kernels (0010 L0); `has_boost` truth table on a fixture | ReagentPlanner (build pure) |
| L0 | `operate_factory_never` debug_assert + review rule | factory intent path |
| L5 | 0010 L0 gate: **zero behavior change** in the smoke run when the planner lands read-only | harness |
| L5 | defense @ PlayerRaid: spawn-immediately + boost-latency + unboosted-fallback + Tier-A refill after consumption (0010 L1) | F9 siege scenario |
| L5 | cross-room chain staging produces T3 end-to-end; boosted-objective fallback deadline; cohesion metric unaffected (0010 L2) | F9 |
| L5 | storage-energy oscillation: compress/decompress across thresholds; price-check-gated sells (0010 L4) | F9 oscillation scenario (shared with §13 P1) |
| L6 | Tier-A-deficit ⇒ Tier-A-next soak invariant | seg-57 nightly |

**(c) Fixtures & seams** — all new code; **build the planner kernels pure** (chain math is arithmetic over reaction tables). The catalyst-less-purchase scenario (0010 L3) is **the same scenario as 0012 M2** (dedup #11) — owned by §12, consumed here.

**(d) Implementation guidelines** — reaction/chain tables as `const` data (mechanically enumerable at L0); boost-readiness check is a pure predicate over a lab-stock DTO; the never-delay-defense rule is enforced at the spawn-demand layer (§6's seam), tested there at L1 and here at L5.

**(e) Roadmap hooks** — **P1** for invariant 1/3 (regression-class), **P2** rest. L0 any time after Inc 0; L1 scenario Inc 4 (needs siege scenario); L2/L3/L4 Inc 7.
Absorbed: ✓ ADR 0010 L0–L4 + the Tier-A soak invariant + the never-gate-defense consumer rule (dedup #17); ✓ IBEX-027 (wire-or-delete validated by the L0 zero-behavior-change gate + L1 scenario); ✓ dedup #11 (consumer side).
Iteration tax: stockpile targets and tier sizing — config churn pinned by the soak invariant and score only.

---

### §12 Market & risk

Modules: ADR 0012's FairValue oracle, TradePlanner, TradeGovernor, risk ledger (seg 58).

**(a) Critical invariants**
1. No buy above the FairValue ceiling; no deal above the transfer-cost cap — even when the oracle is fooled (exposure caps bound the worst case).
2. Kill-switch trips on T1/T2 manipulation **and does not trip on honest volatility**.
3. Plans never reference order-book depth (the manipulable signal).
4. Painted-day fixture: latest-mean moves ≥10× while the oracle moves <5% (0012 M0 calibration case).
5. Forced VM reset preserves exact ledger counters; governor fails toward Restricted until seg 58 loads.

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0/L1 | **adversarial fixture set** (F17): T1 painted-day, T2 thin-book spike, T3 hollow-wall, T5 far-honeypot, T6 stale-quote drift, honest-volatility control — oracle/planner/governor kernels against each: no buy above ceiling, no deal above cap, kill-switch on T1/T2, depth never referenced, Normal retained on honest control (0012 M0). Anchor clamp depends on §11's chain-math kernel | FairValue / TradePlanner / TradeGovernor (build pure over `MarketSnapshot` DTOs) |
| L1 | M1 quick-win on the **existing** system: painted-day → zero buys; far-honeypot → zero deals above cap (IBEX-018) | current trade path behind a snapshot DTO |
| L5 | M1: smoke-run sell throughput unchanged on the honest scenario; M3: forced reset → exact ledger counters preserved; T1 trips Halted within one cadence + operator alert; honest-volatility does not | harness |
| L3+L5 | M2: catalyst-less empire reaches Tier A within caps (dedup #11 **owner**; = 0010 L3); stale sell repriced down within one cadence; unwanted order de-backed never cancelled; intent-diff shows no fill-rate regression | F9 + F4 |
| L5 (evidence-gated) | M4 adversary-bot wash-paint end-to-end on the private server — **build only if fixture-level coverage proves insufficient** (0012's explicit gate; needs F11 second account) | F11 |

**(c) Fixtures & seams** — all new artifacts: F17 fixture set (host JSON/`MarketSnapshot` builders); a snapshot DTO over today's market reads for M1. Seg-58 round-trip rides §1's machinery.

**(d) Implementation guidelines** — oracle/planner/governor never call `game::market` directly; one `MarketSnapshot` ingest per cadence (mirrors the §5 matcher pattern); fixtures as data files under `tests/fixtures/market/` with a builder so new manipulation patterns are one-file additions; ledger counters serialized via §1's versioned format.

**(e) Roadmap hooks** — **P1** (real credit loss) for M0/M1; **P2** for M2/M3; M4 explicitly deferred-by-evidence. M0/M1 any time after Inc 0 (host-only + smoke); M2/M3 Inc 7.
Absorbed: ✓ ADR 0012 M0 (fixtures + calibration), M1 (IBEX-018 quick-win), M2 (catalyst-less + quote hygiene + fill-rate parity), M3 (reset ledger + kill-switch), M4 (adversary bot, evidence-gated); ✓ dedup #11 (scenario owner).
Iteration tax: pricing parameters and buy/sell program composition — pinned by the M-fixtures' pass/fail and the honest-control non-trip only.

---

### §13 Power economy & power creeps

Modules: ADR 0013's bank pipeline, processing throttle, OperatorSystem.

**(a) Critical invariants**
1. Bank go/no-go window: fresh bank accepted, ~2,500-ticks-left rejected (P0 kernel).
2. Duo math: zero healer-deficit deaths inside the decay window.
3. Operator never dies across the suite (8h real-time respawn cooldown — *cannot* be tick-compressed, so death-avoidance gets a **kernel test, not a faithful scenario** — 0013's honest harness-limitation caveat, kept).
4. Forced reset re-derives operator assignment/schedule with **zero serialized additions** (P3).
5. Processing halts below the surplus line, resumes above (P1).
6. Power-bank concurrency: N non-bank attacks at cap=1 still permit a bank launch — independent slot filling (IBEX-043).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `bank_go_no_go_window` (fresh accept / 2,500-left reject); duo heal-vs-decay math; cast-scheduler kernels; **death-avoidance retreat policy kernel** (invariant 3) | bank feasibility + operator policy (build pure; engine numbers per [`engine-mechanics.md`](../references/engine-mechanics.md)) |
| L5 | bank scenario: kill inside decay window, zero healer-deficit deaths (P0); haulers arrive within the 500-tick ruin window (P2) | F9 power-bank-farm scenario |
| L3 | P2: replay parity on the P0 scenario for the `Farm{powerbank}` objective cutover | F4 |
| L5 | `power_bank_concurrency_independent_slots` (IBEX-043): N non-bank attacks active, cap=1 → bank launch still permitted | F9 war-capable scenario |
| L5 | energy-oscillation: processing halts/resumes across the surplus line, hauler traffic follows (P1; shared scenario with §11 L4) | F9 |
| L5 | **PowerFixture@GPL-8** (P3): 3 spawn effects ≥95% uptime (F19); spawn-throughput KPI ≥1.9× baseline at r3 (golden); forced reset re-derives with zero serialized additions; operator never dies across the suite | F12 seeder + F9 |
| L5 | P4: siege FORTIFY holds breach rampart, towers+operator exceed quad-heal line; hostile-operator classified/focused/safe-mode fallback (needs F11 hostile-operator driver) | F9 + F11 |
| L2 | P5: each late power enters only on seg-57 evidence (the Inc-9 menu rule — a golden evidence gate, not a test) | seg-57 |

**(c) Fixtures & seams** — all new. **F12 is mandatory and unbuilt** (orphan #4 resolved: owned here, designed as a harness-bootstrap extension — direct mongo writes via the bollard/CLI path for `user.power`, power-creep docs, `isPowerEnabled`; design due no later than Inc 7 so Inc 8 isn't blocked). F11's hostile-operator driver is the latest-landing opponent piece.

**(d) Implementation guidelines** — operator schedule/assignment derived, never serialized (the P3 zero-additions gate is an architecture rule, enforced by the forced-reset test); power kernels in a JS-free module keyed by engine constants imported from one place.

**(e) Roadmap hooks** — kernels **P0-cheap, land Inc 1** (per the sweep's P0 row); scenarios **P2**, Inc 7 (P1/P2) and Inc 8 (P3/P4); P5 Inc 9.
Absorbed: ✓ ADR 0013 P0–P5, D6 retrofit (F12), the death-avoidance kernel-not-scenario caveat; ✓ IBEX-043; ✓ orphan #4 (seeder owner assigned).
Iteration tax: cast-priority tuning and bank-claim thresholds — score-pinned; the P5 menu rule *is* the scope-control test.

---

### §14 Intel, visibility, threat classification & empire posture

Modules: `military/threatmap.rs`, `room/visibilitysystem.rs`, `room_status_cache.rs`; **ADR 0014's PostureEngine/EmpireAllocator are absorbed here** (the executive layer consumes intel; no separate posture section).

**(a) Critical invariants**
1. `classify_threat` is correct on distributional inputs and **monotone**: an extra hostile never lowers the threat level (metamorphic).
2. VisibilityQueue claim/release/expire never leaks or double-claims.
3. Posture: sacked-room → Recovery; sustained-Conserve → Expand refused; dwell prevents flap on oscillating threat; declarations respect G-14 limits (0014 step 0).
4. No player-room attack launches without a declaration; peace tears down within deadline (0014 step 2).
5. CPU capacity model: under induced pressure, claims refused while policing/economy continue; `marginal_room_cpu` within the 2–10 sanity band (0014 step 3).
6. Posture flips reproducible from recorded seg-57 streams (auditability).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `classify_threat_distributional` — boosted/unboosted mixes, invader cores, nukes | threatmap.rs:162 (pure on `HostileCreepInfo` DTOs today — :8-25/:67-90) |
| L0 | `analyze_hostile_creep` after the body-DTO swap (`Vec<(Part, u32, Option<Boost>)>` param — Tranche-2 item (2), class b, S) | threatmap.rs:93 |
| L0 | `visibility_queue_claim_release_expire`, `best_unclaimed_for`; `request` with tick param (one-line swap, :158) | visibilitysystem.rs:182-260 |
| L0+L1 | posture kernels + fixtures: sacked→Recovery, Conserve→Expand-refused, G-14 limits, dwell anti-flap (0014 step 0); allocator + capacity-rule kernels | PostureEngine/Allocator (build pure) |
| L2 | proptest `classify_threat` monotonicity (metamorphic relation above); posture-flip determinism from recorded streams | threatmap + posture |
| L5 | 0014 step 1: posture resource lands **read-only** — sane postures across bringup + siege, **zero behavior diffs** | harness |
| L5 | 0014 step 2: no undeclared player-room attack; flag-declared war launches; peace teardown within deadline; policing unchanged vs baseline (golden) | F9 |
| L5 | 0014 step 3: pressure-scenario rider (claims refused, economy continues — §3's scenario); manufactured negative-margin remote dropped, resumes on recovery; `marginal_room_cpu` sanity band on the multi-room scenario | F9 |
| L5 | 0014 step 4: declared-war (embargo flips, war chest fills first, fronts provision ahead of demand); **sacked-room Recovery** (the G-12 harness gate — rebuild outranks all, exports halt); peace archives WarDecl with realized cost | F9 (sacked-room scenario owner assigned here) |
| — | pixels (G-15): flag-gated, **untestable-by-harness — marked so**, per 0014 | exclusion |

**(c) Fixtures & seams** — threat DTO layer exists (the inventory's seam #6); `ThreatAssessmentSystem::run` shell stays class **c** (game::time/rooms, :239/:289) and untested. Build: body-DTO swap (S); posture kernels pure-by-design; the sacked-room scenario (orphan from G-12, owner assigned here).

**(d) Implementation guidelines** — posture transitions logged to seg-57 with their inputs (the reproducibility invariant is a replay over that record); the allocator never reads game state directly — it consumes the same snapshot DTOs the posture engine does; `intel_freshness` emitted as a gap-§4 sub-metric (schema slot in F13).

**(e) Roadmap hooks** — threat/visibility kernels **P1, Inc 0** (Tranche 1 + one Tranche-2 swap); posture kernels after Inc 0; read-only resource Inc 4–5; executor split Inc 5; capacity model Inc 7; WAR/PEACE + Recovery Inc 8.
Absorbed: ✓ inventory threat/visibility rows; ✓ ADR 0014 steps 0–4 + auditability + the G-15 exclusion; ✓ gap G-12 (sacked-room owner assigned), gap §4 `intel_freshness`/`deterrence_events` sub-metrics (emitters here, schema §15).
Iteration tax: threat-score *weights* and posture thresholds — dwell/flap behavior is invariant-tested, the numbers churn freely under the scenario gates.

---

### §15 Telemetry, metrics & the harness itself (testing the tester)

Modules: seg-57 emitters, `stats_history.rs`, the colony-health scorer, the eval harness (`screeps-eval`), scenario library, replay tooling, and the renderer/visual flush path (the `VisualBackend` collector — adopted here after adversarial review found it absorbed nowhere). **ADR 0006 owns the harness; this section extends it** with the pieces downstream ADRs assumed but nobody owned.

**(a) Critical invariants**
1. Seg-57 is **one versioned schema with one registry** (dedup #16 — eight contributing ADRs, single owner here); version header on ALL metric segments (seg 56/99 included).
2. The score is reproducible: config-pinned weights/normalization (orphan #9 resolved — placeholder values pinned at Inc 0 in F19/config; recalibrated only at the recorded re-baseline points); term breakdown recorded so regressions are attributable.
3. Harness verdicts are trustworthy: an infra failure (server died, deploy failed) is **never** reported as a bot failure.
4. Stats downsampling is correct under missed ticks/wrapping (stats_history cascade).
5. L5 assertions are conditional polls + N-seed quantiles, never fixed-tick or single-run exacts (0015 policy, enforced in the scenario format).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `stats_history_cascade` — push/cascade/average_tier under missed-tick + wrapping | stats_history.rs:64-117 (pure today) |
| L0 | scorer term computations on synthetic seg-57 series; baseline differ on synthetic `runs/` artifacts; seg-57 encode/decode round-trip (via §1 helpers) | scorer + registry |
| L0 | `intent_cost_table_matches_engine` — the orphaned ADR 0004 "validate intent costs against the open-source engine" (orphan #8 resolved: a host test comparing our `intent_cost(category)` table against constants extracted from the cloned engine source, cited per [`engine-mechanics.md`](../references/engine-mechanics.md)) | cost table |
| L2 | golden: score-schema snapshot (gap §4 sub-metrics: `energy_throughput/cpu_used`, `gcl_per_upgrade_energy`, `spawn_utilization`, `boost_coverage`, `deterrence_events`, `intel_freshness`, `market_pnl`); **dual-scoring re-baseline recorded once at the Inc-8 boundary** | scorer config |
| L5 | harness self-test: re-run a pinned known-good SHA on a pinned scenario → score within tolerance (the harness's own regression test); kill-server-mid-run → infra-failure verdict, not bot-failure | screeps-eval |
| L0 | `visual_watermark_and_finite_coord_clamp` — the kernel half of the rewrite-plan §6 renderer rollback row (IBEX-008): visual-payload watermark math + every coordinate clamped finite/in-range before the (Result-narrowed, per ADR 0005) flush | visual payload builder / `VisualBackend` collector |
| L5 | **renderer-on smoke variant** — one smoke run per increment executes with visuals enabled: zero visual-flush errors, no corrupted-rendering events, payload under watermark (the rewrite-plan §6 renderer rollback trigger, exercised rather than assumed) | F9 smoke (renderer-on) |
| L6 | wasted-move-intent rate emitted (gap G-13's threshold metric — added to the F13 schema so the traffic-solver trigger is observable); MMO seg-57 stream as source of truth for the gap-§4 ratios (harness validates behavior only) | seg-57 nightly |

**(c) Fixtures & seams** — MemoryArbiter shell via F3; everything else is new and host-native by construction (the harness is a host Rust crate).

**(d) Implementation guidelines + owned designs**

- **§15.1 Seg-57 registry (F13):** one module defines the schema struct + version; contributors (0004 spiral block, 0005 panic/serialize-skipped, 0001 dangling-ref, 0003/0008 cohesion, 0011 spawn KPI, 0012 market block, 0013 gpl/ops/uptime, 0014 posture audit, gap §4 sub-metrics, G-13 wasted-move) add fields **only** through it; harness decoding is generated from the same struct.
- **§15.2 Scenario library (F9; resolves orphan #1 — 0006's open question answered by enumeration):** scenario = versioned config (map/terrain, spawn placement, opponent, fault-injection schedule, seeds, gate expressions over seg-57 + console events). Catalogue with first consumer: economy-bringup (Inc 0; also the parity-recording substrate), induced-CPU-pressure (Inc 1, §3), forced-reset-with-active-war (Inc 1, §3), raid-vs-enemy-nuker (Inc 1, §5), teardown-deadline family incl. unreachable-room (Inc 4, §7), engagement-vs-opponent (Inc 4, §7), siege/defense@PlayerRaid incl. tower-drain (Inc 4, §7/§11), hauler-extinction wedge (Inc 4/5, §6), contested-expansion (Inc 7), catalyst-less-empire (Inc 7, §12=§11), storage-energy-oscillation (Inc 7, §11/§13), newborn-colony incubation (Inc 7, §6), multi-room-CPU (Inc 7, §14), power-bank-farm (Inc 8, §13), sacked-room-recovery (Inc 8, §14), hostile-operator (Inc 8, §13), market-adversary (post-M3, §12, evidence-gated).
- **§15.3 Intent differ + replay recorder (F4; resolves orphan #5; split in two by adversarial review):** F4 is two artifacts on two timelines. (1) The **intent-sink differ** — a host tool over intent-sink dumps — lands Inc 1 and serves the scheduler gate via **in-process shadow-dispatch**: old and new scheduler run over the *same live tick*, the new side writing to a shadow sink that is never executed; the differ compares the two sinks tick-by-tick. No replay, no recording, no GameView dependency — this resolves the chicken-and-egg the review flagged (the GameView trait is a skeleton at Inc 0–1 while legacy systems still call JS directly, so a recorder riding it would record a thin slice — false confidence — and a real recorder blocks on the class-L refactors). Pre-recorder cutovers (§2's A2 at Inc 3, §6's orchestrator at Inc 4) use the same shadow mechanism. (2) The **recorder proper** (serializes (tick, reads, emitted intents) per system behind a debug feature; pattern proven by `&dyn TransferRequestSystemData`) rides the GameView-realization work — targeted Inc 4–5, hard-required before §8's Inc-6 HaulJob pilot. **The determinism audit (orphan #11, owned here) is staged, not compressed into Inc 1:** Inc 1 needs only ordering-stability of the diffed intent sinks (sort at the sink; fix spawnsystem.rs:284-289 since spawn placement feeds it); the full HashMap/HashSet decision-path sweep completes before the first recording. Recording convention (orphan #14): `runs/recordings/<scenario>/<git-sha>/` — **≥3 pinned reference recordings: economy-bringup (dedup #12), engagement, and induced-pressure** (a pressure run must be in the parity library). Parity is **necessary-not-sufficient**: anything not on those traces passes trivially, so L3 never substitutes for the L5 gates (0015 §1).
- **§15.4 CPU-pressure inducer (F10; resolves orphan #2):** candidates, in preference order — (i) a debug console command that registers a synthetic CPU-burner system with a configurable per-tick cost (deterministic, scenario-scriptable); (ii) `system.setTickDuration` + tick_limit manipulation; (iii) oversized-world load (slow, non-deterministic — fallback only). Decision lands with Inc 1; the gate "harness can induce CPU pressure" is unblocked by (i).
- **§15.5 Thresholds config (F19; resolves orphans #9/#13):** all gate numbers in one reviewed file; a gate may not cite a literal. Per-gate-class seed counts **N** live here too (the single source — 0015 §3 defers to this file; defaults: nightly N=5, increment/pre-deploy N=9, raised per-gate where the failure class warrants). **Gate form (adversarial-review fix):** paired-seed diffs against the stored (scenario, seed, SHA) baseline (F14 already stores it) — "≥k of N matched seeds within tolerance of the same seed's baseline" — never absolute thresholds pinned by the first run they gate (tautologically green at birth); absolutes are reserved for physical bounds (zero panics, engine deadlines) and stay flagged *provisional* until they survive a re-baseline.
- **§15.6 Flake policy (0015's, applied):** new L5 scenarios run non-blocking on the **nightly lane** for a stability window before promotion (the Riot earned-gate pattern) — but a scenario named as an **increment gate** gates immediately in 0015 §3's increment-gate mode (N=9 on-demand seeded runs, paired-diff form); N per gate class comes from F19 (§15.5), not from this bullet.

**(e) Roadmap hooks** — **P0** (everything else's verification depends on it). Registry, scorer, scenario format, F19, harness self-test: Inc 0. Recorder + differ + determinism audit + inducer: Inc 1. Opponent bootstrap: Inc 4. Dual-scoring re-baseline: Inc 8 boundary.
Absorbed: ✓ ADR 0006 in full (components 1–7, seg-57 schema, score terms, gates — extended, not re-decided); ✓ 0006 §Sequencing per-increment metric dependencies; ✓ plan §5 universal gates (as harness features); ✓ review §9 console-event telemetry list (panic, deser, overflow, force-abort, watchdog, squad-out-of-range — schema'd here, emitted by §§1/3/7/9); ✓ gap §4 score schema + sub-metrics + Inc-8 re-baseline; ✓ gap G-13 (metric added; solver revisit stays evidence-gated); ✓ orphans #1/#2/#5/#8/#9/#11/#13/#14 (owners assigned above); ✓ dedup #16 (one schema registry); ✓ rewrite-plan §6 renderer rollback row (IBEX-008 watermark/finite-coord clamp + renderer-on smoke) + ADR 0005's Result-narrowed visual flush — **the one commitment adversarial review found unabsorbed; owned here now**.
Iteration tax: the harness UI/ergonomics and run-artifact pretty-printing; per 0006's non-goals, MMO-CPU fidelity is explicitly not chased.

---

### §16 Construction, repair, links & terminal balancing

*(Added after adversarial review found these had no owning section: construction/repair execution — `repairqueue` is in 0015's tranche-1 list but no section owned it — build/construction systems, link logistics, and the terminal empire-balancer slice (gap G-6).)*

Modules: the repair queue (repairqueue), build/construction execution systems, link transfer logistics, the terminal empire-balancer slice (G-6).

**(a) Critical invariants**
1. Repair-queue priority ordering is stable and finite (no NaN coalescing — the IBEX-046 class).
2. Construction executes only what the §10 plan placed — no free-hand sites.
3. Links never cycle energy (no source↔storage ping-pong); link throughput feeds, never starves, the §5 hauling economy.
4. Terminal balancing respects per-transaction energy cost and exposure caps (shares §12's governor discipline).

**(b) Test plan by layer**

| Layer | Case | Target |
|---|---|---|
| L0 | `repairqueue_ordering` — priority/bucket ordering kernel (pure today; **tranche 1, Inc 0** — named in 0015's tranche-1 list, owner now assigned) | repairqueue |
| L0 | link routing/threshold kernels (fill rules over a link-state DTO) — when the rewrite first touches link logic; until then **declared shell**, pinning gate named: the economy-bringup throughput term | link logistics |
| L5 | construction execution: structures from the §10 plan appear within the bringup window — pinned by the economy-bringup milestone gates + score; the execution glue is class-c shell, deliberately not unit-tested | F9 economy-bringup |
| L0+L5 | terminal empire-balancer (G-6): balancer decision kernel (pure over a holdings DTO) + a multi-room-scenario rider | Inc 7+, with §12's machinery |

**(c) Fixtures & seams** — repairqueue is class **a** today (tranche 1); construction/link execution glue is class **c** shell; the balancer kernel is new — build pure over DTOs (the §12 pattern).

**(d) Implementation guidelines** — repair priority shares §3's `is_finite` validator; the balancer never calls `game::market`/terminal APIs directly (route through §12's snapshot discipline).

**(e) Roadmap hooks** — **P2** overall; the repairqueue kernel is P1-cheap and lands Inc 0. Links/terminal: Inc 7+ or on first rewrite contact.
Absorbed: ✓ 0015 tranche-1 `repairqueue ordering` row (owner assigned); ✓ gap G-6 (terminal empire-balancer slice).
Iteration tax: link/terminal threshold constants and repair target prioritization weights — score- and scenario-pinned only.

---

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Per-component plans keyed to the purity map + shared infra registry (chosen)** | absorbs every existing ADR commitment exactly once; cheapest-first (Tranche 1 is pure-today); names the unowned infra | a long document to maintain; needs re-sync when ADRs change |
| wasm-executed tests (custom `runner` / wasm-bindgen-test) | tests the real target triple | runner complexity, experimental coverage, no debugger; host runtime boundary already proven safe (probe) — wasm execution buys almost nothing for pure kernels |
| `screeps-server-mockup` in-process server | lighter than Docker; tick-stepped | stale (2020), JS-side, separate from the real deploy path; 0006 already chose launcher+Docker — keep mockup as a fallback note only |
| Unstable `per-package-target` to fix the wasm default | plain `cargo test` works | nightly-unstable, awkward in workspaces (cargo #11779 class issues); the alias is one line and boring |
| Mock the entire game API (trait-per-type over screeps-game-api) | everything host-testable immediately | enormous surface, permanent maintenance tax, fidelity drift; contradicts 0015's kernel/shell split — DTO seams at decision boundaries are strictly cheaper |
| Test everything incl. mission/strategy bodies with unit tests | maximal coverage on paper | brittle by construction on emergent behavior; violates the operator's fast-iteration requirement; review §9 explicitly forbids it |

## Consequences

**Positive** — every validation promise in the corpus now has exactly one owning test plan, one layer, one increment, and a named fixture (the renderer rollback row was the one dropped commitment caught by adversarial review — now owned by §15); the three highest-fan-in orphans (replay recorder, old-snapshot corpus, scenario library + pressure inducer) have owners and land-dates; Tranche 1 (~60 kernel tests, all class-a, zero production edits) is startable the day the `test-host` alias lands; duplicated gates are collapsed (one forced-reset test, one HaulJob parity harness, one pressure scenario, one cohesion metric).

**Negative / risks** — the corpus stays **scenario-promissory**: roughly half the commitments resolve to L5 scenarios whose substrate (F8–F11) is real engineering; if F4 (recorder) or F10 (inducer) slip, Inc 1's and Inc 6's gates slip with them — they are on the critical path and flagged as such. The CreepState DTO refactor (M-L) is the one test-enablement item big enough to contend with feature work. Maintaining commitment-absorption discipline requires that future ADRs add validation rows *here* (or to 0015), not as free-floating promises. **Not yet planned (stated rather than omitted):** the Inc-7 third-pass gap items G-3, G-8, G-11, G-16 and G-17 have no validation rows here; each must gain a row (or a named pinning gate) before its implementing increment starts.

**CPU and tick-safety impact** — none in release: all validators are `debug_assert!`/`feature = "validate"` (log-don't-panic under `panic="abort"`); seg-57 emission is the only always-on cost and is owned/budgeted by 0006.

## Incremental Migration Path

All rows are **Breaking: None** — testing work adds no serialized state and changes no production behavior; the few production edits it requires (DTO swaps, determinism fixes, the sink assert) are mechanical and behavior-preserving, and anything riskier (CreepState refactor, RoomData split) is scheduled *inside* its rewrite increment, not as test enablement. Labels per row anyway, per template.

### Consolidated roadmap table (fold into rewrite-plan §3/§5)

| Inc | Infra landing (F#) | Per-component test work landing | Gate it enables | Breaking |
|---|---|---|---|---|
| **0** | F1 host lane; F3 arbiter double; F8 harness core (0006); F9 scenario format + economy-bringup; F13 seg-57 registry; F14 scorer+baselines; F16 rover testkit; F17 features setter; F19 thresholds file; F5 corpus capture begins | **Tranche 1 (~60 L0 kernels, pure today):** §1 round-trip/wrappers; §4 resolver/traversal; §5 priorities iterator; §6 comparator + create_body; §7 formation/heal/damage/bodies; §8 machine_tick; §14 classify_threat/visibility; §15 cascade + scorer + intent-cost table; §10 layer snapshots + bench gate (F15, host-only, "anytime after 0"); **§16 repairqueue ordering** | M0: smoke-run + score baseline; kernels green = first pre-deploy gate | None |
| **1** | F4 **differ + shadow-dispatch** (sink-ordering determinism only — the full audit completes before the first recording, §15.3); F10 pressure inducer; F9 hooks: forced-reset, panic-injection, pressure, reset-with-war, raid-vs-nuker | §3 governor tier kernels + scheduler parity + panic-injection + cadence profiling; §5 Tranche-2 DTO swaps + variant×mode matrix + capacity helper + stub-fixture matcher tests; §8 sink assert (interim); §9 stub-Mission + IBEX-020 fixtures; §13 bank/duo kernels; §6 IBEX-046 + determinism fix; §14 body-DTO swap | M1: pressure scenario passes (progress, no restart loop, serialize never skipped); parity at no-shed | None |
| **2** | F6 fuzzer; F5 corpus runner | §1 full suite = the advance gate (round-trip + corpus + fuzz via F3); forced-reset cost-matrix test; watermark/inflation test; §4 IBEX-049 L4 | M2: schema drift & overflow loud; zero deser failures | None (the Inc itself carries the sanctioned reset) |
| **3** | F7 snapshot fixtures; dangling-ref counter emitting (F13 slot) | §2 A1 kernel + round-trip/recycle fixtures + A2 replay | M3: same-logical-squad round-trip; counter == 0 | None |
| **4** | F11 opponent bot; **F4 recorder proper rides the GameView-realization work (lands Inc 4–5; hard gate: before Inc 6)**; cohesion + watchdog emitters; F9: engagement, teardown family, siege@PlayerRaid, starvation wedge | §7 Manager kernels + parity + engagement/teardown scenarios; §6 orchestrator parity + CRITICAL-vs-renew + group-spawn scenarios; §9 watchdog; §11 defense-boost-latency scenario; §14 posture read-only zero-diff | M4: cohesion above threshold; force-abort within N; teardown deadlines | None |
| **5** | — | §1 corpus as Stage-2 oracle (format-decision bench); §2 stale-ref fixture + counter soak → **repair-pass deletion gate**; §7 no-deleted-variant round-trip + supervisor scenarios; §6 mission-migration parities begin | M5: new format green on all three suites; repair pass deleted with invariant holding | None (Inc carries the sanctioned reset) |
| **6** | F4 reuse (the one HaulJob harness) | §8 CreepState DTO + FSM pilot **byte-identical parity** + visibility-loss fault test; §5 committed-delivery guard | M6: FSM at parity; transient faults → Wait | None |
| **7** | F9: contested-expansion, catalyst-less, oscillation, incubation, multi-room-CPU | §10 RoomGraph/backoff/trunk + shed-first golden; §5 route-sizing scenario; §6 incubation; §11 L2/L4; §12 M2/M3; §13 P1; §14 capacity-model rider | M7: multi-room sheds first, no headroom regression; bench gate standing | None |
| **8** | F12 power seeder (design ≤ Inc 7); F11 hostile-operator driver; dual-scoring re-baseline (gap §4) | §13 P3/P4; §14 WAR/PEACE + sacked-room Recovery (the G-12 gate) | Inc-8 gates per 0013/0014 | None |
| **9 / continuous** | nightly soak lane | L6 standing invariants: death-spiral alarm, dangling-ref zero, Tier-A priority, watchdog, wasted-move threshold (G-13), cohesion trend; cargo-mutants audits of kernel crates + criterion benches (quiet-phase, never per-commit); §13 P5 evidence menu; §12 M4 if evidence demands | rollback triggers (plan §6) as standing assertions | None |

**Sequencing rule (restated from 0015):** L0/L1/L2 gate per-change; L3 gates each cutover; L4/L5 gate increments; L6 runs nightly with seeds that reproduce locally. New L5 scenarios earn gate status on the nightly lane after a stability window (§15.6) — increment-gate mode (0015 §3) gates immediately with N=9 on-demand paired-diff runs. The first **~60** kernel tests (Tranche 1) cost days, not weeks — the inventory's "cheapest path" is Tranche 1 verbatim, and nothing in it touches production code; the next ~40 wait on the two tranche-2 DTO swaps.

**L4-strength honesty (per-increment, per adversarial review):** the L4 host lane composes only what is host-constructible at each increment — Inc 0–1: serializer + arbiter double, matcher (post-swap), rover, governor/spawn kernels; Inc 3: + SquadStore worlds; Inc 4–5: + Squad Manager reconciler, posture kernels, and the realized GameView; Inc 6: + job FSMs under `CreepState` replay (full strength). Until Inc 6 the per-change "seam confidence" lane is partial by construction, and increment gates lean correspondingly harder on L5.
