# ADR 0031a — Force-composition tunable parameter set (research note)

- **Status:** Research note (input to ADR 0031 D16 `optimize_composition`)
- **Date:** 2026-06-27
- **One line:** Maps a four-bot survey (Overmind, The-International, TooAngel, bonzAI/community meta) + Screeps combat-mechanics ground truth onto our `CompositionParams` knob set, recommends a concrete tournament-sweep parameter list with grounded ranges, and calls out the structural levers our current 7-knob design is missing.

Reads against ADR 0031 §2c/D16. Our current model:

```
EV(C) = P(win | C) · target_value − cost(C)
cost  = w_energy·energy + w_creep·creeps
P(win)= win_probability(heal, incoming) · kill_feasibility(C, defense, window)
CompositionParams { w_energy, w_creep, hold_margin, over_power_margin,
                    dynamic_margin, member_energy, commit_ev_threshold }
```
Search space: creep-split (n_fighters × n_healers, `1..=8`) × over-power factor. Bodies: RANGED/WORK/HEAL/TOUGH via `build_combat_body(CombatBodySpec)`; `MAX_SIZED_MEMBERS = 8`; unboosted v1.

Current seed constants (the ones D16 migrates into `CompositionParams`): `HOLD_MARGIN = 1.3` (`force_sizing.rs:27`), `COORDINATED_DPS_MARGIN = 1.5` (`:34`), `IMPORTANCE_MAX_EXTRA = 0.5` (`:386`), `WIN_PROB_STEEPNESS = 5.0` (`:376`), `PREFERRED_MEMBER_ENERGY = 3000` (`composition.rs:42`), `SK_KEEPER_KILL_TICKS = 34` (`doctrine.rs:461`).

---

## 1. Headline

The two serious bots converge on the SAME shape as our oracle — size to *enemy-heal / my-damage* with a multiplicative safety margin, fold tower DPS into the requirement, and gate "unwinnable → abandon." So our existing knobs are well-founded. What the field has that our **7-knob set lacks** is mostly on the **body/archetype** axis, not the count axis:

