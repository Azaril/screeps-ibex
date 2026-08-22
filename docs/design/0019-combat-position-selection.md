# ADR 0019 — Unified Combat Position-Selection (objective-aware optimal placement)

- **Status:** Decided
- **Owner:** combat-AI
- **Keyed to:** [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) U-roadmap; design lineage [ADR 0008](0008-combat-and-squad-architecture.md) (+ [0008a](0008a-combat-tactics.md))
- **Provenance:** produced by an 11-agent ultracode design deep-dive (6 explorations → synthesis → 3 adversarial reviews → finalized spec), every claim verified against the tree (file:line). Operator question that prompted it: *should flee / stand-ground / close-distance be generalized into one objective-aware optimal-position decision that accounts for composition+health, target goal, pathfinding cost, damage potential from other creeps, and future positions via flood-fill?*

> This document is the implementer's contract. It **supersedes** the proposed synthesized spec.

## 0. Decision in one paragraph

**Adopt a justified HYBRID, not full unification.** Ship two shared maps and a single signed per-tile utility `U` that makes flee/stand/close emerge as the argmax under objective-selected weights — that core is sound, CPU-positive, and prior-art-backed. But **keep five hard guards, not three**, because three of the discrete heuristics the original spec dissolves into weights are not safely expressible as continuous weights (they encode survival-horizon, mobility-prediction, and fixed-point stability that a single-tick weighted sum cannot). **All threat math is integer (fixed-point hits), not f32** — this single decision kills both the live/sim parity flake and the rounding-tie oscillation two reviewers independently flagged as blocking. The first shippable increment (Stage 1) is a pure, byte-parity-locked refactor with **no behavior change** and a hard prerequisite (`tower_dps_at_range` deletion proven bit-identical) — it derisks everything downstream and the operator can approve it in isolation.

Verified against the tree: the engine-delegation edge is genuinely new (combat-decision deps are only `screeps-game-api`+`screeps-rover`), the rover flood has one global edge cost with first-wins tie-break, the two tower curves are duplicated, and `SquadState` is serialized and distinct from the pure enums (`WORLD_FORMAT_VERSION=13`).

## 1. The recommended approach — HYBRID (one utility + five guards)

### 1.1 Why hybrid, not full unification

Full unification claims flee/stand/close are *only* corners of one weight simplex. That is true for the **reward / safety / cohesion / openness / proximity** terms — and those genuinely should unify. It is **false** for three behaviors that encode information a single-tick weighted argmax structurally cannot represent:

1. **Survival over a time-to-kill** (not one tick) — a weight can be out-voted by a large `DamageDealt`; survival cannot be a vote.
2. **"Will this enemy actually chase me"** — a mobility/leash predicate, not a tile score.
3. **Fixed-point stability of the no-threat case** — `Hold` is a true fixed point; argmax-of-`U` with a centroid-dependent cohesion term is a feedback loop that can limit-cycle.

So: **one utility function** for position *preference*, wrapped by **five hard vetoes/short-circuits** for the three things weights can't encode plus the two already-agreed guards.

### 1.2 The signed per-tile utility (integer hits, fixed-point everywhere)

```
U_{O,θ}(tile) =
    + w_dmg   * DamageDealt(tile)      // reward: weapon output onto focus/targets (integer hits/tick)
    - w_taken * DamageTaken(tile)      // penalty: threat field net (integer hits/tick)
    - w_cost  * ReachCost(tile)        // penalty: g-cost centroid->tile (free from flood)
    - w_prox  * ProximityToGoal(tile)  // penalty: max(0, range - r*), closing pull
    - w_safe  * ThreatExposure(tile)   // penalty: present "inside reach" + future TTT (reachability)
    - w_coh   * Cohesion(tile)         // penalty: g-cost (Stage 2+) / Chebyshev (Stage 1) to centroid
    - w_open  * DeadEnd(tile)          // penalty: 8 - walkable_neighbors
    - w_edge  * EdgeTrap(tile)         // penalty: edge proximity while threatened (invertible, ss-F2)
```

**Dimensional normalization (RESOLVES correctness-C2 — blocking).** The original preset table is dimensionally incoherent (`DamageTaken` is hundreds-to-thousands of hits; `Proximity`/`Cohesion` are single-digit tiles). **Every term is normalized to a common fixed-point `[0, SCALE]` band before weighting** (`SCALE = 1000`, integer):

| Term | Normalized form (integer) |
|---|---|
| `DamageDealt` | `min(SCALE, dealt_hits * SCALE / theta.ref_output)` — `ref_output` = this body's max single-tick output |
| `DamageTaken` | `pressure = min(SCALE, net_hits * SCALE / fragile_member_hits)` — body-invariant form (§2.1) |
| `ReachCost` | `min(SCALE, g * SCALE / MAX_G)` |
| `ProximityToGoal` | `max(0, cheb(tile,goal) - r*) * SCALE / ROOM_DIAM` |
| `Cohesion` | steepened-Chebyshev (Stage 1) / g-cost (Stage 2) `* SCALE / ROOM_DIAM` |
| `DeadEnd` | `(8 - walkable) * SCALE / 8` |
| `EdgeTrap` | `max(0, EDGE_THRESH - dist) * SCALE / EDGE_THRESH`, only when a threat is within reach |
| `ThreatExposure` | present-reach + `sum decay(ttt)*threat`, each normalized to `[0,SCALE]` |

