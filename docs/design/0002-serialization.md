# ADR 0002 — Serialization & Persistence

- **Status:** Decided
- **Date:** 2026-06-09
- **Related:** Field Report D (serialization brittle); IBEX-013, IBEX-014, IBEX-004, IBEX-049; review report §3 (positional/unversioned wire format), §5 (deser-failure & seg-55 risk rows), §8 (Serialization pillar). Depends on [0001](0001-entity-model.md). Cross-refs [0004](0004-cpu-governance-and-load-shedding.md) (seg-55 wipe feeds the post-reset route storm), [0005](0005-runtime-and-scheduling-model.md) (panic-skipped serialize), [0006](0006-eval-and-iteration-harness.md) (round-trip/fuzz/old-snapshot tests run as a pre-deploy gate).

## Context
The pipeline is specs `SerializeComponents` → **bincode → gzip → base64 → 50 KiB RawMemory segment chunks**. Pain: **repeated breakage** and fragile **entity-ref mapping**; **positional bincode** has no schema evolution and, before this ADR, no version header; deserialization failure was **unrecoverable** (only a full reset). New fields carried `#[serde(default)]` by convention only. A segment-55 ECS/cost-matrix collision risk and silent >50 KiB truncation were flagged.

The review made these concrete and authoritative:
- **IBEX-013 (Critical):** `COMPONENT_SEGMENTS = &[50, 51, 52, 53, 54, 55]` **overlapped** `COST_MATRIX_SEGMENT = 55` (costmatrixsystem.rs:6). `serialize_world` consumes one segment per 50 KiB chunk, then a trailing `for segment in segments { … .set(*segment, "") }` blanks **every unconsumed** segment. A typical ECS payload is 1–2 chunks, so the clear reaches seg-55 in the **normal** case — and `CostMatrixStoreSystem` writes seg-55 earlier the same tick, with `serialize_world` running strictly after. The persisted cost matrix was therefore destroyed end-of-tick and never benefited a reset; the full per-room rebuild landed on the most CPU-starved post-reset tick, feeding the Field Report C death-spiral (ADR 0004).
- **IBEX-004 (High) / Field Report D:** bincode 1.x `DefaultOptions` is positional/ordinal with no field framing. Reordering a field, inserting an enum variant mid-list (JobData 11 / MissionData 24 / OperationData 6), or appending a trailing field misaligns every subsequent byte. The documented `#[serde(default)]` back-compat is **illusory** under bincode — a truncated old payload has no "absent field" representation, so old buffers lacking the new bytes read the next component's tag and cascade into garbage. Untreated, the failure is **silent**: decode failure → empty `Vec`, deser error → log+continue, both presenting as a spontaneous full colony reset.
- **IBEX-014 (Medium):** on segment exhaustion the chunk loop only `error!`-logs and drops remaining chunks; without a fullness watermark or telemetry the chunk ceiling is invisible until it truncates.
- **IBEX-049 (feeds 013/014):** `CreepRoverData.path` is serialized every tick (pathing/movementsystem.rs:14–17), adding ephemeral per-creep path bytes to the payload. Marking it `#[serde(skip)]` was proposed as size relief and is **rejected** — see Alternatives.

Hard constraints: single-threaded WASM (no threads/locks/atomics-for-parallelism); CPU is execution + intents; VM-reset resilience is the whole point of persistence; the rewrite is incremental and confidence-driven (a stable, verifiable seam per step). Back-compat is **not** required — serialized state may be dropped at a labelled cutover — but the running bot must never break mid-increment.

## Decision

Everything hides behind the frozen `serialize_world` / `deserialize_world` seam. The design has three parts: a robustness layer over the wire format, a segment-allocation regime that makes cross-owner collisions unrepresentable, and a deliberate position on schema evolution.

