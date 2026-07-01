# ADR 0038 — Expansion reach-gating + economic (net-ROI) claim value

- **Status:** **Implemented on the working tree (2026-07-01), pending operator review + commit + deploy.**
  Operator-directed redesign of claim/expansion selection, decided this session: **combined one-pass** change
  (unblock + value-unify together), **ADR-first**, WFV 22→23 reset approved. Landed: the pure
  `claim_economics.rs` kernel (13 tests) + `RoomEconomyFacts::owned_colony`; `claim.rs` reach-gating + cadence +
  kernel scorer + deterministic tie-break; `ClaimFeatures` config migration; `gather.rs` `frontier_truncated`
  removal; `game_loop.rs` WFV 22→23. **Host: 233/233 green; wasm clippy: clean, zero warnings.** Not yet
  committed or deployed (MMO deploy is one loud reset).
- **One line:** The empire stops expanding because claim selection **hard-rejects every candidate below a
  distance-4 floor** and the last-resort escape hatch is gated on a search-radius ratchet that widens one hop
  per ~500-tick discover cycle (or never, when boxed in). Fix: gate claims **only on claimer reach**
  (`is_claim_feasible`, ~11 hops), search the full claimer-viable range every cycle, scale the re-scout cadence
  **formulaically by search-area size**, and replace the ad-hoc scoring with the existing pure
  [`room_economics::room_net_roi`](../../screeps-ibex/src/room_economics.rs) kernel — where the "sprawl to avoid
  source competition" preference is re-derived as **real marginal economics** (a room one hop from a colony
  holds sources that colony *already remote-mines* → ~0 marginal gain; a room past the remote-ring-overlap
  distance unlocks *new* economy), never as a hard stall.

Extends [ADR 0032](0032-ev-optimal-squad-assignment.md) (which built the pure `room_net_roi` kernel and
**explicitly pre-committed** it to "a FUTURE expansion/claim-selection scorer" — `0032:17,79,82-83`;
`room_economics.rs:16-18`). Interacts with [ADR 0017](0017-threat-aware-expansion-lifecycle.md) (the claim
safety/lifecycle gate, unchanged) and [ADR 0014](0014-empire-strategy-and-posture.md) (the CPU room-cap,
unchanged). The adaptive-radius/cannibalization machinery being removed here has **no governing ADR** — it
shipped in `2e54f66` as a code-comment-only "Workstream B" (audited this session), so this is the first ADR to
own claim *selection*.

---

## 0. Problem statement (live MMO, 2026-07-01)

The bot has GCL/CPU headroom (`available=3` room slots, `max_rooms=11`, `owned=8`, `at_capacity=false`,
`cpu_healthy=true`) and **6 valid claim candidates**, yet creates **zero** claim missions, every discover
cycle:

```
ClaimOp [Select]: 6 candidates total, 0 unscored, 0 hostile, 6 remaining
  #1 W14N59 score=0.552 dist=3   #2 W14N58 score=0.463 dist=2
  #3 W13N51 score=0.218 (source=1.00 walk=0.98) dist=1   #4 W12N55 dist=1  #5 W11N56 dist=1  #6 W12N52 dist=1
ClaimOp [Select]: candidate W14N59 at distance 3 below min claim distance 4, skipping (would cannibalize remotes)
  … (all six skipped) …
ClaimOp [Select]: had 6 scored candidates but created no missions
```

The operator clarified: on MMO the empire is **not boxed in** — viable rooms simply sit **far** (many hops).
Note `#3 W13N51` is a **2-source, highly-walkable** room one hop away, rejected as cannibalizing while nothing
farther is ever claimed.

**Two coupled defects:**

1. **Selection is gated on an artificial distance floor + a slow radius ratchet, not on claimer reach.** All
   candidates at distance < 4 are hard-rejected; the only bypass reaches max radius (11) at +1 hop per discover
   cycle — thousands of ticks, and never at all when the reachable frontier is exhausted.
2. **The value function is ad-hoc.** `source_score` (`min(2)/2`), `walkability_score`, a hand-tuned
   `distance_score` peaking at 4, and a `×0.3` `adjacent_claim_penalty` approximate — badly — a real quantity
   the codebase already computes elsewhere: the **net-ROI economic value of controlling a room**
   (`room_net_roi`). The distance curve *is* an unprincipled stand-in for source-competition economics.

