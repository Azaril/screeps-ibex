# ADR 0042 — Unified energy pricing for combat squad forming (the R_net model)

- **Status:** Accepted + **P0 IMPLEMENTED & VALIDATED ON PRIVATE (2026-07-08)** — bid + economic give-up live; deferred refinements R1–R4 + the `opportunity_floor>0` (gated on ADR 0043 A2) tracked below. Supersedes the interim `forming_completion_bid(present, ticks)` escalation (ADR 0040 spawn lane, committed but band-scaled). **MMO deploy pending operator final review.**
- **Date:** 2026-07-07
- **Deciders:** William Archbell
- **Related:** ADR 0040 (the e/t currency, `value_e`, §D5.4 `R_O`, the spawn-request policy), ADR 0038 (`room_net_roi` / reservable-remote economic pricing), ADR 0031 (capability force composition + `pairing_p_win`), ADR 0037 (count-quorum winnability veto), ADR 0028/0034 (the lifecycle/forming harness); memory [[combat-ev-economic-and-pwin-gating]], [[prefer-per-tick-optimal-over-hysteresis]], [[sim-determinism-fence]].
- **One line:** Price a forming combat squad's spawn bid — and its give-up decision — on the **objective's real completed value in true energy** (`R_O_completed = p_win_completed · value_e / est_ticks`), not on a band ordinal or on time-stalled, so a squad forms exactly as hard as its objective is worth and abandons the moment finishing it would destroy more value than the economy alternative. This is the "full normalization" the operator has asked for, scoped to the forming lane.

## Context

**The live failure.** Combat squads reliably fail to reach rooms because their **rosters never complete under normal spawn contention** (the dominant item in the reliability catalog): a combat slot tied with economy loses every tie, stalls at N-1/N for thousands of ticks, renew-camps its present members, and eventually gives up at `MAX_FORMING_BUDGET` (3000) — then re-fields and repeats. RENEW≈70/tick with zero RETIRE/recycle was the live fingerprint.

**Why the interim fix is not enough.** ADR 0040's spawn lane, and the interim `forming_completion_bid(present_members, ticks_forming)` escalation built on it, live in a **band-scaled** currency: `SPAWN_BID_* = old-f32-priority-band × 1000` (CRITICAL 100_000 … LOW 25_000). These are **not** energy-per-tick — 75_000 is a priority ordinal wearing milli-e/t clothing. The transfer market (`body_roi_milli`, `refill_bid`, par = `STORAGE_BID` = 1000) **is** true e/t. The two are incompatible in scale, unified only in datatype (u32 milli). The interim escalation therefore prices **time stalled**, not the objective's **worth** — a mediocre remote and a base-under-assault escalate identically, and the escalation constant (`FORMING_WASTE_STEP_MILLI = 25`) is a tuned band nudge, not a real quantity.

**What we actually want.** One currency in which a forming squad is an energy investment: once complete it throws off an objective **rate** `R_O` (the e/t of economy it unlocks / protects / denies); while incomplete it **burns** a rate `B` (the renew-or-idle lifetime bleed of its present members). Its spawn bid and its give-up both fall out of that one comparison — no `MAX_FORMING_BUDGET` clock as the primary decision, no band nudge.

This ADR is the output of a 10-agent pricing deep-dive (map → design → synthesize → adversarial verify). The naive synthesis was found **unsound** on seven counts (below); the corrected model here is what ships.

## Decision — the R_net model (corrected P0)

Everything is priced in **true milli-e/t at par = `STORAGE_BID` = `BID_SCALE` = 1000**.

### 1. `p_win_completed` (new — load-bearing, NOT deferred)
The objective rate must be valued over the squad we are *forming*, not the members present so far. Project the **full requested roster's** capabilities from `obj.force.squads[0].slots` (expand each slot body via `screeps_combat_decision::spawning::create_body` → `SquadCapabilities`) and compute

```
p_win_completed = pairing_p_win(completed_caps, defense, enemy, MAX_TRAVEL_BUDGET, PairingParams::default())
```

