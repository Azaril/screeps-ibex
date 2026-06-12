# ADR 0016 — Visualization & HUD ("Glance HUD")

- **Status:** Proposed
- **Date:** 2026-06-11
- **Deciders:** operator (acceptance pending, EP-10.7)
- **Related:** IBEX-008 (visual corruption/limits), IBEX-024 (decompose `visualization.rs`), IBEX-025, Field Report H ("renderer corrupts all rendering"), ADR [0004](0004-cpu-governance-and-load-shedding.md) (visuals shed first), ADR [0005](0005-runtime-and-scheduling-model.md), ADR [0015](0015-testing-and-validation-strategy.md), EP-1.1/1.5, EP-2.4/2.6/2.10, EP-3.2/3.4/3.5, EP-4.1/4.2/4.6, EP-5.4, EP-6.1/6.2/6.13
- **Mockups:** [L2 room view](assets/0016-hud-l2-room.svg) · [L1 ambient / L3 firehose](assets/0016-hud-levels.svg)

## Context

The current overlay ([visualization.rs](../../screeps-ibex/src/visualization.rs), ~1500 lines, the IBEX-024 decomposition target) renders **rosters as paragraphs**: every operation, mission, job, and spawn request becomes text lines in solid panels stacked down the room edges. The operator's verdict: messy, hard to read, obscures the room — but the *data* is highly valuable. Specific, file-grounded problems:

1. **Text firehose.** Each line is one `text` primitive (~31 B + content, the most expensive primitive class). The Jobs panel alone is one line per creep (15–50 texts/room); panels regularly cover the left third of the room at 0.96 opacity.
2. **Global layer duplicated per room.** `draw_global_layer` is drawn into the global target *and re-drawn into every room* (visualization.rs:1405) — ~200–350 redundant primitives/tick at 10 known rooms. This is the remaining open item from IBEX-008.
3. **Every known room pays.** `AggregateSummarySystem` creates an entry for *every* `RoomData` entity (visualization.rs:488–491); scouted/remote rooms render skeleton panels nobody benefits from.
4. **String churn.** Each summary line is heap-allocated ~5× between producer and draw call (`format!` → component clone → `to_lines` clone → join/split → per-text clone), every primitive clones a style struct, and each creep is JS-resolved twice (name, then room).
5. **All-or-nothing shedding.** ADR 0004 pins visuals as shed-first, but the only degradation today is binary (whole pipeline on/off) plus a blunt tail-truncation at `MAX_VISUALS_PER_TARGET = 4000` that eats whatever happens to be last.
6. **Dead flags, ungated work.** The `pathing/transfer/room/military.visualize` flag families have no draw code behind them (movement path visuals have *no off-switch below the master flag*), while `VisibilityVisualizationSystem` snapshots and sorts its queue every tick regardless of any flag.
7. **Toggle data loss.** Turning `visualize.on` off removes `StatsHistoryData` and `CpuHistory` from the world (game_loop.rs:810–815), permanently losing accumulated history.