---

## 1. Root-cause map (the stall) — verified against the tree 2026-07-01

| # | Mechanism | file:line |
|---|---|---|
| S1 | **Distance floor = `min_search_radius` (=4).** Every candidate with `distance < 4` is skipped unless `allow_penalized`. | `operations/claim.rs:655` (`min_claim_distance = features.min_search_radius`), `:769-778` (the skip + "would cannibalize remotes" log) |
| S2 | **The escape hatch depends entirely on the radius reaching 11.** `nearest_good` = min distance among candidates with `distance >= 4`; when none reach 4 it is `None`, so `allow_penalized = current_search_radius >= max_radius(11)`. | `operations/claim.rs:657-665` |
| S3 | **The radius ratchet widens +1 per cycle, and only when there is more reachable map.** With `nearest_good=None`, widening requires `available_rooms>0 && covered && frontier_truncated`; else it HOLDS. Boxed-in (`frontier_truncated=false`) ⇒ never widens ⇒ `allow_penalized` never true ⇒ permanent stall. Not-boxed-in ⇒ ~7 cycles × `discover_interval(500)` ≈ 3500–4900 ticks of stall before the hatch opens. | `operations/claim.rs:913-935`; `frontier_truncated` `room/gather.rs:238` |
| S4 | **Scouting window (200) is far too short to score far candidates.** A scout needs ~`50 × distance` ticks just to arrive (~550 for d=11); each cycle only the already-visible near rooms (d 1–3) get scored, so `nearest_good` stays `None` and the far rooms that would clear the floor are pruned unscored. | `features.rs:408` (`scouting_window=200`); prune of unscored `operations/claim.rs:615` |
| S5 | **Every downstream gate would PASS a distance-4 room** (claimer reach `200+50 ≤ 600`, RCL3 affordability, plan, safety) — so the floor + ratchet are the *sole* blocker. | reach `missions/utility.rs:91-96`; affordability `operations/claim.rs:865`; verified this session |

**Consolidated:** S1 rejects the near rooms; S2–S4 ensure the far rooms are never scored *and* the escape
hatch never fires ⇒ "had N scored candidates but created no missions" indefinitely.

---

## 2. Decisions

Two parts. **Part A** (D1–D3) unblocks expansion (mechanical). **Part B** (D4–D8) unifies the value function
on the shared economic kernel. Both land together (operator: combined one-pass).

### Part A — gate on claimer reach; search the full range; scale cadence by area

- **D1 — Fixed search radius = `max_claim_radius_hops()` (11), every cycle.** Replace the adaptive
  `current_search_radius` clamp (`claim.rs:278-283`) with `let radius = missions::utility::max_claim_radius_hops();`.
  A far viable room is found on the **first** discover, not after N widening cycles. "Unlimited expansion to the
  world edge" is emergent: 11 hops is a single claimer's physical reach, and each new colony re-seeds the BFS,
  so the frontier crawls outward indefinitely. The `MAX_GATHER_VISITED_ROOMS=256` budget (`gather.rs:136`) still
  bounds CPU; if the far edge truncates in practice, raising that cap is a separate one-line lever (flagged, not
  changed here).

- **D2 — The SOLE hard claim gate is claimer reach.** Delete the distance floor (S1), the `allow_penalized`
  escape hatch (S2), and the entire radius ratchet (S3): `min_claim_distance`/`nearest_good`/`allow_penalized`
  (`claim.rs:655-665`), the below-floor skip (`claim.rs:769-778`), the ratchet block (`claim.rs:913-935`), and
  the serialized `current_search_radius`/`frontier_truncated` fields. The per-home `is_claim_feasible` gate
  (`claim.rs:868`, `utility.rs:91-96`) becomes load-bearing — it already enforces "a claimer can arrive alive."
  The **anti-cannibalization intent is preserved in Part B's *scoring*** (a distance-1 room scores low via
  `unlock_fraction`), never as a hard block — so a dense/boxed empire still expands. This is the operator's
  "slightly sprawl, but only gate on viable claimer distance."

