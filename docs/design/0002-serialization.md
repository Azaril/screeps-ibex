# ADR 0002 — Serialization & Persistence

- **Status:** Proposed
- **Date:** 2026-06-09
- **Related:** Field Report D (serialization brittle); IBEX-013, IBEX-014, IBEX-004, IBEX-049; review report §3 (positional/unversioned wire format), §5 (deser-failure & seg-55 risk rows), §8 (Serialization pillar). Depends on [0001](0001-entity-model.md). Cross-refs [0004](0004-cpu-governance-and-load-shedding.md) (seg-55 wipe feeds the post-reset route storm), [0005](0005-runtime-and-scheduling-model.md) (panic-skipped serialize), [0006](0006-eval-and-iteration-harness.md) (round-trip/fuzz/old-snapshot tests run as a pre-deploy gate).

## Context
Current: specs `SerializeComponents` → **bincode → gzip → base64 → 50 KiB RawMemory segment chunks** (segments 50–55; cost matrix on 55; planner on 60). Pain: **repeated breakage** and fragile **entity-ref mapping**; **positional bincode** has no schema evolution and **no version header**; deserialization failure is **unrecoverable** (only a full reset). New fields must carry `#[serde(default)]` by convention only. A segment-55 ECS/cost-matrix collision risk and silent >50 KiB truncation were flagged.

The review made two of these concrete and authoritative:
- **IBEX-013 (Critical):** `COMPONENT_SEGMENTS = &[50, 51, 52, 53, 54, 55]` (game_loop.rs:554) **overlaps** `COST_MATRIX_SEGMENT = 55` (costmatrixsystem.rs:6). `serialize_world` consumes one segment per 50 KiB chunk, then a trailing `for segment in segments { … .set(*segment, "") }` (game_loop.rs:453–455) blanks **every unconsumed** segment. A typical ECS payload is 1–2 chunks, so the clear reaches seg-55 in the **normal** case — and `CostMatrixStoreSystem` wrote seg-55 earlier the same tick, with `serialize_world` running strictly after. The persisted cost matrix is therefore destroyed end-of-tick and never benefits a reset; the full per-room rebuild lands on the most CPU-starved post-reset tick, feeding the Field Report C death-spiral (ADR 0004).
- **IBEX-004 (High) / Field Report D:** bincode 1.x `DefaultOptions` is positional/ordinal with no field framing. Reordering a field, inserting an enum variant mid-list (JobData 11 / MissionData 24 / OperationData 6), or appending a trailing field misaligns every subsequent byte. The documented `#[serde(default)]` back-compat is **illusory** under bincode — a truncated old payload has no "absent field" representation, so old buffers lacking the new bytes read the next component's tag and cascade into garbage. Failure is **silent**: decode failure → empty `Vec` (game_loop.rs:508), deser error → log+continue (game_loop.rs:533), both presenting as a spontaneous full colony reset.
- **IBEX-014 (Medium):** on segment exhaustion the chunk loop only `error!`-logs and drops remaining chunks (game_loop.rs:444–450); there is no fullness watermark or telemetry, and the inline "will panic" NOTE (game_loop.rs:495–497) is stale.
- **IBEX-049 (feeds 013/014):** `CreepRoverData.path` is serialized every tick (pathing/movementsystem.rs:14–17, game_loop.rs:408), adding ephemeral per-creep path bytes to the payload and pushing it toward the chunk/segment ceiling. Marking it `#[serde(skip)]` is a non-breaking size relief that lowers the pressure behind IBEX-013/014.

Hard constraints: single-threaded WASM (no threads/locks/atomics-for-parallelism); CPU is execution + intents; VM-reset resilience is the whole point of persistence; the rewrite is incremental and confidence-driven (a stable, verifiable seam per step). Back-compat is **not** required — serialized state may be dropped at a labelled cutover — but the running bot must never break mid-increment.

## Decision
Adopt a **two-stage** plan behind the frozen `serialize_world` / `deserialize_world` seam, plus a separate, urgent segment-collision fix.

