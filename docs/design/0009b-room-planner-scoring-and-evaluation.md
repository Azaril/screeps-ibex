# 0009b — Room-Planner Scoring, Evaluation & RCL Assignment Revamp

**Status:** Proposed
**Relates to:** [0009](0009-room-planning-and-multiroom-layout.md) (room planning), [0009a](0009a-room-planner-performance.md) (planner performance), [0007](0007-hauling-logistics.md) (hauling)
**Driver:** Operator — "tower coverage is poor; build extensions nearest the nearest source first; minimize storage→refill hauling; evaluate other bots and revamp scoring/evaluation."

> All file references are `screeps-foreman/src/...` unless noted, verified against source at time of writing.

---

## TL;DR

A full-shard render of shard3 (5,123 planned rooms) measured **mean tower coverage 0.302, with 73% of rooms below 0.30**. Investigation shows this is **not** a tuning problem — it is structural:

- **Towers are optimized against the wrong perimeter.** `TowerLayer` places towers *before* `DefenseLayer` produces the real min-cut ramparts, fitting them to a synthetic proxy ring; the score then measures them against ramparts they never saw.
- **The scoring "total" barely discriminates layouts.** Of ~13.3 total weight, ~8.3 is anchor-only constants (identical across same-anchor layouts) and `hub_quality` (1.5, the single highest weight) saturates at 1.0 in every room. Only ~3.5 effective weight differentiates the layouts the search actually chooses among — so tower coverage and hauling are mathematically drowned out.
- **RCL assignment is hub-distance-only.** Early extensions are the hub-closest, never source-closest — despite per-source distance fields already being computed and sitting unused.
- **There is no hauling/refill metric at all,** and the bench only echoes the planner's own score, so it can't prove a change is objectively better.

This ADR proposes a revamped scoring + RCL + evaluation system that fixes all three operator goals, grounded in what Overmind/Harabi and the wider community do, with a **sequenced, individually-benchable** rollout that keeps every hard invariant (sub-scores ∈ [0,1] for search-prune soundness; min-cut stays last; one `WORLD_FORMAT_VERSION` bump batched at the end).

---

## 1. Problem & evidence

### 1.1 Measured baseline (shard3, 5,123 rooms)

| Metric | Mean | Distribution |
|---|---|---|
| Tower coverage | **0.302** | 73% < 0.30, 19% in 0.30–0.50, only 0.8% ≥ 0.70 |
| `hub_quality` | **1.000** | saturated in every room |
| Total | 0.769 | — |

### 1.2 Root causes (verified)

- **RC1 — Tower target mismatch.** `TowerLayer` (`layers/tower.rs`) runs before `DefenseLayer`. It greedy+swap-optimizes min damage over `analysis.exits.all` plus a synthetic Chebyshev dist-8..12 ring around the hub (`tower.rs:68-85`), with `HUB_PROXIMITY_WEIGHT=15` pulling towers inward and a leash of `MAX_TOWER_RADIUS=10`. `TowerCoverageScoreLayer` (`tower_coverage_score.rs:41-56`) then measures min damage over the **real** min-cut ramparts (15–22 tiles out). The towers were fit to a different perimeter and can no longer move. *Primary cause of 0.302.*
- **RC2 — Brutal, unreachable metric.** `tower_coverage = min over ALL rampart tiles of (Σ tower_damage_at_range)/3600`. Pure min ⇒ one far rampart zeros the score (no gradient). 3600 (6×600) requires every rampart within range 5 of all 6 towers — impossible; the real ceiling is ~0.42. It also ignores progressive RCL tower onlining (1/2/3/6 towers at RCL 3/5/7/8).
- **RC3 — RCL assignment is hub-only.** `rcl_assignment.rs:82-99` sorts each structure type by hub flood-fill distance and assigns `min_rcl_for_nth`. The 5 earliest extensions are the hub-closest, never source-closest. `analysis.source_distances` (full per-source flood maps) is in scope but bound as `_analysis` and unused.
- **RC4 — No hauling metric.** The only proxy is `extension_efficiency = 1 − avg(hub flood-fill dist)/25` (`extension_score.rs:67-89`): hub-not-storage, extensions-only, average-not-worst, **terrain-only** (ignores placed structures, so it under-counts filler detours). `traffic_congestion` is declared (`plan.rs:131`) and mapped (`layer.rs:239`) but **never computed** (phantom).
- **RC5 — Discrimination collapse.** Total = weighted *arithmetic mean* of [0,1] sub-scores (`layer.rs:76-83`, `:244-248`). ~8.3 of 13.3 weight is anchor-only constants (`source_distance` 3.0, `source_balance` 1.5, `controller_distance` 1.0, `terrain_openness` 1.0, `defensibility` 1.0, `exit_proximity` 0.5, `mineral_distance` 0.3 — identical for same-anchor layouts) and `hub_quality` 1.5 saturates at 1.0. Only ~3.5 weight discriminates. A 0.10-coverage plan still wins on the other terms.
- **RC6 — Bench echoes the self-score.** `screeps-foreman-bench` `cmd_compare` renders PNG + plan JSON + the planner's own score table; it computes no independent ground truth and no aggregate stats. The "mean 0.302" had to be computed externally.

