# ADR 0032 — EV-Optimal Squad↔Objective Assignment (P-AUCTION)

Status: **v1.1 IMPLEMENTED + DEPLOYED 2026-06-28** (the `value_e` energy-equivalent currency +
the `pairing_p_win`/`pairing_ev`/`quantize_ev` EV-of-pairing helper + EV-positive-gated **per-squad**
reassign/claim replacing greedy priority-then-proximity + enemy-creep-force pricing; super `98dc1e7`,
decision `b848c26`). **v1.2 (the GLOBAL Hungarian matching) IMPLEMENTED 2026-06-28** (super `f12a711`,
decision `2da289b`, eval `8543278`; `assignment.rs` replaces BOTH greedy loops with one deterministic
global EV-maximizing solve; transient matrix → no WFV bump). **v2 (the Merge column) IMPLEMENTED
2026-06-28** (super `a03ee91`, decision `43cc43e`, eval `9b26286`; transient matrix → no WFV bump).
**AUCTION COMPLETE** — reassign / claim / StayPut / Recycle / **Merge** are chosen in one global EV-optimal
solve per scan.
**v3 (economic-value-unlocked target EV) IMPLEMENTED 2026-06-28** (reach-bug #3): a combat target's value is
now the ECONOMIC value UNLOCKED by CONTROLLING its room — a FULL net-ROI from a PURE, REUSABLE
room-economics kernel (`screeps-ibex/src/room_economics.rs`) — pushed UP from the auction into war/operation
target selection so an undefended lvl0 invader core (the easiest, most valuable free win — it unlocks a
reservable mining remote) is no longer ignored. Transient `EconomicIntel` on the ephemeral runtime entry →
**no WFV bump**. The pure kernel is reusable by a future expansion/claim-selection scorer. See §"v3 —
economic-value-unlocked target EV (reach-bug #3)" below.
Task #28 (P-AUCTION). Extends ADR 0027 (the reassignment / merge lifecycle + the objective queue),
reuses ADR 0031 (the composition EV machinery), and makes concrete the cross-goal EV currency ADR
0020 §11 deferred. The decision/identity changes live here; the doctrine/body specifics
cross-reference ADR 0026/0031.

### Phase status
- **v1.1 (DONE)** — per-squad EV scoring + the EV-positive gate (StayPut/Recycle alternatives) +
  `value_e` + enemy-force pricing. Offline-proven (the kernel pairing/gate/quantize tests +
  `objective_ev_prices_enemy_creeps_no_free_win`).
- **v1.2 (DONE)** — the global Hungarian solve (`assignment.rs`: `build_ev_matrix` + `solve_assignment`)
  replaces BOTH greedy loops (`best_reassignment_near` + the per-squad `best_by_ev` claim/reassign) over
  the `N×K` matrix; Phase C folded into EV-ranked claimable rows. Deterministic (integer-quantized EV, no
  HashMap, stable tie-break). The solver requires a per-row zero-cost escape column (the standard optional-
  assignment formulation; `build_ev_matrix` supplies it via per-row Recycle@0) — documented contract +
  per-row `debug_assert` + a 6400-matrix brute-force cross-check proptest vs the exhaustive may-skip
  optimum (0 mismatches). Offline-proven: `hungarian_strictly_beats_greedy_on_total_ev` +
  `run_auction_flow` (`auction_global_strictly_beats_greedy_in_the_flow`).
- **v2 (DONE)** — the `Merge→Bk` column class (`ColumnKind::Merge`, appended so v1.2 indices are
  byte-unchanged): cell EV = the receiver's **marginal P(win) lift** `[P(win|B.comp+donor.sheddable) −
  P(win|B.comp)]·value_e(B.obj) − transfer_cost`; column-feasibility = the **Lanchester pending-slot
  guard** (merge-eligible donor + role-matched open slot + no self-merge — the dilutive split is *never*
  representable as a column). The mechanism (`apply_merges`): transfer the donor's role-matched member →
  B's open slot (rebind squad-ref+room, spawn-slot dropped, B is the coordination unit), emptied donor
  retires; the same-tick deferred-`exec_mut` vs Phase-B double-fill is guarded by the
  `create_spawn_callback` `is_slot_filled` recheck (surplus recalled-to-recycle, never orphaned).
  Offline-proven: `merge_is_picked_over_a_marginal_solo_reassign_and_the_dilutive_split_is_absent` +
  `run_merge_flow`. No WFV bump.
- **v3 (DONE)** — economic-value-unlocked target EV (reach-bug #3). A PURE, REUSABLE room-economics net-ROI
  kernel values CONTROLLING a room; war/operation target selection ranks by `P(win)·net_roi − cost`; the
  emitted objective carries the net-ROI (transient `EconomicIntel`) so `value_e`'s economic arm prices it.
  Defense stays dominant. Offline-proven: the standalone pure-kernel tests
  (`close_reservable_remote_beats_far_beats_no_economy`, `winnable_reservable_core_has_healthy_positive_value`,
  determinism), the war-EV ranking tests (`economic_rank_ranks_close_winnable_above_far_above_deathtrap`,
  `economic_rank_lifts_winnable_core_above_bare_score`), and the defense-dominant test
  (`high_value_defend_out_ranks_a_remote_economic_target`). No WFV bump. See the §below.

## v3 — economic-value-unlocked target EV (reach-bug #3)

### The bug
An undefended **level-0 invader core** — the easiest, most valuable free win, since clearing it unlocks a
**reservable mining remote** — was effectively ignored. `war.rs`'s offense scan scored each core by
**threat/proximity only** (`invader_core_attack_score`: a base 30/60 minus level + distance penalties),
then mapped it to `Dismantle → Denial`. `value_e(Denial)` = `denial_value · DENIAL_DISCOUNT`, and a
dismantle target with dps 0 carried a denial value ≈ 0 — so the auction priced a free, economy-unlocking
win at ~nothing, below trivial denial/farm work. The strategic value (the remote's income) never entered
the currency at all.

### The fix — a PURE, REUSABLE room-economics kernel, value pushed UP into target selection
A combat target's value is the **economic value unlocked by controlling the room**, as a **FULL net-ROI**:
`net = gross_income − hold_cost − mining − haul − cpu/distance_penalty`, amortized to energy/tick and
projected over a horizon. This GENERALIZES the SK-farm net-ROI (ADR 0018 §3.2) and the remote
`MiningOutpost` economics into a single controlled-room model; it does **not** invent a parallel currency
— it produces `value_e`'s economic-arm inputs (`income_per_tick`/`horizon`).

**Architecture (the reuse contract).** The net-ROI computation is a **functionally PURE kernel**
([`crate::room_economics::room_net_roi`], `screeps-ibex/src/room_economics.rs`): NO `game::*`/world reads.
It takes plain room FACTS ([`RoomEconomyFacts`]: source count + per-source yield, distance, hold model,
horizon) and returns an energy-equivalent net-ROI ([`RoomEconomyValue`]). It is positioned as a
**standalone module in the bot crate**, deliberately NOT in `war.rs` and NOT in the combat-decision crate,
chosen by the dependency graph: it must be importable by BOTH combat target selection (`operations::war`
now) AND a FUTURE expansion/claim-selection scorer (`expansion`/`claim.rs`, which lives in the bot crate).
Putting it in the combat-decision crate would force expansion to depend on combat; putting it in `war.rs`
would couple expansion to the war operation. A pure module in the bot crate avoids both. The bot ADAPTER
(`war.rs`) GATHERS the facts from `RoomData`/visibility and passes them in; the kernel stays pure +
unit-testable + bit-deterministic.

**The flow.**
1. `war.rs` InvaderCore arm: for a winnable lvl0 core in a sourced room, build `RoomEconomyFacts`
   (`reservable_remote(source_count, min_distance · TILES_PER_ROOM)`) and call `room_net_roi` → the
   economic ROI; store it on `AttackCandidate.economic_roi`. NOTE the units: `min_distance` is ROOM HOPS
   (route steps from `min_distance_to_homes`), but `RoomEconomyFacts::haul_tiles` is ACTUAL TILES, so the
   adapter converts hops → tiles (× `TILES_PER_ROOM` = 50) at the call site — the same conversion the SK
   scorer applies (`candidate.distance() · TILES_PER_ROOM`). Skipping it under-counts haul + cpu ~50× and
   over-values far cores.
2. **Target selection** ranks by an EV-augmented score (`economic_rank_score`):
   `score = base + P(win_proxy)·ROI_SCALE·net_roi`, where the P(win) PROXY is a cheap pure read of the
   room's defense (energized-tower DPS — a death-trap reads a low proxy). The precise winnability +
   affordability VETO stays the launch loop's (`plan_engagement().winnable()` + `can_afford_military`);
   this only RANKS (a continuous sort key, never a discrete branch — determinism preserved).
3. The emitted objective carries the net-ROI via a transient `EconomicIntel { net_income_per_tick, horizon }`
   on the **ephemeral runtime entry** (`ObjectiveRuntimeEntry.economic_intel`, like `claimed_by` — NEVER
   serialized). The auction's `project_value_kind`/`project_intel` read it: an objective with economic
   intel is priced as `FarmCore` (`income·horizon`) regardless of its `ObjectiveKind`, so a winnable core
   (a `Dismantle`/`Denial` kind) is re-valued from ≈0 to its real remote income.

**Defense stays dominant.** A high-value `Defend` (RCL8-magnitude asset under a genuine assault) still
out-values a remote economic target — the economic fix lifts a winnable core from ~0 to its net-ROI, but a
remote never out-bids defending the base (`high_value_defend_out_ranks_a_remote_economic_target`).

**No `WORLD_FORMAT_VERSION` bump.** `EconomicIntel` is transient (ephemeral runtime, re-attached every
offense scan), exactly the `RequiredForce`/matrix discipline. No serialized shape changed.

### Critical files (v3)
- `screeps-ibex/src/room_economics.rs` (NEW — the pure, reusable net-ROI kernel + its standalone tests).
- `screeps-ibex/src/operations/war.rs` (`AttackCandidate.economic_roi`, the InvaderCore arm's
  `room_net_roi` call, `economic_rank_score`, the `set_economic_intel` attach + tests).
- `screeps-ibex/src/military/objective_queue.rs` (`EconomicIntel` + `ObjectiveRuntimeEntry.economic_intel`
  + `set_economic_intel`/`economic_intel`).
- `screeps-ibex/src/military/squad_manager.rs` (`project_value_kind`/`project_intel`/`objective_ev_q` read
  the economic intel; the cell builder + `ev_of_claim` thread it).
- `screeps-combat-decision/src/objective_value.rs` (the defense-dominant test; the economic arm is reused
  unchanged).

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
