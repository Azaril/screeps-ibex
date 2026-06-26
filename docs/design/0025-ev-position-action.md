# ADR 0025 — Unified EV-of-(Position × Action) Per-Creep Combat Decision

- **Status:** Accepted (clean build, operator-approved 2026-06-24)
- **Date:** 2026-06-24
- **Supersedes (mechanism):** the role/`MemberCaps`-driven positioning in ADR 0024; the separate `decide_combat` action pipeline; `assign_focus_fire` + `assign_heals` + the `score_tile` engage/healer presets as the layout mechanism. These are **deleted**, not flagged off.
- **Builds on (kept):** ADR 0019 (`score_tile` term math, as `mhp` scalars), ADR 0020 (Lanchester engage gate, EV target kill-budget + spill, force sizing), ADR 0024 (hierarchical positioning: shared threat field + reachability flood + one target-flood).
- **Crates:** `screeps-combat-decision` (kernel), `screeps-combat-agent` (adapter), `screeps-combat-eval` (harness).

### Build directives (operator, 2026-06-24)

This is a **clean replacement**, optimized for the best-designed system with the least technical debt — not a backward-compatible migration:

1. **No incremental migration / feature flags / legacy path.** Build the kernel, swap it in, delete the old machinery in the same change. The harness suite is the correctness gate; we do **not** preserve byte-identity with the old path, the live-vs-old IntentRecorder digest, or any serialized shape (no `WORLD_FORMAT_VERSION` concern — positioning is per-tick). "Parity" that still matters is **sim-vs-real-engine** correctness (so the harness is a faithful oracle), not back-compat.
2. **Structure / breach / declaim value is IN v1** (§2.4), not deferred. Without it the squad cannot break ramparts, breach into rooms, or raze bunkered spawns/towers — the whole point of an offensive combat system.
3. **Seed constants only; the tournament tunes.** Every `mhp` scalar is a tournament-tunable seam. We do **not** hand-tune in this build — get to a fully clean, correct, working system; then tune the complete system with the self-play tournament.

## 1. Context

The combat decision still uses a **fixed layout** mechanism, just expressed in capability terms instead of role labels. Today a creep is classified by `MemberCaps` (`can_melee` / `can_range` / `can_heal`) → handed a `desired_range` (melee→1, ranged→3, support→back-line) → marched to that ring by `score_tile` with a role-selected preset (`engage` / `healer`) → and then, *separately*, `decide_combat` picks attack-vs-heal intents once it is standing.

The operator rejects this:

> "this is STILL a fixed layout by trying to identify role by capabilities and then ordering. **Calculation for what the creep CAN DO IN THE SLOT they'll be in and positioning to get highest EXPECTED VALUE outcome is more important.**"
>
> "Are you just updating the movement formation? Or actual full layout AND action?"

The answer this ADR commits to: **full layout AND action, jointly.** A creep's value at a tile *is* the expected value of the best engine-legal set of intents it can actually fire from that tile this tick (real working parts, real targets/allies/threats in range, engine intent-exclusion respected), netted against the incoming-damage risk at that tile. The creep goes to the `(tile, action-set)` pair of highest EV. **No role archetype, no per-role `desired_range`, no claim-priority ordering as the mechanism.** Formation *emerges* because each creep maximizes its **marginal** contribution to one squad win-probability currency, and the squad coordinates only through **shared residual budgets** (don't double-count a kill, don't over-heal) — never through a role sort.

This unifies what ADR 0019/0020/0024 left split: position utility and action choice now share **one currency** and are chosen in **one argmax**.

## 2. Decision

### 2.1 The one currency — `mhp` (milli-hits of squad win-probability swing)