**Stage 1 — robustness, any format, ships non-breaking (review-recommended, authoritative).**
1. Add a **version header** to the serialized payload (a leading version byte/tag, written by the pure `encode` helper, checked by `decode`).
2. On a version (or magic) **mismatch, reject-and-reset deterministically**: turn today's silent-garbage path (decode→empty `Vec` at game_loop.rs:508; deser-error→log+continue at game_loop.rs:533) into a **single, intentional, loud reset** — drop the world cleanly, emit telemetry, rebuild from a fresh tick. A loud intentional reset is strictly better than a silently-empty world that masquerades as a spontaneous colony wipe (Field Report D).
3. Stand up tests on the **pure encode/decode helpers** (serialize.rs:310–344): round-trip, an **old-snapshot corpus** (real captured payloads from prior schema versions), and **fuzz** (random/truncated/bit-flipped buffers must reject-and-reset, never panic, never silently half-decode). The helpers are already pure, and a **MemoryArbiter double** (memorysystem.rs) makes the whole serialize→chunk→deserialize pipeline testable on the host target today, with no game runtime — the highest-ROI test seam called out in §9.
4. Apply IBEX-049's `#[serde(skip)]` on `CreepRoverData.path` to shrink the payload, and emit IBEX-014's **segment-fullness watermark** (encoded size + chunk count) to the metrics segment so we see the chunk ceiling approaching before it truncates; treat overflow as a hard, loud error rather than a silent chunk-drop, and rewrite the stale game_loop.rs:495–497 NOTE to match the actual log-and-continue policy.

**Stage 2 — format swap, rewrite (review-recommended).** Replace the **body** of the format (positional bincode) with a **tagged / schema-evolving** format (explicit hand-written versioned (de)serialization, or a self-describing binary format) behind the unchanged `serialize_world` / `deserialize_world` seam, so callers do not move. This is what actually makes `#[serde(default)]`-style additive evolution real instead of illusory. Stage 2 is **gated on ADR 0001**: it lands only after 0001 removes the specs `Entity` wrappers from the payload (the marker-remapped `Entity` indices and the raw-u32 squad ref, IBEX-002b), because evolving a tagged schema while the payload still encodes recyclable entity indices would re-import the dangling/aliasing class into the new format. The cutover is the second of the two **intentional one-time state drops** in the sequencing plan; confine it to a low-stakes tick.

**Separate & urgent — segment disjointness (closes Critical IBEX-013), independent of Stage 1/2.**
- Make `COMPONENT_SEGMENTS` and `COST_MATRIX_SEGMENT` **provably disjoint with a compile-time assertion** (a `const` assert that 55 ∉ `COMPONENT_SEGMENTS`), so the trailing clear at game_loop.rs:453–455 can never blank a segment another subsystem owns, and a future segment-map edit fails to compile rather than silently re-introducing the wipe.
- Move the cost matrix to a **dedicated segment** (or shrink `COMPONENT_SEGMENTS` to 50–54 after confirming the payload stays under 5 chunks — the Stage-1 watermark provides that confirmation). The non-breaking interim (shrink + assert) ships immediately; a fully dedicated cost-matrix segment with per-owner reservation is the durable form.

### Segment allocation registry (authoritative, owned here)

The disjointness assert is only as good as the map it checks. This table is the **single registry** of RawMemory segment ids across the design set, and the RULE is: **every new segment must be added here at design time** — an ADR that allocates a segment without a row in this table is incomplete (an unregistered allocation is exactly how the seg-55 collision happened). **Code form (operator directive, landed with the Phase-0 D1 fix):** a dedicated `segments` module in the bot crate is this table's executable twin — every segment id is a named constant there, the compile-time uniqueness check runs over the whole table, and all subsystems (game loop, cost-matrix store, stats, planner) import their ids from it rather than defining them locally; the core loop never references another subsystem's segment constant. The engine caps **active** segments per tick (`RawMemory.setActiveSegments`, max 10 ids — see [`../references/engine-mechanics.md`](../references/engine-mechanics.md) §RawMemory, pinned in this design pass), so the post-reset tick must fit **all must-load segments** within that cap; everything else loads lazily on later ticks.