**Engine ground truth** (verified against the cloned engine, driver, the path-patched screeps-game-api 0.23.1 fork, the `@screeps/backend` deployed in the running eval container, the Steam client's bundled sources, and live Redis buffers — replay harness preserved at [`tools/viz-repro/`](../../tools/viz-repro/README.md)): visuals are *not* intents — every call appends JSON to a per-target string buffer inside our timed VM (`console.addVisual`: `JSON.stringify(data)+"\n"` for objects; **string payloads are appended verbatim with NO newline added**, console.js:65); the driver ships each `(user, target, tick)` buffer to Redis with a short TTL; the **backend concatenates the `""` all-rooms buffer in front of every room's websocket payload with no separator**, while the `"map"` target ships on its own channel; the client renders it. Therefore **CPU cost = building objects + stringification, all on our clock**, and visuals persist exactly one tick. Limits: **512,000 B per room target** (the `None`/`""` target is its own bucket), **1,024,000 B for the map target**. Theoretical per-primitive wire sizes: circle 24 B, rect 36 B, line 42 B, text 31 B + content, poly ~26 B + ~10 B/point; a style block adds ~15–55 B — but **live lines measure ~150–250 B** because `serde_wasm_bindgen` widens f32→f64 (`0.06f32` → `0.05999999865889549`), which also means the current `MAX_VISUALS_PER_TARGET = 4000` count cap does *not* guarantee staying under the byte limit (4000 × ~200 B ≈ 800 KB). `draw_multi` is an unbatched loop — per visual, a field-by-field `serde_wasm_bindgen` boundary crossing plus one `addVisual` call plus a JS-side re-stringify (~2–5 µs/primitive); the verbatim-string path is the highest-leverage cost lever available. There is **no watcher-detection API** (confirmed unimplemented feature request): feature flags *are* the interaction model, and the overlay is non-interactive by construction.

**Failure-mode ground truth** (root-caused end-to-end 2026-06-11; supersedes the IBEX-008-era account, which conflated two distinct modes):

- **Mode A — write-side abort (loud, volume-dependent, transient).** Exceeding a byte limit **throws from inside `addVisual` before appending**; the Rust binding lacks `#[wasm_bindgen(catch)]`, so the throw aborts the wasm tick. Because `draw_claim_map_visuals` draws unbuffered in `RenderSystem` *before* `ApplyVisualsSystem` flushes room visuals, one mid-draw throw blanks ALL rendering for the tick. Signature: per-tick error spam, panic/abort counters move. **A single small primitive can never trigger this mode.** Self-heals when volume drops.
- **Mode B — read-side line poisoning (silent at the bot; the corruption mode).** The bot is healthy and under-cap, but one unparseable line — or one parsed object whose client draw path throws — blanks rendering at the *client*. Two flavors, both verified in client source and replayed in the harness: **B1 (`""`/room, transient, all rooms)** — the room view does `clearRect` then unguarded per-line `JSON.parse`; since the backend prepends the `""` buffer to every room's payload, one bad `""` line (or a missing trailing newline at the concat boundary itself) blanks essentially all visuals in all rooms, recovering on the next clean tick. **B2 (map, PERSISTENT)** — the map view's unguarded parse + `validate()` that never checks `s` + clear-then-draw with unguarded `s` derefs runs in an rxjs subscription with no error handler: one bad line or one `s`-less text **permanently kills map visuals until client reload**. B2 produces a *masquerade*: after one bad tick, every primitive added later falsely appears to "break all rendering" — the leading explanation for the observed single-primitive symptom. Verified injectors: (i) a string payload without a trailing `"\n"` (merged `}{` line); (ii) map text without `s` (the upstream screeps-game-api <0.20 bug, fixed in 0.20.0 / PR #481 — every visual failing to render from one style-less primitive); (iii) **non-finite f32 in a *style* field** → JSON `null` → the room view's `font: null.replace` TypeError aborts the whole frame (the P1.C6 clamp covers *coordinates only*; style fields are unguarded today).
- **Mode C — silent degradation.** ES-`Map`-serialized lines become `"{}"` under `JSON.stringify` — *valid* JSON that both clients skip/filter: invisible visuals, no errors (reachable only via a serde regression or `#[serde(flatten)]`, not the current API). Plus the f32-widening byte bloat above.

The earlier "determined root cause" claim (limit-throw + non-finite coords) is **retracted**: the limit-throw is real but is Mode A and cannot explain a single-primitive corruption; Field Report H's shape — bot healthy, one world primitive, all rendering gone — is Mode B, and its specific injector remains **unproven** (no emitted payload was ever captured; the incident may predate the current toolchain, and the upstream PR #481 bug class plus the B2 dead-subscription masquerade are the strongest candidates). Verified clean today: all 10 current primitive paths emit wire JSON property-identical to the engine's own JS producers (source diff + wasm repro through the real serde path + live Redis buffers, including the in-production `""` ops panels); the `to_value()` panic and packed-`Position` vectors stay refuted. The `visualization.rs:1255` comment ("string-based to avoid serde_wasm_bindgen corruption") is **false as a description of the code it annotates** — the map path goes straight through `serde_wasm_bindgen` — and is corrected at M0.

Constraints carried in from the corpus: visuals are shed-first under Critical (ADR 0004) and must never be the home of telemetry (seg-57 metrics stay decoupled, ADR 0004 §4 / EP-4.3); the flush path can never throw or abort a tick (EP-3.4/3.5, P1.C6 clamps); state is Resources, never statics (EP-1.1); zero cost and zero panic surface when the flag is off; reset-anytime applies (EP-5.1 — nothing here is serialized anyway, EP-5.4).

## Decision

Replace the panel renderer with the **Glance HUD**: an exception-first, edge-railed, level-gated overlay whose primitives are serialized **at emit time** into per-target wire-format string buffers and submitted with **one `console::add_visual` call per target**. Show **exceptions, not rosters**; move data **to where it lives** (world-anchored badges); preserve every current datum **by translation, not deletion** (the complete mapping is Appendix A). Seven pipeline systems collapse into three over a typed, string-free model.

### D1. Visual design

**Coordinate contract (50×50 room space).** Hard rule, enforced in code by the Painter's region clamp: at L2 and below, no screen-anchored primitive may enter the **center 40×40** (`5.0 ≤ x ≤ 45.0 ∧ 5.0 ≤ y ≤ 45.0`). The center belongs to the game; only world-anchored badges (attached to real `Position`s) may appear there. L3 — a deliberately-enabled per-domain debug firehose — may intrude (expanded tables), and that trade is explicit.

**Regions** (all underlays ≤ 0.55 opacity — the room stays visible *through* the HUD):

| Region | Extent | Target | Level | Content |
|---|---|---|---|---|
| Empire strip | x ∈ [1, 49], y ∈ [0.2, 1.4] | `None` (all rooms, serialized **once**) | L1+ | CPU bar colored by governor tier, bucket, creep count, GCL, owned/max rooms, ops count; right-aligned **alert badges** |
| Global ops rail | x ∈ [44.8, 49.5], y ∈ [4, 20] | `None` | L2+ | One row per operation (glyph · phase · target); L3: full audit detail (below) |
| Room header strip | x ∈ [1, 49], y ∈ [1.6, 2.8] | per room | L2+ | `W7N3 RCL7`, RCL bar, energy bar + value, 12-point storage sparkline + ▲/▼ trend text, `H3` hostile badge, intel-age dot |
| Left rail (econ) | x ∈ [0.5, 5], y ∈ [4, 45], rows 0.62 | per room | L2+ | Exception rows: state dot + glyph + ≤10-char label; nominal population collapses to one quiet row (`OK 14m 27j`) |
| Right rail (military) | x ∈ [44.8, 49.5], y ∈ [21, 45] | per room | L2+ | Squad chips (`a 3/4 W5N8`, border red below retreat), defense posture chip, threat badge |
| Bottom strip | x ∈ [1, 49], y ∈ [46, 49.5] | per room | L2+ | Spawn chips (max 6 + `+N`), transfer micro-bars (max 4 by \|imbalance\|); balanced room = one green dot |
| World annotations | anchored to `Position`s, offset (0, −0.85), wall-adjacent badges mirror via property-tested `offset_flip` | per room | per item | Badges at sources/controller/spawn/extractor, squad member HP bars + role glyphs, nuke markers, stuck/failed/immovable markers |
| Map layer | `MapVisual` | map | L2+ intel | Claim candidates (capped, blockers protected), intel badges (owner/reservation glyph + age) for known rooms |

**Disclosure levels** — flags are the interaction model; effective level per domain = `min(visualize.level, visualize.<domain>)` over domains `{econ, military, intel, infra, pathing}`:

- **L0 Off** — resources absent, zero cost; `visualize.on=false` stays the kill switch and rollback path.
- **L1 Ambient** — empire strip + safety-critical world badges only (nuke impact, active attack). ≤ 28 primitives empire-wide. **The L1 alert contract: anything operator-actionable surfaces here** as a typed alert, pinned in code: `WarActive, NukeInbound, SafeModeActivated, SpawnStall, HostileInOwned, CpuCritical, PathingStorm, HudClipped`. (`PathingStorm` reads the always-on ADR 0004 repath-storm telemetry, so death-spiral onset is visible at L1 *before* anyone enables L3 path visuals; `HudClipped` surfaces the EP-3.2 dropped-primitive counters.)
- **L2 Panels** (default operating mode) — all regions, in **render-eligible rooms only**: `owned ∪ focus ∪ alert`, where `focus` is the operator-settable `visualize.focus` room list (the remote-inspection workflow) and alert rooms are capped at `MAX_ALERT_ROOMS = 2` by threat score and render **header + badges only** (~15 prims) so an invader sweep cannot DoS our own tick. Scouted rooms cost ~zero; their intel lives on the map layer.
- **L3 Debug** — per-domain firehose, enabled deliberately, one domain at a time. Econ: full per-creep census (paginated deterministically, `page = (game_time / 50) % pages`, indicator `p2/3`, ≤ 56 rows/page), full spawn table (numeric priority · cost · description), full transfer table (every resource, numeric supply/pending/demand/pending + `Any` generic-demand row) in an expanded bottom region (y ∈ [40, 49.5]). Military: member names + per-member orders. Intel: visibility queue top-15 (`W8N2 p90 obs`), CPU histogram. Infra: room-plan ghost (cached layer). Pathing: full path polys + anchor lines.

**Glyph / color / typography.** Single-char ASCII glyph alphabet in a `const` table (`M` miner, `L` hauler, `B` builder, `U` upgrader, `S` scout, `D` dismantle, `R` reserve, `C` claim, `Q` squad, `X` failed; ASCII-first — unicode variants behind a flag, untrusted across client fonts). State dots: `#3fb950` ok / `#d29922` warn / `#f85149` bad / `#8b949e` idle / `#8b5cf6` unknown-intel. **The glyph carries the noun; the color carries the verdict.** Monospace text, 0.45 body / 0.55 strips; GitHub-dark palette retained (`#0d1117` underlays, `#58a6ff` accent, `#c9d1d9` text). HP bars are block-character text glyphs (`▮▮▮▯▯` / `###--`), one primitive, drawn **only when damaged**.

**Exception predicate.** A mission/job is an exception if: failed, stalled, zero-creep when count expected, **or state unchanged > `STATE_AGE_EXCEPTION_TICKS`** — the state-age clause exists specifically for the operator-observed lifecycle-hang mode where a creep idles in a plausible `Wait` state. Constants are named and operator-tuned (EP-4.6); the L3 full census is the always-available fallback when the predicate is wrong.

### D2. Code design

**Module tree** (executes IBEX-024; `visualization.rs` is deleted at M6): `src/hud/{mod,model,collect,layout,paint,wire,render,glyphs,cache}.rs`. `layout.rs` kernels are pure (`fn(model, Region) -> placements`, JS-free below the seam, EP-6.1/6.2, property-tested: rows never escape regions, center exclusion holds, overflow math). `glyphs.rs` is immutable `'static` data (EP-1.1-compliant like screeps-visual).

**Core types** — Resources, never statics; persistent across ticks, **cleared with capacity retained** at tick start (the `ClearVisualizationSystem` slot becomes `HudResetSystem`), eliminating the current re-grow/realloc churn (~50 KB/tick). Inserted when the flag turns on, removed when off; presence-gating via `Option<>` SystemData is preserved unchanged.

```rust
pub enum HudLevel { Off = 0, Ambient = 1, Panels = 2, Debug = 3 }
pub enum Domain { Econ, Military, Intel, Infra, Pathing }

/// Rebuilt each tick from Features. Present only when master >= Ambient.
pub struct HudConfig {
    master: HudLevel,
    domains: [HudLevel; 5],                    // min(master, flag), precomputed
    pub focus_rooms: SmallVec<[RoomName; 4]>,  // visualize.focus — remote inspection
}

/// Typed, string-free below L3. SmallVec-inline rows; ArrayString labels (no heap).
pub struct HudModel {
    pub empire: EmpireStrip,                   // cpu/bucket/gcl/ops + SmallVec<[Alert; 8]>
    pub global_ops: SmallVec<[OpsRow; 12]>,    // the global ops rail (None target)
    pub rooms: FnvHashMap<RoomName, RoomHud>,  // render-eligible rooms ONLY
}
pub struct HudRow { pub dot: StateDot, pub glyph: Glyph, pub label: ArrayString<10> }
pub struct WorldBadge { pub x: u8, pub y: u8, pub glyph: Glyph, pub count: u16, pub dot: StateDot }
```

**Producer API.** The string-returning `summarize()`/`describe_operation()` family is replaced by one typed hook per dispatch surface (default no-op; each concrete type writes its own rows — no `is_X` bleed, EP-2.4):

```rust
fn hud(&self, ctx: &MissionHudContext, w: &mut RoomHudWriter) {}   // missions/jobs
fn hud_global(&self, ctx: &OperationHudContext, w: &mut GlobalHudWriter) {} // operations

impl RoomHudWriter<'_> {
    pub fn level(&self) -> HudLevel;          // producers skip detail below level
    pub fn econ_row(&mut self, dot: StateDot, glyph: Glyph, label: fmt::Arguments<'_>);
    pub fn badge(&mut self, pos: Position, glyph: Glyph, count: u16, dot: StateDot);
    pub fn alert(&mut self, a: Alert);        // bubbles to the L1 empire strip
    // squad_chip / spawn_chip / transfer_bar / mil_row ...
}
hud_detail!(w, "wave {}/{} eco-wait {}t", ...); // checks level BEFORE formatting — zero cost below L3
```

Contributor contract: **a nominal entity emits nothing at L2** (the writer maintains the quiet count automatically); caps make over-emission harmless (full `SmallVec` ⇒ overflow counter ⇒ `+N`). Operations write to the **global** writer — global state gets a global surface, fixing the structural hole where per-room panels were the only home for empire-level data.

**Systems and dispatcher** (slots unchanged — ADR 0004 tiering is not re-decided; all HUD systems stay `StageClass::SkipUnderCritical`):

| Slot | System | Notes |
|---|---|---|
| `clear_visualization` | `HudResetSystem` (Always) | `clear()`-with-capacity on `HudModel`/`TargetBufs` |
| `transfer_stats_snapshot` | kept | needs `Write<TransferQueue>`; gains `econ ≥ Panels` early-return **and a mutation-counter dirt signal** so `flush_all_generators` (real compute, not drawing) is skipped on unchanged ticks |
| `summarize_*` ×4, `visibility_visualization`, aggregate | **`HudCollectSystem`** | single pass: joins ops/missions/jobs once, calls `hud()` hooks, resolves each creep **once** (name + room together), reads SpawnQueue (still before `SpawnQueueSystem` consumes it), ThreatAssessment, VisibilityQueue (at `intel ≥ Debug` — the ungated snapshot system is deleted). The four Summary components are deleted |
| `room_plan_visualize` | `RoomPlanHudSystem` | gated `infra ≥ Debug`; cached wire layer (below) |
| `render` | `HudRenderSystem` | `HudModel` → pure layout kernels → Painter → `TargetBuf`s. CPU/governor data read from always-on `MetricsState`/`CpuHistory` (telemetry sourced outside the sheddable path). **Bucket-aware degradation** (EP-4.1/4.2, no tier change): effective level clamps to L1 when `bucket < BUCKET_HUD_FLOOR`; one `cpu::get_used()` re-check between per-room passes sheds remaining rooms mid-tick |
| `apply_visuals` | `ApplyVisualsSystem` | the stable seam. One `console::add_visual(JsString)` per target |

**Painter + wire serialization (the decisive cost mechanism).** The Painter is the single choke point: it owns the region clamp, the budgets, and a `WireWriter` that serializes each admitted primitive **directly into the per-target `String` buffer** in the engine's wire format (`{"t":"r",...}\n`) — `draw_multi`'s per-primitive boundary crossing and JS re-stringify are gone. This lands at **M1**, not as a late optimization: it is what makes the CPU story true during the migration, and it makes the byte guard *exact* (`payload.len()`) instead of heuristic.

- **Styles** are ~10 pre-serialized `&'static str` JSON fragments appended verbatim — zero per-primitive style allocation.
- **Byte ledger, one per target**, covering cached + live content in a **single concatenated buffer** checked once against `HARD_TARGET_BYTES = 480_000` before the single `add_visual` call. This closes the guard-composition hole where a cached replay plus a separately-counted live flush could jointly exceed 512,000 B and abort the tick. `MAX_VISUALS_PER_TARGET = 4000` is retained only on the residual `draw_multi` fallback path; the byte ledger is the load-bearing guard.
- **Payload order is z-order**: debug/plan layers are concatenated first, chrome and alerts last (they render on top). **Shedding is LayerClass-ordered and line-atomic**: under budget pressure, Debug-class bytes are dropped before Core, **never** by tail-truncation (which would eat the alerts first — inverting EP-3.5), and never mid-line (a half-line is exactly the IBEX-008 corruption shape). Pinned by a property test.
- **Producer-side pre-budgeting**: bulk L3 layers (threat map) aggregate at the source (10×10 quads or top-N ≤ 256 hottest tiles) — nothing is constructed that the budget cannot admit; Painter drops are the backstop, not the mechanism (no build-then-drop waste).
- **Newline-by-construction** (kills Mode B injector i): the WireWriter's *only* emit API serializes one primitive and appends `"\n"`; no raw-string append exists anywhere. This is doubly required because the engine appends string payloads verbatim and the backend's `""`+room concatenation fabricates a merged corrupt line at the buffer boundary if the final line is unterminated — even when both buffers are individually valid.
- **Schema-by-construction against the CLIENT's acceptance predicates** (kills Mode B injector ii): required fields mirror the client's `validate()` and draw-deref expectations, not the Rust crate's types — map shapes always serialize `s` (`"s":{}` when empty; the upstream <0.20 omission killed all map rendering, PR #481), room-text `font` is string-or-number and never null.
- **Total numeric encoding** (kills Mode B injector iii, and ~2–3× of the byte bloat): *every* numeric crossing the wire — coordinates **and style fields** — is finite-checked (drop-with-count, extending P1.C6 which covers coordinates only) and fixed-precision rounded (1 decimal coords, 2 decimals styles; eliminates the f32→f64 widening noise). Dropped counts keep flowing to the EP-3.2 fault counters and surface as the `HudClipped` L1 alert.
- Determinism (EP-6.13): rooms emitted in `RoomName` sort order, rows in producer entity-id order — no HashMap iteration order reaches the payload.

**Wire-format coupling insurance** (the one new risk class this design takes): the golden reference is **the engine's own JS producer shapes** (rooms.js:1146–1200, map.js:281–349) **plus a Rust port of the client's acceptance predicates** (per-line parse, map `validate()`, draw-deref expectations — already encoded in [`tools/viz-repro/`](../../tools/viz-repro/README.md)). Explicitly **NOT** `serde_json::to_string(&screeps::Visual)`: serde_json diverges from the live `serde_wasm_bindgen` + JS `JSON.stringify` path exactly in the danger zones (ES-`Map` → `"{}"`, non-finite floats, f32 widening), and the crate's own shapes were the historical bug (PR #481) — the crate is the thing under test, not the oracle. A serde-wasm-bindgen-path snapshot is kept as a drift *tripwire* asserted against the engine-derived golden, including a regression test that **no visual type ever serializes via `serialize_map`** (the `#[serde(flatten)]`/serde-version-bump → ES-Map → `"{}"` footgun). Debug-build canary (one primitive cross-checked every N ticks) and the `draw_multi` fallback flag stay. On every primitive-schema change, a one-time **render-acceptance probe** runs on the private server: emit each primitive type → read `roomVisual:<uid>,<target>,<tick>` back from Redis → replay through both client parsers.

**Caching** — exactly one cached layer, keyed on a plan-generation counter added to `RoomPlanData` (bumped on replan, 2k–32k ticks apart): the room-plan ghost (~350–600 primitives) is wire-serialized once per replan and replayed as a stored `String` slice into the target buffer. This is an **L3-infra-only** lever — it saves nothing in the default L2 mode and is ranked accordingly. Content-hash retained-mode caching of live HUD content is **explicitly rejected** (judge consensus): against an every-tick-resubmit engine, its marginal win cannot justify the staleness risk and tuning surface. `HudCache` is derivable, never serialized (EP-5.4).

**MapPainter**: same byte-ledger pattern for the map target, `MAP_BYTE_BUDGET = 200_000` of the 1 MB limit; claim markers capped `MAX_CLAIM_MAP_MARKERS = 40` score-sorted, with unknown-room `?` and blocked-by-visibility markers classed **Core** (candidates shed first — "what is blocking selection" is the stated value of the view).

### D3. Cost model and budgets

Named constants, measured not asserted (EP-4.6); the byte side is exact post-`WireWriter` (`payload.len()`), the CPU side is calibrated on the private server at M1 exit.

| Budget | Value | Rationale |
|---|---|---|
| `EMPIRE_STRIP_MAX` | 28 prims (~3 KB) | L1 total, one target |
| `ROOM_L2_MAX` | 196 prims (typical ~100, ~6–8 KB) | header 24 + rails 66 + bottom 26 + badges 80 |
| `ROOM_L3_MAX` | 900 prims | single-domain firehose; plan layer rides the cache |
| `HARD_TARGET_BYTES` | 480,000 | vs the 512,000 B engine throw; byte-exact |
| `MAX_ALERT_ROOMS` | 2 | header + badges only |
| `BUCKET_HUD_FLOOR` | tune (~2000) | below ⇒ clamp to L1 |

Honest estimates for 6 owned RCL7 rooms, 40 creeps, war off, L2 (critic-audited): **~280–620 primitives/tick** typical, 1,204 absolute worst — 2–3 % of the per-target byte limit. CPU ≈ **0.5–1.0 ms/tick** with the wire path (vs ~1.5–3.5 ms if the same design ran through `draw_multi`, and vs the current renderer's strictly larger primitive count plus its ~5× string churn — a ~3–6× win). L1 ambient < 0.1 ms. There is no watcher detection, so the L2 default burns this with nobody looking; the default level (L1 on MMO, L2 pinned in harness scenario Memory) is an explicit operator sign-off item at M3.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| Evolve the current panel renderer (tighten layout, cap lines) | smallest diff; no retraining | keeps rosters-as-paragraphs, the 5× string chain, per-room global redraw, and the flat truncation guard; obstruction is intrinsic to solid panels |
| **In-world annotation first** ("Fieldmark"): kill panels entirely, all data at its world anchor | best obstruction story; data-where-it-lives is the right instinct | global/empire data has no world anchor; badge density collapses in busy rooms; judged 2nd (41/41/39). Its serialize-at-emit `TargetBuf`, byte-exact guard, `offset_flip`, and damaged-only HP bars are **grafted in** |
| **Retained scene + dirty tracking** ("Slate"): content-hash caching, replay unchanged payloads | strongest rebuild-avoidance machinery | fights the engine's every-tick-resubmit model; staleness/tuning surface; war-time thrash; judged 3rd (35/35/34). Its parity tests, z-order-as-payload-order, LayerClass shedding, dirt-signal on `TransferQueue`, and block-char HP bars are **grafted in** |
| **Edge-rail HUD + disclosure levels** ("Glance", chosen) | exceptions-not-rosters directly attacks readability and cost; levels replace interactivity; clean ECS collapse 7→3 systems; strongest migration story | glyph literacy curve; exception predicate can hide what it mispredicts (mitigated: state-age clause + L3 full census); pre-graft CPU story was 2–4× optimistic (fixed: wire path moved to M1) |

A standalone client-side viewer (external dashboard over seg-57 metrics) was considered out of scope: it complements but cannot replace in-game spatial overlays, and the operator's requirement is explicitly room/world-space rendering.

## Consequences

**Positive.** The room view is legible: center 40×40 structurally clear at L2, translucent edge rails, exceptions surfaced instead of buried in rosters. Both verified failure modes are structurally closed, not merely guarded: Mode A (write-side limit throw) by the byte-exact ledger, line-atomic LayerClass shedding, single `add_visual` per target, and the catch-on-binding; Mode B (client-side line poisoning — the mode that actually matches the operator-observed "one world primitive breaks everything") by newline-by-construction, client-predicate schemas, and total numeric encoding. CPU drops ~3–6× at L2 while *showing more* useful state (alerts, trends, world anchors); L1 gives an always-affordable ambient mode that did not exist. Seven systems become three; `visualization.rs` is deleted (IBEX-024); the four Summary component storages, the double creep resolve, and the per-room global redraw all go away. Dead flag families become real per-domain switches; toggling visuals off no longer destroys stats history. Every datum has an enumerated home (Appendix A) — including the previously homeless global operations audit (attack reason / measured threat intel / cost vs invested / spawn homes / war posture / defend lists, now on the global ops rail).

**Negative / risks.**
- **Glyph literacy:** denser but learnable; ≤10-char labels accompany glyphs at L2 and the legend ships in this ADR. First weeks will feel cryptic.
- **Exception predicate misprediction** hides exactly what it misses; the state-age clause targets the known lifecycle-hang mode, constants are operator-tuned, and the L3 paginated full census is the fallback the current design's 24-row cap lacked.
- **Wire-format coupling** to `addVisual` string passthrough and the serde shape: bounded by golden parity tests, the debug canary, and the `draw_multi` fallback flag.
- **Workflow changes needing sign-off:** MMO default L1 (vs full panels today), remote inspection via `visualize.focus` instead of fly-over panels, path polys at L3 (compensated by `PathingStorm` at L1 from always-on telemetry). All three are pinned as the M3 operator sign-off, not silently shipped.
- **Wide mechanical diff:** typed `hud()` hooks touch 30+ mission/job/op files; the M2 shim (legacy `summarize()` strings forwarded into L3 detail) lets domains migrate one at a time — and is labelled interim scaffolding that must die at M6 (EP-2.10).
- **War-domain regression risk:** member names/orders are L3-military by design; the legacy AttackMission dense view stays callable until **typed parity is demonstrated field-by-field** (checklist in Appendix A), not merely "one war fought".

**Verdict ledger (2026-06-11 root-cause investigation — do not re-flag):** refuted as the corruption-mode cause: the 500 KB limit-throw (it is Mode A — real, loud, volume-dependent, never single-primitive), current-toolchain shape divergence (all 10 primitive paths byte-identical to the engine's JS producers; live Redis verified), the `to_value()` panic, packed-`Position` on the wire, tagged-enum→ES-Map→`"{}"` (unreachable via the current API, and `"{}"` is skipped not fatal — Mode C). Still open: which injector produced Field Report H (no payload was captured; predates-the-toolchain, PR #481 class, and the B2 dead-subscription masquerade are the candidates). Coverage gap on record: P1.C6 guards only the `RoomVisualizer` path — `draw_claim_map_visuals` bypasses clamp, cap, and buffering, and has never run in a green smoke. Movement visuals (per-room, clamped, literal styles) are not exposed to Mode B.

## Incremental Migration Path

Stable seam: `Visualizer`/`ApplyVisualsSystem` + resource-presence gating — the IBEX-008 clamps and the rollback flag (`visualize.on=false` ⇒ resource absence ⇒ zero cost) hold at every step. Each step lands battery-green on master.

- **M0 — bug fixes against the current renderer** (independent, land first): (a) stop removing `StatsHistoryData`/`CpuHistory` on toggle-off (game_loop.rs:810–815); (b) delete the per-room redraw of the global layer (visualization.rs:1405–1413) — closes the IBEX-008 open item, ~200–350 prims/tick back immediately; (c) viz-gate `VisibilityVisualizationSystem`; (d) fix the two false/stale comments — visualization.rs:1255–1257 ("string-based…" describes a mitigation that was never wired in; it has already misdirected one investigation) and the visualize.rs:5–10 header (map cap is 1,024,000 B, and the 4000-count cap does not bound bytes); (e) add `#[wasm_bindgen(catch)]` to `console::add_visual` in the path-patched fork and log-and-drop on `Err` — makes Mode A's tick-abort structurally impossible everywhere; (f) reorder `draw_claim_map_visuals` after (or route it through) the buffered room-visual flush so a map-side failure can never erase room visuals; (g) replace the count cap with a measured byte budget via the existing `get_visual_size` binding before `draw_multi`. **Gate on enabling `claim.visualize`**: run the render-acceptance probe first (the map path bypasses every P1.C6 guard today and has never run in a green smoke). Operational note: after any bad line ships to the map target, the client's map-visual subscription is dead until full client reload — reload before re-testing, or every subsequent primitive will falsely appear to "break all rendering".
- **M1 — skeleton + wire path + L1**: land `src/hud/` (levels, config, Painter, **WireWriter + byte ledger**, regions, glyphs) with golden parity tests; `HudRenderSystem` renders only the empire strip to the `None` target via one `add_visual`. Legacy renderer keeps running behind `visualize.legacy` (default true, labelled EP-2.10). **Exit gate:** parity tests green; measured per-primitive CPU and `get_visual_size` readings recorded in the constants' comments (EP-4.6).
- **M2 — collect seam**: `HudCollectSystem` replaces the four Summarize systems + aggregate in their dispatcher slots; Summary components deleted. Compatibility shim: default `hud()` forwards legacy `summarize()` strings into L3 detail so all 30+ producers stay visible from day one.
- **M3 — L2 econ + global ops rail**: typed `hud()` for econ missions/jobs (source badges with L3 role split, exception rows), spawn chips + L3 table, transfer micro-bars + L3 table, header strip + sparkline/trend, `hud_global()` for operations; transfer dirt-signal lands here (mandatory). Flip `visualize.legacy` default off. **Operator sign-off:** default levels, `visualize.focus` workflow, L1 alert set.
- **M4 — L2/L3 military + intel + pathing**: squad chips, damaged-only HP bars, member names/orders at L3, focus lines (the war-cohesion debugging surface), threat/defense chips with producer-side aggregation, visibility top-15, claim map caps + protected blockers, map intel badges; wire `pathing` levels into `IbexMovementVisualizer` (stuck/failed/**immovable** at L2, polys at L3).
- **M5 — plan cache**: `RoomPlanHudSystem` + plan-generation counter + cached wire layer + payload-shape snapshot test.
- **M6 — deletion** (EP-2.6): remove `visualization.rs`, the legacy flag and renderer, dead FSM `visualize()` hooks, dead `TransferNode::visualize`, dead flag families; close-out note. **War-parity checklist (Appendix A) must be checked before the legacy attack view is removed.**

Validation per step: layout-kernel property tests + payload-shape snapshots (ADR 0015 ceiling), one visual smoke pass on the private server per region-introducing step (row metrics lock at M3), and the harness scenario pins `level=2/3` in scenario Memory so eval runs always exercise the HUD.

---

## Appendix A — Data-home mapping (preserved by translation)

Every datum the current overlay surfaces, and its new home. **Losing any of these silently is a defect**; the war rows double as the M6 parity checklist.

| Current datum (source) | New home | Level |
|---|---|---|
| Operations list (`describe_operation`, drawn per room) | Global ops rail rows (glyph · phase · target) | L2 |
| Attack op audit: reason, towers/dps/heal intel, cost est vs invested, spawn homes, waves, eco-wait (attack.rs:497–563) | Global ops rail detail | L3 mil |
| War posture: offense on/off, concurrency cap, active targets, defend-flag rooms (war.rs:1393–1447) | Global ops rail rows + map layer | L2 |
| Mission summaries per room | Econ-rail exception rows; quiet row when nominal | L2 |
| Jobs roster (1 line/creep) | Quiet count L2; **full paginated census** | L3 econ |
| Spawn queue (priority/cost/desc) | Chips (≤6, `+N`) L2; **full numeric table** | L3 econ |
| Transfer panel (all resources, numbers, pending, `Any` row) | Micro-bars (≤4) L2; **full numeric table incl. `Any`** | L3 econ |
| Room intel: owner, reservation, SK, hostile structures, numeric age (visualization.rs:230–244) | Header badges + intel-age dot L2; full text L3; **map intel badges for non-rendered rooms** | L2 intel |
| Storage sparkline (60 pts) | 12-pt decimated spark + ▲/▼ trend text | L2 |
| CPU histogram | Empire-strip CPU bar (governor-tier colored) L1; histogram L3 | L1/L3 |
| Visibility queue top-15 | Intel rows, **top-15 kept** | L3 intel |
| Claim map: candidates + sub-scores, `?` unknowns, blockers, active-claim arrows, owned/max | Map layer (blockers/unknowns Core-classed); owned/max on empire strip | L2 intel |
| Room-plan ghost | Cached wire layer | L3 infra |
| Movement: path polys, anchor lines | Path/anchor polys | L3 pathing |
| Movement: stuck / failed / **immovable** markers | World markers (exception class) | L2 pathing |
| Squad chips: id, alive/expected, target room+**kind+position**, retreat threshold (numeric at L3) | Right-rail chips + global ops detail | L2/L3 mil |
| Squad members: HP, role, **name, movement order, attack/heal assignment** (attack_mission.rs:1805–31) | Damaged-only HP bar + role glyph L2; name + order glyphs L3 | L2/L3 mil |
| Source mining `L:/C:/H:` role split (source_mining.rs:539) | Badge `M2` count L2; role-split badge text `L1C1H2` | L2/L3 econ |
| Safe mode activated (safe_mode.rs:85–91) | **`Alert::SafeModeActivated`**, pinned | L1 |
| Nuke detection (nuke_defense.rs) | `Alert::NukeInbound` + world impact marker | L1 |
| Haul flow arrows (dead haulbehavior hooks) | Pickup→delivery lines, capped 20 | L3 econ |

## Appendix B — Mockups

- [`assets/0016-hud-l2-room.svg`](assets/0016-hud-l2-room.svg) — L2 room view: empire strip, header, rails, bottom strip, world badges; center clear.
- [`assets/0016-hud-levels.svg`](assets/0016-hud-levels.svg) — L1 ambient strip vs L3 econ firehose (full census + tables).
