# 0047 — Reset-tolerant world serialization (tagged encoding)

- **Status:** Decided
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

## Experiment results — round 1 (2026-08-23, real live payload, native `--release`, best-of-5)

Input: the live WFV-28 world (8 rooms, 286,116 raw bincode bytes). Bincode decode baseline 1.83 ms.

| Scheme | Raw bytes | Deflated | Encode ms | vs baseline (deflated / encode) |
|---|---|---|---|---|
| bincode (live baseline) | 286,116 | 70,156 | 0.38 | — |
| msgpack compact (positional) | 355,900 | 76,744 | 0.56 | +9% / +47% — no tolerance gained |
| **msgpack NAMED (struct-map)** | 682,437 | **93,432** | 0.75 | **+33% / +97%** |
| CBOR (named maps) | 682,140 | 92,890 | 0.62 | +32% / +63% |
| JSON (named ceiling) | 1,040,463 | 121,743 | 1.10 | +74% / +189% |

**Findings.**

1. **Deflate does NOT absorb field-name tags**: full field-level tolerance (msgpack-named / CBOR)
   costs ~+33% deflated — the segment-relevant number. At 8 rooms that is 93KB vs 70KB deflated
   (≈125KB vs ≈94KB after base64) against the 10×100KB segment budget; the tax scales with the
   empire exactly when headroom matters most.
2. **CPU is acceptable** for every candidate: worst named encode 0.75 ms native ⇒ plausibly 2–4 ms
   on wasm — real but affordable. Size, not CPU, is the binding constraint.
3. **The emerging recommendation is SELECTIVE tolerance**: the byte-dominant stores (`RoomData`,
   `RoomPlanData`) are the most shape-stable, while the stores that churn shape (missions,
   operations, squad state) are small. Encoding only the churny stores named — bincode for the
   bulk — should buy most of the migration value for a fraction of the +33%. **Round 2 must
   produce the per-store size breakdown** to confirm, plus the migration simulation (field
   add/remove per scheme) and a wasm-side timing.

Harness: `encoding_bench` in `operations/claim.rs::live_world_decode` (host-only dev-deps
`rmp-serde`/`serde_cbor`; run like `decode_live_world`).

## Viability of selective tolerance under the single-pass constraint (operator question, 2026-08-23)

**The constraint is real:** the live serialize is ONE `SerializeComponents` call driving ONE
serializer type across the whole 12-store tuple — serde cannot switch encoders per tuple element,
so mixed encodings *inside* the existing single pass are not implementable. Selective tolerance
survives through two mechanisms:

- **A — sectioned passes (preferred).** Split the tuple into a stable-bulk group (bincode) and a
  shape-churny group (msgpack-named), one `SerializeComponents` call each, concatenated behind a
  tiny envelope (section tag + length + per-section version byte). Deserialize is N passes sharing
  the marker allocator — **verified against specs 0.20 source**: `MarkerAllocator::retrieve_entity`
  returns the existing entity for a known marker id, so pass 2 attaches components to pass 1's
  entities by construction. Bonus: this IS ADR 0002's deferred Stage-2 (per-section isolation — a
  bad section resets alone), and the envelope version byte gives the bincode sections coarse
  hand-migration as a middle tier. Pass-count CPU is noise: total bytes dominate, and the extra
  cost is one walk over ~250 marked entities against a 0.38 ms full encode.
- **B — nested blobs (rejected-unless-A-fails).** One pass, churny components serde-emit a byte
  blob containing their own named encoding. Works, but double-buffers every wrapped component and
  buries encoding policy inside component impls.

**Round-1 gap this question exposed:** the bench did not time DEFLATE, which runs every tick and
scales with RAW bytes — full-named is 682KB raw vs 286KB (≈2.4× the per-tick compression work) on
top of the +33% segment cost, so whole-stream named is worse than the round-1 table suggests.
Sectioned encoding also wins this axis (raw stays near baseline). **Round 2 additions:** bench the
full pipeline (encode + deflate), prototype the two-pass split (measure envelope overhead + verify
the shared-allocator round-trip on the real payload), and the per-store size breakdown.

## Round 2 (2026-08-23): full pipeline, per-store breakdown, tolerance proof — and the decision

Operator steer for this round: *prefer paying for ONE encoding everywhere over pushing multiple
encodings through the code — if the segment budget allows.* The measurements say it does.

**Full pipeline** (encode + the live `encode_buffer_to_string` gzip+base64; native `--release`;
segment budget = COMPONENT_SEGMENTS 50–53 = 400KB chars, all 10 segments already allocated):

| Scheme | Raw | Seg chars | % budget | Encode+gzip ms | Decode ms |
|---|---|---|---|---|---|
| bincode (live) | 286,116 | 93,568 | 22.8% | 4.29 | 1.96 |
| **msgpack NAMED** | 682,437 | **124,600** | **30.4%** | 6.28 | 2.84 |
| CBOR named | 682,140 | 123,880 | 30.2% | 6.04 | — |
| JSON | 1,040,463 | 162,348 | 39.6% | 10.67 | — |

**Per-store breakdown** (the scaling model): **`RoomPlanData` is 86% of all bytes** (252,606 of
293,849 bincode) and is the most shape-stable store in the corpus; every shape-churny store
(missions/operations/squads/jobs/queues) is 1–7KB. Named's per-store multiplier is large on the
small stores (4–8×) but their absolute cost is trivial; gzip crushes the repeated keys (raw ×2.4 →
segment chars only ×1.33).

**Tolerance proven twice**: (a) the mechanics pin `named_encoding_tolerates_field_add_and_remove`
— a V1 payload decodes into a V2 shape (field added with `#[serde(default)]`, field removed →
ignored) under struct-map, and the identical migration **errors under positional bincode**; (b) the
**real live world round-trips through msgpack-named** end-to-end (all `ConvertSaveload` types +
markers) with no code changes beyond the serializer swap. The `ConvertSaveload` derive **clones
field attributes verbatim** into its generated `SaveloadData` structs (verified in specs-derive
0.4.1 source), so `#[serde(default)]` discipline on new fields is the entire ongoing cost.

### Decision

**Whole-stream msgpack struct-map (rmp-serde named mode) — ONE encoding, no sectioning.**

- **Segment budget is not binding**: 30.4% today at 8 rooms; growth is `RoomPlanData`-dominated
  (∝ planned rooms), projecting ~60–65% at ~2.5× empire scale — comfortable, and monitorable.
- **CPU cost is bounded and known**: +46% on the serialize pipeline slice (+2.0ms encode+gzip,
  +0.9ms decode, native). The wasm-side absolute is the one residual to confirm live.
- **Complexity is minimal**: a two-line serializer/deserializer swap in `game_loop.rs` (+ the
  decoder test), ONE transition WFV bump, and `#[serde(default)]` on new fields thereafter. The
  sectioned-envelope design (previous section) is **rejected as unnecessary complexity** at these
  numbers — it survives in this document as the escape hatch if segment pressure ever
  materializes, alongside the second lever: moving `RoomPlanData` (86% of bytes, regenerable by
  the planner) out of the tolerant stream.
- WFV remains the outer fingerprint for genuine semantic breaks; `reset.*` stays the escape hatch.

**Post-adoption watch item**: live serialize CPU (wasm) and segment chars per tick; both are
directly observable and each has a documented lever if it trends wrong.
