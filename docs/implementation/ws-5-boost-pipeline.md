# WS-5 — boost pipeline (Phase 5: ADR 0010 L0 → ADR 0041)

**Workstream:** WS-5 (Phase 5) · **Advances:** ADR 0041 (consumer), ADR 0010 (supply) ·
**Status:** active

## Resume point

Phases 3+4 shipped + live-validated 2026-08-23. Starting ADR 0041's dark-first phasing (§7,
operator-ratified O1(b): consumer first against the labs' existing autonomous brew; the
demand-driven ADR 0010 L1/L2 planner follows once the consumer is proven).

## Target

The bot fields BOOSTED combat forces where the EV says so: boost tier as an EV-search axis
(full T0→T3 ladder, uniform per body), supply-clamped with a T0 floor (empty stock ⇒
byte-identical unboosted bot), cost market-priced with the O4 trust-gated fallback. Closes review
risk R1's own-side half and the 0031 §5 escalate-vs-abandon gap (#38); unblocks 0019
boosted-TOUGH, 0020-TOUGH, 0008a Tier 3, 0008 S2.

## Plan (ADR 0041 §7 phasing; P2's "one WFV bump" is OBVIATED by ADR 0047 — an additive
`#[serde(default)]` field is now reset-free; recorded as a design delta)

- [x] **P0a — the EV axis, dark** (decision `04cc020`) (this increment): `BoostTier` (T0–T3, output ×1/2/3/4) +
      `BOOST_LADDER` in `optimize_composition` — tier-scaled requirement (¼ the parts at T3),
      caps ×m, boost cost per REAL body part (O4 stage-3 conservative constants as the P0
      default), lowest-tier-at-equal-EV tie-break (D1). Carried via `CompositionParams`
      (`boost_max_tier` default T0 ⇒ DARK, byte-identical). Pins: existing calibration gates
      unchanged; the T-TOWER-3 proof (synthetic T3 makes a T0-deferred towered target
      fieldable+committable); determinism; tie-break.
- [~] **P0b — seam generalization** RE-SCOPED: the optimizer-internal per-tier emit (P0a) already covers D3's winnability effect; the remaining seam-signature churn (bool→tier) waits for a consumer, and TOUGH-reduction is the 0019 boosted-TOUGH item (post-0041 by schedule). Nothing blocks P3.
- [x] **P1 — supply machinery** (decision `d217a3d`, super `5b29c9a`): `available_boosts` populated (labs+storage+terminal, 18 compounds); `max_supplied_tier` pure clamp (fully-suppliable per family, RED-verified); offense wiring GATED on `features.military.boost_military` (default OFF — the P3 activation switch; sizing before the apply path exists would field ¼-size forces); boosted-verdict override at the war.rs skip. FOLLOW-UPS (before/with activation): the O4 market-fed `mineral_value_e` resolver (constants govern till then — over-pricing is the safe direction); reservation vs concurrent requests (physical only at P3).
- [x] **P2 — persisted tier** (decision, this commit): `CombatBodySpec.boost` serde-default T0 (old payloads tolerant — pinned); optimizer stamps the winning rung per slot; `required_boosts()` real (30×parts per family incl. derived MOVE). Spawn-side attach deferred INTO P3 (the job re-derives from its slot's spec — no extra field needed).
- [ ] **P3 — apply**: bounded `AwaitBoost` job state → ADR 0010 boost station → `boostCreep`;
      falls through unboosted on deadline/stock-loss. BoostQueue keyed by DemandId (EP-1.7).
- [ ] **P4 — sweep**: which rungs EV-win per bed (the O3 full-ladder validation) + the O4
      approximation validation, over the chokepoint basket.
- [ ] ADR 0010 L1/L2 demand-driven supply planner (after the consumer is proven live).

## Design deltas

- ADR 0041 §7 P2's "one WFV bump / one loud reset" is OBVIATED by ADR 0047 (post-dates it):
  `CombatBodySpec.boost` with `#[serde(default)]` decodes old payloads as T0 with no reset —
  the T-HEAL-3a `effective_hits` precedent.

## Verification

Per phase, per the ADR's own validation lines; suites + fence per RULING-8; P1+ live-verified
against whatever the autonomous labs already stock.

## Log

- 2026-08-23 — created; Phases 3+4 shipped same day. Start: P0a.
- 2026-08-23 — P0a landed (decision `04cc020`): tier axis + per-tier ceiling assessment + real-body boost cost; 3 RED-verified pins (T-TOWER-3 proof green); 355/334/114 + fence green. DARK (T0 default) — byte-identical live, so NOT separately deployed; rides with the P0b/P1 batch. NOTE for P1: the CALLER-side winnability gate (`plan.winnable()` in doctrine/choose_fielded_comp paths) still reads the T0 assessment — the optimizer unlocks boosted fielding, but the caller's defer must learn the boosted verdict when supply goes live (wire at P1).
- 2026-08-23 — P1 (`d217a3d`+`5b29c9a`) and P2 (`78e70b8`) landed; P0b re-scoped (optimizer-internal emit covers D3's winnability; TOUGH → 0019). All dark behind `boost_military=false`. Next: P3 — the AwaitBoost apply path (read ADR 0010 §4 for the boost-station/queue discipline first).
