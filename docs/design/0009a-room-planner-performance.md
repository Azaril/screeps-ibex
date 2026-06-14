# ADR 0009a — Room-Planner Performance: Diagnosis & Redesign

- **Status:** Proposed — **Phase 1 (UNDEFENDED fix + D4 cleanups) and Phase 2 (road Dijkstra) IMPLEMENTED 2026-06-14** (see §9); D1/D5 + placement-driven reachability outstanding.
- **Date:** 2026-06-14
- **Deciders:** William Archbell
- **Addendum to:** [0009 — Room Planning & Multi-Room Layout](0009-room-planning-and-multiroom-layout.md)
- **Supersedes (in part):** ADR 0009's "Alternatives Considered" row that *rejected* a planner rewrite on the grounds that *"no evidence the current cost is a problem (it is budgeted)."* Operator field evidence — the planner is "very slow" — plus the measurements below overturn that premise. The cost **is** a problem: not in wall-clock per tick (it is budgeted), but in **convergence latency** — a freshly-claimed open room burns hundreds of tick-budget slices before it has *any* layout to build.

---

## 1. Context & the metric that matters

The planner amortizes across many ticks on the live server (multi-tick `CpuBudget`, seg-60 resume — ADR 0009). So raw seconds are not charged to one tick. The harm is **how many ticks pass before a room has a plan**: total CPU work ÷ per-tick budget = ticks-to-converge. The offline `screeps-foreman-bench` wall-clock is therefore the right proxy for *total CPU work*, which is the thing that determines convergence latency. Minimizing it is the goal; keeping (or improving) layout quality is the constraint.

**Hard constraint (operator, confirmed canonical):** the min-cut (`DefenseLayer`) **must run last** — it computes the rampart ring enclosing the *complete* placed-structure set, so it structurally depends on every prior placement. This is also canonical practice (Harabi runs min-cut at step 9 of 11 — the "run min-cut early" idea is refuted folklore). The lever is therefore **not** reordering defense; it is *reducing how many candidates reach it.*

---

## 2. Measured diagnosis

Profiled with temporary per-layer instrumentation (since reverted) on the bench, room **W3S52** (shard3, an open room), single-threaded release build.

> **Bench build note:** `screeps-foreman-bench` is a workspace `exclude`; `cargo build -p screeps-foreman-bench` from the repo root **silently no-ops** (it built a stale binary that masked this regression at first). Build it with `cargo build --release --manifest-path screeps-foreman-bench/Cargo.toml`; binary at `screeps-foreman-bench/target/release/`.

**W3S52: 37 s, 47,234 candidate evaluations, 18 complete plans kept (1 winner).**

Per-layer cost (× call count):

| Layer (depth) | Total time | Calls | Note |
|---|---:|---:|---|
| `defense` (16, Dinic min-cut) | **10.9 s** | 12,564 | **rejects 6,228 / 6,282 = 99.1%** — runs last |
| `road_network_infra` (13, A*) | **10.3 s** | 12,564 | re-run per candidate |
| `extension` (14, places 60) | **6.4 s** | 12,564 | re-run per candidate |
| `controller_infra_v2` (10) | **4.8 s** | 6,631 | **branches ~18×**; also recomputes a full room flood-fill every call |
| `tower` (7) | 2.8 s | 698 | 4 ms/call |
| `lab_stamp` (5) | 1.3 s | 698 | radius-8 scan |

**The candidate explosion is multiplicative:**

```
anchor (~88 DT-valid tiles)  ×  hub rotation (~4)  ×  controller_infra_v2 (~18 placements)  =  ~6,282 candidates
                                                                          each runs the full expensive tail (road A*, 60 extensions, min-cut)
```

