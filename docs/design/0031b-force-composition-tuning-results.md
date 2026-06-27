# ADR 0031b — Force-composition tuning results (sweep note)

- **Status:** Results note (output of ADR 0031 D16/D17 / ADR 0031a §4 tournament sweep)
- **Date:** 2026-06-27
- **One line:** Reports the `CompositionParams` Tier-1 (count × margin) tournament sweep across all four bed regimes, maps the emergent per-regime strategy, and concludes the research-grounded seeds (`hold 1.3 / over 1.5 / dyn 1.0 / mem 3000 / commit 0`) are **confirmed Pareto-optimal across regimes and KEPT** — no `CompositionParams::default()` change.

Reads against ADR 0031 §2c/§4 (the "Tournament lens (P6, D13/D16)" entry) and ADR 0031a (the knob set + the tiered sweep plan). 0031a is the *plan*; this is the *result*.

---

## 1. Methodology

The sweep is a pure, bit-deterministic scorer driven by environment variables — no code edits per run, no live Docker.

- **Harness:** `screeps-combat-eval` test `harness::param_sweep::tests::sweep_composition_params` (`src/harness/param_sweep.rs`, `#[ignore]`, run with `--ignored`). It reads `SWEEP_HOLD` / `SWEEP_OVER` / `SWEEP_MEM` / `SWEEP_COMMIT` (comma-separated axis values) + `SWEEP_REGIME` (`all` | `structure` | `creep` | `defended`), takes the Cartesian product, scores each point with `evaluate_params(&CompositionParams, regime)`, and rank-orders winning-but-efficiently (gates-held, then win-rate DESC, then mean-spawn-cost-per-win ASC). Release profile, rayon-parallel; a full 48-point grid runs in ~0.2–1.3 s.
- **The pure scorer** (`evaluate_params`, `param_sweep.rs:140`) folds four bed families into one `ParamScore`:
  - **OracleCalibration** (always runs — the FP/FN substrate): 80 `RandomDefendedBase` scenarios → `fp_rate` / `fn_rate`.
  - **SizingWins** (structure regime): structure-/core-sizing win-rate + spawn-cost-per-win.
  - **CreepClearWins** (creep regime): creep-clear / raid / SK win-rate + cost.
  - **Acceptance defended beds** (defended regime): the four `acceptance_regimes()` (`param_sweep.rs:120`) — canonical 30k-rampart + 100k-tower + 2-guard, 50k-rampart + light guard, tower-only + 100k-tower + guard, corridor-choke + guard — each must return `Killed`.
- **The gate** (the hard `gates_held` predicate): `FP ≤ 0.010` **AND** `FN ≤ 0.200` **AND** every *run* acceptance bed `Killed`. A regime filter that skips a family makes that family's gate component vacuously satisfied — so `regime=creep` is graded purely on creep-clear win-rate + the always-on FP/FN, etc. The acceptance Kill is the cross-regime energy floor; the FP gate is the cross-regime over-commit ceiling.
- **Determinism fence:** `tests::sweep_point_is_deterministic` asserts same `params` ⇒ bit-identical `ParamScore`; the underlying sim is bit-deterministic (`sim_is_deterministic_over_rounds`).

**Sweep design** (ADR 0031a §4 Tier-1, coordinate-descent then a joint broad grid):
- Broad Tier-1: `hold ∈ {1.15, 1.3, 1.45, 1.6}` × `over ∈ {1.3, 1.5, 1.8}` × `mem ∈ {1300, 2000, 3000, 5400}` × `commit ∈ {0}`, run per regime (`all`, `structure`, `creep`, `defended`).
- Per-regime narrow / floor passes to map the FP cliff and the energy floor edges.

