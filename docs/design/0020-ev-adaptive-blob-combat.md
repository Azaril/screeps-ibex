# ADR 0020 — Expected-Value-Driven, Adaptive, Blob-Generalized Combat

- **Status:** Decided
- **Date:** 2026-06-19
- **Owner:** combat-AI
- **Provenance:** produced by an 8-agent `ultracode` research workflow plus an adversarial completeness critique; the design here is the synthesis **as corrected by that critique** and the author's reconciliation. The six §8 questions were resolved at the operator interview of the same date.
- **Follow-on ADRs that extend this design:** **0027** (objective→squad lifecycle), **0031/0031a/0031b** (capability-driven force composition + tuning), **0032** (EV-optimal squad assignment), **0034** (rally/travel convergence), **0035** (scout-before-commit / abandon-on-contact), **0036** (opportunistic structure targeting), **0037** (tower-aware neighbour defense).
- **Relationship to prior ADRs:** extends [0008](0008-combat-objectives-and-squads.md) (objective queue / squad manager / tactics model), [0008a](0008a-combat-tactics.md) (the ~55-tactic catalog + EXP-* register), and [0019](0019-combat-position-selection.md) (the per-tile position utility `score_tile` over cached `PositionLayers`). 0019's positioning utility is the substrate; this ADR adds the **squad-level EV decisions above it**, the **blob generalization**, the **adaptivity/anti-counterability layer**, and the **self-play/tournament + room-generation harness** that tunes and gates them.

## 1. Context — the operator brief

> "Identify design options that result in positive expected value for us. I don't know how to tie that into our squad tactics yet. We should be broad self-play, tournaments, robust room-layout generation, and improve the strategy and tactics. We should also generalize to blobs (arbitrary number of creeps with uniform or variety of roles) to ensure we have adaptive behavior, not a fixed set of counterable behaviors."

Four asks: (1) tie **expected value** into squad tactics; (2) broad **self-play + tournaments**; (3) robust **room-layout generation**; (4) generalize to **arbitrary-N heterogeneous blobs** with **adaptive, non-counterable** behavior.

## 2. The EV framing — HONEST version (critique-corrected)

The synthesis proposed "ONE scalar EV currency: expected net HP-exchange per tick, in integer hits," claiming `score_tile` is "already EV in net hits/tick." **That over-claims, and the critique is right to flag it.** Ground truth (`kite.rs:379-508`): `score_tile` returns an `i64` that is a **normalized, `[0,SCALE]`-clamped convex blend** of terms (e.g. the DMG reward at `:495-497` blends `eff` and `kill` 0.6/0.4 then rescales) — it is deliberately squashed to choose *the best tile*, **not** an unbounded hits quantity. The squad-level decisions the synthesis adds are in genuinely different units:

| Decision | Quantity proposed | Unit | Same currency as `score_tile`? |
|---|---|---|---|
| Position (today, 0019) | `score_tile` argmax | normalized `[0,SCALE]` | — (it IS the positioning scorer) |
| Target select | `threat_removed·killprob/ttk` | hits·prob/tick | no |
| Engage/retreat | Lanchester μ = α·A₀ⁿ − β·B₀ⁿ | hitsⁿ | no |
| Blob auction | `capability·EV(goal) − reach_cost` | **undefined** | no |