Now a weight is a pure dimensionless mixing ratio. The whole pipeline (field sums, normalization, weighting, final `U`) is **`i32`**; the rover search already minimizes an `i64` cost, so we feed `U_MAX - U` directly with **zero float and zero `.round()`** (RESOLVES correctness-E and architecture-MF3: byte-identical goals become achievable by construction, not by hoping f32 sums in the same order live and sim).

### 1.3 Objective presets (dimensionless mixing ratios over normalized terms)

| weight | Retreat | Engage | Breach/Defend |
|---|---|---|---|
| `w_dmg`   | 0.2 | **3.0** | 1.0 |
| `w_taken` | **2.0** | 1.5 | 0.8 |
| `w_cost`  | 0.5 | 0.3 | **1.5** |
| `w_prox`  | 0 | 1.0 | **4.0** |
| `w_safe`  | **8.0** | 2.0 | 1.0 |
| `w_coh`   | 1.5 | 1.0 | 1.0 |
| `w_open`  | 2.0 | 1.0 | 0.5 |
| `w_edge`  | **3.0** | 1.0 | 0.5 |

Weights stored fixed-point (`x256`). These are *seeds*; the only sanctioned tuner is the EXP-* sim loop (ADR 0008a), never hand-fit to one opponent. Flee/stand/close emerge as the original spec argued — now over comparable scales, so the "continuous morph" story is actually true.

**Proximity IS the advance-to-damage layer (engage `w_prox` is dominant, ≈1.5).** The engage search is a *bounded* flood, so with a weak proximity weight it stalls short of a far focus. Rather than branch "approach vs position", proximity carries the march: it is 0 inside `r*`, so ONE search both marches to the focus and picks the engage tile (the survival veto still forbids a lethal march). Proximity distance is **Chebyshev** — Screeps charges equal diagonal and cardinal move cost, so true distance *is* Chebyshev. Its flat square iso-range plateaus would let the deterministic tie-break drift the block to a ring corner or off a corridor mouth, so they are broken by a small **perpendicular-deviation tie-break** from the centroid→focus line (capped and low-weighted, so the block beelines along the real approach line without distorting the movement-correct cross-ring ordering; a euclidean magnitude here is wrong — it mis-prices diagonals).

**`FUTURE_HORIZON = 3`** (not 5): a longer horizon makes the kiter flee toward the horizon instead of HOLDING `r*`; 3 produces durable standoff.

**Weapon range `r*` is parameterized** (3 ranged / 1 melee), so the same scorer serves ranged standoff and melee/siege range-1 engagement.

**The DMG reward is the net hits the squad would actually land on the focus**, not a flat "focus is in range" bonus: a convex blend of EFFECTIVENESS (net / max-output — pulls a melee block to range 1) and KILL-PRIORITY (net / focus hits — commit to a near-dead focus), and **0 when the focus is out-healed** (so safety disengages from an unkillable target). This needs melee/ranged power on `SquadMemberView` and focus hits + nearby enemy heal on `FocusDamage`; absent that data the term degrades to the flat in-`r*` reward.

### 1.4 θ (composition + health) parameterizes, never selects objective

`θ` sets `r*` (standoff: 3 ranged, 1 brick/dismantler), the damage curve, and perturbs weights from HP/heal. **`dmg_at` must be a parameterized curve, not a bare `fn(u32)->f32`** (RESOLVES correctness-H): use `enum DamageCurve { RangedFalloff{per_part_hits,parts,boost}, MeleeStep{...}, Mixed(Box<…>) }` with an integer `output_at(range)->i32`. A bare fn-pointer cannot capture per-creep boost/part counts and cannot sum two curves for mixed bodies.

```rust
// integer perturbation, fixed-point x256
let hp_scale = 256 + 2*(256 - hp_frac_q8);              // low HP -> safety up, smooth slide
w.w_safe  = w.w_safe  * hp_scale / 256;
w.w_taken = w.w_taken * hp_scale / 256;
let tol = 256 + (heal_q8 * 128 / max(1, incoming_q8));  // HEAL -> threat tolerance up
w.w_taken = w.w_taken * 256 / tol;
```

### 1.5 The FIVE guards (hard, around the score — RESOLVES correctness-G)

```rust
// Ordering is load-bearing: HP-flee BEFORE cohesion clamp (the sanctioned cohesion break).
// GUARD 1 - Critical-HP raw flee (vote can't encode "must not die"):
if hp_frac < CRITICAL_HP_FRACTION && hostile_within_3 {
    return CombatIntent::Flee { from: hostiles_le_3, range: 3 };   // Flee stays a live intent (s3)
}
// GUARD 2 - Cohesion rejoin clamp (score discourages, clamp forbids permanent self-eviction):
if cheb(member, centroid) > SQUAD_COHESION_RADIUS {
    return CombatIntent::MoveTo { goal: centroid, range: K };
}
// GUARD 3 - Trivial-geometry / no-threat short-circuit (NEW; load-bearing CPU gate, perf MF-7):
//   Subsumes the deleted should_kite. MUST fire BEFORE the maps are sampled or the flood runs.
if hostiles.is_empty() || (focus_in_range(r*) && no_threat_within_horizon(centroid)) {
    return advance_or_hold(focus, r*);   // zero search
}
// GUARD 4 - Survival-horizon veto (NEW: promoted from optional; weights can't model time-to-kill):
//   The chosen goal must be survivable for the most-fragile member, full TOUGH+heal model:
//   reject any goal where net_at(goal, fragile_body) > fragile_hits / H_MIN   (H_MIN ~= 3)
//   Applied as a hard reject inside plan_position's candidate acceptance, re-scoring to next-best.
// GUARD 5 - Reachability seed filter (NEW: a tile-score can't represent "won't chase"):
//   R is seeded ONLY by enemies that can_move (MOVE parts, not fatigued) AND are plausible chasers
//   (offensive parts, not a leashed NPC). Immobile/fatigued/non-chaser enemies -> T only, never R.

// Otherwise: one bounded scored search. Objective from squad state + focus kind:
let obj = match (state, focus) {
    (Retreating, _)                      => Objective::Retreat,
    (Engaged, Some(f)) if f.is_structure => Objective::Breach,
    (Engaged, Some(_))                   => Objective::Engage,
    (Holding, _) if defending            => Objective::Defend,
    _                                    => Objective::Engage,
};
```