`SWEEP_TOUGH` is accepted-but-ignored by the harness (the TOUGH ladder is internal to `optimize_composition`'s `TOUGH_LADDER`, not a `CompositionParams` field). `dynamic_margin` / `w_energy` / `w_creep` were held at their Tier-1-frozen seeds (1.0 / 0.001 / 0.0); they are Tier-4 efficiency/tie-break knobs.

---

## 2. Per-regime winners + the emergent strategy map

Every gate-holding point shares `fn_rate = 0.0962` (the FN gate ≤ 0.200 is **never** the binding constraint — it is set by the always-on calibration substrate, not by the params under test). The discriminators are **win-rate**, the **FP gate** (≤ 0.010), and the **acceptance Kill**. `member_energy` is the dominant live axis; `over_power_margin` bites only where there are creeps to out-mass; `hold_margin` is flat everywhere in the unboosted/creep-light v1 beds.

| Regime | Best (cheapest gate-holding) point | win | fp | cost/win | Binding constraint | Emergent strategy |
|---|---|---|---|---|---|---|
| **all** | hold 1.15 / over **1.5** / mem **3000** (Default co-best at hold 1.3) | 0.926 | 0.000 | 11 502 | acceptance Kill (needs mem ≥ 3000) **and** the FP cliff | A few heavyweight ~3000e members. `over=1.3` drops a few creep-clear beds (win 0.926→0.889); `over=1.5/1.8` co-best, 1.5 cheapest. |
| **structure** | hold 1.15 / over 1.5 / mem **800–1300** | 1.000 | 0.000 | 2 350–3 600 | FP cliff (mem ≥ 1500 trips fp = 0.0625) | Smallest/cheapest force — structures don't fight back, so over/hold are inert no-ops; member-energy is the only live knob, monotone-cheaper down to the per-member floor. |
| **creep** | hold 1.15 / over **1.6** / mem **1300** | 1.000 | 0.000 | 4 400 | a *narrow band*: mem ≥ 1300 to clear, mem < 1600 to not trip FP; over ≥ 1.6 to win all beds | More, cheaper members at a **modest** over-power margin. Heavyweight 3000e members are strictly dominated here, and over-arming members (mem ≥ 1600) trips the FP gate. |
| **defended** | hold 1.15 / over 1.3 / mem **3000** (Default metric-identical) | n/a (no win beds) | 0.000 | n/a | acceptance Kill: mem ≥ 3000 HELD, mem ≤ 2000 FAIL | One thing only — enough per-member energy to finish the breach/kill on the heavyweight towered/ramparted cores. hold/over completely flat (no creeps, no square-law). |

**The strategy map in one line:** the *cost-efficiency tiebreak* pulls each single-regime winner to a different member-energy edge (structure → low ~800–1300, creep → mid ~1300, defended → high ≥3000), but the **cross-regime gate is the intersection of all of them** — and only `member_energy ≥ 3000` survives the defended-Kill floor while staying winning on structure + creep. `over_power_margin` lands at **1.5–1.6** (1.5 is the cheapest co-best in `all`; creep wants ≥ 1.6; structure/defended don't care). `hold_margin` is free to sit anywhere in 1.15–1.6 (flat in v1). `5400 == 3000` and `mem == PREFERRED` everywhere because `assemble_force`/`optimizer_ceiling_budget` clamp the probe to `PREFERRED_MEMBER_ENERGY = 3000` (`composition.rs:362`, `:603`) — so `mem > 3000` is inert, not better.

**The surprise the gate is designed to catch:** in `structure`-only and `creep`-only the cheap mem=1300/2000 points WIN (and are far cheaper); in `all`/`defended` those same points FAIL because they cannot field a force that kills the heavyweight defended core. The acceptance Kill is exactly the constraint that rejects a force that wins the easy beds but folds on a towered/ramparted core — the original ADR-0031 failure mode, generalized.

---

## 3. Recommended default + rationale

**Recommended: KEEP `CompositionParams::default()` unchanged** — `hold_margin = 1.3`, `over_power_margin = 1.5`, `dynamic_margin = 1.0`, `member_energy = 3000`, `commit_ev_threshold = 0`, `w_energy = 0.001`, `w_creep = 0.0`.

Rationale:
1. **It is on the cross-regime Pareto front.** In the `all` broad grid the gated surface is FLAT — Default is metric-identical to the #1 point (win 0.926, fp 0.000, fn 0.0962, cost/win 11 502). No gated point in `all` outranks it.
2. **No single default beats it across all gates.** The single-regime winners that *do* beat Default in isolation (structure: mem 800–1300; creep: hold 1.15/over 1.6/mem 1300) **fail the defended acceptance Kill** (mem < 3000 cannot finish the heavyweight core) and/or the FP cliff (mem ≥ 1500). The seeds are the *intersection-feasible* point.
3. **The seeds were beaten only inside lower-stakes regimes, never globally.** Per the rule in ADR 0031 §4 / the task: change Default only on a STRICT cross-gate improvement. There is none — the four-bot-grounded seeds (0031a) sit dead-center of the field's 1.2–1.5 bracket and dead-center of the gated bracket.
4. **`over = 1.5` is confirmed, not merely tolerated:** in `all` it is the cheapest co-best (over=1.3 loses creep-clear beds; over=1.8 ties win but is no cheaper); creep prefers ≥ 1.6 but that point fails the defended gate. 1.5 is the best feasible compromise.
5. **`mem = 3000` is confirmed by two independent boundaries:** it is the *floor* for the defended Kill and below the FP *ceiling* in the heavyweight beds; the PREFERRED clamp makes anything above it inert.

**Regime tension (documented, not resolved by a default change):** a default tuned for *resource-denial / creep-clear-only* contexts would prefer more-cheaper members (mem ~1300, over ~1.6); a default tuned for *structure razing* would prefer the cheapest member-energy. The single global default cannot capture both edges — it sits at the defended-Kill floor, which is the correct conservative choice because the gate that the global default must satisfy is the union of all beds. The path to *regime-aware* params (a per-objective `CompositionParams`) is a Tier-2/3 follow-up below, not a Tier-1 default change.

**Gates at the recommended (= current) Default:**
- `cargo test -p screeps-combat-decision` → 179 passed, 0 failed.
- `cargo test -p screeps-combat-eval` → 65 passed, 0 failed, 19 ignored; including `default_params_hold_the_structure_gates`, `oracle_sized_force_forms_and_kills_a_defended_core`, `assembler_kills_across_defended_regimes`, `calibration_is_deterministic`.
- OracleCalibration: FP = 0.0000 (≤ 0.010), FN = 0.0962 (≤ 0.200). SizingWins / CreepClearWins / acceptance Kill: HELD. (No `default()` change → no re-baseline needed; the gate suites above run at Default.)

---

## 4. Open Tier-2/3 items (next sweep)

The Tier-1 count × margin sweep is exhausted (the seeds are confirmed). The remaining levers are the body/archetype axis — they require *extending the search*, not just sweeping `CompositionParams`. From ADR 0031a §2B/§4:

- **Tier-2 — `archetype` (weapon select) as a tuned EV dimension.** The biggest gap and the original failure (WORK-siege-vs-guard = 0 damage). `{RangedBlob, MeleeAttack, WorkDismantle, derived Drainer}` swept per bed-type, with `fighter_role` demoted to a feasible-set constraint. Expected emergent result: RangedBlob wins creep-defended + immune-core beds, WorkDismantle wins dismantle-able-ring beds.
- **Tier-2 — `tough_fraction` / EHP.** Already wired as the internal `TOUGH_LADDER = [0.0, 0.1, 0.2]` in `optimize_composition`; promote it to a graded, tower-present acceptance bed so `tough > 0` becomes *required* to pass (today every v1 bed passes at tough 0 — the beds don't yet punish bare bodies under sustained tower fire). Couple to heal (broken-TOUGH/tick must be refillable).
- **Tier-3 — drain / kite commit mode + `engage_range`.** The only viable unboosted path vs multi-tower rooms (50 HEAL parts to out-heal one tower point-blank is infeasible). Needs the cost-side EV branch + a multi-tower bed where Siege is infeasible so the sweep *selects* Drain. Leverages the existing `AssaultMode::Drain`.
- **Tier-3 — within-member `attack_to_heal_mix` (~0.75).** We split whole members but never tune the part mix inside one; costs small-scale self-sustain efficiency.
- **Tier-4 — cost-weight + secondary** (`w_energy`, `w_creep`, `dynamic_margin`, `importance_margin`, retreat/reengage). Narrow tie-break ranges; only sweep `dynamic_margin > 1.0` if a growing-threat under-size is observed, and the retreat/reengage hysteresis only if edge-thrash recurs (ADR 0031 invariant: no hysteresis without observed oscillation).
- **Regime-aware params.** If the regime tension in §3 becomes load-bearing live, carry a per-objective `CompositionParams` (e.g. a cheaper-member creep-clear/raid profile vs the conservative defended-Kill profile) rather than one global default — but only on demonstrated need.
- **The optimizer_ceiling_budget residual.** The renamed 3+5 ceiling survives only as `emit_requirement`'s winnability budget; a budget-free `emit_requirement` (the per-candidate EV/commit is already budget-free) retires it. Not a tuning item but it removes the last presumed-shape constant the sweep has to reason around. See ADR 0031 §2c.
- **P6 position-weights re-sweep.** The assembler changes WHICH forces are fielded, so the ADR-0019 position-utility weights + the tournament/exploitability tuning are re-swept once the archetype/drain dimensions land (ADR 0031 §4 Tournament lens).

---

> **Provenance.** Sweeps run 2026-06-27 against `screeps-combat-eval` `harness::param_sweep` on the bit-deterministic sim, release profile, rayon-parallel. Per-regime raw outputs were written to scratchpad files during the runs (broad Tier-1 + narrow/floor coordinate-descent per regime). Methodology + knob set: ADR 0031a. Architecture: ADR 0031 §2c/D16/D17.