- **D3 — Formulaic re-scout cadence, scaled by search-area size ("ring").** The number of rooms tracked grows
  with the reachable area; re-discovering (re-BFS + re-prioritising scouts) on a fixed 500-tick interval thrashes
  the scout queue so scouting "never completes." Scale both cadences by the *actual* tracked-room count (which
  reflects how much reachable map exists — more adaptive than a radius-derived constant now that the radius is
  fixed):
  ```
  tracked        = candidates.len() + unknown_rooms.len()          // "candidate room size"
  discover_every = clamp(discover_interval + rediscover_ticks_per_room · tracked,
                         discover_interval, max_discover_interval)
  scout_window   = clamp(scouting_window + TICKS_PER_HOP · radius   // let a scout reach the frontier ring
                                         + scout_ticks_per_room · unknown_rooms.len(),
                         scouting_window, max_scouting_window)
  ```
  Small/dense empires stay responsive (tracked small ⇒ near the base values); a wide MMO frontier re-scans
  proportionally *less* often and gives scouts time to reveal the outer ring. The existing coverage-early-exit
  (`scouting_coverage_complete`) still fires Select the moment the reachable ring is covered — the window is only
  the ceiling. Constants (`rediscover_ticks_per_room≈10`, `max_discover_interval≈5000`, `scout_ticks_per_room≈5`,
  `max_scouting_window≈2500`) are tunable; `TICKS_PER_HOP` is the existing `utility.rs:69` const.

### Part B — unify the value on `room_net_roi` (sprawl = marginal economics)

The unified per-candidate value, computed in a new **pure** kernel `claim_economics.rs` (sibling to
`room_economics.rs`; world-free, host-tested), consumed by a thin adapter in `claim.rs` that gathers facts —
mirroring the `room_economics`/`war.rs` split ADR 0032 sanctioned:

```
claim_value(R, d) = intrinsic_roi(R) · unlock_fraction(d) · support_decay(d) · plan_quality(R)
```

- **D4 — `intrinsic_roi(R)` is the room's OWNED-colony net-ROI, and is distance-INDEPENDENT.** Build
  owned-colony facts and call the existing kernel:
  `room_net_roi(RoomEconomyFacts{ source_count, source_capacity = SOURCE_ENERGY_RESERVED_CAPACITY(3000),
  hold_model = HoldModel::None, haul_tiles = INTERNAL_HAUL_TILES(≈25), horizon })`. A claimed room becomes its
  **own self-hauling colony**: sources restore to the owned 3000/cycle (`engine-mechanics.md:445,466`), are
  mined and hauled *internally* (~half a room ≈ 25 tiles), and need no standing reserver (`HoldModel::None`).
  **The claim distance MUST NOT be passed as `haul_tiles`** — that is the war.rs *remote* adapter's pattern
  (`war.rs:1181-1185`, correct for a remote held at range) and is **wrong for a claim**.

  > **⚠ C-DEFECT-1 (caught in pressure-test, the reason this is a decision, not a footnote).** `room_net_roi`
  > is *not* distance-independent: its haul + cpu terms drive `net_per_tick` to **exactly 0 at d≥4 (1-source) /
  > d≥6 (2-source)** when `haul_tiles = d·50`. If intrinsic passed claim-distance as haul, then
  > `intrinsic(d)·unlock_fraction(d) = 0` in the very "new economy" band (d≥4) the model is meant to reward — a
  > 1-source room at d=4 would score 0 and a boxed-in empire whose only reachable rooms are 1-source at d≥4
  > would be **permanently stalled**: the exact failure this ADR removes. Fixing haul to `INTERNAL_HAUL_TILES`
  > makes intrinsic depend only on source count/capacity, so distance enters **once**, via `unlock·support`.
  > Add a convenience ctor `RoomEconomyFacts::owned_colony(source_count, internal_haul_tiles)` beside
  > `reservable_remote` (`room_economics.rs:112`); no change to `room_net_roi`'s body ⇒ war.rs numbers unmoved.

