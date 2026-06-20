# ADR 0019 — Unified Combat Position-Selection (objective-aware optimal placement)

- **Status:** Accepted; **Stage 0 + Stage 1 core LANDED 2026-06-19** (`54cee73`) — operator approved. Stages 2-4 return for separate approval (each flagged + CPU-gated).
- **Implementation status:** Stage 0 + Stage 1 core + **Stage 2 reachability core DONE** (2026-06-19). **Stage 2 (`1670723` rover + `1838e1a`):** the operator's flood-fill landed as rover **`reachability_from`** (multi-source weighted Dijkstra — soonest any source reaches each tile, per-source cadence, one flood regardless of N) + folded into kite scoring as a **future-threat term** (the kiter prefers durable standoff, not just safe-this-tick; `KiteThreat.step_ticks` cadence from the body, mobile-chaser-only seeds per Guard 5, `w_future` weight, `None`→byte-identical). **Stage 2-tail behavior DONE (`9d8d1f6`):** cohesion→true **g-cost** (wall-aware) via a centroid `reachability_from` flood — a tile reachable only around a wall is correctly "far" (identical to Chebyshev in open terrain; +1 test). **Folded into Stage 3 (judgment, not blocking):** the integer `ThreatField` *precompute*, the `boost` field + TOUGH/heal net conversion, and the perf scratch-buffer reuse + double-matrix-dedup — each is structural/CPU, not behavior: the precompute is a slight CPU *regression* for the current single-block case (its win is multi-block + the unified utility), the net conversion needs the field to be actual-hits (Stage 3), and the perf work is premature while not deploying. They land *with* Stage 3's unified utility + CPU bench, where they're exercised and gated together. A full sim **EXP-COHESION** scenario (vs the unit test) also rides Stage 3 (needs the squad-sim harness).
- **Stage 3 architecture refined (operator, 2026-06-19): terms are cached per-tick LAYERS.** Each score term is a `(room,tick)`-cached field; objectives/uses (kite vs attack-positioning vs defend) are weight vectors over the SAME cached layers, so threat/reachability/centroid-distance are built once and reused across uses (no per-consumer rebuild). A `PositionLayers` struct owns them — see §2 (the layer table) + §5 Stage 3a/3b. This resolves the Stage-2-tail centroid-flood redundancy and is the natural home for the integer `ThreatField` precompute (amortized across uses, not a single search).
- **Stage 3a DONE (`28d6540`):** `PositionLayers` + `::build` (use-agnostic) + `.fields()`; `plan_kite_anchor` builds once (byte-identical). **Stage 3b started:** the **CPU compound-worst-case bench** is built (`8121b81`, `screeps-combat-eval::bench`) — the default-on gate; baseline **~1121 µs/block-tick** (4 blocks × 25 ticks; open room, 6 towers, 10 chasers), surfacing that the floods + matrix churn dominate. **Flood-dedup DONE (`0b38863` + rover `f83b339`):** `search_scored` now threads the settled path-cost `g` into its cost fn, so cohesion reads the kite search's own `g` (= distance-from-centroid) instead of a separate centroid flood; `PositionLayers` keeps only the shared threat flood; byte-identical → **bench 1121 → 950 µs/block-tick (~15%, 3 floods → 2)**. **Flee↔stand unification DONE (`e7a7ad9`):** `score_tile` gained a focus-engagement REWARD (negative cost for a tile in weapon range of the focus) + `KiteScoreParams::engage()` (the same scorer reweighted to stand-and-fight). So *flee* (kite preset, safety dominates) and *stand/close* (engage preset, focus-damage dominates) emerge from ONE scorer purely by the objective weights — the ADR's thesis, demonstrated. `w_focus_dmg` defaults 0 ⇒ kite byte-identical. decision 50. **Live engage positioning DONE (`01823d6`):** `decide_squad_with_pathing` (the live `SquadManager` decision) gained an ENGAGE branch — a ranged squad engaged with a creep focus and not kiting runs the scored search with `engage()` weights → `Advance{range:0}` onto the EV-optimal tile (or Hold), instead of a naive straight-line Advance. Mutually exclusive with the kite branch (one scored search either way); flee vs stand differ only by the preset. Ranged + creep-focus scoped (melee/siege keep range-1 Advance; breach keeps the breach Advance). +1 test. decision 51. This reaches the live bot (the manager already calls this fn). **FINAL NORMALIZED UTILITY DONE (`992b191`):** `score_tile` is the complete unified utility — every term normalized to `[0, SCALE]` (weights = dimensionless ratios; flee/stand emerge by reweighting): TAKEN (the ThreatField's net hits per fragile-HP — **#2/#3**, so ranged threats + towers now shape positioning, which the `reach-0` proxy ignored), FUTURE, COHESION (wall-aware g), PROXIMITY (focus beyond `r*`), OPENNESS, EDGE (anti-corner scored-path), DMG reward. **Weapon-range `r*`** parameterized (3 ranged / 1 melee). `FUTURE_HORIZON` 5→3 so a kiter HOLDS `r*` (durable standoff) vs fleeing to the horizon. Must-fixes **#1 (integer terms/field) + #2 (fragile-θ) + #3 (normalized) now complete** (with #4 veto + #5 + #6a earlier). CPU bench ~868 µs/block-tick. **BUILD-ONCE-PER-ROOM DONE (`4185aef`):** `plan_kite_anchor` / `decide_squad_with_pathing` gained `shared: Option<&PositionLayers>`; `build_room_layers()` pub helper; the live `SquadManager` holds a per-tick `HashMap<RoomName, (LocalCostMatrix, PositionLayers)>` and builds the threat field + floods ONCE per target room (they depend only on the room's enemies, not on which squad asks), sharing across every squad engaging there — the layer architecture's perf payoff (`Some(layers)` reuses; `None` builds inline = unchanged single-squad path; bench + agent sim pass `None`). `build_target_matrix()` extracted so the layer build + the kite search share one matrix. All crates green (engine 40 / rover 31 / decision 55 / agent 35 / eval 14). **Remaining Stage 3b:** `focus_damage` actual-hits richness (RMA / kill-priority, vs the in-`r*` proxy), **#6b goal-latching**, default-on gating, full sim EXP-COHESION.

