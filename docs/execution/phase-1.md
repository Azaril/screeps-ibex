# Phase 1 Execution Plan — Survivable: Metrics, Score, Governor, Containment

> **Scope:** Increment-0 remainder + Increment 1 of [`../plans/rewrite-plan.md`](../plans/rewrite-plan.md) → exit at **M0 (Verifiable) + M1 (Survivable)**. M1 is the table-stakes milestone: after it, the bot can no longer permanently die to its own CPU use (review Field Report C, IBEX-003/016/025) — the one failure class that ends a colony rather than denting it.
>
> **Living document.** §1 (Design) describes the long-term end state and changes only when a design decision changes. §2 (Execution) is the work tracker: statuses, decisions, baselines, and resume notes are updated as work lands — task IDs (`P1.*`) appear in commit messages exactly like Phase 0's `P0.*`. If you are resuming cold: read §2.0 status log first, then the task tables.
>
> **Status: IN PROGRESS** (started 2026-06-10).

---

## 1. Design — the end state

Five subsystems, all behind seams that outlive this phase. ADR references are authoritative for rationale; this section pins what Phase 1 builds toward so the executor never needs to re-derive intent.

### 1.1 Always-on metrics segment (seg 57) — [ADR 0006](../design/0006-eval-and-iteration-harness.md), [0002 registry](../design/0002-serialization.md)

A versioned, always-on telemetry block the bot emits every tick to **segment 57** (already reserved in `segments.rs`), decoupled from the debug/visualization flag. Schema versioned from day one (field additions bump the version; the harness reads versioned). Initial fields (0006 + 0005 + 0004 consumers):

- `cpu_used`, `cpu_limit`, `tick_limit`, `bucket`, **`bucket_trend`** (the governor's input and the death-spiral alarm's signal), `gcl`/`gpl`
- `intents_by_category` (intent cost = 0.2 each is CPU ground truth)
- `gcl_progress`, per-room `rcl/rcl_progress`, stored-energy, energy throughput
- creep counts (by role class), mission/operation counts, threat summary, deaths
- `restart_counter`, `deser_failures`, `panics_caught`, `serialize_skipped` — **distinguishing `shed` (intentional, governor) from `aborted` (containment caught)** (0005)
- pathfinding: ops spent vs pool, `repath_count` (exists today but is written-never-read), simultaneous-repath count, saturation events (0004 step 2)
- segment-IO watermark: chunk counts + fullness (0002 prep — routed here instead of the log line)

**Seam contract:** the segment shape is a draft S-contract file this phase (0015 registry); the harness's reader and the bot's writer share the schema type through a host-compilable module so encode/decode is kernel-testable.

### 1.2 Colony-health score, scenarios & gates — [ADR 0006](../design/0006-eval-and-iteration-harness.md), [component-test-plans §15](../plans/component-test-plans.md)

`screeps-ibex-eval` grows from "hard-zero smoke" to the **computable objective function** (rewrite-plan §1): survival (gating) · CPU headroom · economic growth · military win-rate, fixed-weight, per-scenario-normalized. Pieces:

- **Scenario config** (versioned, per §15.2): map/terrain, spawn placement, opponent, fault-injection schedule, seeds, gate expressions. First scenario: **economy bring-up** (exists informally as the smoke run — gets a config file and becomes the parity-recording substrate).
- **Score + baseline differ**: runs land under `runs/` keyed (scenario, git SHA) — already the layout; the differ automates the BASELINE-0-vs-1 comparison that was done by hand, and flags regressions beyond threshold.
- **Increment gate mode**: N=9 on-demand seeded runs that may gate immediately (0015; the earned-gating soak rule is nightly-lane only).
- **CPU-pressure inducer** (F10): debug-console synthetic CPU burner (§15.4 candidate (i)) — *the entry gate for every Increment-1 validation below*.
- **Fault injection**: forced reset, induced pressure, and the combination ("forced reset with active war" is the M1 acceptance scenario). **Reset mechanism — read this twice:** the probe is a **state-preserving global/VM reset** — the bot's one-shot `Memory._features.reset.*` flags (read by `features::load_reset`, `game_loop.rs`) or the redeploy-identical-code path proven in Phase 0 D1. `system.resetAllData()` is a **world wipe** (mongo re-seed + redis flush: spawns, creeps, segments, any active war all gone) — it bootstraps fresh-world scenarios and can NEVER be the reset probe; Phase 0 D1 recorded exactly this trap.