1. **No archetype selector.** Every serious bot picks between a *ranged-blob* (4× RANGED, cross-heal, formation siege) and a *melee/dismantle + heal* shape by target type. Our search is pure count×over-power over a fixed RANGED/WORK weapon — it cannot choose melee ATTACK, cannot choose a pure-RANGED anti-creep blob vs a WORK dismantle siege as a *tuned* decision. (ADR 0031 picks the fighter weapon upstream in the doctrine `fighter_role`, NOT in the EV search — so the optimizer can't trade it off against EV.)
2. **No TOUGH stacking ratio / EHP knob.** `tough_parts` is hardwired to 0 (v1). Every bot that fights towered rooms front-loads TOUGH, and the community's load-bearing rule is heal-must-refill-broken-TOUGH-per-tick. With `tough=0` we cannot model surviving point-blank tower fire at all.
3. **No within-member attack:heal MIX knob.** The-International *trades* ranged-attack vs heal parts inside a member around a 0.75 ratio to fit the threat. We split *whole members* into fighter-vs-healer but never tune the part mix inside a member (our bodies bake a fixed self-heal via `build_combat_body`).
4. **No tower-drain / kite mode.** The dominant *low-cost* commit path in the meta is drain (tanky+heal cycling at tower-falloff range), which flips the cost side of EV from "out-heal the wall" to "out-spend their tower refill." Our EV has no drain branch and no kite-range parameter.
5. **No boost-tier axis** (correctly out of scope for unboosted v1, but it is the single biggest lever the moment v2 boosts land — it reshapes every ratio below).

---

## 2. Recommended parameter set

Legend — **MAPS TO**: existing `CompositionParams` field, or **NEW**. Priority = tournament-sweep priority.

### A. Knobs that map to existing `CompositionParams`

| Param | Controls | Maps to | Priority | Suggested range | Rationale / source |
|---|---|---|---|---|---|
| `hold_margin` | Heal-surplus safety: required `heal/tick ÷ incoming/tick`. Drives `win_probability` (1.3 ≈ P 0.82). | `hold_margin` | **high** | **1.15 – 1.6** (seed 1.3) | Overmind invasion/outpost formula uses **1.5** overmatch; The-International heal-vs-heal uses **1.2**; community winnability gate wants 1.15–1.25. Our 1.3 sits dead-center — sweep the 1.2–1.5 band the two serious bots bracket. |
| `over_power_margin` | Coordinated square-law over-match for creep-clear (`COORDINATED_DPS_MARGIN`). How much DPS you out-mass a creep blob by. | `over_power_margin` | **high** | **1.3 – 1.8** (seed 1.5) | Overmind's per-channel rule is literally `ceil(1.5 × enemyPotential / ourPotential)` per role; this IS our 1.5 seed. Existing eval `COORDINATED_DPS_MARGIN` sweep (`harness/mod.rs:132`) already targets this band — keep that sweep, widen to 1.3–1.8. |
| `dynamic_margin` | Inflates observed hostile force so a *growing* threat still loses (threat headroom). | `dynamic_margin` | **medium** | **1.0 – 1.4** | Overmind remembers enemy potential via an EMA (`THREAT_DECAY_TIMESCALE = 100` ticks) so sizing tracks a rising threat; The-International adds a **+10%** tower margin (`maxTowerDamage ×1.1`). 1.1 is a defensible floor; 1.0 = trust the snapshot. |
| `member_energy` | Per-member energy cap = body size cap → small-many vs few-big. `probe_energy = min(member_energy, 3000)`. | `member_energy` | **high** | **1300 – 5400** (seed cap 3000; clamp ≤ MAX_CREEP_SIZE×~108) | This is our small-many-vs-few-big lever and it *emerges* from the sweep (D16). Overmind/International both fill bodies to energy capped at **50 parts**; a 50-part 1:1 body ≈ 5000–6500e. 3000 forces ≥2 members for big forces (bankable) — sweep 1300 (RCL4 cap) → 5400 (≥RCL7, 50-part). |
| `w_energy` | Cost weight on total spawn+lifetime energy. Trades EV against econ spawn contention. | `w_energy` | **medium** | **0.5 – 2.0 ×(value/energy normaliser)** | Community standard: cost = build energy + **lifetime upkeep** + **spawn-time opportunity** (150 ticks for a 50-part creep; 100% uptime ≈ 33.3 e/t refill). w_energy is how hard econ contention down-weights a marginal siege. Set so a borderline-EV siege loses to econ when spawn-starved. |
| `w_creep` | Cost weight per creep body (spawn-slot occupancy / micro overhead). | `w_creep` | **low** | **0 – 300 energy-equiv/creep** | Each extra body is 3 ticks/part of spawn-time and a micro/coordination cost. Smaller than `w_energy`'s effect; mostly a tie-break nudging toward fewer, fatter members. The-International caps a member at 50 parts which already bounds this; keep small. |
| `commit_ev_threshold` | EV floor to field at all (else `None` defer). The honest unwinnable gate. | `commit_ev_threshold` | **high** | **0 – 0.3·target_value** (seed 0) | Universal across bots: The-International `deleteCombatRequest` when parts>50 / cost>capacity; Overmind defers via canBoost; community RCL5-without-boosts hard gate; safeMode veto everywhere. A threshold >0 means "only commit with real EV headroom," directly encoding no-half-measures. |

### B. NEW knobs the field uses that our 7 do not have

| Param | Controls | Maps to | Priority | Suggested range | Rationale / source |
|---|---|---|---|---|---|
| `archetype` (weapon select) | Choose RANGED-blob vs MELEE(ATTACK) vs WORK-dismantle as a *tuned* dimension in the EV search, not fixed by doctrine `fighter_role`. | **NEW** (enum dimension) | **high** | `{RangedBlob, MeleeAttack, WorkDismantle}` (+ derived `Drainer`) | Every serious bot selects by target: International `quad = {rangedAttacker:4}` OR `{attacker:1,heal:3}` OR `{dismantler:1,heal:3}`; Overmind ranged-vs-melee by colony stage; quad-design issue #69 strongly prefers RANGED in formation (works in box, mass-attack 9 tiles, no counter-hit). MELEE only when boosted or vs undefended/cores (ATTACK 30 vs RANGED 10 = 3× dmg, cheap, good vs invader cores). **This is the biggest gap** — the measured ADR-0031 failure was exactly a weapon mismatch (WORK siege vs guard). |
| `tough_fraction` / `tough_parts` | EHP front-armor as a fraction of a fighter body. Hardwired 0 in v1. | maps to **`tough_parts`** (currently always 0) | **high** | **0.0 – 0.25** of body (≈ 0–2 TOUGH per 10) | Meta ~10–12% TOUGH unboosted, ~20% at T3. Overmind armored bodies = 1 TOUGH : 3 dmg : 4 MOVE (12.5%). Required to survive any towered engage. Couple to heal: TOUGH EHP added must be refillable by onboard+ally HEAL/tick (the 4-TOUGH:9-HEAL T3 self-recovery floor). |
| `attack_to_heal_mix` | Within-member ranged-attack : heal part split, traded to fit the threat. | **NEW** (feeds `CombatBodySpec`) | **medium** | **0.6 – 0.85** dominant-part fraction (seed ~0.75) | The-International "trades" parts around **0.75** of the dominant part inside a quad member, shrinking until it fits spawn capacity. Overmind bakes **1 HEAL per 3 RANGED** self-heal into hydralisks (~25% heal). Lets a member self-sustain vs needing a separate healer at small scale. |
| `heal_to_dmg_target_ratio` | Squad-level required heal/tick ÷ incoming/tick at the *engage tile* (incl. towers), as distinct from `hold_margin` which is the safety factor on it. | **NEW** input (feeds `kill_feasibility`) | **high** | derived; **`heal/tick ≥ incoming/tick`** with `hold_margin` on top | The core winnability inequality every bot computes. Concrete anchors: 1 tower = 600/tick → needs ~17 T3-HEAL (or proportional unboosted) to out-heal; 6 towers point-blank = 3600/tick. Unboosted, HEAL=12/part, so out-healing one tower needs **50 HEAL parts** — i.e. unboosted v1 *cannot* sit point-blank vs towers, which is WHY drain/kite mode (below) matters. |
| `engage_range` / `kite_range` | Stand-off distance → tower-falloff DPS taken. Drives `incoming` in the inequality. | **NEW** input | **medium** | **range 1 – 20** (tower 600@≤5 → 150@≥20) | Tower DPS at a position = Σ `600·(1 − 0.75·(range−5)/15)` for range>5. Standing at range 20 vs 6 towers = 900/tick vs 3600 at range 5 — a **4× incoming reduction** purely from positioning. Community: ranged kite at room edge beats healed melee blobs in open field. Already half-present in `force_sizing` tower model; expose engage-range as a search dimension. |
| `commit_mode` (siege vs drain) | Switch from out-heal-the-wall to out-spend-their-tower-refill when required heal exceeds what ≤8 unboosted members can supply. | **NEW** (mode flag) | **medium** | `{Siege, Drain}`; Drain when `required_heal_at_wall > deliverable_heal(8, member_energy)` | Drain is the dominant *low-cost* commit (towers cost 10e/shot, finite 1000 cap). Win flips to: defender tower-refill-energy/tick > our creep+boost upkeep/tick. **Especially relevant for unboosted v1**, which can't out-heal towers point-blank (see above). Overmind's `isEdgeDancing` (re-enter ≥3×) detects exactly this pattern. |
| `retreat_threshold` (engage) / `reengage_threshold` | Hysteresis HP fractions for break-off and re-commit. Already on the doctrine as `retreat_threshold`; pair it. | **NEW** `reengage` (retreat exists on doctrine) | **low** | retreat **0.75–0.85**, reengage **0.95–1.0** | Overmind: melee retreat **0.75**/reengage 0.95; ranged retreat **0.85**/reengage 0.95; tanky/boosted retreat earlier (0.85) than squishy (0.75); CombatZerg reengage at 1.0 HP. Boost/TOUGH presence should *lower* the retreat trigger (tankier ⇒ break off sooner to preserve the investment). Note ADR 0031 invariant says no hysteresis unless oscillation observed — keep low priority, only sweep if edge-thrash recurs. |
| `importance_margin` (`IMPORTANCE_MAX_EXTRA`) | Extra over-invest for high-value targets (×1.0 marginal → ×1.5 critical). | maps to **`hold_margin`**-adjacent; currently a separate seed | **low** | **0.0 – 0.7** extra (seed 0.5) | Overmind `numSwarms = directive.memory.amount` and International `quadQuota` are operator-set value scalars; our `importance` is the principled version. Already a knob; fold into the sweep but low priority — value enters EV directly via `target_value`, so this is a secondary over-invest dial. |

---

## 3. What our 7-knob set is structurally MISSING (summary)

1. **Archetype / weapon as a tuned dimension** (ranged-blob vs melee-quad vs WORK-dismantle vs drainer). Picked upstream in doctrine `fighter_role`; the EV search can't trade it. The exact ADR-0031 measured failure (WORK siege vs guard = 0 damage) is a weapon-mismatch — promoting weapon into the EV search would let `EV(C)` itself reject the bad weapon. **Highest-value addition.**
2. **TOUGH stacking ratio (EHP)** — `tough_parts` hardwired 0. No model of surviving towered engagements; the heal-refills-broken-TOUGH coupling is the load-bearing survival rule in the meta.
3. **Within-member attack:heal mix** (~0.75 trade) — we split whole members, never tune part mix inside one. Costs us small-scale self-sustain efficiency.
4. **Tower-drain / kite commit mode + engage-range** — the dominant low-cost EV path, and the ONLY viable path for unboosted v1 vs multi-tower rooms (50 HEAL parts to out-heal one tower point-blank is infeasible). Flips the cost side of EV entirely.
5. **No-half-measures value discount** — value of a raid that can't finish (de-claim) is ~0 (PvP meta). Should multiply `target_value` by a finishability factor, not just gate on EV>0.
6. **Boost-tier axis** — out of scope for unboosted v1, but the single biggest lever for v2: T3 MOVE collapses the MOVE budget (1:1 → 1:4), freeing slots for TOUGH/HEAL/damage; reshapes every ratio above. Reserve the param name now.
7. **Lifetime-and-spawn-time cost accounting** — `cost` should be build + lifetime-upkeep + spawn-time-opportunity, not just spawn energy. `w_energy` can absorb this only if the energy term already includes lifetime upkeep.

---

## 4. Tournament sweep plan

Sweep in priority tiers; freeze lower tiers at seed while sweeping a higher one (coordinate-descent), then a final joint sweep over the top tier. All sweeps run on the bit-deterministic sim with the determinism fence (`emit_requirement_is_deterministic_over_objectives`, `sim_is_deterministic_over_rounds`) asserted per case. Grade against `OracleCalibration` (FP ≤ 0.010 / FN ≤ 0.200), `SizingWins`, `CreepClearWins`, and the ADR-0031 acceptance bed (`oracle_sized_force_forms_and_kills_a_defended_core`) + regime sweep (`assembler_kills_across_defended_regimes`).

**Tier 1 — count×margin core (existing knobs, highest priority):**
- `hold_margin` ∈ {1.15, 1.3, 1.45, 1.6}
- `over_power_margin` ∈ {1.3, 1.5, 1.8}
- `member_energy` ∈ {1300, 2000, 3000, 5400}
- `commit_ev_threshold` ∈ {0, 0.1·V, 0.2·V}
- Cross-product (4×3×4×3 = 144) on the defended-regime beds; this validates the count axis against the two-bot bracket (1.2–1.5 margin, ≤50-part bodies).

**Tier 2 — archetype + EHP (NEW, requires extending the search):**
- `archetype` ∈ {RangedBlob, MeleeAttack, WorkDismantle} swept per bed-type (creep-defended vs structure vs core vs SK).
- `tough_fraction` ∈ {0.0, 0.1, 0.15, 0.2} — graded by surviving an added-tower regime.
- Expected emergent result to confirm: RangedBlob wins creep-defended + immune-core beds; WorkDismantle wins dismantle-able-ring beds; TOUGH>0 is required to pass any tower-present bed.

**Tier 3 — drain/kite + mix (NEW, requires the cost-side branch):**
- `commit_mode` ∈ {Siege, Drain}; `engage_range` ∈ {1, 5, 12, 20} — graded by EV on a multi-tower bed where unboosted Siege is infeasible (the bed should select Drain).
- `attack_to_heal_mix` ∈ {0.6, 0.7, 0.75, 0.85}.

**Tier 4 — cost-weight + secondary (lowest priority, joint with Tier-1 winners):**
- `w_energy`, `w_creep`, `dynamic_margin`, `importance_margin`, retreat/reengage pair — narrow ranges, tie-break/efficiency role.

**Output:** per-tier best params + the emergent strategy map (which archetype/mode/member-energy the sweep selects per bed regime), feeding the ADR-0031 §4 "Tournament lens (P6, D13/D16)" re-sweep of the ADR-0019 position weights.

---

## 5. Concrete numbers other bots actually use (quick-reference)

- Overmatch / hold margin: **Overmind 1.5** (per-channel `ceil(1.5·enemy/ours)`), **The-International 1.2** (heal), tower margin **×1.1**. Ceiling bias **+0.5**.
- Boost gates (Overmind): boost when enemy dmg > **1500/tick** (ATTACK×50) OR heal > **1000/tick** (RA×100); add dismantler arm when dmg > **2100/tick** (ATTACK×70).
- Retreat/reengage: melee **0.75/0.95**, ranged **0.85/0.95**, tanky retreat **0.85** vs squishy **0.75**, reengage **1.0**.
- Fixed offense shapes (when not adaptive): Overmind swarm = 2 melee + 2 heal; pair = 1+1; International quad = 4 ranged OR 1 atk:3 heal OR 1 dismantle:3 heal; TooAngel siege = 1 dismantle + 3 heal.
- Within-member trade ratio: **0.75** (The-International).
- Body ratios (Overmind, TOUGH first / HEAL last): unboosted ranged 3 RA:4 MOVE:1 HEAL; armored melee 1 TOUGH:3 ATK:4 MOVE; T3 melee 2 TOUGH:6 ATK:2 MOVE; healer T3 2 TOUGH:6 HEAL:2 MOVE.
- MOVE ratio by boost tier: **1:1** unboosted (plain full speed), **1:2** road/half-speed, **~1:4** at T3 MOVE boost.
- Game ground truth: ATTACK 30, RANGED 10 (mass 10/4/1 @ r1/2/3), HEAL 12 (rangedHeal 4), WORK dismantle 50; TOUGH 10e/part, every part 100 HP; T3 TOUGH absorbs ~333 effective dmg per 100 HP; tower 600@≤5 → 150@≥20; MAX_CREEP_SIZE 50; spawn 3 ticks/part; ~33.3 e/t refill for 100% uptime.
- Self-recovery floor (T3): 4 TOUGH + 9 HEAL. Out-heal one tower unboosted = ~50 HEAL parts (infeasible solo) ⇒ drain/kite for unboosted v1.

---

> **Provenance.** Source corpus = cloned masters of bencbartlett/Overmind (CombatIntel.ts, setups.ts, meleeDefense/rangedDefense/outpostDefense.ts, invasionDefense.ts, swarm/pairDestroyer), CarsonBurke/The-International (combatRequest.ts, spawnRequests antifa(), squadQuotas), TooAngel/screeps (brain_squadmanager.js, config.js), bonzAI SWC writeups, and screepspl.us / docs.screeps.com / screeps-game-api constants. Mapped onto ADR 0031 §2c/D16 by William Archbell's combat-overhaul work (force_sizing.rs / composition.rs / doctrine.rs seed constants).
