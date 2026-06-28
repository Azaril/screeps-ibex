# ADR 0032 — EV-Optimal Squad↔Objective Assignment (P-AUCTION)

Status: PROPOSED 2026-06-28. Task #28 (P-AUCTION). Extends ADR 0027 (the reassignment /
merge lifecycle + the objective queue), reuses ADR 0031 (the composition EV machinery), and
makes concrete the cross-goal EV currency ADR 0020 §11 deferred. The decision/identity changes
live here; the doctrine/body specifics cross-reference ADR 0026/0031.

## Problem

ADR 0027 v1 assigns squads to objectives **greedily**: `reconcile` classifies per squad, and
the manager picks the reassign/claim target via `objective_queue::best_reassignment_near` /
`best_unclaimed_near_excluding` — ranked `priority → room_distance → broad-class capability`.
Two defects the operator named:

1. **Not EV-positive.** Producer priority bands + Chebyshev proximity are **not** `P(win)·value
   − cost`. A squad can reassign into a fight it loses, or into a lower-net-value objective than
   continuing its current fight / recycling.
2. **Per-squad greedy, not global.** Phase A iterates squads in ECS order; each greedily claims
   its best + `covered`-marks it. First-come: squad A grabs the objective squad B was better
   suited for. Phase C's claim loop is the same shape.

## Decision — a global EV-maximizing matching over squads × {objectives + StayPut + Merge + Recycle}

Replace **both** greedy loops with one global assignment solve per scan.

### EV of a (squad, objective) pairing
```
EV(S, O) = P(win | caps(S) vs O.defense) · value_e(O)     [common-currency upside]
         − w_travel · travel_cost(S → O.room)             [reach delay/exposure]
         − w_opp    · opportunity                          [via StayPut/Recycle columns]
```
- `caps = S.composition.capabilities(member_energy)` (composition.rs) — the **existing squad's**
  surviving capability, read once (not an `optimize_composition` candidate search).
- `P(win)` reuses the ADR 0031 decomposition verbatim (`win_probability`, the undefended binary
  `p_kill` branch) — lifted into a shared pairing helper. Travel is priced *automatically* via the
  shrinking `onsite_window` (shorter window → lower `deliverable` → lower `p_kill`) **plus** a
  small linear penalty for crossing rooms (replacing the ad-hoc proximity tie-break).

### EV currency — `value_e` (energy-equivalent), the ADR 0020 prerequisite made concrete
A pure per-kind valuation `objective_value::value_e(kind, intel) -> f32` in **energy-equivalent**
units, so all goal types are comparable in one matrix (today they are not: defense uses a
`DEFENSE_TARGET_VALUE = 1_000_000` sentinel; offense uses `score · OFFENSE_TARGET_VALUE_SCALE`):

| kind | `value_e` |
|---|---|
| Defend/Secure owned room | asset replacement cost + lost income over downtime + safe-mode/GCL penalty (large but **finite + comparable**) |
| Farm{Core} (lvl0 reserver) | denied-reservation income recovered |
| Farm{SourceKeeper} | SK net energy/tick × horizon − suppression upkeep |
| Farm{PowerBank} | the existing `estimated_roi` (already energy-equivalent) |
| Dismantle/Harass/raid | resource denial × strategic discount |

This is the *minimum* currency the auction needs — **not** the harder intra-engagement tactical
exchange-rate (focus/breach/drain EV), which is the S5 blob auction, out of scope here.

### The matching — Hungarian / Kuhn–Munkres
Dense `N × K` matrix: `N ≤ ~6` assignable squads (terminal/idle/forming, ≤ `MAX_CONCURRENT_SQUADS`
+ forming) × `K ≈ 12` columns (top-`C` objectives by a cheap pre-rank + `StayPut` + one
`Merge→Bk` per forming receiver + `Recycle`). Maximize total EV.
- **CPU trivial:** `O(N²·K) ≈ 430` int ops, **once per scan** (~every 2–10 ticks). The matrix
  *build* (N·C `capabilities()`+`win_probability` evals) dominates and is still cheap. Combat is
  `StageClass::Always` (never CPU-shed) so it must be bounded — it is.
- **Provably optimal** — the point of P-AUCTION (the sim test constructs a case where greedy is
  strictly worse).
- **"Auction" is the role; Hungarian is the implementation** at this N. Swap to the Bertsekas
  auction algorithm only if `MAX_CONCURRENT_SQUADS` ever goes CPU-governor-dynamic (ADR 0020 S5).
- **Determinism (hard):** `Vec`-ordered rows (stable id, never `Entity` index) + columns (by
  `ObjectiveId`); **integer-quantized EV** (`ev_q = (ev·1000) as i64`) *before* the combinatorial
  solve (per the ADR 0020 §6 no-float-into-a-discrete-branch rule); stable lexicographic
  `(row,col)` tie-break in the augmenting-path order. No `HashMap` on any path.
- Kernel: `screeps-combat-decision/src/assignment.rs` (`build_ev_matrix` + `solve_assignment`) +
  `objective_value.rs` (`value_e`).

