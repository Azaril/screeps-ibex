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

- [ ] Bench harness over the real payload (bincode baseline / msgpack named / msgpack compact / CBOR)
- [ ] Migration-tolerance simulation per scheme (field add/remove, enum variant add)
- [ ] wasm-side timing (the number that matters) — needs a wasm bench lane; native first
- [ ] Write results into 0047; decide granularity (field vs per-section hybrid); promote to Decided

## Design deltas

None yet.

## Verification

Bench runs on a same-tick real payload; sizes reported post-deflate (the segment-relevant number).

## Log

- 2026-08-23 — doc created; harness construction begins.