| Seg | Owner / contents | Post-reset |
|---|---|---|
| **50–53** | ECS component payload (`COMPONENT_SEGMENTS`; shrunk 50–55 → 50–54 by the disjointness fix, → 50–53 on 2026-06-12 to fund the always-active market segment — watermark-gated: BASELINE-2 scale used 1 chunk; the watermark warns at budget − 1) | **must-load** (tick 1) |
| **54** | *freed 2026-06-12* (former 5th component chunk; stale data may linger server-side until reused) | — |
| **55** | cost matrix **only** (dedicated after the disjointness fix — the former IBEX-013 collision) | **must-load** (the warm cache averts the post-reset route storm, [0004](0004-cpu-governance-and-load-shedding.md)) |
| **56** | stats history (unversioned JSON today — version header per [0006](0006-eval-and-iteration-harness.md)) | lazy |
| **57** | metrics block ([0006](0006-eval-and-iteration-harness.md), versioned, always-on) | lazy (write-mostly) |
| **58** | market memory (`MARKET_SEGMENT`): per-resource history-day cache + exposure ledger — the interim form of [0012](0012-market-and-risk.md) M3's risk ledger, **landed 2026-06-12** with its own `MARKET_MEMORY_VERSION` field, decoupled from `WORLD_FORMAT_VERSION` by design; M3's `TradeGovernor` state joins it here | **always-active** (operator decision 2026-06-12: risk data wants zero save gaps — slot funded by the component shrink; `on_load` callback fills the resource, trading gates on `loaded`, saves land same-tick). The arbiter's queued-write reservation remains the path for future NON-active segments, and a rotating slot for periodic systems (e.g. planner seg 60) is planned to reclaim headroom |
| **60** | room-planner resume state (`PLANNER_MEMORY_SEGMENT`, [0009](0009-room-planning-and-multiroom-layout.md)) | lazy (planning resumes next budget slice) |
| **61** | `RoomGraph` + inter-room road sets ([0009](0009-room-planning-and-multiroom-layout.md) left "labelled addition to 60 or a dedicated free id" open — **pinned to 61 here**, keeping 60 resume-only) | lazy (warm before route planning resumes) |
| **99** | live stats (legacy JSON — version header per [0006](0006-eval-and-iteration-harness.md)) | lazy |

Must-load today = 50–53 + 55 (**5 of 10**) — comfortably inside the cap; any future must-load addition must re-check that sum in this table. The full steady-state ACTIVE set (must-load + 56/57/58 + ad-hoc 60/99) sits at exactly **10 of 10** — adding any always-active segment requires freeing a slot first (the planned periodic-rotation mechanism for lazily-used ids like 60 is the intended source of headroom).