**Reconciliation (the actual design):** there is **not** one number; there is one **decision principle** — *act to maximize expected net resource-exchange in our favor* — realized as **several argmaxes in their own honest units**, with **explicit conversions** where they meet. Concretely:
- **Positioning** stays the `score_tile` normalized blend (correct for *ranking tiles*; do not pretend it's hits).
- **Target / engage / sizing** use an **integer net-hits ledger** computed from bodies + the threat/heal layers — kept separate from the positioning scalar.
- The **blob auction's cross-goal currency is the one genuinely missing piece** and is a *prerequisite*, not an afterthought: `EV(focus)` (net hits off a creep), `EV(breach)` (hits off a blocking wall × how much it unlocks), `EV(drain)` (tower energy removed → future damage denied) must be put in a **common "future net hits enabled" unit** before the auction can argmax across them. Defining that exchange rate is design work this ADR flags, not hand-waves.

So "tie EV into squad tactics" = **make engage/target/size argmax an explicit integer net-hits computation** (replacing min-by-hits and flat-HP heuristics), keep positioning as-is, and define the auction exchange rate before building the auction.

## 3. CORRECTION — Screeps has NO line-of-sight; the real mechanic is rampart damage-redirect

The critique's headline finding called `focus_damage_inputs` "occlusion-blind (no line-of-sight, no rampart, no off-room)." **A deep dive against the engine source (2026-06-19) shows that framing is wrong**, and the operator's doubt was right. Verified ground truth (engine source + our own [`engine-mechanics.md`](../references/engine-mechanics.md) §2.6/§2.9):

1. **There is NO line-of-sight / occlusion in Screeps combat.** `attack` (`creeps/attack.js:21`), `rangedAttack` (`rangedAttack.js:21`), `rangedMassAttack` (`rangedMassAttack.js:26-30`), `heal`/`rangedHeal`, and tower fire (`towers/attack.js`, range-only falloff, no range cap) are **pure Chebyshev range** checks. Walls/structures between attacker and target do **not** block anything. So summing reachable enemy heal by `get_range_to` (`focus_damage_inputs`, `lib.rs:944-966`) is **CORRECT, not a bug** — and there is no "off-room heal" because intents run per-room over `roomObjects` (cross-room interaction can't happen). The critique imported an RTS LOS assumption that does not exist here.

2. **The real "shelter" is rampart DAMAGE-REDIRECT, not occlusion.** A creep standing on a rampart: single-target `attack`/`rangedAttack` and **tower** fire **redirect to the rampart** (`attack.js:33-36`, `rangedAttack.js:33-36`, `towers/attack.js:27-30`), and `rangedMassAttack` **skips it entirely** (`rangedMassAttack.js:38-40`). The redirect is **ownership-blind**. So a healer/defender on a rampart is unkillable by direct fire until the rampart breaks — this IS the operator-observed "rampart-healer bait," but the mechanism is *damage redirect on OUR fire*, **not** occlusion of THEIR heal. (Heal is never rampart-gated — enemy heal reaches the focus by range regardless.) Our own ADR [0008a](0008a-combat-tactics.md):413 already calls a creep-on-rampart "the single biggest defensive multiplier in the game."

3. **`safeMode` nullifies all non-owner combat** in the room (`attack.js:30-32` and the guard in every combat/heal intent; `engine-mechanics.md` §2.9). Our damage TO anything in an enemy safe-moded room is zero — a hard engage veto.

4. **Melee attack-back** hits the attacker for the target's ATTACK power **unless the attacker stands on a rampart** (`_damage.js:14-21`).

**THE KEYSTONE FIDELITY REQUIREMENT (the real one, and worse than the phantom):** the **offline combat-engine sim must model the rampart redirect**, or it rewards a fantasy. A sim that adds single-target `Attack`/`RangedAttack` damage **directly to the creep** (`resolve.rs:279,294`) contradicts both the real engine and our own `engine-mechanics.md` §2.6 (marked VERIFIED): in such a sim a defender on a rampart dies to direct fire (far too easy), when in reality you must break the rampart first. **Any siege/turtle tactic tuned against a non-redirecting sim is miscalibrated**, so this is a prerequisite for the tournament/room-gen plans ever fielding rampart defenders. `resolve.rs` therefore redirects single-target attack/ranged/tower/dismantle to a co-located rampart (and suppresses attack-back when the damage is redirected); RMA-skip, attack-back exemption and safeMode were already modelled correctly.

**So the corrected keystone is: (a) sim fidelity on the single-target rampart redirect (above); (b) make the LIVE EV target/focus selection RAMPART-AWARE** — a creep on a rampart is effectively shielded from single-target fire + RMA, so its time-to-kill must price breaking the rampart first (or redirect to the rampart, which the structure-focus `breach_redirect` does for *structure* targets but not yet for creep-on-rampart targets); **(c) keep the safeMode engage-veto.** The heal-by-range estimate needs **no** change. This replaces the synthesis/critique "occlusion-aware estimator" keystone.

Other real models to treat as first-class (these stand):
- **`rangedMassAttack` reward** in the blob damage superposition (a ranged blob vs a clustered enemy should value RMA tiles — a different, falloff-shaped reward surface than single-target `ranged_power`; remember RMA skips creeps-on-ramparts).
- **Fatigue / MOVE-parity as a `min`-over-members gate** (analogous to `fragile_hits`): a kite whose *slowest* member can't outrun the enemy commits to a rout.
- **`safeMode`** as an explicit engage veto (above).

## 4. Design options (synthesis, with critique caveats folded in)

Ranked by value/effort. Each notes the caveat that gates it.

1. **Rampart-redirect fidelity + rampart/safeMode-aware "killable" (PREREQUISITE).** The sim (`resolve.rs`) redirects single-target `Attack`/`RangedAttack`/tower damage to a rampart on the target's tile (matching the engine + `engine-mechanics.md` §2.6). On top of that, make "killable / time-to-kill" rampart-aware (a creep-on-rampart costs the rampart's hits first; RMA skips it) and safeMode-aware (zero damage in an enemy safe-moded room). Heal-by-range needs **no** change (Screeps has no LOS). *Effort M.* Gates options 2–4. **This is the keystone — a sim that does not redirect rewards a fantasy where rampart defenders die to direct fire.**
2. **EV target selection + damage spill.** `select_focus_target` is `argmax threat/ttk` over killable hostiles, discarding the unkillable (rampart-shielded, or out-healed). **Kill budget = `hits + maximum same-tick heal`** — the engine nets damage→heal→death (`creeps/tick.js:120-136`), so a target dies only if `damage ≥ hits + heal`; the heal estimate counts creep healers (range-banded) **and energized hostile towers** (`tower_heal_at_range`, ~400→100/tick) — only towers with `≥ TOWER_ENERGY_COST` energy (a drained tower neither heals nor fires; the same energy gate now filters the threat-field tower **damage**). The **damage spill** (`assign_focus_fire` → `SquadDecision.focus_assignments`, both adapters) allocates shooters across EV-ordered targets capping each at its kill budget, so combined fire doesn't over-damage one creep — symmetric to the already-deficit-capped `assign_heals`. *(The safeMode veto belongs to the engage gate, not to target selection.)*
3. **Lanchester engage/retreat gate.** Replaces flat-HP `squad_should_retreat` with `assess_engage`: the fighting-strength balance μ = our − killable-enemy (strength = `dps × ehp^(n-1)`, integer **n=2** default; out-healed/shielded enemies excluded), + the operator's **retreat-is-LOSS** framing — `unwinnable` iff damage we can neither kill (unkillable creeps) nor out-heal (energized hostile towers at the centroid) exceeds our heal/tick. So a **tank+heal siege that out-heals the towers engages + dismantles** (no loss = a win) rather than retreating; a squad that's outgunned retreats **even at full HP**. The **safeMode veto** (`SquadView.enemy_safe_mode`, both adapters) blocks engaging where our combat is nullified. HP floor + re-engage band kept (no yo-yo). The gate is also what gives the weight sweep real signal (a flat-HP gate makes the sweep flat). *(Per-archetype `n` and the boosted-TOUGH tank reduction are refinements on top — see §11.)* Per §8.1: n = 2 for a ranged mirror, 1 for melee/choke, tuned in sim — *not* the 1.56 the synthesis cargo-culted from StarCraft — so it's integer math, dodging the wasm-vs-native `powf` parity landmine (§6). Heal does NOT corrupt μ: the **unkillable gate (§8.2 / D>Hb) runs first**, excluding out-healed targets from μ, so μ only reasons over killable forces. *Effort M.*
4. **Blob role→sub-goal greedy auction over one utility.** Above `decide_squad_with_pathing`, an O(N) auction assigns each member a sub-goal {focus, breach, drain, heal, screen} by `bid = capability·EV(goal) − reach_cost`; each member then runs the *same* single scored search toward its anchor. *Effort L.* **Blocked on the §2 cross-goal currency being defined** — the auction can't argmax across incommensurable `EV(breach)/EV(drain)/EV(focus)` until the exchange rate exists.
5. **Centroid-soft, fragility-weighted blob cohesion** (retire fixed quads for N>4): N-aware radius `K=ceil(√N)`, damage-weighted centroid, `separation` + `claimed` crowd layers. *Effort M.* Caveats: the damage-weighted centroid pulls toward the *most-damaged* (likely-dying, deep-in-fire) creep — there's a real tension between "clamp it so it's safe" and "make it strong enough that armor-faces-threat emerges"; and `separation`/`claimed` is order-dependent (id-sorted for determinism, but greedy-by-id packing quality under splash is unanalyzed). **Use integer/`min` math, no float centroid division on the hot path** (parity).
6. **Self-play tournament + exploitability ship-gate.** `eval/tournament.rs`: an antisymmetric `PayoffMatrix` over a strategy population (default + aggressive/cautious/kite-heavy/advance-heavy `SquadTacticParams`), each pair played symmetric self-play across a **bed basket** — open field, a wall **corridor**, and mutual **tower crossfire** — both sides (side-bias cancelled), scored by the net-HP exchange, reusing `run_managed`. Outputs: zero-sum mean-payoff ranking, **meta-Nash** mixed strategy (fictitious play — the robust randomization weights, the bridge to step 6 adaptivity), and the **exploitability** ship-gate (largest margin any strategy beats a candidate by). `TournamentBudget` tier (§8.4). An **EV-per-CPU-at-large-N** gate (10v10 managed self-play, per-squad-tick bounded ~1.7 ms) guards the blob regime. **FINDING (tournament-backed, not single-bed):** the basket discriminates (default exploitability 0→90), and the **"cautious" engage preset** (`w_taken 1.5, w_dmg 1.0`) **dominates** the field (mean +66, Nash 1.00) — the default (`w_taken 0.5, w_dmg 2.0`) over-weights damage vs safety and is beaten by 90 net HP. The robustness gate still PASSES (90 ≪ gross 1500: no hard counter), so this is a tuning *lead*, not a forced retune — don't adopt it globally off a 3-bed all-ranged basket (overfit); broaden the basket (melee / mixed / asymmetric objective beds with the §8.6 turtle scorer) first, or let the adaptivity layer pick the mix. The basket's own enrichment axis: asymmetric objective beds, scripted-archetype-vs-managed matches, PFSP/behavioral dedup, and formal Elo (≡ mean-payoff for a complete round-robin).
7. **Archetype classifier → preset selector + seeded mixed-strategy draw** (adaptivity). Classify the opponent from `RoomThreatData` into a finite archetype → pick the preset menu; draw the variant via PRNG seeded from `mission⊕tick⊕room` sampling the offline-solved meta-Nash π*. *Effort M.* **Heaviest caveat (critique):** a *finite hand-enumerated* classifier is itself a fixed policy (an adversary builds a blob straddling two archetypes to force misclassification, and "re-classify on sustained loss" eats the loss first); and the seed is only partially private (`room_hash` constant, `tick_bucket` public). Honest framing: a *menu with a partially-predictable selector*, robust **only if** π* is genuinely solved (option 6) **and** the seed is genuinely opponent-unobservable. Ship last, depends on 6.

## 5. The four plans (concrete)

- **Self-play / tournaments:** `eval/tournament.rs` — antisymmetric `PayoffMatrix` (cell = mean `KiteOutcome::score` over a bed basket, played both sides to cancel side-bias) reusing `run_managed`; PFSP/80-20 opponent mixing + behavioral (not param) de-dup to prevent collapse; stalemate scorer (decisive → {1,0}, engaged-timeout → net-HP margin = the discrimination fix for the FLAT sweep, passive-timeout → double-loss with an objective-aware turtle exception); Elo (headline) + meta-Nash π* (robust ranking + the runtime mixing distribution); exploitability ship-gate; **EV-per-CPU at large N**.
- **Room-layout generation:** `eval/scenario_gen.rs` — parameterized seeded *families* over `ScenarioBuilder` (open/swamp / choke(g) / layered-walls / rampart-bunker / tower-nest / mixed-base), `ChaCha8Rng` seeded for byte-identical worlds; fairness via reject-resample + mirror-for-self-play; **adversarial minimax-regret** generation (regret = reference-policy score − agent score, so high regret = real bug not impossible room) with evolutionary editing, worst finds frozen as `EXP-ADV-*` regression beds; **pairwise covering array** (greedy IPOG) over factor levels so every interaction (e.g. "rampart behind a choke with a tower") is covered in tens of rooms; domain randomization over Screeps-grounded bounds incl. **force composition**.
- **Blob generalization:** every `score_tile` term becomes a role/range/HP-weighted reduction over members (damage = superposition of each member's weapon curve incl. **RMA**; safety = most-fragile θ, already `fragile_hits`; proximity = dominant-DPS r*); formation-free cohesion (§4.5); the auction (§4.4) for division of labor; EV-gated sizing (escalate Solo→Duo→Quad→blob on `attack_parts_to_kill==None`). A "20-creep blob" is ONE squad (the auction scales within it, CPU-gated); separately, **`MAX_CONCURRENT_SQUADS` becomes CPU-governor-dynamic** (§8.3 — scales with the CPU bucket, hysteresis-damped) so the count of distinct objectives flexes with budget rather than a hard 4.
- **Adaptivity / anti-counterability:** §4.7 — online archetype adaptation + preset menu + seeded draw over the offline-solved π*, with brittle tactics entered at a *floor probability* so a hard-counter only wins that fraction. Robust *only* with option 6's exploiter gate behind it.

## 6. Cross-cutting risks (hard gates, not notes)

- **Parity (wasm bot vs native sim/tournament):** the new squad-level decisions sit **above** `score_tile` and feed **discrete branches** (engage/retreat, anchor assignment) — a 1-ULP float difference flips a branch and desyncs replay. **Rule: no `powf`/float division on any path that feeds a discrete combat branch.** Lanchester n ∈ {1,2} (integer), centroid/auction in integer/fixed-point. The CPU-Critical "abort the auction" path is a *different decision* — it must be parity-safe (deterministic from the same inputs) or it breaks live==sim.
- **CPU:** boost-aware + rampart/tile-aware threat widens the hottest cached layer; the auction + separation are O(N). Make the `bench.rs` budget a **hard gate with a number**, and add the large-N EV-per-CPU tournament gate.
- **"FLAT sweep" diagnosis:** the synthesis assumes flatness = weak damage signal and bets an L-effort boost rewrite on it. The critique's likelier cause: `[0,SCALE]` **term saturation** in `score_tile`. **Instrument the score-term histogram before** committing to the boost rewrite as the fix.

## 7. Recommended sequence (critique-corrected)

1. **Rampart-redirect sim fidelity fix + rampart/safeMode-aware killability.** Fix `resolve.rs` single-target redirect (+ the wrong test/comment) to match the engine; make ttk/killable rampart- and safeMode-aware. Prove on a rampart-defender + tower regression bed (`ScenarioBuilder` already builds these). *The keystone — the sim must stop rewarding kills that the engine wouldn't allow.* (NOT an "occlusion" fix — Screeps has no LOS.)
2. **EV target selection** (D>Hb unkillable-discard + spill) on top of the now-correct estimator. Smallest high-value behavior change; kills the dogpile-the-sheltered-healer exploit.
3. **Integer-n Lanchester engage/retreat gate** replacing flat-HP hysteresis (validate the heal-fold in sim first).
4. **Tournament substrate** (`tournament.rs` + bed basket + stalemate scorer) — *now* worth building, because it tunes a corrected signal; immediately unsticks the FLAT-sweep finding and becomes the exploiter ship-gate.
5. **Blob core:** define the auction cross-goal currency (§2) → N-aware fragility-weighted cohesion → the role auction. Validate under the step-4 tournament with composition-varying opponents. Make `MAX_CONCURRENT_SQUADS` CPU-governor-dynamic (§8.3).
6. **Adaptivity:** archetype classifier → preset selector → seeded π* draw. Last; depends on a populated, exploiter-gated tournament.
7. **Adversarial room-gen** (`scenario_gen.rs` + regret search + pairwise covering array); runs continuously as the regression frontier.

## 8. Resolved decisions (operator interview, 2026-06-19)

1. **Lanchester `n` → per-archetype integer ∈ {1, 2}.** Integer powers (parity-safe, no `powf`); `n=2` (square law) for instant-acquisition fights like a ranged mirror, `n=1` (linear) where concentration isn't free (melee / chokes / corridors). Which archetype gets which is **tuned empirically** in the sim, not hand-fixed. The `n`-per-archetype table lives in `SquadTacticParams`.
2. **Heal → explicit unkillable gate (D>Hb) FIRST, then μ over killable forces.** Apply the kill-inequality we already have (`attack_parts_to_kill`, `damage.rs`): if a target's effective-HP regen ≥ our focusable net DPS it is **unkillable** — exclude it from focus and from μ entirely (route to drain/dismantle/disengage). μ then only reasons over genuinely-killable forces, so the heal-as-negative-β approximation never corrupts μ's sign. This subsumes the engage-gate trust concern and reuses existing code.
3. **`MAX_CONCURRENT_SQUADS` → CPU-governor-dynamic.** The cap scales with the available CPU bucket each tick rather than a hard 4 (so multi-threat flooding isn't starved when CPU allows, and we shed under pressure). **Risk to manage:** churn — squads created/dropped as the bucket swings; damp with hysteresis on the cap + a floor so an in-progress assault isn't abandoned mid-fight. (Blobs still scale *within* one squad via the auction; this is about how many distinct objectives run at once.)
4. **Offline tuning budget → a tunable budget TIER, not a fixed bound.** The tournament/exploiter/adversarial-search `G·P·R` is a **runtime parameter** with at least two presets: **minutes** (CI / iteration default — small population, modest exploiter, runs per-change) and **hours** (final evaluation — large league, deep exploiter + adversarial search, run manually). Same code, budget knob picks the tier.
5. **Mixed-strategy seed → simple `mission⊕tick⊕room` is good enough for now.** Realistic MMO opponents don't model us to the depth needed to recover the schedule; robustness comes mainly from archetype adaptation + the offline exploiter gate. Revisit (private entropy source) only if we face a provably adaptive adversary.
6. **Stalemate scoring → objective-aware, holding defender wins.** An attacker must make real progress (structure razing / damage) or it scores as the attacker's **loss**; a defender that repels the assault with a positive HP-exchange **wins**. Turtling that works is correct play and must not be punished as "passivity." (True mutual do-nothing is still a double-loss.)

## 9. Decision

Adopt the corrected design and the §7 sequence: the decision principle is *act to maximize expected net resource-exchange in our favor*, realized as several argmaxes in their own honest units (§2), on top of a sim that models the rampart redirect faithfully (§3). Steps 1–4 (rampart-redirect fidelity + rampart/safeMode-aware killability, EV target selection + spill, the integer-n Lanchester engage/retreat gate, the tournament + exploitability ship-gate) form the core; steps 5–7 (blob role auction, adaptivity, adversarial room-gen) are the deferred outer ring described in §11, gated on the cross-goal EV currency. **Note for reviewers:** the original "occlusion-aware estimator" keystone (from the research critique) was wrong — Screeps has no line-of-sight; see §3.

## 10. Path to MMO deploy (deployability constraints)

Deployment of this combat stack is a **whole-bot** decision, and the operator's posture is
**attack/offense on**, not defense-only. Three constraints shape how the combat slice is allowed to go
live, and they are design decisions, not process notes:

- **Offense is a runtime flag, not a build flag.** Offense is gated behind
  `Memory._features.military.offense` (`war.rs:575` early-returns when off) and features reload from
  `Memory._features` every tick, so the flag is a **live console off-ramp** — offense can be switched
  off without a redeploy. `attack_players` is a separate, more conservative flag. This is what makes an
  offense-on deploy recoverable in one console command.
- **`MAX_CONCURRENT_SQUADS` stays static (4) for a first live window.** Combat is
  `StageClass::Always` — never CPU-shed by ADR-0004 design — so the static cap × `MAX_KITE_OPS` ×
  build-once-per-room layer sharing is the conservative bound. Feeding a dynamic CPU-governor loop
  (0020-S5-CAP, §11) into the very subsystem that is the headline CPU risk is explicitly *not*
  first-window material; it needs live CPU-at-scale data first.
- **CPU headroom is measured against MMO limits, not Docker's.** The empire must fit the MMO per-tick
  limit (30 CPU for a new player, +10/GCL, cap 300; a 500 bucket-funded hard cap; 10k bucket) — a Docker
  budget of 100 proves nothing about the live constraint.

**Deploy-and-watch, not deploy-and-assume.** The position utility, breach/tower-drain and
siege-clear-a-core tactics are validated in sim and on Docker, but a *verified* sim≠engine divergence
(the rampart redirect, §3) is direct evidence that host-green ≠ proof. The pre-live validation is
therefore a forced-combat soak over four scenarios — **A** owned-room defense, **B** invader-core breach
with a core actually observed cleared, **C** SK/stronghold cross-room flee, **D** A+B+C concurrency
against the squad cap — watching that cohesion forms and recovers, that there is no CPU/pathfinding
death-spiral, and that nothing orphan-idles or scatters.

**The live arbiter is the seg-57 canary.** `screeps-ibex-metrics` `CohesionMetrics` emits
`cohesion.avg_in_formation_rate` (~1.0 when forming/idle, recovering after engagements),
`max_pairwise` (the scatter alarm) and `engaged_squads` (the is-there-combat gate); alongside it the
telemetry that matters is `faults.deser_failures` (exactly one bump on an intended reset, then flat),
`vm_starts` (one bump per deploy — repeated bumps mean a panic/halt loop), `cpu.used`/`bucket`/
`bucket_trend`, `governor.tier` (returns to Normal) and `segment_chunks_used`. The canary only
populates once squads exist, which is why the first window must contain real combat.

**Residual risks the design accepts:** tactics firing live for the first time (mitigated by the forced
soak + the canary from the first engagement); combat never being CPU-shed (mitigated by the static cap
and bounded floods, with the non-combat governor still protecting the bucket); squad identity being
interim-keyed by `SquadContext` Entity until a minted `SquadId` exists, so aliasing must be watched as
a *symptom* in cohesion/squad behavior rather than as a counter that does not yet exist.

## 11. Scope boundary — the outer ring

The design has a deliberate inner/outer split. The inner ring is §7 steps 1–4 plus the §12 sizing
solver; the outer ring below is equally part of the end state but each piece has a **prerequisite** that
must exist first, and naming that prerequisite is the point of this section:

| ID | Item | Prerequisite / rationale |
|----|------|--------------------------|
| **0020-S5** | Blob role→sub-goal greedy auction + N-aware fragility-weighted cohesion | Gated on its own prerequisite: the **cross-goal EV currency** — a common "future net hits enabled" unit for EV(focus)/EV(breach)/EV(drain) (§2, §4.4). The single-squad **force-sizing oracle** (§12) is the deploy-relevant subset and does *not* need that currency, which is why it comes first. |
| **0020-S5-CAP** | `MAX_CONCURRENT_SQUADS` → CPU-governor-dynamic | Needs live CPU-at-scale data; a feedback loop into the headline-risk subsystem is not first-window material (§10). |
| **0020-S6** | Archetype classifier → preset selector + seeded mixed-strategy draw | Consumes the step-4 meta-Nash mix, so it ships behind a populated, exploiter-gated tournament (§4.7's caveat: a finite hand-enumerated classifier is itself a fixed policy). |
| **0020-S7** | Adversarial room-gen (minimax-regret + pairwise covering array) | A continuous regression frontier with zero runtime impact (`eval/scenario_gen.rs`). |
| **0020-CAUTIOUS** | Cautious-engage retune lead (`w_taken 1.5/w_dmg 1.0` beats the default `0.5/2.0` by ~90 net HP, Nash 1.00) | A *lead*, not a retune: adopting it off a 3-bed all-ranged basket would overfit. Broaden the bed basket (terrain / tower-pressure beds) first; the robustness gate passes meanwhile, so the shipped default is safe. |
| **0020-TOUGH** | Boosted-TOUGH net-damage conversion (ThreatField safety/veto + Lanchester μ) | Needs a `boost` field threaded through `CombatBodyPart` + a most-fragile-member reduction. Unboosted v1 **over-flees**, which is the safe-conservative direction. |
| **0020-N** | Per-archetype Lanchester n ∈ {1,2} | `n=2` as a fixed default is safe; per-archetype selection is empirical tuning in `SquadTacticParams` + `assess_engage`. |
| **0020-S4-RES** | Tournament enrichment: asymmetric objective beds (+ the §8.6 turtle scorer), scripted-vs-managed matches, PFSP/behavioral dedup, formal Elo | Harness-only; sharpens the exploiter gate the adaptivity layer depends on. |
| **0019-S4-TUNE** | ADR 0019 Stage-4 tuning: engage-stickiness + a weight-**discriminating** sweep bed | The sweep is flat on melee beds; a terrain/tower bed must exist before the weights can be tuned meaningfully (the §6 "FLAT sweep" caveat). |

## 12. Force-sizing solver

The single-squad force-sizing solver: the deploy-relevant subset of the blob work that does **not**
need the §2 cross-goal EV currency. The operator's chosen depth here is **force-intelligence first** —
make the bot stop fielding losing squads before making it field cleverer ones.

### 12.1 Phases

| Phase | What |
|---|---|
| **P1** | Healer heal-coverage positioning (ADR [0019](0019-combat-position-selection.md) §8/§8.1) |
| **P2a** | EV winnability **GATE** + tower-drain EV — the "can a single squad win?" half of §12.2 |
| **P2b** | **Force-DRIVEN composition sizing** (§12.5) — size the squad to the Lanchester-favorable force; the "what composition wins" half of §12.2. Realized by `RequiredForce`/`sized_for`/`win_probability`/`importance_margin` + the dynamic body builder, consumed by war.rs's InvaderCore arm, the SK mission and the eval, with the RANGED kill parts force-sized (`RequiredForce.ranged_parts`, priced off the ranged ceiling) so a dismantle-immune core is not deferred as "breach too slow for one creep lifetime" |
| **P3** | Watch a winnable clear end-to-end on the private server + the §10 force soak A–D |
| **P4** | Whole-bot hygiene + operator go-ahead → live deploy (defense + smart core offense, `offense` default-true, conservative, watching the seg-57 canary) |
| **P5** | Identity (a minted `SquadId`) → multi-squad **G4-HEAVY** for towered strongholds (the phase-2 oracle defers those to it) → **S5** full blob auction (needs the cross-goal EV currency) → **S6** adaptivity, with `attack_players` still off |

**Why this order:** P2 is what makes offense *safe and effective* live — it stops the bot fielding
losing squads, the original complaint — and is the headline "better combat" lever, so it precedes the
first live window. G4-HEAVY / strongholds is a genuine later tier (the first tier is single-tower cores)
and belongs with the identity work in P5.

### 12.2 P2 — the force-sizing oracle (design)

Invert the forward Lanchester (`assess_engage`, lib.rs:849-909) into a **required-force** model: given a target's defense profile, decide (a) **can a single squad win** and (b) **what composition wins** within an HP/time budget — replacing the tower-count proxy at `war.rs:899-905`.

- **Intel enrichment.** The serialized `RoomThreatData` (threatmap.rs:56-90) carries: **breach-relevant rampart hits** (NOT all ramparts — see §12.3), **per-tower energy**, and a **repair-rate estimate** (tower self-repair + enemy WORK repair). `ThreatAssessmentSystem` (threatmap.rs:238-344) populates them when the room is visible.
- **The oracle (pure, decision/eval crate, live==sim — no fork):**
  - **Tower damage at the assault tile** = Σ over *energized* towers of `tower_attack_damage_at_range(range from assault tile)` → the HEAL/tick the squad must out-heal at that position.
  - **Out-heal feasibility** sizes HEAL parts; if no single-squad HEAL load out-heals it → take the **drain path**.
  - **Breach time** = breach-relevant rampart hits ÷ squad dismantle DPS; must finish inside the squad's heal-sustained effective-HP budget AND a tick budget.
  - **Tower-drain EV** (replacing a "towers unkillable ⇒ veto" rule). A tank soaks tower fire at the edge; each shot costs the tower 10 energy; drain time ≈ Σ `tower_energy` ÷ (10 × shots/tick), bounded by the tank's heal-sustained survival. Positive-EV when `drain_time + breach_time < budget` and the tank survives — then assault the drained base. This is the principled successor to the `MAX_SINGLE_SQUAD_STRONGHOLD_TOWERS` heuristic.
  - **Composition sizing** → pick + size the `SquadComposition` (HEAL / WORK-dismantle / TOUGH counts, capped at RCL energy). If no single-squad force wins → **defer to G4-HEAVY (P5)** — but via a real force calculation, not a tower count.
- **Wiring (`war.rs`).** `assess_required_force(defense_profile) -> ForceAssessment { winnable, composition, est_ticks }` (decision/eval crate, pure + tested) replaces the tower-count proxy gate (`war.rs:899-905`) with `!winnable ⇒ skip`, and `assessment.composition` feeds `ForceRequirement::single(...)` (war.rs:943-947) in place of a hardcoded `siege_quad`. `AttackCandidate` (war.rs:72-87) is transient → free to carry the profile.
- **Dynamic vs players.** The oracle is a pure function of the *observed* defense, recomputed each scan as intel refreshes, with **no opponent-specific constants** — so it scales to any defender including players whose defense changes. The full adaptive mixed-strategy (anti-exploitation vs an adversary modelling us) is S6/P5; `attack_players` stays off for the first tier.

### 12.3 Rampart relevance — ONLY objective/tower-gating ramparts count

**Not all ramparts are equal.** A base has many ramparts; summing them all would massively *overestimate* breach cost and falsely mark winnable targets unwinnable. Only the ramparts that actually gate the assault matter:

1. **Ramparts on the breach corridor to the objective** (the dismantle target) — already isolated by the existing **`breach_path_blockers` Dijkstra kernel** (the same one driving controller breach-corridor dismantle priority).
2. **Ramparts shielding the towers** we must remove on the **drain/kill path** (only when that path requires breaching a tower's rampart).

So `rampart_hits` in the enriched intel = the hits of the **breach-corridor blockers to the objective** (+ for the drain path, the **tower-guarding ramparts**) — **never a room-wide rampart sum**. Interior/decorative/peripheral ramparts and ramparts over unrelated structures are irrelevant to the breach-cost estimate. Reuse `breach_path_blockers` (lib.rs breach kernel) as the source of truth; do not re-derive a one-off rampart scan (see [[no-one-off-pathfinding-algorithms]]).

### 12.4 Seams

- **`war.rs`** (operations): `AttackCandidate` 72-87 (transient, extend freely); `tower_count` from `hostile_tower_positions.len()` :733 (a raw count includes *drained* towers — price them by per-tower energy instead); `estimated_dps/heal` :778-779; **the proxy gate this replaces** :899-905; the composition match :884-926; `ForceRequirement::single` :943-947.
- **`military/threatmap.rs`**: `RoomThreatData` 56-90 (**serialized — enriching it changes persisted shape**); `HostileCreepInfo` 92-159 (bodies already captured); `ThreatAssessmentSystem` 238-344 (populate the new fields when visible).
- **`screeps-combat-decision/src/lib.rs`**: `assess_engage` 849-909 (forward model to invert); `heal_reaching` 250-275; `breach_redirect` 1203-1277 + the `breach_path_blockers` kernel (the §12.3 rampart-relevance source).
- **`military/composition.rs` / `bodies.rs`**: the composition/body factories the oracle sizes.
- **Build/test gate:** `cargo test` (decision/eval/ibex) + `check-wasm` + `clippy-wasm` warning-free.

### 12.5 Force-driven composition sizing (P2b) — closing the open loop

The other half of §12.2(b): make the composition an output of the assessment.

**The problem.** Three uses of one Lanchester model pull apart if they are not tied together:
- the **gate** (`war.rs`) asks "does the *fixed* comp win?" — building a `ForceBudget` FROM a hardcoded `siege_quad`'s `capabilities()` and using `assess()` only as go/no-go (composition → budget → verdict, **backwards**);
- **sizing** is energy-driven (`queue_slot_spawn` → `body_definition(home_energy)`), with **zero coupling to enemy strength** — sizing to "the strongest home's energy" merely swaps *which* energy and is still a band-aid;
- the **runtime retreat** (`assess_engage`, lib.rs:867-915) reacts to the squad's **actual spawned** strength.

⇒ a squad fielded under-strength (spawned to *energy*, not to *win*) correctly computes "unwinnable" at runtime and retreats — the **engage/retreat cycle**, seen live as an SK duo that trickles in and bails, so suppression never holds and mining never starts. The oracle's `required_heal_per_tick`/`required_dismantle_dps` are dead-ends unless something consumes them.

**The principle.** Size to the **smallest force whose Lanchester balance is favorable** (with a margin). That single computation yields all three needs at once: the **minimum size**, the **go/no-go gate** ("can an in-range home afford that force?"), and a **runtime that holds** (sized above the retreat band, so `assess_engage` won't bail). Sizing-to-the-best-home then stops being needed at all.

**Design — the inversion (`required-force → composition → spawn`):**
1. **Required-capability solver.** Promote the oracle's outputs from diagnostics to contract: emit required `heal_per_tick`, `structure_dps`, `tank_ehp` + mode, sized so `our_strength ≥ killable_enemy_strength × (1 + ENGAGE_MARGIN)` using the **same Lanchester μ as `assess_engage`**, so the runtime gate is satisfied *by construction*.
2. **Capabilities → part targets** (the inverse of `SquadComposition::capabilities()`, built in P2a): `heal_parts = ceil(req_heal / HEAL_POWER)`, `work|attack_parts = ceil(req_dps / DISMANTLE_POWER|ATTACK_POWER)`, tough/total from `req_ehp`. Per-role target part counts.
3. **Part targets → body.** Decision **D1**: *(v1, recommended)* **energy-from-parts** — invert the template's repeat math (inverse of P2a's `part_count`) to find the energy budget that yields the target parts, then drive the existing `body_definition(budget)`/`create_body` path with THAT budget instead of `best_home_capacity` (reuses everything; constrained to the template's part RATIO). *(later)* a **dynamic body builder** targeting arbitrary role part-counts (MOVE-balanced, ≤50) — only if the required HEAL:DPS:TOUGH mix must diverge from the template ratio.
4. **Member-count / role-mix.** v1 keeps the composition's role STRUCTURE (siege = tank + healer(s) + dismantler(s)) and sizes each member's parts; if the 50-part cap is hit, scale member COUNT within the structure. Full role re-allocation across a blob = **0020-S5** (needs the cross-goal EV currency) — out of scope here.
5. **Consume it (`war.rs`/`SquadManager`).** Replace the hardcoded `siege_quad()` with `size_composition(assessment) -> SquadComposition`; the **gate becomes "can an in-range home afford the required force?"** (subsumes `best_force_budget` and any best-home sizing band-aid). Unaffordable ⇒ defer (G4-HEAVY / wait for RCL) — same conservatism, now for the right reason.
6. **Runtime consistency (free).** Sized above the retreat band ⇒ `assess_engage` holds. No separate "hold posture" needed in the common case — correct sizing *is* the hold.

**SK farming (the live failure this fixes).** Without this, the SK path has **zero keeper-strength coupling** — the duo is sized by home energy, never against the keepers, and runs the offense retreat gate (bailing mid-suppression). P2b gives SK a `DefenseProfile` from the **keepers** (DPS/HP/count) and sizes the duo to out-heal/hold them; the affordability/ROI gate then = "can this home field a keeper-suppressing duo?" (if not, don't farm — correct). A distinct suppression *hold-posture* may still help, but correct sizing is the primary fix.

**Structural blockers / open decisions:**
- **D1 — body builder:** energy-from-parts (v1, reuse templates) vs dynamic part-targeting builder. Recommend v1; the `repeat_body: &'static [Part]` constraint is what makes part-targeting non-trivial.
- **D2 — margin:** `ENGAGE_MARGIN` (how far above break-even to size) — a tunable seed; align with `assess_engage`'s retreat band so sizing and runtime agree.
- **D3 — placement:** the pure capability/part math in `screeps-combat-decision` (or `military::force_sizing`); the body/composition synthesis bot-side (it touches `SquadComposition`).
- **D4 — WFV:** likely **NONE** — `SquadComposition`/`ForceRequirement` shapes are unchanged (different VALUES of the same serialized types). Confirm no new fields before committing.
- **D5 — SK keeper intel:** add keeper DPS/HP/count to the SK `DefenseProfile` (NPCs with known bodies — cheap to read when the room is visible).

**Scope boundary.** P2b = size ONE squad to the required force for ONE objective (a single Lanchester balance), within the fixed role structure. The general version (role auction across a blob, multi-squad sequencing) is **0020-S5**, gated on the cross-goal EV currency — which P2b does **not** need, so P2b is buildable now.

**Validation.**
- Host: required-capability solver (defense → caps with margin); part-target mapping round-trips with `capabilities()`; `size_composition` yields a comp whose `capabilities() ≥ required`.
- Sim/eval (EXP harness): a managed squad **sized to win holds** (no retreat) vs the defense it's sized for; an under-affordable scenario is **gated off** (not committed).
- Live: the SK duo **holds + suppresses W6N4** (the regression that started this); a winnable core clear.

### 12.6 The ladder to the full auction (build order)

P2b is built as **independently-shippable rungs** that each refine behavior AND build a primitive the full **0020-S5** auction reuses — so value lands early and S5 becomes "the last two rungs," not a from-scratch XL. (Effort is relative T-shirt sizing.)

| Rung | What | Ships (value) | The primitive it builds for S5 | Effort |
|---|---|---|---|---|
| **R1** | **Dynamic body builder** — `build_combat_body(part_spec, move_policy, max_energy)`: MOVE-balanced for the intended speed (off-road combat ≈ 1:1 MOVE:non-MOVE; roads/boosts looser), survivability-ordered (TOUGH front), ≤50 parts, cost-clamped. Pure, host-tested. | nothing yet (primitive, unwired) | the auction emits per-role part-specs → **this builds them** | M |
| **R2** | **Required-force → part-spec** — promote the oracle's `required_heal/dps/ehp` to a contract; invert `SquadComposition::capabilities()` (P2a forward) into target part counts. Pure, tested. | nothing yet | the auction's per-role budget → part-spec via this map | S–M |
| **R3** | **Wire single-squad sizing** — `size_composition(assessment)` = R2∘R1; `war.rs` fields it instead of the hardcoded `siege_quad`; gate becomes "can an in-range home afford the required force?" (subsumes the best-home sizing band-aid). | **offense squads sized to win** (first behavior change + live value) | the consume-point that S5's auction output plugs into | M |
| **R4** | **P(win) model** — `assess_engage` balance → **P(win)** (logistic over the margin; distributional later). Sizing targets a P(win) threshold instead of a hard balance margin. | smoother, less brittle sizing | EV needs P(win), not a boolean | M |
| **R5** | **Importance·P(win) investment** — size to maximize `importance·P(win) − cost` (importance from `OBJECTIVE_PRIORITY_*`). Scales the hammer to the target's value. | **proportional investment** (minimal force for marginal targets, overwhelming for strategic ones) | this IS the single-goal case of the EV the auction maximizes | M |
| **R6** | **SK application** — keeper `DefenseProfile` (DPS/HP/count) + size the duo to *hold* the keepers; fixes the live SK-suppression failure. | **SK farming actually works** | force-sizing generalized to a non-breach goal (suppress) | S–M |
| — | *(R1–R6 together are P2b: single-squad, single-objective, EV-sized)* | | | |
| **R7** | **Cross-goal EV currency** — a common "future net hits enabled" unit for `EV(focus)/EV(breach)/EV(drain)/EV(heal)/EV(screen)` (the S5 gating prereq). | — | the unit the auction allocates in | M–L |
| **R8** | **Role→sub-goal auction** — greedy argmax-marginal-EV allocation of the part-budget across roles, using R1 (build) + R5 (EV) + R7 (currency). | adaptive blob composition | **the full auction** | L |
| **R9** | **N-aware fragility cohesion + multi-squad sequencing** (needs I1/I2 SquadId) | blob cohesion at scale, G4-HEAVY | multi-squad coordination | L |

**Start: R1** (the dynamic body builder) — foundational, isolated, host-testable, no behavior change, and every later rung depends on it. Then R2→R3 (first live value), R4→R5 (EV), R6 (SK). R7–R9 are S5, and wait until P2b is validated live.

**Reuse before building: force-matched sizing already exists for DEFENSE.** `military/bodies.rs::sized_defender_body` sizes offense to `damage::attack_parts_to_kill(target_hp, enemy_heal, window, dmg)` and heal to `damage::defender_heal_parts_for_dps(incoming_dps)`, assembled by `assemble_combat_body` (budget-degrading, TOUGH-front). The OFFENSE/SK paths simply don't use it — they field fixed templates. So P2b is mostly *wiring the existing defense primitives to offense/SK*, not building from scratch: **R1** generalizes `assemble_combat_body` → `build_combat_body` (full part spec + `MoveProfile`); **R2** reuses the `damage::*_parts_*` helpers to turn the oracle's `required_*` into a `CombatBodySpec`; **R3** wires it. (`damage::drain_heal_parts_for_dps` covers the SK/drain case.)

**The sizing primitives (R1–R3).**
- **R1 — `bodies::build_combat_body(CombatBodySpec, MoveProfile, max_energy) -> Option<Vec<Part>>`:** TOUGH-front + round-robin fill; MOVE ratio per terrain (combat default = Plains 1:1); `None` when the spec doesn't fit 50 parts / the energy budget — and that `None` **is** the solver's "can't afford" signal, not an error.
- **R2 — `force_sizing::RequiredForce::from_assessment()`**, the inverse of `capabilities()`: the oracle's `required_heal`/`required_dps` become total `{heal_parts, dismantle_parts, tough_parts}` (heal via `damage::defender_heal_parts_for_dps`); `as_solo_spec()` bridges to R1. `tough_parts` starts at 0 — the EHP margin arrives with R5.
- **R3 — the spawn seam:** `BodyType::Sized(CombatBodySpec)` is an **appended** enum variant carrying a force-sized body, and `BodyType::build_body(energy, MoveProfile)` is the single spawn entry (Sized → the R1 builder; template → `create_body`), used by `queue_slot_spawn`. `SquadComposition::sized_for(RequiredForce, max_member_energy)` distributes the required parts across role slots (Healer→heal, Dismantler→work, Tank→tough; even split), sets each to `Sized`, and returns `None` if any member can't fit — the "can't afford ⇒ defer" signal. Unsized roles keep their template. Everything that inspects a composition (`capabilities`/`estimated_*`/`part_count`) must handle `BodyType::Sized`; an unhandled arm is a halt, not a fallback.

**`HOLD_MARGIN = 1.3` — size to hold, not to break even.** The oracle and `assess_engage` are *separate* models, so sizing heal to ≈break-even is fragile: the first damage can trip the runtime retreat. Heal is therefore sized to out-heal incoming ×1.3, which keeps HP recovering through damage (no degradation → `assess_engage`'s `tower_dps > our_heal` veto stays clear → the squad HOLDS). The same margin is the **commit gate**: field only if the margin-force is affordable; never field a break-even squad — defer instead. The regression that pins this is `sk_setup_fields_a_holding_composition_or_defers_never_undersizes`, swept across keeper strengths (weak ⇒ fielded, strong ⇒ fielded with margin, overwhelming ⇒ defers).

**R4 — P(win) instead of a boolean.** `force_sizing::win_probability(heal, incoming)` is a logistic on the heal surplus: 0.5 at break-even, ≈0.82 at the +30% `HOLD_MARGIN`, →1 when nothing hits us. It is the principled reading of the magic 1.3 ("field enough to win about four times in five"), and it is the seam through which a fixed margin can later be replaced by a per-objective P(win) *target*.

**R5 — importance-weighted investment.** `force_sizing::importance_margin(importance ∈ [0,1]) = 1 + importance × 0.5`, with `RequiredForce::scaled(factor)` (ceil; zeros stay zero; <1 clamps to a no-op). `war.rs` normalizes the objective priority (`(p − LOW)/(CRITICAL − LOW)` — a MEDIUM core → 0.33 → ~1.17× over-invest) and scales the required force, so a higher-value target fields a higher-P(win) squad. The full EV trade (importance·P(win) − cost *across* goals) needs R7's cross-goal currency.

**The runtime-hold property.** The property the sizing must satisfy, and the one the harness asserts (`force_sized_squad_keeps_holding_while_damaged`): a squad sized to out-heal the incoming holds and keeps dismantling not only at full HP but while **damaged** (60%) — no early retreat; only at critical (<25%) does the sanctioned individual flee fire; an under-sized squad retreats. Correct sizing *is* the hold posture.

**R6 — SK keeper sizing.** `SourceKeeperFarmMission` force-sizes the suppression duo's HEALER to out-heal a Source Keeper (`SK_KEEPER_MELEE_DPS = 168` — the melee DPS a keeper lands if it catches the kiter) × `HOLD_MARGIN`, at the strongest in-range home's energy (the same energy the spawn path sizes a `Sized` body against). The root cause of a duo that "never sustains" is a *template* healer whose heal caps below 168 at a low-energy home (`maximum_repeat` never reached); making the heal target explicit against the keeper threat removes that accident. When no home affords the sized healer (low RCL), `sized_for` returns `None` and the duo falls back to the template (the spawn path still builds the largest healer that home affords). SK suppression is **positional** — mine while the keeper is away — so the duo is sized to *survive a keeper engagement*, not to tank keepers continuously.

**R-attack — size the RANGED kill parts.** Kill RATE, not survivability, is what defers level-0 cores: a core has 100k hits and NO ramparts (`breach_hits = 0`), and a balanced template `QuadMember` brings too few RANGED parts to chew 100k within a creep lifetime, so the oracle reads "breach too slow for one creep lifetime" and skips it forever. Four coupled pieces fix it:
1. **`RequiredForce::ranged_parts`**, threaded through `from_assessment`/`scaled`. `as_solo_spec` stays WORK — a solo uses one weapon, not both.
2. `from_assessment` computes `ranged_parts = ceil(required_dismantle_dps / RANGED_ATTACK_POWER)` — the same kill rate expressed in ranged parts (5× the WORK count).
3. `sized_for` sizes the **`RangedDPS`** slots from `ranged_parts` via `CombatBodySpec::ranged_attack`; WORK roles keep `dismantle_parts`, so the composition's role structure is what picks the weapon.
4. `capabilities()` computes a ranged attacker's `structure_dps` from the **RANGED CEILING** (the max ranged parts a member can field at the home's energy, probed via `build_combat_body`), not from the balanced template — otherwise `best_force_budget` never sees the DPS a *sized* quad would field and the oracle keeps deferring.

**Engine-truth findings that constrain the offense path.**
- **A core is immune to dismantle.** `StructureInvaderCore` has no `CONSTRUCTION_COST`, so the engine no-ops dismantle on it; it must be **ATTACKED**. An InvaderCore objective therefore routes to a RANGED composition (`quad_ranged` + healers) and the Engaged seam targets the core as a hostile structure — a WORK/`siege_quad` force is useless against it. The oracle still force-sizes the HEALERS to out-heal towers, and the winnability gate still defers un-out-healable strongholds.
- **A formation squad split across a room boundary must hold the crossed member.** If the anchor's `virtual_pos` is frozen in the rear room by the boundary-hold quorum gate, `get_formation_target` hands a member that has already crossed an exit-edge tile, the engine bounces it back out, and the squad ping-pongs on the border forever. The rule: hold a crossed member in place until the anchor advances. This is a general cross-room-squad property, not a core-specific patch.

**SK keeper-kill sizing.** The SK mission also sets `ranged_parts = SK_KEEPER_KILL_RANGED_PARTS` (15 ≈ 5000 HP ÷ 34t ÷ `RANGED_ATTACK_POWER`, the proven full-template suppression rate) alongside the R6 heal sizing, so `sized_for` sizes the KITER to actually **kill** the keeper rather than merely kite it — a dead keeper clears the source for the ~300t respawn (the positional mining gate stays open), where a kited one returns and the miner flees. A keeper carries no HEAL (engine `keeper-lairs/tick.js` = TOUGH/MOVE/ATTACK/RANGED_ATTACK), so net kill rate == gross ranged DPS and no self-heal model is needed. At a low-energy home where the template caps below the kill rate, `sized_for` grows the kiter — the same `maximum_repeat` gap R6 closed for the healer.

**Creep-clear targets (AttackFlag → `Secure`, ResourceDenial → `Harass`) do NOT reuse the InvaderCore arm.** The oracle is **structure-shaped**: `DefenseProfile` has no enemy-creep-HP-to-kill field (`enemy_dps` is incoming-only, an out-heal target), so a creep clear (`breach_hits = 0, objective_hits = 0`) hits the "undefended → trivially winnable" path and sizes to ZERO ranged. And `candidate.defense` is `None` for both arms (ResourceDenial sets it `None`; AttackFlag is built outside the threat-scan loop with hardcoded zeros and `target_pos: None`), so reusing the InvaderCore `(Some(pos), Some(defense))` arm makes AttackFlag fall through to skip and **never fire** — inverting explicit operator intent. The correct home is a **creep-target oracle path**: an `enemy_creep_hits`/`enemy_heal` field, a `clear_creeps` Lanchester branch, AttackFlag intel plumbing, and SIZE-BUT-ALWAYS-FIELD semantics for the operator flag. That is the R8 / §12.7 archetype work, not a P2b bolt-on; until it exists these arms field their own compositions.


### 12.7 Beyond sizing — archetype selection + the full input model

Forward-looking; R2–R6 deliver fixed-structure sizing first, this is where the ladder grows next.

**(A) Archetype selection — *which* roles/body types, not just how many parts.** P2b sizes a FIXED role structure (siege = tank + heal + dismantle; SK = ranged + heal). But the right ARCHETYPE is itself a function of the objective + the opposing force:
- **structure** target (breach/dismantle) → WORK dismantlers + heal (boosted vs deep walls);
- **creep** target (defend/secure/harass) → RANGED kiters (kite the fight) or melee brawlers (cheap / when cornering is fine);
- **towers** (drain) → high-TOUGH tank + heal (soak, don't trade);
- **SK keepers** (suppress) → ranged + heal sized to *out-heal and hold*, not to win-and-leave;
- **mobile player squad** → counter their composition (ranged vs their melee; extra heal vs burst; TOUGH vs sustained dps).

So a rung sits **between R5 (size a structure) and R8 (full role auction): an archetype SELECTOR** — `(objective, force-profile) → role set` — then R2–R5 size it. This SELECTION axis ("right tool for the job") is **distinct from 0020-S6's mixed-strategy axis** ("don't be predictable / anti-exploitation"); both read the same inputs. R8's role auction is the general form (it picks roles by marginal EV, subsuming the selector); a hand-rolled classifier is the heuristic precursor for the common cases. Provisionally **R5.5** (heuristic selector) → folded into **R8**. **The concrete rule-registry realization is designed in [ADR 0026 §9](0026-combat-strategy-selection.md) (the *doctrine* registry — a sibling activator-registry to the combat-strategy selector), which adds the axis (B)'s table omits: the enemy COORDINATION model (NPCs fought individually = size to the worst single; players fight coordinated = size to the aggregate under a square-law Lanchester). The AttackFlag/Harass creep-clear deferral (§12.6) is its `PlayerRaid` rung.**

**(B) The full input model — what the solver needs, by source + what it drives.** The solver should be a pure function of two input groups — the **OBJECTIVE** (what we're trying to do) and the **EXPECTED OPPOSING FORCE** (what resists us) — so archetype, size, mode, margin, and engage policy all derive from the same inputs rather than ad-hoc per call site:

| Input | Source | Drives |
|---|---|---|
| Objective kind (Dismantle/Secure/Harass/Farm/Defend) + goal (kill/breach/suppress/hold/deny) | `CombatObjective` | **archetype**, mode (breach/drain) |
| Target type (structure/creeps/controller/keeper) + hits | RoomData / `DefenseProfile` | archetype, structure-dps need, kill-time |
| Importance / priority | `OBJECTIVE_PRIORITY_*` | **investment scale** (R5: importance·P(win)) |
| Time budget (`CREEP_LIFE_TIME − spawn − travel`) | `estimated_combat_time` | feasibility, breach-vs-drain choice |
| Tower threat (positions + energy + range-to-assault) | `RoomThreatData` (P2a) | heal need, drain mode, tank EHP |
| Breach cost (corridor rampart hits) | `breach_path_blockers` (§12.3) | dismantle-dps need, breach-time |
| Enemy creep force (bodies → DPS/heal/TOUGH, count) | `RoomThreatData.hostile_creeps` / keeper bodies | **archetype (counter)**, kill-time, our heal need, P(win) |
| Repair rate (tower/creep rampart repair) | `DefenseProfile` (P2b D5) | net breach-dps need |
| Safe-mode | `RoomThreatData` | hard veto |
| Expected reinforcement / escalation (players: predicted incoming; NPCs: keeper respawn 300t, invader waves) | intel + heuristics (**FUTURE**) | margin, archetype, commit-at-all |
| Our boosts available | lab/mineral state (0020-TOUGH/S2, **FUTURE**) | effective part power → smaller bodies, archetype |

Most inputs exist (P2a/P2b); the FUTURE rows (expected reinforcement/escalation, our boosts) are S6/S2. **Design implication:** the solver's signature should take an `ObjectiveContext` + a `DefenseProfile` (the expected opposing force), not just a defense snapshot — the same two inputs feed archetype selection, sizing, mode, margin, and the engage policy.

## Landed

- 0d262c8 sim rampart damage-redirect fidelity (the §3 keystone) (2026-06-20)
- 2256c9f EV target selection + damage spill over the kill budget (2026-06-20)
- eabaed6 Lanchester engage/retreat gate with the safeMode veto (2026-06-20)
- b73e344 self-play tournament, meta-Nash mix + exploitability ship-gate (2026-06-22)