Every term — offense, denial, heal, survival, cohesion — is converted to a single signed integer in **`mhp`** (thousandths of one hit-point of *squad fighting-strength margin*). Integer-only on the hot path (the proposals' "no floats" promise is honored here as a real, scoped rewrite — see §2.6 and §7, not as the impossible "byte-identical re-expression of the f32 `score_tile`" the source proposals hand-waved).

We do **not** invent a new exchange rate between damage, deaths-prevented, and risk. We reuse the **Lanchester fighting-strength model already in `assess_engage`** (lib.rs:937–1008) as the win-probability functional, and price each action delta as its signed contribution to that margin. This is proposal **D**'s core insight grafted onto proposal **A**'s per-tile kernel.

**The functional `W`.** Per squad per tick `assess_engage` already computes:

- `our_strength   = fighting_strength(our_dps, our_ehp, n=2) = our_dps * our_ehp`
- `enemy_strength = fighting_strength(killable_dps, killable_ehp, n=2)`
- `μ = clamp((our_strength − enemy_strength) * 1000 / enemy_strength, −1000, 1000)`  (permille, i128 math, lib.rs:1003–1005)

`W` maps `μ` to a `[0,1000]` win-permille via a **41-entry integer sigmoid LUT** baked at build time (monotone, no `powf`, deterministic). `W` is strictly increasing in `our_strength`, strictly decreasing in `enemy_strength`, so the sign of every term below is correct by construction.

**Sensitivities (the exchange rate, computed ONCE per squad per tick).** Rather than re-evaluate `W` per candidate, take two integer scalars from the LUT's local slope at the current `μ`:

```
g_us   = W'(μ) · dμ/d(our_strength)      // mhp gained per unit of our fighting-strength preserved
g_them = W'(μ) · dμ/d(enemy_strength)    // mhp gained per unit of enemy fighting-strength removed
```

These two numbers are the entire calibration surface between "deal damage", "prevent a death", and "take risk". They are derived from the *existing* tuned model, not new magic constants. (Degeneracy fix for blowouts is in §7.)

### 2.2 The per-`(tile, action-set)` EV kernel

The whole decision collapses to one pure function:

```
tile_action_ev(member, tile, residuals, layers, g_us, g_them) -> (ev: i64 mhp, ActionSet)
```

For a candidate `tile` it (1) enumerates the engine-legal action-sets the member's working parts permit from that tile (§3), (2) for each set picks the best in-range target(s)/ally and prices the combo against the **live shared residual budgets**, (3) nets incoming-damage risk and melee attack-back, and (4) returns the argmax set and its EV. The EV of a member at `tile` with chosen legal `ActionSet A`:

```
EV(m, t, A) =  OFFENSE(A,t) + DENIAL(A,t) + HEAL(A,t)
             − RISK(m,t)    − SELF_RISK_MELEE(A,t)
             − DISCOHESION(t) − APPROACH_DEFICIT(m,t)
```

All terms in `mhp`:

| Term | Definition | Provenance |
|---|---|---|
| **OFFENSE** | `g_them · Σ_targets min(damage_landed(A,t,target), residual_kill[target])`. `damage_landed` respects rampart-redirect (0 credit for a single-target hit redirected to a rampart, resolve.rs) and the engine net (an out-healed target has residual 0). The `min` against the live residual is what kills overkill. | `ev_target_order` budget (lib.rs:318); `assign_focus_fire` spill (lib.rs:332-359) |
| **DENIAL** | When cumulative committed squad damage on a target **crosses its budget this tick** (it dies), add `g_them · threat_value(target) · KILL_HORIZON` (small `~3`). Removing a healer's future output thereby beats chipping a tank. Granted **once per target** (the budget-crossing member books it), so no two creeps both claim the kill swing. | `threat_value`/`ttk` (lib.rs:294,316) |
| **HEAL** | `g_us · Σ_allies min(heal_output(A,t,ally), residual_heal_need[ally])`, with the **MORTAL** case (`projected_incoming ≥ ally.hits`) crediting the ally's **whole** remaining fighting-strength (a prevented death is the max swing). "Mortal" is thus a *region of the continuous curve*, not a boolean veto. | `best_heal_target` mortal-first (lib.rs:482); `assign_heals` deficit+risk (lib.rs:1138) |
| **RISK** | `g_us · (net_incoming_at(t) lost from THIS member)` where `net = max(0, ThreatField.raw_at(t) − reaching_squad_heal)`, scaled by how close it is to killing this member over `SURVIVAL_HORIZON`. Plus a hard **`LETHAL_TILE_PENALTY`** backstop (astronomical, dominates all EV) when `net · SURVIVAL_HORIZON > member.hits` — the binary survival veto (kite.rs:701) is **kept as a floor** under the graduated curve. | `incoming_damage_at` / `ThreatField` |
| **SELF_RISK_MELEE** | Expected melee attack-back if `A` lands a melee `Attack`, the target has `ATTACK` parts, and we are **not on a rampart** (resolve.rs:317-321). **New EV the current model ignores** — and the cleanest proof that joint position+action matters (a near-dead melee+ranged creep should often *not* melee). | resolve.rs:317-321 (verified) |
| **DISCOHESION** | Wall-aware distance-from-centroid penalty past `K`, converted to `mhp` by one scale constant. Holds a forming/no-target blob together. | `score_tile` cohesion term |
| **APPROACH_DEFICIT** | When **no** action lands from `t` (out of every weapon/heal range): `−` (scaled) safe-path distance `D[t]` from the shared target-flood, giving an out-of-range creep a continuous downhill gradient toward where it *would* have EV. Replaces the `LAYOUT_DOABLE_BONUS` step-function with a gradient. | target-flood `D[]` (lib.rs:883) |

`OPENNESS` / `EDGE` / `FUTURE` survive as small additive `mhp` terms reusing the existing layers (a short-horizon `FUTURE` is the deferred extension in §10, *not* in v1).

**Squad win-probability is the sum of member EVs by construction.** Because OFFENSE/HEAL are capped by the *shared* residuals already consumed by earlier-committed members, each member is scored on what the rest left uncovered — its **marginal** contribution. This is a greedy submodular maximization (proposal B's framing): the two coverage terms are submodular (the k-th shooter on a target / k-th healer on an ally has diminishing return as the budget fills), which is exactly why a one-pass greedy + bounded re-pass is near-optimal and we never need Hungarian matching.

### 2.3 The joint selection algorithm

Per engaged squad per tick (the engage/retreat **gate runs first**, unchanged — `assess_engage` + `ENGAGE_BALANCE_BAND` hysteresis, lib.rs:935; this layer only decides *how* to fight once committed):

**STAGE 0 — shared, once (no new cost vs today):**
- Build `PositionLayers` (`ThreatField` + reachability flood) — build-once-per-room, reused across squads (unchanged).
- Run the **one** target-flood Dijkstra `D[]` from the focus over the threat-weighted matrix (`TARGET_FLOOD_OPS = 2500`, unchanged, kite.rs:887).
- Compute `μ`, `W`, `g_us`, `g_them` from `assess_engage`'s existing strengths (~20 ops).
- Build the **residual budget ledgers** (Vec-indexed, integer):
  - `residual_kill[e] = e.hits + heal_reaching(e)` for each killable enemy (the `ev_target_order` budget verbatim).
  - `residual_heal_need[a] = max(0, projected_incoming(a) − a.hits)` (+ a deficit top-up) per ally.

**STAGE 1 — per member, in a deterministic commit order:**
- Candidate tiles = the member's Moore neighbourhood (current ±1) ∩ walkable, **plus** its current tile (incumbency). This is the **same local 9-tile window** `plan_squad_layout` already scans (kite.rs:927). **No per-member flood is added** — the shared `D[]` supplies the long-range gradient.
- For each candidate tile `t`: enumerate engine-legal `ActionSet`s (§3), price each via `tile_action_ev` against the **current** residual ledgers, keep the max-EV `(t, A)`.
- Apply the **incumbency dead-band** (`LAYOUT_DEAD_BAND`) to the **joint `(tile, ActionSet)` pair** and the **spacing penalty** against already-committed tiles. Member commits to its argmax.

**STAGE 2 — commit + drain (the coordination):**
- Subtract the committed OFFENSE from `residual_kill[targets]` and the committed HEAL from `residual_heal_need[ally]`. The next member sees reduced budgets → naturally spills to the next-best target / next at-risk ally. This *is* `assign_focus_fire` + `assign_heals`, re-expressed as one greedy drain, preserving the same no-overkill / no-over-heal guarantee.
- Emit `member_goal = chosen tile` (flows through the existing `member_goals → decide_movement → rover-validates` seam) and the `ActionSet` as the per-creep `CombatIntent` vector directly (**no second `decide_combat` pass**).

**The commit order is value-derived, NOT role-derived.** Sort members by `(descending best-achievable single-tile EV, then ascending hits, then idx)`. The pre-pass best-tile-ignoring-others is `O(members × 9)`, negligible. Highest-leverage members claim scarce high-value tiles first; a creep that *loses* a range-1 tile finds its next-best neighbour now has the higher EV and self-demotes **by EV**, with no melee→ranged→healer rule. (Honest note for review: in a target-rich fight a melee+ranged creep's best single-tile EV systematically exceeds a ranged-only creep's, which exceeds a pure healer's — so the *order* correlates with the old buckets. That is acceptable and arguably correct: it is value-correlated, not label-driven, and it sets *who picks first under contention*, not *what range anyone holds*. We reject re-introducing a hard role sort; see §9 for the rejected "contestedness sort" variant and why a single optional swap-pass is the escape hatch instead.)

**Optional STAGE 3 — one bounded re-pass (added only if measured):** any committed member whose chosen target/ally was depleted by a later committer re-picks against final residuals; switch only if it beats the held slot by `> LAYOUT_DEAD_BAND`. Bounded to **1 sweep** for determinism and CPU. Catches the "A aimed at a target B finished" case. Greedy + incumbency is expected to be at a fixed point in the common case; ship STAGE 3 only if the harness shows EV left on the table.

### 2.4 Structures, breach, and declaim — objective EV (IN v1)

Breaking a base is the point of an offensive system, so enemy **structures** and the **controller** are first-class targets in the *same* `mhp` currency and the *same* kernel — not a special phase. The only additions are more entries in the residual ledger and two more legal actions in the enumerator.

**Structure value `V_struct(kind)` (seed, tournament-tunable), priced in the `g_them` currency** — destroying it removes enemy fighting capability or unlocks the win condition:

| Structure | Why it has value | Seed `V_struct` |
|---|---|---|
| Tower (energized) | Its `tower_dps` is literally a term in `enemy_strength`/the `μ` the gate already computes; razing it *directly* raises `W`. Also kills enemy heal-reaching. | highest |
| Spawn / InvaderCore | Denies reinforcement / is the room's heart (the objective the `CombatObjective` usually names). | high |
| Rampart / Wall **on the breach corridor** | Inherited value: it is the gate to a shielded high-value structure (below). | derived, not intrinsic |
| Container / road / other | Negligible. | ~0 |

**OFFENSE against a structure** is the same term as against a creep: a member in range with a legal damaging intent (`Attack`/`RangedAttack` melee/ranged, or `Dismantle` with WORK parts — 2× structure damage) adds `g_them · min(damage_landed, residual_struct[s]) · (V_struct / s.hits_max)` — progress toward removing `V_struct`. **DENIAL** books the full `g_them · V_struct` on the tick the structure dies (for a tower this is exactly the `tower_dps` drop the gate will see next tick — self-consistent).

**Breach (reuse the existing machinery, recast as inherited value).** Ramparts/walls shield the valuable structures; you must break them to reach the objective. The kernel reuses ADR 0024's breach search verbatim — `breach_redirect` + `breach_path_blockers` Dijkstra priced by hits (`BREACH_HIT_WEIGHT`, lib.rs) — but instead of *redirecting the focus* (the current hack), it **assigns inherited value** to the blocker tiles: the first rampart/wall on the cheapest corridor to a shielded objective `O` gets `V_struct = V(O) · (remaining corridor discount)`, so dismantling/attacking it has real, monotone EV (each hit is progress toward opening `V(O)`). A member with WORK at the breach tile finds `{Dismantle}` is its highest-EV `(tile, action-set)` — a "siege role" with no taxonomy. Behind a breach, the next blocker lights up once the first is gone. This makes "tank-and-dismantle through a rampart to the spawn" fall out of the EV, and replaces `breach_redirect`'s focus-rewrite with a priced term.

**Declaim** (take/neutralize the room — the live `SalvageMission`/`DeclaimJob` objective): a member with CLAIM parts at range 1 of a strategic enemy **controller** can `AttackController` (engine `CombatAction::AttackController`, already in the sim engine). It is just another target in the ledger: `residual = attack-to-neutral`, `V = V_controller` (high, discrete — room neutralization), enumerated as the legal singleton `{AttackController}` (drops melee, per the engine table §3). No separate declaim path.

So one kernel prices: kill a creep, raze a tower/spawn, breach a rampart toward a shielded objective, and declaim a controller — all in `mhp`, all chosen jointly with position. `select_focus_target`/`breach_redirect` (kept) seed *which* structures are valuable and *where* the corridor is; the kernel decides each creep's best `(tile, action)` toward them.

## 3. Engine-legality table (ground truth)

Verified this session against `C:/code/screeps-engine/src/processor/intents/creeps/intents.js` (the canonical priority table) and the sim mirror `screeps-combat-engine/src/resolve.rs:142-164`.

The canonical table (`intents.js:3-13`):

```
rangedHeal:        ['heal']
dismantle:         ['attackController','rangedHeal','heal']
attack:            ['build','repair','dismantle','attackController','rangedHeal','heal']
rangedMassAttack:  ['build','repair','rangedHeal']
rangedAttack:      ['rangedMassAttack','build','repair','rangedHeal']
```

`checkPriorities` (intents.js:21-23): an intent fires iff it is queued AND none of its listed higher-priority conflicts are also queued. So the **drop rules for the combat subset** are:

| Intent | Dropped when ALSO queued | Consequence |
|---|---|---|
| `move` | — | **Composes with everything** (it IS the position choice). |
| `attack` (melee) | `dismantle`, `rangedHeal`, `heal` | Melee `Attack` dropped if any heal **or** dismantle present. |
| `rangedAttack` | `rangedMassAttack`, `rangedHeal` | Dropped if RMA **or** `rangedHeal` present. |
| `rangedMassAttack` | `rangedHeal` | **Dropped if `rangedHeal` present.** RMA is NOT dropped by plain `heal`. |
| `rangedHeal` | `heal` | Use one heal flavour at a time. |
| `dismantle` | — (within combat subset) | Drops melee `attack` (as the inverse of the row above). |

**Composition closure (what a creep may emit together):**

| Combo | Legal? | Note |
|---|---|---|
| `{Attack, RangedAttack}` | ✅ | Both weapons at range 1 — the canonical front-line slot. |
| `{Attack, RangedMassAttack}` | ✅ | Melee + RMA compose. |
| `{Attack, Heal}` | ❌ | Heal drops melee Attack. |
| `{RangedAttack, Heal}` | ✅ | **Plain `heal` does not drop `rangedAttack`.** Heal-capable ranged creep fires + heals. |
| `{RangedAttack, RangedHeal}` | ❌ | **`rangedHeal` drops `rangedAttack`.** |
| `{RangedMassAttack, Heal}` | ✅ | Plain `heal` does not drop RMA. |
| `{RangedMassAttack, RangedHeal}` | ❌ | **`rangedHeal` drops RMA.** |
| `{RangedAttack, RangedMassAttack}` | ❌ | RMA drops `rangedAttack` — emit one, never both. |
| `{Dismantle, anything-melee}` | ❌ | Dismantle drops melee Attack. |
| `{Heal}`, `{RangedHeal}`, `{}` | ✅ | Singletons / idle always legal. |

**This corrects the fatal flaw in source proposals A and D**, both of which listed `{RMA, RangedHeal}` (and A also `{RangedAttack, RangedHeal}`) as legal "maximal sets". They are **engine-illegal** — `rangedHeal` is in both `rangedMassAttack`'s and `rangedAttack`'s conflict lists. The enumerator MUST encode: `rangedHeal` drops both ranged-offense intents; plain `heal` drops neither. The asymmetry (`{RMA,Heal}` legal, `{RMA,RangedHeal}` not) must be exact.

**Sim/live parity hole (must fix before any deploy).** `resolve.rs:142-164` (verified) only drops `rangedAttack` on `has_rma`, and only drops melee `Attack` on heal/rangedHeal/dismantle. It does **NOT** model `rangedHeal` dropping `rangedAttack`/`rangedMassAttack`. So a combo that is illegal on live (`{RangedAttack, RangedHeal}`) would be *accepted* by the sim, and the harness "every emitted set passes `filtered_actions`" gate would NOT catch it. **Decision:** the enumerator enforces the **strict live rule** (conservative: never emit `rangedHeal` with any ranged offense). The sim slightly under-uses `rangedHeal`+ranged vs a hypothetical permissive engine — acceptable. Additionally, **`resolve.rs::filtered_actions` MUST be patched** to mirror the live `rangedHeal` drops, with a parity unit test asserting the Rust mirror matches `intents.js` for every `(parts, queued-set)` case. This is migration Stage 1.

**The enumerator is a tiny fixed menu, not a powerset.** `enumerate_legal_sets(member, tile)` = `choose-one-of {none, Attack}` × `choose-one-of {none, RangedAttack, RMA}` × `choose-one-of {none, Heal, RangedHeal}`, then delete any illegal pairing per the table above (≤ 6 surviving candidates per creep, mostly pruned by in-range targets). RMA-vs-`RangedAttack` is itself an EV choice inside the enumerator (RMA when `Σ min(rma_dmg_at_range, residual)` over clustered hostiles beats single-target fire), replacing the hardcoded `≥3-in-range` heuristic (lib.rs:540). The melee-vs-heal exclusion becomes a **priced choice** (`{Attack,RangedAttack}` vs `{RangedAttack,Heal}`), replacing the `apply_heal` mortal-only veto (lib.rs:413-429). No intent the engine would drop is ever emitted, so no decision cycle is wasted (fixing a real current silent-drop bug).

## 4. How formation EMERGES (worked examples)

No example uses a role label. Each creep runs the *same* `tile_action_ev` argmax.

**(a) Melee + ranged + heal creep ("triple"), allies healthy, enemy block in front.**
At a **range-1** tile, `enumerate_legal_sets` offers `{Attack, RangedAttack}` (both weapons land — they compose). OFFENSE is high (`melee + ranged` net hits, capped by residual), but `SELF_RISK_MELEE` deducts the target's attack-back and `RISK` deducts the higher incoming at the front. At a **range-3** tile, only `{RangedAttack}` lands (lower OFFENSE) but `RISK`/`SELF_RISK_MELEE` are lower. If allies are healthy, `{RangedAttack, Heal}` scores `HEAL ≈ 0` (no residual need), so the heal option is dominated. The argmax is **range 1 with both weapons** whenever `(melee OFFENSE gain) > (attack-back + extra incoming)` — i.e. the creep closes *because that is where its priced output peaks*, not because it was labelled melee. Drop one ally to mortal and `{RangedAttack, Heal}` (heal composes with ranged) suddenly books the ally's **whole** fighting-strength via `g_us` — the argmax flips to fire-and-heal **without** surrendering the ranged weapon, and **without** the engine dropping anything. The melee `Attack` is the only thing sacrificed, and only because the EV says so.

**(b) Pure healer (HEAL parts only).** OFFENSE = DENIAL = 0 at every tile (no weapon). Its EV is dominated by `HEAL − RISK − DISCOHESION`. The argmax is the tile that maximizes `Σ min(heal_output_at_range, residual_heal_need)` while staying out of lethal incoming and near the centroid — i.e. it hugs the at-risk cluster from the safest covering tile. No `is_support` branch, no separate healer preset search — the **same** kernel produces "back-line healer" as a byproduct of where heal value is. Two healers don't over-heal the same ally because the first drains `residual_heal_need[a]`; the second's HEAL on `a` is then `0` and it triages the next ally.

**(c) Siege / dismantle creep, tower-drain room.** Against a wall/rampart blocking the focus, `{Dismantle}` (drops melee, legal alone) scores OFFENSE against the **structure** (breach progress) priced via the same `g_them`; `DENIAL` fires when the rampart breaks and the focus behind it becomes killable. `RISK` from energized towers is the `assess_engage` `tower_dps` drain folded into the threat field. The creep picks the breach tile + `Dismantle` because that is the highest-EV `(tile, action-set)` — a "siege role" emerges with no taxonomy. (Caveat: full structure/breach/declaim pricing is the §10 extension; v1 ships creep-vs-creep + a basic breach term, see Open Questions.)

In all three, **who gets the scarce range-1 tile** is decided by the value-sorted commit order + spacing + residual drain — not by a melee→ranged→healer sequence.

## 5. What it SUBSUMES (replaced vs kept)

| Current piece | Fate | How |
|---|---|---|
| `MemberCaps::desired_range()` (kite.rs:756) | **REPLACED** | Range emerges from where the member's priced OFFENSE/HEAL peaks net of RISK. |
| `MemberCaps::order()` claim priority (kite.rs:765) | **REPLACED** | Value-sorted commit order (EV desc, hits asc, idx) + shared-residual drain. Not a role sort. |
| `MemberCaps::is_support()` + separate healer search (kite.rs:750, 908) | **REPLACED** | One kernel; a pure healer just has OFFENSE=0, HEAL>0. |
| `KiteScoreParams` `engage`/`healer` presets as role mechanism (kite.rs:195) | **REPLACED** | Weights survive only as scalar `mhp` mixing constants (EXP-* tunable); no preset chosen by classification. |
| `decide_combat` attack/heal pipeline + `apply_heal` mortal veto (lib.rs:375-430) | **REPLACED** | Heal-vs-attack is a priced choice between legal action-sets; `decide_combat` becomes "emit my committed slot's intents + MoveTo". |
| `attack_with_orders` / `fallback_attack` + RMA `≥3` heuristic (lib.rs:514-589, 540) | **REPLACED** | Target/weapon = enumerator argmax; RMA-vs-single is an EV choice. |
| `LAYOUT_DOABLE_BONUS` approach step (kite.rs:804) | **REPLACED** | Continuous `APPROACH_DEFICIT` gradient over the existing `D[]`. |
| `MemberCaps` (parts-presence bits) | **KEPT (demoted)** | Only feeds `enumerate_legal_sets`; no longer drives position/order. |
| `ev_target_order` kill-budget + `threat_value`/`ttk` (lib.rs:305) | **KEPT** | Seeds `residual_kill` and the OFFENSE/DENIAL currency. |
| `assign_focus_fire` spill (lib.rs:332) | **SUBSUMED** | Becomes the `residual_kill` drain side-effect. Run in parallel as a debug-build cross-check during migration. |
| `assign_heals` mortal-first + deficit (lib.rs:1112) | **SUBSUMED** | Becomes the `residual_heal_need` drain + MORTAL credit. Debug cross-check during migration. |
| `best_heal_target` mortal-first (lib.rs:458) | **KEPT (recast)** | The HEAL term + MORTAL whole-fighting-strength credit. |
| `ThreatField` / `incoming_damage_at` / survival veto (kite.rs:43, 701) | **KEPT** | RISK input + the `LETHAL_TILE_PENALTY` floor. |
| target-flood `D[]` + `PositionLayers` build-once sharing (kite.rs:887, 339) | **KEPT VERBATIM** | The dominant shared cost; consumed by APPROACH_DEFICIT and the local window. |
| incumbency dead-band + spacing penalty (kite.rs:813, 828) | **KEPT** | Anti-oscillation, now on the joint `(tile, ActionSet)`. |
| `assess_engage` / Lanchester gate / `force_sizing` (lib.rs:960) | **KEPT UNCHANGED** | Outer "fight or retreat" gate AND now the `W`/`g_us`/`g_them` source. |
| `select_focus_target` (lib.rs:230) | **KEPT** | Shared stable focus for orientation + the unengaged/solo fallback; also seeds focus stability so the squad concentrates below the kill boundary (see §11 chipping risk); seeds *which structure* is the objective for §2.4. |
| `breach_redirect` + `breach_path_blockers` Dijkstra (lib.rs) | **KEPT (recast)** | No longer *rewrites the focus*; instead seeds the breach corridor so the first blocker tiles get **inherited `V_struct`** (§2.4). Breaching becomes a priced OFFENSE term, not a focus hack. |
| structure `Attack`/`RangedAttack`/`Dismantle` targeting + `AttackController` declaim | **NEW (core)** | Structures + controller are targets in the same ledger/kernel with `V_struct`/`V_controller` value (§2.4) — so razing towers/spawns, breaching ramparts, and declaiming rooms are EV-chosen jointly with position. |
| `SquadDecision` output shape (focus, movement, `member_goals`, intents) | **KEPT** | `member_goals` now populated for ALL members; `focus_assignments`/`heal_assignments` become outputs for telemetry. Agent/host seams untouched. |

## 6. Tractability

**Shared (unchanged):** one `PositionLayers` build + one target-flood Dijkstra (`TARGET_FLOOD_OPS = 2500`) per engaged squad per tick, build-once-per-room, amortized across members. This dominates wall-time and is **not increased**.

**New local stage** = `O(members × 9 tiles × ≤6 action-sets × per-set work)`. Per-set work = best in-range target scan (capped at **top-K=4 by kill-budget** — spill beyond 4 is vanishingly rare) + best in-range ally scan (≤ N). For a typical N=8 squad with H≤10 in-range hostiles: `8 × 9 × 6 × (4 + 8) ≈ 5,200` cheap integer ops on top of the 2500 flood. Each op is array reads (`ThreatField.raw_at` is `O(1)`) + clips + a multiply; no pathfinding, no allocation (fixed-size action-set array, Vec-indexed ledgers).

**End-to-end:** `~2500 (flood, shared) + ~5,200 (local)` vs today's `~2500 + plan_squad_layout O(members×9×score_tile) + a separate decide_combat per member`. The new local stage **fuses** the layout re-score and `decide_combat` into one pass, so the ≤6× action factor is offset by deleting the second pass. Net is `~1.5–2×` the per-squad *non-flood* cost, which is the smaller term — within the brief's "keep it in that ballpark" constraint, and matching the architecture note's own `O(2500 + N×36)` estimate.

**Worst case (20-creep blob, many in-range hostiles):** the K-cap on the target scan keeps the inner factor constant; `20 × 9 × 6 × (4 + 20) ≈ 26k` local ops. Mitigations, in order: (1) K-cap (above); (2) cap candidate tiles to the 5 best by `D[]` pre-rank when `N > N_BIG`; (3) the optional STAGE-3 re-pass is bounded to 1 sweep. The flood remains the dominant per-squad cost; if a hard ceiling is hit on MMO, the orthogonal lever is share-the-flood-per-target (ADR 0024 Future-work #6), not changing this kernel. **CPU gate:** `cpu_bench_compound_worst_case_is_bounded` must stay green before any K or sweep increase ships.

**Memory:** residual ledgers `O(targets + allies)`, per-member best-bid cache `O(N)` — tick-scoped, no persistent state. **No `WORLD_FORMAT_VERSION` bump** (pure per-tick decision, serialized shape unchanged) — confirm at Stage 4.

## 7. Anti-oscillation + determinism

**Determinism.** Per the build directives we do **not** preserve byte-identity with the old path or worry about a live-vs-old digest — the old `score_tile`/`decide_combat` machinery is deleted, and the *only* parity that matters is **sim-vs-real-engine intent legality** (§3), which both the sim and the bot honour because they run the same kernel. We still want the kernel itself **deterministic** (reproducible replays, stable behaviour, tournament-comparable):

- The `mhp` kernel is **integer/fixed-point throughout** (sigmoid LUT, `g_us`/`g_them`, all term sums) — no f32 on the hot path, so no IEEE-ordering subtleties. (The old `score_tile` was fixed-order f32; this is a real integer rewrite, simpler and cheaper, and we delete the f32 path rather than keep it byte-compatible.)
- Tie-breaks are total and explicit: `(EV desc, then x, then y, then ActionSet rank, then idx)`. Residual ledgers are `Vec`-indexed (no `HashMap` iteration-order nondeterminism). The shared target-flood result is iterated in sorted-key order. A determinism unit test asserts identical output on repeated runs (as `layout_is_deterministic` does today).

**Anti-oscillation (must not regress single-room ~0.5%).** Two new degrees of freedom vs the positional baseline: action choice, and the commit order.

1. **Joint dead-band.** The `LAYOUT_DEAD_BAND` incumbency bonus applies to the **joint `(tile, ActionSet)` pair**: last tick's chosen pair gets a `−band` cost, so neither the tile nor the action flips unless beaten by `> band`.
2. **Mortal-flag hysteresis (the multiplicative-threshold fix).** A fixed-cost band does **not** damp a *multiplicative* swing: when an ally's HP crosses the MORTAL threshold the HEAL credit jumps by the whole-fighting-strength factor, easily exceeding any fixed band → a healer-capable fighter flips `{Attack,RangedAttack} ↔ {RangedAttack,Heal}` on a knife-edge ally. Mitigation: apply hysteresis to the **MORTAL flag itself** (the ally is "mortal" for `assign`/credit purposes with a small HP dead-zone around `incoming = hits`), not only to the resulting EV. This is the explicit fix the adversarial review flagged that the source proposals missed.
3. **Order stability.** The value-sorted commit order can churn if a creep flips across the sort comparator tick-to-tick. Mitigation: the order key includes the incumbent EV (which carries the dead-band), so a stable creep stays stable in the order. (The rejected "contestedness sort" — §9 — is *more* churn-prone because the contested/uncontested boundary is a hard threshold; that is one reason it is rejected in favour of value-sort + optional swap.)
4. **New metric.** `oscillation_rate` (metrics.rs:291) is **purely positional** and cannot see action-thrash (a creep standing still while flipping Attack↔Heal scores 0.0). We therefore **add an `action_oscillation_rate` metric** (A-B-A on the emitted `ActionSet` per creep) and gate Stage 3+ on it ≤ a baseline-derived threshold. Gating only on positional oscillation would be blind to the regression this design is most likely to cause.

### 7.1 Oscillation is a PROXY, not the objective (operator, 2026-06-25)

**Oscillation is not inherently bad.** Period-2 movement that *improves the outcome* is correct behaviour: stepping in-and-out of a tower's optimal-damage band, jinking to force a tower/defender to re-acquire (wasting its shot), or shuffling to keep a focus in weapon range while denying the enemy a clean line — these are *beneficial* A-B-A patterns. **Fatigue is not a cost** for a properly-built creep (enough MOVE parts → it moves every tick for free), so "it moved a lot" is not itself a problem.

What we actually want to minimise is **UNNECESSARY oscillation** — movement that does *not* improve win-probability EV (the seam-cycling, swap-churn, and frozen-then-twitch failures in §7.2): motion that burns the tick without buying damage dealt, damage avoided, or a better next-tick position. So:

- The **primary** judge is the **EV / net-HP outcome** (the tournament, §9 ranking), not the oscillation count. A config that "oscillates more but wins more" is *better*.
- `oscillation_rate` stays as a **cheap regression tripwire** for *gross* unnecessary jitter (the 85%-swap-churn class), not a hard optimisation target. Its threshold is a sanity bound, not a goal — do not tune *toward* a lower number at the expense of net HP.
- **Future metric refinement (open question §11):** make the metric *outcome-aware* — only count an A-B-A as "unnecessary" when the reversal did **not** reduce incoming damage / increase dealt damage / improve the EV of the resulting position vs holding. That separates beneficial jinking from confused twitching. Until then, read the positional rate together with the net-HP outcome, never alone.

### 7.2 As-built stabilisers (2026-06-25) — and why the planned set changed

The planned damping above (joint dead-band, mortal-flag hysteresis, incumbent-EV-weighted order) was partly superseded once the kernel ran against the harness; the **shipped** stabilisers (all seeds in `KernelParams`, tournament-tunable) are:

1. **Always-on approach pull + offense-gated incumbency (the regime split).** The approach gradient (`−unit·approach_coef·D[tile]`, `approach_coef=2`) is applied at **every** tile, *not* only when out of range. The incumbency dead-band (`+unit·incumbency_coef`, `=3`) is applied **only at a tile where an OFFENSE action lands** (`offense_reachable`). Rationale, learned the hard way: a heal-capable creep has a "doable action" (heal an at-risk ally) almost everywhere, so a doable-gated incumbency *froze a melee+heal siege at range 5* — it healed in place and never advanced. Splitting it — *approach always pulls; incumbency only brakes once you're actually fighting* — fixes both the freeze (approach wins while closing, `approach_coef > 0` with no incumbency competing) and the engaged jitter (strong incumbency where the fight is, where OFFENSE's huge `g_them·damage` already dominates so the brake costs nothing).
2. **Stable commit order (member index), NOT value-sorted.** The §9 "highest-leverage first" value-sort **churned**: its comparator depends on per-tick residuals/positions, so the order flipped tick-to-tick, reassigning contested tiles between creeps → a swap-driven period-2 (measured: self-play hit **85%**). A stable index order resolves contention deterministically and dropped it to ~3.6%. The value/contestedness priority is deferred until it carries its own hysteresis (§11).
3. **Bounded centroid-cohesion** (`discohesion_coef=10` past `cohesion_k=3`) — feedback-free while the blob is tight (zero force within K), only pulling a straggler back. Tightened from the initial loose seed after EXP-POS-SELFPLAY-1 wanted pairwise ≤ 6; the tournament independently confirms tighter cohesion wins.
4. **`g`-floor blowout guard** + the hard **LETHAL survival veto** (below), both retained as designed.

Result: **single-room oscillation mean 3.13%** (worst, the open-skirmish Designed#0, 9.3%), all 15 harness gates + EXP register 10/10 green. The `KernelParams` seam is exactly this tuning surface.

**Blowout degeneracy (proposal D's real weakness, fixed here).** At `|μ|` extremes the sigmoid flattens, `W'(μ) → 0`, so `g_us`/`g_them` shrink and *all* EV terms collapse toward noise — the squad stops fighting intelligently exactly when winning/losing hard. Mitigation: **floor `g_us`/`g_them`** at a minimum slope, so even in a blowout the relative ordering of (kill A vs kill B vs heal vs safe tile) is preserved. The floor is a single EXP-* constant; the Lanchester gate still prevents committing to a losing fight, so the floor only governs *how* a decided fight is fought, never *whether*.

**Cross-room (out of scope, must-not-worsen).** `ThreatField` and the flood are room-scoped. Cross-room positioning is the known **93% oscillation** open problem (Designed#4); it is independent of this kernel. The kernel degrades to the existing `MoveToRoom` handoff at room borders (it does not score across the edge), so it neither helps nor worsens cross-room. The single-room gate is `≤ 0.5%`; Designed#4 may stay at its known value.

## 8. Build plan (clean replacement — no migration stages)

Per the build directives, this ships as one coherent replacement, not a flagged migration. Development still proceeds in **verifiable build steps** (each leaves the workspace compiling + the relevant tests green so the harness stays a usable gate), but the **end state is a single path with the old machinery deleted** — no feature flag, no legacy fallback, no back-compat parity. Gates throughout: `cargo clippy-wasm` clean, `cargo test -p screeps-combat-decision`/`-agent`, and `cargo test -p screeps-combat-eval --lib harness` (the ADR-0023a validators incl. **Designed#1/#3 bunker+guard breach and #4 cross-room must pass**, OracleCalibration FP/FN, PositioningOscillation single-room ≤ 0.5%, plus the new `action_oscillation_rate`).

**Status (2026-06-25): steps 1–3 LANDED + green; the EV kernel drives engaged combat in the sim.**
Engine `7b348e4` (intent-legality fix), decision `7f72516` (kernel + wired + dead machinery deleted),
agent `2a4f790` (consumes `member_intents`), super `fc841c8`. Gates: decision 126 / agent 50 / engine 52
/ eval 33 (harness 15 incl. designed-4 cross-room + bunker/breach #1/#3; single-room oscillation mean
**3.13%**; EXP register 10/10) — all green; clippy-wasm + host clippy clean. Remaining: step 4
(`action_oscillation_rate` metric — positional gate already passes), the **bot's per-creep action wiring**
to `member_intents` (positioning is already kernel-driven; the bot is undeployed), and the tournament tune.

Build steps (each a green checkpoint commit):

1. ✅ **Sim engine-correctness.** Patch `resolve.rs::filtered_actions` so the sim mirrors the real engine table (rangedHeal drops `rangedAttack`+`rangedMassAttack`; confirm dismantle/attackController drops) + a parity unit test vs `intents.js`. This makes the harness a faithful oracle for the kernel; it is sim-correctness, not back-compat.
2. ✅ **Kernel + currency (pure, with unit tests).** `enumerate_legal_sets` (incl. Dismantle + AttackController), `tile_action_ev`, the sigmoid-LUT `W` + `g_us`/`g_them`, the residual ledgers (creep kill-budget + structure value/breach-inherited + heal-need + controller). Unit tests: every emitted set passes the patched `filtered_actions`; `μ→W` monotone; worked examples (§4) pick the expected `(tile, action)`.
3. ✅ **Swap in + delete old.** `decide_squad_with_pathing` computes per-member `(goal, action-set)` via the kernel and returns them on `SquadDecision` (`member_goals` for all + a new `member_intents`). The agent (`ManagedSimSquad.step`) and the bot (`SquadManager`/`squad_combat`) consume `member_intents` directly — **no per-creep `decide_combat` pass for managed creeps** (`decide_combat` is kept only for the unmanaged/solo creep, itself a 1-member kernel call). Delete `MemberCaps::desired_range`/`order`/`is_support` + the healer preset path, `plan_squad_layout`'s role ordering, `apply_heal`'s veto + the attack pipelines, `assign_focus_fire`, `assign_heals`, and the now-dead `score_tile` presets. Green the full harness.
4. **Metric + validation.** Add `action_oscillation_rate` (A-B-A on the emitted `ActionSet`); confirm single-room positional oscillation ≤ 0.5% and the action metric ≤ its baseline; confirm bunker/breach (Designed#1/#3) and cross-room (#4) pass; spot-check replays.

**Tuning is out of scope for the build** (directive 3): land seed constants marked tournament-tunable, get the system fully clean + correct + green, then run the self-play tournament to tune the *complete* system. The ADR-0020 §10 Docker-soak → operator-go-ahead path still gates any MMO deploy; never deploy MMO without explicit go-ahead.

## 9. Alternatives considered

All four source proposals share the same spine (per-`(tile, action-set)` EV with shared residual budgets) and were judged *viable*. This ADR takes **A** as the spine and grafts the best of the others.

- **A — Per-tile action-EV greedy (CHOSEN spine).** Cleanest seam fit; the per-tile kernel + shared-residual drain is exactly the operator's "what the creep CAN DO in the slot". **Grafted as-is**: the kernel, `SELF_RISK_MELEE` (its strongest unique idea), the priced melee-vs-heal choice, `APPROACH_DEFICIT` over the flood. **Fixed**: its `{RMA,RangedHeal}` / `{RangedAttack,RangedHeal}` engine-illegal sets (§3); its integer-only claim (§7); the mortal-flag hysteresis gap (§7).
- **B — Joint-assignment auction.** Best engine-correctness write-up and the submodularity justification (grafted into §2.2). The full epsilon-auction with re-bid sweeps and lazy dirty-set recompute is **rejected as the default mechanism**: it is the lowest-simplicity, lowest-anti-oscillation option (a 2D `(tile, action)` re-bid over per-tick-churning residuals can re-aim every tick through no squad action), and its determinism story (lazy recompute order) is fragile. We adopt its *one bounded re-pass* (STAGE 3) and *spacing-as-constraint* framing, not the multi-round auction.
- **C — Short-horizon rollout.** The strongest win-probability fidelity and the "approach-under-fire / caught-en-route" cases are real. **Rejected for v1, kept as the §10 extension.** Reasons: (1) its load-bearing "future ThreatField" (offset enemy melee-reach inward by `t`) is an undefined operation on the static stamp, mishandles ranged (range-3) and towers (immobile — must NOT shift), so the horizon tail rests on the weakest input; (2) CPU `~21k–43k`/squad at K=2–3 is the worst tractability of the four; (3) it adds the same action-thrash axis with more surface. We adopt its cleanest idea — **`score_tile`/this-tick EV as the K=0 slice of a horizon function** — as the *structural framing* so a tail can be added later without a rewrite, and its `{A,RA}`-vs-`{H,RA}` priced heal comparison (already in A).
- **D — Marginal win-probability.** The **win-probability currency itself** — promoting `assess_engage`'s `μ` into the per-tile objective via a sigmoid LUT + `g_us`/`g_them` sensitivities — is the single best framing and is **grafted as §2.1** (it removes the "what is 1 mHP worth?" hand-wave that A left open: the answer is `g_us`/`g_them` from the *existing* tuned Lanchester model, introducing zero new exchange-rate constants). **Rejected from D**: its same `{RMA,RangedHeal}` engine-illegal combo (§3); its "contestedness sort" (members with fewest high-EV tiles first) — it is just claim-ordering with a different, *more* churn-prone sort key, so we use value-sort + optional swap instead; its un-floored sigmoid blowout degeneracy (fixed in §7); its linearized partial-kill chipping risk (mitigated by keeping `select_focus_target` as a concentration anchor, §11).

## 10. Future work / extensions (NOT in v1)

- **Short-horizon tail (graft C properly).** Add `FUTURE_K` discounted ticks to the kernel once a *correct* forward threat model exists: re-stamp chasers at their projected positions (not the hand-wavy inward-offset), keep towers immobile, keep ranged at range 3. Structural framing (this-tick = K=0 slice) is already in place.
- **Multi-squad shared residuals.** Residuals are per-squad. Two squads on one focus may under-fire (safe direction — never over-commit). Cross-squad ledgers are a P5 item.
- **Boosted-TOUGH threat field.** `ThreatField` stamps unboosted output; a win-probability currency compounds the mis-estimate through `W`. May need the boost field sooner than the weighted model did.

## 11. Open questions

1. **Chipping vs concentration.** Linearized partial-kill OFFENSE credit can let N creeps each chip a different high-threat enemy for fractional EV instead of concentrating to finish one (the square law's whole point). The residual budget bounds *overkill* but not *under-concentration*. v1 mitigation: keep `select_focus_target` as a soft concentration anchor (a small EV bonus for firing the shared focus) and consider super-linear OFFENSE near the kill boundary. Validate on the blob scenarios; decide whether the focus anchor stays or super-linear suffices.
2. **`g_us`/`g_them` staleness.** Computed once per squad per tick from the current `μ`; the within-pass residual drain shifts `μ` slightly. Once-per-squad is proposed (cheap); validate the staleness is harmless vs per-member refresh (`~N×` the sensitivity cost).
3. **Is STAGE 3 (re-pass) ever needed?** Greedy + incumbency is expected at a fixed point. Measure on the 50-scenario suite whether single-pass leaves measurable EV on the table before adding the swap.
4. **`KILL_HORIZON` and MORTAL multiplier magnitudes.** Seeds: `KILL_HORIZON ≈ 3`; a prevented death credits the ally's whole `dps × ehp`. The EXP-* sweep must calibrate the kill-vs-save symmetry (a kill removes enemy future strength; a save keeps ours — Lanchester suggests near-symmetry).
5. **Action dead-band magnitude** — does the joint `(tile, ActionSet)` band need a separate action-component magnitude, or does one band + mortal-flag hysteresis suffice without action-thrash? Settle via `action_oscillation_rate`.
6. **`our_dps` double-count audit.** `assess_engage` sums `melee_power + ranged_power` per member (lib.rs:968) then squares into `our_strength`. This is inherited unchanged, but the entire `W`/sensitivity surface now sits on top of it — audit whether the square law over an already-summed dps biases `g_us`/`g_them` before locking the sweep.
7. **Outcome-aware oscillation metric (operator, 2026-06-25).** `oscillation_rate` counts *all* period-2 movement, but beneficial A-B-A (dodging a tower volley, forcing a re-acquire, keeping a focus in range) is *good* and fatigue is free with enough MOVE (§7.1). Refine the metric to flag only **unnecessary** reversals — those that did not reduce incoming / increase dealt / improve the resulting EV vs holding. Until then the positional rate is a gross-jitter tripwire to be read *with* net-HP, never a standalone optimisation target.
8. **Tournament basket enrichment BEFORE adopting any tuning lead (roadmap 0020-S4-RES; operator-recalled).** *Partly done 2026-06-25.* **LANDED:** (a) a **random squad-composition population** (`harness::roster` — free-form body mixes within an energy budget) + (b) **Lanchester validation** (`roster::lanchester_validation`: predicted `predict_engage` vs actual sim outcome over a random population — **95.4% sign accuracy, 8 confidently-wrong outliers** surfaced = mispredicted/"broken" comps to inspect) + (c) a **comp-varied tournament basket** (`tournament::comp_basket` / `run_tournament_over_comps`: Bed × N random comps, both sides mirror the comp so a match isolates `KernelParams`). The diversity already shifted the ranking (fixed-ranged → k-tight-coh; comp-varied → k-spread tops mean payoff, k-tight-coh stays the robust Nash pick) — confirming the don't-adopt-off-a-thin-basket rule. **ALSO LANDED:** (d) the **base attack/defend lens** — `generate::realistic_bases()` (5 varied real bases: open tower-nest, corridor-choke+guard, thick-rampart bunker turtle w/ tower crossfire, mid bunker+2 towers, swamp turtle — terrain + structures + defenders) + `validate::assault_score` (objective-aware: HP razed + destroyed bonus + attacker survival) + `tournament::base_attack_ranking`. k-tight-coh tops base attack too (+4867), consistent with the open-combat Nash pick. **ALSO LANDED (e):** the base attacker is now **winnable-sized** (`choose_fielded_comp`, the force-sizing solver's breach force) + an **efficiency-weighted** assault score (razed + destroyed bonus + survival×2 − per-tick). **KEY FINDING:** with a winnable force the base breach is **position-INSENSITIVE** — every `KernelParams` config breaches the crackable bases identically. So **base attack/defend is a breach-CAPABILITY gate** (all pass), and positioning-param **discrimination lives in the open-combat comp-varied tournament** (k-spread mean-payoff lead, k-tight-coh robust Nash). The combined tuning *pass* runs over both lenses; **adoption is deferred to the post-realistic-rooms re-tune** (operator sequencing).
   **STILL TODO — realistic simulation worlds (operator-staged 2026-06-25):** (i) **terrain import only** (real shard terrain → the harness world); (ii) **single + multi-room variants** over imported terrain × a variety of **objectives** (farm/secure/breach/raze/declaim) × **squad compositions**; (iii) the same single/multi-room with **Foreman base planning layered** over the imported terrain (planner-generated realistic bases). Then the **final re-tune** over the full realistic basket + deliberate adoption. (literally-real layouts subsume the hand-authored `realistic_bases`; §8.6 turtle scorer + scripted-vs-managed + PFSP/Elo remain follow-ons.)
9. **Value/contestedness commit priority needs hysteresis.** §7.2 ships a *stable index* commit order because the value-sorted order churned (period-2 swaps). Re-introducing "highest-leverage/most-contested first" (better contention resolution) requires the order key to carry its own dead-band (e.g. incumbent-EV-weighted) so it can't flip tick-to-tick. Deferred until measured to matter.
10. **Cross-room positioning (the standing ~93% oscillation, Designed#4).** `ThreatField` + the approach flood are per-room, so at a room seam the kernel has no coherent cross-border gradient and the lead creep flip-flops at the edge. Needs the flood/threat stitched across the seam (or a cross-room strategic goal the local step homes to). Orthogonal to the kernel; the harness gate excludes it.
11. **Declaim targeting needs a controller in the decision view.** The `Declaim`/`AttackController` action is enumerated + priced, but `CombatStructureDto`/the squad view carry no controller, so it never fires in v1. Add the strategic enemy controller to the view (pos + downgrade ticks) to activate room-neutralization EV.
12. **Bot per-creep ACTION wiring to `member_intents`.** Bot *positioning* is kernel-driven (`member_goals` via `apply_squad_decision`); bot *actions* still use the `TickOrders` `attack_target`→`SquadCombatJob` path. Carry the kernel's `member_intents` through `TickOrders` + emit them in the job so a deploy uses the kernel end-to-end (bot is undeployed, so not harness-tested today).

## 12. Realistic simulation worlds — staged build plan (operator-staged 2026-06-25; mapped by workflow `wf_43948641`)

Turns §11 #8's "realistic simulation worlds" into a four-stage build, grounded in real harness/engine/foreman/rest-api seams. Spine: real **terrain** into a host `CombatWorld` → **single+multi-room × objective × comp** scenarios → **offline-cached foreman base plans** → **re-tune** + adopt a `KernelParams` winner.

**Build status (running ledger):** Stage 1 **LANDED** (eval `7a5b312`) — `harness/terrain_import.rs` decoder + `resources/real-terrain.json`. Stage 2 **LANDED** (eval `287a689`) — `ObjectiveKind` + kind-driven `run_until_for` + `ImportedRoom` generator (single+multi-room × 5 kinds × comps).

**Coordinate mismatch — ROOT-CAUSED + FIXED** (eval `279006b`): from first principles vs the screeps bindings, the terrain decode is **correct** (row-major `y*50+x` — proven by `screeps-game-api` `LocalRoomTerrain`'s own `addresses_data_in_row_major_order` test + the rest-api `TerrainEntry` doc + the exit-gap border pattern; only `LocalCostMatrix` is column-major `x*50+y`). The **dump's object coords are the bug**: every object (7883/7883 across 3000 rooms) is exactly Chebyshev-distance 1 from open under the correct decode — systematically nudged one tile into wall-edges; NO rigid transform recovers them (identity→100% on-wall, every flip/rotation→~28%≈chance). Fix: `snap_to_open()` snaps each object to its nearest open tile at load (recovers the true adjacent tile, keeps objects distinct), preserving the room's real layout. Regenerated `real-terrain.json` to **13 varied rooms**. **The offset itself is SUSPECT + not root-caused — documented for revisit in [ADR 0025a](0025a-coordinate-offset-anomaly.md)** (the snap is a sound workaround; the blast radius is narrow — only the foreman seed positions, never the terrain or planned structures).

Stage 3 **LANDED** — `harness/foreman_capture.rs` (the reusable capture lib: `CapturedBase`/`CapturedStructure` + `capture()` calling `screeps_foreman::planner::plan_room` — the planner is a clean LIBRARY, NOT the bench binary, so no binary dependency) + the `capture_base` bin (offline, `CARGO_MANIFEST_DIR`-anchored, incremental) → committed `resources/captured-bases.json` cache (13 real bases) + `ForemanGenerator` (loads cache, never plans) with `realize_base` + adaptive `breach_from_ramparts` (breach point = the real rampart ring's tile nearest a navigable entry). `decode_fast`/`fast_to_combat` bridge added.

Stage 4 **LANDED (machinery) + INITIAL RE-TUNE RUN** (`tournament.rs`): `Bed::Imported(idx)` (mirror-symmetrized real terrain) + `realistic_comp_basket` (synthetic + imported beds) + `realistic_base_scenarios` (foreman + imported `Raze` bases) + the `realistic_kernel_tournament`/`realistic_base_attack` dashboards. **The heavy sims are rayon-parallelized** (tournament cells, base-attack `(strategy×base)` pairs, exploitability) — the full re-tune runs in ~5s. **KEY FINDINGS (the re-tune result):**
- **Open combat (21 beds):** the shipped default `k-default` is **robust — exploitability 431 net HP ≪ GROSS 1500** (no hard counter). Field/Nash leaders are `k-spread`/`k-tight-coh`, but they each REGRESS base-attack, so the §12 adoption protocol (no base-attack regression) does **not** adopt them — the default holds for open combat.
- **Base attack (26 real foreman+imported Raze bases) — OVERTURNS §11 #8:** base-attack is **strongly position-SENSITIVE** on realistic bases (it was position-insensitive on synthetic beds). `k-approach-hot` (approach_coef 4) **dominates: +21154 vs every other config deeply negative (~−25k to −29k)** — the default kernel *chips and loses creeps* instead of breaching real foreman rampart rings; approaching hard breaches them. But `k-approach-hot` is WORST in open combat (−118 mean).
- **Adoption:** no single `KernelParams` wins both lenses → an **objective/information-dependent strategy-selection layer** picks the weight profile per objective (designed in **ADR 0026**). Shipped default unchanged pending that layer.

**THOROUGH re-tune (operator-requested many-minutes run, `realistic_full_retune`: 48-config grid × 56 beds × 52 Raze+Breach bases, rayon, ~20min) — REVISES the quick-run finding:**
- **Open combat:** the optimum is **low-approach / high-incumbency / TIGHT-cohesion** — `a1-i6-tight` (approach 1, incumbency 6, tight) wins with **exploitability 0** (unexploitable). The shipped default (`a2-i3-def`) is middling (#21/48, exploit 313). So a real open-combat improvement exists — and it is NOT "spread", it is hold-tight-at-range.
- **Base attack (Raze+Breach, winnable forces):** weakly discriminating (all winnable forces breach; scores cluster +814k–+883k); LOW approach slightly wins on survival. **`approach-hot` (a4) did NOT replicate its quick-run dominance** — a4 configs are middling-to-poor at scale. The quick-run "+21154 approach=4 dominates" was an artifact of the small Raze-only basket; at scale, approach stays LOW (winners are approach 1–2).
- **Best BALANCED:** `a2-i4-tight` (open #6, base #4) — a modest tweak from the default (incumbency 3→4, cohesion def→tight, approach unchanged at 2).
- **Net:** objective-awareness still helps (best-open `a1-i6-tight` is poor at base #45; best-base `a1-i4-def` is mediocre at open #20), but the discriminating levers are **incumbency + cohesion**, not approach. **ADR 0026's `breach_hot` seed (approach=4) is superseded** — the breach profile tuning (ADR 0026 Step 4) should sweep incumbency/cohesion with approach low, and the open-combat default candidate is `a2-i4-tight`/`a1-i6-tight`. **Re-tune machinery clean + reusable.**

**Global constraints:** host-only (everything in `screeps-combat-eval`, wasm-excluded; no live `game::*` at sim time). Deterministic (`Rng::seeded(index)`; no Date/random/network/fs inside `generate()`). **Foreman is SLOW** (~3.6s typical, up to 55s; docs/0009a §2) → runs **offline once → cached JSON**, never in `generate()`/`validate()`/a tournament loop. No new engine types needed except the Stage-2 objective tag (`CombatTerrain`/`CombatWorld.rooms`/`SimStructure`/`SimTower`/`SimController` exist, engine `state.rs:11-176`).

### Stage 1 — Terrain import only
Get real shard terrain into a host `CombatWorld` + prove a squad navigates it. **Use committed encoded-terrain fixtures (a `const` table), NOT a live fetch** — a room is a 2500-char digit string (`0`plain/`1`,`3`wall/`2`swamp; rest-api `types.rs:156`); the rest-api `room_terrain_encoded` (`client.rs:387`) is async+HTTP+360/hr-capped → deferred to the Stage-3a offline tool. **Blocker:** no committed fixtures yet → fallback: decode a room from `screeps-foreman-bench/resources/map-*.json` (identical encoding, decoded at `bench/src/main.rs:464-501`). NEW `harness/terrain_import.rs`: `decode_terrain(&str)->CombatTerrain` (inverse of the bench visitor; `walls.insert`/`swamps.insert`, `state.rs:11-29`), `fast_to_combat(&FastRoomTerrain)->CombatTerrain` (the cross-crate bridge; `terrain.rs:39-49`), `decode_fast`, `TerrainFixture { room, terrain, controller, sources, mineral }` + `FIXTURES`. Add `screeps-foreman` dep (Stage 3 needs it). Tests: `decode_roundtrips_a_known_pattern`, `fast_to_combat_matches_decode`, **`imported_terrain_is_navigable`** (the operator's smoke test — `ManagedSquadIntegration::validate` over a fixture passes; render via `render_managed_replay`). **Foundation — land first.**

### Stage 2 — Single + multi-room × objectives × comps
An `ImportedRoom` `Generator` over imported terrain, single + multi-room (ADR 0023a S3 `in_room`/`terrain_for`), parameterized by objective kind × comp. **Close the objective-kind gap first** (today `Objective` is a hard-coded "destroy spawn"; `EngageObjective` only Destroy/Hold): add a generation-side `enum ObjectiveKind { Farm, Secure, Breach, Raze, Declaim }` + `Objective.kind` (`scenario.rs`), each mapped to existing `RunUntil`/`EngageObjective`/world-population (Raze=`ObjectivesDestroyed`; Breach=rampart-falls; Secure=`SideWiped(defender)`; Farm=survive-N+`Hold`; Declaim=push `SimController`+ new `ControllerNeutralized` RunUntil). `ImportedRoom { multi_room, n_comps }` decodes `index→(fixture, kind, comp_seed)` like `Permutations`; **vary the DEFENDER comp via `roster::random_squad`**, leave attacker variety to the tournament `comp_basket`. Multi-room mirrors `twin_room_siege` (`generate.rs:400`). Tests: `imported_room_every_kind_is_assessable`, `imported_declaim_has_a_controller`, `imported_room_navigable`, `multi_room_imported_crosses_border` (gate on *reach*, not the known ~93% cross-room oscillation). **After Stage 1.**

### Stage 3 — Foreman-layered realistic bases
**3a (offline, cached, committed):** reuse the bench `plan_room(&data_source)` path (`bench/src/main.rs:259-335`, `CpuBudget::unlimited()`) with a new output mode emitting `CapturedBase { room, terrain, controller, structures: Vec<CapturedStructure{kind,x,y}> }` — iterate `Plan::structures` (`plan.rs:267`), map `RoomItem.structure_type`→Spawn/Tower/Rampart/Wall (drop roads/extensions/labs), write one JSON per room to committed `resources/captured-bases/`. **Run once, manually, never in CI.** (Live-shard variant = same tool via `room_terrain_encoded`+`room_objects`, credential-gated, Phase G.) **3b (fast — loads cache, never plans):** `ForemanGenerator::new(dir, n_comps)` loads the JSONs; `realize_base(&CapturedBase, kind, comp_seed)->Scenario` decodes terrain + pushes `SimStructure`/`SimTower` (towers energized 100k, the calibration convention) via `ScenarioBuilder`. **New `breach_from_ramparts(core, &ramparts, &terrain)`** — the synthetic `breach_geometry` assumes one west rampart; derive staging from the real rampart ring (shortest BFS flood-fill to a room exit, `terrain.rs:315/52`). Tests: `foreman_cache_realizes`, `foreman_base_is_assessable`, `foreman_breach_geometry_is_in_range`, `#[ignore] write_foreman_replays`. **After Stages 1+2; 3b can land with a checked-in sample cache.**

### Stage 4 — Final re-tune over the full realistic basket
Widen the EXISTING tuning machinery (`tournament.rs`), build no new mechanism: extend `Bed` with `Imported(usize)` (its `apply_bed_terrain` copies a fixture + mirrors cached towers both ends) for the open-combat `run_tournament_over_comps`; point `base_attack_ranking` at `ForemanGenerator`+`ImportedRoom` scenarios (`assault_score` unchanged); add a terrain-aware `lanchester_validation` variant. **Adoption protocol:** (1) realistic kernel tournament → mean-payoff + meta-Nash; (2) realistic base-attack = no-regression capability gate (position-insensitive with a winnable force, §11 #8); (3) pick the **robust** (lowest-exploitability/Nash) config, re-run `exploitability ≤ GROSS`; (4) edit `KernelParams::default()` + re-green oracle-calibration / single-room-oscillation / self-play-decisive / Lanchester-floor / action-oscillation — a `KernelParams` change is a decision-crate constant, **no `WORLD_FORMAT_VERSION` bump**; (5) record the adopted constants + ranking in this ledger. MMO deploy stays gated on ADR-0020 §10 Docker-soak → operator go-ahead. **Last — requires Stages 1–3.**

**Dependency graph:** `Stage1 → Stage2 → Stage3a → Stage3b → Stage4`.
**Blockers/fallbacks:** (1) no committed fixtures → decode a bench `map-*.json` room; (2) live import credential/rate-capped → terrain from fixtures, foreman supplies structures offline (live `room_objects` import is the off-critical-path Phase G); (3) foreman is single-room → plan per room + compose with `in_room` (inter-room roads are cosmetic in the combat sim); (4) kernel doesn't yet target controllers (§11 #11) → Stage-2 `Declaim` is *scored* by neutralization, goes fully live when #11 lands.