**Two root causes:**
1. **`controller_infra_v2` (the 2026-06-10 upgrade-area work-slot rewrite, ADR 0009 D2) is the regression.** It turned a deterministic 1-candidate layer into an ~18-way branch (`MAX_LINKED_CANDIDATES = 12` + `MAX_FALLBACK_CANDIDATES = 6`) sitting at depth 10 — *before* the three costliest layers — multiplying all of them ~18×. (A stale pre-rewrite binary planned the same room in ~5.7 s.) It additionally recomputes a full-room flood-fill on each of its ~6,600 calls.
2. **The feasibility gate (`defense`) runs last and rejects 99%.** Score-margin pruning is inert (163 / 47,234 = 0.3% pruned) because `cumulative_score` is a *weighted average over scored-so-far*, which is not comparable to the complete-plan average — so it almost never prunes.

**Defense rejection breakdown (W3S52): 93% `UNDEFENDED`, 7% `BORDER`.** Most candidates get a full min-cut computed and *then* fail `validate_all_structures_defended` (a structure is reachable from a room exit without crossing the cut). Only ~7% fail because the protected region touches the room edge. So the architecture is, in effect: *generate thousands of full layouts → run the expensive Dinic min-cut on each → keep the ~1% that seal.*

**Baseline corpus (shard3 `compare --limit 40`, 32 rooms planned):** total CPU work **203.9 s**, mean 6.4 s, p50 1.9 s, p90 10.1 s, **max 82.4 s**. Quality tells independent of speed: **`hub_quality` = 1.000 for every room** (a saturated, uninformative metric) and **`tower_coverage` ≈ 0.25 for nearly every room** (systematically poor placement — real quality headroom).

---

## 3. Prior-art research (cross-bot, adversarially verified — full cited brief in the appendix reference)

Consensus across Overmind, glitchassassin ("Architects"), Harabi, and TooAngel: **fix the *what* (stamps) and search only a tiny *where* (anchor); never run an expensive tail per candidate.** The full-tail-per-candidate shape foreman uses is used by *no* leading public bot, precisely because of CPU cost.

- **Overmind** [Bartlett]: one fixed RCL8 bunker stamp; DT to find open space → sample a *small set* of candidate anchors → pick min total path-length to sources+controller. Ramparts are a *static ring* around the known stamp → **no per-candidate min-cut at all**.
- **glitchassassin**: small fixed stamps (FASTFILLER/HQ/LABS) placed by **beam search** — enumerate fits, score with cheap Chebyshev-distance composite, keep only `TOP_N_LAYOUTS = 7` before placing the next stamp. Expensive work runs on ~7 survivors. Strict plan-once + cache + hard CPU budget.
- **Harabi**: 11-step pipeline; floodfill from the *single committed* anchor categorizes buildable tiles **and doubles as the reachability check** (run once, not per candidate); min-cut at step 9 (last).
- **Algorithms to keep** (verified correct in foreman): the Dinic vertex-split min-cut construction, the `TowerCoverageScoreLayer` metric (min-over-perimeter tower damage), the two-pass Chebyshev distance transform, A* road-reuse, and the multi-tick `CpuBudget`/seg-60 machinery. The problem is *how often and in what order* they run, not the algorithms themselves.

**Quality levers identified:** weight the min-cut's per-tile capacity toward chokepoints (currently uniform `1` — mainstream but leaves a quality gain on the table); fix `UpkeepScoreLayer` (its weights are *backwards* — ramparts decay ~30× more than roads but are weighted lowest); add a filler-walk-distance / extension-compactness score (currently unmeasured, yet it is the dominant *runtime* CPU axis).

---

## 4. What we tried (prototypes) and what the numbers proved

Every surgical, search-level fix was prototyped and measured on the corpus; all are reverted (the tree is clean). The results are the core evidence for the decision:

