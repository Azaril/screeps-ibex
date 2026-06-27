# ADR 0030 — Squad composition & size tuning (the lifetime/wave axis)

- **Status:** Proposed
- **Date:** 2026-06-27
- **Extends:** [0029](0029-generalized-force-composition.md) (the one-oracle force composition), [0026 §9](0026-*.md) (the doctrine registry), [0020 §12](0020-ev-adaptive-blob-combat.md) (the force-sizing oracle + the deferred part-auction), [0028](0028-lifecycle-harness.md) (the lifecycle/rally harness that proves it)
- **Supersedes:** ADR 0029 D9 (the OFFENSE-only / "defenders deploy immediately" rally short-circuit, FIX A) — replaced by the lifetime-aware **quorum** gate (§5)

## 1. The question (operator, 2026-06-27)

Four observations drive this ADR:

1. > It may be worth having separate tuning for when a squad needs to accomplish their goal in **1 LIFETIME**, or if it is a **MULTI-LIFETIME WAVE** of squads so just making **PROGRESS** is okay.
2. > Start an ADR for squad **COMPOSITION** and **SIZE TUNING** that can be used by the **DOCTRINE** and **SPAWN** layers.
3. > Look at how/if this composes into the **BLOB/BODY AUCTION**.
4. > Grouping up **DEFENDERS** still may be needed to avoid just **LOSING** squad members because of being under-sized/under-powered. I do **NOT** think **DISCONNECTING the rally stage** is right.

Point (4) is the operator **rejecting ADR 0029 FIX A** — the `is_defend` rally short-circuit that made defenders deploy one-at-a-time. The fix deadlocked-at-N-1 was real, but the cure (deploy with *whatever* has spawned) over-corrected: a lone defender deploys, gets focus-fired, dies, re-fields. The right model is a **quorum** (a bloc big enough to fight), not on/off.

## 2. Answer (the decision)

There is a single axis underneath all four points: **does this objective have to be won in one creep lifetime, or is it a sustained campaign where each wave only needs to make progress?** That axis already exists in the code — but **implicitly, always-on, and scattered** across four unrelated places (see §3). This ADR names it, gives it ONE home, and threads it from the objective producers → the doctrine/oracle sizing layer → the spawn/rally pacing layer.

The model is deliberately the **smallest thing that captures the axis**: a 2-value enum carrying the load-bearing data, not a continuous "tempo" knob. The operator named exactly two regimes and they map cleanly onto the existing `assess`/`clear_force` fork.