The hysteresis state machine (`SquadOrderState` + `re_engage_band`) stays as the **source of the objective + the HP→weight scale + the stickiness ε** — it no longer selects a movement code path.

### 1.6 Per-block, not per-creep (RESOLVES correctness-B — the biggest internal contradiction)

Scoring is strictly **per-block** (one search per member-block from the centroid; members consume the goal via the existing cheap per-creep move — no re-search, no scatter):

- **Safety terms (`DamageTaken`, `ThreatExposure`, Guard 4)** use **θ of the most-fragile member** (lowest `hits`). The block is as cautious as its weakest creep.
- **Reward terms (`DamageDealt`, `r*`)** use a **block-representative role** (the dominant offensive role's curve).
- **Drop the "same weights, body-invariant, per-creep auto-avoid" claim** — it described the rejected per-creep design.

**DEFER — per-role sub-block searches** (ranged sub-block at `r*=3`, melee sub-block at `r*=1` scored separately). Reason: right long-term answer to mixed-formation scatter, but multiplies search cost by roles and reintroduces sub-block coordination. Post-Stage-3, gated on a measured EXP-* need; single-block-with-fragile-θ is the shippable default, formation-splitting mitigated (not solved) by cohesion + Guard 4.

## 2. The cached layer set — compute-once-per-(room,tick), combined per objective

**Architecture (operator, 2026-06-19): each term is a cached per-tick LAYER, not an inline computation.** The score is `U = Σ wᵢ · Lᵢ(tile)` where each `Lᵢ` is a per-tile field built **at most once per (room, tick)** and cached behind a `(room, tick, matrix_fingerprint)` key. **Different objectives — and different *uses* (kite vs attack-positioning vs defend) — are just different weight vectors over the SAME cached layers.** A `PositionLayers` struct owns them; the kite scorer and the attack-positioning scorer (T-POS, Stage 3) borrow the same instance, so the expensive floods/stamps are computed once and *reused across uses*, never rebuilt per consumer. This is the resolution to the redundancy flagged in Stage 2-tail (the centroid cohesion flood currently re-floods what a sibling search already computed): promote it to a layer, build once, everyone reads it.

The layers, and which uses read each (✓ = weighted in, — = weight 0):

| Layer | Built by | Kite/Retreat | Engage/Attack-pos | Defend/Breach |
|---|---|---|---|---|
| `threat` T (incoming hits, integer; creep + tower stamps) | stamp pass | ✓ safety | ✓ damage-taken | ✓ |
| `reachability` R (soonest a chaser reaches, ticks) | `reachability_from` (multi-source) | ✓ future-threat | ✓ when does the enemy close | ✓ |
| `centroid_dist` C (wall-aware path tiles from the squad) | `reachability_from` (centroid src) | ✓ cohesion | ✓ cohesion | ✓ cohesion |
| `focus_damage` D (hits we'd DEAL to the focus from here) | focus-stamp / curve | — | ✓ reward | ✓ (vs structure) |
| `walkable`/`openness` O (dead-end avoidance) | terrain, cached room-life | ✓ | ✓ | ✓ |

`threat`/`reachability`/`centroid_dist`/`openness` are **use-agnostic** — built once, shared by every consumer this tick. `focus_damage` is the only **use-specific** layer (attack/breach need it; retreat weights it 0), and is itself cacheable per (focus, tick). The kite path (Stage 2) already builds the R and C layers ad-hoc inside `plan_kite_anchor`; Stage 3 hoists them into `PositionLayers` so the attack-positioning path reuses them with no extra flood.

Both core fields are `[i32; 2500]` over the 50x50 room, decider-agnostic at build — the single-build guard checked **above** the per-block loop, never lazily per block (RESOLVES perf-MF-4a / architecture-MF3).

**Flood dedup — cohesion rides the search's own `g`.** A separate centroid flood is redundant with the scored search that is already running: `search_scored` threads the settled path-cost `g` into its cost function, so the cohesion term reads that `g` (= wall-aware distance from the centroid) directly. `PositionLayers` therefore keeps only the shared *threat* flood, and the room drops from three floods to two per block-tick (~15% of the compound-worst-case bench) with byte-identical plans.

**Build once per ROOM, not per squad.** The threat field and the floods depend only on the room's enemies, never on which squad asks, so the caller may hand the planner a shared `Option<&PositionLayers>`: the live manager holds a per-tick `RoomName → (LocalCostMatrix, PositionLayers)` map and builds each target room's layers once, sharing them across every squad engaging there. Passing `None` builds them inline (the unchanged single-squad path). One `build_target_matrix()` is extracted so the layer build and the scored search share a single matrix — this is the perf payoff the layer architecture exists for.

### 2.1 Threat field `T[2500]` — integer hits, "damage if I stand here NOW"

`T[tile]` = raw (pre-mitigation, pre-self-heal) incoming **integer hits/tick** summed over every enemy at its current position. **`i32`, never f32** (powers are integer constants -> exact, order-independent; enemies still stamped in **sorted-by-id order** on both sides as a belt-and-braces against any future fractional term).

**Build = STAMP (scatter):** melee 3x3, ranged 7x7 precomputed flat kernels. **Towers STAMPED into `T`, not lazy** (RESOLVES perf-MF-6): a `tower_stamp[r]` LUT applied once per tower (<=6 x 2500 = 15k adds) shared by all blocks; the lazy per-priced-tile add re-pays `max_ops*towers*B` and shares nothing when B>1.

**Query-time per-decider conversion** (the only decider-dependent part, integer):
```
mitigated = engine_body.damage_after_tough(T[tile])          // exact engine TOUGH reducer
net[tile] = max(0, mitigated - engine_body.heal_power())     // self-sustain
pressure  = min(SCALE, net[tile] * SCALE / fragile_hits)     // normalized safety term (s1.2)
```

**One source of truth — delete `kite::tower_dps_at_range` (kite.rs:83), delegate to `damage.rs::tower_attack_damage_at_range`** (verified duplicated; engine version routes through `tower_amount_at_range`). Stage 1 gates the deletion on a proven bit-identical assertion (§5).

### 2.2 Reachability map `R[2500]` — integer ticks, "how SOON can a chaser threaten here"

`SOONEST(tile) = min over CHASER enemies e of TTR_e(tile)`. Seeds filtered by Guard 5 (mobile + plausible chaser only) — RESOLVES correctness-D1 (decoy-herding). `SpeedProfile` may be "infinite / not a source."

- **Per-step cost = exact engine fatigue cadence** (`ticks_per_step(rate) = max(1, ceil(W*rate / 2M))`), 2-3-entry LUT per distinct profile.
- **Multi-source Dijkstra**, all chasers seeded `g=0`, `src[tile]` carries the owning wave, per-source edge pricing. **This is a NEW rover search, not a thin generalization** (RESOLVES architecture-MF2): the existing `run`/`room_grid_dijkstra` have a single global edge cost (local_pathfinder.rs:187). New `reachability_from(sources: &[(Position, SpeedProfile)], cost: Fn(src_idx,x,y)->Option<u32>)` does `src[]` bookkeeping. Re-scoped **M-to-L**.
- **Range-awareness:** exact **separable two-pass 7x7 min-filter** dilation (~35k ops) — not the naive 49x2500 = 122k, and not the `-3*step` approximation (RESOLVES perf-MF-4c + correctness-D3: the approximation is a parity hazard, diverges on mixed terrain).

**Folding into the score — relative-mobility future term (RESOLVES correctness-D2, the corner-kiting bug):**
```
future_threat(tile) = decay(ttt(tile)) * threat_of(src_enemy)
// decay only penalizes a tile the enemy reaches BEFORE you can re-open the standoff gap:
decay(k) = SCALE        if k == 0
         = gamma^k * SCALE   for 1 <= k <= HORIZON  AND  k < your_time_to_reopen_gap
         = 0             otherwise
```
A raw `gamma^ttt` makes the kiter retreat faster than the chaser advances and back into a corner; gating the future penalty on "the enemy reaches it before you can re-establish standoff" keeps a fixed standoff instead of accelerating retreat.

**Cost:** ~20-28k relaxations once per room (independent of N), HORIZON-capped, shared across all blocks (≈ one pathfinder call per *room* per tick).

## 3. Crate placement + live/sim parity

| Piece | Crate / file | Notes |
|---|---|---|
| **SEARCH** (scored flood + **new `reachability_from`**) | `screeps-rover::LocalPathfinder` | no-one-off rule. `reachability_from` is a first-class new search (per-source edge pricing), NOT a thin generalization (architecture-MF2). |
| **SCORE / pricing / presets** | `screeps-combat-decision::position` (new module from `kite.rs`) | pure, integer, no `game::*`. |
| **Threat/reach field build** | `screeps-combat-decision`, math delegated to `screeps-combat-engine::{damage,body}` | **NEW dependency edge** `combat-decision -> combat-engine` (RESOLVES architecture-MF1: absent today; engine is a leaf -> no cycle). |
| **Body conversion** | new `CombatBodyPart { part, hits, boost }` -> engine `Body` at the field boundary | **RESOLVES architecture-MF1 (blocking):** the DTO currently lacks `boost`, so `damage_after_tough`/`heal_power` cannot be called and TOUGH mitigation is silently dropped. DTO is a live-rebuilt view (not serialized) -> adding `boost` is a free reshape. Build an engine `Body` from the DTO at the boundary; do NOT call engine methods on the DTO type. |
| **Live adapter** | `jobs/squad_combat.rs` | emits existing `CombatIntent::{MoveTo,Flee}` -> zero adapter change. |

**Parity invariant (now mechanically enforceable):**
1. **All-integer score pipeline** -> live and sim produce byte-identical goals by construction (no f32 sum-order dependence). Makes the U6 "byte-identical goal" gate achievable rather than flaky.
2. **Maps built from the same matrix the search floods** — borrowed input, never re-fetched; **cache key includes a matrix fingerprint** so a mid-tick structure change can't desync the cached field from a later flood (architecture-MF3).
3. **Deterministic argmax tie-break** (RESOLVES correctness-A2): prefer `(a) == last_goal`, then lower reach-cost `g`, then a stable `(x,y)` key — never rely on heap pop order.

**Serialized-state boundary (RESOLVES architecture-MF4 — verified):** `SquadOrderState`/`SquadMovement` (lib.rs:590/605) are pure and recomputed each tick -> collapsing `SquadMovement` to `MoveTo/Hold` is **WORLD_FORMAT_VERSION-neutral**. The live `military::squad::SquadState` (squad.rs:14) is **`Serialize`/`Deserialize`, persisted, distinct, and MUST NOT be touched/merged/reordered**. The two enums look near-identical, which makes an accidental merge tempting and dangerous. **No `WORLD_FORMAT_VERSION` bump (stays 13).**

## 4. Anti-oscillation: a deterministic tie-break, not per-tile stickiness (RESOLVES correctness-A1/A3)

The `w_stick*(tile != last_goal)` term is **dropped** — it compares against a goal the drifting squad never reached (a moving carrot).

**Goal latching in the driver (commit to a goal for `N` ticks) was considered and REJECTED.** Disabling it left every kiting/cohesion scenario passing unchanged (no oscillation appeared without it), it saved no CPU (the full search still runs each tick), and its stickiness introduced a real bug: it pinned an advance short of the focus. Latching also needs persisted last-goal state that the design otherwise does not require.

The kept oscillation guard is the **deterministic argmax tie-break** (§3): the search heap is a total order on `(g, x, y)` and `best` updates only on a strict improvement, so ties resolve to the closest-to-origin tile — no RNG, no path-dependence, and therefore no tick-to-tick flip between equal-scoring tiles. The proximity plateau tie-break (§1.3) is the same mechanism applied to the flat iso-range bands.

Fire the trivial-geometry short-circuit (Guard 3) **before** centroid recomputation — the no-threat case stays a true fixed point.

## 5. Staged, measurable plan (keyed to U-roadmap; every stage flagged + abortable)

The stages are the shape of the build. Each carries its own gate **and its own kill-switch flag**, whose OFF path is the previous stage's behaviour verbatim — the flags are named with their stages below, and flipping one is rung 1 of the abort ladder. The harness (byte-parity + behaviour + CPU-bench gates below) is the acceptance mechanism, and while the work is not being deployed harness validation is what a stage is accepted on; the live flag is what wraps it near deploy. Under runtime pressure it is the abort ladder, not the flag, that degrades the system.

### Stage 0 (prerequisite commit — gates the kite delete, ~hours)
Add an **exhaustive `range in 0..=49` equality assertion** that `kite::tower_dps_at_range` == `damage::tower_attack_damage_at_range`. Must pass **before** Stage 1 deletes the duplicate. If they differ, reconcile to the engine curve first.

### Stage 1 — shared INTEGER threat field, pure refactor, NO behavior change (S, parity-locked)
Extract `score_tile`'s SAFETY+OPENNESS into `ThreatField::build(view, &matrix) -> ThreatField` (creep stamps within footprint; **towers stamped via the engine LUT**; `walkable[2500]`), **all `i32`**. Delete `tower_dps_at_range`. Change `plan_kite_anchor`'s closure to read the field. Add the **new `combat-decision -> combat-engine` dep + `boost` on `CombatBodyPart`** here. Add the **trivial-geometry short-circuit (Guard 3)** as a real fast-path. **No rover change, no reachability flood, no new score terms** — cohesion/value stay exactly as today.
- **Kill-switch:** `features.combat.shared_threat_field` (OFF -> today's `score_tile` verbatim).
- **Parity gate:** byte-identical `Kite{goal}` on EXP-KITE-1 / EXP-BREACH-1 / EXP-NEST-1 (host assert).
- **CPU gate:** per-room field-build counter to seg-57; saved recompute `K*(E+T)` must exceed build cost.
- **Static-map cache (RESOLVES perf Refutation-2):** the walkable/openness map is **terrain-derived -> cached for the room's life, rebuilt only on structure-destruction dirty-flag**, from Stage 1.
- **Deliberately NOT in Stage 1 (rationale):** the eager integer `ThreatField` *precompute* and the f32→i32 conversion of the whole score. Precomputing 2500 tiles to serve a single ~400-op kite search is a CPU *regression* until reachability and multi-block sharing amortize it, and the f32→i32 rewrite risks the byte-identity gate for zero Stage-1 gain. Likewise the boosted-TOUGH net conversion: the `boost` field lands here, but *using* it needs the field to carry actual hits (Stage 3). Each lands with the stage that exercises it.

### Stage 2 — reachability flood in rover + scratch-buffer reuse (M-to-L, U9 cohesion)
Add `LocalPathfinder::reachability_from` (per-source edge pricing, `src[]` bookkeeping — a NEW search). Apply **Guard 5 seed filter**. Cohesion switches Chebyshev->true g-cost; unreachable tiles get a hard penalty. **Land scratch-buffer reuse here (perf-MF-1):** hoist `g`/`came`/`snapshot` into reusable pathfinder-owned scratch, **version-stamped `g`** to avoid the ~20 KB zero-fill churn per search. **Eliminate the double matrix materialization** (perf-MF-2): snapshot once, build all maps + flood from that one grid.
- **Kill-switch:** `features.combat.reachability_cohesion` (OFF -> Stage-1 Chebyshev). Under CPU pressure, **R is the first thing dropped** -> T-only score (a separate named kill-path — perf-MF-4b).
- **Gate:** U5 cohesion fraction + U6 outcome hold/improve on a new walled-corridor EXP-COHESION.
- The future-threat term this flood feeds is what makes the kiter prefer a **durable** standoff rather than merely a safe-this-tick tile: `KiteThreat.step_ticks` carries each chaser's cadence from its body, only mobile chasers seed the flood (Guard 5), and `w_future` weights the resulting decay. With no reachability data the term is absent and the score is the Stage-1 score exactly.

### Stage 3 — `PositionLayers` cache + unified signed utility + offensive positioning (M, T-POS/U-TOWER) — DEFAULT-ON gated on the correctness must-fixes
**Stage 3a — the layer cache (operator architecture):** introduce `PositionLayers` (§2) — hoist the threat field, reachability R, centroid-distance C, and openness into one per-(room,tick) cached struct; build the integer `ThreatField` as the first layer here (this is where the deferred precompute lands, now justified — it's amortized across uses, not a single search). Refactor `plan_kite_anchor` to consume `PositionLayers` instead of building R/C ad-hoc (removes the duplicate centroid flood). **Stage 3b — the unified utility:** `position_utility` (signed, **normalized integer terms** over the layers, objective presets, θ perturbation, future-threat with relative-mobility, **Guard 4 survival veto mandatory**). `plan_engage_anchor` (attack-positioning, T-POS) reuses `search_scored` over the **same** `PositionLayers` — different weights, zero extra layer builds (the operator's point). **Bound `DamageDealt` to focus + <=4 nearest heal targets** or precompute it as the `focus_damage` layer (perf-MF-5).

**The live engage branch.** `decide_squad_with_pathing` — the one decision the live manager calls — carries an ENGAGE branch: a ranged squad that is engaged with a creep focus and not kiting runs the scored search under `engage()` weights and takes `Advance{range:0}` onto the EV-optimal tile (or Hold) instead of a naive straight-line advance. It is **mutually exclusive with the kite branch** (one scored search either way); flee and stand differ only by the preset. Melee/siege keep the range-1 advance and breach keeps the breach advance, so the search is scoped to the case where standoff geometry actually matters.

- **Kill-switch:** `features.combat.engage_positioning` (OFF -> non-searching `Advance{goal,range=r*}`).
- **Tick-global combat-search op budget (perf-MF-3, blocking):** a hard ceiling on `B*max_ops` carried in the ADR-0004 CPU context; once exhausted, remaining blocks fall to non-search `Advance`/`Hold`. Per-stage `max_ops` bounds one search; nothing bounds B today — the death-spiral shape.
- **CPU gate (perf-MF-8, blocking):** gate on a **measured tick cost** from a compound-worst-case sim bench — large open room, 6 towers, 5 melee + 5 ranged enemies, ~4 converging blocks — **on the sim, before live**. The EXP-* parity scenarios are single-block and never exercise the `B*search` term that is the actual risk. The bench is a **standing** gate with a generous per-block-tick budget (5 ms, chosen to be non-flaky across debug and release) covering BOTH the per-squad and the build-once-per-room paths, plus a behaviour-preserving check that the plans are identical.
- **Behavior gate:** U5 DPS/efficiency + U6 outcome improve on EXP-FOCUS/box-fight; self-play catches over-fit.
- **Default-ON requires the correctness must-fixes**, and their *coupling* is the load-bearing part: **#1 integer field, #2 per-block fragile-θ, #3 normalized terms and #4 the survival veto are ONE change** — together they are the signed, normalized, integer utility over actual hits, and doing any of them against the reach-depth proxy would mean arbitrary normalization references and a body-agnostic veto, both redone later. **#5 the chaser seed filter** (skip harmless creeps; filter immobile ones out of `threat_step_ticks`, so only mobile attack-capable chasers seed the flood; the leashed-NPC nuance is a refinement of the same predicate) and **#6 the deterministic tie-break** (§4) are independent and land wherever they are cheapest.

### Stage 4 — incremental creep-stamp field (S-M, MEASURE-FIRST only)
Rebuild only the creep-threat stamp each tick if the Stage-1 counter demands it. Gated on measurement, not assumed.

### The standing weight-tuning loop
Weights are seeds (§1.3), so the design carries its own tuner and keeps it as a permanent regression guard rather than a one-off exercise:

- **A tunable seam.** `decide_squad_with_pathing` takes a `SquadTacticParams { kite, engage }`; `Default` *is* the shipped preset set, so the sweep can explore weights without touching live behaviour (the live path and every test pass `Default`; the sim squad injects a variant).
- **Managed-squad EXP beds.** A `run_managed` runner steps ≥2 managed squads head-to-head and drives towers, so the scenarios exercise *this* utility rather than the per-creep engagement path most of the register uses: **EXP-POS-SELFPLAY-1** (two managed ranged squads close and fight cohesively) and **EXP-POS-KITE-1** (a managed ranged squad kites and kills a melee squad).
- **The sweep as a guard.** A sweep over the kite preset's `w_future`/`w_prox`, scored by the continuous net-HP exchange, trips if a scorer change leaves the default dominated.
- **What the sweep taught (design knowledge, not just a run):** on the melee beds (open / under-MOVE'd / healing-bruiser) the response is **FLAT** — the outcome is invariant to these weights, i.e. the utility is not brittle to weight choice in a melee standoff, because positioning there shifts tiles rather than who bleeds. Real tuning signal needs a **weight-discriminating bed** (terrain or tower pressure, where damage-taken is continuous in position). Separately, self-play between two ranged squads is **low-casualty** — they reposition instead of sustaining the range-3 trade — which points at engage-stickiness as the lever, not the safety weights.

### Abort/fallback ladder (strict, budget-driven, cheapest last)
1. Per-stage kill-switch -> prior stage's behavior.
2. Tick-global op budget exhausted (perf-MF-3) -> non-search `Advance`/`Hold`.
3. CPU-Critical (ADR-0004) -> drop R flood first (T-only), then last-tick stale goal, then `Advance`/`Hold`.
4. Bounded-search abort: best-so-far on `max_ops`; `None` => Hold centroid.
5. Cornered/all-unsafe -> Guard 1 critical-HP raw-flee still fires; multi-room flee is the separate L1 phase.
6. Parity backstop: U6 self-play / U5 oracle + seg-57 canary fail the nightly gate on any field drift.

## 6. Honest tradeoffs

- **CPU is the real risk, and the original spec hid four multipliers** (all now bounded): `B*max_ops` searches (Stage-3 op budget + bench gate), the O(targets) `DamageDealt` loop (capped to <=5), the per-search ~20 KB alloc churn (version-stamped scratch, Stage 2), the 122k naive dilation (separable two-pass). The maps-shared core is flat-in-E and CPU-positive; the *system* is only affordable with the bounds above. The operator's recorded CPU-death-spiral failure is why Stage 3 default-ON is gated on a measured compound-worst-case bench, not outcome metrics alone.
- **Explainability:** a weighted argmax is harder to debug than a branch tree. Mitigation: a `score_breakdown` debug dump of per-term contributions for the chosen tile; EXP-* sim loop as the only sanctioned weight tuner.
- **Integer quantization vs continuity:** fixed-point `SCALE=1000` can coarsen near-ties — but that absorbs noise (deterministic tie-break handles the rest) and is the price of parity. Accepted.
- **Where cheap heuristics stay genuinely better (kept, not regretted):** critical-HP flee, cohesion clamp, survival-horizon veto, non-chaser seed filter, trivial-geometry short-circuit, CPU-starved `Advance`/`Hold`, and **cross-room flee** (single-room scored search can't flee to an adjacent room — stays the separate L1 `MoveToRoom` phase; and `w_edge` must **invert/zero on the resolved cross-room exit tile** so the per-tile utility doesn't fight the L1 planner at the boundary — RESOLVES correctness-F2).

**DEFERRED (with reasons):** (a) per-role sub-block searches — post-Stage-3, gated on measured formation-scatter need; (b) Stage-4 incremental creep-stamp — measure-first; (c) focus-sanity co-design (correctness-F1, gamed-decoy tractor-beam) — **flagged, owned by focus-selection, not this spec**; position unification assumes focus is sane, and the Guard-4 survival veto is the backstop that stops the block diving onto a decoy's kill-zone. Do not ship the `DamageDealt` reward without the focus team adding "don't focus a target whose only approach is through a veto-level threat tile."; (d) the **boosted-TOUGH net-damage conversion** in `ThreatField` — it needs a `boost` field plumbed through the body DTO so the most-fragile member's damage reduction can be applied at the safety term and the veto. Unboosted is the safe-conservative direction (it over-estimates incoming, so the block over-flees a boosted brick rather than dying to one); (e) a **terrain/tower weight-discriminating sweep bed** and the engage-stickiness lever the self-play beds point at.

## 7. First increment (the derisking rationale)

Stage 0 + Stage 1 stand alone (Stage 0 is an hours-long prerequisite; Stage 1 is a small, pure refactor with no behavior change):

> Add the `combat-decision -> combat-engine` dependency + a `boost` field on the view-only `CombatBodyPart`; prove `kite::tower_dps_at_range` bit-identical to the engine curve, then delete it; extract the kite scorer's safety+openness into an **integer** `ThreatField::build` (creep + engine-LUT tower stamps, cached static walkable map, trivial-geometry fast-path); rewire `plan_kite_anchor` to read the field. All of it sits behind `features.combat.shared_threat_field` (default OFF). It is done when it produces **byte-identical `Kite{goal}`** on all three U7 scenarios and the seg-57 field-build counter shows net CPU savings.

That increment is risk-free (a behavior-preserving refactor behind a byte-equality gate and a kill-switch), deletes a real duplication-drift hazard, lands the integer foundation + the engine-delegation edge every later stage reuses, and commits nothing about the unified utility until Stage 3 — which itself stays gated behind the CPU bench and the correctness must-fixes before it goes default-ON.

## Key files (verified)

- `screeps-rover/src/local_pathfinder.rs` — `run` L138 (single global edge cost + first-wins `<` tie-break L173); `search_scored` L212; add `reachability_from`
- `screeps-combat-decision/Cargo.toml` — deps = `screeps-game-api`+`screeps-rover` only (engine edge is new)
- `screeps-combat-decision/src/kite.rs` — `tower_dps_at_range` L83 **delete**; `score_tile` L101; `plan_kite_anchor` L179 -> `position.rs`
- `screeps-combat-decision/src/lib.rs` — `SquadOrderState` L590 / `SquadMovement` L605 (pure, collapse-safe)
- `screeps-combat-engine/src/damage.rs` — `tower_attack_damage_at_range` L35 -> `tower_amount_at_range` L28 (single source of truth)
- `screeps-combat-engine/src/body.rs`
- `screeps-ibex/src/military/squad.rs` — `SquadState` L14 (**serialized, do not touch**)
- `screeps-ibex/src/game_loop.rs` — `WORLD_FORMAT_VERSION=13` (**no bump**)
- `screeps-ibex/src/jobs/squad_combat.rs` — live adapter

## 8. Increment — Healer positioning objective (heal-coverage vs danger)

Operator direction: "pick a location that maximizes potential future
healing, weighting healing output against danger." The squad healer picks its
tile with the SAME unified utility, under a healer objective preset — not an
escort heuristic (healer moves to range 1 of the nearest damaged ally, else the
nearest friendly combatant), which is only ever a stopgap for "healer lags until
it takes damage".

Design (the principled version):
- **New reward term in `score_tile` — heal coverage.** For a candidate tile, sum
  over friendly combatants the healer can REACH (Chebyshev ≤ heal range: 1 `HEAL`,
  3 `RANGED_HEAL`) of their **risk-weighted need** = (in the threat field) ×
  (HP-deficit + a base in-combat weight). Rewards the tile covering the most
  AT-RISK allies = "maximum potential future healing output." Subtract it from the
  score like `w_dmg`, scaled by a new `w_heal`.
- **New healer preset** (`KiteScoreParams` + `SquadTacticParams`): `w_heal` dominant,
  `w_taken` moderate (the danger balance), `w_cohesion` to stay with the block,
  `w_prox`/`w_dmg` ≈ 0 (a healer doesn't advance to deal damage). Hug-the-attacker
  vs back-off emerges from the weights, like kite/engage.
- **Plumbing:** add `allies: &[AllyNeed]` (pos + risk/HP-deficit) to `SquadKiteView`
  (today it carries only `centroid`, not per-ally need); build it in the
  `squad_combat` seam from friendly creeps + the threat field; route healers through
  `decide_movement` with the healer preset (not the slot formation).
- **Test (EXP-HEAL-POS):** healer prefers a tile covering 2 at-risk allies over a
  safer tile covering 0; backs off a high-danger tile when coverage ties.

Scope: a real ADR-0019 increment (view plumbing + term + preset + tests), not a
one-liner. Every struct it touches is transient, so it costs no WFV bump.

### 8.1 The realized mechanism

One principled divergence from the sketch above: the heal-coverage **flood** lives in
`decide_squad_with_pathing` (which holds the cost matrix + the shared `PositionLayers`
threat field + the survival veto), **not** in `decide_movement` — the latter is
terrain-blind (it emits `MoveTo`/`Flee` and lets rover avoid walls), so a tile-by-tile
heal score there could pick a wall. The healer goal is computed by the same scored
search the block uses (no one-off algorithm) and routed per-member.

- **`kite.rs`:** `KiteScoreParams.w_heal` (0 in kite/engage, dominant in the new
  `KiteScoreParams::healer()` preset: `w_heal 2.0`, `w_taken 1.5`, `w_cohesion 0.6`,
  `w_prox/w_dmg 0`); `pub struct AllyNeed { pos, need }`; `SquadKiteView.allies:
  &[AllyNeed]`; the **HEAL-COVERAGE** reward term in `score_tile` — `Σ allies need ×
  heal-efficacy(range)` (full at range ≤1 = HEAL 12/tick, ⅓ at 2–3 = RANGED_HEAL
  4/tick, 0 beyond), normalized by `HEAL_COV_REF` and SUBTRACTED like `w_dmg`. 0 for
  any non-healer search (empty `allies` or `w_heal == 0`) → byte-identical to pre-§8.
- **`lib.rs`:** after the block movement, an engaged, non-kiting squad computes each
  **pure-support** healer's goal via `plan_kite_anchor` over a healer view (allies =
  the other members' `AllyNeed`; `need = HEALER_BASE_NEED + hp_deficit +
  HEALER_RISK_LOOKAHEAD × incoming_damage_at(pos)`), and stores it in the new
  `SquadDecision.member_goals: Vec<Option<Position>>`. `SquadTacticParams.healer` makes
  the preset sweepable through the §5 tuning seam.
- **`squad_manager.rs`:** `apply_squad_decision` stamps a member's `member_goals[i]` as
  its `squad_movement = Advance{goal, range:0}`; everyone else follows the block. Only
  the **anchorless** `decide_movement` path reads `squad_movement`, so this is inert for
  a siege formation (which keeps its healers-back slots) and active exactly for the
  SK/skirmish duo the operator reported.
- **EXP-HEAL-POS:** two `kite.rs` term tests — `heal_coverage_prefers_
  covering_more_at_risk_allies` (covers 2 vs 0, cohesion tied) and `heal_coverage_
  backs_off_danger_when_coverage_ties` (the §8 acceptance, at the scorer level). A full
  managed-sim healer-coverage EXP is an optional follow-up if tuning needs it.
- The seeds (`w_heal`, `HEALER_BASE_NEED`, `HEALER_RISK_LOOKAHEAD`, `HEAL_COV_REF`) are
  tunable through the same `SquadTacticParams` sweep seam as the other presets.

## Landed
- `f96748b` integer `ThreatField` layer + survival veto (2026-06-19)
- `992b191` final normalized unified position utility (2026-06-19)
- `4185aef` build-once-per-room `PositionLayers` sharing (2026-06-20)
- `93c2063` unified utility as the default, CPU bench as the standing gate (2026-06-20)