- **D5 — `unlock_fraction(d)` — the sprawl term, grounded in remote-ring economics, floored nonzero.** A colony
  remote-mines at **radius 1** (`miningoutpost.rs:110` BFS `max_distance=1`; no downstream distance cap). So a
  room at d=1 holds sources that colony *already* mines (full cannibalization, ~0 marginal gain); two colonies'
  radius-1 rings become disjoint only past `2·1 + 2 = 4` hops of separation (the exact rationale in the
  `features.rs:434-437` comment, now made load-bearing). Curve — **monotone-nondecreasing** (more distance ⇒
  more unlocked economy, up to full separation; the decline that the old peak-at-4 `distance_score` conflated is
  now `support_decay`'s job):
  ```
  unlock(0) = 0                                                             // home room (excluded anyway)
  unlock(d) = UNLOCK_FLOOR + (1-UNLOCK_FLOOR)·(d-1)/(RING_SEP-1)   for 1 ≤ d < RING_SEP
  unlock(d) = 1.0                                                  for d ≥ RING_SEP
  ```
  `RING_SEP` = the (renamed) `min_search_radius` config value **4** — reuse the existing config so the remote-ring
  math and the curve knee can never drift; do **not** hardcode 4. `UNLOCK_FLOOR = 0.05` is the **anti-stall
  constant**: its existence is what guarantees a cannibalizing room scores *low-but-nonzero* (so a dense/boxed
  empire still expands as a last resort). This subsumes both `distance_score` (peak@4) and
  `adjacent_claim_penalty` (`×0.3` at d=1) — and `unlock(1)=0.05` is a *stronger, principled* penalty than the
  old `0.3`.

- **D6 — `support_decay(d)` — mild far-room establishment/support penalty, RECIPROCAL form, never a gate.**
  ```
  support_decay(d) = 1 / (1 + k·d),   k ≈ 0.05     // ⇒ 0.645 at d=11; strictly > 0 for all finite d
  ```
  Models that reinforcement/logistics thin with distance beyond the internal-haul term. The **hard** reach
  cutoff is `is_claim_feasible` (~11 hops) upstream (D2); `support_decay` only *tilts* among feasible rooms and
  **must never reach 0 within reach**. A linear `1 − k·d` form is forbidden (hits 0/negative within range and
  re-introduces a stall) — the reciprocal form is mandatory and asserted in tests.

- **D7 — A valid plan is a HARD claim prerequisite; plan *quality* is a soft multiplier.** Two distinct roles,
  kept separate (operator: "not having a plan for a room should block claiming — we can't then build the room"):
  - **(1) Existence + validity = hard gate (block).** A room may **not** be claimed without a valid plan. This
    is already enforced and is KEPT: the viable gate excludes an *invalid* plan (`can_plan = plan.valid()`,
    `gather_candidate_room_data` `claim.rs:134-138` → `viable`), and the mission-creation gate defers a
    *not-yet-planned* room — requesting a plan and skipping this cycle — until the plan exists
    (`claim.rs:830-837`). **No claim mission is ever created for a room lacking a valid plan.**
  - **(2) Quality = soft multiplier.** `plan_quality = 0.1 + 0.9·plan.total` for a planned room — floored so a
    low-but-*valid* plan cannot hard-zero an otherwise-claimable room (C-DEFECT-2). While a room is still
    awaiting its plan, *scoring* uses a neutral `1.0` **only to keep it in the ranked set so its plan gets
    requested** — this is NOT a bypass of gate (1): the room still cannot be *committed* until a valid plan
    lands. (Scoring a not-yet-planned room as 0 would prune it before its plan is ever requested — a deadlock;
    the block correctly lives at *commit*, not at *scoring*.)

- **D8 — Composite + quantized total-order tie-break (the determinism fence).** `claim_value` is `f64`; the only
  discrete step is the argmax/sort in `run_select` (`claim.rs:626`). A continuous `f64` sort key is itself fence-
  legal (war.rs blesses the same pattern, `war.rs:1925`), **but the candidate Vec derives from a
  `HashMap<RoomName,…>` BFS (`gather.rs:149`)**, so equal/near-equal-scored candidates would inherit seed-flaky
  iteration order (the ~1% noise `[[sim-determinism-fence]]` forbids). Case-2 below shows ~1% margins are real.
  **Sort/argmax on an explicit total order `(quantized_score_desc, room_name_asc)`**, where
  `quantized = (claim_value · 1000.0).round() as i64` — quantize so f64 rounding can't split a genuine tie, then
  break remaining ties on `RoomName`. No `HashMap` iteration on the selection path.

**Worked ordering (corrected model, `intrinsic` distance-independent; illustrative energy-equiv values):**

| candidate | value | note |
|---|---|---|
| 2-source @ d=1 (W13N51) | ~1210 | **low but nonzero** — `unlock(1)=0.05`; claimable last-resort, ~17× below the ring |
| 2-source @ d=4 (ring) | ~21167 | dominates the adjacent room ~17× — cannibalization discount is decisive |
| 1-source @ d=8 | ~8571 | **beats** the near cannibalizing 2-source @ d=2 (~8467) — sprawl intent, by ~1% (⇒ D8 tie-break is load-bearing) |
| 0-source / no-controller | 0 | excluded by the **viable gate** (`claim.rs:147`) primarily; score-0 is defense-in-depth |

---

## 3. The pure `claim_economics` kernel + the `claim.rs` adapter

**`screeps-ibex/src/claim_economics.rs` (NEW, pure, world-free, host-tested).** Holds `unlock_fraction`,
`support_decay`, and `claim_value(source_count, source_capacity, distance_hops, plan_total: Option<f32>, cfg)`
— which internally calls `room_economics::room_net_roi` with owned-colony facts (D4), applies D5–D7, and
returns both the `f64` value and its `i64` quantization (D8). **All** economic math lives in the two pure
kernels; **zero** `game::*`. Bit-deterministic scalar `f64` (no `HashMap`), per the ADR 0032 kernel contract.

**`claim.rs` adapter (in `score_candidate`).** Gathers facts only — `source_count` from
`static_visibility_data.sources().len()` (mirror `war.rs:1154-1158`), the BFS `distance`, `plan.score.total` —
and calls `claim_economics::claim_value`. No policy in the kernel; no economics in the adapter.

---

## 4. Migration ledger

**DELETE** (`claim.rs` unless noted): `current_search_radius` field+init+writes (`:64-69,98,282-283`);
`frontier_truncated` field+init+write (`:72-73,99,301`); the adaptive-radius clamp (`:278-283`); `source_score`
(`:155-167`); `walkability_score` (`:169-186` — terrain is already in the foreman plan score; dropping avoids
double-counting); `distance_score` + its test (`:193-205,1271-1293`); `adjacent_claim_penalty` multiply
(`:249-251`); `min_claim_distance`/`nearest_good`/`allow_penalized` (`:655-665`); the below-floor skip
(`:769-778`); the `min_claim_distance` clause in `scouting_coverage_complete` (`:539,542` — keep the freshness-
coverage logic for all viable candidates); the ratchet block (`:913-935`). Optional: `gather.rs`
`frontier_truncated` field+accessor (`:71,88,238`) — only caller was `claim.rs:301`; `sourcekeeper.rs:259` and
`miningoutpost.rs` never read it.

**KEEP (unchanged):** `plan_score` (`:207-214`); **the plan hard-gate (D7): the viable `can_plan = plan.valid()`
exclusion (`gather_candidate_room_data` `:134-138`) + the mission-creation defer-until-planned gate (`:830-837`)
— a claim is never committed without a valid plan**; the ADR-0017 safety gate + commit-time re-check
(`:421-456,784-808`; `utility.rs:17-47`); `is_claim_feasible` per-home gate (`:868`) — now the sole reach gate;
the Discover→Scouting→Select machine + `try_score_candidates`/`refresh_visibility_requests`; `compute_maximum_rooms`
+ CPU-cap; `max_concurrent_missions`, `max_score_delta`; all ADR-0017 lifecycle features.

**ADD:** `claim_economics.rs` (D5–D8) + `RoomEconomyFacts::owned_colony` ctor + `INTERNAL_HAUL_TILES`; the
kernel-based `score_candidate` body (D4); fixed `radius` (D1); the two formulaic-cadence helpers (D3); reshaped
`CandidateSubScores` viz fields (`roi`/`unlock`/`decay`/`plan` replacing `source`/`walkability`/`distance`) +
the updated `[Select]` log (`:633-648`) and viz map (`:967-977`) — viz type is not world-serialized.

**`ClaimFeatures` (`features.rs`, `#[serde(default)]` ⇒ no WFV impact):**
- REMOVE: `distance_score_weight` (`:441`), `adjacent_claim_penalty` (`:445`), `plan_score_weight` (`:401` —
  the D7 `plan_quality` uses a fixed `0.1 + 0.9·total` floor/slope, so the old weighted-average weight is dead;
  the `plan_score` *function* that reads `plan.total` is KEPT).
- RENAME/REPURPOSE: `min_search_radius` (`:438`, =4) → `ring_separation_hops` — no longer a hard floor; now the
  `RING_SEP` unlock-saturation knee (D5). (Reconciles the two research findings: the *floor role* is deleted, the
  *value 4 = 2·remote_range+2* lives on as the curve knee.)
- ADD: `rediscover_ticks_per_room`, `max_discover_interval`, `scout_ticks_per_room`, `max_scouting_window` (D3);
  `unlock_floor` (=0.05), `support_decay_k` (=0.05), `internal_haul_tiles` (=25), `roi_reference` (net-ROI→~0–1
  normaliser for `max_score_delta` compatibility) (D4–D6).
- KEEP: `on`, `visualize`, `max_concurrent_missions`, `max_score_delta`, `discover_interval`, `scouting_window`,
  `remote_build_interval`, CPU-cap block, all ADR-0017 fields. Update `ClaimFeatures::default()`.

---

## 5. Sim-first test plan (pure-kernel; the complete surface for Part B)

Per operator standing rule + `[[sim-determinism-fence]]`: prove RED→GREEN offline. Part B is a **pure scalar
kernel** — its correctness is fully determined by the math, so the test surface is **kernel unit tests only**
(mirroring `room_economics.rs:181-243` + `claim.rs`'s `distance_score` tests). **No claim-specific ibex-eval**
— the Docker harness would exercise the *pipeline* (Part A reach/cadence), not the value function, and buys
nothing the kernel tests don't pin. In `claim_economics.rs`'s `mod tests`:

1. `unlock_zero_at_home_floored_positive_for_all_reachable` — `unlock(0)=0`; `unlock(d) ≥ UNLOCK_FLOOR > 0` for
   `1 ≤ d ≤ 11`; `unlock(d)=1.0` for `d ≥ RING_SEP`.
2. `unlock_is_monotone_nondecreasing` — the anti-peak-at-4 regression guard.
3. `support_decay_strictly_positive_and_monotone_decreasing_over_reach` — reciprocal-form guard (a linear form
   fails); `support(d) > 0` for all `d ≤ max_claim_radius_hops()`.
4. `intrinsic_roi_is_distance_independent` — same facts ⇒ byte-identical intrinsic regardless of `d` (proves
   C-DEFECT-1's fix: distance sensitivity lives only in `unlock·support`).
5. `w13n51_two_source_d1_is_low_but_strictly_nonzero` — `0 < value(2,d=1) < value(2,RING_SEP)/5` (anti-stall fence).
6. `far_single_source_beats_near_cannibalizing_double_source` — `value(1,d=8) > value(2,d=2)` (sprawl ordering).
7. `ring_room_dominates_adjacent` — `value(2,RING_SEP) ≥ 10·value(2,d=1)`.
8. `zero_source_room_scores_zero` — with a note that the viable gate is the primary exclusion.
9. `plan_zero_does_not_hard_zero` — `value(2,d=4,plan=Some(0.0)) > 0` (floored *quality*); `plan=None` ⇒ neutral
   1.0 (D7 soft role). The D7 **hard** gate (no valid plan ⇒ no mission) is a pipeline invariant guarded by the
   existing viable/commit gates (KEEP), not this kernel.
10. `claim_value_is_deterministic` — same inputs ⇒ byte-identical `f64`, `.is_finite()`.
11. `selection_is_total_and_stable` — the **fence test**: a candidate set with a deliberate near-tie (case 6)
    and an exact tie (identical facts, different `RoomName`); shuffle the input Vec, assert the
    `(quantized_desc, room_name_asc)` argmax yields a **fixed** winner (claim-side analogue of
    `sim_is_deterministic_over_rounds`).
12. `ring_sep_tracks_config` — pins `RING_SEP == features default ring_separation_hops` so config + kernel can't
    drift (mirrors `max_claim_radius_is_derived_from_claim_creep_reach`).
13. `support_never_gates_at_reach_ceiling` — `value(1,d=max_claim_radius_hops()) > 0`.

Part A (reach/cadence) is mechanical; guard it with a `run_select`-shaped unit test if practical, else rely on
the existing pipeline + a live post-deploy watch (a boxed/dense empire must create a claim mission).

---

## 6. WORLD_FORMAT_VERSION — **BUMP 22 → 23** (one loud reset)

`ClaimOperation` derives `ConvertSaveload` (`claim.rs:50`) and rides the positional bincode component stream;
removing the serialized `current_search_radius: u32` and `frontier_truncated: bool` (D2) is a struct-shape
change bincode cannot decode from an old payload ⇒ the version fingerprint is the only gate. Bump
`WORLD_FORMAT_VERSION` at `game_loop.rs:672` **22 → 23**, add a `/// 23 = …` history line, and correct the WFV=6
note (`game_loop.rs:595-597`) that describes the now-removed fields — a clean reset per the reset-anytime
policy. **No bump** for: `ClaimFeatures` (`#[serde(default)]`, lives in `Memory._features`, not the world save);
the `claim_economics` scoring (per-cycle transient, like `EconomicIntel`/the EV matrix — `0032:108-109,242-243`);
`gather.rs frontier_truncated` (transient scan struct); the `CandidateSubScores` viz reshape (not world-serialized).

---

## 7. Interactions

- **ADR 0032 (the kernel owner).** `room_net_roi`/`RoomEconomyFacts`/`HoldModel` are the sanctioned shared
  substrate; 0032 explicitly pre-committed the claim consumer (`0032:17,79,82-83`). ADR 0038 adds a **second bot
  adapter** (claim-side) + a new *composition* kernel (`claim_economics`) + one convenience ctor
  (`owned_colony`) — it does **not** fork the kernel, add a currency, or touch `room_net_roi`'s body (which would
  move war.rs's numbers; war.rs call sites + `room_economics` tests must stay green). New distance curves live in
  the adapter-side pure kernel, not in `room_net_roi`.
- **ADR 0017 (threat-aware expansion).** Untouched and orthogonal: the safety gate + commit-time re-check +
  claimer-death abort + avoid-cooldown all remain; ADR 0038 changes *which room* is preferred and *whether a
  close room is hard-blocked*, not *whether a contested room is safe*.
- **ADR 0014 (CPU room cap).** `compute_maximum_rooms` and the governor veto are unchanged — they cap *how many*
  rooms; ADR 0038 changes *which* and removes the artificial *reach* cap.
- **ADR 0018 / SK-farm scorer.** `room_net_roi` generalises the SK net model (`room_economics.rs:6-8`); the SK
  scorer keeps its own `TILES_PER_ROOM`/haul copy. No change — a shared-kernel precedent, not a dependency.
- **`[[sim-determinism-fence]]`.** D8's quantize + `RoomName` tie-break + no-`HashMap`-on-the-path is the direct
  application; test #11 is the fence's claim-side guard.

---

## 8. Cross-references
- [ADR 0032](0032-ev-optimal-squad-assignment.md) — the `room_net_roi` kernel + its pre-committed claim reuse.
- [ADR 0017](0017-threat-aware-expansion-lifecycle.md) — the claim safety/lifecycle gate (unchanged).
- [ADR 0014](0014-empire-strategy-and-posture.md) — the CPU room cap (unchanged).
- [ADR 0018](0018-source-keeper-room-exploitation.md) — the SK net model `room_net_roi` generalises.
- Kernel: `screeps-ibex/src/room_economics.rs` (`room_net_roi:151`, `reservable_remote:112`, `HoldModel:73`,
  `TILES_PER_ROOM:65`, engine consts `:31-65`).
- Claim pipeline (verified 2026-07-01): `operations/claim.rs` — scoring `:155-262`, radius `:278-283`, floor/
  escape `:655-665`, skip `:769-778`, ratchet `:913-935`, feasibility gate `:868`, sort `:626`, viable gate `:147`.
- Reach: `missions/utility.rs:91-103` (`is_claim_feasible`, `max_claim_radius_hops=11`). Remote ring:
  `operations/miningoutpost.rs:110` (radius 1). Config: `features.rs:382-496`. WFV: `game_loop.rs:595-597,672`.
- war.rs adapter template: `operations/war.rs:1148-1188` (fact-gather + hops→tiles), `:1907-1956`
  (`economic_rank_score`, continuous-key discipline).
```
