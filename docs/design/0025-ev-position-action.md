# ADR 0025 — Unified EV-of-(Position × Action) Per-Creep Combat Decision

- **Status:** Decided
- **Date:** 2026-06-24
- **Supersedes (mechanism):** the role/`MemberCaps`-driven positioning in ADR 0024; the separate `decide_combat` action pipeline; `assign_focus_fire` + `assign_heals` + the `score_tile` engage/healer presets as the layout mechanism. These are **deleted**, not flagged off.
- **Builds on (kept):** ADR 0019 (`score_tile` term math, as `mhp` scalars), ADR 0020 (Lanchester engage gate, EV target kill-budget + spill, force sizing), ADR 0024 (hierarchical positioning: shared threat field + reachability flood + one target-flood).
- **Crates:** `screeps-combat-decision` (kernel), `screeps-combat-agent` (adapter), `screeps-combat-eval` (harness).

> **Where the tuned constants live.** `KernelParams::default()` (`kernel.rs`) is a seed, not the
> adoption vehicle: the per-objective winners of the §12 re-tune live in the ADR 0026
> strategy-selection layer, which is what a fighting squad actually uses
> (`decide_strategy(default_strategies())` → `open_combat()` / `breach()`). The default is reached
> only from tests and the never-fights fallback.

### Build directives (operator direction)

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
| **DENIAL** | As built there is NO separately booked kill swing: every hit is priced linearly at `value_per_hit = g_them · threat_value(target) / residual_budget`, so the last hit that empties a target's budget is worth exactly its share and removing a healer's future output still beats chipping a tank (the healer's `threat_value` is higher and its residual smaller). The once-per-target `KILL_HORIZON` swing described here originally was never implemented (`kernel.rs` records it as a tournament refinement — corrected 2026-09-07, see Design deltas → A). | `threat_value`/`ttk` (lib.rs), `plan_squad_ev` damage ledger (kernel.rs) |
| **HEAL** | *(As built since 2026-08-24 — RULING-9; the original row read `g_us · Σ min(heal_output, residual_heal_need)` with a whole-fighting-strength MORTAL credit.)* Each healed HP is priced at the ally's **progress-diluted** `value_per_hp = g_us · member_output / horizon_hits` — the exact mirror of the attack side's `value_per_hit` — drained against TWO ledgers (URGENT anticipated incoming first, then BACKLOG real deficit), × `MORTAL_HEAL_MULT` (4) when `incoming ≥ hits`. Design deltas → A. | `kernel.rs` `HealTarget`, `apply_act` `Act::Heal` |
| **RISK** | *(As built since 2026-08-24.)* `net = max(0, ThreatField.raw_at(t) − deliverable_heal(t))` — the heal that can actually LAND on `t` next tick, not the squad total — priced at the member's own **marginal** `value_per_hp` (`g_us.max(unit) · my_out / max(raw_at(t), hits / SURVIVAL_HORIZON)`). Plus the hard **`LETHAL`** backstop when `net · SURVIVAL_HORIZON > member.hits` — the binary survival veto is **kept as a floor** under the graduated curve. Design deltas → A/B. | `kernel.rs` `best_tile`, `deliverable_heal` |
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
  - per ally, TWO heal ledgers (as built): `inc` = anticipated incoming at its tile (evidence-gated, ÷4 when unwounded) and `deficit` = `hits_max − hits`, plus its `value_per_hp` price — Design deltas → A.

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

**(c) Siege / dismantle creep, tower-drain room.** Against a wall/rampart blocking the focus, `{Dismantle}` (drops melee, legal alone) scores OFFENSE against the **structure** (breach progress) priced via the same `g_them`; `DENIAL` fires when the rampart breaks and the focus behind it becomes killable. `RISK` from energized towers is the `assess_engage` `tower_dps` drain folded into the threat field. The creep picks the breach tile + `Dismantle` because that is the highest-EV `(tile, action-set)` — a "siege role" emerges with no taxonomy. (The structure/breach/declaim pricing this leans on is §2.4 — in scope by build directive 2, not an extension.)

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
- Tie-breaks are total and explicit: `(EV desc, then approach distance asc, then x, then y)` for the tile (`best_tile`'s key is `(cost, Reverse(d), Reverse(x), Reverse(y))` — the approach-distance leg was added 2026-08-24 because a lower-x/lower-y tie-break is objective-blind on a cost plateau, see Design deltas → D), and enumeration order for the action-set. Residual ledgers are `Vec`-indexed (no `HashMap` iteration-order nondeterminism). The shared target-flood result is iterated in sorted-key order. A determinism unit test asserts identical output on repeated runs (as `layout_is_deterministic` does today).

**Anti-oscillation (must not regress single-room ~0.5%).** Two new degrees of freedom vs the positional baseline: action choice, and the commit order.

1. **Joint dead-band.** The `LAYOUT_DEAD_BAND` incumbency bonus applies to the **joint `(tile, ActionSet)` pair**: last tick's chosen pair gets a `−band` cost, so neither the tile nor the action flips unless beaten by `> band`.
2. **Mortal-flag hysteresis (the multiplicative-threshold fix).** A fixed-cost band does **not** damp a *multiplicative* swing: when an ally's HP crosses the MORTAL threshold the HEAL credit jumps by the whole-fighting-strength factor, easily exceeding any fixed band → a healer-capable fighter flips `{Attack,RangedAttack} ↔ {RangedAttack,Heal}` on a knife-edge ally. Mitigation: apply hysteresis to the **MORTAL flag itself** (the ally is "mortal" for `assign`/credit purposes with a small HP dead-zone around `incoming = hits`), not only to the resulting EV. This is the explicit fix the adversarial review flagged that the source proposals missed.
3. **Order stability.** The value-sorted commit order can churn if a creep flips across the sort comparator tick-to-tick. Mitigation: the order key includes the incumbent EV (which carries the dead-band), so a stable creep stays stable in the order. (The rejected "contestedness sort" — §9 — is *more* churn-prone because the contested/uncontested boundary is a hard threshold; that is one reason it is rejected in favour of value-sort + optional swap.)
4. **New metric.** `oscillation_rate` (metrics.rs:291) is **purely positional** and cannot see action-thrash (a creep standing still while flipping Attack↔Heal scores 0.0). We therefore **add an `action_oscillation_rate` metric** (A-B-A on the emitted `ActionSet` per creep) and gate Stage 3+ on it ≤ a baseline-derived threshold. Gating only on positional oscillation would be blind to the regression this design is most likely to cause.

### 7.1 Oscillation is a PROXY, not the objective

**Oscillation is not inherently bad.** Period-2 movement that *improves the outcome* is correct behaviour: stepping in-and-out of a tower's optimal-damage band, jinking to force a tower/defender to re-acquire (wasting its shot), or shuffling to keep a focus in weapon range while denying the enemy a clean line — these are *beneficial* A-B-A patterns. **Fatigue is not a cost** for a properly-built creep (enough MOVE parts → it moves every tick for free), so "it moved a lot" is not itself a problem.

What we actually want to minimise is **UNNECESSARY oscillation** — movement that does *not* improve win-probability EV (the seam-cycling, swap-churn, and frozen-then-twitch failures in §7.2): motion that burns the tick without buying damage dealt, damage avoided, or a better next-tick position. So:

- The **primary** judge is the **EV / net-HP outcome** (the tournament, §9 ranking), not the oscillation count. A config that "oscillates more but wins more" is *better*.
- `oscillation_rate` stays as a **cheap regression tripwire** for *gross* unnecessary jitter (the 85%-swap-churn class), not a hard optimisation target. Its threshold is a sanity bound, not a goal — do not tune *toward* a lower number at the expense of net HP.
- **Future metric refinement (open question §11):** make the metric *outcome-aware* — only count an A-B-A as "unnecessary" when the reversal did **not** reduce incoming damage / increase dealt damage / improve the EV of the resulting position vs holding. That separates beneficial jinking from confused twitching. Until then, read the positional rate together with the net-HP outcome, never alone.

### 7.2 The adopted stabilisers — and why they differ from the planned set

The planned damping above (joint dead-band, mortal-flag hysteresis, incumbent-EV-weighted order) was partly superseded once the kernel ran against the harness; the **adopted** stabilisers (all seeds in `KernelParams`, tournament-tunable) are:

1. **Always-on approach pull + offense-gated incumbency (the regime split).** The approach gradient (`−unit·approach_coef·D[tile]`, `approach_coef=2`) is applied at **every** tile, *not* only when out of range. The incumbency dead-band (`+unit·incumbency_coef`, `=3`) is applied **only at a tile where an OFFENSE action lands** (`offense_reachable`). Rationale, learned the hard way: a heal-capable creep has a "doable action" (heal an at-risk ally) almost everywhere, so a doable-gated incumbency *froze a melee+heal siege at range 5* — it healed in place and never advanced. Splitting it — *approach always pulls; incumbency only brakes once you're actually fighting* — fixes both the freeze (approach wins while closing, `approach_coef > 0` with no incumbency competing) and the engaged jitter (strong incumbency where the fight is, where OFFENSE's huge `g_them·damage` already dominates so the brake costs nothing).
2. **Stable commit order (member index), NOT value-sorted.** The §9 "highest-leverage first" value-sort **churned**: its comparator depends on per-tick residuals/positions, so the order flipped tick-to-tick, reassigning contested tiles between creeps → a swap-driven period-2 (measured: self-play hit **85%**). A stable index order resolves contention deterministically and dropped it to ~3.6%. The value/contestedness priority is deferred until it carries its own hysteresis (§11).
3. **Bounded centroid-cohesion** (`discohesion_coef=10` past `cohesion_k=3`) — feedback-free while the blob is tight (zero force within K), only pulling a straggler back. Tightened from the initial loose seed after EXP-POS-SELFPLAY-1 wanted pairwise ≤ 6; the tournament independently confirms tighter cohesion wins.
4. **`g`-floor blowout guard** + the hard **LETHAL survival veto** (below), both retained as designed.

With these four in place single-room oscillation sits around **3%** (worst case, the open-skirmish bed, ~9%) — the band the design targets. The `KernelParams` seam is exactly this tuning surface.

**Blowout degeneracy (proposal D's real weakness, fixed here).** At `|μ|` extremes the sigmoid flattens, `W'(μ) → 0`, so `g_us`/`g_them` shrink and *all* EV terms collapse toward noise — the squad stops fighting intelligently exactly when winning/losing hard. Mitigation: **floor `g_us`/`g_them`** at a minimum slope, so even in a blowout the relative ordering of (kill A vs kill B vs heal vs safe tile) is preserved. The floor is a single EXP-* constant; the Lanchester gate still prevents committing to a losing fight, so the floor only governs *how* a decided fight is fought, never *whether*.

**Cross-room.** `ThreatField` and the flood are room-scoped. *(Corrected 2026-09-07 — this paragraph used to call cross-room "the known 93% oscillation open problem"; that is no longer true.)* The kernel now prices the seam explicitly: it is anchored on the **fight room** (the focus room, else the centroid room — never a straddling centroid), scores room-edge tiles as transitional (`EXIT_EDGE_PENALTY`), assesses towers room-locally, and hands out-of-contact members to the block mover (see **Design deltas → C**). Designed#4 measures ~0.6% period-2 (from 99.6% at its worst and ~93% originally); the harness keeps a `≤ 0.97` not-fully-regressed bound on the cross-room bed (`screeps-combat-eval/src/harness/mod.rs`, `PositioningOscillation`). What remains OPEN is the design item, not a defect: the threat/approach field is still built per room, so there is no seam-stitched gradient — a member on one side prices the other side's threat as zero (§11 #10, ADR 0024 follow-up).

## 8. Build plan (clean replacement — no migration stages)

Per the build directives, this ships as one coherent replacement, not a flagged migration. Development still proceeds in **verifiable build steps** (each leaves the workspace compiling + the relevant tests green so the harness stays a usable gate), but the **end state is a single path with the old machinery deleted** — no feature flag, no legacy fallback, no back-compat parity. Gates throughout: `cargo clippy-wasm` clean, `cargo test -p screeps-combat-decision`/`-agent`, and `cargo test -p screeps-combat-eval --lib harness` (the ADR-0023a validators incl. **Designed#1/#3 bunker+guard breach and #4 cross-room must pass**, OracleCalibration FP/FN, PositioningOscillation single-room ≤ 0.5%, plus the new `action_oscillation_rate`).

Build steps (each a self-contained, green checkpoint):

1. **Sim engine-correctness.** Patch `resolve.rs::filtered_actions` so the sim mirrors the real engine table (rangedHeal drops `rangedAttack`+`rangedMassAttack`; confirm dismantle/attackController drops) + a parity unit test vs `intents.js`. This makes the harness a faithful oracle for the kernel; it is sim-correctness, not back-compat.
2. **Kernel + currency (pure, with unit tests).** `enumerate_legal_sets` (incl. Dismantle + AttackController), `tile_action_ev`, the sigmoid-LUT `W` + `g_us`/`g_them`, the residual ledgers (creep kill-budget + structure value/breach-inherited + heal-need + controller). Unit tests: every emitted set passes the patched `filtered_actions`; `μ→W` monotone; worked examples (§4) pick the expected `(tile, action)`.
3. **Swap in + delete old.** `decide_squad_with_pathing` computes per-member `(goal, action-set)` via the kernel and returns them on `SquadDecision` (`member_goals` for all + `member_intents`). The agent (`ManagedSimSquad.step`) and the bot (`SquadManager`/`squad_combat`) consume `member_intents` directly — **no per-creep `decide_combat` pass for managed creeps** (`decide_combat` is kept only for the unmanaged/solo creep, itself a 1-member kernel call). Delete `MemberCaps::desired_range`/`order`/`is_support` + the healer preset path, `plan_squad_layout`'s role ordering, `apply_heal`'s veto + the attack pipelines, `assign_focus_fire`, `assign_heals`, and the now-dead `score_tile` presets. Green the full harness.
4. **Metric + validation.** Add `action_oscillation_rate` (A-B-A on the emitted `ActionSet`); confirm single-room positional oscillation ≤ 0.5% and the action metric ≤ its baseline; confirm bunker/breach (Designed#1/#3) and cross-room (#4) pass; spot-check replays.

**Tuning is out of scope for the build** (directive 3): land seed constants marked tournament-tunable, get the system fully clean + correct + green, then run the self-play tournament to tune the *complete* system. The ADR-0020 §10 Docker-soak → operator-go-ahead path gates any MMO deploy; never deploy MMO without explicit go-ahead.

## 9. Alternatives considered

All four source proposals share the same spine (per-`(tile, action-set)` EV with shared residual budgets) and were judged *viable*. This ADR takes **A** as the spine and grafts the best of the others.

- **A — Per-tile action-EV greedy (CHOSEN spine).** Cleanest seam fit; the per-tile kernel + shared-residual drain is exactly the operator's "what the creep CAN DO in the slot". **Grafted as-is**: the kernel, `SELF_RISK_MELEE` (its strongest unique idea), the priced melee-vs-heal choice, `APPROACH_DEFICIT` over the flood. **Fixed**: its `{RMA,RangedHeal}` / `{RangedAttack,RangedHeal}` engine-illegal sets (§3); its integer-only claim (§7); the mortal-flag hysteresis gap (§7).
- **B — Joint-assignment auction.** Best engine-correctness write-up and the submodularity justification (grafted into §2.2). The full epsilon-auction with re-bid sweeps and lazy dirty-set recompute is **rejected as the default mechanism**: it is the lowest-simplicity, lowest-anti-oscillation option (a 2D `(tile, action)` re-bid over per-tick-churning residuals can re-aim every tick through no squad action), and its determinism story (lazy recompute order) is fragile. We adopt its *one bounded re-pass* (STAGE 3) and *spacing-as-constraint* framing, not the multi-round auction.
- **C — Short-horizon rollout.** The strongest win-probability fidelity and the "approach-under-fire / caught-en-route" cases are real. **Rejected for v1, kept as the §10 extension.** Reasons: (1) its load-bearing "future ThreatField" (offset enemy melee-reach inward by `t`) is an undefined operation on the static stamp, mishandles ranged (range-3) and towers (immobile — must NOT shift), so the horizon tail rests on the weakest input; (2) CPU `~21k–43k`/squad at K=2–3 is the worst tractability of the four; (3) it adds the same action-thrash axis with more surface. We adopt its cleanest idea — **`score_tile`/this-tick EV as the K=0 slice of a horizon function** — as the *structural framing* so a tail can be added later without a rewrite, and its `{A,RA}`-vs-`{H,RA}` priced heal comparison (already in A).
- **D — Marginal win-probability.** The **win-probability currency itself** — promoting `assess_engage`'s `μ` into the per-tile objective via a sigmoid LUT + `g_us`/`g_them` sensitivities — is the single best framing and is **grafted as §2.1** (it removes the "what is 1 mHP worth?" hand-wave that A left open: the answer is `g_us`/`g_them` from the *existing* tuned Lanchester model, introducing zero new exchange-rate constants). **Rejected from D**: its same `{RMA,RangedHeal}` engine-illegal combo (§3); its "contestedness sort" (members with fewest high-EV tiles first) — it is just claim-ordering with a different, *more* churn-prone sort key, so we use value-sort + optional swap instead; its un-floored sigmoid blowout degeneracy (fixed in §7); its linearized partial-kill chipping risk (mitigated by keeping `select_focus_target` as a concentration anchor, §11).

## 10. Future work / extensions (NOT in v1)

- **Short-horizon tail (graft C properly).** Add `FUTURE_K` discounted ticks to the kernel once a *correct* forward threat model exists: re-stamp chasers at their projected positions (not the hand-wavy inward-offset), keep towers immobile, keep ranged at range 3. Structural framing (this-tick = K=0 slice) is already in place.
- **Multi-squad shared residuals.** Residuals are per-squad. Two squads on one focus may under-fire (safe direction — never over-commit). Cross-squad ledgers are a P5 item.
- **Boosted-TOUGH threat field.** *(Refreshed 2026-09-07.)* The boost field now EXISTS on the decision view — `CombatBodyPart.boost_mult` + `CombatCreepDto::effective_output` (ADR 0041 delta) — and every creep-side consumer of this kernel is boost-aware: `threat_value`, `heal_reaching`, `kite_threats` (so `ThreatField::build` receives boosted attack/ranged output; the `kite.rs` module doc still calls itself "unboosted" — stale wording, the inputs are boosted). What is still NOT modelled is boosted **TOUGH** damage *reduction* on the receiving side: the field stamps raw output, so a boosted-TOUGH defender is over-counted (conservative) and our own boosted-TOUGH member's risk is over-priced. Open until a fight shows it matters.

## 11. Open questions

1. **Chipping vs concentration.** Linearized partial-kill OFFENSE credit can let N creeps each chip a different high-threat enemy for fractional EV instead of concentrating to finish one (the square law's whole point). The residual budget bounds *overkill* but not *under-concentration*. v1 mitigation: keep `select_focus_target` as a soft concentration anchor (a small EV bonus for firing the shared focus) and consider super-linear OFFENSE near the kill boundary. Validate on the blob scenarios; decide whether the focus anchor stays or super-linear suffices.
2. **`g_us`/`g_them` staleness.** Computed once per squad per tick from the current `μ`; the within-pass residual drain shifts `μ` slightly. Once-per-squad is proposed (cheap); validate the staleness is harmless vs per-member refresh (`~N×` the sensitivity cost).
3. **Is STAGE 3 (re-pass) ever needed?** Greedy + incumbency is expected at a fixed point. Measure on the 50-scenario suite whether single-pass leaves measurable EV on the table before adding the swap.
4. **`KILL_HORIZON` and MORTAL multiplier magnitudes.** Seeds: `KILL_HORIZON ≈ 3`; a prevented death credits the ally's whole `dps × ehp`. The EXP-* sweep must calibrate the kill-vs-save symmetry (a kill removes enemy future strength; a save keeps ours — Lanchester suggests near-symmetry).
5. **Action dead-band magnitude** — does the joint `(tile, ActionSet)` band need a separate action-component magnitude, or does one band + mortal-flag hysteresis suffice without action-thrash? Settle via `action_oscillation_rate`.
6. **`our_dps` double-count audit.** `assess_engage` sums `melee_power + ranged_power` per member (lib.rs:968) then squares into `our_strength`. This is inherited unchanged, but the entire `W`/sensitivity surface now sits on top of it — audit whether the square law over an already-summed dps biases `g_us`/`g_them` before locking the sweep.
7. **Outcome-aware oscillation metric (operator, 2026-06-25).** `oscillation_rate` counts *all* period-2 movement, but beneficial A-B-A (dodging a tower volley, forcing a re-acquire, keeping a focus in range) is *good* and fatigue is free with enough MOVE (§7.1). Refine the metric to flag only **unnecessary** reversals — those that did not reduce incoming / increase dealt / improve the resulting EV vs holding. Until then the positional rate is a gross-jitter tripwire to be read *with* net-HP, never a standalone optimisation target.
8. **The tournament basket must be enriched BEFORE any tuning lead is adopted (roadmap 0020-S4-RES).** A thin basket produces a confident, wrong winner — measured directly: widening from a fixed-ranged bed to a comp-varied one *changed* the ranking (`k-tight-coh` displaced the fixed-ranged leader; with comp variation `k-spread` tops mean payoff while `k-tight-coh` remains the robust Nash pick). The basket therefore carries: (a) a **random squad-composition population** (`harness::roster` — free-form body mixes within an energy budget); (b) **Lanchester validation** (`roster::lanchester_validation`: predicted `predict_engage` vs actual sim outcome over that population — ~95% sign accuracy, and the confidently-wrong outliers it surfaces are the mispredicted comps worth inspecting); (c) a **comp-varied tournament basket** (`tournament::comp_basket` / `run_tournament_over_comps`: Bed × N random comps with both sides mirroring the comp, so a match isolates `KernelParams`); (d) a **base attack/defend lens** — varied bases (open tower-nest, corridor-choke + guard, thick-rampart bunker turtle with tower crossfire, mid bunker + 2 towers, swamp turtle: terrain + structures + defenders) scored by an objective-aware `assault_score` (HP razed + destroyed bonus + attacker survival); and (e) a **winnable-sized** attacker (the force-sizing solver's breach force) with an efficiency-weighted assault score (razed + destroyed bonus + survival×2 − per-tick). Against *synthetic* beds a winnable force makes the breach position-INSENSITIVE — every `KernelParams` config cracks them alike — so on those beds the base lens is a breach-CAPABILITY gate and the discrimination lives in the open-combat comp-varied tournament. (§12 shows this does not carry over to realistic foreman bases, where position-sensitivity returns.) Adoption always waits for the full realistic basket (§12).
9. **Value/contestedness commit priority needs hysteresis.** §7.2 ships a *stable index* commit order because the value-sorted order churned (period-2 swaps). Re-introducing "highest-leverage/most-contested first" (better contention resolution) requires the order key to carry its own dead-band (e.g. incumbent-EV-weighted) so it can't flip tick-to-tick. Deferred until measured to matter.
10. **Seam-stitched threat/approach field (Designed#4 — the remaining cross-room DESIGN item).** *(Reworded 2026-09-07; the "standing ~93% oscillation" this line used to describe was closed by the 2026-08-24 border-crossing work — Designed#4 now sits ~0.6%, see §7 and Design deltas → C.)* `ThreatField` + the approach flood are still built per room, so a member standing on one side of a seam prices the far side's towers/creeps at zero and the far side's approach gradient does not exist until it crosses. The fight-room anchoring, exit-edge pricing and room-local tower assessment make crossings work; a stitched field would make them *priced*. The harness pin for this bed is `≤ 0.97` (not-fully-regressed), deliberately loose — a kernel design item (ADR 0024 follow-up), not a harness closeout (WS-CLOSE D4).
11. **Declaim targeting needs a controller in the decision view.** The `Declaim`/`AttackController` action is enumerated + priced, but `CombatStructureDto`/the squad view carry no controller, so room-neutralization EV cannot be scored. The strategic enemy controller (pos + downgrade ticks) belongs on the view; whether the view should also carry the reservation/ownership state the EV would need is the undecided part.

## 12. Realistic simulation worlds — staged build plan

Turns §11 #8's "realistic simulation worlds" into a four-stage build, grounded in real harness/engine/foreman/rest-api seams. Spine: real **terrain** into a host `CombatWorld` → **single+multi-room × objective × comp** scenarios → **offline-cached foreman base plans** → **re-tune** + adopt a `KernelParams` winner.

**Terrain decoding — see [ADR 0025a](0025a-coordinate-offset-anomaly.md) for the settled convention.** The terrain string decodes **column-major (`index = x*50 + y`)**, matching the engine's positional indexing (object spatial index, pathfinder, `LocalCostMatrix`); a row-major `y*50+x` decode transposes the room, and that transpose — not corrupt data — is what put 100% of dump objects on walls. The dump's object coordinates are **not** wrong: the dump's terrain is byte-identical to the official API and its object coords match. A residual ~15–20% of objects still read as wall under the correct decode, with no global transform fixing them; that residual is mitigated by snapping each object to its nearest open tile at load (`snap_to_open()`) — a no-op for the majority, a small nudge otherwise — so the foreman always gets valid, non-wall seed positions. The blast radius stays narrow: foreman seed positions only, never the terrain or the planned structures.

**Foreman capture is a library call, not a binary dependency.** `harness/foreman_capture.rs` (`CapturedBase`/`CapturedStructure` + `capture()` calling `screeps_foreman::planner::plan_room`) plus an offline `capture_base` bin produce a committed base cache; `ForemanGenerator` only ever *loads* that cache and never plans. `realize_base` reconstructs the scenario, and `breach_from_ramparts` derives the breach point from the real rampart ring's tile nearest a navigable entry (the synthetic geometry assumed a single west rampart).

**The tuning lenses.** `Bed::Imported(idx)` (mirror-symmetrized real terrain) feeds `realistic_comp_basket` (synthetic + imported beds) for open combat, and `realistic_base_scenarios` (foreman + imported `Raze`/`Breach` bases) feeds base attack; the heavy sims are rayon-parallelized (tournament cells, base-attack `(strategy × base)` pairs, exploitability) so a full grid re-tune is minutes, not hours.

**What the re-tunes established (design knowledge, in the order it was learned):**
- **Basket size changes the answer.** A small Raze-only basket made `k-approach-hot` (`approach_coef = 4`) look dominant (+21154 while every other config scored deeply negative); at 48-config × 52-base scale that did not replicate — a4 configs are middling-to-poor. The lesson is the §11 #8 rule restated: never adopt off a thin basket.
- **Realistic bases ARE position-sensitive** (unlike the synthetic beds of §11 #8) — the default kernel can chip at a real foreman rampart ring and bleed creeps instead of breaching. But with a *winnable-sized* force the base lens is only weakly discriminating (scores cluster), and LOW approach slightly wins on survival.
- **Open combat's optimum is low-approach / high-incumbency / TIGHT cohesion** (`a1-i6-tight` at this re-tune; the profile has since been re-tuned twice — the adopted `open_combat` is `a2-i6-tight` under the boosted 9-cell maximin, Design deltas → A results / ADR 0026) — hold tight at range, not spread. `a2-i4-tight` is the best *balanced* single config (a modest tweak from the `a2-i3-def` seed: incumbency 3→4, cohesion default→tight).
- **The discriminating levers are incumbency and cohesion, not approach.** Objective-awareness still pays (the best open config is poor at base attack and vice versa), which is exactly what ADR 0026's per-objective strategy-selection layer exists to exploit — but its breach profile should be swept over incumbency/cohesion with approach LOW, not around an `approach = 4` seed.
- **Spacing was a structural blind spot in the original grid** (it fixed `spacing = 1`). Screeps AoE is pure Chebyshev with no line-of-sight, so a tight blob eats stacked ranged-mass-attack and overlapping tower fire. Once spacing is a live axis, the spacing-1 profile is negative-mean and exploitable, and **spacing 2 is the generic sweet spot** over the real-opponent field (spacing 4 only wins a pure-ranged mirror — a candidate situational mode). See ADR 0026a.
- **Tournament discovery beats ideation here.** The hand-designed ADR-0026a catalog modes mostly under-performed the data sweep (e.g. the "lower incumbency for a ranged duel" idea *lost* the ranged mirror), and the headline lever was the one the hand-built grid had excluded.
- **A bit-deterministic sim is a precondition for trusting any of this.** Base-attack absolute scoring was once noise-dominated (~1% cross-process), which is why the profile split was argued from the robust open-combat win plus the dismantle-needs-range-1 principle rather than from a base-attack lead. The noise was two seed-ordered hash iterations in `screeps-rover`'s resolver — the per-tick pathfinding-ops budget consumed in `topological_sort_follows`'s seed order on dense bases, and `current_pos_to_entity`'s last-write-wins when two creeps stack on a tile. The fixes are structural: sort the topological move order by `Handle`, and keep the lowest `Handle` on a position collision. `sim_is_deterministic_over_rounds` (spread 0 across fresh-seed rounds) is the standing fence.

**Global constraints:** host-only (everything in `screeps-combat-eval`, wasm-excluded; no live `game::*` at sim time). Deterministic (`Rng::seeded(index)`; no Date/random/network/fs inside `generate()`). **Foreman is SLOW** (~3.6s typical, up to 55s; docs/0009a §2) → runs **offline once → cached JSON**, never in `generate()`/`validate()`/a tournament loop. No new engine types needed except the Stage-2 objective tag (`CombatTerrain`/`CombatWorld.rooms`/`SimStructure`/`SimTower`/`SimController` exist, engine `state.rs:11-176`).

### Stage 1 — Terrain import only
Get real shard terrain into a host `CombatWorld` + prove a squad navigates it. **Use committed encoded-terrain fixtures (a `const` table), NOT a live fetch** — a room is a 2500-char digit string (`0`plain/`1`,`3`wall/`2`swamp; rest-api `types.rs:156`); the rest-api `room_terrain_encoded` (`client.rs:387`) is async+HTTP+360/hr-capped → belongs in the Stage-3a offline tool. Absent committed fixtures, a room decodes from `screeps-foreman-bench/resources/map-*.json` (identical encoding, decoded at `bench/src/main.rs:464-501`). `harness/terrain_import.rs`: `decode_terrain(&str)->CombatTerrain` (inverse of the bench visitor; `walls.insert`/`swamps.insert`, `state.rs:11-29`), `fast_to_combat(&FastRoomTerrain)->CombatTerrain` (the cross-crate bridge; `terrain.rs:39-49`), `decode_fast`, `TerrainFixture { room, terrain, controller, sources, mineral }` + `FIXTURES`. Add `screeps-foreman` dep (Stage 3 needs it). Tests: `decode_roundtrips_a_known_pattern`, `fast_to_combat_matches_decode`, **`imported_terrain_is_navigable`** (the operator's smoke test — `ManagedSquadIntegration::validate` over a fixture passes; render via `render_managed_replay`). **Foundation — land first.**

### Stage 2 — Single + multi-room × objectives × comps
An `ImportedRoom` `Generator` over imported terrain, single + multi-room (ADR 0023a S3 `in_room`/`terrain_for`), parameterized by objective kind × comp. **Close the objective-kind gap first** (today `Objective` is a hard-coded "destroy spawn"; `EngageObjective` only Destroy/Hold): add a generation-side `enum ObjectiveKind { Farm, Secure, Breach, Raze, Declaim }` + `Objective.kind` (`scenario.rs`), each mapped to existing `RunUntil`/`EngageObjective`/world-population (Raze=`ObjectivesDestroyed`; Breach=rampart-falls; Secure=`SideWiped(defender)`; Farm=survive-N+`Hold`; Declaim=push `SimController`+ new `ControllerNeutralized` RunUntil). `ImportedRoom { multi_room, n_comps }` decodes `index→(fixture, kind, comp_seed)` like `Permutations`; **vary the DEFENDER comp via `roster::random_squad`**, leave attacker variety to the tournament `comp_basket`. Multi-room mirrors `twin_room_siege` (`generate.rs:400`). Tests: `imported_room_every_kind_is_assessable`, `imported_declaim_has_a_controller`, `imported_room_navigable`, `multi_room_imported_crosses_border` (gate on *reach*, not oscillation — the cross-room bed carries only the loose `≤ 0.97` bound, §11 #10). **After Stage 1.**

### Stage 3 — Foreman-layered realistic bases
**3a (offline, cached, committed):** reuse the bench `plan_room(&data_source)` path (`bench/src/main.rs:259-335`, `CpuBudget::unlimited()`) with a new output mode emitting `CapturedBase { room, terrain, controller, structures: Vec<CapturedStructure{kind,x,y}> }` — iterate `Plan::structures` (`plan.rs:267`), map `RoomItem.structure_type`→Spawn/Tower/Rampart/Wall (drop roads/extensions/labs), write one JSON per room to committed `resources/captured-bases/`. **Run once, manually, never in CI.** (Live-shard variant = same tool via `room_terrain_encoded`+`room_objects`, credential-gated, Phase G.) **3b (fast — loads cache, never plans):** `ForemanGenerator::new(dir, n_comps)` loads the JSONs; `realize_base(&CapturedBase, kind, comp_seed)->Scenario` decodes terrain + pushes `SimStructure`/`SimTower` (towers energized 100k, the calibration convention) via `ScenarioBuilder`. **New `breach_from_ramparts(core, &ramparts, &terrain)`** — the synthetic `breach_geometry` assumes one west rampart; derive staging from the real rampart ring (shortest BFS flood-fill to a room exit, `terrain.rs:315/52`). Tests: `foreman_cache_realizes`, `foreman_base_is_assessable`, `foreman_breach_geometry_is_in_range`, `#[ignore] write_foreman_replays`. **After Stages 1+2; 3b can land with a checked-in sample cache.**

### Stage 4 — Final re-tune over the full realistic basket
Widen the EXISTING tuning machinery (`tournament.rs`), build no new mechanism: extend `Bed` with `Imported(usize)` (its `apply_bed_terrain` copies a fixture + mirrors cached towers both ends) for the open-combat `run_tournament_over_comps`; point `base_attack_ranking` at `ForemanGenerator`+`ImportedRoom` scenarios (`assault_score` unchanged); add a terrain-aware `lanchester_validation` variant. **Adoption protocol:** (1) realistic kernel tournament → mean-payoff + meta-Nash; (2) realistic base-attack = no-regression capability gate (position-insensitive with a winnable force, §11 #8); (3) pick the **robust** (lowest-exploitability/Nash) config, re-run `exploitability ≤ GROSS`; (4) edit the adopting profile's constants + re-green oracle-calibration / single-room-oscillation / self-play-decisive / Lanchester-floor / action-oscillation — a `KernelParams` change is a decision-crate constant, **no `WORLD_FORMAT_VERSION` bump**; (5) record the adopted constants + their per-objective ranking with the profile that adopts them (ADR 0026 §8). MMO deploy stays gated on ADR-0020 §10 Docker-soak → operator go-ahead. **Last — requires Stages 1–3.**

**Dependency graph:** `Stage1 → Stage2 → Stage3a → Stage3b → Stage4`.
**Blockers/fallbacks:** (1) no committed fixtures → decode a bench `map-*.json` room; (2) live import credential/rate-capped → terrain from fixtures, foreman supplies structures offline (live `room_objects` import is the off-critical-path Phase G); (3) foreman is single-room → plan per room + compose with `in_room` (inter-room roads are cosmetic in the combat sim); (4) controller targeting depends on the view carrying a controller (§11 #11) → Stage-2 `Declaim` is *scored* by neutralization, which is independent of that view work.

## Landed
- `7b348e4` sim intent-legality mirrors the real engine table (2026-06-25)
- `7f72516` `plan_squad_ev` kernel in, old layout/action machinery deleted (2026-06-25)
- `2a4f790` sim adapter consumes `member_intents` (2026-06-25)

## Design deltas (2026-09-07 — WS-CLOSE write-back)

What the kernel (`screeps-combat-decision/src/kernel.rs`, `lib.rs`) became between the WS-VAL
corpus (2026-08-23) and the WS-CLOSE tie-off (2026-09-07). Every item below is verified against the
code as of this write-back; the body sections above were corrected in place where they had become
false (§2.2 HEAL/RISK rows, §2.3 STAGE 0 heal ledger, §7 tie-break and cross-room paragraph, §10
boosted-TOUGH, §11 #10, §12 Stage 2 and the `a1-i6-tight` line). Provenance, cited once: decision
`be725c9` (A/B), `b0b7ea0` (C), `acf3600`/`71c6e0a` (D), `f8b97a6` (A), `d693229` (E), and the
2026-09-07 WS-CLOSE Phase A lanes (F).

### A. One progress-diluted currency across all three legs (RULING-9, operator 2026-08-24)

The §2.1 promise — no new exchange rate between damage dealt, deaths prevented and risk taken — was
only half kept by the first build: attack value was diluted by progress (`value_per_hit =
g_them · threat_value / residual`, each landed hit is a fraction of the kill), but heal value and
self-risk were priced FLAT per HP with multiplicative premiums bolted on. The asymmetry showed up as
survivor remnants that PERCHED: in a long fight a perpetual heal out-priced every killable focus.
The redesign makes all three legs the same primitive — `g × output × survival-time per HP`:

- **HEAL leg — the triage price.** `HealTarget::value_per_hp = g_us · member_output /
  horizon_hits`, where `member_output = melee + ranged + heal_parts·HEAL_POWER + dismantle_power`
  (a pure siege dismantler's whole contribution is WORK; excluding it priced its heals at zero) and
  `horizon_hits = max(1, hits − inc·SURVIVAL_HORIZON)` at the ally's UNGATED incoming. Dividing by
  the *remaining* hits mirrors the attack side exactly (the event a heal prevents is this ally's
  death, and the enemy still needs `hits` more damage to realize it), so the price rises as death
  nears — graduated mortality falls out — while a full-HP target stays cheap. `MORTAL_HEAL_MULT`
  (4, when `inc ≥ hits`) is kept as the one sanctioned nonlinearity (the finish-bonus mirror). The
  flat `URGENT_HEAL_MULT` premium (×8, then a ×3 calibration) is DELETED.
- **Two heal ledgers, urgency as allocation not price.** Each ally carries `inc` (URGENT —
  anticipated incoming from the threat field, drained first) and `deficit` (BACKLOG — `hits_max −
  hits`). Equal-value candidates tie-break toward the most UNCOVERED incoming (`apply_act`,
  `Act::Heal` branch), which is the dying-first stack the old ledger-index tie lacked.
- **Evidence gate on the URGENT ledger.** The field stamps the enemy's full output on every tile in
  range, but focused fire lands on one creep; pricing every full-HP member as "about to take it all"
  made each healer's top EV a self-pre-heal while the actually-focused member died (watched: healers
  at full HP emitting `Heal(self)` as the focused healer spiralled −78→−318/tick). A member is
  `wounded` — carries its full `inc` — when it has a real deficit OR a hostile is inside its weapon
  band (melee ≤2, ranged ≤3); otherwise `inc` is discounted `÷ UNFOCUSED_INC_DIV` (4). The range
  clause exists because deficit-only evidence BREATHES in an even duel (healed-full on alternating
  ticks) and period-2'd the healer's tile with it.
- **RISK leg — the MARGINAL price, not the triage price.** `best_tile` charges `net × my_vph` with
  `my_vph = g_us.max(unit) · my_out / max(raw_at(tile), hits / SURVIVAL_HORIZON)`. The triage form
  diverges as a risk price: a healthy squad reads its exposure as ~`out/hits_max` (near zero — six
  of eight marched into the choke kill zone), then the wounded remnant prices its HP as infinite and
  PARKS outside tower range to timeout. The marginal form pays what one HP actually buys —
  `d(ttl)/d(hp) = 1/inc` ticks of output under fire, capped at the horizon rate when fire is light —
  bounded both ways. `g_us.max(unit)` is the siege risk-currency floor from B (with no killable
  creeps `g_us → 0` and members priced their HP at nothing). The flat `NET_RISK_MULT` (×4) is
  DELETED: it out-massed every diluted attack value for healer-less comps and produced universal
  cowardice (a T3 trio refused a 4:1 trade with a HOLDING T0 twin). The `LETHAL` survival veto is
  untouched.
- **Kite dead-zone fall-through** (`lib.rs`, `decide_squad_with_pathing`, `kite_settled`). A
  `plan_kite_anchor` result of `None` means "already the safest, most cohesive tile" — a kite with
  nothing to flee. Previously the kernel was gated on `!should_kite`, so such a squad neither fled
  nor fought: a stable non-fighting equilibrium just outside weapon range (the T3-twin margin-0
  standoff, gate-traced). Now the kernel runs anyway unless the squad state is `Retreating` (a
  withdrawing block must not re-engage because its tile happens to be safe).
- **Focus-stall gate** (`select_focus_target`, fallback 1). A squad with `our_dps == 0` (WORK/HEAL
  siege comp) no longer falls back to the lowest-hits unshielded creep when a valued hostile
  STRUCTURE exists: the creep focus dragged the approach gradient to a creep it could not hurt
  (dismantlers crowded an unkillable guard at range 2 of the core, zero acts, engage budget burned).
  With no structure alternative the creep focus is kept even at dps 0 (harmless, and it holds the
  committed state across roster flaps).
- **Heal-incumbency dead-band.** At its current tile a member gets `+unit · incumbency_coef` only
  where OFFENSE lands (`offense_reachable`); where only a HEAL lands (`heal_reachable`) it gets a
  light `+1 · unit`. The duel-breathing this damps is sub-unit (~0.005 unit on designed#2, where
  `value_per_hp` breathes with the ally's HP), so one unit crushes the flip without out-pulling a
  real approach gradient (≥ 2 units/tile) — the full coefficient froze healer-dps lockstep pairs
  mid-march. designed#2 oscillation 29.5% → 5.0%.
- **Results under the currency.** `t3_twin_decisively_beats_unboosted_twin` is pinned against a
  HOLDING defender (an equal-speed mirror fleer is honestly uncatchable — the eternal chase is
  correct, not a defect); the generated-bed fairness bound moved 2000 → 3000 because mirror fights
  now TRADE (order bias compounds over real casualties; the sign is still seed-varying — open). The
  boosted re-tune that followed (item 6) adopted `open_combat = a2-i6-tight` (`strategy.rs`:
  approach 2, incumbency 6, discohesion 20, k 2, spacing 1) by maximin-with-a-noise-band over
  3 tiers × 3 terrains; the profile table itself is ADR 0026's.
- **Note on the §2.2 DENIAL row.** As built there is no separately booked once-per-target kill
  swing: kill value is linear in `DamageTarget::value_per_hit` over the residual (the comment in
  `kernel.rs` records the finish/concentration bonus as a tournament refinement, §11 #1).

### B. Cohesion under focused fire (Phase 4.5 item 1)

The L1@T3 stronghold sizing was honest (3 T3 healers out-heal a 600-dps tower) but only at heal
range 1; the squad approached strung out, ranged-heal landed at ⅓ output, and `focusClosest` killed
full-HP members serially. Four kernel defects, each fixed per-tick-optimally with no formation state:

1. **Deliverable heal, not squad total** (`deliverable_heal`). The RISK net at a candidate tile
   subtracts the member's own full self-heal plus what each OTHER living healer can land there from
   where it stands NOW: full `heal` within 1, `rangedHeal` within 2..3, nothing beyond. Deliberately
   NO catch-up slack: crediting a healer one tile past real reach is a lie during a march (both move
   1 tile/tick, the gap never closes; slack-1 wiped the L1-chokepoint squad by t69). Strict reach
   makes the advance self-gate — the front tile prices uncovered the moment it leaves range, the
   member brakes, the healers' own approach pull restores adjacency, the wedge crawls forward tight.
2. **Lockstep healer-tile advertising.** After a healer commits, `HealerReach.pos` is set to its
   LANDED tile, so every member after it in the commit order prices coverage one step ahead (a wedge
   advancing together reads full coverage; a healer four tiles back still reads as the chase it is).
   This relies on the STABLE member-index order (§7.2 item 2): the optimizer emits Healer slots first,
   so healers already commit before the dps they cover in oracle comps. **Rule (recorded in
   `plan_squad_ev`):** an explicit healers-first SORT was tried and REVERTED — reshuffling contention
   away from member index drove designed#0's period-2 rate 3% → 44%; the claimed/spacing/held
   interactions are tuned around index-stable contention. No kernel-order change ships without the
   oscillation gate in the loop.
3. **Siege risk-currency floor** — `g_us.max(unit)` in the RISK price (A above). The item-1 form
   also carried a ×4 uncovered-net steepener; that steepener was the flat coefficient RULING-9 later
   deleted, so only the floor survives.
4. **Evidence-gated URGENT/BACKLOG triage** (A above).

Also from this pass: the **flood-gap fallback** (`APPROACH_FALLBACK_TILE_COST` = 2) — the target
flood is ops-bounded (`TARGET_FLOOD_OPS`) and threat-cost inflation shrinks its radius; a member
beyond it read `u32::MAX` for every candidate and froze permanently (L1 probe: parked 15 tiles out
from t200). Beyond the flood the approach term degrades to Chebyshev distance to the approach
target at plains cost, handing over to the wall-aware flood where coverage begins. Acceptance:
L1-open@T3 → Killed, zero losses (`stronghold_floor_t0_defers_t3_kills_every_l1_rung`).

### C. Border crossing — the kernel's share (Phase 4.5 item 2, item 8a)

Of the seven-defect chain behind "one creep enters and everything outside the room stalls", four
live in this kernel; the bloc crossing gate, full-roster member views (parity H5) and room-gated
mover anchor are `screeps-combat-agent`'s, rout-to-rally is ADR 0034's and Retreating state decay
ADR 0027's.

- **`plan_squad_ev` takes the FIGHT room explicitly.** `decide_squad_with_pathing` derives it as the
  focus room, else the centroid room, and passes it down; members in any other room are excluded
  from the kernel (goal `None` → the shared `Advance` rejoin governs them) and `best_tile` asserts
  every `EvMember` is in that room (the V-1 aliasing guard). Deriving the room from the CENTROID had
  transplanted every tile into the STAGING room whenever a mid-crossing squad's full-roster centroid
  sat across the border, and integer centroid flapping re-mapped the goals tick to tick.
- **Exit-edge tiles are transitional** (`EXIT_EDGE_PENALTY = LETHAL / 8` on `x,y ∈ {0, 49}`). In
  the real engine a creep on an exit tile transits — it cannot hold there — while the sim grid let it
  stand; an entrant camping the arrival tile "waiting for coverage" was also the doorway jam (three
  entrants at x=49, the five members carrying their coverage queued behind them forever). Priced
  below every tactical alternative but above the survival veto (transiting beats dying).
- **Room-local tower assessment** (`assess_engage`). The tower damage curve floors at 150 for any
  large range, so with full-roster views a staging-room centroid read the target room's towers as a
  phantom 150/tick each, latched `Retreating`, and never crossed. Towers now count only in the
  centroid's room — a tower cannot fire across a room boundary.
- **Out-of-contact handoff** (`block_advance`, item 8a). Under a block `Advance` whose goal is in the
  fight room, a squad with no combat signal near ANY member (`squad_in_contact`: field incoming > 0,
  a living hostile creep or structure within 4, or a healer with a needy ally within 4) yields every
  member's goal to the block directive (`goal: None` → the traffic-managed mover). The per-tile EV
  is a CONTACT instrument; out of contact it deadlocks the pack as a rigid body — with `act = 0`
  everywhere each lone step prices as leaving the pack (discohesion ≫ approach at the tuned profile)
  while the centroid, an average, can never lead (tile-traced on border g1@T3). Squad-level on
  purpose: a per-member handoff flapped control kernel↔mover at the contact boundary (designed#4
  → 99.6% period-2); cross-room goals are excluded because the mover marched members over the exit
  edge. Kite / Drain / Hold blocks are untouched. designed#4: 99.6% → 0.6%.

Acceptance: every fielding stronghold rung kills (L1 open / choke / choke-multi) and border g1@T0,
g1@T3, g2@T3 kill with the bloc crossing together. What remains open is §11 #10 (the seam-stitched
field).

### D. Drain rework and the plateau tie-break (Phase 4.5 item 5)

- **Placement, not the form phase** (correcting an earlier log entry). The harness placed drain
  squads at ~r11 INSIDE the tower falloff; the focused tank died in ~3 ticks in every variant and the
  canary's verdict rode whichever remnant survived. Drain scenarios now place at
  `TOWER_FALLOFF_RANGE + 2` along the entry ray (`screeps-combat-eval/src/harness/validate.rs`) —
  live parity, since a real squad meets the standoff before entering the falloff.
- **Deliverable standoff sizing** (`lib.rs`, `DRAIN_ADJACENT_SLOTS = 3`). The standoff range is
  chosen against the heal that can actually reach the tank: the largest healers fill the three
  REAR-ring full-rate slots, overflow support heals at ⅓ (ranged) from the set-back band. Squad-total
  sizing picked a band ~5 ranges too deep (r15; the tank bled −45/tick) — the survivable band is r19.
  `drain_member_goals` fills only behind/level ring tiles (`range_to(nest) ≥ standoff`), never the
  deeper-falloff side.
- **Approach-aware plateau tie-break** (`best_tile`). Equal-cost bands are common in flat,
  threat-free states; the old lower-x/lower-y tie-break was objective-blind, and a post-drain squad
  west of its target plateau-drifted WEST and perched. Ties now prefer the smaller approach distance
  `d` first, then lower x, lower y (§7 corrected in place).
- **Tried and reverted, recorded in `best_tile` / `plan_squad_ev`:** an EXACT-CLAIM hard exclusion
  (no two members may pick the same range-0 destination) cost the kite beds their concentrated
  chip-fire (`EXP-POS-KITE-1` went red) — the goal-convergence churn is real but its fix must ride
  the EXP register; and a kernel-side REPAIR pass for early goals landing on later committed-stayers
  (both full and adjacent-only strengths) either stalled the drain endgame or broke corridor
  queueing — that dance stays damped mover-side (`convert_persistent_doomed_goals`).

### E. Siege member-clamp machinery — landed, wired off (Phase 4.5 item 8a)

`composition.rs`: `member_cap_for(objective)` is the ONE policy point both the winnability ceiling
and the assembler cap derive from; it returns `MAX_SIZED_MEMBERS` (8). `SIEGE_MAX_SIZED_MEMBERS`
(16) is kept as the wiring anchor. Measured with the lift active: the sizing works (L2@T3 fields at
p_surv 0.82, the whole gate battery green) but the 16-blob loses tactically on every terrain —
congeals in the choke, parks as a rigid body at the tower-threat edge in the open (five deliverable
healers cannot gate eleven fighters forward). Shipping it would convert live L2+ defers into repeated
wipes, so the L2+ path is the multi-squad doctrine (ADR 0048, parked Draft). The by-products that DID
ship and this kernel is now graded by: honest gauntlet verdicts (`Killed` = the core RAZED; the old
defender-wipe stop mis-scored border rungs at camper-kill), the in-contact stall gate (ADR 0035),
and the out-of-contact handoff (C above).

### F. `threat_value` prices WORK and CLAIM (WS-CLOSE D1, 2026-09-07)

`lib.rs::threat_value` is the one additive capability-removed currency for every consumer — the
kill ledger here (`g_them × threat_value` via `ev_target_order`), squad focus EV, and the tower
redirect order in `tower_fire` (parity M17/M18). It now adds `effective_output(Work,
DISMANTLE_POWER)` (50/part, UNCONDITIONAL — a ranking proxy only; force SIZING must never fold WORK
into creep dps, per the dismantle ruling) and `effective_output(Claim, CONTROLLER_ATTACK_PER_PART)`
(300/part, the engine's own controller-damage unit). One CLAIM part = ten ATTACK parts, so a
declaimer outranks any realistically-sized breacher and ADR 0008a's T-DEF-4 ordering falls out of
the currency instead of a lexicographic tower-only rule. Every term is boost-aware through
`CombatCreepDto::effective_output`.

### G. Open items recorded 2026-09-07

- **F2 — duplicate-goal park.** The kernel has been seen to assign two members the SAME goal tile on
  a tower's flank (the ADR 0023 S5 GROUP-UP bed, members at (46-48,21-23) vs a tower at (46,23));
  the agent's dance damper then converts both to `Immovable` holds and the squad parks `Engaged`
  forever without acting. The soft spacing penalty does not prevent it and the exact-claim exclusion
  (D) was reverted for cost. Bar: goal assignment excludes tiles already claimed this tick, or the
  damper never freezes two members on one tile. Repro: the GROUP-UP bed with the staging moved onto
  the tower's flank.
- **Boosted-TOUGH damage reduction** — §10, refreshed in place (the boost field exists; receiving-
  side TOUGH reduction does not).
- **Goal-convergence churn** (D) — needs the EXP register + oscillation surfaces in the loop.
- **Generated-bed fairness** — mirror fights trade; the sign of the order bias is seed-varying.
- **Process rule** — the oscillation gate rides every kernel-order change (B.2).