**Format robustness — versioned payload, reject-and-reset.**
1. The serialized payload carries a **version header**: the encode path writes `WORLD_FORMAT_VERSION` as a leading little-endian `u32` (`game_loop.rs:515`), and the decode path checks it as a fingerprint.
2. On a version (or magic) **mismatch, reject-and-reset deterministically** (`game_loop.rs:793–803`): the payload is dropped wholesale, `record_deser_failure()` fires, and the world rebuilds from empty. This replaces the silent-garbage paths (decode→empty `Vec`, deser-error→log+continue) with a **single, intentional, loud reset**. A loud intentional reset is strictly better than a silently-empty world that masquerades as a spontaneous colony wipe (Field Report D).
3. The **pure encode/decode helpers** (serialize.rs:310–344) carry the test weight: round-trip, an **old-snapshot corpus** (real captured payloads from prior schema versions), and **fuzz** (random/truncated/bit-flipped buffers must reject-and-reset, never panic, never silently half-decode). Because the helpers are pure and a **MemoryArbiter double** (memorysystem.rs) stands in for the game, the whole serialize→chunk→deserialize pipeline is testable on the host target with no game runtime — the highest-ROI test seam called out in §9.
4. A **segment-fullness watermark** (encoded size + chunk count) is emitted to the metrics segment so the chunk ceiling is visible before it truncates; overflow is a hard, loud error rather than a silent chunk-drop.

**Schema evolution — positional bincode plus a loud versioned reset is the format of record.** The format body stays positional bincode (`Serializer::new(&mut serialized_data, DefaultOptions::new())`, `game_loop.rs:517`). The sanctioned migration for *any* serialized-shape change is a **`WORLD_FORMAT_VERSION` bump and the resulting loud reset**, which the robustness layer above turns into a clean, attributable event. The reasoning is **reset-anytime**: a deploy resets serialized state regardless, so the no-reset additive evolution a tagged format would buy has no consumer, and buying it would cost either a hand-maintained versioned (de)serialization layer or a new dependency plus WASM size. The consequence is a hard rule rather than an inconvenience: **a shape change without a version bump is a defect**, because positional bincode will read the old bytes as garbage instead of rejecting them.

A **tagged / schema-evolving format swap** — replacing the body with explicit hand-written versioned (de)serialization or a self-describing binary format, behind the unchanged `serialize_world` / `deserialize_world` seam — is the escape hatch if a genuine no-reset migration need ever appears. Two conditions bound it. First, it is **gated on ADR 0001**: it may land only once the payload is free of raw specs `Entity` wrappers (marker-remapped indices, the raw-u32 squad ref of IBEX-002b), because evolving a tagged schema over a payload that still encodes recyclable entity indices would re-import the dangling/aliasing class into the new format. Second, the choice between explicit-versioned and self-describing is settled by a **WASM-size and CPU benchmark on the captured corpus**, not in advance, and the cutover is a labelled one-time state drop confined to a low-stakes tick.

**Segment disjointness (closes Critical IBEX-013).**
- `COMPONENT_SEGMENTS` and `COST_MATRIX_SEGMENT` are **provably disjoint by a compile-time assertion** — a `const _` assert over the whole segment table (`segments.rs:131–137`), so the trailing clear can never blank a segment another subsystem owns, and a future segment-map edit **fails to compile** rather than silently re-introducing the wipe. The assert *is* the disjointness regression test.
- The cost matrix owns a **dedicated segment**; `COMPONENT_SEGMENTS` is sized to fit the payload with watermark headroom rather than sprawling across the whole range.

### Segment allocation registry (authoritative, owned here)

The disjointness assert is only as good as the map it checks. This table is the **single registry** of RawMemory segment ids across the design set, and the RULE is: **every new segment must be added here at design time** — an ADR that allocates a segment without a row in this table is incomplete (an unregistered allocation is exactly how the seg-55 collision happened). **Code form:** a dedicated `segments` module in the bot crate (`src/segments.rs`) is this table's executable twin — every segment id is a named constant there, the compile-time uniqueness check runs over the whole table, and all subsystems (game loop, cost-matrix store, stats, planner) import their ids from it rather than defining them locally; the core loop never references another subsystem's segment constant. The engine caps **active** segments per tick (`RawMemory.setActiveSegments`, max 10 ids — see [`../references/engine-mechanics.md`](../references/engine-mechanics.md) §RawMemory), so the post-reset tick must fit **all must-load segments** within that cap; everything else loads lazily on later ticks.

