# ADR 0020 — Expected-Value-Driven, Adaptive, Blob-Generalized Combat

- **Status:** **Proposed (needs operator approval before implementation).** Produced by an 8-agent `ultracode` research workflow (internal tactics audit + external Screeps-bot / RTS-AI / self-play / room-gen / heterogeneous-swarm research) + an adversarial completeness critique, 2026-06-19. This ADR records the synthesis **as corrected by the critique** plus the author's reconciliation; it does not authorize code yet.
- **Owner:** combat-AI
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

## 3. The single most important correction — fix the inputs before the math

The critique's headline finding (verified): **`focus_damage_inputs` (`lib.rs:944-966`) is occlusion-blind.** It sums *all* hostile HEAL parts by raw `get_range_to`, with **no line-of-sight, no rampart/wall occlusion, no off-room check, no safe-mode**. This is the mechanism behind the operator-observed live failure (dogpiling a rampart-sheltered healer / feeding a heal wall). **Every** downstream EV decision (target killprob, engage μ, the safety term) reads damage/heal estimates built on this. Tuning or adding Lanchester/boost layers on top of an occlusion-blind heal/threat estimate **optimizes the wrong objective faster.**

**Therefore the first slice is the occlusion/reachability fix, not the tournament.** (The synthesis ranked the tournament first; the critique's re-ranking is correct and adopted in §7.)

Related missing models the critique surfaced, to treat as first-class (not afterthoughts):
- **Line-of-sight / structure occlusion** for reachable-heal and "is this target actually killable through its shield." The biggest omission.
- **`rangedMassAttack` reward** in the blob damage superposition (a ranged blob vs a clustered enemy should value RMA tiles — a different reward surface than single-target `ranged_power`, currently unmodeled).
- **Fatigue / MOVE-parity as a `min`-over-members gate** (analogous to `fragile_hits`): a kite that doesn't know its *slowest* member can't outrun the enemy commits to a rout. Add a min-MOVE-ratio gate to the kite/engage decision.
- **`safeMode`**: an enemy that can deny all damage for N ticks dominates the engage decision; the EV math needs a term for it.

## 4. Design options (synthesis, with critique caveats folded in)

Ranked by value/effort. Each notes the caveat that gates it.

1. **Occlusion-aware threat + heal estimator (PREREQUISITE).** Make `focus_damage_inputs` + the `ThreatField` honor LOS/rampart/off-room/safe-mode so "reachable heal" and "killable" are real. *Effort M.* Gates options 2–4. **This is the keystone — without it the rest tunes a wrong signal.**
2. **EV target selection (D>Hb discard + spill-aware focus matrix).** Replace unconditional healer-first min-by-hits (`lib.rs:202`) with `argmax_e threat_removed·killprob/ttk`, discarding unkillable (heal ≥ our focusable DPS *through occlusion*), greedy-claim with damage-spill decrement. *Effort M.* **Only positive-EV once option 1 lands** (else it reuses the occlusion-blind estimator and keeps the bait).
3. **Lanchester engage/retreat gate** replacing flat-HP hysteresis (`squad_should_retreat`, `lib.rs:695`). μ = α·A₀ⁿ − β·B₀ⁿ, hysteresis band around μ=0. **Use integer n ∈ {1, 2}** (square-law for ranged-mirror, linear else) — *not* the 1.56 the synthesis cargo-culted from StarCraft — so it's integer math, dodging the wasm-vs-native `powf` parity landmine (§6). *Effort M.* Caveat: Screeps heal is **regeneration, not attrition** — validate the heal-fold in sim before trusting μ near zero (an unkillable target wants "drain + outlast," which μ-sign mishandles).
4. **Blob role→sub-goal greedy auction over one utility.** Above `decide_squad_with_pathing`, an O(N) auction assigns each member a sub-goal {focus, breach, drain, heal, screen} by `bid = capability·EV(goal) − reach_cost`; each member then runs the *same* single scored search toward its anchor. *Effort L.* **Blocked on the §2 cross-goal currency being defined** — the auction can't argmax across incommensurable `EV(breach)/EV(drain)/EV(focus)` until the exchange rate exists.
5. **Centroid-soft, fragility-weighted blob cohesion** (retire fixed quads for N>4): N-aware radius `K=ceil(√N)`, damage-weighted centroid, `separation` + `claimed` crowd layers. *Effort M.* Caveats: the damage-weighted centroid pulls toward the *most-damaged* (likely-dying, deep-in-fire) creep — there's a real tension between "clamp it so it's safe" and "make it strong enough that armor-faces-threat emerges"; and `separation`/`claimed` is order-dependent (id-sorted for determinism, but greedy-by-id packing quality under splash is unanalyzed). **Use integer/`min` math, no float centroid division on the hot path** (parity).
6. **Self-play tournament + exploiter ship-gate** (anti-counterability harness). Generalize `sweep_kite_weights` into an antisymmetric `PayoffMatrix` over a population (params ∪ scripted archetypes) across a seeded **bed basket**; rank by Elo + meta-Nash; replace the single-bed dominance guard with an **exploitability gate** (a broad exploiter search may not beat the candidate by > TOL). *Effort L.* Caveats: the offline wall-clock/CPU budget is unstated (open question — bounds how broad the exploiter can be); the **passive-timeout double-loss** scorer risks punishing legitimate turtling; **add an EV-per-CPU metric at large N** so an auction/separation design that wins on HP but blows the per-tick budget at N=20 *fails* the gate.
7. **Archetype classifier → preset selector + seeded mixed-strategy draw** (adaptivity). Classify the opponent from `RoomThreatData` into a finite archetype → pick the preset menu; draw the variant via PRNG seeded from `mission⊕tick⊕room` sampling the offline-solved meta-Nash π*. *Effort M.* **Heaviest caveat (critique):** a *finite hand-enumerated* classifier is itself a fixed policy (an adversary builds a blob straddling two archetypes to force misclassification, and "re-classify on sustained loss" eats the loss first); and the seed is only partially private (`room_hash` constant, `tick_bucket` public). Honest framing: a *menu with a partially-predictable selector*, robust **only if** π* is genuinely solved (option 6) **and** the seed is genuinely opponent-unobservable. Ship last, depends on 6.

## 5. The four plans (concrete)

- **Self-play / tournaments:** `eval/tournament.rs` — antisymmetric `PayoffMatrix` (cell = mean `KiteOutcome::score` over a bed basket, played both sides to cancel side-bias) reusing `run_managed`; PFSP/80-20 opponent mixing + behavioral (not param) de-dup to prevent collapse; stalemate scorer (decisive → {1,0}, engaged-timeout → net-HP margin = the discrimination fix for the FLAT sweep, passive-timeout → double-loss with an objective-aware turtle exception); Elo (headline) + meta-Nash π* (robust ranking + the runtime mixing distribution); exploitability ship-gate; **EV-per-CPU at large N**.
- **Room-layout generation:** `eval/scenario_gen.rs` — parameterized seeded *families* over `ScenarioBuilder` (open/swamp / choke(g) / layered-walls / rampart-bunker / tower-nest / mixed-base), `ChaCha8Rng` seeded for byte-identical worlds; fairness via reject-resample + mirror-for-self-play; **adversarial minimax-regret** generation (regret = reference-policy score − agent score, so high regret = real bug not impossible room) with evolutionary editing, worst finds frozen as `EXP-ADV-*` regression beds; **pairwise covering array** (greedy IPOG) over factor levels so every interaction (e.g. "rampart behind a choke with a tower") is covered in tens of rooms; domain randomization over Screeps-grounded bounds incl. **force composition**.
- **Blob generalization:** every `score_tile` term becomes a role/range/HP-weighted reduction over members (damage = superposition of each member's weapon curve incl. **RMA**; safety = most-fragile θ, already `fragile_hits`; proximity = dominant-DPS r*); formation-free cohesion (§4.5); the auction (§4.4) for division of labor; EV-gated sizing (escalate Solo→Duo→Quad→blob on `attack_parts_to_kill==None`). **Resolve `MAX_CONCURRENT_SQUADS=4` (`squad_manager.rs:53`)** — it's the real blob ceiling; a "20-creep blob" is either one squad (auction does real work, CPU-gated) or blocked by the cap.
- **Adaptivity / anti-counterability:** §4.7 — online archetype adaptation + preset menu + seeded draw over the offline-solved π*, with brittle tactics entered at a *floor probability* so a hard-counter only wins that fraction. Robust *only* with option 6's exploiter gate behind it.

## 6. Cross-cutting risks (hard gates, not notes)

- **Parity (wasm bot vs native sim/tournament):** the new squad-level decisions sit **above** `score_tile` and feed **discrete branches** (engage/retreat, anchor assignment) — a 1-ULP float difference flips a branch and desyncs replay. **Rule: no `powf`/float division on any path that feeds a discrete combat branch.** Lanchester n ∈ {1,2} (integer), centroid/auction in integer/fixed-point. The CPU-Critical "abort the auction" path is a *different decision* — it must be parity-safe (deterministic from the same inputs) or it breaks live==sim.
- **CPU:** boost-aware/occlusion-aware threat widens the hottest cached layer; the auction + separation are O(N). Make the `bench.rs` budget a **hard gate with a number**, and add the large-N EV-per-CPU tournament gate.
- **"FLAT sweep" diagnosis:** the synthesis assumes flatness = weak damage signal and bets an L-effort boost rewrite on it. The critique's likelier cause: `[0,SCALE]` **term saturation** in `score_tile`. **Instrument the score-term histogram before** committing to the boost rewrite as the fix.

## 7. Recommended sequence (critique-corrected)

1. **Occlusion/reachability fix** to `focus_damage_inputs` + `ThreatField` (LOS/rampart/off-room/safe-mode). Prove on a rampart-healer + tower regression bed (`ScenarioBuilder` already builds these). *The keystone — every EV decision reads it.*
2. **EV target selection** (D>Hb unkillable-discard + spill) on top of the now-correct estimator. Smallest high-value behavior change; kills the dogpile-the-sheltered-healer exploit.
3. **Integer-n Lanchester engage/retreat gate** replacing flat-HP hysteresis (validate the heal-fold in sim first).
4. **Tournament substrate** (`tournament.rs` + bed basket + stalemate scorer) — *now* worth building, because it tunes a corrected signal; immediately unsticks the FLAT-sweep finding and becomes the exploiter ship-gate.
5. **Blob core:** define the auction cross-goal currency (§2) → N-aware fragility-weighted cohesion → the role auction. Validate under the step-4 tournament with composition-varying opponents. Resolve `MAX_CONCURRENT_SQUADS`.
6. **Adaptivity:** archetype classifier → preset selector → seeded π* draw. Last; depends on a populated, exploiter-gated tournament.
7. **Adversarial room-gen** (`scenario_gen.rs` + regret search + pairwise covering array); runs continuously as the regression frontier.

## 8. Open questions for the operator

1. **Lanchester n:** per-archetype integer ∈ {1,2}, or a single global default? (Adopt integer to protect parity regardless.)
2. **Heal-as-attrition:** is the heal-fold into β accurate enough near μ=0, or do we need an explicit eHP-regen term? Gates trust in the engage gate.
3. **`MAX_CONCURRENT_SQUADS=4`:** does the blob generalization let us raise/remove it, and what CPU ceiling does the pathfinding budget allow?
4. **Offline tuning budget:** what wall-clock / CPU is available for the O(G·P·R) tournament + exploiter + adversarial search? Fixes the compile-time bounds.
5. **Mixed-strategy seed:** is `mission⊕tick⊕room` unobservable enough, or does deterministic timing leak the draw? Threat-model call.
6. **Passive-timeout double-loss vs legitimate turtling:** the objective-aware draw thresholds (attacker progress-toward-razing bar) need an operator call so correct defense isn't punished.

## 9. Decision

**None yet — this ADR is Proposed.** It captures the research + the corrected design and the sequence. Recommendation: approve the **keystone first slice (step 1, occlusion fix) + step 2 (EV target selection)** as a self-contained, parity-safe, no-new-harness increment that directly kills a live exploit, and defer the tournament/blob/adaptivity stack to follow-on approvals once the corrected EV signal is in.
