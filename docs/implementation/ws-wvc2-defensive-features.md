# WvC-2 — military completion wave 2 (defensive features)

**Workstream:** WvC-2 (Phase 3) · **Advances:** ADR 0008a (T-DEF-1, T-DEF-5, T-POS-5), ADR 0037
(T3), ADR 0028 (multi-squad lane contention) · **Status:** active

## Resume point

WvC-1 shipped 2026-08-23 (its doc holds the live-watch). Working the ranked list below, Wave-B
style: pure kernel + RED-verified pins per item, batch-ship per RULING-8.

## Target

The defensive half of the "peak effectiveness" program: owned rooms defend from cover instead of
open-field kiting, safe mode fires BEFORE the last rampart breaks instead of after the spawn is
chewed, combat creeps stop getting shoved through exits, and the neighbour-threat kernels get
their candidate feed.

## Plan (ranked by effectiveness-per-risk)

- [x] **T-POS-5 — exit-tile discipline** (decision `3d451ac`): flat `EXIT_TILE_SURCHARGE`
      (3×SCALE, threats-present only) in `score_tile` — dominates preset mixing, not ∞ (lethal
      interior may still eject, T-POS-8(b)). RED-verified pin. Fence deferred to the batch ship.
- [x] **T-DEF-1 — rampart cover** (decision `47a163a`, eval `47f9d0b`): NO DTO threading needed —
      `structure_to_dto` already carries friendly ramparts, and the sim's ownership is
      viewer-relative. `ThreatField::build_covered` zeroes maintained-friendly-rampart tiles (the
      engine redirect, which the sim engine models faithfully) → the TAKEN term, EV risk, survival
      veto, and traversal pricing ALL inherit cover from one point; `MIN_RAMPART_HOLD`=10k filter;
      empty cover ⇒ byte-identical. 3 RED-verified pins (incl. one rewritten after its first RED
      check showed it wasn't discriminating). Anchoring EMERGES from scoring — no state machine, no
      hysteresis (per the per-tick-optimal rule); kiting fallback is automatic.
- [x] **T-DEF-5 — predictive safe-mode arm** (`8502af9`): `predictive_breach_arm` kernel OR'd
      with the reactive floor — breach race projection (rampart hits/breach dps vs hostile
      pool/net tower dps), watch floor 10k, out-healed-defense = lost race; downstream
      upgrade_blocked/charge/cooldown checks unchanged. RED-verified 6-case pin.
- [x] **0037 T3 — candidate emission** — CLOSED BY RULING, no code: emitting candidates from the
      T3 seam (or feeding `observe_neighbours`/`neighbour_threats` into offense) would contradict
      BOTH the ADR 0037 T3 design of record ("the seam must remain structurally incapable of
      opening a new attack path") AND the D27 operator decision (bare armed neighbours are ignored
      — the standing-intercept drain class). The worthwhile towered cases already emit through the
      EV+winnability-gated InvaderCore/ResourceDenial arms; the T1/T2 kernels are sim/harness
      kernels by design (they drive the `run_v1_flow` offline proofs). Tracker §6/§8 rows updated.
- [x] **0028 multi-squad lane contention** — RE-ROUTED to the HARNESS lane: it is scenario
      coverage (ADR 0028 "Scenario coverage" beds 1 + 3 — forming-under-lane-contention and
      multi-squad claim pacing), part of the `run_defended_lifecycle` closeout already gated on
      B-1. Nothing bot-side to build; the shared `claim_pacing` kernel (WvC-1) is what those beds
      will exercise. Tracker 0028 row updated.
- [x] Batch-shipped to MMO 2026-08-23 (hot swap `1fb233b30416`). Live-watch OPEN: cover-anchoring + predictive safe-mode on the next owned-room defense; exit discipline on any kite fight.

## Design deltas

- (running)

## Verification

Kernel pins RED-verified per item; suite + wasm + fence when kernels change; T-DEF-1/T-DEF-5 have
0008a sim metrics (defender survives siege on rampart; safe mode fires with spawn intact) — run
what the offline harness supports now, the private-soak versions when B-1 unblocks.

## Log

- 2026-08-23 — created; WvC-1 shipped same day. Start: T-POS-5.
- 2026-08-23 — T-POS-5 (`3d451ac`), T-DEF-1 (`47a163a`+eval `47f9d0b`), T-DEF-5 (`8502af9`) landed; 0037-T3 + lane-contention closed by ruling. Fence + batch-ship next.
- 2026-08-23 (ship) — batch deployed to MMO (hot swap `1fb233b30416`, 48.69% code limit); tail clean (no panics/deser; known transient foreman placement warns only). Live-watch open: next owned-room defense exercises cover-anchoring + predictive safe mode; any kite fight exercises exit discipline.