This is a **second** caps vector, distinct from `caps_from_members` (the present/binding-member caps that stay in the movement lane). Using present caps for the bid was the fatal bug in the naive model (see Flaw 1).

### 2. The objective rate (reused verbatim)
```
R_O_completed_milli = round(p_win_completed · value_e · 1000 / est_ticks.max(1))   as i64
```
`value_e` (per objective kind) and this exact `float → round → i64` transform already exist (`squad_manager.rs:921`, the `military_priority_bid` numerator). Determinism idiom preserved.

### 3. The forming bid — reserved sub-CRITICAL band, ordered by real value
```
window = SPAWN_BID_CRITICAL - SPAWN_BID_HIGH                 // 25_000
edge   = if is_defense_critical { DEFENSE_SPAWN_EDGE } else { 0 }   // REC-052(c)
escal  = R_O_completed_milli.min(window - 1 - edge)
bid    = (SPAWN_BID_HIGH + escal + edge).min(SPAWN_BID_CRITICAL - 1)   // ∈ [75_000, 99_999]
```
Properties, by construction:
- **Bootstraps** — a fresh (0-present) squad still bids ≥ `SPAWN_BID_HIGH`, so member 1 always wins its lane against the economy bulk. (The naive model's present-caps `R_O ≈ 0` abandoned squads before member 1 spawned.)
- **Orders by objective worth** — within [75k, 99k) the bid ranks by `R_O_completed`, so a base-under-assault outbids a mediocre remote. (A raw `p_win·value_e·1000/est_ticks` saturates the 999_999 / CRITICAL-1 cap for any `value_e ≳ 100`, collapsing all objectives to a tie — hence order **within** a reserved band, do **not** attempt true cross-lane arbitration in P0.)
- **Income-never-preempted** — capped strictly below `SPAWN_BID_CRITICAL` (miners).
- **No consumer flips** — never drops below `SPAWN_BID_HIGH`, so every `>= SPAWN_BID_HIGH` gate and `spawn_bid_label` keeps its meaning (a hard pre-deploy grep-audit still required).
- **B_present is NOT in the bid** — the queue orders by value *delivered*, not waste *accrued* (adding it rewards past over-commitment; Flaw 7). `B` lives only in the give-up.

### 4. The burn (give-up only)
```
B_present_milli = round(roster_present_cost_e · 2 / 3)   // = roster_cost/1500 e/t → milli
```
Identity (verified): steady-state renew-to-hold spend `renew_energy_cost·len/600 ≈ cost/1500 e/t` **equals** the idle-lifetime amortization `body_cost/1500 e/t` — renew and idle-decay are the *same* flow, charged **once** (summing them double-counts). Sunk one-time body cost is a **stock**, realized only on abandon; it enters the give-up **only at salvage** (recoverable remaining deployable life), never as a flow — the sunk-cost-fallacy guard.

### 5. The give-up decision — economic, hysteretic (Phase 2)
```
give_up  iff  (R_O_completed_milli − B_present_milli) < opportunity_floor_norm_milli
              held TRUE for K = 20 consecutive ticks
```
where
```
opportunity_floor_norm_milli =
    ema8( max ready civilian body_roi_milli in the home room ) · est_ticks / 1500
```
- **Horizon-normalized** (`· est_ticks / 1500`) — `R_O` is an unlock-rate over ~`est_ticks`; civilian `body_roi` is amortized over `CREEP_LIFE_TIME = 1500`. Comparing them raw is the 10× mismatch (Flaw 4).
- **EMA(8) + K=20-tick latch** — the raw per-room civilian max spikes when a civilian request is energy-blocked (refill ~10–12k, stressed hauler 25k+), oscillating give-up on/off; the EMA + consecutive-tick latch is the sanctioned exception to [[prefer-per-tick-optimal-over-hysteresis]] (observed oscillation, not speculative anti-flap). `instant_spawnability_premium` is the fallback floor when the civilian queue is transiently empty. `mark_unwinnable` kept for boundary-flicker.
- **`MAX_FORMING_BUDGET = 3000` demoted** to a hard liveness backstop only (OR-ed), never the primary decision.
- **Completed p_win on both sides** — the give-up LHS uses `R_O_completed`, never present-caps (Flaw 2: present-caps trips give-up one member short at max sunk cost → re-field loop).

### 6. Safe-mode arm
If `p_win_completed == 0` **solely** because `defense.safe_mode`, do **not** abandon: back off via `mark_unwinnable` + a **timed re-check** (bounded duration), preserving the roster. Only reinforced/permanent unwinnability abandons. (Flaw 6: safe-mode is a bounded window, not permanent unwinnability; immediate abandon re-pays travel/forming each cycle.)

## The seven fixes the adversarial pass forced (naive → corrected)

| # | Flaw in the naive synthesis | Severity | Fix (in P0) |
|---|---|---|---|
| 1 | **Bootstrap collapse** — present-caps `p_win ≈ 0` at 0 present → bid ≈ floor, give-up before member 1 | blocker | `p_win_completed`; bid floored at `SPAWN_BID_HIGH` |
| 2 | **Give-up one member short** — present-caps R_O < completed while B rises → abandon at max sunk cost | blocker | give-up LHS uses `R_O_completed` |
| 3 | **Band saturation** — `value_e` up to ~1e6 pins every objective at the 99_999 cap | blocker | reserved band: order `R_O_completed` **within** [75k, 99k) |
| 4 | **Floor thrash / rich-colony never-attack** — raw civilian-max floor oscillates & dominates | major | horizon-normalize + EMA(8) + K=20 latch |
| 5 | **Consumer flips** — bid dropping below HIGH reclassifies `>= SPAWN_BID_HIGH` gates | major | reserved band never < HIGH; grep-audit gate |
| 6 | **Safe-mode = permanent-unwinnable thrash** | major | safe-mode back-off + timed re-check, preserve roster |
| 7 | **Sunk-cost in the bid** — `bid = R_O + B` rewards over-commitment | minor | `B` in give-up only; bid on `R_O_completed` alone |

## Reuses (no parallel machinery)

`value_e` and `project_value_kind`/`project_intel`/`defense_asset_value`; `R_O`'s `round(p_win·value_e·1000/est_ticks)` transform (`squad_manager.rs:921`); `pairing_p_win` (`composition.rs:774`) — a **second** call over completed-roster caps; `room_net_roi`/`reservable_remote`/`owned_colony` (the true-e substrate under every objective class); `body_roi_milli` + `instant_spawnability_premium` (the opportunity floor); `renew_energy_cost` + `CREEP_LIFE_TIME` (the `B` identity); `create_body`/`SquadCapabilities` (projecting the completed roster); the Hungarian reassign `pairing_ev` (salvage, deferred R1). Kept: `SPAWN_BID_CRITICAL` as the income cap, `SPAWN_BID_HIGH` as the reserved-band floor, the band ladder for coarse civilian roles (claim/scout/reserve/salvage).

## Wiring (localized seams)

1. **`spawn_policy.rs`** — replace `forming_completion_bid(present, ticks) -> u32` with `forming_completion_bid(r_o_completed_milli: u32, is_defense_critical: bool) -> u32` (the reserved-band form, §3). **Delete `FORMING_WASTE_STEP_MILLI`.** ~8 lines, pure integer, host-testable.
2. **`squad_manager.rs:318` `spawn_priority_for`** — signature → `(r_o_completed_milli, is_defense_critical)`; the MEDIUM-threshold gate (only active offense/defense forms) stays; REC-052(c) `DEFENSE_SPAWN_EDGE` stays as an intra-band nudge. Call site `:2064` extracts `r_o_completed_milli` (the `p_win_completed · value_e · 1000 / est_ticks` numerator, before any anchor-add) + the critical-defense flag.
3. **`squad_manager.rs:1719`** — add `should_abandon_forming(r_o_completed_milli, b_present_milli, opportunity_floor_norm_milli, streak) -> bool`, OR-ed with the `MAX_FORMING_BUDGET` backstop; thread the per-room `opportunity_floor` EMA + the K-tick streak into `SquadManagerSystemData` from the `sink_economics` per-room civilian bids. Safe-mode arm (§6).
4. **Audit obligation (hard pre-deploy gate):** grep every `>= SPAWN_BID_` and `spawn_bid_label` consumer; re-derive the pinned host fixtures (`squad_manager.rs:3960-3990`) and the `lifecycle.rs` `combat_priority = 85_000` sim pin under the new true-value bids.

## Harness validation (red → green)

`run_forming` + the room × intel × economy matrix must assert:
- (a) a fresh 0-present squad bids `SPAWN_BID_HIGH` (bootstrap);
- (b) the bid rises monotonically with **completed-R_O**, NOT with present-count/ticks;
- (c) the bid never reaches `SPAWN_BID_CRITICAL`;
- (d) give-up fires when `R_O_completed − B < normalized floor` for K ticks, NOT on the 3000 clock, and NOT one-member-short for a normally-progressing squad;
- (e) `p_win → 0` (non-safe-mode) ⇒ abandon; safe-mode ⇒ back off + re-check, roster preserved;
- (f) two top squads ORDER by completed-R_O (no cap tie).

Matrix axes: **room** {owned-base-under-attack, reserved-remote defense, lvl0-invader-core remote, hostile-player remote (± econ-intel), far thin-margin farm} × **intel** {full econ-intel vs proxy-only — assert a proxy value never out-ranks a real-economy value of the same kind} × **economy** {rich colony (high floor → early give-up for marginal squads) vs poor colony (bleeds → gives up on affordability/p_win path)}.

**Determinism:** every float `round()→i64` before any ordering (the `squad_manager.rs:921` idiom); no HashMap iteration reaches a result; ties on stable id. Run `sim_is_deterministic_over_rounds`. **A/B caveat:** the live forming arm now DIVERGES from the sim's band baseline arm, so the M4/M5 by-construction A/B equality BREAKS — re-baseline the tournament sweep before trusting any comparison.

## Phasing

**P0 (deploy-safe, WFV-unbumped — all ephemeral, recomputed per tick, nothing serialized):** `p_win_completed` + `R_O_completed` + the reserved-band bid + the economic give-up (with EMA + K-latch) + the safe-mode arm; delete `FORMING_WASTE_STEP_MILLI`; re-pin fixtures; grep-audit; matrix + fence; private soak (verify miner-starvation ticks DOWN and forming still completes), then MMO per go-ahead.

**Deferred (additive, non-blocking, each closes a named gap):**
- **R1** — salvage via the Hungarian `pairing_ev` on the give-up RHS (full continuation-vs-salvage stopping boundary); same-tick ordering care vs the auction.
- **R2** — `γ^delay` capture-delay discount (keeps huge-value/huge-`est_ticks` objectives off the cap where `est_ticks` is large).
- **R3** — denial-of-encroaching-player: point `reservable_remote` at the enemy's scouted `source_count`/`haul_tiles` in the `ResourceDenial` arm (`war.rs:2144`), escaping the `priority·100` proxy where intel exists.
- **R4** — Defend: expected-disruption-duration `T_dis` (replaces the fixed 1500 horizon) + a room-RECOVERY/rebuild-cost term (structures + spawn + controller re-progress) — the two genuinely-absent primitives; until then room loss stays priced as denied-income-over-1500.

## Consequences

- **Positive:** forming reliability is now a *pricing* property, not a tuned band — a squad forms as hard as its objective is worth and abandons on economic merit, with hysteresis to avoid thrash. Combat forming finally competes with civilian economy in one currency for the reserved band. Removes the `FORMING_WASTE_STEP_MILLI` and `MAX_FORMING_BUDGET`-as-primary hacks. Give-up is O(1), reusing existing EV.
- **Risks / open (tracked):** (1) `opportunity_floor` empty-queue reads ~par — does `instant_spawnability_premium` + the EMA stabilize enough, or is a longer EMA needed? (2) `est_ticks` noise scales `R_O` linearly (the old band was insensitive) — does it warrant R2 sooner? (3) proxy `value_e` arms (denial `priority·100`, farm horizon 100) read ~10× smaller than the same objective with real econ-intel — proxy-never-out-ranks-real is the P0 stopgap until `project_intel` unifies the horizon (R3/R4). (4) the reserved band still cannot express a genuinely huge `value_e` (owned base ~1e6) outbidding *all* non-miner economy — true cross-lane arbitration needs `value_e` rescaled to par-milli once and the CRITICAL cap dropped (a later, larger normalization, out of P0 scope).

## End state — P0 as implemented & validated (2026-07-08)

**Shipped (committed on master, WFV-free — all R_net state is ephemeral, recomputed per tick):**
- **Bid** — `forming_completion_bid(r_o_completed_milli)` (`spawn_policy.rs`) orders the objective's completed value in the reserved band `[HIGH, CRITICAL)`; the bot computes `r_o_completed_milli` via `forming_objective_rate_milli` (`p_win` over the **full** projected roster — `caps_from_composition` — × `value_e` / `est_ticks`, reusing the `squad_objective_bid` projections). `spawn_priority_for` takes the rate. The band-scaled `present×ticks` escalation and `FORMING_WASTE_STEP_MILLI` are deleted.
- **Give-up (§5)** — `should_abandon_forming(r_o, burn, floor)` + `forming_burn_rate_milli(cost)` (kernel), wired into reconcile: while forming, a K-tick-latched (`FORMING_ABANDON_STREAK=20`), safe-mode-exempt `economic_giveup` OR's into the forming lease (`forming_budget_remaining = budget_clock && !economic_giveup`), demoting `MAX_FORMING_BUDGET` to a liveness backstop. `floor=0` (the sound value-negative bound) until A2.
- **Tests:** kernel unit tests (bid ordering, burn, give-up); harness `value_priced_completion_finishes_a_roster_tied_with_economy` + `zero_value_objective_does_not_preempt_economy`; bot `spawn_priority_for` ordering. Full suite + clippy-wasm clean + determinism fence spread-0.

**Private validation (2026-07-08, vs the pre-fix stuck state):** RALLY-holding **153→12**, RENEW **1074→302**, RETIRE **0→2 (both `GaveUp`** — the economic give-up firing), assaults active against real target rooms, `IN_ROOM_NO_FOCUS=0` (no E1 arrival failures), `ESCALATE-BLOCK=69` (reachable-subset straggler handling working). Range-1 home occupancy is now **mostly renewers** (tight-leash-correct, ttl<300) + a single healthy straggler. Conclusion: the dominant "rosters never complete → camp forever" failure is resolved; squads form on merit and abandon worthless objectives.

**Tracked residuals (production-quality: known, bounded, deferred — NOT silent gaps):**
1. **`opportunity_floor > 0`** — the give-up's economy-alternative term. Blocked on ADR 0043 **A2** (the civilian spawn lane must be true-EV `body_roi` for the floor to be commensurable). `floor=0` is sound now (abandons value-**negative** squads); the floor only *raises* the bar (abandon value-positive-but-economy-is-better). No WFV.
2. **F1 — escalated-out straggler not recycled** — a member dropped from the gather quorum (stalled past `SOLO_TRAVEL_STALL_WINDOW`) stays rostered and can camp at home until the squad's economic give-up retires it (now bounded, was 3000t). The clean fix is to recall an escalated-out member to recycle; deferred to avoid a re-spawn-churn regression against a persistent block (the war-lifecycle whack-a-mole risk) — the give-up bounds it. Track for the travel-reliability pass.
3. **Harness end-to-end give-up test** — the give-up *decision* is kernel-tested and *live*-validated; an end-to-end `run_lifecycle_churn` scenario driving the economic give-up (the room×intel×economy matrix, ADR 0042 §Harness) is the GAP-2 harness extension — deferred with the intel-axis wiring.
4. **R1–R4** (salvage, γ-discount, denial-of-player pricing, Defend rebuild-cost) — additive valuation refinements, unchanged from above.
