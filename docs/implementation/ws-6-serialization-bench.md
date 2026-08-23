# WS-6 — ADR 0047 encoding experiments

**Workstream:** WS-6 (Phase 2.5) · **Advances:** ADR 0047 · **Status:** active

## Resume point

Building the bench: an `#[ignore]`d host test alongside `decode_live_world` that loads the REAL
live payload, rebuilds the specs World (the existing decoder path), then re-serializes it through
each candidate scheme, measuring encode/decode wall time and raw + deflated sizes. Dev-deps only
(`rmp-serde`, `ciborium`, `flate2`) — nothing enters the wasm build.

## Target

Real numbers in ADR 0047's experiment matrix → promote it to Decided with a chosen scheme.

## Plan

- [x] Bench harness over the real payload — round-1 numbers in ADR 0047 (named tolerance = +33% deflated; CPU fine; selective-tolerance hybrid emerging)
- [x] Round 2: full-pipeline timing (gzip via the live codec, vs the real 400KB budget), per-store breakdown (RoomPlanData = 86% of bytes), tolerance pin + REAL-world named round-trip. Sectioned prototype SKIPPED — the data made it moot (rejected as unnecessary complexity in the ADR decision).
- [ ] IMPLEMENT: swap the serializer/deserializer in game_loop.rs to rmp-serde struct-map (one WFV transition bump, ships batched per RULING-8); then watch live serialize CPU + segment chars — the wasm-side confirmation happens on the real thing
- [x] Results + DECISION in 0047 (whole-stream msgpack named; sectioning rejected as unnecessary); **promoted to Decided**

## Design deltas

None yet.

## Verification

Bench runs on a same-tick real payload; sizes reported post-deflate (the segment-relevant number).

## Log

- 2026-08-23 — Harness built + round 1 run on the real payload. Key result: field-name tags survive deflate (+33%), so full field-level tolerance is expensive; CPU is not the constraint; selective per-store tolerance is the emerging design. Next: per-store breakdown.
- 2026-08-23 (later) — Operator challenged single-pass viability: confirmed mixed encoding inside
  one SerializeComponents call is impossible; sectioned passes (shared marker allocator, verified
  in specs source) is the viable mechanism and also delivers 0002 Stage-2 isolation. Round-1 gap
  found: deflate time (scales with RAW bytes) was never measured — full-named is worse than the
  table suggests. ADR updated.
- 2026-08-23 (round 2) — Full pipeline + per-store data landed the decision the operator's
  simplicity steer pointed at: ONE encoding (msgpack struct-map) everywhere. Segment budget not
  binding (30.4% today, RoomPlanData-dominated growth); tolerance proven on the REAL world
  round-trip; ConvertSaveload attr propagation verified in specs-derive source. 0047 → Decided.
  Next: the game_loop swap (one WFV bump, batched per RULING-8).