| Seg | Owner / contents | Post-reset |
|---|---|---|
| **50–53** | ECS component payload (`COMPONENT_SEGMENTS`; narrowed from the original 50–55 by the disjointness fix, and again to fund the always-active market segment — watermark-gated: BASELINE-2 scale used 1 chunk; the watermark warns at budget − 1) | **must-load** (tick 1) |
| **54** | unallocated (former 5th component chunk; stale data may linger server-side until reused) | — |
| **55** | cost matrix **only** (dedicated by the disjointness fix — the former IBEX-013 collision) | **must-load** (the warm cache averts the post-reset route storm, [0004](0004-cpu-governance-and-load-shedding.md)) |
| **56** | stats history (unversioned JSON — version header per [0006](0006-eval-and-iteration-harness.md)) | lazy |
| **57** | metrics block ([0006](0006-eval-and-iteration-harness.md), versioned, always-on) | lazy (write-mostly) |
| **58** | market memory (`MARKET_SEGMENT`): per-resource history-day cache + exposure ledger — the interim form of [0012](0012-market-and-risk.md) M3's risk ledger, carrying its own `MARKET_MEMORY_VERSION` field, decoupled from `WORLD_FORMAT_VERSION` by design; M3's `TradeGovernor` state joins it here | **always-active** (risk data wants zero save gaps — the slot is funded by the component shrink; an `on_load` callback fills the resource, trading gates on `loaded`, saves land same-tick). The arbiter's queued-write reservation remains the path for future NON-active segments, and a rotating slot for periodic systems (e.g. planner seg 60) reclaims headroom |
| **60** | room-planner resume state (`PLANNER_MEMORY_SEGMENT`, [0009](0009-room-planning-and-multiroom-layout.md)) | lazy (planning resumes next budget slice) |
| **61** | `RoomGraph` + inter-room road sets ([0009](0009-room-planning-and-multiroom-layout.md) left "labelled addition to 60 or a dedicated free id" open — **pinned to 61 here**, keeping 60 resume-only) | lazy (warm before route planning resumes) |
| **99** | live stats (legacy JSON — version header per [0006](0006-eval-and-iteration-harness.md)) | lazy |

Must-load = 50–53 + 55 (**5 of 10**) — comfortably inside the cap; any future must-load addition must re-check that sum in this table. The full steady-state ACTIVE set (must-load + 56/57/58 + ad-hoc 60/99) sits at exactly **10 of 10** — adding any always-active segment requires freeing a slot first (the periodic-rotation mechanism for lazily-used ids like 60 is the intended source of headroom).