### 1.3 The good news: the signals already exist

`AnalysisOutput` (`pipeline/analysis.rs:13-32`) already carries per-source / per-controller / per-mineral 50×50 flood-fill fields, the distance transform, and exit-distance fields. `PlacementState` exposes `hub_pathing_distances` — a **structure-aware** flood (roads/containers/ramparts passable) used by `ExtensionLayer` but by **no scorer**. The real rampart set is `landmark_set("ramparts")` (produced by `DefenseLayer`). Every fix below uses existing signals — no new analysis cost.

---

## 2. What "good" looks like (external landscape)

- **Overmind** (bencbartlett) gets high tower damage from a **compact fixed bunker** — all 6 towers cluster near the center of a small, near-uniform rampart ring, so each hits every perimeter tile at near-max range. It selects the base anchor by **minimizing total path length to sources + controller**. *Lesson: compactness + towers-central-to-the-real-ring is the lever; foreman's min-cut perimeter can be large/irregular and its towers are leashed to the wrong ring.*
- **Harabi** ("Automating Base Planning in Screeps") states the tower objective verbatim: *"maximize the minimum damage of towers over base boundaries,"* and names **storage→extension** and **terminal→lab** distance as the quantities to minimize for logistics. foreman already implements the geometry (distance transform, Dinic min-cut); the gap is purely scoring/ordering.
- **screepspl.us / community**: towers can be drained from afar, so coverage must be measured at the rampart line and normalized against a realistic *heal-throughput threshold*, not an absolute max. Worst-component aggregation (soft-min / geometric mean / penalty floor) beats arithmetic mean for multi-objective layout selection.

Consensus: a good layout is **compact and near-circular**, near sources+controller, with min-cut ramparts, **towers central to the real ring**, **hauling measured as structure-aware path distance from storage**, and an **early game that puts the first structures on the cheap source→economy path**. Notably, operator goal #2 (source-proximate *early extensions*) goes **beyond** Overmind (which builds outward from bunker center) — this is a place to lead, not copy.

---

## 3. Revamped scoring model

### 3.1 Sub-score table (the discriminating set)

Retired terms are kept as `ScoreEntry`s at **weight 0.0** (not deleted) so the search's deterministic push order and learned `total_score_weight` stay stable — zeroing is provably prune-safe; removing pushes is churn with no benefit.

