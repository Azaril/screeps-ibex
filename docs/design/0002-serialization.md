# ADR 0002 — Serialization & Persistence

- **Status:** Proposed
- **Date:** <YYYY-MM-DD>
- **Related:** Field Report D (serialization brittle); review prompt §6.2, §12. Depends on [0001](0001-entity-model.md).

## Context
Current: specs `SerializeComponents` → **bincode → gzip → base64 → 50 KiB RawMemory segment chunks** (segments 50–55; cost matrix on 55; planner on 60). Pain: **repeated breakage** and fragile **entity-ref mapping**; **positional bincode** has no schema evolution and **no version header**; deserialization failure is **unrecoverable** (only a full reset). New fields must carry `#[serde(default)]` by convention only. A segment-55 ECS/cost-matrix collision risk and silent >50 KiB truncation were flagged.

## Decision
<TBD after review.>

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep bincode + add a **version header** & round-trip tests | small change; some safety | positional fragility & entity-index coupling remain |
| **Explicit hand-written (de)serialization** with versioned schemas | full control; explicit migrations | more code to maintain |
| **Schema-evolving binary format** (FlatBuffers / Cap'n Proto / protobuf) | forward/backward compat; defined evolution | new dep & build step; WASM size |
| **Persist stable game IDs**, not entity indices (pairs with 0001) | eliminates entity-repair coupling; robust | resolve IDs on load; ID lifecycle for ops/missions |

## Consequences
<TBD — incl. how deserialization failure degrades (telemetry + intentional reset vs. partial recovery).>

## Incremental Migration Path
<e.g. add version header + round-trip/fuzz/old-snapshot tests first; then swap format behind the serialize/deserialize seam. Back-compat not required — a clean state drop at cutover is acceptable.>