Ordering constraints: the CPU governor and panic containment (ADR 0004/0005) come first, because the latter ensures `serialize_world` always runs even if a system aborts, so a panic can no longer skip persistence and present as a reset. The format-robustness layer advances on round-trip/old-snapshot/fuzz coverage being green. Any future format-body swap follows ADR 0001's removal of entity wrappers from the payload.

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep bincode + add a **version header** & round-trip tests (chosen) | small change; deterministic reject-and-reset; no new deps | positional fragility remains — every shape change costs a reset |
| **Explicit hand-written (de)serialization** with versioned schemas | full control; explicit migrations | more code to maintain, for evolution nothing currently consumes |
| **Schema-evolving binary format** (FlatBuffers / Cap'n Proto / protobuf) | forward/backward compat; defined evolution | new dep & build step; WASM size |
| **Persist stable game IDs**, not entity indices (pairs with 0001) | eliminates entity-repair coupling; robust | resolve IDs on load; ID lifecycle for ops/missions |
| `#[serde(skip)]` on `CreepRoverData.path` (IBEX-049) | shrinks the payload; non-breaking | **operator-DECLINED** — the operator declined this change; no further rationale was recorded. Treat as closed; do not re-propose |

Rows 2 and 3 remain the menu *if* a no-reset migration need appears; the choice between them is a benchmark, not a preference. Row 4 is owned by ADR 0001 and is the precondition that would make either sound.

## Consequences
**Positive.**
- IBEX-013 (Critical) is closed: the cost matrix survives a VM reset, so the post-reset tick no longer pays a full per-room cost-matrix rebuild on the most CPU-starved tick — direct relief for the Field Report C spiral (ADR 0004). The compile-time disjointness assert makes the wipe unrepresentable going forward.
- Deserialization failure **degrades to a loud, intentional reset with telemetry**, not a silent empty-world masquerading as a spontaneous colony wipe — Field Report D's "repeated breakage" becomes an attributable, alarmed event (a nonzero deser-failure count is a pre-deploy gate failure per §9).
- The pure encode/decode helpers gain round-trip / old-snapshot / fuzz coverage — the single most survival-critical kernel (§9 ranks deser-failure first because it is unrecoverable). This is verifiable on the host target via a MemoryArbiter double, before any runtime.
- The fullness watermark makes the chunk ceiling observable before it truncates.
- Holding the line at positional bincode keeps the serialization surface small: no second (de)serialization implementation to keep in sync, no extra dependency, no WASM-size tax — paid for by accepting a reset per shape change, which the deploy cadence imposes anyway.

**Negative / costs.**
- Introducing the version header is a **one-time intentional state drop** (old headerless payloads fail the version check and reject-and-reset), confined to a low-stakes tick and acceptable per the no-back-compat policy. Label: **Memory/format** (a labelled cutover, not silent breakage).
- Every subsequent serialized-shape change costs a **full state reset**. This is the accepted price of the format choice, and it makes version-bump discipline load-bearing rather than hygienic.
- A future format-body swap would ship a second labelled state drop, and cannot land before ADR 0001.

**New risks / what becomes harder.**
- A wrong disjointness fix (e.g. dedicating a segment that collides with stats seg-56, planner seg-60, or live-stats seg-99 per §9) would re-introduce a wipe; the compile-time assert and the explicit segment map mitigate this, and the watermark surfaces a too-small `COMPONENT_SEGMENTS` budget before it truncates.
- Versioning is only as good as the discipline of bumping the version on a breaking schema change; the old-snapshot corpus test is the backstop that catches a forgotten bump (an old payload that should reject but decodes is a corpus-test failure).

**CPU / tick-safety.** The header costs one comparison. Fuzz/round-trip tests run **offline** on the host target and never touch the tick. Reject-and-reset replaces an undefined silent-corruption path with a deterministic one, so it is strictly tick-safer; no panic is introduced (fuzz must prove decode never panics). The disjointness fix is zero runtime cost (a `const` assert) and removes a per-tick destructive write to seg-55.

## Incremental Migration Path
The seam is the **frozen `serialize_world` / `deserialize_world` pair** (game_loop.rs); every step hides behind it so callers never change. Each step is validated by the eval harness (ADR 0006) before the next; never break the running bot mid-increment.

1. **Segment hygiene (None-breaking):** add the compile-time **disjointness assertion** for `COMPONENT_SEGMENTS` vs `COST_MATRIX_SEGMENT`; narrow `COMPONENT_SEGMENTS` and dedicate the cost-matrix segment; emit the **fullness watermark** + chunk count to the metrics segment and fail-loud on overflow. **Validate:** force a reset, assert `load_cost_matrix_cache` returns non-empty (IBEX-013 repro); inflate the ECS past the chunk budget and assert a loud watermark error, not a silent drop (IBEX-014).
2. **Format robustness (Memory/format, one-time intentional reset):** add the **version header**; switch the silent decode→empty / deser-error→continue paths to **reject-and-reset + telemetry**; land **round-trip + old-snapshot-corpus + fuzz** tests against the pure encode/decode helpers via a **MemoryArbiter double** (no game runtime). **Gate to advance:** all three test suites green; zero deser-failures and zero panics in a sim smoke-run (§9 pre-deploy gates). Confine the headerless→headered cutover to a low-stakes tick.
3. **Format-body swap (Memory/format, one-time intentional reset) — only on a no-reset migration need, gated on ADR 0001:** once the payload is free of `Entity` wrappers, swap the body to a tagged/schema-evolving format behind the unchanged seam; choose explicit-versioned vs self-describing by a WASM-size + CPU benchmark on the captured corpus; carry the old-snapshot corpus forward as the migration/regression oracle. Confine the cutover to a low-stakes tick.

**Breaking-change labels:** Step 1 — **None**. Step 2 (version header) — **Memory/format** (labelled one-time reset). Step 3 (format swap) — **Memory/format** (labelled one-time reset). No behavioral breaks; the state drops are deliberate, labelled, and confined to low-stakes ticks.