- **`EngagementTempo` is a shared POD policy in `screeps-combat-decision`** (JS-free, so the bot and the eval read the identical sizing — the ADR 0026/0029 parity, extended to the lifetime axis). It is **set by the objective producers** (`war.rs` / `sourcekeeperfarm.rs`) per `ObjectiveKind`, and **consumed by two interfaces**: the **doctrine/oracle sizing** (`assess`/`clear_force`/`sized_for`) and the **spawn pacing/priority/quorum** (`squad_manager.rs`).
- **`SingleLifetime` objectives size to GUARANTEE the kill-in-time** (today's `assess` behavior, now NAMED + fed a real deadline). **`MultiLifetimeWave` objectives size for SUSTAINABLE PROGRESS** + a fight-capable quorum, and they stop over-deferring winnable-across-waves targets to G4-HEAVY.
- **The rally gate becomes lifetime-aware (the point-4 fix):** `SingleLifetime` keeps the strict full-roster bloc (offense must arrive whole); `MultiLifetimeWave` deploys a **quorum** — a fight-capable minimum that is `< requested`, so the N-1 deadlock is impossible **and** a lone creep is never sent. The grouping is retained; the rally stage is **not** disconnected.
- **This composes cleanly with the deferred body/blob auction (point 3):** `EngagementTempo` is an **INPUT** to the auction's EV currency, not a competitor. The oracle is the blob special-case of the auction (ADR 0029 §8); the tempo is the demand-side weight both the oracle's importance margin and the future auction's bid valuation already want. No EV-currency change — tempo selects all-or-nothing (single-lifetime) vs marginal/divisible (multi-lifetime) bidding.

## 3. The axis already exists — latent, always-on, and scattered

The single-lifetime assumption and the multi-lifetime intuition are both already in the code; nothing names the axis, so each site hard-codes its half.

1. **A real single-lifetime DEADLINE lives in the oracle.** `force_sizing::assess` gates EVERY winnable verdict on `total <= budget.onsite_budget_ticks` (the Breach branch at `force_sizing.rs:160`, the Drain branch at `:189`) and returns *"breach too slow for one creep lifetime"* / *"needs heavy assault (G4-HEAVY)"* otherwise (`:176`/`:201`/`:206`). `onsite_budget_ticks = CREEP_LIFE_TIME − spawn − travel`. So today the oracle **always** assumes single-lifetime: a target that can't be cleared in one on-site window is judged unwinnable and deferred — there is **no** "smaller force, just make progress, reinforce next wave" path.
2. **The multi-lifetime "no deadline" is a magic `hits = 0` per call site.** `clear_force` has the kill-in-time term `kill_in_time = enemy_hits / onsite_budget_ticks + enemy_heal` (`force_sizing.rs:252`). `GarrisonDefense::plan` (`doctrine.rs:306`) passes `0` deliberately, with the comment *"defense has no kill-deadline"* (`doctrine.rs:297`). `PlayerRaid::plan` (`doctrine.rs:245`) **also** passes `0`. That is the multi-lifetime intuition hard-coded as a literal — and it is **wrong for a raid** (§4): a raid IS single-lifetime.
3. **SK hand-rolls a single-lifetime kill window inside a multi-lifetime campaign.** `SkSuppression` uses `SK_KEEPER_KILL_TICKS = 34` (`doctrine.rs:333`) to size `ranged_parts` to kill the keeper *this wave* — but the SK farm as a whole is the canonical perpetual campaign (suppress forever, re-field each lifetime). The two tempos coexist in one objective and nothing names that.
4. **The rally gate is binary on/off.** `rally.rs:17 squad_ready_to_depart` requires `present >= requested_slots` (full roster). ADR 0029 FIX A (`squad_manager.rs:926-927`) short-circuits it for `is_defend` (`ready = is_defend || squad_ready_to_depart(...)`). So *"wait for ALL"* (offense) and *"wait for NONE"* (defense) are the only two settings — exactly what the operator rejects. (`STRICT_QUORUM_RATIO = 0.75` exists but only governs the **boundary** cross-hold `should_hold_at_boundary`, not the **depart-from-home** gate.)
5. **Spawn pacing distinguishes the two tempos crudely + per-kind.** `MAX_FORMING_SQUADS = 2` (`squad_manager.rs:66`) paces offense roster formation; FIX C (`squad_manager.rs:459-464`) EXEMPTS `ObjectiveKind::Defend` from the forming count. The code already treats "defense = deploy + reinforce, don't pace as a forming bloc" as a special case — bolted onto the kind, not derived from a tempo.

**Phase 1 has landed (the interim point-4 fix).** `rally.rs:40 squad_ready_to_depart_at_quorum` (with `MIN_VIABLE_GROUP = 2`, ratio-floored at `STRICT_QUORUM_RATIO`, capped at `requested`) is the quorum kernel point (4) needs, and it is now **wired live**: `squad_manager.rs` gates `ready_to_depart = if is_defend { squad_ready_to_depart_at_quorum(..) } else { squad_ready_to_depart(..) }`, **deleting FIX A's bare `is_defend ||` short-circuit** (commit `ca9184c`, deployed). Defense now deploys at a grouped quorum — never a lone creep, never the unspawnable full roster. The remaining phases (§9) generalize this `is_defend` key onto the `EngagementTempo` policy.

## 4. The lifetime/wave taxonomy

```rust
// screeps-combat-decision/src/force_sizing.rs (next to ForceBudget — it parameterizes the budget gate)
pub enum EngagementTempo {
    /// Win within ~one creep lifetime: kill before it levels/collapses/decays, a time-boxed raid.
    /// Sizing MUST guarantee kill-in-time; a target that can't be cleared in `deadline_ticks` is
    /// unwinnable for one squad → defer to G4-HEAVY (no progress credit). The current `assess` behavior.
    SingleLifetime { deadline_ticks: u32 },
    /// A sustained campaign: successive squads/waves chip away; MAKING PROGRESS is acceptable. Sizing is
    /// for sustainability + hold, NOT a one-shot clear. No kill-in-time gate.
    MultiLifetimeWave,
}
```

The two variants are **perfectly correlated** with two orthogonal-sounding properties — a deadline being present, and partial progress being acceptable — in every objective we field. `SingleLifetime` IS "deadline present + progress not acceptable"; `MultiLifetimeWave` IS "no deadline + progress acceptable." So ONE enum, not two fields, is the honest model. `deadline_ticks` defaults to `onsite_budget_ticks` and is **tightened** by an objective-supplied clock (collapse timer, level-up clock, decay TTL) — the deadline source that today is read in `war.rs` but never plumbed into the oracle (§3.1, the duplicate ad-hoc core-decay skip).

### Per-objective classification

| `ObjectiveKind` | `DoctrineObjective` | Tempo | Deadline source | Sizing effect |
|---|---|---|---|---|
| `Farm{Core}` (the core arm) | `KillImmuneStructure` | `SingleLifetime` | `min(onsite, ticks_to_level_up, collapse_ticks)` | win-in-time ranged kill DPS; defer if `kill_ticks > deadline`. A core LEVELS UP + self-collapses — kill THIS wave or it's a worse target next, no partial credit. |
| `Dismantle` (small breach) | `DismantleStructure` | `SingleLifetime` | `onsite` | win-in-time breach + kill DPS (today's `assess`). A short breach finished in one window. |
| `Dismantle` (long siege, regrowing walls) | `DismantleStructure` | `MultiLifetimeWave` | — | drop the `breach_ticks <= onsite` gate; size to **out-pace repair** (`net_dismantle > 0`, already `force_sizing.rs:145`) + out-heal. ROI = net rampart hits/wave. Walls regrow; successive quads net-progress the ring down. |
| `Secure` (operator attack flag) | `ClearCreeps` (offense) | `SingleLifetime` | `onsite` | `clear_force` with `hits = enemy_hits` (**NOT 0** — fixes the `doctrine.rs:245` magic literal). A raid is time-boxed: out-power + kill THIS deployment or withdraw; you don't chip enemy creeps across waves (they heal/leave). |
| `Defend` (owned room) | `ClearCreeps` (defense) | `MultiLifetimeWave` | — | `clear_force` with `hits = 0` (the existing inert term, now **derived** from tempo not a magic literal). Hold indefinitely; defenders die + re-field; progress = stay alive + attrit. Quorum rally (§5). |
| `Farm{SourceKeeper}` | `Suppress` | `MultiLifetimeWave` (with a per-wave `SingleLifetime` kill sub-goal) | per-wave `SK_KEEPER_KILL_TICKS` | keep the `SK_KEEPER_KILL_TICKS` kill window for the kiter's ranged parts; the tempo tells rally/spawn "re-field continuously, quorum = duo, don't G4-HEAVY-defer a wave that can't fully suppress." The farm is perpetual; each wave's keeper kill is the single-lifetime sub-goal. |
| `Farm{PowerBank}` | (Farm) | `SingleLifetime` | `ticks_to_decay` | size attacker + heal so `bank_hits / dps <= ticks_to_decay`; defer if not. The bank DECAYS — classic deadline (`war.rs` already reads `ticks_to_decay`). |
| `Harass` | `Harass` | `MultiLifetimeWave` | — | fixed solo (today), no oracle gate, no deadline. Deny/annoy forever, throwaway solos replaced each lifetime. |
| `Escort` | (Escort) | `SingleLifetime` | `onsite` | win-in-time pre-clear while the claimer commits — bounded. |

**Reclassification wins:** (i) the magic `hits = 0` at `doctrine.rs:245` (`PlayerRaid`) is **wrong** under this taxonomy — a raid is `SingleLifetime`, so it passes real `enemy_hits` and binds the kill-in-time term; only `GarrisonDefense` (`MultiLifetimeWave`) keeps `hits = 0`, now derived. (ii) SK and the long siege **stop over-deferring** to G4-HEAVY because `MultiLifetimeWave` removes the `<= onsite` gate.

## 5. The shared tuning struct

`EngagementTempo` is the policy; one struct, one home, two consumption interfaces.

- **Where it lives:** `screeps-combat-decision/src/force_sizing.rs` (next to `ForceBudget`/`RequiredForce` — it parameterizes the budget gate). JS-free, `Copy`, `Serialize`/`Deserialize`, `Default = SingleLifetime { deadline_ticks: <onsite default> }` (so old saves + un-migrated call sites decode to today's behavior — the change is **behavior-inert** until a producer opts into `MultiLifetimeWave`).
- **Who SETS it:** the objective **producers** (`war.rs`, `sourcekeeperfarm.rs`), per `ObjectiveKind`, when they build the `EngagementContext` AND when they build the `ObjectiveRequest`. The producer is authoritative because it owns the `source → ObjectiveKind` mapping + the deadline source (collapse timer / decay TTL) that the pure crate can't model.
- **Where it RIDES:** a `tempo: EngagementTempo` field on **(a)** `EngagementContext` (`doctrine.rs:75`, the doctrine reads it) and **(b)** `CombatObjective` / `ForceRequirement` (`objective_queue.rs:147/129`, the live manager reads it each tick — it has no doctrine handle at gate time). The objective field is serialized; add it with `#[serde(default)]` so old saves decode to `SingleLifetime` — **no `WORLD_FORMAT_VERSION` bump needed** if defaulted (verify against the `game_loop.rs` discipline; tempo is additive + defaulted). Mirror the `is_defend` threading: `squad_manager.rs:419-440` already derives per-objective facts from `obj`; add `let tempo = obj.tempo;` and pass it into `compute_squad_orders`.

### Consumption interface A — doctrine / oracle sizing

Thread `tempo` into the oracle's two entry points:

- **`assess(profile, budget, tempo)`** — replace the `<= budget.onsite_budget_ticks` checks (`force_sizing.rs:160`, `:189`) with `<= deadline_ticks.min(onsite_budget_ticks)` for `SingleLifetime`; for `MultiLifetimeWave`, **skip the deadline gate entirely** (a target unwinnable-in-one-lifetime is still winnable across waves). The binding constraints for a wave become only: out-pace repair (`net_dismantle > 0`) + out-heal incoming × `HOLD_MARGIN`. `est_ticks` becomes informational ("ticks to win across waves"), not a gate. This is the **single point** that turns "always single-lifetime" (today) into the two regimes.
- **`clear_force(...)` taking `tempo`** — `SingleLifetime` uses `kill_in_time = enemy_hits / deadline_ticks + enemy_heal` (deadline binds the kill DPS); `MultiLifetimeWave` passes `enemy_hits = 0` so the kill term is inert (the existing defense behavior, now derived). The per-doctrine `hits` literal is **removed**: `SingleLifetime → real enemy_hits`, `MultiLifetimeWave → 0`.
- **No new doctrine variants.** Tempo is orthogonal to the classifier, so the same doctrine (e.g. `SiegeBreach`) serves both a quick breach and a long siege depending on the objective's tempo.
- **No change to `composition.rs::sized_for` itself** — it already grows member count to meet a `RequiredForce`. Tempo changes WHAT force is requested: `SingleLifetime` requests the win-in-time force (may be large → may defer); `MultiLifetimeWave` requests the sustainable per-wave force (out-heal + net-progress, typically the floor template), which by construction fits one squad — so a wave essentially never defers, it just re-fields.

### Consumption interface B — spawn pacing / priority / quorum (`squad_manager.rs`)

- **Rally quorum** (the §6 point-4 fix) — gate on `tempo`.
- **Spawn pacing** — replace the `is_defend`-exempt FIX C (`squad_manager.rs:459-464`) with `matches!(tempo, EngagementTempo::MultiLifetimeWave)`-exempt. Waves don't count against `MAX_FORMING_SQUADS` (they deploy on quorum + reinforce continuously, so they aren't forming blocs competing for the offense form budget). Same behavior for defense today, but generalized + keyed on the tempo, not the kind.
- **Spawn priority** — `spawn_priority_for` may take `tempo` so a `MultiLifetimeWave` maps one notch lower (a steady drip that never bursts/starves economy) while `SingleLifetime` keeps HIGH (complete the roster fast). v1 may leave `spawn_priority_for` unchanged (it is already priority-keyed) and revisit if a wave is observed bursting.

## 6. The winnability-validated deploy gate (fixes BOTH the FIX-A over-correction AND the quorum's Lanchester gap)

The most important deliverable, and it took two iterations. The first (a fixed-ratio quorum) was WRONG — the operator caught it: *"Does the quorum still validate Lanchester probability of winning? How do we know quorum is sufficient to be useful?"* It does not.

### 6.1 Why the fixed-ratio quorum failed (the operator is mathematically exact)

`squad_ready_to_depart_at_quorum` deployed at `STRICT_QUORUM_RATIO = 0.75` of the requested roster — a COUNT fraction, not a win check. The oracle sizes the roster Lanchester-favorable on two linear axes: **survival** (`required_heal = incoming × HOLD_MARGIN`, `HOLD_MARGIN = 1.3`, force_sizing.rs:245) and **kill** (`required_dps = enemy_dps × COORDINATED_DPS_MARGIN`, `1.5`, :253). A count-subset scales BOTH our heal and our DPS by ≈`f`, so the break-even fraction is `1/min(margin) = 1/1.3 ≈ 0.77` (the survival axis binds, being the tighter margin). **`0.75 < 0.77`** → the quorum deploys a force that CANNOT out-heal the incoming (`win_probability ≈ 0.47`, a coin-flip loss) — and a count is composition-blind (it can drop the healer entirely). A winnable SUBSET exists only in the margin-bound regime (down to `f ≈ 0.77`, measured on real heal/DPS, NOT count), and NOT when `kill_in_time` binds (a minimum-sized grind → winnable === full roster). A fixed ratio distinguishes neither — it is the wrong instrument.

### 6.2 The gate: validate the PRESENT force, reuse the oracle's model

Deploy iff the present (spawned, positioned) force is itself Lanchester-favorable vs the threat, validated with the SAME `win_probability` + `clear_force` predicates the oracle sizes with — **one combat-math home, no second Lanchester model** (extract the three predicates into shared `pub(crate)` helpers so the gate and the oracle can never drift):

```rust
// screeps-combat-decision/src/rally.rs
pub const MIN_VIABLE_GROUP: usize = 2;
pub const WAVE_DPS_MARGIN: f32 = 1.1;
pub const WAVE_MIN_WIN_PROB: f32 = 0.60;

pub struct PresentForce { pub kill_dps: f32, pub heal_per_tick: f32, pub count: usize }

pub fn present_force_is_winnable(present: &PresentForce, threat: &EnemyForce, tower_dps: f32,
    onsite_budget_ticks: u32, tempo: EngagementTempo, safe_mode: bool) -> bool {
    if safe_mode { return false; }                                    // mirrors clear_force (zero dmg possible)
    if present.count < MIN_VIABLE_GROUP { return false; }             // cheap pre-filter: never a solo
    let t = tempo.deploy_threshold();
    let incoming = tower_dps + threat.dps;
    if win_probability(present.heal_per_tick, incoming) < t.min_win_prob { return false; } // SURVIVE
    if present.kill_dps <= threat.heal { return false; }              // their heal cancels our kill
    if present.kill_dps < threat.dps * t.dps_margin { return false; } // square-law DECISIVE
    if threat.hits > 0 {                                              // KILL-IN-TIME (SingleLifetime grind)
        let net = present.kill_dps - threat.heal;
        if net <= 0.0 || (threat.hits as f32 / net).ceil() as u32 > onsite_budget_ticks { return false; }
    }
    true
}
```

Tempo thresholds (named consts, harness-tuned seeds): **SingleLifetime** → decisive (`dps_margin = COORDINATED_DPS_MARGIN = 1.5`, `min_win_prob = 0.80` ≈ the full sized force, matching `squad_ready_to_depart`); **MultiLifetimeWave** → favorable-not-decisive (`WAVE_DPS_MARGIN = 1.1`, `WAVE_MIN_WIN_PROB = 0.60`, clearly above the `0.77` heal break-even, so a wave subset still out-heals before committing). The gate is **monotone** — as members spawn, `present.heal/kill_dps` rise, so it opens exactly when the spawned force can win: earlier than the full roster when margin allows, never before it can win.

### 6.3 Inputs (all at the gate except the threat)

`present` (Σ`kill_dps` = melee+ranged power; Σ`heal_per_tick` = heal_parts × `HEAL_POWER`(12); `count` = positioned members) from `member_views` (squad_manager.rs:795-817). `tower_dps` = 0 for owned-room defense, `DefenseProfile` for offense. `onsite_budget_ticks` from the objective/home. `tempo` + `threat` must **ride on the objective** — at home-rally time the target room is NOT visible, so the gate cannot recompute the threat from DTOs (they're empty → it would wrongly pass). Add a `ThreatSnapshot { dps, heal, hits, count, boosted, safe_mode }` (`#[serde(default)] → None → legacy path → no WFV bump`), set by the producer where it already builds `EnemyForce` (war.rs:356), refreshed through `request()`. When the room IS visible, take `max(snapshot, live)` (conservative).

### 6.4 The deadlock is spawn-completion, not a rally concern

If no positioned subset is winnable, the gate HOLDS at the full roster — correct: there is no winnable subset to deploy. The resulting "last member never spawns" deadlock is a **spawn-completion problem**, fixed at the source: ADR 0029 FIX B (the small duo floor makes `quorum == requested` for most defense — no N-1 gap at all), FIX C (forming exemption), renew, CRITICAL priority. For a genuinely-minimum-sized grind that still can't complete, that is `UnwinnableTarget` back-off / re-size territory — the gate holding is the SIGNAL that drives it, never license to field a loser.

### 6.5 Status + the FIX-A revision plan

- **INTERIM LANDED (commit `9705b6a`, deployed):** the unsound ratio-quorum is reverted to the FULL ROSTER (`squad_ready_to_depart` for defense too) — winnable by construction, never ships a loser; FIX B keeps most defense rosters small enough to complete. The now-unused `is_defend` param + the quorum re-export were removed; the `squad_ready_to_depart_at_quorum` kernel stays in `rally.rs` for the winnability gate to reuse, then is deleted.
- **The winnability gate then RESTORES principled subset-deploy:** `squad_ready_to_engage(positions, present, threat, tower_dps, onsite, requested, tempo, safe_mode)` = a cheap count pre-filter (SingleLifetime full roster / MultiLifetimeWave `MIN_VIABLE_GROUP`) AND `present_force_is_winnable`. Replaces the squad_manager.rs:927 gate; build `PresentForce` from `member_views`; derive `tempo`/`threat` at :419-440; generalize FIX C's exemption to `matches!(tempo, MultiLifetimeWave)`. Delete the dead quorum kernel. `STRICT_QUORUM_RATIO` is retained ONLY for `should_hold_at_boundary` cohesion (also made tempo-aware: a deployed `MultiLifetimeWave` wave does not re-hold at the boundary for a still-spawning reinforcement).
- **Harness proof (the operator's "how do we know it's sufficient"):** drive `present_force_is_winnable` over the SAME `(threat, budget)` the oracle sized for (assert it opens at the sized force / favorable-subset boundary, HOLDS for the 0.75 heal-short subset — the operator's case reproduced-then-fixed); then run the deployed force to completion in the engine-backed sim and assert it **CLEARS without a wipe** — measuring the outcome (win/wipe), not "enough bodies departed."

## 7. Composition with the body/blob auction

The auction (task #28 / ADR 0029 §8 / ADR 0020 R7-R8 — *"value a part MIX in a common EV currency"*) is **deferred**; the oracle covers blob sizing today, and the oracle is the auction's **blob special-case** (ADR 0029 §8: the auction's single-goal argmax with a fixed role partition; the importance margin is the EV weight). `EngagementTempo` composes with it **as an input, not a competitor** — and it does so **without changing the EV currency**:

- **The auction reads `tempo` only as a BID-VALUATION mode.** A `SingleLifetime` objective's parts are worth their full win-in-time value **only if the whole force is affordable this wave** — an under-bid that can't guarantee the kill is worthless → **all-or-nothing** bidding (bid the decisive force or bid 0). A `MultiLifetimeWave` objective values parts **marginally** — each extra part buys incremental progress/hold, so a partial fill still has positive value → **divisible/smooth** bidding (compete for spare parts each reinforcement tick).
- **The currency stays `parts → EV`.** Tempo is a property of the *demand* (all-or-nothing vs divisible), so it slots into the auction without a new currency term. SingleLifetime sets a hard spend ceiling (over-match now, one buy); MultiLifetimeWave sets a per-tick drip budget (re-bid each reinforcement at lower margin).
- **The oracle = the blob special-case.** When the auction lands, the SAME `tempo` that the oracle's deadline gate + importance margin read also weights the auction's cross-goal EV terms (focus = burst, up-weighted by `SingleLifetime`; drain/breach = durable unlock the next wave inherits, up-weighted by `MultiLifetimeWave`). This is the ADR 0020 §2 principle — *one decision principle, several argmaxes in their own units, explicit conversions where they meet*.

**Auction-readiness requirement on this ADR:** define the `EngagementTempo` field on the objective + `EngagementContext` so the auction has the hook to read. **No auction code in v1** — the auction is deferred (ADR 0029 D8 / D8 below); tempo is the seam it will read.

## 8. Decisions

- **D1.** Name the axis: a 2-value `EngagementTempo { SingleLifetime { deadline_ticks }, MultiLifetimeWave }` in `screeps-combat-decision/src/force_sizing.rs`. Smallest model that captures the operator's two regimes; not a continuous tempo. `Default = SingleLifetime` (today's behavior).
- **D2.** `deadline_ticks` defaults to `onsite_budget_ticks` and is tightened by an objective-supplied clock (core collapse / level-up, power-bank decay). Fold the ad-hoc `war.rs` core-decay skip INTO the oracle deadline — delete the duplicate hand-calc.
- **D3.** Per-`ObjectiveKind` classification per §4: core/small-breach/raid/secure/power-bank/escort = `SingleLifetime`; defend/SK/harass/long-siege = `MultiLifetimeWave`.
- **D4.** The tempo is set by the **producers** and rides on BOTH `EngagementContext` (doctrine reads) and `CombatObjective` (live manager reads). Serialized field is `#[serde(default)]` → no `WORLD_FORMAT_VERSION` bump (verify against `game_loop.rs`).
- **D5.** The oracle (`assess`/`clear_force`) consumes `tempo`: `SingleLifetime` keeps the deadline gate (`<= deadline.min(onsite)`); `MultiLifetimeWave` drops it (sustain + net-progress only). Remove the per-doctrine magic `hits` literal — derive it from the tempo (`SingleLifetime → real hits`, `MultiLifetimeWave → 0`). Fixes the `PlayerRaid` `hits=0` mis-classification.
- **D6.** No new doctrine variants — tempo is orthogonal to the classifier. No change to `sized_for`; tempo changes the *requested* force, not the distribution.
- **D7.** **Supersede ADR 0029 D9 (FIX A).** The deploy gate is WINNABILITY-VALIDATED (§6), not a count quorum — the operator caught that a 0.75 count ratio does not validate Lanchester winning (it is below the `1/HOLD_MARGIN ≈ 0.77` survival break-even and is composition-blind). `squad_ready_to_engage` = a cheap count pre-filter AND `present_force_is_winnable` (reuses `win_probability`/`clear_force`, no second model): `SingleLifetime` deploys the decisive sized force; `MultiLifetimeWave` deploys the smallest FAVORABLE present force (`WAVE_DPS_MARGIN`/`WAVE_MIN_WIN_PROB`) and reinforces. It never deploys a force that loses; the residual no-winnable-subset HOLD is a spawn-completion concern (§6.4), not license to field under-strength. **Interim landed** (`9705b6a`): reverted to the full roster (winnable by construction) until the gate lands. `STRICT_QUORUM_RATIO`/`squad_ready_to_depart_at_quorum` are retired from the depart gate (the ratio kept only for `should_hold_at_boundary`).
- **D8.** Generalize FIX C: the `MAX_FORMING_SQUADS` exemption is keyed on `MultiLifetimeWave`, not `ObjectiveKind::Defend`.
- **D9.** `should_hold_at_boundary` is tempo-aware: `MultiLifetimeWave` skips the boundary re-hold (committed waves reinforce across the boundary on arrival); `SingleLifetime` keeps the 0.75 cohesion hold.
- **D10.** The body/blob auction (deferred) reads `tempo` as a **bid-valuation mode** — `SingleLifetime` = all-or-nothing, `MultiLifetimeWave` = marginal/divisible — without a new EV currency. The oracle remains the auction's blob special-case. No auction code in v1; the tempo field is the seam.
- **D11.** Migration is behavior-inert until a producer opts into `MultiLifetimeWave`: every `SingleLifetime` default reproduces today's margins. The ONE deliberate behavior change is D7 (FIX A → winnability-validated deploy), gated by the defense/SK producers choosing `MultiLifetimeWave`.

### Cleanup & consolidation decisions (from the §10 audit)

- **D12.** One sizing pattern: every doctrine is HONESTLY-FIXED or HONESTLY-DYNAMIC — no murky middle. Delete the silent-static fallback: replace `.unwrap_or(template)` at doctrine.rs (PlayerRaid/GarrisonDefense/SkSuppression) with `composition: Option` propagation (the `GatedPlayerRaid` model). Operator-intent "always field" uses an explicit, LOGGED `field_or_floor` helper, never an inline silent unwrap.
- **D13.** Fix the `member_energy: 0` root cause at the **remote-defense** arm (war.rs ~552) — pass the room's `energy_capacity_available()` like the owned arm (the ADR 0029 FIX-B miss); fill `member_energy` for the PlayerRaid arm. Then delete the redundant `.unwrap_or_else(solo_ranged/duo_sk_farmer)` call-site fallbacks; the producer DEFERS (don't upsert this tick) on `None`.
- **D14.** Replace `is_sized() -> bool` with `enum Sizing { Fixed, Dynamic }`. Only `HarassRemote` is `Fixed`; PlayerRaid/GarrisonDefense/SkSuppression become `Dynamic` and flow through the ONE Option-honoring war.rs gate (eliminating the reason their fallbacks existed). The mislabel was the ROOT of the murky middle.
- **D15.** Generalize FIX-B's floor-decouple: the count floor (composition.rs:770) becomes an explicit `min_count` (default 1), force-driven above it. Delete the per-doctrine GarrisonDefense duo-floor hack.
- **D16.** Delete the dead/duplicate code: the `bodies.rs` pre-ADR-0029 parallel defender-sizing path (`assemble_combat_body`/`sized_defender_body`/`attack_parts_to_kill`/`drain_*` + consts), `as_solo_spec`, the unreferenced comps (`duo_tank_heal`/`duo_drain`/`duo_melee_heal`/`solo_core_attacker`/`quad_siege`-dup; verify power-bank consumers first), `required_boosts` + `mod boosts` (or wire it). The serde-discriminant-shifting `BodyType` variant deletions ride a `WORLD_FORMAT_VERSION` bump.
- **D17.** Route the last unsized bypass (war.rs defend-flag hardcoded duo) through GarrisonDefense with a real `EngagementContext`; unscouted → an honest no-fight template, not a silent fallback.
- **D18.** Replace the hand-rolled `SK_DUO_BODY_COST`/`SK_DUO_MAX_BODY_COST` (operations/sourcekeeper.rs) with the real `SkSuppression`-sized duo's `estimated_cost` so the SK ROI gate and the fielded force agree.
- **D19 (test lock-in).** Add a `no_dynamic_doctrine_silently_fields_static` invariant + a `{doctrine × tempo × affordability}` matrix (`affordable ⇒ Sized slot present & caps ≥ required`; `must-defer ⇒ None`). Make `SizingWins` count an oracle-winnable-but-`sized_for`-None scenario as a FAILURE (today it silently EXCLUDES it — validate.rs:646 — which is why the bug hid); un-ignore `CreepClearWins`; make validators doctrine-parametric; add a `sized_force_strictly_beats_static_template` tournament match.

## 9. Remaining work / phased implementation

Each step is independently testable in the decision crate (pure kernels) before any live wiring — the offline-harness-first discipline the war-lifecycle work uses.

1. **Interim quorum wiring (highest leverage — the point-4 fix, no new type).** Wire the already-landed `squad_ready_to_depart_at_quorum` into the live gate: add `squad_ready_to_engage` keyed initially on `is_defend` (defense → quorum, offense → full roster), delete FIX A's bare short-circuit, generalize FIX C. This fixes the operator's rejection **before** the policy struct lands, using the kernel that already exists. Tests: quorum deploys at 3/4 (regression for the N-1/4 stall); full-bloc still needs 4/4; `MIN_VIABLE_GROUP` floor holds a lone defender.
2. **The policy struct.** Add `EngagementTempo` to `force_sizing.rs` (+ `Default`). Thread it through `assess`/`clear_force` (the two regimes) + tests (a wave drops the kill-in-time gate but keeps the sustain gate; single-lifetime is byte-identical to today).
3. **Doctrine wiring.** Add `tempo` to `EngagementContext`; remove the magic `hits` literals in `doctrine.rs` (derive from tempo). Re-key `squad_ready_to_engage` from `is_defend` to `tempo` (subsumes step 1's interim).
4. **Objective + producer wiring.** Add `tempo` to `CombatObjective`/`ObjectiveRequest` (`#[serde(default)]`). Set it in `war.rs`/`sourcekeeperfarm.rs` per §4; fold the `war.rs` core-decay skip into the oracle deadline. Thread `should_hold_at_boundary` tempo (D9).
5. **Harness proof.** Extend the lifecycle/forming harness (ADR 0028) so a `MultiLifetimeWave` quorum deploys at quorum, holds, and reinforces across waves vs a defended target — proving D7/D9 reproduce-then-fix the N-1 stall + the lone-defender pick-off offline (the operator's tune-offline-not-live preference).
6. **(Deferred) auction bid-valuation reads `tempo`** (D10) — only when a measured objective demonstrably loses value to blob-only sizing (ADR 0029 D8). The tempo field is already the hook; no work until then.

## 10. Cleanup & consolidation — one sizing pattern, no murky middle

The operator (2026-06-27): *"the OLD squad sizing and composition code is CONFUSING us."* This section names the confusion and removes it. It is **orthogonal** to the lifetime/wave axis (§1-9): tempo decides *what force is requested*; this decides *how a doctrine fields-or-defers and what code expresses that*.

**10.1 The thesis.** A doctrine is EITHER **honestly-fixed** (no fight to size) OR **honestly-dynamic** (oracle-sized, `plan()` returns `composition: Option`, `None` → DEFER + log, never a silent template). The "murky middle" — a *sized* doctrine that silently degrades to its un-grown static template via `.unwrap_or(template)` — is the source of the W9N8 under-size (`member_energy: 0 → sized_for None → bare duo`) and the SK trickle (weak home → None → bare duo, in-and-bails). It is deleted everywhere.

**10.2 The three confusions, named.**
1. **Silent-static fallback** — six `.unwrap_or(template)` sites (doctrine.rs PlayerRaid/GarrisonDefense/SkSuppression; war.rs owned+remote defense; sourcekeeperfarm.rs) turn an honest `sized_for` defer into a dishonest "field a weak template." `GatedPlayerRaid` already propagates `None` correctly — the others must match it (D12).
2. **The `is_sized()` lie** — the flag claims "runs the oracle," but 3 of 4 false-returners DO size in a custom `plan()`; they set `is_sized()==false` only to dodge the budget `.expect()`. That mis-routes them down war.rs's "fixed, no gate" arm — which is *why* they needed fallbacks. Replace with `enum Sizing { Fixed, Dynamic }` (D14). **This mislabel is the ROOT.**
3. **Two parallel sizing systems + dead templates** — the pre-ADR-0029 threat-matched `bodies.rs` defender path is dead (superseded by `force_sizing` + `build_combat_body`); 7 predefined comps + their `BodyType` variants + all 4 `Boosted*` (boosts never wired) + `as_solo_spec` are dead; a hand-rolled SK cost constant duplicates the real sized-duo cost (D16/D18).

**10.3 The pattern.** Templates are ROLE-SEEDS (role mix + non-sized bodies + formation); the member COUNT is always force-driven (`≥1`, not `≥template_count`), with an explicit per-doctrine `min_count` (D15). ONE pipeline (`template → sized_for → BodyType::Sized → build_combat_body → queue_slot_spawn`, no double-sizing — already true). ONE gate shape at every call site: `Some → field`, `None → defer + log`.

**10.4 Testability (D19).** A `no_dynamic_doctrine_silently_fields_static` invariant + a `{doctrine × tempo × affordability}` matrix make a silent-static regression IMPOSSIBLE offline; the previously-untested `None` path is driven on purpose — exactly the path that hid the bug. `SizingWins` stops silently excluding the winnable-but-None case; `CreepClearWins` is un-ignored; validators go doctrine-parametric; a `sized_force_strictly_beats_static_template` tournament match puts a sizing regression into a ship-gate (the deletions are tournament-safe — the tournament tunes kite tactics over fixed bodies and never sizes).

**10.5 What stays FIXED, and why.** `HarassRemote` (throwaway deny-and-leave; no fight to size). `PlayerRaid`'s no-scout `dps<=0` branch (honest no-op until flag intel is wired). The eval `managed_assault_comp` / `siege_ceiling`-as-display scaffolds (grading lenses, not fielded forces). The count floor stays as the minimum sane shape — but as an explicit `min_count` input, not an implicit `.max(template_count)`.

### Phased cleanup plan (low-risk first)

- **Phase 0 — pure deletions** (no behavior change, no WFV): `as_solo_spec`, the `bodies.rs` legacy sizing path, the unreferenced comps (verify power-bank), `required_boosts`/`mod boosts`.
- **Phase 1 — SK cost duplicate** (D18, isolated to the SK ROI gate).
- **Phase 2 — silent-fallback → principled-defer** (D12/D13, the core fix; fix the remote `member_energy:0` root cause first, then drop the fallbacks; handle the new defers at the call sites).
- **Phase 3 — `Sizing { Fixed, Dynamic }`** (D14) — routes the 3 mislabeled doctrines through the Option-honoring gate.
- **Phase 4 — generalize the count floor** (D15).
- **Phase 5 — route the defend-flag bypass** (D17).
- **Phase 6 — `BodyType` variant deletions** (D16) — folded into the next `WORLD_FORMAT_VERSION` bump.
- Each phase is host-testable in the decision crate before any live wiring (offline-first).