### EV-positive gate
`StayPut` (re-score `EV(S, current_objective)` with current survivors) and `Recycle`
(`value_e(recycle_refund) − walk`) are **columns in the matrix**, so the optimal solution never
contains a net-negative move. `commit_ev_threshold` (the ADR 0031 knob, reused) is the floor that
prevents thrash on near-ties. A reassign must beat *continuing the current fight* — the biggest
correctness gain over v1.

### Merge / attach as a first-class column (the ADR 0027 v2 transfer, now EV-scored)
A `Merge→Bk` column for each forming receiver with an open pending slot, scored by the
**receiver's marginal P(win) lift**: `[P(win | Bk.comp + S.members) − P(win | Bk.comp)] ·
value_e(Bk.objective) − transfer_cost`. The **Lanchester pending-slot rule** (ADR 0027) is the
column-feasibility filter: a merge column exists only where the donor's members are role-
compatible with an open pending slot (so it's concentration, never a dilutive split — the rejected
case is simply never a column). Reassign-vs-merge-vs-recycle are thus chosen in one optimization.

### Integration with ADR 0027 v1
The global solve runs between Phase-A *classify* and *apply*:
1. `reconcile` still classifies the terminal per squad — but `ReconcileAction::Reassign` becomes a
   **row-admission signal** ("eligible to be re-matched"), not a greedy target pick. Wiped/GaveUp/
   Duplicate still retire (not assignable rows).
2. Build the matrix; solve; apply the gated solution: current→`Keep`; new→the existing **in-place
   rebind** (unchanged — only *which* `new_id` changes); `Merge→Bk`→the v2 transfer; `Recycle`→
   retire + zero-orphan recall.
3. **Reconcile feeds, does not subsume** — the commitment-lease + forming/travel-budget lifecycle
   is orthogonal and stays. `best_reassignment_near` is **deleted**; its filtering becomes
   column-feasibility (`EV = −∞` for claimed-by-another / backoff / capability-incompatible).
   `capability_class` stays a cheap pre-filter (and `−∞` for robustness). The `covered`
   double-claim guard is retired — Hungarian column-exclusivity makes double-claim impossible.
   Phase C (greedy field-new) becomes additional "about-to-field" rows, capped by the concurrency
   limits.

### Sim (the standing offline-provability requirement)
The optimizer is a **pure deterministic kernel**, driven offline like `reconcile`:
- Kernel tests: **the constructed greedy-suboptimal case** (2 squads × 2 objectives; assert
  `solve_assignment` beats a `greedy_baseline` on total EV — the headline proof); the EV-positive
  gate (a sub-threshold objective is not taken); merge-as-option (picks `Merge→Bk` over a marginal
  solo reassign; the dilutive column is absent); determinism (twice → byte-identical; permuted
  input → same assignment).
- Flow test: `run_auction_flow` extends `run_v1_flow` to N squads × M objectives with a
  greedy-vs-global toggle (the existing RED→GREEN discipline) — proves global-optimality in the
  *flow*, not just the kernel.

## Phasing
- **v1.1** — EV-score the (still per-squad) choice + the gate (no matrix yet): replace
  `best_reassignment_near`'s `priority.then(proximity)` with `max_by(EV)` + the StayPut/Recycle
  gate. Fixes defect (1), deployable alone. Sim: the gate + determinism kernel tests + a
  `run_v1_flow` EV-positivity assert.
- **v1.2** — the global Hungarian (`assignment.rs`) replacing both greedy loops + Phase C; no merge
  yet. Fixes defect (2). Sim: the greedy-suboptimal kernel test + `run_auction_flow`.
- **v2** — the `Merge→Bk` column class + the pending-slot guard (the ADR 0027 v2 transfer wired as
  an EV option). Sim: merge kernel test + a forming-consolidation bed.
- **later** — tournament-tune `value_e` weights (the `CompositionParams`/`param_sweep` lens);
  Hungarian→auction only if N goes dynamic.

## Non-goals / risks
- **No `WORLD_FORMAT_VERSION` bump** — the EV matrix is a transient per-scan structure, never
  serialized (the `RequiredForce`/`CompositionParams` discipline). `value_e` weights, if persisted
  as tunables, would touch serde — they needn't (env-driven at sweep time).
- Determinism is the load-bearing risk → integer-quantized EV + stable tie-break + no `HashMap`.
- The intra-engagement tactical exchange-rate (S5) is explicitly **not** part of this — the auction
  needs only per-objective `value_e`.

### Critical files
- `screeps-combat-decision/src/assignment.rs` (new — the Hungarian kernel) + `objective_value.rs`
  (new — `value_e`)
- `screeps-combat-decision/src/composition.rs` (the P(win) decomposition + `commit_ev_threshold` to
  lift into a shared pairing helper) · `lifecycle.rs` (`Reassign` reframed as row-admission)
- `screeps-ibex/src/military/squad_manager.rs` (Phase-A apply + Phase-C, the integration site) ·
  `objective_queue.rs` (`best_reassignment_near` retired → column feasibility)
- `screeps-combat-eval/src/harness/lifecycle.rs` (`run_v1_flow` → `run_auction_flow`)
