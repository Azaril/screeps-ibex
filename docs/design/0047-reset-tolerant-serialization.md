# 0047 — Reset-tolerant world serialization (tagged encoding)

- **Status:** Draft
- **Date:** 2026-08-23
- **Deciders:** William Archbell (motivation + direction stated; encoding choice awaits experiments)
- **Related:** [0002](0002-serialization.md) (the serialization design of record — this ADR would
  supersede its *reset-anytime* posture in part, and absorbs its deferred Stage-2 per-section
  isolation), the WFV loud-reset machinery (`game_loop.rs`), the `reset.*` one-shot family.

## One-line

Replace (or wrap) the positional-bincode world stream with a **migration-tolerant encoding** so a
deploy that changes a serialized shape recovers state through regular serde mechanics —
missing fields default, unknown fields are ignored — instead of discarding the world.

## Context — why the reset-anytime posture no longer prices correctly

The world state (segments 50–52) is positional bincode behind a `WORLD_FORMAT_VERSION`
fingerprint: any serialized-shape change bumps WFV and the next deploy discards the world (the
"loud reset"). ADR 0002 chose this deliberately — migration machinery was judged not worth it when
a reset cost little.

That pricing has inverted on live MMO (operator, 2026-08-23). The empire impact of a reset is
acceptable; **the recovery latency is not**: MMO simulates ~1 tick/sec, and a reset means
re-scouting the frontier, re-planning rooms, and rebuilding mission/intel state — hours to days of
wall-clock before the bot is back to its pre-reset decision quality (observed directly in the
WFV 27→28 deploy: multi-hour re-plan/re-scout convergence). With the testing posture now
deploy-to-live, every WFV bump buys that latency again. Serde-tolerant recovery would make most
shape changes (field add/remove, the dominant class) **free**, reserving the loud reset for
genuine semantic breaks.

## Constraints (the two stated worries)

1. **CPU.** Serialize runs every tick inside the wasm budget. Positional bincode is near the floor;
   any self-describing encoding pays for tags. The budget question is *marginal cost per tick*, on
   wasm, at real world sizes (~60KB decoded today; grows with empire).
2. **Size.** Segments cap at 100KB × 10; the stream ships deflate+base64. Named-field encodings can
   balloon raw size; tagged-numeric ones (protobuf-style field ids) cost far less, and deflate
   compresses repeated tags well — measure POST-compression, not raw.

## Candidate schemes (the experiment matrix)

| Scheme | Tolerance mechanism | Expected cost |
|---|---|---|
| Status quo (positional bincode + WFV) | none — reset on any shape change | baseline |
| **Per-section version headers + component isolation** (ADR 0002 Stage-2) | hand-written per-section migration; a bad section resets alone | near-zero CPU/size; per-change engineering |
| **rmp-serde (MessagePack), named-field mode** | full serde tolerance (skip unknown / default missing) | tags = field names; deflate mitigates; decode cost ? |
| **ciborium (CBOR)** | same | similar to msgpack |
| **prost / protobuf-style numeric tags** | field-number tolerance, explicit ids | compact tags, fast; loses serde-derive ergonomics |
| **postcard / bincode w/ serde-default** | ✗ still positional — does NOT deliver tolerance | (eliminated on paper unless framed per-field) |

Hybrid worth testing: **coarse framing + fine positional** — a tagged envelope per *component
store* (the ConvertSaveload tuple members), each internally positional-bincode with its own
version byte. Field-level tolerance is lost, but per-component migration/reset becomes possible
and the tag overhead is per-store, not per-field — likely the best CPU/size point if field-level
tolerance proves too expensive.

## Experiments to run (offline first, per the testing posture)

1. Extract a real live payload (the world-decoder fixture flow already fetches segs 50–52).
2. Bench encode+decode for each scheme on the real data: native first, then wasm (the number that
   matters); record raw + deflated sizes.
3. Simulate the migration classes against each scheme: field add, field remove, enum variant add,
   nested-struct add. Score: survives-with-defaults / per-section reset / full reset.
4. Decide the tolerance granularity (field vs component) from the measured costs; write the
   decision into this ADR and promote it to Decided.

## Consequences (anticipated)

- WFV remains as the outer fingerprint for *semantic* breaks; it stops being the tax on every
  additive change. The `reset.*` one-shot family and the loud-reset path stay as the escape hatch.
- The entity-marker remapping (ADR 0001/0002 `ConvertSaveload`) must survive whatever encoding
  wraps it — the marker ids are the stable identity and are encoding-agnostic, but the experiment
  must verify round-trip through the chosen scheme.
- Serialized-shape discipline (the "batch WFV bumps" convention) relaxes to "batch semantic
  breaks"; the tracker's deploy calculus changes accordingly.