| Sub-score | Layer | New wt | Old wt | Status |
|---|---|---:|---:|---|
| `tower_coverage` | TowerCoverageScore | **3.0** | 1.0 | reshaped + up-weighted (goal #1) |
| `refill_distance` | **new** RefillScore | **2.0** | — | **new** (goal #3) |
| `early_extension_source` | RclAssignment | **1.0** | — | **new** (goal #2) |
| `fill_efficiency` | ExtensionScore | 0.5 | 0.5 | kept (cheap throughput signal) |
| `upgrade_area_quality` | UpgradeAreaScore | 0.5 | 0.5 | kept |
| `upkeep_cost` | UpkeepScore | 0.5 | 0.5 | kept, floored at 0.05 |
| `extension_efficiency` | ExtensionScore | **0.0** | 1.0 | retired (superseded by refill) |
| `hub_quality` | HubQualityScore | **0.0** | 1.5 | retired (saturates 1.0; cannot gate¹) |
| `source_distance`, `source_balance`, `controller_distance`, `terrain_openness`, `defensibility`, `exit_proximity`, `mineral_distance` | AnchorScore | **0.0** | 8.3 | retired from total² |
| `traffic_congestion` | — | — | — | deleted (phantom) |

¹ `hub_quality` cannot be repurposed as a rejection gate: Storage/Spawns are *mandatory* hub-stamp placements (`stamps/hub.rs:28-31`), so a stamped hub always scores 1.0.
² Anchor terms still drive `AnchorLayer`'s beam pre-rank via the separate `composite_anchor_score` (`anchor_score.rs:36`), so anchor quality still gates which hubs enter the search — they're just no longer double-counted (as constants) in the per-layout total.

**Discriminating total weight: 7.5** (was effectively ~3.5 amid 13.3). Tower coverage now carries 40% of the objective, matching operator priority #1.

### 3.2 Aggregation — two tiers

The four design proposals split on the aggregator; every judge flagged that geometric-mean / unfloored soft-min designs suffer **zero-collapse** (a legitimately-zero term — e.g. empty towers — collapses the whole product and zeroes the prune's `best_complete_score`, re-creating RC5). Resolution: a two-tier model that ships the safe option first.

- **Tier 1 (ships first, zero prune risk):** keep the arithmetic weighted mean `Σ(sᵢwᵢ)/Σwᵢ` *unchanged in structure* — just zero the dead weights and embed worst-component behavior **inside** the tower (soft-min) and refill (p90) sub-scores. Prune admissibility is **identical to today** (`search.rs:286` untouched).
- **Tier 2 (optional, flag-gated, ships last):** replace the top-level mean with a **bounded power-mean** (order p=−2) over **floored** inputs (`floor(s)=0.05+0.95s`), so a true-zero term contributes a large-but-**bounded** `0.05⁻²=400` rather than ∞/NaN — discrimination without collapse. Empty-towers/empty-ramparts rooms push the term at weight 0.0 instead of score 0.0. The prune bound is re-derived (unseen terms → `floor(1)⁻²=1`, the minimum contribution, which maximizes the aggregate ⇒ a true upper bound) and stays admissible. Ship only if the bench shows a corpus gain over Tier-1.

**Invariant preserved:** every sub-score ∈ [0,1]; both aggregators output [0,1]; prune never discards the optimum.

---

## 4. Tower-coverage fix (goal #1)

> **IMPLEMENTED 2026-06-14 (placement; coverage term).** The placement architecture below is built and benched. Final shape: `TowerReservationLayer` (pre-extension, reserves a compact central zone via the `excluded` set — **space only, no tower structures in the cut**) → **coverability-weighted min-cut** (`defense.rs`, coverage term `cost = BASE + ALPHA·(1−P(t)/600)`) → `TowerPlacementLayer` (the **single** post-defense placement against the real passable-rampart perimeter; the old proxy `TowerLayer` and the place-then-swap `TowerRepositionLayer` are gone). Results on shard3 `compare --limit 500`: **mean tower coverage 0.294 → 0.409 (+39%)**, rooms below 0.30 78% → 28%, **6/6 towers in every room**, 496/500 planned (4 infeasible — ≈ the pre-existing ~1.3% rate, no regression). Isolated contributions (limit 120): single placement vs real perimeter 0.294→0.357; weighted cut adds 0.357→0.406. NOT yet implemented: the threat-direction & wall-adjacency cost terms (§4.3, the next two increments), the staged soft-min metric (§4.6 — coverage is still measured on the old min/3600 metric, so these numbers are directly comparable to the 0.302 baseline), and the §3 scoring re-weight. Files: `defense.rs`, `tower.rs` (now `TowerReservationLayer` + shared optimizer), new `tower_placement.rs`, `layers/mod.rs`; `tower_reposition.rs` deleted.


> **Revision (2026-06-14):** §4.1 (post-defense tower repositioning) has been **prototyped** (`layers/tower_reposition.rs`, uncommitted) and lifts mean coverage **0.30 → 0.355** on shard3 `--limit 40`. But its own measurements expose a hard ceiling: a residual set of "floor rooms" stays pinned at the minimum (all 6 towers ≥ range 20 from the worst rampart → 6×150 = 900 damage), and widening the tower radius 10→14 only nudges 0.355→0.362 while costing refuel distance. **Repositioning cannot fix an uncoverable perimeter.** §4.2–§4.4 below are the new core of the fix: make the *min-cut itself* produce a coverable perimeter. §4.5 keeps repositioning as the final polish; §4.6 is the metric.

### 4.1 The bidirectional dependency (the real problem)

Towers and ramparts depend on each other:
- **Ramparts must protect towers** — towers have to sit *inside* the cut.
- **Towers must cover ramparts** — every rampart tile wants a tower within range (damage is 600 at range ≤5, decaying to 150 at ≥20; **range-only, ignores line-of-sight**, so it is purely geometric).

Today the min-cut (`DefenseLayer`) minimizes pure rampart **count** (`defense.rs:381`: `cap = if is_border { INF } else { 1 }`), with **no awareness of where towers can stand**. So it can produce a perimeter whose worst points are ≥20 tiles from any tower-placeable interior tile — a "floor room" that no amount of repositioning rescues. The deeper cause is that a *spread-out protected footprint* (sprawling extensions, far controller-infra/mining) forces the tightest possible seal to be large and far from the hub-centered tower cluster.

**Resolution — break the cycle with a shared, static field, not iteration or coupling.** Both layers reference one geometric signal — the **tower-influence field** `P(t)` — computed once from where towers *can* stand. The min-cut consumes it as a *cost* (prefer ramparts towers can reach); the tower layer consumes the *real perimeter* the cut produced. Neither layer reads the other's output, so they stay conceptually separate, yet the perimeter is now shaped to be coverable and the towers are placed against the real ramparts. This is exactly the "distance transform that weights the min-cut toward tower-placeable places" idea, made precise.

### 4.2 The shared tower-influence field `P(t)`

Computed per candidate hub (cheap; one Chebyshev distance-transform, negligible beside the per-candidate Dinic):
```
C       = tower-placeable region = buildable interior tiles within R_place (≈10) Chebyshev of hub
          (non-wall, not occupied by a blocking non-tower structure) — where towers will sit
D_C(t)  = Chebyshev distance from tile t to the nearest tile of C   (multi-source DT seeded from C)
P(t)    = tower_damage_at_range(D_C(t))  ∈ [150, 600]               // best single-tower damage at t
```
Because tower damage is range-only, `P(t)` is an exact upper bound on what one tower (placed optimally in `C`) could deliver to `t`. A single-tower proxy is sufficient for *biasing* the cut (full 6-tower coverage is optimized later by §4.5). `C` is computed pre-cut from the known hub + building footprint, so there is no circular dependency.

### 4.3 Coverability-weighted min-cut (the core mechanism)

Replace the uniform interior cost in `compute_min_cut` (`defense.rs:381`) with a composite per-tile **rampart cost**. The protected region (`build_protected_region`) still decides *what is inside* (incl. the existing 3-tile ranged-standoff buffer for valuables, 1-tile for the controller); the cost decides *which boundary tiles to pick among valid seals*. Each term is independently weighted and calibratable:

```
cost(t) = BASE                                   // (a) upkeep / count
        + ALPHA · (1 − P(t)/600) · threat(t)     // (b) coverage × threat direction
        − OMEGA · wall_adjacency(t)              // (c) natural-wall / chokepoint leverage
cost(t) = clamp(cost(t), COST_MIN, BASE + ALPHA) // keep positive + bounded (INF scaling, below)
```
Integer-valued (e.g. `BASE=10, ALPHA=20, OMEGA=3, COST_MIN=1`). `λ = ALPHA/BASE` remains the master coverage-vs-upkeep knob; `λ=0, OMEGA=0` reproduces today's uniform behavior exactly.

**(a) `BASE` — upkeep / count.** The uniform floor; minimizing the count of cut tiles = minimizing maintained ramparts (today's sole objective). Everything else perturbs around it.

**(b) `ALPHA · (1 − P(t)/600) · threat(t)` — coverage, weighted by threat direction.**
- `P(t)` is the tower-influence field (§4.2): fully coverable tiles (`P=600`) add nothing, least-coverable (`P=150`) add up to `0.75·ALPHA·threat`. The cut routes ramparts where towers reach.
- `threat(t) ∈ [THREAT_MIN, 1]` rises as `analysis.exit_distances(t)` falls (closer to an exit = more attackable). Towers are a *finite* DPS budget, so this spends the cut's freedom making the **attack-facing** perimeter coverable while tolerating weaker coverage on deep/corner sections an attacker must walk around to reach. `exit_distances` is already computed — zero new analysis cost. (Offline, all exits are weighted equally; the live bot could re-weight toward hostile neighbours later.)
- **Repair synergy (free):** towers *repair* the ramparts they can reach, so this term improves siege *survivability*, not just burst damage — a covered rampart is both harder to break and faster to re-seal.

**(c) `− OMEGA · wall_adjacency(t)` — natural-wall / chokepoint leverage.** `wall_adjacency(t) ∈ [0,1]` from the count of natural-wall neighbours of `t`. A rampart backed by natural walls has a shorter exposed frontage and sits in a narrower funnel, so discount it. Count-minimization already favours chokepoints, but term (b) can *pull the perimeter away* from a cheap-but-distant natural chokepoint toward a closer, wider, more-coverable line — quietly trading away the terrain's free defense. This term guards that and keeps `λ` honest (without it, aggressive coverage weighting can balloon rampart count).

**Compactness falls out for free:** a tighter perimeter has both fewer tiles *and* better coverage, so for most rooms this *reduces* rampart count while improving coverage — the two goals align (the Overmind "compact bunker → high tower damage" lesson, on a flexible min-cut).

**Correctness preserved:** only the in→out tile-edge capacities change; the node-split graph, the INF movement/source/sink edges, and the single-tile-thick seal are unchanged. The cut still separates interior from exits.

**INF scaling (must-do):** `INF_CAP` must dominate the largest possible weighted perimeter across *all* terms, so set `INF_CAP = ROOM_AREA · (BASE + ALPHA) + 1` (was `ROOM_AREA + 1`); the `COST_MIN` clamp keeps every tile cost ≥ 1 so no edge becomes free. Otherwise a heavy weighted perimeter could exceed INF and the cut would (incorrectly) sever a movement/border edge.

**Determinism:** every term is a pure function of hub + terrain + placed structures + `analysis` (which is fixed pre-search), so the seg-60 resume fingerprint and reproducibility are unaffected (layer names unchanged).

**What deliberately stays OUT of the cost** (so it remains one clean, bounded field and the layers stay separate):
- *upkeep/count* → already the `BASE` term; don't double-encode.
- *wall-vs-rampart split* → a downstream classification (depends on which cut tiles creeps must traverse, unknown until the cut exists).
- *what is protected + the standoff buffer* → stays in `build_protected_region`; orthogonal to boundary cost.
- *hauling / RCL / refill / economy* → other layers and scores; folding economy into the defensive cut would muddy both concepts.

**Keeping the layer concept clean:** `DefenseLayer` takes an injected `rampart_cost: &dyn Fn(Location) -> u32` (or a precomputed cost field) rather than hard-coding tower/threat logic — its concept stays "minimum-cost seal." A small shared helper (alongside `tower_damage_at_range` in `stamps/tower.rs`) builds the composite field from `P(t)`, `exit_distances`, and terrain. The min-cut never imports the tower *layer*; it consumes a neutral cost field.

**Calibration (incremental, on the §7 ground-truth bench):** introduce the terms in order — coverage-only (`ALPHA`) first, then threat (`threat(t)`), then the wall discount (`OMEGA`) — each gated on a corpus gain, and **track rampart count alongside coverage** so no term quietly trades upkeep for a marginal coverage bump. An optional later term, *value-behind-the-wall* (a coverage premium for perimeter sections shielding the controller/storage/spawns), largely overlaps compactness and is deferred.

### 4.4 Complementary lever — footprint compactness (optional, upstream)

The weighted cut shapes the perimeter *within* the footprint it must enclose. When the footprint itself is spread, even the best seal is far from the tower cluster. Two upstream pressures help and reinforce goals #1 and #3 together:
- **Let the cut drop far, low-value structures from the protected core.** `build_protected_region` already separates required-core from best-effort mining (ADR 0009a). Extend that policy so a handful of far extensions (or remote mining infra) may sit *outside* the wall when protecting them would force an uncoverable perimeter — they are cheap and rebuildable, and this keeps the defended ring compact and coverable.
- **A `footprint_compactness` / perimeter-coverability score term** (∈ [0,1]) so the search prefers anchors/layouts whose protected mass is tight around the hub. This dovetails with the `refill_distance` term (§6) — compactness lowers hauling *and* lifts tower coverage simultaneously.

### 4.5 Tower repositioning — final polish (implemented)

`TowerRepositionLayer` (now at index 17, after `DefenseLayer`, before `RoadNetworkLayer::all_buildings`) stays, but its role shrinks from *rescuing* a bad perimeter to *optimally placing into a coverable one*. With §4.3 in place the floor-room set should largely vanish, so the greedy+swap is fine-placement, not damage control. It already: filters the `ramparts` landmark to the real perimeter, constrains candidates to the strictly-interior flood (`compute_interior`, so a tower never lands outside the wall), requires a walkable neighbour (so `ReachabilityLayer` won't reject), and reuses the shared `optimize_towers` engine. Refinements to fold in: filter the perimeter to **passable ramparts** (the landmark holds walls too — `defense.rs:104,114`), and consider widening `REPOSITION_RADIUS` once §4.3 reduces the refuel-distance pressure that currently caps it at 10.

This is the residual "place-then-swap." It is acceptable *because* it now depends on a perimeter that was itself made coverable — the bidirectional dependency is resolved by the shared field (§4.2), not by the swap.

### 4.6 The coverage metric — staged threshold soft-min

`TowerCoverageScoreLayer`, over passable ramparts `R`, for the tower subset `T_k` online at RCL tier k:
```
dmg_k(r)  = Σ_{t∈T_k} tower_damage_at_range(range(t,r))      // existing fn
cov_k(r)  = clamp01( dmg_k(r) / DESIRED_DPS_k )              // threshold, not cap
softmin_k = −1/β · ln( mean_r exp(−β·cov_k(r)) )             // β=6, smooth worst-case
tower_coverage = clamp01( Σ_k ω_k · softmin_k )              // Σω_k = 1
```
- `DESIRED_DPS_k = {600, 1100, 1500, 2400}` for k = {1,2,3,6} towers — **reachable** thresholds (cures the unreachable-3600 ceiling), fixed per-tier constants (a per-room/per-candidate ceiling would let a layout score higher by pushing ramparts *farther* — unsound).
- `ω = {0.15, 0.20, 0.25, 0.40}` — weighted to the RCL8 steady state but crediting early defensibility.
- soft-min (β=6) gives a dense gradient (fixes the pure-min cliff) while remaining dominated by the worst ramparts. [0,1] and prune-admissible by construction.

### 4.7 Alternatives considered

| Option | Idea | Verdict |
|---|---|---|
| **Weighted cut + shared field (§4.2–4.3)** | bias the min-cut toward tower-coverable perimeters via `P(t)` | **Chosen** — fixes perimeter shape (the floor-room root cause), cheap, keeps layers decoupled |
| Tune the reposition layer only | wider radius / better swap | Rejected — diminishing returns (0.355→0.362), can't change perimeter shape |
| Two-phase cut | cut → place provisional towers → re-cut weighted by *actual* tower positions | Rejected — 2× the expensive Dinic per candidate; marginal over the stable-`C` proxy |
| Joint search | branch over (cut variant × tower placement), score jointly | Rejected — combinatorial blow-up, violates the planner-perf budget |
| Coverability as a score term only | penalize uncoverable perimeters in scoring, leave the cut uniform | Weaker (the search must *stumble* onto good perimeters); kept only as the §4.4 complement |
| Footprint compactness (§4.4) | shrink the protected mass upstream | **Complementary** — addresses the deeper cause; pairs with the chosen option |

### 4.8 Calibration & open decisions

- **λ = ALPHA/BASE** (coverage vs upkeep): sweep on the corpus via the §7 ground-truth bench (track mean coverage *and* mean rampart count — confirm coverage rises without upkeep ballooning). Introduce the cost terms in order — coverage, then threat, then wall-discount — each gated on a corpus gain.
- **`THREAT_MIN`** (floor of the threat weight for deep/corner ramparts): how much coverage to still demand far from exits. Start ~0.3 (corner ramparts cost ~30% of the coverage penalty).
- **`OMEGA`** (natural-wall / chokepoint discount): start small (~3 on a `BASE=10` scale) so it nudges toward terrain leverage without overriding the seal; raise if the weighted cut abandons cheap chokepoints (watch rampart count).
- **`R_place`** (tower-cluster radius defining `C`): start at 10 (matches `MAX_TOWER_RADIUS`).
- **Drop far structures from the protected core for compactness?** A policy decision (a few structures left undefended). Recommend gating behind a threshold (only when protecting them would force a coverage floor) and only for low-value types.
- **Single- vs multi-tower `P(t)`:** single-tower is a sufficient cut-biasing proxy; revisit only if the bench shows it under-shapes.
- **Value-behind-the-wall premium?** Optional later term (more coverage for sections shielding controller/storage/spawns); overlaps compactness — deferred.

---

## 5. Source-aware RCL assignment & build order (goal #2)

Both changes mutate only existing fields (`required_rcl` values, `BuildStep` order) ⇒ **no version bump**. `analysis.source_distances` is already in scope in `RclAssignmentLayer`.

- **Exclude towers** from the RCL loop (they're RCL-tagged by `TowerRefineLayer` now).
- **Extensions — phase-blended key** (smooth, not a cliff):
  ```
  N_early = max_structures_at_rcl(Extension, 4) = 20         // derive from the RCL4 cap
  src_dist(loc) = min over sources of analysis.source_distances[s][loc]
  phase(i)      = clamp01((N_early − i)/N_early)             // 1.0 early → 0 by N_early
  key(i)        = phase·src_rank(i) + (1−phase)·hub_rank(i)
  ```
  Sort ascending by `key`, assign `rcl = min_rcl_for_nth(Extension, i+1)`. Early extensions are dominated by **source proximity** (built when the economy is source-bound); later ones by hub/storage proximity (steady-state filling). The `min_rcl_for_nth` caps are respected exactly — only *which* extension gets which slot changes.
- **`early_extension_source` sub-score** (weight 1.0): `mean over the N_early lowest-RCL extensions of (1 − clamp01(src_dist/25))`. Without this, the re-ordering is invisible to the search.
- **Build-order tie-break (deferred):** `compute_build_order` already sorts by `required_rcl` before the hub tie-break, so §5 carries the priority; `FinalizePhase` has no analysis access. A finer same-RCL source tie-break can be added later via a transient (`#[serde(skip)]`) `src_dist` stash — optional follow-up, not core.

---

## 6. Storage→refill hauling metric (goal #3)

- **`storage` landmark:** add `(Storage, "storage")` to the hub-stamp `landmark_mappings` (`stamp_layer.rs:154-157`) so the *actual placed* storage tile is recorded — robust under `all_rotations()` (a hardcoded offset would be wrong). Fall back to `hub` if absent.
- **`RefillScoreLayer`** (after `ReachabilityLayer`, before `RclAssignment`, so the final pruned structure layout exists): a **structure-aware** flood (`flood_fill_distance_with_obstacles`, roads/containers/ramparts passable) **seeded from storage**. Distance to each target = distance to the nearest passable tile **adjacent** to it (the hauler stops beside the structure).
  ```
  targets/freq:  Extension 1.0, Spawn 1.0, Tower 0.6, Lab 0.3   (Terminal excluded — it sources hauling)
  q_t = 1 − clamp01(d_t / 40)
  refill_distance = 0.6·(freq-weighted mean of q_t) + 0.4·(p90 of q_t)
  ```
  p90 (not pure min) gives a worst-inclusive signal that survives one pathological target — worst-component behavior *inside* the sub-score, so it works under both aggregator tiers.
- Retire `extension_efficiency` (weight 0): `refill_distance` is its correct replacement (storage-rooted, all targets, worst-inclusive, structure-aware).

---

## 7. Independent bench evaluator (ships first)

The bench currently can't prove a change helped (RC6). Add an **independent ground-truth evaluator** computed from raw `plan.structures` + a re-run analysis — deliberately separate code so it can't share the scorer's bugs:
- `gt_tower_min / p10 / mean` over passable ramparts (the operator's true objective; `gt_tower_mean/3600` maps to "0.302"), plus per-RCL staged variants.
- `gt_storage_ext_dist`, `gt_refill_worst/mean` (structure-aware storage floods).
- `gt_early_ext_src` (mean source distance of the 20 lowest-RCL extensions).

Extend `cmd_compare` with **corpus aggregates** (mean / p10 / p50 / p90 / histogram), a **`--baseline <csv>` diff mode** (per-metric mean shift + % rooms improved), and the planner self-score table **side-by-side** with ground truth (catches scorer/reality divergence). A `#[test]` asserts the bench's copy of `tower_damage_at_range` and the passability rule match foreman's constants (drift guard). Bench-only — no serialization impact. (The real shard3 JSONs are operator-held, so the calibration sweep is operator-run.)

---

## 8. Sequenced implementation plan

Each step is independently benchable via §7's ground-truth diff.

| # | Step | Effort | Risk | Serialization |
|---|---|---|---|---|
| 1 | Bench ground-truth evaluator + corpus stats + `--baseline` CSV; establish baseline | M | Low | none |
| 2a | *(done, uncommitted)* `TowerRepositionLayer` (post-defense reposition vs real ramparts) — coverage 0.30→0.355 | — | — | none |
| 2b | **Coverability-weighted min-cut**: shared tower-influence field `P(t)` (§4.2) + composite `cost(t)` in `compute_min_cut` (coverage×threat − wall-adjacency, §4.3) + scaled `INF_CAP` + injectable cost field. The headline floor-room fix. Land coverage-only first, then add threat + wall terms. | L | Med | none (layer names unchanged) |
| 2c | Passable-rampart filter in reposition + staged soft-min metric (§4.6) + `tower_coverage` weight→3.0 | M | Low | none |
| 2d | *(optional)* footprint compactness: drop far low-value structures from the protected core + `footprint_compactness` score (§4.4) | M | Med | none |
| 3 | Zero dead weights (anchor terms, `hub_quality`, `extension_efficiency`) | S | Low | none |
| 4 | Source-aware phase-gated extension RCL + exclude towers + `early_extension_source` push | M | Low | none |
| 5 | `storage` landmark + `RefillScoreLayer`; retire `extension_efficiency` | L | Med | none yet |
| 6 | **The one bump:** add `refill_distance` + `early_extension_source` fields, delete `traffic_congestion`, `to_plan_score` arms, **WORLD_FORMAT_VERSION 7→8**, **recalibrate `operations/claim.rs` `plan_score_weight`/`max_score_delta`** for the new `.total` scale | M | Med | **WFV 7→8** |
| 7 | Calibration sweep (operator-run): **λ=ALPHA/BASE, THREAT_MIN, OMEGA (cut weighting — track coverage *and* rampart count)**, `R_place`, `DESIRED_DPS_k`, ω, β, `N_early`, `D_REFILL_MAX`, claim weights | M | Low | none |
| 8 | *(optional, flag-gated)* Tier-2 power-mean aggregator + re-derived prune bound | M | Med-High | none |

Bench first (measurement). The tower fix is the biggest lever: repositioning (2a) is done; **the coverability-weighted min-cut (2b) is the new headline** — it removes the floor rooms repositioning can't, by shaping the perimeter to be coverable. 2b/2c are independent of the scoring/RCL/refill steps. The single bump batches all field changes + the claim recalibration. The risky aggregator swap is last and reversible by flag.

---

## 9. Cross-cutting consequences & invariants

- **`operations/claim.rs:213` is the only live consumer of `plan.score.total`** (expansion candidate scoring: `plan_score_weight=2.0`, absolute `max_score_delta=0.15`). Re-weighting (steps 3–5) shifts the `.total` scale, so claim's constants **must** be recalibrated (step 6). Open question: also expose a separate cross-room composite for expansion so it keeps the source/controller terms it implicitly relied on? (See §10.)
- **Invariants honored:** every sub-score ∈ [0,1] (prune soundness); `DefenseLayer` min-cut stays last among structure layers; one batched `WORLD_FORMAT_VERSION` 7→8; pure re-ordering/re-RCL needs no bump.
- **Rejected/deferred:** `traffic_congestion` articulation penalty (overlaps `refill_distance`, adds a full-room pass — deleted, not implemented); branching tower positions in the search (CPU cost; subsumed by `TowerRefineLayer`); seeding extension stamps near sources (geometry risk — revisit only if §5 ordering proves insufficient); offline weight tuning over the corpus (do last, after metrics are sound).

---

## 10. Open decisions for the operator

1. **Tier-2 aggregator — ship or not?** Tier-1 (arithmetic mean + zeroed weights + worst-component-inside-subscores) is safe and fixes RC5's worst case. Tier-2 adds discrimination among surviving layouts but loosens the prune and adds moving parts. *Recommendation: ship Tier-1, bench it, flip Tier-2 only if the corpus shows a measurable gain.*
2. **Tower threat model** sets `DESIRED_DPS_k`. Proposed values out-damage a boosted dismantler at each RCL (⇒ mean coverage targets ~0.5–0.7). Confirm the threat model (boosted dismantler vs ranged swarm).
3. **`claim.rs` recalibration scope:** (a) re-tune the two constants empirically, or (b) give expansion its own cross-room composite (keeps source/controller desirability that zeroing strips from `.total`). *Recommendation: (a) now, (b) as a clean follow-up.*
4. **`N_early` scaling** — fixed at the RCL4 cap (20, assumes a 2-source room); should it scale with source count?
5. **Tower perimeter** — passable ramparts only (proposed) vs the full walls+ramparts cut line.
6. **Lab hauling** — model a separate terminal→lab flood (Harabi's other named quantity) or keep the single storage-rooted score with lab freq 0.3?
7. **Coverability-weighted min-cut (§4.3) — adopt?** This is the headline tower fix beyond repositioning. It trades a little upkeep for a coverable perimeter (λ knob) and shapes the wall toward where towers can stand. *Recommendation: adopt; it is the only lever that removes the floor rooms.* Sub-decision: also allow dropping far low-value structures from the protected core for compactness (§4.4), or keep "protect everything" and rely on the weighting alone?

---

## 11. Provenance

Diagnosis and design were produced via two adversarial multi-agent workflows (deep code map + external research → synthesized brief; then a 4-philosophy judge-panel design → integrated spec), with all load-bearing claims independently verified against source. The shard3 corpus measurement (5,123 rooms) is the empirical baseline these changes will be measured against.

**Revision 2026-06-14 (§4 rework):** after the `TowerRepositionLayer` prototype (coverage 0.30→0.355) exposed an uncoverable-perimeter ceiling, §4 was reworked around a **coverability-weighted min-cut** that shares a tower-influence field `P(t)` with the tower layer — resolving the tower↔rampart bidirectional dependency via a static shared field while keeping the layers conceptually separate. Repositioning is retained as the final polish; footprint compactness is added as a complementary upstream lever.

**Revision 2026-06-14 (§4.3 composite cost):** the cut cost was generalized from coverage-only to a composite of three calibratable terms — `BASE` (upkeep/count) + coverage×**threat-direction** (via `exit_distances`, concentrating finite tower DPS on the attack-facing perimeter) − **natural-wall/chokepoint** leverage (`OMEGA`, preserving terrain advantage so coverage-weighting can't abandon cheap chokepoints) — with upkeep, wall/rampart split, protected-region membership, and economy/RCL deliberately kept out so the cut stays one clean bounded field. Terms land incrementally on the ground-truth bench, each gated on a corpus gain.