Per §8 Sequencing, this is **Increment 2** (gate: round-trip/old-snapshot/fuzz green), after Increment 1's CPU governor + panic containment (ADR 0004/0005 — the latter ensures `serialize_world` always runs even if a system aborts, so a panic can no longer skip persistence and present as a reset). Stage 2 is **Increment 5** (gate: ADR 0001 stable-ID store landed).

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep bincode + add a **version header** & round-trip tests | small change; some safety | positional fragility & entity-index coupling remain |
| **Explicit hand-written (de)serialization** with versioned schemas | full control; explicit migrations | more code to maintain |
| **Schema-evolving binary format** (FlatBuffers / Cap'n Proto / protobuf) | forward/backward compat; defined evolution | new dep & build step; WASM size |
| **Persist stable game IDs**, not entity indices (pairs with 0001) | eliminates entity-repair coupling; robust | resolve IDs on load; ID lifecycle for ops/missions |

The decision **composes** these rather than picking one: row 1 is exactly Stage 1 (cheap, non-breaking, ships first); Stage 2 chooses between rows 2 and 3 at format-swap time (a concrete benchmark of WASM-size and CPU on the captured corpus settles it then); row 4 is owned by ADR 0001 and is the precondition that makes Stage 2 sound.

## Consequences
**Positive.**
- IBEX-013 (Critical) is closed: the cost matrix survives a VM reset, so the post-reset tick no longer pays a full per-room cost-matrix rebuild on the most CPU-starved tick — direct relief for the Field Report C spiral (ADR 0004). The compile-time disjointness assert makes the wipe unrepresentable going forward.
- Deserialization failure **degrades to a loud, intentional reset with telemetry**, not a silent empty-world masquerading as a spontaneous colony wipe — Field Report D's "repeated breakage" becomes an attributable, alarmed event (a nonzero deser-failure count is a pre-deploy gate failure per §9).
- The pure encode/decode helpers gain round-trip / old-snapshot / fuzz coverage — the single most survival-critical untested kernel (§9 ranks deser-failure first because it is unrecoverable). This is verifiable on the host target today via a MemoryArbiter double, before any runtime.
- Stage 2 makes additive schema evolution actually work, retiring the illusory-`serde(default)` convention and the operator's recurring format breakage.
- IBEX-049 `#[serde(skip)]` and the fullness watermark lower segment pressure and make the chunk ceiling observable before it truncates.

**Negative / costs.**
- Stage 1 ships a **one-time intentional state drop** when the version header is introduced (old headerless payloads fail the version check and reject-and-reset). This is **Increment 2** of the sequencing plan and must be confined to a low-stakes tick; it is acceptable per the no-back-compat policy. Label: **Memory/format** (a labelled cutover, not silent breakage).
- Stage 2 ships a **second one-time intentional state drop** at the format-body swap (the cutover), again confined to a low-stakes tick. Label: **Memory/format**. Stage 2 cannot land before ADR 0001 — a sequencing dependency, not optional.
- More serialization code to own (explicit versioned (de)serialization) or a new dependency + build step and WASM-size cost (self-describing format). The Stage-2 choice is deferred to a benchmark on the captured corpus.

**New risks / what becomes harder.**
- A wrong disjointness fix (e.g. dedicating a segment that collides with stats seg-56, planner seg-60, or live-stats seg-99 per §9) would re-introduce a wipe; the compile-time assert and the explicit segment map mitigate this, and the watermark surfaces a too-small `COMPONENT_SEGMENTS` budget before it truncates.
- Versioning is only as good as the discipline of bumping the version on a breaking schema change; the old-snapshot corpus test is the backstop that catches a forgotten bump (an old payload that should reject but decodes is a corpus-test failure).

**CPU / tick-safety.** Stage 1 adds negligible CPU (one header byte, one comparison). Fuzz/round-trip tests run **offline** on the host target and never touch the tick. Reject-and-reset replaces an undefined silent-corruption path with a deterministic one, so it is strictly tick-safer; no panic is introduced (fuzz must prove decode never panics). The disjointness fix is zero runtime cost (a `const` assert) and removes a per-tick destructive write to seg-55.

## Incremental Migration Path
The seam is the **frozen `serialize_world` / `deserialize_world` pair** (game_loop.rs); every step hides behind it so callers never change. Each step is validated by the eval harness (ADR 0006) before the next; never break the running bot mid-increment.

1. **Now / Increment 2 prep (None-breaking):** add the compile-time **disjointness assertion** for `COMPONENT_SEGMENTS` vs `COST_MATRIX_SEGMENT`; interim-shrink `COMPONENT_SEGMENTS` to 50–54 (or dedicate a cost-matrix segment); apply `#[serde(skip)]` to `CreepRoverData.path` (IBEX-049); emit the **fullness watermark** + chunk count to the metrics segment and fail-loud on overflow; rewrite the stale game_loop.rs:495–497 NOTE. **Validate:** force a reset, assert `load_cost_matrix_cache` returns non-empty (IBEX-013 repro); compare serialized segment bytes before/after the skip at scale (IBEX-049); inflate the ECS past the chunk budget and assert a loud watermark error, not a silent drop (IBEX-014).
2. **Increment 2 — Stage 1 (Memory/format, one-time intentional reset):** add the **version header**; switch the silent decode→empty / deser-error→continue paths to **reject-and-reset + telemetry**; land **round-trip + old-snapshot-corpus + fuzz** tests against the pure encode/decode helpers via a **MemoryArbiter double** (no game runtime). **Gate to advance:** all three test suites green; zero deser-failures and zero panics in a sim smoke-run (§9 pre-deploy gates). Confine the headerless→headered cutover to a low-stakes tick.
3. **Increment 5 — Stage 2 (Memory/format, one-time intentional reset), gated on ADR 0001:** after 0001 removes `Entity` wrappers (marker indices and the raw-u32 squad ref) from the payload, **swap the format body** to a tagged/schema-evolving format behind the unchanged seam; choose explicit-versioned vs self-describing by a WASM-size + CPU benchmark on the captured corpus; carry the old-snapshot corpus forward as the migration/regression oracle. Confine the cutover to a low-stakes tick.

**Breaking-change labels:** Step 1 — **None**. Step 2 (Stage 1 version header) — **Memory/format** (labelled one-time reset). Step 3 (Stage 2 format swap) — **Memory/format** (labelled one-time reset). No behavioral breaks; the two state drops are deliberate, labelled, and confined to low-stakes ticks per the sequencing plan.
