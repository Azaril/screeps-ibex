# WS-4 — R19 chokepoint re-tune (Phase 4)

**Workstream:** WS-4 (Phase 4) · **Advances:** ADR 0044 (R19 cross-sim finding), ADR 0025 §12,
ADR 0019 S4-TUNE, ADR 0026 L6c, 0031a/b re-sweeps, 0032 `value_e`, 0024 FU#4 · **Status:** active

## Resume point

WvC-1/WvC-2 shipped + live-validated 2026-08-23 (CPU 34/140, bucket full, 0 panics; live-watch
passive until a real fight). Phase 4 begins: the R19 re-tune FIRST (RULING-6 — it gates every
other kernel-parameter item), then the downstream sweeps.

## Target

The combat kernel's position-shaping parameters proven on REALISTIC chokepoint terrain, not just
open/synthetic beds. R19 (ADR 0044, 2026-07-09): the tuned `open_combat` edge over `default`
ranges −750..+890 across generated cave seeds (consistent −835 on OpenField) — the tuning does
not generalize; MMO rooms are chokepoint-heavy. Fix = `Bed::Generated` in the tournament basket,
re-run the kernel grid, adopt the cross-terrain winner, re-pin the EXP-* canaries.

## Plan

- [x] **Basket extension** (eval `940f739`): `chokepoint_comp_basket` = synthetic + imported + 6
      ADR-0044 cave seeds × comps (39 entries); `r19_chokepoint_retune` pass with per-regime
      payoff breakdown + maximin ranking.
- [x] **Kernel re-tune run** (13.7s release): winner = `ranged_duel_kite` `{0,3,14,3,2}` — the
      ONLY config positive in ALL regimes ([+513/+315/+131] vs default, maximin +131, mean +319).
      The live `open_combat` (`a1-i6-tight-s2`) measured [−839/+556/−6], rank 49/54 — R19
      quantified: the June tuning did not survive 3 months of kernel changes + real terrain.
- [x] **Adopt + re-pin** (decision `a7acb0b`): `open_combat()` → the winner (used by OpenCombat +
      SafeModeHold; breach/drain untouched; `default` stays as the 0-baseline fallback). ADR 0026a
      rejection REVERSED with full reconciliation (both eras' verdicts stand — the landscape moved).
      Canaries + 352/334/114 suites + fence (release, 18s) green. Ship with the next batch.
- [ ] **Downstream (each gated on the adopted params)**: 0031a/b re-sweeps under `w_energy=1.0`
      (`sweep_composition_params`), 0019 S4-TUNE, 0026 L6c weight sweeps, 0032 `value_e`,
      0024 FU#4, 0033 kite retune. Sequence one at a time; amend each owning ADR's conclusions.

## Design deltas

- (running)

## Verification

Tournament reports (payoff tables + Nash) recorded here; canaries + suites + fence green before
any adoption ships; live-watch after ship.

## Log

- 2026-08-23 — created after WvC-1/WvC-2 shipped + live-validated. Start: basket extension.
- 2026-08-23 — R19 core re-tune DONE same-session: basket + pass (`940f739`), winner adopted (`a7acb0b`), 0026a reversed-with-reconciliation. Process note: run the fence in RELEASE (18s vs 385s debug). Next: ship, then the downstream sweeps (start 0031a/b under w_energy=1.0).
- 2026-08-23 (ship) — R19 adoption deployed (hot swap `9913ef980109`). WATCH note: both post-deploy tails showed a one-tick burst of `INTEGRITY: dead squad ref scrubbed` (a squad retiring with living members right after the hot-swap reset — the REC-009b backstop handles it; members recall via job fallback). If it recurs on every deploy it is re-field churn per swap (ObjectiveGone/claim timing on the reload tick?) — attribute then.