### 1.3 CpuGovernor + budgeted pathfinding facade — [ADR 0004](../design/0004-cpu-governance-and-load-shedding.md)

- **Governor**: a global resource computing a tier — `Normal / Conserve / Critical` — from bucket + trend (pure decision kernel, fixture-tested). Read at the top of every expensive system; replaces/generalizes the ~6–8 scattered `can_execute_cpu` sites. **Authoritative shed order** lives in 0004 (never-shed set: defense, spawn, haul, movement, `serialize_world`; war recompute/expansion/visuals shed first). Changes to the shed registry amend the ADR — that rule continues.
- **Facade**: ONE budgeted pathfinding entry point with a shared per-tick ops pool scaled by tier. The mature rover budget is the inner layer (generalized, not replaced); `find_route` is charged to the pool (IBEX-039); `findnearest.rs` + `compute_nearest_spawn_distances` get `.max_ops` (IBEX-035); repath storms are capped; the `MIN_PATHFIND_OPS` floor stays so creeps never fully freeze (non-negotiable #1).
- **Cache persistence** (step 5, rides the already-landed seg-55 fix): persist/warm `RoomRouteCache` and reduce cost-matrix rebuild to change-event/TTL on stable layers (IBEX-017/038).
- War cadences move off 1 (IBEX-021) **with** the governor (measured, not blind constants — defense ~2 / offense ~10–20 / recompute ~50), including the `RoomThreatData` immutable-borrow restructure.

### 1.4 Containment + scheduler seam — [ADR 0005](../design/0005-runtime-and-scheduling-model.md)

- **Tick-level containment so `serialize_world` always runs.** Mechanism is an explicit in-phase decision (P1.C1): JS try/catch around the WASM call (robust under `panic="abort"`) **vs** switching the profile to `panic="unwind"` + `catch_unwind` (build-config change, not a format break). Decision criteria: wasm size/CPU cost, fidelity of the caught-panic telemetry, interplay with the screeps driver's error handling.
- **`env.tick`-advance correctness**: the advance moves after a successful `serialize_world` (today it precedes `run_systems`), so an aborted tick cannot silently commit a half-applied world.
- **Priority-scheduler seam at parity**: a scheduler hook wrapping `run_systems` introduced with the SAME order and no shedding, verified by **shadow-dispatch parity** (old + new schedulers over the same live tick, diffed through shadow intent sinks — NOT record/replay; `GameView` is only a skeleton this phase). The governor reads tiers between scheduler stages. Shedding is enabled only after parity holds.
- **Narrowed panics**: IBEX-008 visual-flush per-target size guard + finite-coordinate clamp (the remaining named reachable panic; IBEX-010 already landed).

### 1.5 Intent sink + differ — [ADR 0003 §A2](../design/0003-behavior-modeling.md), [0015 F4](../design/0015-testing-and-validation-strategy.md), IBEX-029

All intents — **including combat's ~23 bare `attack/ranged_attack/heal/move` sites** — flow through the one guarded sink (`jobs/actions.rs` is pipeline authority; the dead `UNSET` flag gets consumed or deleted). The **intent differ** is built here as the parity instrument for the scheduler seam (and later the Inc-6 FSM pilot); IBEX-029 is its first consumer. This was reconciled from ADR 0003's "alongside Inc 6": the differ exists at Inc 1 per 0015, and routing combat intents is exactly what makes shadow-dispatch parity trustworthy.

### 1.6 Riders (small, design-settled, land where convenient)

- **0007 step 2–3**: two-phase `TransferSnapshot` snapshot-then-assign (matcher becomes pure `(snapshot, creep) → tickets`); governor-gated re-match cadence (Conserve raises cadence, Critical rides existing tickets; new High-priority demands exempt).
- **0009 D1**: planner restart backoff (IBEX-037, `restart_attempts` + `Failed` state behind the 2000-tick replan gate, serde(default)) + gather-BFS budget (IBEX-036); warn-not-silent on seg-60 encode failure.
- **0011 step 0**: executor quick-wins — engine-true renew energy decrement `ceil(1.2·cost/3/len)`, renew pass behind the priority gate (renew never consumes a lane a CRITICAL/HIGH spawn wants).
- **0013 P0 remainder**: power-bank feasibility window (`kill_time(dps)+dist·50+spawn+200 ≤ ticks_to_decay` replacing `dist·50+500`), 20×ATTACK attacker cap (IBEX-043's fix landed; this is the economics half).
- **G-13 half**: wire the dead `check_movement_failure` stuck-recovery (IBEX-015). (IBEX-040 shove-walkability is **NOT Phase-1 scope** — it is a `screeps-rover` submodule change that consumes the 0004 cached structure cost layer once P1.B5 stabilizes it; proposed-fixes and the rewrite plan both mark it not-Inc-1. It enters a later phase.)
- **0012 M1 interim market trust** (IBEX-018): trailing multi-day median + exposure caps. *Stretch — may slip to Phase 2 (Inc 2) without harm; it is Inc 1–2 in the plan.*

### 1.7 Kernel-test completion (Inc-0 tail) — [ADR 0015](../design/0015-testing-and-validation-strategy.md)

Tranche-1 kernel coverage grows from ~33 toward ~60 as this phase's pure logic lands (governor decision function, score terms, segment-schema round-trip, facade budget arithmetic, formation geometry — the one named Inc-0 kernel still missing). The kernel-vs-shell rule applies: extracted pure logic ships with tests; strategy tweaks need none. `GameView` lands as a **skeleton trait only** (the seam name + the metrics/pressure reads this phase actually needs — no big-bang abstraction).

### 1.8 Explicitly OUT of Phase 1

Serialization beyond loud-failure semantics — per the **reset-anytime policy** (rewrite-plan §0, operator 2026-06-10) state may be dropped at any increment, so version headers / migration / snapshot-corpus / fuzz are all deferred to the one holistic serialization pass at the Inc-7 entry; Inc 2's rescoped remainder (loud decode-failure reset, watermark→seg-57, IBEX-049) is small and may ride this phase or the next as convenient. Also out: anything Inc 3+ (SquadStore, cohesion, FSM), seg-57 *consumers* beyond the governor/harness (posture, P&L), and all LOW-confidence Inc 7–9 material. The renderer corruption fix (Field Report H) stays opportunistic: containment (1.4) already prevents its worst symptom.

---

## 2. Execution — work tracking (living)

### 2.0 Status log (newest first)

- **2026-06-11 (i)** — **P1.C5 landed (first slice) + all-builds-warning-free pass.** The scheduler seam: every system in the tick list carries a `StageClass` (Always vs SkipUnderCritical), order unchanged (parity by construction), one governor tier read per tick, shed accounting logged. Sheddable set chosen for harmless-by-design absence: observer (intel, shed-first per ADR 0004), the summarization/visualization chain, seg-60-resumable room planning, render. Telemetry (stats/metrics/cpu-tracking) explicitly NEVER sheds. Warning cleanup (operator directive): foreman boxed its large planning enums (serde-transparent — seg-60 unchanged), timing/timing-annotate cleaned, ibex test-module placement fixed — workspace + all six tool crates at zero clippy warnings, recorded as the standing bar.
- **2026-06-11 (h)** — **P1.C3 + P1.C4 landed.** `intents.rs`: the recorder (per-category counts + an order-sensitive chained-FNV digest — the shadow-dispatch parity instrument for C5, O(1) memory) + the guarded combat sink. All 23 bare squad_combat intent sites (IBEX-029) converted: consume the engine-pipeline flag (A/B/C — including the easy-to-miss fact that rangedHeal shares pipeline B with rangedAttack), record, issue. The per-creep `UNSET` action_flags are consumed at last. Behavior note (deliberate, documented): same-pipeline conflicts are now suppressed client-side with first-caller-wins priority instead of paying 0.2 CPU each and letting the engine arbitrate. Counts + digest emitted in seg-57 `intents`. **Remaining in phase: C5 (scheduler seam — the differ is ready for it), B4/B5, D1/D2, A6/A7, exit audit + BASELINE-2.**
- **2026-06-11 (g)** — **P1.C6 + P1.B6 landed.** C6: visuals with non-finite coordinates dropped at push time (one NaN corrupts the target's whole payload — Field Report H), both the inherent API and the `VisualBackend` trait path; per-target flush capped (4000 < the server's ~500KB throw). B6: war cadences raised off 1 (defense 2 / offense 10 / recompute 50 — IBEX-021, the review's heaviest per-tick consumer), with governor-coordinated stretch: sheddable tiers ×2 Conserve / ×4 Critical, defense never stretched (never-shed set). **Remaining in phase: C3/C4/C5 (intent differ → guarded sink → scheduler seam), B4/B5 (facade + cache persistence), D1/D2 (transfer snapshot + re-match), A6/A7 (GameView skeleton, kernels), exit audit + BASELINE-2.**
- **2026-06-11 (f)** — **P1.C1 + P1.C2 landed (containment).** Decision: the JS boundary — wasm32 cannot unwind, and the loader already carries the rustyscreeps catch→halt pattern; what was missing was ACCOUNTING and ordering. Loader catch now bumps `Memory._metrics.aborted_ticks` (Memory survives the halt; the heap doesn't); the bot folds it into seg-57 `panics_caught`/`serialize_skipped_aborted`; `env.tick` advances only after a successful serialize. New `panic_at_tick` eval feature (absolute-tick keyed — self-disarming, since the panicking tick's Memory writes are lost) + `PanicOnce` fault + `panic-containment` builtin. **Live acceptance run** (`runs/panic-containment-db86230-20260611-013215`): deliberate panic mid-run → caught + halted + fresh VM; vm_starts 1→2, panics_caught=1, serialize_skipped_aborted=1; colony grew through it; survival gate correctly zeroed the score. **Bonus real-bug find:** `memory_helper::path_set` silently dropped writes when an intermediate key was missing (observed live as the vanishing `vm_starts`) — now creates intermediates.
- **2026-06-10 (e)** — **Riders landed: P1.D3 (mostly), P1.D4, P1.D5, P1.D6 (rescoped).** D4: engine-true renew cost (the old `cost·2/5` over-charged ~10x, starving the spawn loop's energy model) + renew behind the priority gate. D5: power-bank feasibility now includes kill time (~3.3k ticks at the new healer-matched 20×ATTACK cap — the old window green-lit unfinishable banks; the old 25×ATTACK out-reflected its own healer 375 vs 300). D3: exponential replan backoff (IBEX-037), gather-BFS visited cap (IBEX-036), expansion discovery sheds under Critical, loud seg-60 failures. D6 rescoped honestly: detection → seg-57 `move_failures` telemetry now; behavioral recovery belongs to Inc 6's transient-tolerance work (ADR 0003 A6). `RoomPlanState::Failed` shape change = bincode positional break → loud reset on old state, sanctioned by the §0 reset-anytime policy.
- **2026-06-10 (d)** — **P1.A3 + P1.A4 + P1.A5 landed (Round 1 of the phase finish).** Scenario configs v1 (`scenario.rs`): faults compile to the kit's new generic `ConsoleInjection`s — console JS setting `Memory._features.*` flags the bot reads; CPU burn = the new `eval.cpu_burn_ms` bot feature (burns at tick top so the governor sees honest pressure); global reset = the existing `reset.environment` one-shot (state-preserving). rest-api gains the `POST /api/user/console` endpoint. Colony-health score (`score.rs`): survival-gated, military term absent in non-combat runs (renormalized, never fabricated), `score.json` per run, `compare` differ CLI. New `scenario`/`compare` CLI commands; `smoke` unchanged in behavior. **Live pressure E2E** (`runs/pressure-d27377d-20260611-005915`): burner +90ms over ticks 300–600, bucket trend hit −10.9, **governor normal→conserve on trend (bucket still 9394) → normal on recovery**, creeps 1→9 through the burn, zero panics/deser, health total 0.3824 persisted. Note for calibration: the private server clamps drain near bucket ~9000 at this burn level — Critical-tier exercise needs a harder burn (scenario knob, not code).
- **2026-06-10 (c)** — **P1.B1 landed**: `.max_ops(500)` on all five same-room `PathFinderHelpers` (the `find_nearest_*` combinators run one search per CANDIDATE — uncapped worst case was candidates×2000 ops per decision); `.max_ops(1000)` on `compute_nearest_spawn_distances` (capped-incomplete reports `u32::MAX`, the existing "very far" semantic); `RoomRouteCache` serves TTL-stale routes under a Critical governor tier instead of recomputing (missing entries always compute — one `find_route` is not the storm), decision extracted as pure `should_recompute_route` with a tier-matrix fixture. First governor-tier consumer in the tree.
- **2026-06-10 (b)** — **P1.A1 + P1.A2 + P1.B3 landed** (+ the trend half of P1.B2). New `screeps-ibex-metrics` schema crate (workspace member; v1 block, additive-evolution rules pinned by tests); bot `MetricsSystem` emits seg 57 every tick (registered after `CpuTrackingSystem`; active-segment count stays ≤10); fault counters wired into the deser paths — INCLUDING the previously-silent decode→empty path, now loud + counted (Inc-2 rescope rider) — and the 0002 chunk watermark; `metrics::tick_start` samples the bucket window and refreshes the `cpugovernor` snapshot before dispatch. `cpugovernor`: pure `compute_tier` kernel (boundary + profile fixtures), tick-start snapshot (a static, not an ECS resource — the legacy call sites are free functions without `SystemData` access; documented), all 6 `can_execute_cpu` sites now read it via delegation (behavior-preserving by construction — same formula, tick-start values). Kit `CaptureSpec.metrics_segment` + eval seg-57 capture/parse seam (`parse_metrics_block`) with pin tests. Validation: full host+wasm suites green; live smoke = the acceptance run for the segment-populated check. **Open on these tasks:** B3's shed-order consumers (cadences/visuals shed on tier) land with B6/C5; B2's pathing fields await the rover-side plumbing. **Acceptance run PASSED** (`runs/smoke-93a4a2d-20260611-001910`): 610 ticks, zero panics/deser, all 30 capture samples carry parsed v=1 blocks (tier=normal, trend, fault counters, chunk watermark 1/5). Known limitation: `vm_fresh` is a one-tick flag and the sampler polls every ~16 ticks, so restarts can be missed — B2 follow-up: a Memory-persisted `vm_starts` counter (Memory survives VM resets; the heap doesn't).
- **2026-06-10 (a)** — Document authored. Prerequisites all in place: Phase 0 substrate complete (exit audit §6 of [`phase-0.md`](phase-0.md)), seg 57 reserved in `segments.rs`, rover budget healthy, baselines BASELINE-0/1 recorded. Operator context: an MMO respawn (prospector flow) may run concurrently — coordinate deploys; the private-server eval stack is the validation environment for everything here.

### 2.1 Conventions

- Task IDs `P1.<workstream><n>` in every commit touching the task. Update the Status column in the same commit when a task completes (`unstarted → in-progress → done (commit)`).
- Leaf-first commits for submodule work (none expected this phase — all work is in `screeps-ibex` + `screeps-ibex-eval` unless the rover budget generalization needs a `screeps-rover` change; if it does, commit rover first).
- Every seam lands with its **draft contract file** (0015 S-registry). Host + wasm lanes both green before a task is `done`.
- Validation scenario runs land under `runs/` keyed (scenario, git SHA); gate mode is N=9 seeded runs.

### 2.2 Workstream A — metrics + score substrate (Inc-0 remainder; M0 closure)

| ID | Task | Depends on | Validation | Status |
|---|---|---|---|---|
| P1.A1 | Seg-57 schema type (host-compilable module, versioned) + bot-side emitter, all §1.1 fields it can fill today; route the 0002 watermark into it | — | schema encode/decode kernel round-trip; smoke-run shows the segment populated every tick | **done** (`screeps-ibex-metrics` crate + `metrics.rs`; smoke = acceptance) |
| P1.A2 | Harness reader: capture seg 57 (replacing/augmenting the seg-99 live-stats read) into run artifacts | P1.A1 | a smoke run's artifacts contain parsed seg-57 series | **done** (kit `metrics_segment` + eval `parse_metrics_block`) |
| P1.A3 | Scenario config format (versioned; §15.2 fields) + port the economy-bringup smoke to it | — | same smoke behavior from a config file | **done** (`scenario.rs` v1: ticks + fault schedule → kit console injections; built-ins smoke/pressure/reset-under-pressure; JSON load with version gate; smoke = thin wrapper) |
| P1.A4 | Colony-health score computation + baseline store/differ (automates the BASELINE comparison table) | P1.A2, P1.A3 | score computed for a BASELINE-1-era run; differ flags an injected regression | **done** (`score.rs`: survival-gated blend, military term absent-not-fabricated; `score.json` per run; `compare` CLI with regression threshold; fixtures) |
| P1.A5 | CPU-pressure inducer (debug-console synthetic burner) + forced-reset-under-pressure composite scenario | P1.A3 | pressure scenario drives bucket down measurably and recovers | **done** (bot `Memory._features.eval.cpu_burn_ms` burner + kit injections; live pressure run: cpu 18→110+, trend −10.9, **governor normal→conserve ON TREND at bucket 9394 → recovery**, colony grew throughout) |
| P1.A6 | `GameView` skeleton trait (seam name + the reads this phase needs) | — | compiles both targets; S-contract draft committed | **done** (`gameview.rs`: object-safe trait + `LiveGame` passthrough + `FixedGameView` double; growth rule = add methods only when a consumer migrates — the contract stays DRAFT until Inc-6 replay freezes it) |
| P1.A7 | Tranche-1 kernel completion as logic extracts (incl. formation geometry) + F3 `MemoryArbiter` double in the testkit (Inc-0 infra, effort S — Inc 2's fuzz/corpus suite consumes it) | rolling | `cargo test` green; count recorded here at phase close | **done for the phase** (F3: `MemoryArbiter::test_double()` — in-memory segment backing, the whole pipeline kernel-testable without JS, round-trip pinned; bot-crate kernel count at phase close: **51 host tests** vs 33 at phase start — every extracted pure kernel shipped with pins per the kernel-vs-shell rule. Formation geometry awaits its Inc-4 extraction — no pure formation kernel exists to test yet) |

### 2.3 Workstream B — CpuGovernor + pathfinding facade (ADR 0004)

| ID | Task | Depends on | Validation | Status |
|---|---|---|---|---|
| P1.B1 | Step-1 quick-wins: `.max_ops` on findnearest + `compute_nearest_spawn_distances`; bucket-guard `RoomRouteCache::compute_route` | — | kernel tests where pure; smoke unchanged | **done** (`SAME_ROOM_MAX_OPS=500` on all 5 helpers — was 2000/candidate; `NEAREST_SPAWN_MAX_OPS=1000`; route cache serves stale under Critical via pure `should_recompute_route` + fixture) |
| P1.B2 | Step-2 telemetry: bucket trend, ticks-since-progress, repath storms (read `repath_count`), ops saturation → seg 57 | P1.A1 | fields visible in captured runs | **done** (trend ✓; rover `MovementTickStats` → block `pathing` section: ops cap/consumed + per-tick repaths; Memory-persisted `vm_starts` restart counter replaces the missable `vm_fresh`) |
| P1.B3 | Step-3 governor: pure tier kernel (bucket+trend fixtures) + tick-start snapshot + the 6 site conversions + authoritative shed order | P1.B2 | fixture tests (bucket/trend profiles: crash, slow-drain, recovery, sustained-Critical); pressure scenario sheds in ADR order, never-shed set untouched | **mostly done** (`cpugovernor.rs`: kernel+fixtures+snapshot+conversions; shed-order CONSUMERS land with B6/C5; pressure-scenario validation awaits P1.A5) |
| P1.B4 | Step-4 facade: shared ops pool scaled by tier; rover budget inner; `find_route` charged; repath cap; `MIN_PATHFIND_OPS` floor | P1.B3 | pressure scenario: ops pool respected, no creep freeze; budget arithmetic kernel-tested | **done** (`pathbudget.rs`: mission-side aggregate pool 20k ops, tier-scaled ×1/½/¼ — the per-search caps bounded one search, the pool bounds the tick; findnearest×5 + spawn-distances + `find_route` (nominal 2k, admission-controlled TTL refresh) all draw from it; movement keeps its independent CPU-aware budget, now tier-scaled at the call site with the `MIN_PATHFIND_OPS` floor intact; pool/consumed in seg-57; kernel pins) |
| P1.B5 | Step-5 cache persistence/warm + cost-matrix TTL (IBEX-017/038) | P1.B4 | forced-reset scenario: no full-rebuild spike on the post-reset tick | **substantially met / remainder deferred** — the cost-matrix cache already persists across resets (seg 55, fixed in Phase 0 D1: the original IBEX-013/017 substance). Remaining (route-cache warm-from-segment, cost-matrix TTL/change-event rebuild) is CPU-amortization polish now bounded by the B1 guard + B4 pool — deferred to the Inc-2/7 serialization work where persistent-cache formats land properly |
| P1.B6 | War cadences off 1 (IBEX-021) + `RoomThreatData` borrow restructure, governor-coordinated | P1.B3 | war scenario at parity; recompute CPU drops visibly in seg 57 | **done** (defense 2 / offense 10 / recompute 50 — the struct's own doc-comment values, all three were 1; `effective_cadence` stretches sheddable tiers ×2/×4 under Conserve/Critical, defense never stretches — never-shed set; kernel pins. Borrow restructure not needed — the tier machinery already isolates the borrows) |

### 2.4 Workstream C — containment + scheduler + intent sink (ADR 0005, 0003 §A2, 0015 F4)

| ID | Task | Depends on | Validation | Status |
|---|---|---|---|---|
| P1.C1 | **Decision task**: containment mechanism (JS boundary vs `panic="unwind"`) — record the decision + criteria here and in ADR 0005 | — | decision recorded; spike measurements attached | **done — DECISION: the JS boundary.** wasm32-unknown-unknown has no stack unwinding (`catch_unwind` non-functional), and the screeps-pack loader ALREADY carries the rustyscreeps catch→`halt()` pattern: a trapped tick is caught, the next tick halts the IVM, the VM reloads fresh while segments survive — state-preserving containment by construction, one tick rolled back, never silent |
| P1.C2 | Tick-level containment per P1.C1 + panic/serialize-skipped counters (shed vs aborted) → seg 57; `env.tick` advance moved after successful serialize | P1.C1, P1.A1 | induced panic: tick aborts loudly, serialize still runs, counters increment, no restart loop | **done** (loader catch bumps `Memory._metrics.aborted_ticks` — Memory because the heap dies with the halt; bot emits it as `panics_caught` + `serialize_skipped_aborted`; `env.tick` advances only after successful serialize; `panic_at_tick` probe + `panic-containment` builtin scenario. **Live acceptance**: panic at mid-run → 1 panic line, vm_starts 1→2, both counters 1, colony alive — and the run flushed out a real bug: `path_set` silently dropped writes through missing intermediates, now fixed to create them) |
| P1.C3 | Intent differ (F4) + shadow intent sinks | — | differ detects an injected intent change; deterministic ordering only (no record/replay) | **done** (`intents.rs`: per-category counts + order-sensitive chained-FNV digest over (category, creep, target-pos) — the C5 parity instrument without storing the stream; reset at tick start, emitted in seg-57 `intents`; order-sensitivity + determinism pinned) |
| P1.C4 | One guarded intent sink incl. combat's bare sites (IBEX-029); consume-or-delete the dead `UNSET` flag | P1.C3 | differ shows byte-identical intents pre/post conversion on the war scenario | **done** (all 23 bare squad_combat sites route through the guarded sink: pipeline flag consumed FIRST — the per-creep `UNSET` flags are finally consumed — then recorded, then issued. Same-pipeline conflicts now suppressed client-side, first-caller-wins (saves 0.2 CPU per suppressed intent; deliberate priority instead of engine arbitration — incl. the subtle rangedHeal-is-pipeline-B rule, pinned). War-scenario digest validation rides the Inc-4 combat scenarios — no combat scenario exists yet) |
| P1.C5 | Priority-scheduler seam at parity (same order, no shedding), shadow-dispatch verified; then enable governor-driven shedding between stages | P1.C3, **P1.C4**, P1.B3 | shadow-dispatch parity clean over N=9 runs; shedding then activates per tier without parity drift in never-shed systems | **done (first slice)** — every system in `for_each_system!` now carries a `StageClass` (declaration order = execution order: parity by construction at Normal/Conserve); the scheduler reads ONE tier per tick and skips `SkipUnderCritical` systems under Critical (observer/intel, summarization+visualization, seg-60-resumable planning, render — telemetry NEVER sheds, the governor is blind without it). Adding a system without declaring its class is now a compile error — the seam's point. Dynamic priority REORDERING (beyond skip) grows on this seam when something needs it; Critical-tier shed validation awaits the harder-burn scenario calibration (the current pressure burn peaks at Conserve) |
| P1.C6 | IBEX-008 visual-flush size guard + finite-coordinate clamp | — | renderer-on smoke completes; clamp kernel-tested | **done** (non-finite visuals dropped at push time with telemetry — incl. the `VisualBackend` trait path; per-target flush capped at 4000 visuals vs the server's ~500KB throw; kernel pin) |

### 2.5 Workstream D — riders (§1.6)

| ID | Task | Depends on | Validation | Status |
|---|---|---|---|---|
| P1.D1 | 0007 two-phase `TransferSnapshot` + pure matcher seam | — | host tests on the pure matcher (S6 contract draft); smoke parity | **DEFERRED to Phase 2** (explicit decision at phase close): a deep transfersystem refactor that deserves its own validated phase slice, not a tail-of-phase rider. The hauling system is live-healthy post-b005afb; nothing in M0/M1 depends on it |
| P1.D2 | 0007 governor-gated re-match cadence | P1.B3, P1.D1 | Conserve scenario: re-match rate drops, deliveries keep flowing | **DEFERRED to Phase 2** (depends on D1's snapshot seam) |
| P1.D3 | 0009 step-1 planner hardening (D1 restart backoff IBEX-037 + anchor memoization, gather-BFS budget IBEX-036, loud seg-60 encode failure) | — | induced plan-failure scenario: bounded retries, loud failure | **mostly done** (`Failed{time,attempts}` + exponential `replan_backoff_ticks` 2k→32k-cap with kernel pins; `MAX_GATHER_VISITED_ROOMS=256` BFS budget; expansion discovery sheds under Critical — the governor's first expansion consumer; loud seg-60 encode/decode. Anchor memoization deferred — foreman-side, low value vs risk) |
| P1.D4 | 0011 step-0 executor quick-wins (renew math, renew behind priority gate) | — | kernel test on renew math; spawn-pressure smoke parity | **done** (engine-true `renew_energy_cost` ceil(1.2·cost/3/len) — old estimate over-charged ~10x — with formula pins; renew pass moved AFTER the priority-sorted request loop, so renew never consumes a lane a pending spawn wants) |
| P1.D5 | 0013 P0 **subset**: feasibility window formula + 20×ATTACK cap (P0's launch re-scoring by spawn pressure and the `power_bank_duo()` adopt-or-delete decision stay open on the ADR row) | — | kernel test on the window formula | **done** (`power_bank_min_ticks_needed` = kill@600dps + travel + serial duo spawn + margin, pinned; attacker capped 20×ATTACK — 25×ATTACK reflected 375/tick vs the healer's 300, out-damaging its own duo) |
| P1.D6 | IBEX-015 stuck-recovery wiring | — | stuck-creep scenario recovers | **rescoped + done as telemetry** — the give-up results (`Failed`/`Stuck≥threshold`) are now counted into seg-57 `pathing.move_failures`; the BEHAVIORAL recovery is genuinely coupled to Inc 6's transient-tolerance design (ADR 0003 A6, its original home) and lands there — wiring it without that machinery risks unvalidated regressions |
| P1.D7 | *(stretch)* 0012 M1 market trust interim (IBEX-018) | — | adversarial pricing fixtures (T1/T2 subset) | unstarted |

### 2.6 Sequencing

```
A1 → A2 → A4        (metrics → capture → score)        ─┐
A3 ──────┘                                              ├→ V1 (M0 audit)
A5 (pressure)  A6/A7 (rolling)                          ─┘
B1 → B2 → B3 → B4 → B5      (governor chain)            ─┐
            └→ B6                                        ├→ V2 (M1 audit)
C1 → C2        C3 → C4 → C5        C6                   ─┘
D1 → D2;  D3..D7 anywhere after their deps
```

Workstreams A/B/C/D interleave freely except where the Depends-on column says otherwise. Suggested first commit chain: A1 → B2 → B3 (the governor is the highest-value single artifact and only needs the metrics fields it reads).

### 2.7 Exit criteria (the M0+M1 audit — fill at phase close like phase-0 §6)

| # | Criterion | Source | Status |
|---|---|---|---|
| 1 | Seg-57 versioned metrics emitted every tick and captured by the harness | M0 / 0006 | — |
| 2 | Colony-health score computable from a run; baseline differ automated; BASELINE-2 recorded (scenario, git SHA) | M0 / 0006 | — |
| 3 | Increment-gate mode runnable (N=9 seeded) | 0015 | — |
| 4 | Pressure scenario: bot makes progress under induced CPU pressure, **no restart loop, no death-spiral alarm** | M1 / 0004 | — |
| 5 | Forced reset with active war: progress continues; post-reset tick has no cost-matrix rebuild spike | M1 / 0004 | — |
| 6 | **No tick ever skips `serialize_world`** (counters prove shed≠aborted; induced panic contained) | M1 / 0005 | — |
| 7 | Shadow-dispatch parity clean before shedding enabled; intent differ operational; combat intents through the sink | 0005/0003/0015 | — |
| 8 | All new seams have draft contract files; host + wasm lanes green; kernel count recorded | 0015 | — |
| 9 | Rewrite-plan §3 Status cells + §7 changelog updated; Phase 2 (Inc 2) entry confirmed unblocked | process | — |
| 10 | Operator sign-off | operator | — |