| Prototype | Speed | Quality / safety | Verdict |
|---|---|---|---|
| Cap `controller_infra` 18 → 5 | corpus 203.9 → **75.7 s (2.7×)** | **lost E11N25** (the ADR's own motivating room) — a plan-loss regression per ADR 0009's standing gate; other 30 rooms score-identical | ✗ unsafe |
| Cap `AnchorLayer` to top-48 by the scorer's composite | n/a | **0 complete plans** — the source-proximity objective (weight 3) ranks edge-adjacent anchors highest, and those are *undefendable*; defendable anchors fall outside the top-K | ✗ unsafe |
| `satisfice` controller infra (anchors stay exhaustive; commit to the first infra placement that completes) | corpus **196 s (~4%)**, no plan loss, scores ±0.002 | safe but **near-useless** — open-room completions are too rare to trigger the early stop | ◑ safe, ineffective alone |

**Why nothing surgical works:** the cost *is* the 99% rejection. You cannot cheaply pre-filter the candidates that will be rejected, because defendability is a *hard* property (min-cut feasibility + seal validation) that is **anti-correlated with the soft objective** (open + close-to-sources = often undefendable) and **depends on the full layout** (extensions/roads), not just the anchor. Capping or satisficing the *search* therefore either loses plans or loses nothing.

---

## 5. Decision — defendable-by-construction footprint + bounded anchor search

Keep the per-room `PlannerRoomDataSource → Plan` seam, the stamp infrastructure, the min-cut, the scoring metrics, and the multi-tick/seg-60 machinery. **Change the search shape** so the expensive passes run a handful of times, not thousands, by making layouts *defendable and reachable by construction*:

**D1 — Compact, bounded footprint (the headline change).** Place the core (hub + labs + spawns + towers) as stamps, and **bound extension placement to a compact region** around the hub (a stamp field or a DT-bounded disc sized for 60 extensions, ~radius 6–7), with a graceful 1×1-scatter fallback only when a bounded fit is impossible. A compact footprint is defendable by construction → the min-cut seal-validation passes for nearly every anchor → the 93% `UNDEFENDED` rejection collapses toward ~0. This is the Overmind/Harabi/glitchassassin family and is what makes their planners cheap. *Layout changes (and should improve): compact bases have better tower coverage, shorter rampart perimeter, shorter filler walk — all of foreman's measured quality weaknesses.*

**D2 — Bounded anchor search, exhaustive only where it matters.** With defendability no longer a near-random filter, rank anchors by the existing composite *plus a real (cheap, conservative) defensibility term* and evaluate the full pipeline on the **top-K (~10–20)** survivors — glitchassassin's beam. The anchor dimension is the one that drives quality, so it stays a genuine (bounded) search; infra-detail layers `satisfice` (commit to first valid). Together: ~10–20 anchors × ~1 infra each = tens of full-tail runs, not 6,282.

**D3 — Min-cut once, on the winner (or the top-K).** Demote the exact Dinic min-cut, the heavy `all_buildings` road pass, road-prune, and the reachability BFS to **single-winner finalization** (or top-K), and fold reachability into the committed-anchor floodfill (Harabi) so unreachable layouts are never built up. Min-cut still runs *last*; it just runs O(K) times, not O(candidates).

**D4 — Cheap mechanical fixes (do regardless):** memoize `controller_infra_v2`'s flood-fill (it recomputes a full BFS ~6,600×); add `satisfice()` to the layer trait (it is sound and becomes powerful once completions are common after D1); add proper `screeps-timing` annotations to the layers so this profiling is reproducible without ad-hoc instrumentation.

**D5 — Quality fixes (make the output *objectively better*, not just faster):** correct `UpkeepScoreLayer` to weight by real decay energy; weight the min-cut toward terrain chokepoints; add a filler-walk/extension-compactness score; retire or rescale the saturated `hub_quality` metric. Calibrate weights on the bench corpus (ADR 0009 D2's standing plan-loss-accounting gate applies).

**Investigate first — possible cheap win:** the **93% `UNDEFENDED`** rate is suspicious. A correct min-cut over an 8-neighbour graph should rarely fail its own 8-neighbour seal-validation. Candidate causes worth a focused look before the larger redesign: `defense.rs:428` **skips ramparts on border tiles** (`!loc.is_border()`) which can leave a seal gap; and a possible mismatch between the *protected* set (what the cut seals) and the *must_defend* set (what validation checks). If this is over-rejection rather than genuine, fixing it would raise the completion rate dramatically and make D2/`satisfice` immediately effective — a large win inside the current architecture. If it is genuine (open rooms truly cannot be sealed sprawled), it directly confirms D1.

---

## 6. Layout family — tiered, terrain-adaptive

1. **Compact bunker/stamp (try first):** near-zero planning cost, best runtime CPU, concentrated tower damage, single short rampart ring. Use DT to test the square fits.
2. **Stamp/modular (fallback when the bunker square won't fit):** the family foreman already resembles (hub + lab + extension stamps); exploits terrain to route the min-cut perimeter through natural chokepoints — often *fewer* ramparts than a bunker in walled rooms.
3. **Organic/path-based (cramped rooms of last resort):** weakest defensively, highest upkeep; only when neither stamp fits.

Fallback ladder: bunker square fits → tier 1; else stamps fit → tier 2; else → tier 3. Removes the exhaustive DFS for the common open-room case that currently costs 37–82 s.

---

## 7. Incremental roadmap (each bench-validated; plan-loss accounting per ADR 0009 D2)

0. **Reproducible profiling:** add `screeps-timing` annotations to the layers; commit the shard3 corpus baseline (203.9 s, scores) as the regression yardstick. *(None-breaking, offline.)*
1. **Investigate the 93% `UNDEFENDED`.** If over-rejection: fix → re-measure (may already be a large win). *(Behavioral, in-flight-plan reset.)*
2. **D4 cheap fixes** (flood-fill memoization, `satisfice()`): safe, no plan loss. *(Behavioral, fingerprint reset.)*
3. **D1 bounded extension footprint** behind a builder flag; A/B vs baseline on the corpus — expect rejection rate and total CPU work to fall sharply with equal-or-better scores. *(Behavioral, in-flight-plan reset.)*
4. **D2/D3 beam + min-cut-on-survivors**; verify identical-or-better winning plans at a fraction of the candidate count. *(Behavioral.)*
5. **D5 quality fixes + weight calibration** on the corpus. *(Behavioral.)*

**Breaking-change posture:** all changes are intra-room `Plan`-producing behavior + in-flight `PlanningState` (seg-60) resets via the layer-name fingerprint (completed `Plan`s persist). No `Plan`-format break.

---

## 8. Consequences

**Positive:** convergence latency drops from hundreds of ticks toward tens for open rooms; layout quality improves where it is measurably weak today (tower coverage, rampart perimeter, filler walk, upkeep weighting); profiling becomes reproducible; the planner aligns with proven prior art while keeping its (verified-correct) min-cut, tower metric, DT, and multi-tick machinery.

**Negative / risks:** D1 changes the layout family (acceptable — operator approved "objectively better, need not be identical"); a bounded footprint must keep its 1×1 fallback so cramped rooms (E11N25-class) never lose their plan — the standing plan-loss-accounting gate is mandatory on every step; weight changes shift which layout wins (a Behavioral change confined to low-stakes ticks; in-flight replans only, completed plans persist).

---

## 9. Implemented — Phase 1 (2026-06-14)

The UNDEFENDED investigation and the safe cleanups landed together; all on the bench corpus (shard3, `compare --limit 40`), all unit tests pass, warning-free.

**Root cause of the 93% UNDEFENDED (identified, then fixed).** Instrumentation proved **100% of UNDEFENDED rejections had the min-cut routing through room-edge tiles** (3–40 per rejection). The flow network treated edge tiles as cuttable (capacity 1), so the cheapest cut "sealed" exits by ramparting the *exit tiles themselves* — unbuildable (no structure sits on the room border), so they were dropped on cut-extraction (`defense.rs` `!is_border()` filter), leaving seal gaps. The planner was **rejecting layouts that had a valid buildable interior seal**, just not the cheapest one.

**Fix (`defense.rs`):** give room-edge tiles **infinite in→out capacity** (uncuttable), forcing the min-cut onto buildable interior tiles. Standard min-cut-for-Screeps practice. Now the computed cut is always buildable; defense rejects ~0% instead of 99%.

**Supporting changes (to exploit the now-mostly-accepting defense without exploding the search):**
- **`satisfice()`** layer-trait method (`layer.rs`): a satisficing layer's candidates only need to be *valid*, not optimal, so the search stops exploring its siblings once a branch completes. `controller_infra_v2` opts in (its placements barely affect the final score). Anchor/hub stay exhaustive.
- **Admissible pruning** (`search.rs`): replaced the old `partial-average + margin < best` test (a *non-admissible* bound that, once the edge-fix made completions common, pruned the true best and regressed 9 rooms) with an optimistic-completion upper bound — *even if every remaining layer scored 1.0, can this branch beat the best?* It never prunes the optimum (`prune_margin` now 0.0).
- **Anchor beam** (`anchor.rs` + `composite_anchor_score` in `anchor_score.rs`): emit only the top-K (=16) anchors by the same composite the scorer optimizes. Now *safe* (the edge-fix made defendability no longer anti-correlated with the score, so the top-K contains defendable, high-scoring anchors — before the fix this lost every plan).
- **`controller_infra_v2` cap** 12+6 → **4+3**: the edge-fix eased E11N25's controller crevice, so the generous count is no longer needed (E11N25 still plans).

**Results (shard3 corpus, vs the pre-fix baseline of 203.9 s / 32 rooms planned):**

| Metric | Baseline | Phase 1 | |
|---|---|---|---|
| Rooms planned | 32 / 40 | **40 / 40** | +8 recovered (none lost) |
| Total CPU work | 203.9 s | 210.8 s | ~flat total, but for 8 more rooms (≈16% faster on the 32 common rooms) |
| Worst room | **82.4 s** | **8.5 s** | **9.7× better tail latency** |
| W3S52 (open) | 37 s, score 0.7271 | **4.2 s, score 0.8134** | 8.8× faster, +0.086 score |
| Quality vs baseline | — | **30 improved, 1 same, 1 −0.0045** | strictly better, no real regressions |

The defense fix is a **correctness** win (it was wrongly rejecting defendable layouts); the headline outcomes are far better worst-case latency, +8 recovered rooms, and uniformly better layouts.

## 9a. Implemented — Phase 2: road network as a single Dijkstra tree (2026-06-14)

Phase 1 left the *total* CPU flat because the edge-fix made defense accept ~all candidates, so the expensive tail ran on every survivor. The first idea — "demote `road_network`/`road_prune`/`reachability` to the winner" — is **unsound**: `reachability` is a *rejection gate*, so if the winner fails it you have no plan and have discarded the runners-up. The better fix is to make those layers cheap and/or non-rejecting in place:

**Road network → one hub shortest-path tree (`road_network.rs`).** Replaced the N independent per-destination A* searches with a **single Dijkstra from the hub** (same edge-cost model: existing-road 0, plain 10, swamp 60, container +100, slot +200, building impassable, −1 road-adjacency discount), then trace each destination back through parent pointers. Trunk-merging is automatic (a shortest-path tree shares edges near the root); early-terminates once all *reachable* targets are finalized. This eliminates the all-buildings pass's biggest waste — N **failed** full-room A* searches looking for impassable building goals (1022 ms → ~580 ms on W3S52), and is behaviour-preserving (impassable/unreachable destinations get no road, exactly as the old A* returned "no path"). **Corpus: 210.8 s → 168.7 s (now below the 203.9 s baseline), worst room 8.5 s → 6.46 s, quality unchanged (30 improved / 1 same / 1 negligible, 40/40 rooms).**

**Reachability investigated, not changed (correctly).** Two hypotheses were tried and reverted: a reachability-aware min-cut (flip interior-disconnecting walls to ramparts) left rejections *exactly* unchanged — so they are **not** min-cut-wall-caused; and routing roads to building-adjacent tiles was inert. Instrumentation showed **~89 % of reachability rejections are placement issues** — 98/99 "unreachable" cases are the **Factory boxed in by `UtilityLayer`** (`place_near_hub` takes the first free tile with no access check), the rest are spawn-adjacent dead-end pockets. These rejections are **benign** (they don't reject the *best* plan and cause no room failures), and `reachability` is already cheap (~94 µs/candidate), so it stays per-candidate. The real remedy is a placement fix (reserve an access tile for the Factory/PowerSpawn, like the controller approach) — deferred as its own change.

### Cumulative result (baseline → Phase 2), clean single-threaded measure

The `compare` harness runs rooms in parallel (rayon), so its per-room durations inflate under core contention — early SUM figures (203.9 / 210.8 / 168.7 s) varied ±20 % with machine load. The honest apples-to-apples is **single-threaded** (`RAYON_NUM_THREADS=1`): each room's duration is its true CPU, and the sum is the true total. Quality is deterministic and load-independent.

| Metric (single-threaded) | Baseline | Phase 2 |
|---|---|---|
| Rooms planned | 32 / 40 | **40 / 40** (+8 recovered, 0 lost) |
| Total CPU (planned rooms) | 132.9 s / 32 | **105.0 s / 40** (≈1.6× faster per room; baseline also paid for 8 *failed* full searches not counted here) |
| Worst room | **54.9 s** | **4.7 s** (≈11.7×) |
| W3S52 (open room) | 37 s / 0.727 | **3.6 s / 0.813** (≈10×, +0.086 score) |
| Quality vs baseline | — | **30 improved, 1 same, 1 × −0.002** |

10 files changed (+417/−188), **clippy-clean**, 24 tests pass. Uncommitted, pending operator review.

**Adversarial correctness review (6-agent workflow):** verdict GO after one must-fix (a clippy `needless_range_loop` in the edge-fix loop — fixed). Core changes verified **sound** — the edge-fix can never make a defendable room undefendable (a finite interior cut always exists; `INF_CAP` never overflows), the admissible prune is a genuine upper bound (all scores ∈[0,1]; `total_score_weight` constant), the road Dijkstra is behaviour-preserving and deterministic, and serde/wasm/fingerprint are neutral. Two **should-fix** heuristic caveats remain (empirically 0 plan-loss on 40 rooms, but not *provably* safe): the anchor beam (top-16) and `controller_infra` satisfice/4+3 cap could drop a pathological room whose only feasible anchor/placement ranks low — an **adaptive "widen the beam / lift the cap if no plan is found" fallback** would make the no-plan-loss gate provable. The reviewer also notes the beam and the defense edge-fix are a **coupled** change set (top-K was unsafe *before* the edge-fix) and must ship together.

## 10. Still outstanding (next phases)

- **Placement-driven reachability rejections** — fix `UtilityLayer` Factory/PowerSpawn placement (reserve a hub-connected approach tile) and the spawn dead-end-pocket cases. Reduces wasted per-candidate work and recovers anchors currently rejected.
- **`tower` (≈4 ms/call) and `defense` min-cut (≈0.9 ms/call)** are now the co-dominant per-candidate costs — candidates flow through them because nothing rejects early. Cheaper tower-coverage scoring and/or fewer candidates (tighter beam, or D1) are the next speed levers.
- **D1 — bounded/stamped footprint** further shrinks the search and improves compactness.
- **D5 — scorer quality fixes**: `UpkeepScoreLayer` weights are backwards (ramparts decay ~30× more but are weighted lowest); weight the min-cut toward chokepoints; add a filler-walk / extension-compactness score; retire the saturated `hub_quality`.

## Appendix — evidence artifacts

- Per-layer timings, candidate counts, and defense rejection breakdown: §2 (reproduce with the bench build note + `RUST_LOG=screeps_foreman=debug`).
- Prototype experiment results: §4 (all reverted; tree clean).
- Cited research brief (Overmind/Harabi/glitchassassin/TooAngel, min-cut/DT/beam, quality metrics): multi-agent research run, 19 agents, 35 findings, 14 adversarially verified, 2 refuted (including the "min-cut early" folklore).