**Richer layer 1 DONE (`f96748b`):** integer `ThreatField` (raw incoming hits/tile, stamped from enemy creep power + tower falloff via a per-range LUT) as a cached `PositionLayers` layer, + the **survival veto (#4)** — `plan_kite_anchor` vetoes a tile whose net incoming would kill the most-fragile member in `< SURVIVAL_HORIZON=3` ticks (`SquadKiteView` gains `fragile_hits`+`squad_heal`, #2). v1 unboosted. +3 tests; CPU bench 950→1042 µs/block-tick (per-squad build; build-once-per-room sharing recovers it). **Remaining Stage 3b (default-ON MMO gate):** the normalized integer **safety term** reading the field (#1/#3 + #2 full — replaces the reach-depth proxy), the **`focus_damage`** layer (actual hits dealt, vs in-range), **build-once-per-room sharing** (the perf fix the layer architecture intends), weapon-range `r*` for melee/siege engage, **#6b goal-latching**, default-on gating on the CPU bench, a full sim EXP-COHESION. **No kill-switch flag** added (operator: not deploying until combat is better → harness-validation suffices; a live flag can wrap it near deploy). gates: engine 40 / rover 31 / decision 47 / agent 35 / eval 13; clippy + bot wasm-check clean; no WFV bump.
- **(earlier) Stage 0 + Stage 1 core:** — `combat-decision→combat-engine` dep added (bot host+wasm build clean), the duplicated `kite::tower_dps_at_range` deleted and delegated to the engine curve (byte-identical: all agent+eval U7 scenarios unchanged). **Re-sequenced into Stage 2 (with rationale):** the integer `ThreatField` *precompute*, the f32→i32 conversion, the `boost` field on `CombatBodyPart`, and the trivial-geometry guard — eagerly precomputing 2500 tiles for a single ~400-op kite search is a CPU *regression* until reachability + multi-block sharing amortize it, and the f32→i32 rewrite risks byte-identity for zero Stage-1 gain. The Stage-1 *foundation* (delete the drift-prone duplication + land the engine-delegation edge, per §7) shipped cleanly. No `WORLD_FORMAT_VERSION` bump; not deployed (byte-identical, host-validated).
- **Owner:** combat-AI
- **Keyed to:** [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) U-roadmap; design lineage [ADR 0008](0008-combat-and-squad-architecture.md) (+ [0008a](0008a-combat-tactics.md))
- **Provenance:** produced by an 11-agent ultracode design deep-dive (6 explorations → synthesis → 3 adversarial reviews → finalized spec), every claim verified against the tree (file:line). Operator question that prompted it: *should flee / stand-ground / close-distance be generalized into one objective-aware optimal-position decision that accounts for composition+health, target goal, pathfinding cost, damage potential from other creeps, and future positions via flood-fill?*

> This document is the implementer's contract. It **supersedes** the proposed synthesized spec; it does **not** yet change code — Stage 0+1 (§7) is the increment awaiting operator approval.

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

## 4. Anti-oscillation: goal latching, not per-tile stickiness (RESOLVES correctness-A1/A3)

The `w_stick*(tile != last_goal)` term is **dropped** — it compares against a goal the drifting squad never reached (a moving carrot). Replace with **goal latching in the driver** (state in `SquadOrderState`):

- Commit to a goal for `N` ticks **or** until reached/invalidated; re-plan only when latch expires, goal becomes unreachable, focus changes, or `U(new_best) > U(current_latched) + ε`.
- **Invalidate the latch on room change or centroid jump > K** (a stale `last_goal` in a different room makes the term garbage).
- Fire the trivial-geometry short-circuit (Guard 3) **before** centroid recomputation — the no-threat case stays a true fixed point.

## 5. Staged, measurable plan (keyed to U-roadmap; every stage flagged + abortable)

### Stage 0 (prerequisite commit — gates the kite delete, ~hours)
Add an **exhaustive `range in 0..=49` equality assertion** that `kite::tower_dps_at_range` == `damage::tower_attack_damage_at_range`. Must pass **before** Stage 1 deletes the duplicate. If they differ, reconcile to the engine curve first.

### Stage 1 — shared INTEGER threat field, pure refactor, NO behavior change (S, parity-locked)
Extract `score_tile`'s SAFETY+OPENNESS into `ThreatField::build(view, &matrix) -> ThreatField` (creep stamps within footprint; **towers stamped via the engine LUT**; `walkable[2500]`), **all `i32`**. Delete `tower_dps_at_range`. Change `plan_kite_anchor`'s closure to read the field. Add the **new `combat-decision -> combat-engine` dep + `boost` on `CombatBodyPart`** here. Add the **trivial-geometry short-circuit (Guard 3)** as a real fast-path. **No rover change, no reachability flood, no new score terms** — cohesion/value stay exactly as today.
- **Kill-switch:** `features.combat.shared_threat_field` (OFF -> today's `score_tile` verbatim).
- **Parity gate:** byte-identical `Kite{goal}` on EXP-KITE-1 / EXP-BREACH-1 / EXP-NEST-1 (host assert).
- **CPU gate:** per-room field-build counter to seg-57; saved recompute `K*(E+T)` must exceed build cost.
- **Static-map cache (RESOLVES perf Refutation-2):** the walkable/openness map is **terrain-derived -> cached for the room's life, rebuilt only on structure-destruction dirty-flag**, from Stage 1.

### Stage 2 — reachability flood in rover + scratch-buffer reuse (M-to-L, U9 cohesion)
Add `LocalPathfinder::reachability_from` (per-source edge pricing, `src[]` bookkeeping — a NEW search). Apply **Guard 5 seed filter**. Cohesion switches Chebyshev->true g-cost; unreachable tiles get a hard penalty. **Land scratch-buffer reuse here (perf-MF-1):** hoist `g`/`came`/`snapshot` into reusable pathfinder-owned scratch, **version-stamped `g`** to avoid the ~20 KB zero-fill churn per search. **Eliminate the double matrix materialization** (perf-MF-2): snapshot once, build all maps + flood from that one grid.
- **Kill-switch:** `features.combat.reachability_cohesion` (OFF -> Stage-1 Chebyshev). Under CPU pressure, **R is the first thing dropped** -> T-only score (a separate named kill-path — perf-MF-4b).
- **Gate:** U5 cohesion fraction + U6 outcome hold/improve on a new walled-corridor EXP-COHESION.

### Stage 3 — `PositionLayers` cache + unified signed utility + offensive positioning (M, T-POS/U-TOWER) — DEFAULT-ON gated on six must-fixes
**Stage 3a — the layer cache (operator architecture):** introduce `PositionLayers` (§2) — hoist the threat field, reachability R, centroid-distance C, and openness into one per-(room,tick) cached struct; build the integer `ThreatField` as the first layer here (this is where the "folded-into-Stage-3" precompute lands, now justified — it's amortized across uses, not a single search). Refactor `plan_kite_anchor` to consume `PositionLayers` instead of building R/C ad-hoc (removes the duplicate centroid flood). **Stage 3b — the unified utility:** `position_utility` (signed, **normalized integer terms** over the layers, objective presets, θ perturbation, future-threat with relative-mobility, **Guard 4 survival veto mandatory**, **goal latching**). `plan_engage_anchor` (attack-positioning, T-POS) reuses `search_scored` over the **same** `PositionLayers` — different weights, zero extra layer builds (the operator's point). **Bound `DamageDealt` to focus + <=4 nearest heal targets** or precompute it as the `focus_damage` layer (perf-MF-5).
- **Kill-switch:** `features.combat.engage_positioning` (OFF -> non-searching `Advance{goal,range=r*}`).
- **Tick-global combat-search op budget (perf-MF-3, blocking):** a hard ceiling on `B*max_ops` carried in the ADR-0004 CPU context; once exhausted, remaining blocks fall to non-search `Advance`/`Hold`. Per-stage `max_ops` bounds one search; nothing bounds B today — the death-spiral shape.
- **CPU gate (perf-MF-8, blocking):** gate on a **measured tick cost** from a NEW compound-worst-case sim bench — large open room, 6 towers, 5 melee + 5 ranged enemies, ~4 converging blocks — **on the sim, before live**. The EXP-* parity scenarios are single-block and never exercise the `B*search` term that is the actual risk.
- **Behavior gate:** U5 DPS/efficiency + U6 outcome improve on EXP-FOCUS/box-fight; self-play catches over-fit.
- **Default-ON requires all six correctness must-fixes** — status (2026-06-19): **#5 seed filter DONE** (`kite_threats` skips harmless creeps; `threat_step_ticks` filters immobile → only mobile attack-capable chasers seed the flood; tested — minor "leashed-NPC" nuance is a follow-up); **#6 deterministic tie-break DONE** (rover's heap is a total order on `(g,x,y)` + `best` updates on strict `<` → ties resolve to the closest-to-origin tile, also anti-oscillation; no RNG; tested). **#1 integer field, #2 per-block θ, #3 normalized terms, #4 survival veto are COUPLED to the richer actual-hits ThreatField — they ARE the signed-normalized-integer-utility-over-actual-hits and land WITH the richer layers** (doing them on the reach-depth proxy means arbitrary normalization refs + a body-agnostic veto, both redone). **#6 goal-latching** (commit-to-goal N ticks) needs a persisted last-goal — a focused live follow-up (ephemeral `#[serde(skip)]` on `SquadContext`, no WFV bump); the deterministic tie-break already covers core determinism.

### Stage 4 — incremental creep-stamp field (S-M, MEASURE-FIRST only)
Rebuild only the creep-threat stamp each tick if the Stage-1 counter demands it. Gated on measurement, not assumed.

### Abort/fallback ladder (strict, budget-driven, cheapest last)
1. Per-stage kill-switch -> prior stage's behavior.
2. Tick-global op budget exhausted (perf-MF-3) -> non-search `Advance`/`Hold`.
3. CPU-Critical (ADR-0004) -> drop R flood first (T-only), then last-tick stale goal, then `Advance`/`Hold`.
4. Bounded-search abort (built): best-so-far on `max_ops`; `None` => Hold centroid.
5. Cornered/all-unsafe -> Guard 1 critical-HP raw-flee still fires; multi-room flee is the separate L1 phase.
6. Parity backstop: U6 self-play / U5 oracle + seg-57 canary fail the nightly gate on any field drift.

## 6. Honest tradeoffs

- **CPU is the real risk, and the original spec hid four multipliers** (all now bounded): `B*max_ops` searches (Stage-3 op budget + bench gate), the O(targets) `DamageDealt` loop (capped to <=5), the per-search ~20 KB alloc churn (version-stamped scratch, Stage 2), the 122k naive dilation (separable two-pass). The maps-shared core is flat-in-E and CPU-positive; the *system* is only affordable with the bounds above. The operator's recorded CPU-death-spiral failure is why Stage 3 default-ON is gated on a measured compound-worst-case bench, not outcome metrics alone.
- **Explainability:** a weighted argmax is harder to debug than a branch tree. Mitigation: a `score_breakdown` debug dump of per-term contributions for the chosen tile; EXP-* sim loop as the only sanctioned weight tuner.
- **Integer quantization vs continuity:** fixed-point `SCALE=1000` can coarsen near-ties — but that absorbs noise (deterministic tie-break handles the rest) and is the price of parity. Accepted.
- **Where cheap heuristics stay genuinely better (kept, not regretted):** critical-HP flee, cohesion clamp, survival-horizon veto, non-chaser seed filter, trivial-geometry short-circuit, CPU-starved `Advance`/`Hold`, and **cross-room flee** (single-room scored search can't flee to an adjacent room — stays the separate L1 `MoveToRoom` phase; and `w_edge` must **invert/zero on the resolved cross-room exit tile** so the per-tile utility doesn't fight the L1 planner at the boundary — RESOLVES correctness-F2).

**DEFERRED (with reasons):** (a) per-role sub-block searches — post-Stage-3, gated on measured formation-scatter need; (b) Stage-4 incremental creep-stamp — measure-first; (c) focus-sanity co-design (correctness-F1, gamed-decoy tractor-beam) — **flagged, owned by focus-selection, not this spec**; position unification assumes focus is sane, and the Guard-4 survival veto is the backstop that stops the block diving onto a decoy's kill-zone. Do not ship the `DamageDealt` reward without the focus team adding "don't focus a target whose only approach is through a veto-level threat tile."

## 7. Recommended first increment — operator yes/no

**Approve Stage 0 + Stage 1 only** (Stage 0 ~hours prerequisite; Stage 1 is S, pure refactor, no behavior change):

> Add the `combat-decision -> combat-engine` dependency + a `boost` field on the view-only `CombatBodyPart`; prove `kite::tower_dps_at_range` bit-identical to the engine curve, then delete it; extract the kite scorer's safety+openness into an **integer** `ThreatField::build` (creep + engine-LUT tower stamps, cached static walkable map, trivial-geometry fast-path); rewire `plan_kite_anchor` to read the field. Behind `features.combat.shared_threat_field` (default OFF). Ship only when it produces **byte-identical `Kite{goal}`** on all three U7 scenarios and the seg-57 field-build counter shows net CPU savings.

Risk-free (behavior-preserving refactor with a byte-equality gate and a kill-switch), deletes a real duplication-drift hazard, lands the integer foundation + the engine-delegation edge every later stage reuses, and commits us to nothing about the unified utility until Stage 3 — which itself stays gated behind a CPU bench and six correctness must-fixes before it can go default-ON.

**Decision requested:** approve Stage 0 + Stage 1 as scoped? (Stages 2-4 return for separate approval, each behind its own flag and gate.)

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
