# ADR 0008a - Combat Tactics & Behavior Catalog (+ experiment register)

- **Status:** Proposed (catalog v0 - each entry is a testable hypothesis, tuned empirically via the experiment register; not a one-time decision)
- **Implemented subset (2026-06-18, via G3/G3-tail):** T-FOCUS-1 focus-fire, T-ENGAGE coupled-hysteresis engage/retreat, T-HEAL heal assignment, and T-POS pathfinding-scored kiting are shipped in `screeps-combat-decision` and self-play-validated (EXP-SQUAD-KITE-1). The remaining catalog entries and the EXP register below are still forward-looking — see the [master plan doc](../plans/combat-overhaul-plan.md) §5 (EXP-REGISTER).
- **Date:** 2026-06-17
- **Deciders:** William Archbell
- **Related:** Companion to [ADR 0008](0008-combat-and-squad-architecture.md) (combat & squad architecture - this details its Section 4 tactics model); [ADR 0006](0006-eval-and-iteration-harness.md) (the combat micro-sim + private-server gate that validate these); [ADR 0003](0003-behavior-modeling.md) (the anchor mover the positioning tactics ride on); [ADR 0015](0015-testing-and-validation-strategy.md) (tactics are the experimental SHELL - tuned by sim/server, not unit-tested); [ADR 0014](0014-empire-strategy-and-posture.md) (target valuation under a WarDecl). Execution: [phase-2.md](../execution/phase-2.md) workstream G (tactics in the manager) + the experiment register below. Engine ground truth: [engine-mechanics.md](../references/engine-mechanics.md), verified against the cloned engine.

> **How to read this.** "The combat arithmetic" section pins the engine-derived invariants every tactic rests on. The catalog (A-J) is the behavior to implement and experiment with: each entry is `id - trigger -> behavior -> tunable params (default-guess + sweep) -> sim-measurable success metric -> robustness`. The tunable-params table and the ordered experiment register turn the catalog into sim/server experiments - we expect to ITERATE here to find effective tactics, not to ship these defaults as final. Robustness flags: **robust** = opponent-agnostic engine-math; **mixed**/**brittle** = needs a read of the opponent (mitigated by seed diversity + an opponent roster + the MMO canary). Tactic IDs (T-AREA-n) and experiment IDs (EXP-*) are stable and appear in commits + phase-2. This is a living catalog: entries are added/retired as experiments resolve.

## The combat arithmetic (foundations)

Four invariants, all derived from fixed engine constants, underpin every tactic below. They are opponent-agnostic — the source of the catalog's robustness.

**(1) The kill inequality (focus-fire vs aggregate heal).** Combat resolves in **two phases** (`creeps/tick.js:118-135`): all damage and all heal accumulate into per-object pools during the intent phase, then at each object's own tick they net **damage first, then heal, then the death check**. Consequence: a target of effective HP `H_eff`, receiving aggregate enemy focus-heal `Hb`/tick, hit by our aggregate DPS `D`, dies in `t = ceil(H_eff/(D − Hb))` ticks **and only if `D > Hb`**. If `D ≤ Hb` the target is unkillable by that force and every shot is wasted. This is exactly the bot's `attack_parts_to_kill(target_hp, enemy_focus_heal, window, dmg_per_part)` (`damage.rs:181`) and the tower-side `should_towers_fire` / `net_tower_damage` (`damage.rs:84-92`) generalized from towers to creeps. Two corollaries: (a) heal landing the *same tick* can save a creep from otherwise-lethal damage (pre-heal is never wasted) — so always heal an exposed creep every tick; (b) to kill you must out-DPS the **whole enemy heal stack** concentrated on the focused creep, not its self-heal.

**(2) Tower range / drain math.** A tower does `600 − ((clamp(r,5,20)−5)/15)·450` damage (`tower_attack_damage_at_range`, `damage.rs:8`): 600 at r≤5, linear to 150 at r≥20; 10 energy/shot, one action/tick, heal>repair>attack priority. `N` towers stack additively and net in the same two-phase step. At the room edge (the kernel samples x=25,y=0, `tower_dps_at_room_edge`, `damage.rs:66`) a centred RCL8 bunker is ~range 20-25 so each tower floors at **150** — a **4× cut** from the 600 in-bunker figure. Edge totals: `{1:150, 2:300, 3:450, 4:600, 5:750, 6:900}`; in-bunker totals: `{1:600 … 6:3600}`. HEAL sustains 12/part raw, **48/part** boosted (XLHO2 ×4). `drain_heal_parts_for_dps(dps) = ceil(dps/12)` (`damage.rs:57`). The whole reason range management exists: a 50-part creep self-sustains ~25 HEAL = 300/tick raw → N≤2 towers at the edge unboosted; everything else needs boosts or a heal-train.

**(3) Per-part front-to-back degradation.** Hits fill the body from the **last** part backwards (`_recalc-body.js`), so `body[0]` dies first; `calcBodyEffectiveness` then drops dead parts so output degrades mid-fight. Two design facts fall out: order TOUGH/combat **front**, MOVE **back**, so a kiter keeps speed as it loses DPS (and a tank presents fresh armour first); and **TOUGH is a one-time HP buffer, not a sustained rate** — a boosted XGHO2 part = 100 hits / 0.3 = ~333 effective HP, consumed front-to-back, so model TOUGH as a fixed eHP pool and heal+MOVE as the sustained rate.

**(4) Kiting MOVE-parity.** Fatigue added per step = `(non-MOVE non-CARRY parts + carry weight) × terrainRate` (road1/plain2/swamp10); each MOVE clears 2 fatigue (boosted ×2/3/4). To move every tick: plain needs `MOVE ≥ other` (1:1), road `MOVE ≥ other/2`, swamp `MOVE ≥ 5·other` (effectively unkiteable unboosted), boosted-T3-plain `MOVE ≥ other/4`. All attack-range checks use **tick-start positions** (`processor.js` step 4, before movement executes) — that is *why* a range-3 kiter at MOVE-parity is untouchable by an equal-speed melee chaser. Kiting **loses** the instant parity breaks (swamp, dead front MOVE parts), the kiter is cornered/exit-trapped, or the enemy is also ranged (symmetric → a DPS+heal race, not a positioning win).

---

## A. Target selection & focus-fire

**T-FOCUS-1 — Net-heal-gated focus target (replace raw min-by-hits).** *Robustness: robust.*
- **Trigger:** an offensive/defensive squad has ≥1 hostile creep in the engaged room (the `compute_focus_target` / squad-combat fallback pick).
- **Behavior:** for each candidate compute (1) the squad DPS `D` landable this tick (sum of in-range members' part DPS at their actual ranges), (2) `Hb` = aggregate enemy heal reaching it (adjacent 12·mult + ranged-in-3 4·mult, from `threatmap` heal-per-tick), (3) discard candidates with `D ≤ Hb` (unkillable — do not waste fire, exactly `should_towers_fire`), (4) among killable, pick **min `ceil(H_eff/(D−Hb))`**, tie-broken by the heal-relief the target's death grants the rest of the enemy (so heal-carriers float up only when also near-killable). No-killable-creep branch keeps `InvaderCore > Spawn > Tower > other`.
- **Params:** `kill_window_ticks` = 25 (reuse `KILL_WINDOW_TICKS`; sweep 15-40); `heal_relief_weight` w in `score = ttk − w·Hb_provided` = 0.05 tick/HP (sweep 0.0-0.15); `range_penalty` = 10/tile (Overmind value; sweep 0-20); `rma_rampart_exclude` = true.
- **Metric:** sim — ticks-to-clear a tank+healer pair drops vs the current healer-first rule; wasted-fire ticks (target netted ≥0 HP) → ~0. Server — kills per energy in scripted duels rises.
- Source: Overmind `CombatTargeting.ts` (https://github.com/bencbartlett/Overmind/blob/master/src/targeting/CombatTargeting.ts).

**T-FOCUS-2 — Predicted-hits chaining (overkill avoidance / spill-to-next).** *Robustness: robust.*
- **Trigger:** ≥2 friendly shooters can hit hostiles this tick (any Duo/Quad, or multiple solo defenders in one room).
- **Behavior:** the **manager** (not each creep independently — kills the current per-creep `min_by_key(hits)` overkill) assigns targets in one pass: maintain `hits_predicted = current_hits + Hb_this_tick`; iterate shooters by descending DPS, each commits to the highest-priority target whose `hits_predicted > 0`, then subtract its expected landed damage. Booked-dead targets are skipped; later shooters spill to the next. Push per-member `AttackTarget` into `TickOrders` (the channel `squad.rs` already uses for `heal_target`).
- **Params:** `over_book_margin` = Hb·1 tick already included (optional +5% safety); `shooter_order` = by_dps_desc (alt by_range_asc, sweep).
- **Metric:** sim — overkill ratio (damage dealt to already-dead targets / total) → 0; distinct kills per 25-tick window rises for a fixed squad vs a fixed enemy line.
- Source: Overmind `CombatIntel.ts`.

**T-FOCUS-3 — Kill-the-healer-first *only when the healer is killable* (conditional).** *Robustness: mixed.*
- **Trigger:** enemy force contains ≥1 heal-bodied creep AND ≥1 non-heal target.
- **Behavior:** compute `D_on_healer` vs `healer_self+peer_heal`. Focus the healer first **iff** `D_on_healer >` that heal AND its `Hb_to_others ≥ threshold·softest_H_eff` (the 23-vs-44-tick case: removing a 600/tick healer subtracts 600 from every later target). Otherwise (rampart-shielded / peer-healed / out of net-DPS reach) treat it as just another scored candidate so it loses to a near-dead soft target. Replaces the current **unconditional** healer-first that dogpiles an unkillable healer while a one-shot kill stands next to it.
- **Params:** `healer_relief_threshold` = `Hb_to_others ≥ 0.4·softest_H_eff` (sweep 0.2-0.8); `require_net_positive_on_healer` = true.
- **Metric:** sim duels vs tank+healer — total-clear time ≤ min(always-healer-first, always-softest); zero cases of fixating on a rampart-protected healer >3 ticks at net≤0.
- Source: https://screeps.com/forum/topic/2483/simultaneous-actions-clarification.

**T-FOCUS-4 — RangedMassAttack accounting in the heal-netting ledger.** *Robustness: robust.*
- **Trigger:** our ranged creeps trigger RMA (the existing in_range cluster condition).
- **Behavior:** when booking targets dead for T-FOCUS-2, credit RMA with the engine falloff `{0:1,1:1,2:0.4,3:0.1}·10·parts` to **every non-rampart** hostile in range, so single-target shooters don't double-book what RMA already softened. **Never** count RMA against rampart-shielded hostiles — the engine skips them (`rangedMassAttack.js:38-40`); a healer that steps onto a rampart drops out of the killable set.
- **Params:** `rma_falloff` = engine-fixed; `rma_vs_single_threshold` = current `in_range_1≥3 || (in_range_3≥3 && in_range_1≥1)` (sweep cluster sizes 2-4).
- **Metric:** sim vs a tight enemy line — HP removed/tick via RMA within 5% of the falloff-weighted prediction; no rampart-shielded hostile ever scored as taking RMA.
- Source: https://wiki.screepspl.us/index.php/Combat.

**T-FOCUS-5 — Off-room/edge drain recognition on the offensive side.** *Robustness: mixed.*
- **Trigger:** a focused hostile gains hits across ticks despite our fire (returns healthier than it left) — sustain from a healer we cannot reach.
- **Behavior:** port the tower-side confirmed-drainer logic (`is_likely_tower_drain`, the bounded probe) to creep focus selection: if a candidate's hits recover under our fire, drop it from the killable set and spill to a reachable target. Prevents a squad emptying its life into an edge-kited creep whose healers sit one room over.
- **Params:** `drain_confirm_cycles` (reuse the tower `DRAIN_CONFIRM_CYCLES`); `edge_band` = x≤3‖x≥46‖y≤3‖y≥46; `probe_budget_ticks` = 3.
- **Metric:** sim with an off-screen healer — squad disengages within `probe_budget`; ticks/energy on unwinnable edge targets → ~0.
- Source: https://screeps.com/forum/topic/2801/.

---

## B. Positioning & kiting

**T-POS-1 — Hold-range-3 ranged anchor.** *Robustness: robust.*
- **Trigger:** our ranged creep is in combat and the nearest hostile with melee ATTACK parts (`threatmap` `melee_dps>0`) is at range >3, OR we have a clear shot with no melee threat within range 2.
- **Behavior:** hold the tile keeping the primary target at exactly range 3 (max RANGED reach, 0 melee return — melee does 0 at range ≥2). Each tick fire (rangedAttack or RMA per T-POS-4), then step toward range 3: away if <3, closer if >3. Prefer the move minimizing terrain rate (avoid swamp) and keeping interior position (x,y ∈ 2..=47). This is the **missing ordered-path kite** ADR 0008 §4 calls out — the logic exists only in the unreachable `fallback_movement` today.
- **Params:** `hold_range` = 3 (sweep 2..3); `approach_when_range_gt` = 3; focus = highest (heal_per_tick then melee_dps) hostile.
- **Metric:** sim — over an N-tick melee engagement our creep takes 0 melee damage (range never drops to 1) while dealing ≥10·RA/tick; our-HP-lost / enemy-HP-lost < 0.3 vs an equal-cost melee body.
- Source: Overmind `Movement.ts`; https://docs.screeps.com/api/#Creep.rangedAttack.

**T-POS-2 — Flee-at-range-2 (melee break-contact).** *Robustness: robust.*
- **Trigger:** a `melee_dps>0` hostile is at range ≤2 (about to be in melee range next tick).
- **Behavior:** top priority — vacate to a range-3 tile (still firing; ranged action and move are independent intents). The tile must (a) restore range 3 to the closest melee threat, (b) not be an exit tile, (c) not be swamp, (d) not back into a wall/dead-end (lookahead 1-2 tiles). If no range-3 tile exists, fall through to T-POS-8.
- **Params:** `flee_trigger_range` = 2; `melee_avoid_radius` = 3 (Overmind +1 cost ring for ATTACK, +3 for RANGED); `retreat_lookahead` = 2.
- **Metric:** sim — starting adjacent to an equal-parity melee chaser on plain, re-establish range 3 within 1 tick and sustain it; never two consecutive ticks of melee damage.
- Source: Overmind `Movement.ts`.

**T-POS-3 — MOVE-parity body sizing for kiters.** *Robustness: robust.*
- **Trigger:** spawning any creep intended to kite (solo ranged defender, harasser, SK ranged attacker, ranged duo/quad member).
- **Behavior:** size MOVE to clear all fatigue every tick on the expected terrain: unboosted plain 1:1 (e.g. 10RA+10MOVE — matches `sk_ranged_attacker_body`), road 1:2, boosted-T3 ~1:4 (matches the `boosted_*` ratios). Order parts MOVE **at the back** (`[TOUGH, combat, HEAL, MOVE]`) so speed survives front-part attrition. Never spawn a sub-parity kiter; under energy pressure drop combat parts before MOVE. Add a parity-assert to the kiter builders.
- **Params:** `plain_move_ratio` = 1.0; `road_move_ratio` = 0.5; `boosted_t3_move_ratio` = 0.25; `part_order` = [TOUGH, combat, HEAL, MOVE].
- **Metric:** sim — simulated post-move fatigue ≤0 every tick across life including after losing the first 30% of combat parts; SK ranged attacker holds range 3 vs a keeper (300 melee DPS @r1) for its full life taking 0 melee hits.
- Source: https://docs.screeps.com/creeps.html.

**T-POS-4 — Mass-vs-single ranged selection.** *Robustness: robust.*
- **Trigger:** our ranged creep fires this tick and ≥2 hostiles are within range 3.
- **Behavior:** compute mass = Σ over in-range non-rampart hostiles `RA·10·falloff[range]` vs single = `RA·10` on the best focus target; use RMA only when mass > single. Practically RMA wins only with multiple hostiles at range ≤2 (it is a "swarm closed on me" button, not the default).
- **Params:** falloff engine-fixed; switch when ≥2 hostiles at range ≤1, or ≥4 at range 2 (sweep).
- **Metric:** sim vs a 3-creep melee swarm that closes — total damage/tick ≥ max(pure-single, pure-mass) baseline; clears the swarm faster than either fixed policy.
- Source: Overmind `CombatZerg.ts`.

**T-POS-5 — Stay-off-exit-tiles discipline.** *Robustness: robust.*
- **Trigger:** a combat creep's chosen move would land on an exit tile (x∈{0,49}‖y∈{0,49}), or it is within 1 tile of the border with a hostile adjacent.
- **Behavior:** forbid ending a combat move on an exit tile (mark exit tiles high-cost in the combat pathfinder, except for an intentional room-transition retreat). Keep a ≥2-tile buffer; if pushed toward the border, prefer lateral (interior) retreat over backing onto the exit. One shove on an exit tile ejects you to the adjacent room, resetting positioning and often separating you from healers.
- **Params:** `exit_buffer` = 2 (sweep 2-3 vs boosted chasers); `exit_tile_cost` = very-high (not ∞, so intentional transitions remain possible); applies only `in_combat`.
- **Metric:** sim — a kiter pressured toward an edge never involuntarily transitions rooms; zero unplanned room exits in adversarial kite scenarios.
- Source: https://docs.screeps.com/api/#Room.

**T-POS-6 — Mirror-Y retreat (armour-facing block rotation).** *Robustness: mixed.*
- **Trigger:** a squad/creep with a TOUGH front is retreating from a threat that stays on one side.
- **Behavior:** use the anchor mover's `mirror_y`/orientation so the armoured face (TOUGH-heavy, body[0] side) stays toward the threat while the formation moves away — back away presenting fresh armour rather than turning and exposing soft rear/MOVE parts. Rotate the formation (engine 4-cycle) so a not-yet-damaged creep takes point when the current point's TOUGH depletes. This is the capability lead-follower structurally cannot do (ADR 0008 §5).
- **Params:** trigger when point-creep TOUGH eHP <25% OR retreat ordered; `rotation_cooldown` ≥2 ticks (anti-thrash).
- **Metric:** sim — fighting-retreat aggregate squad eHP-lost < a naive turn-and-run baseline; point-creep swaps before any creep loses a combat part.
- Source: https://wiki.screepspl.us/index.php/Combat.

**T-POS-7 — Turn-from-tower-range block.** *Robustness: robust.*
- **Trigger:** a drain/siege creep is in/near hostile tower range and tower fire dominates incoming damage (`threatmap` hostile tower positions non-empty; tower damage > enemy creep DPS).
- **Behavior:** hold at the room edge / range ≥20 from all towers (each does only 150). When forced through closer range, rotate so fresh-armour/TOUGH creeps absorb the higher-damage tiles and HEAL stays at the lower-damage edge. Step one tile further from the nearest tower whenever `net (tower_damage − our_heal) > 0`.
- **Params:** prefer range ≥20 (150/tower); drain HEAL ≥ `drain_heal_parts_for_dps(total_tower_damage)`; escalate to `drain_body_heavy` when required HEAL >13.
- **Metric:** sim/server — drain sustains indefinitely at the edge (net HP non-decreasing) against the room's actual tower count; `should_towers_fire`/`net_tower_damage` correctly predicts the sustain.
- Source: https://wiki.screepspl.us/index.php/Combat.

**T-POS-8 — Cornered fallback: commit or eject.** *Robustness: mixed.*
- **Trigger:** our kiter is at range ≤1 of a melee threat AND no retreat tile restores range 3 — the kite has failed.
- **Behavior:** pick the higher-EV option deterministically: (a) if our ranged DPS can kill the cornerer before it kills us (`attack_parts_to_kill` net of `enemy_focus_heal` within our survival window), stand and burst it; (b) else deliberately retreat through the room exit to break contact and reset (the one sanctioned exit-tile use); (c) if a healer/rampart is one tile away, fall back onto it. Never thrash between failed range-3 moves.
- **Params:** `survival_window = our_HP / (enemy_melee_dps − our_heal)`; commit if `ticks_to_kill_target ≤ survival_window`; else eject.
- **Metric:** sim — in forced-corner scenarios the creep either kills the cornerer (when math says it can) or survives by exiting, vs the naive baseline of dying in place.
- Source: Overmind `CombatZerg.ts`.

**T-POS-9 — Don't-kite-vs-ranged switch.** *Robustness: robust.*
- **Trigger:** the dominant threat is RANGED-based (`threatmap` ranged_dps > melee_dps) with MOVE-parity ≥ ours.
- **Behavior:** recognize kiting wins no positioning here (both deal 10/part at range 3; stepping back doesn't reduce incoming). Switch from "hold range 3 and retreat" to a DPS+heal race: maximize ranged uptime + heal stack, focus-fire their healers first (collapse their `enemy_focus_heal`), and only retreat to consolidate with our healers or to disengage if `attack_parts_to_kill == None` (escalate squad COUNT instead, Solo→Duo→Quad).
- **Params:** classify ranged-dominant when Σranged_dps > Σmelee_dps; `disengage_if attack_parts_to_kill==None`; target priority HEAL first.
- **Metric:** sim — vs a ranged-mirror, the bot does NOT waste ticks kiting; win/loss vs ranged opponents improves over the always-kite baseline.
- Source: https://wiki.screepspl.us/index.php/Combat.

---

## C. Tower handling

**T-TOWER-1 — Edge-range drain (range ≥20 self-heal tank).** *Robustness: robust.*
- **Trigger:** target room has hostile towers (count N in `RoomData`) and a reachable border tile at range ≥20 from ALL towers, AND `drain_heal_parts_for_dps(N·150)` fits the squad's HEAL budget. Gate on `tower_dps_at_room_edge`.
- **Behavior:** spawn `duo_drain` (2× drain body, `FormationMode::Strict` Line so they stay ADJACENT and cross-heal at range 1 — rangedHeal at 16/part boosted is ⅓ throughput and under-heals). Path to a border tile maximizing range from every tower, step just inside so towers fire, self+cross-heal through 150/tower to burn 10 energy/tower/tick. Hold until target energy ~0, then push the real siege/dismantle squad in.
- **Params:** `operating_range` = 20 (sweep 18-23); HEAL via `drain_body_for_tower_dps` (13 for ≤1 tower, 20 heavy for ≥2); `cross_heal_pair` = true; `push_when_target_energy_below` = 200 (sweep 0-500); `retreat_hp_fraction` = 0.3.
- **Metric:** sim — target tower energy → 0 within K ticks while drain creeps stay above `retreat_hp_fraction`; `energy_drained_per_our_energy_spent > 1`.
- Source: https://wiki.screepspl.us/Combat/; https://screeps.com/forum/topic/2995/.

**T-TOWER-2 — Range-management dismantle line (operate above range 5).** *Robustness: robust.*
- **Trigger:** sieging a towered room where the breach corridor (`breach_path_blockers`) lets dismantlers work walls/ramparts at range ≥6 from the towers (an outer wall far from the centred cluster).
- **Behavior:** prefer breach tiles where the max tower contribution is in the falloff band (range 10-19 → 450-180/tower) over the worst in-bunker tile (600/tower). Dismantle the rampart; healers sit adjacent. Re-evaluate the tile each tick as walls fall and the reachable set changes.
- **Params:** `min_dismantle_range` = 6 (sweep 6-12); `accept_extra_path_ticks_for_range` ≤8; `switch_to_in_bunker_when` remaining_walls ≤2.
- **Metric:** sim — total HEAL parts to keep the line alive is lower at the chosen range than at range 5 (compare `N·tower_dmg(range)` vs `N·600`), and wall HP/tick destroyed stays positive after subtracting tower repair (`tower_repair_at_range`, up to 800/tower).
- Source: https://wiki.screepspl.us/Combat/.

**T-TOWER-3 — Boosted heal-train sizing for in-bunker siege.** *Robustness: robust.*
- **Trigger:** the NO-GO check (T-TOWER-4) passes for a boosted assault (N ≤ ~5 in-bunker, or any N at the edge) and the boostqueue can satisfy XLHO2/XGHO2/XZHO2.
- **Behavior:** size the heal escort to the **in-bunker** damage the dismantler will eat: dedicated 25-HEAL boosted healers, **1 per ~2 towers** (each = 1200/tick = 2·600). The dismantler carries XGHO2 TOUGH as a *burst buffer* (~10 parts ≈ 3330 eHP) to survive the spike before heal locks on, NOT as sustained mitigation. Healers stay ADJACENT (range 1).
- **Params:** `healers = ceil(N·600 / (25·48)) = ceil(N/2)` (sweep per-healer HEAL 20-33); `tough_parts` on dismantler 8-12 (model as one-time HP); `heal_range` strict=1; `abort_if_tough_depleted_and_heal_deficit` = true.
- **Metric:** sim — dismantler net HP non-decreasing on the worst tile (sustained heal ≥ N·600), TOUGH depletes no faster than walls fall; squad completes the breach before any member drops below retreat threshold.
- Source: https://github.com/bencbartlett/Overmind/blob/2eae97b/src/creepSetups/setups.ts; https://screeps.com/forum/topic/2801/.

**T-TOWER-4 — NO-GO gate (skip un-attackable towered rooms).** *Robustness: robust.*
- **Trigger:** before committing a drain or siege squad: compute `T_dps = N · tower_attack_damage_at_range(planned_operating_range)` vs the squad's sustainable heal (Σ HEAL_parts · (12|48)).
- **Behavior:** if `T_dps > sustainable_heal` AND escalating count/boost tier can't close the gap (unboosted N≥3 at edge / N≥2 in-bunker; boosted N≥6 in-bunker without a 3+ healer train), **DECLARE NO-GO**: do not spawn, log the deficit, fall back to harass/blockade or wait for boosts. Wires to ADR 0008's `UnwinnableTarget` backoff so the room isn't re-attempted for a cooldown. Prevents feeding creeps into a room that mathematically cannot be cracked.
- **Params:** `no_go_margin` = 1.0 (require sustainable_heal ≥ margin·T_dps; sweep 1.0-1.3 for tower re-focus buffer); `planned_operating_range` from the breach/edge planner; `reconsider_after_ticks` = 3000.
- **Metric:** sim/live — zero squads spawned-then-wiped against rooms the math flagged NO-GO; every room cleared by a committed squad had passed the gate; track false-NO-GO rate (skipped-but-crackable) separately.
- Source: https://wiki.screepspl.us/Combat/.

**T-TOWER-5 — Defender hold-fire vs a confirmed drain.** *Robustness: robust.*
- **Trigger:** our defending tower(s) target a hostile and `net_tower_damage(target) ≤ 0` (total ≤ target heal/tick) AND the target is near the room edge (`is_likely_tower_drain` already implements this).
- **Behavior:** HOLD FIRE — firing only converts stored energy into wasted shots the drain out-heals (the attacker's win condition). Keep towers full; redirect fire only where `net > 0`, and reserve energy for the real assault behind the drain. Resume if the target moves into a net-positive tile (range drops / its healers die).
- **Params:** `hold_fire_when_net_dmg≤0` (the `should_towers_fire` gate); `edge_band` x/y ≤3 or ≥46; `reserve_energy_for_assault_fraction` = 0.5.
- **Metric:** sim — our tower energy stays above reserve while a hostile drain sits at the edge; when the real assault arrives towers still have energy for net-positive shots.
- Source: https://www.jonwinsley.com/screeps/2021/08/17/screeps-patrolling-perimeter/; https://wiki.screepspl.us/Combat/.

**T-TOWER-6 — Two-phase pre-heal timing (don't lose the tank to a spike).** *Robustness: robust.*
- **Trigger:** a drain/siege creep on a fired-upon tile where one tick of aggregate tower damage approaches its current HP.
- **Behavior:** because same-tick heal can save a creep from otherwise-lethal damage, schedule heal **every** exposed tick (never skip to reposition) and never let HP fall below one tick of incoming damage before heals are committed. If `HP < N·tower_dmg(range)` and heal-this-tick < the deficit, step OUT one tile (range up → damage down) rather than eat a lethal net.
- **Params:** `safety_hp_floor = N·tower_dmg(range) − committed_heal_this_tick`; `heal_every_exposed_tick` = true; `prefer_step_out_over_eat_lethal` = true.
- **Metric:** sim — zero drain/siege deaths attributable to a single-tick spike where same-tick heal would have saved them (deaths only on sustained deficit). Confirm via the deterministic micro-sim (ADR 0006).
- Source: https://screeps.com/forum/topic/2801/.

**T-TOWER-7 — Drain-then-economic-attrition (cheap creep, expensive defense).** *Robustness: mixed.*
- **Trigger:** target room is edge-drainable (N≤2 unboosted, or any N boosted) but a full bunker breach is NO-GO/too costly; goal is to grind the defender's economy.
- **Behavior:** field the cheapest drain that out-heals the edge damage (`drain_body` with just enough HEAL for N·150) and hold indefinitely. Each tick the defender either burns 10 energy/tower firing or lets the drain sit; either way our rebuild/heal spend per 1000 ticks < their forced tower spend. Rotate a fresh drain in before the old one dies. **Revocable** — abort if they stop firing (net ≤0 means we are no longer draining; the one opponent-dependent read).
- **Params:** `min_heal_for_edge = drain_heal_parts_for_dps(N·150)`; `rotate_at_remaining_life` = 200; commit only if `our_cost_per_1000 < their_tower_energy_per_1000`.
- **Metric:** sim/live — defender tower energy → 0 and their upgrade rate drops while our spend/1000 < their forced tower spend; cost-exchange ratio > 1 sustained.
- Source: https://screeps.com/forum/topic/2995/; https://wiki.screepspl.us/Combat/.

---

## D. Rampart / wall breach

**T-BREACH-1 — Cheapest-corridor breach point selection (min-cut edge).** *Robustness: robust.*
- **Trigger:** a combat objective (controller/source/core) is walled off; `breach_path_blockers` returns a non-empty corridor.
- **Behavior:** target the **FIRST dismantlable blocker** in the returned walk-order corridor (the kernel weights `BREACH_HIT_WEIGHT=4096` per hit ≫ the 2500-cell step ceiling, so it strictly minimizes total HP-to-clear, ties broken by path length — the attacker's-eye min-cut edge). Re-evaluate only on blocker-set CHANGE (fingerprint), never on hit drift, so the dismantler never flaps mid-chew. If it returns None (sealed past the hit horizon or terrain), abort/escalate to drain rather than chew an infeasible tile.
- **Params:** objective set = {controller, sources}; `max_structure_hits` horizon (`features.derelict.max_structure_hits`, default 2_000_000; sweep 1e6..3e7 for war vs salvage); `BREACH_HIT_WEIGHT` = 4096 (must stay > 2500); corridor cache TTL = until fingerprint change.
- **Metric:** sim — total HP cleared to reach the objective equals the min over all gaps; live — dismantler reaches the objective without re-targeting more than once per blocker-death.
- Source: https://wiki.screepspl.us/Combat/.

**T-BREACH-2 — Dismantle, never melee/ranged, to chew structure HP.** *Robustness: robust.*
- **Trigger:** the chosen breach tile carries a rampart or constructed wall (`BreachBlocker::Dismantlable`).
- **Behavior:** assign WORK dismantlers (`siege_dismantler_body`); do NOT spend ATTACK/RANGED on the structure. dismantle is 50/part vs ATTACK 30 (1.67×) and RANGED 10 (5×), and refunds 0.25× the dealt energy. Reserve melee/ranged exclusively for defenders at the breach mouth.
- **Params:** blocker == Dismantlable; WORK parts W (siege body repeat); boost tier {none,T1,T2,T3} → per-part {50,100,150,200}; default T3 (XZH2O) for any rampart > ~500k HP.
- **Metric:** sim — HP/tick on the tile = `W·50·b_mult`; ticks-to-break beats a same-energy ATTACK body by ≥1.67× and ranged by ≥5×.
- Source: https://wiki.screepspl.us/Combat/; https://screeps.com/forum/topic/194/.

**T-BREACH-3 — Size WORK to BEAT net repair, not raw HP.** *Robustness: robust.*
- **Trigger:** planning a breach against a room with towers and/or repairers covering the target tile.
- **Behavior:** compute `net_per_tick = W·50·b_mult − R_repair(tile)`. Size W so net is comfortably positive (≥2× margin) for the contact window. The two-phase rule means partial WORK does nothing if repair ≥ your dismantle that tick. If no affordable W makes net positive (towers point-blank + repairers), do NOT commit — switch to tower suppression (T-BREACH-6) or stack dismantlers (T-BREACH-7). Worked floor: a single tile under full tower+repair coverage is **not** breachable by one dismantler within its 1500-tick life.
- **Params:** `R_repair` = Σ `tower_repair_at_range(tile)` over hostile towers + estimated creep repair (repairers·WORK·100); `safety_margin` m = 2.0 (sweep 1.3-3.0); assume worst-case all towers can hit the tile.
- **Metric:** sim — breach completes within one dismantler life when `net·1500 ≥ H`; mission aborts (no wasted creep) when net ≤0.
- Source: https://wiki.screepspl.us/Combat/.

**T-BREACH-4 — Breach-then-hold-then-pour (heal-screened single dismantler).** *Robustness: mixed.*
- **Trigger:** a multi-creep squad is breaching a single-tile gap (column mode).
- **Behavior:** Phase 1 HOLD — attackers + healers form up one tile back, keeping the dismantler out of defender melee range while it chews. Phase 2 POUR — once the rampart falls, **relax-to-column** (the corridor mode; an anchored 2×2 cannot fit a 1-wide gap) and feed creeps single-file, the dismantler holding the gap open by re-dismantling any rebuilt rampart for N ≥ squad-size ticks. Healers focus all HEAL on whichever creep is in the gap.
- **Params:** gap width == 1; `hold_open_ticks` = member_count + 2; healer focus = single creep in gap; dismantler retreat trigger = `own_hits < incoming_tower_dps · 3`.
- **Metric:** sim — all N members get inside with 0 deaths in the gap; the breach does not reseal before the last creep passes.
- Source: https://github.com/bencbartlett/Overmind/blob/master/CHANGELOG.md.

**T-BREACH-5 — Never RMA against a held breach.** *Robustness: robust.*
- **Trigger:** a ranged creep is at the breach face and defenders stand on the rampart/behind it (all in-range hostiles rampart-shielded).
- **Behavior:** use single-target rangedAttack on exposed (non-rampart) targets only; switch to dismantle (if WORK) or single-target on the rampart structure. Suppress any auto-RMA heuristic when all in-range hostiles are rampart-shielded — the engine skips them, RMA deals 0.
- **Params:** RMA permitted only when ≥1 in-range hostile is NOT rampart-shielded; otherwise hard-disabled (no tunable).
- **Metric:** sim — a ranged creep using RMA vs 4 rampart-shielded defenders deals 0 net (confirm the engine skip); single-target on the rampart yields the expected 10/part chip.
- Source: https://wiki.screepspl.us/Combat/.

**T-BREACH-6 — Drain towers before breaching when net repair is negative.** *Robustness: mixed.*
- **Trigger:** `net_per_tick ≤ 0` for any affordable dismantler because hostile towers can repair the breach tile faster than the squad dismantles.
- **Behavior:** deploy edge drains (`drain_body_for_tower_dps`) to tank tower fire and force towers to spend energy until they empty. Once tower energy is depleted, `R_repair` collapses and the dismantler's net goes positive — THEN breach (the multi-wave handoff, T-CTRL/I cross-ref).
- **Params:** drain HEAL = `drain_heal_parts_for_dps(edge tower DPS)`; breach-go signal = hostile tower energy < cost-to-repair-one-tile/tick.
- **Metric:** sim — after K drain-ticks tower energy → 0; dismantler then breaks the tile in `H/(W·50·b_mult)` ticks with R≈0.
- Source: https://wiki.screepspl.us/Combat/; https://screeps.com/forum/topic/2809/.

**T-BREACH-7 — Stack dismantlers when a solo can't beat repair within a life.** *Robustness: robust.*
- **Trigger:** `H/net_per_tick(single) > 1500` but `net_per_tick(single) > 0` (repair beatable, just slow), OR net is barely positive.
- **Behavior:** place 2+ dismantlers adjacent to the SAME tile so dismantle sums in one tick: `net = (ΣW_i)·50·b_mult − R`. Stagger spawns so a fresh one arrives before the first dies (continuity); healers cover the stack.
- **Params:** `stack_size S = ceil(H / (T_budget·50·b_mult) / W_per_creep)` accounting for R; stagger so a replacement arrives ~50 ticks before death.
- **Metric:** sim — aggregate dismantle·ticks ≥ H + R·ticks before wipe; breach completes with continuous coverage (no idle gap where R re-heals the tile).
- Source: engine arithmetic (sources pending — rests on `dismantle = WORK·50/part` and two-phase netting).

**T-BREACH-8 — Abort/route-around impassable or over-horizon tiles.** *Robustness: robust.*
- **Trigger:** `breach_path_blockers` marks the only corridor through a `BreachBlocker::Impassable` (undismantlable: controller/core/keeper-lair under it, or HP past `max_structure_hits`), or returns None.
- **Behavior:** never assign a dismantler to an impassable tile (it would wedge forever — the `can_dismantle` guard). Re-route to an alternate objective, raise `max_structure_hits` only deliberately for a war mission, or fall back to a different entry. For salvage, abandon the room rather than chew a mega-wall.
- **Params:** blocker == Impassable OR hits > horizon; horizon (salvage default low, war override high); fallback = nearest reachable objective.
- **Metric:** sim/host — mission terminates instead of pinning a dismantler on an unbreakable tile (preserves the `within_dismantle_hits_horizon` completion invariant).
- Source: engine arithmetic + `dismantlebehavior.rs` `BreachBlocker` (sources pending).

---

## E. Healer handling & squad sustain

**T-HEAL-1 — Concentrate-heal + predictive pre-heal on the focused creep.** *Robustness: robust.*
- **Trigger:** our squad is being focused — a member took damage last tick, or sits in the enemy's fire fan.
- **Behavior:** keep `squad.rs`'s deficit-sorted greedy assignment (adjacent-12 preferred over ranged-4, overkill-capped, plus the unassigned-healer proactive pre-heal on `damage_taken_last_tick`) — a good pattern. Mirror what we model the enemy doing: stack ALL our healers on the single creep the enemy focuses (highest `damage_taken_last_tick`) so OUR Hb makes the enemy's `D ≤ Hb` and that creep is unkillable (the kill inequality from defense). **Fix the math:** use the boost-aware runtime model (HEAL = 48 boosted, `damage.rs`) instead of the flat 12/part, and recompute `heal_power` each tick (a creep that lost HEAL parts is reassessed). If predicted incoming this tick > our max heal, rotate that creep to a safe slot (`reassign_slots` scores healers to the back) BEFORE it drops, not after.
- **Params:** `preheal_lookahead` = 1 tick (sweep EMA over 2-3 ticks); `rotate_trigger` = predicted_incoming > 0.8·our_heal_capacity (sweep 0.6-1.0).
- **Metric:** sim — when our heal capacity > enemy D on the focused creep, its min-HP-fraction stays >0 (never dies); rotations happen before HP crosses the retreat threshold.
- Source: https://screeps.com/forum/topic/319/.

**T-HEAL-2 — Cross-heal adjacency discipline (range 1, not range 3).** *Robustness: robust.*
- **Trigger:** any heal-bearing formation (drain pair, duo, quad) where members rely on mutual heal.
- **Behavior:** keep healers ADJACENT to the focused creep — rangedHeal is 4/part (16 boosted) = ⅓ of adjacent 12/part (48 boosted), so a range-3 heal-train silently under-delivers and breaks the kill inequality from the wrong side. `duo_drain` uses `FormationMode::Strict` Line precisely for this. Out-of-range healers fall back to `heal_best_nearby` rather than wasting the tick.
- **Params:** `heal_range` strict = 1; `ranged_heal_fallback` = nearest in range 3.
- **Metric:** sim — sustained heal delivered to the focused creep within 5% of `Σ adjacent HEAL · 48`; no tick where a healer idles while a damaged ally is in range 3.
- Source: engine arithmetic (HEAL 12 r1 / rangedHeal 4 r3, verified).

**T-HEAL-3 — eHP / aggregate-heal estimation correctness (the gate's input).** *Robustness: mixed.*
- **Trigger:** any winnability/abandon decision that consumes `enemy_focus_heal` (`RoomThreatData.estimated_heal`) or a boosted-TOUGH eHP estimate.
- **Behavior:** ensure `enemy_focus_heal` reflects only **reachable** healers (adjacent-12 + ranged-in-3) on the focused creep — over-counting (summing ALL hostile heal regardless of reach) makes the bot abandon winnable fights; under-counting makes it feed a heal wall. For TOUGH, `threatmap` assumes T3 (×0.3 ≈ 333 eHP/part) — validate against real boosted bodies before trusting the abandon decision, since a mis-estimate biases T-ENGAGE-2 both ways.
- **Params:** `heal_reach_model` = reachable-only (adjacent 12 + ranged-3 4); `tough_eHP_per_part` = 333 (T3 assumption; sweep against measured bodies).
- **Metric:** sim — modeled `enemy_focus_heal` matches the actual heal landing on the focused creep within 10%; abandon decisions track the true winnable/unwinnable label.
- Source: https://screeps.com/forum/topic/2801/.

---

## F. Engage / commit / retreat / abandon

**T-ENGAGE-1 — Winnability gate before committing a wave.** *Robustness: robust.*
- **Trigger:** `AttackOperation` has recon (towers, estimated_dps, estimated_heal, safe_mode) and is about to build a force plan / launch a wave.
- **Behavior:** gate commit on the kill-math — can our planned composition out-DPS the enemy's aggregate focus heal within the kill window net of tower damage? If `attack_parts_to_kill == None` (>MAX_OFFENSE_PARTS for a solo) escalate squad COUNT, not size. Scale composition by detected threat (`plan_by_detected_threat`: towers≥4 → drain+quad; towers≥2 OR dps>200 OR heal>100 → quad; towers≥1 OR dps>0 → duo; else solo). If safe_mode is ACTIVE, delay (our combat is zeroed 20K ticks); if only available, plan the CLAIM-strike opener (T-CTRL-1). Refuse to commit if even a max-COUNT force can't break the pooled heal.
- **Params:** `kill_window_ticks` = 25; `max_offense_parts_solo` = 25; `tower_dps_lookup` = `tower_dps_at_edge` (cached); `delay_on_active_safe_mode` = true; `max_waves` = 3; escalation thresholds tunable.
- **Metric:** sim — win-rate of committed waves vs the predicted-winnable label; false-commit rate (launched then wiped without a kill) → 0.
- Source: engine arithmetic + `attack.rs` (sources pending).

**T-ENGAGE-2 — Aggregate-heal escalate-vs-abandon gate for squad sizing.** *Robustness: robust.*
- **Trigger:** `squad_defense` escalation check, or any squad's per-tick should_retreat, while hostiles present.
- **Behavior:** compute required `D* = enemy_focus_heal + worst_target_H_eff/kill_window`. Compute achievable D at current count and at Quad (4·per-creep cap). If `D*` exceeds even Quad-capacity AND the enemy heal is sustained (healers alive, room not energy-starved) → mathematically unwinnable: ABANDON instead of feeding creeps (Quad cap is a hard ceiling). If `D*` is between current-D and Quad-D → escalate to the minimum count crossing it (`N = ceil(D*/per_creep_DPS)`). Reuse `attack_parts_to_kill == None` as the per-creep can't-solo signal.
- **Params:** `kill_window` = 25; `sustain_check` = enemy_healers_alive && room_energy_ok; `quad_cap_parts` = 100; `abandon_hysteresis_ticks` = 5 (anti-flap).
- **Metric:** sim — squad no longer spawns a 3rd/4th creep into a provably-unwinnable heal wall; against winnable comps it escalates to exactly the count crossing D*.
- Source: Overmind `CombatIntel.ts`.

**T-ENGAGE-3 — Coupled-hysteresis retreat/re-engage.** *Robustness: robust.*
- **Trigger:** a squad's per-tick retreat evaluation (replaces the decoupled squad-level `any<25%` and per-creep `<50% out / >80% in` thresholds that yo-yo today).
- **Behavior:** one squad-owned policy: retreat when avg HP < `retreat_threshold` OR any member < a hard floor; re-engage only above a separated higher band. `retreat_threshold` is **enemy-DPS-aware** (sized from the threat model), not a flat 0.3. Per-creep states are *subordinate* — an individually-critical creep requests retreat but does not unilaterally flip the squad.
- **Params:** `retreat_threshold` enemy-DPS-aware (baseline avg HP 0.4; sweep 0.3-0.6); `reengage_band` = retreat_threshold + 0.2 (sweep 0.1-0.3); `hard_floor` = 0.2; `deficit_retreat` = incoming > 10× heal/tick.
- **Metric:** sim — retreat/re-engage oscillations per engagement → near 0; squad disengages before a member dies and re-commits once safely above the band.
- Source: ADR 0008 §4 (sources pending — rests on the decoupled-threshold yo-yo diagnosis).

**T-ENGAGE-4 — Wave-wipe vs reinforce (TTL-cohesive respawn).** *Robustness: mixed.*
- **Trigger:** a squad in an active mission has lost members (partial wipe) or members are aging out (TTL mismatch within the squad).
- **Behavior:** prefer WIPE-AND-RESPAWN-COHESIVELY over trickle-reinforce — renewing a 40-part body regains only ~15 TTL/intent and strips boosts, and a trickled replacement arrives alone and out-of-cohesion (cohesion is an ADR 0008 invariant; a lone reinforcement gets focused and dies). On full wave wipe, `handle_wave_wipe` increments the wave and respawns the whole squad fresh from Planning so the new squad arrives cohesive. Give up at `max_waves`. During Spawning/Rallying, renew only members below `RENEW_TTL_THRESHOLD` at a home with `≥ RENEW_MIN_ROOM_ENERGY` and an idle spawn, non-CLAIM only.
- **Params:** `max_waves` = 3 (sweep 2-5); `renew_ttl_threshold` = 1200; `renew_min_room_energy` = 10000; prefer respawn_cohesive; SquadContext retreat thresholds per T-ENGAGE-3.
- **Metric:** sim — cohesive-respawn waves achieve higher kill-efficiency than trickle-reinforced; squad cohesion (members within range 1) at engagement correlates with win-rate.
- Source: engine arithmetic (renew strips boosts / 1.2 ratio, verified) + `attack_mission.rs` (sources pending).

**T-ENGAGE-5 — Target valuation: loot/territory vs cost/risk.** *Robustness: mixed.*
- **Trigger:** `WarOperation` offense evaluation scoring candidate rooms (resource denial, expansion, invader core, power bank).
- **Behavior:** `score = value − distance_penalty − tower_penalty − safe_mode_penalty`, gated by economy. Hostile player rooms only if `total_stored_energy > 150K` and `min_distance ≤ 6`. Power banks only if power ≥1000, ticks_to_decay > `power_bank_min_ticks_needed`, economy >100K, under the concurrency cap. Invader cores scored by detected DPS/heal (not core level). The cost side MUST include the CLAIM-body sink for any de-claim/siege (~405K energy to force one RCL8 downgrade, T-CTRL-2) — a room is worth a siege only if territory/loot value exceeds that plus the wave cost.
- **Params:** `min_economy_for_player_attack` = 150000; `max_distance_player` = 6; `tower_penalty_per` = 5.0; `safe_mode_penalty` = 20.0; `power_bank_min_power` = 1000; `max_concurrent_attacks` scales with room_count + economy.
- **Metric:** sim/server — positive net resource/territory ROI on committed attacks; `should_abort` rate stays low because valuation pre-filtered unwinnable/unaffordable targets.
- Source: ADR 0014 + `war.rs` (sources pending).

---

## G. Per-composition playbooks

Concrete 50-part RCL8 breakdowns. Boosts: TOUGH XGHO2 ×0.3, HEAL XLHO2 ×48/part, RANGED XKHO2 ×30/part, ATTACK XUH2O ×90/part, dismantle XZH2O ×200/part, MOVE XZHO2 (boosted MOVE clears ~1 non-move per 4 on plain). The bot encodes all of these in `composition.rs` (`solo_ranged`, `duo_attack_heal`, `duo_tank_heal`, `quad_ranged`, `quad_siege`, `duo_drain`, `solo_harasser`, `duo_melee_heal`, `duo_sk_farmer`, `power_bank_duo`, `power_bank_haulers`, `solo_core_attacker`) + the `bodies.rs` builders.

**T-COMP-1 — Boosted RA+HEAL brick quad (uniform members).** *Robustness: robust.* Counters: defended RCL7-8 rooms, ≥2-tower nests, boosted defender stacks.
- **Trigger:** offensive objective vs ≥2 active towers OR a defended RCL7-8 controller/core, home can produce T3 XGHO2/XLHO2/XKHO2/XZHO2 and ≥4 boostable bodies.
- **Behavior:** 4 IDENTICAL 50-part bricks (not the current 2-ranged/2-healer split) so any member can tank/heal/DPS and the box stays symmetric under rotation. Per member: **6 TOUGH (XGHO2) + 16 RANGED (XKHO2) + 14 HEAL (XLHO2) + 14 MOVE (XZHO2)**. Box2x2 strict; focus-fire one target with all 64 RA; RMA only when ≥2 hostiles cluster in r3 AND none rampart-shielded. Pool heal = 4·14·48 = 2688/tick > 6-tower in-bunker 3600 after XGHO2 (×0.3 → ~1080 effective) — survives indefinitely.
- **Params:** per-member TOUGH 4-8 (6), RA 14-18 (16), HEAL 12-16 (14), MOVE ~12-14 (fatigue=0 in formation on plain); `retreat_threshold` 0.3→0.5 sweep; RMA switch cluster≥2 AND shielded_fraction<0.3.
- **Metric:** sim — survives a 6-tower nest indefinitely and kills a lone 25-HEAL boosted defender in ≤8 ticks; server — takes an RCL7 controller room without losing a member.
- Source: https://github.com/JonathanSafer/screeps/issues/69; https://wiki.screepspl.us/Combat/; https://screeps.com/forum/topic/860/.

**T-COMP-2 — Unboosted swarm quad (low-RCL / pre-lab).** *Robustness: robust.* Counters: invader-core rooms, RCL≤5 players, contested remotes with 0-1 towers.
- **Trigger:** offensive/heavy-defense objective when home CANNOT boost (no labs/minerals) AND target has 0-1 towers.
- **Behavior:** 2× RANGED+MOVE bricks (`quad_member_body`) + 2× HEAL+MOVE healers (`duo_healer_body`) — the bot's current split is correct here. Box, focus-fire. Accept attrition; never sieges maxed ramparts. Disengage if any single target's self+ally heal > total quad RA DPS (`attack_parts_to_kill == None`).
- **Params:** member RA 8-15 scaling with `energy_capacity` (550-2300+); healer HEAL 6-13; TOUGH 4-6 front; 1:1 MOVE.
- **Metric:** sim — clears a 1-tower defender + 2 small defenders without total wipe; if `attack_parts_to_kill == None` the squad retreats, no death-spiral.
- Source: https://wiki.screepspl.us/Combat/; https://wiki.screepspl.us/Creep_body_setup_strategies/.

**T-COMP-3 — Attacker+healer duo (universal workhorse).** *Robustness: robust.* Counters: SK, power bank, invader core, lightly-defended remote.
- **Trigger:** single-target PvE or harassing one undefended/lightly-defended remote; default unit when a quad is overkill/unaffordable.
- **Behavior:** 2 creeps one tile apart (Line strict). Attacker is RANGED (kite for SK/players) or MELEE (power bank/core). Healer trails, tops attacker each tick, never leads. SK: kite so SK never reaches r1.
- **Params:** SK duo — attacker 10RA+10MOVE+1HEAL, healer 10HEAL+12MOVE (`sk_*` bodies). Power bank — attacker **20 ATTACK** (cap: 600 dealt → 300 reflected) + 20MOVE, healer 25HEAL (300/tick = the reflect). Core — 5-10 ATTACK + small heal.
- **Metric:** sim — SK duo sustains a 3-source SK room 0 deaths over 1500 ticks; power-bank duo destroys a 2M bank (~3334 ticks) with healer never dropping attacker below 50%.
- Source: https://wiki.screepspl.us/Combat/; https://docs.screeps.com/creeps.html.

**T-COMP-4 — Drain pair (tower-energy bleed).** *Robustness: robust.* Counters: high-wall RCL7-8 rooms (as the siege opener), forcing tower-energy exhaustion.
- **Trigger:** target room's net tower energy is the siege bottleneck.
- **Behavior:** 2 TOUGH+HEAL tanks at the edge (range ≥20 → 150/tower), strict-adjacent so they cross-heal. NO offense — pure soak. Sit until towers run dry, then signal the siege squad. Retreat one tile out to reset if HP critical.
- **Params:** vs 1 edge tower (150): 13HEAL+10TOUGH (`drain_body`, 156 heal). vs 2 (300): boost HEAL (7 XLHO2 = 336) OR raise to 25 unboosted (`drain_body_heavy`'s 20 is **undersized** for 2 at the edge — see open question). vs 3+: XGHO2+XLHO2 mandatory.
- **Metric:** sim/server — a 2-tank pair survives indefinitely at the edge of a 2-tower room AND the room's stored energy → 0; tanks never die (net heal ≥ net tower damage).
- Source: https://screeps.com/forum/topic/860/; https://wiki.screepspl.us/Combat/.

**T-COMP-5 — Boosted dismantle / siege quad (rampart breach).** *Robustness: mixed.* Counters: maxed ramparts/walls gating the controller/core, after towers are drained/out-healed.
- **Trigger:** target has maxed ramparts/walls (>1M HP) AND towers are drained or out-healed.
- **Behavior:** swap the quad's RANGED for WORK: 2 dismantlers (XZH2O = 200/part) + 2 boosted healers. Dismantle the single breach-corridor blocker (`breach_path_blockers`), don't spread damage. Melee defenders can't reach dismantlers behind their own rampart line once towers are handled.
- **Params:** dismantler — 8 TOUGH (XGHO2) + ~20 WORK (XZH2O) + ~12 MOVE = 4000/tick each, 8000/quad → ~125 ticks/1M HP; healer 6 TOUGH + 14 HEAL (XLHO2) + MOVE. Unboosted siege only vs walls <300k. Sweep WORK 16-25.
- **Metric:** sim — breach a 3M-HP rampart in <450 ticks while healers hold dismantlers >70%; server — open a corridor to an RCL7 controller without losing the squad.
- Source: https://github.com/JonathanSafer/screeps/issues/69; https://wiki.screepspl.us/Combat/.

**T-COMP-6 — Single ranged harasser (remote denial).** *Robustness: robust.* Counters: enemy/neutral remote mining (reservers/haulers), no tower coverage.
- **Trigger:** enemy/neutral remote mining or reservation to disrupt cheaply, no tower coverage and no committed enemy combat squad present.
- **Behavior:** 1 disposable all-RANGED+MOVE kiter, no formation (Loose). Kite miners/haulers/reservers at r3; never engage anything that out-heals it; flee any approaching duo/quad. Prioritize reservers and haulers (deny logistics) over miners.
- **Params:** 3-5 repeats RANGED+MOVE (`harasser_body`, 1300e cap, 100% MOVE); `flee_range` 4-5; engage only if target_heal/tick < own RA DPS; disengage if any HEAL+ATTACK creep enters.
- **Metric:** sim — reduces a target's remote income (kills reservers/haulers) over its 1500-tick life while surviving; cost-per-disruption favorable vs a full duo.
- Source: https://wiki.screepspl.us/Combat/.

**T-COMP-7 — Tank+healer duo (front-line soak / defense).** *Robustness: mixed.* Counters: glass-cannon (no-heal) attackers by out-lasting them; anchors a small assault.
- **Trigger:** defending an owned room against a melee/ranged push, or anchoring a small assault needing a damage sponge.
- **Behavior:** heavy-TOUGH ATTACK tank leads (melee counter-damage to anything that hits it off-rampart), healer trails. Tank parks on a rampart when defending (rampart blocks attack-back and RMA, makes the tank invulnerable while it holds). Wins any fight where enemy DPS < tank-survivable + healer throughput.
- **Params:** unboosted tank 8 TOUGH + ~15 ATTACK + ~12 MOVE (`tank_body`); boosted 12 TOUGH (XGHO2) + 15 ATTACK (XUH2O) + 8 MOVE (`boosted_tank_body`); healer 6-8 TOUGH + 13-20 HEAL; tank-on-rampart when defending: drop TOUGH, add ATTACK (T-DEF-7).
- **Metric:** sim — defeats an equal-cost unboosted ranged duo (melee counter + heal out-lasts); survives 1 tower + 1 attacker indefinitely on a rampart.
- Source: https://wiki.screepspl.us/Combat/; https://wiki.screepspl.us/Creep_body_setup_strategies/.

---

## H. Controller warfare

**T-CTRL-1 — Siege-opener CLAIM strike to deny safe mode.** *Robustness: robust.*
- **Trigger:** committing a real siege against a player room whose `controller.safe_mode_available > 0` and `upgrade_blocked == 0`, with a CLAIM body able to reach the controller this tick.
- **Behavior:** land an `attackController` strike the SAME tick the assault opens, BEFORE the breach squad triggers tower fire. The controller tick applies `_upgradeBlocked` before `_safeModeActivated`, so a same-tick strike **beats** a same-tick safe-mode pop and denies activation for 1000 ticks. Repeat one strike per 1000 ticks; each also freezes their upgrade/clock-restore. Use a small CLAIM body — the upgradeBlocked flag is binary, magnitude is irrelevant here.
- **Params:** `claim_parts` = 2 (range 1-5; only the flag matters); `strike_cadence_ticks` = 1000 (engine floor); `co_commit_with_breach` = true; `abort_if_safe_mode_already_active` = true (a live safe mode zeroes our combat 20K ticks — delay instead).
- **Metric:** sim/server — target safe_mode never activates during the siege despite available>0; track ticks-of-upgradeBlocked sustained vs siege length.
- Source: https://docs.screeps.com/api/; https://support.screeps.com/hc/en-us/articles/204585441.

**T-CTRL-2 — Forced-downgrade then breach in the 50K blackout.** *Robustness: robust.*
- **Trigger:** a high-value player room to capture/raze where a full razing is too costly, reachable, sustainable CLAIM bodies, and not our only target (long campaign acceptable).
- **Behavior:** sustained `attackController` strikes (one max-CLAIM body / 1000 ticks) to hold upgradeBlocked (they can't re-upgrade the clock) and ratchet the downgrade clock down. Once the clock crosses the safe-mode lockout threshold (`downgradeTime < DOWNGRADE[level]/2 − 5000`; RCL8 = 95000) they can no longer safe-mode at all; at zero they downgrade, `safeModeAvailable=0`, a 50,000-tick cooldown opens — breach with the main force during the blackout. RCL8→7 also turns off the 3 towers furthest from the controller.
- **Params:** `claim_parts` = 25 (max strike −7500/tick; range 10-25); `lockout_threshold` = DOWNGRADE/2 − 5000; `give_up_if_clock_not_trending_down_after` = 5000 ticks; breach trigger = downgrade_event OR clock < lockout_threshold.
- **Metric:** sim — ticks-to-force-one-downgrade vs the closed-form `clock/(300·CLAIM)·1000`; server — the breach landed inside the 50K blackout with zero safe-mode interruption.
- Source: https://screeps.com/forum/topic/345/.

**T-CTRL-3 — Reserve-denial parts race (out-claim the reserver).** *Robustness: robust.*
- **Trigger:** an enemy is reserving a remote we want to deny/take, with no tower/defender that would kill our CLAIM body.
- **Behavior:** spawn a declaimer with strictly MORE CLAIM parts than the enemy reserver and `attackController` EVERY tick (no cooldown on reserved controllers, −1/CLAIM/tick). Any N>M wins (net `−(N−M+1)/tick`). Once reservation hits 0 the room is claimable/re-reservable; hand off to our reserver/claimer. Do NOT use the owned-controller path (that has the 1000-tick block).
- **Params:** `claim_parts` = enemy_reserver_claim + 2 (min 2, max 25); `strike_every_tick` = true; `handoff_on_reservation_zero` = reserve|claim; `abort_if_defender_or_tower_present` = true.
- **Metric:** sim — ticks to drive a 5000-cap reservation to 0 matches `−(N−M+1)/tick`; server — enemy reservation stays at 0 and our reserver/claimer succeeds.
- Source: https://blog.screeps.com/2018/03/changelog-2018-03-05/; https://wiki.screepspl.us/Reservation/.

**T-CTRL-4 — De-claim derelict-room takeover (the existing SalvageMission path).** *Robustness: robust.*
- **Trigger:** a hostile-OWNED but militarily-dead room with sources we want; controller reachable now or breachable (`ControllerAccess::ReachableNow | Breachable`).
- **Behavior:** spawn exactly ONE declaimer (more is idle — only one strike/1000 ticks lands). It travels with high-cost routing, strikes once, then `DeclaimState::Wait(25)` re-checks until upgradeBlocked clears or the controller goes neutral. The mining-outpost pipeline takes the room over via normal candidate flow once it decays to neutral. If walled in (Sealed), breach dismantlers run first (the M10 corridor) and the declaimer holds.
- **Params:** `declaimer_count` = 1 (hard); body `[Claim,Move]` repeat, min 1 max 4; `wait_ticks` = 25; gate spawn on `ControllerAccess::ReachableNow`; breach-first on Sealed; `features.derelict.declaim` kill-switch (default TRUE).
- **Metric:** server (validated live) — derelict controller reaches neutral and the outpost claims/mines it; declaimer count never exceeds 1; no CLAIM bodies wasted against a walled controller.
- Source: in-repo `salvage.rs` / `declaim.rs` (sources pending).

**T-CTRL-5 — Self-room no-win abort on `upgrade_blocked`.** *Robustness: robust.*
- **Trigger:** a spawnless owned room under sustained player-hostile presence past the anti-flap window, OR `controller.upgrade_blocked() > 0` on our own controller.
- **Behavior:** abandon via `controller.unclaim()` — free, instant, one tick. `upgrade_blocked()>0` on our own room is near-decisive: an enemy `attackController` is freezing our climb to RCL2 (so we can never earn a safe-mode charge) and the clock-restore — a structural "cannot win." Halt child establishment missions, unclaim, tag avoid-cooldown. GCL is preserved. Do NOT reuse DeclaimJob/attackController on our OWN controller (wrong primitive). Exceptions that keep fighting: the room already has a my() spawn, or it's our only colony.
- **Params:** `abort_persistence_ticks` = 20; `establishment_stall_ticks` = 3000; `death_budget` ~2 claimers / ~3 builders; `features.claim.abort_on_contest` (default TRUE).
- **Metric:** sim/server — contested spawnless claims unclaim promptly (no meat-grinder bleed); winnable claims (spawn present / sole colony) are NOT abandoned; net GCL/energy preserved.
- Source: ADR 0017 §7 (sources pending).

**T-CTRL-6 — Spawn-kill the freshly-emerged defender.** *Robustness: mixed.*
- **Trigger:** harassing/sieging a room where the enemy is visibly spawning a defender (`spawn.spawning != null`) and we have an attacker (ranged preferred) within range of the emergence tile.
- **Behavior:** pre-position to focus the emergence tile so the full burst lands the TICK the newborn appears — it spawns full-HP but untargetable while spawning, becomes targetable the tick it lands, and is bare (no active boosts, no ally heal — reactive heal is 1 tick late). Catch it before it moves/heals = a free kill. You CANNOT bottle the spawn with bodies (spawnstomp). Telegraph = 3 × body_parts ticks.
- **Params:** `prefer_ranged` = true; `target_tile` = predicted emergence tile; commit only if our burst ≥ newborn_max_hp + likely ally heal that tick; `disengage_after_kill` = true.
- **Metric:** sim — newborn dies on its emergence tick before acting; (defenders spawn-killed)/(defenders reaching a useful position) maximized.
- Source: https://screeps.com/forum/topic/273/; https://screeps.com/forum/topic/2165/.

---

## I. Defense playbook

**T-DEF-1 — Anchor owned-room defenders to ramparts (stand-on-cover, don't kite).** *Robustness: robust.*
- **Trigger:** a Defend objective is active in an OWNED room with ≥1 rampart and a `hostile_warrants_defender` creep inside/adjacent to a rampart line.
- **Behavior:** replace `kite_toward_objective` for owned-room defenders with a rampart-seek: pick the maintained rampart tile (hits ≥ `MIN_RAMPART_HOLD`) within attack range (1 melee / 3 ranged) of the highest-priority hostile (breacher first), step onto it, attack from cover. A creep on a rampart takes 0 damage until the rampart breaks — the single biggest defensive multiplier in the game. Towers focus the SAME breacher (T-DEF-2) so defender DPS stacks for free. Falls back to kiting only when the room has no usable rampart (early RCL).
- **Params:** `MIN_RAMPART_HOLD` = 10_000 hits (don't anchor on a rampart about to break; sweep against representative siege DPS); rampart pick = nearest maintained within range to the tower focus; re-anchor hysteresis = move only if target leaves range >2 ticks.
- **Metric:** sim — defender survives a 10× boosted-ATTACK siege indefinitely (HP never drops while on a maintained rampart) vs an open-field defender dying in `ceil(hp/1200)` ticks; deaths-per-engagement → 0, structures lost → 0.
- Source: https://docs.screeps.com/defense.html; https://wiki.screepspl.us/Combat/.

**T-DEF-2 — Tower + rampart-defender focus-fire coordination.** *Robustness: robust.*
- **Trigger:** hostiles in an owned room with ≥1 my-tower AND ≥1 anchored defender; `best_target` (tower danger ordering: ATTACK/RANGED/WORK first, then lowest hits) selected.
- **Behavior:** all towers focus the single highest-priority breacher; the anchored defender attacks the SAME target id. Combined net = `600N_at_range + 30·atk_parts (or 10·ra)`, which must exceed the AGGREGATE enemy heal that tick. Share the target id from the tower mission's `best_target` to the squad job so they never split fire.
- **Params:** `focus_target` = tower `best_target` id broadcast into room threat data; defender priority order matches the tower danger order.
- **Metric:** sim — time-to-kill the breacher with coordinated fire vs split fire; breacher killed before it breaks the first inner rampart.
- Source: https://wiki.screepspl.us/Combat/.

**T-DEF-3 — Conserve tower energy vs a confirmed drainer (bounded probe).** *Robustness: mixed.*
- **Trigger:** a hostile's hitpoint sawtooth shows it re-entered the room with MORE hits than it left (`drain_cycles ≥ DRAIN_CONFIRM_CYCLES`), OR `is_likely_tower_drain` fires.
- **Behavior:** stop firing at the confirmed drainer by default; fire at non-drainer hostiles normally. Periodically test with a bounded probe: at most `MAX_PROBE_STRIKES` volleys, spaced `PROBE_COOLDOWN`, pressing to the kill only if a volley drops it ≥ `MIN_PROBE_PROGRESS` (its off-room healer is gone). Already in `tower.rs`; the tactic is to KEEP it and tune the constants. Never let a drainer pull steady energy.
- **Params:** `DRAIN_CONFIRM_CYCLES` = 1; `MAX_PROBE_STRIKES` = 3; `PROBE_COOLDOWN` = 20 (sweep 10-40); `MIN_PROBE_PROGRESS` = 200 (sweep 100-400).
- **Metric:** sim — total tower energy on the drainer over 1000 ticks bounded to ≤ `MAX_PROBE_STRIKES·N·10` = 180; a real attacker whose healer dies is still finished within `PROBE_COOLDOWN` + kill-time once a probe succeeds.
- Source: https://docs.screeps.com/defense.html.

**T-DEF-4 — Priority-kill the controller-attacker before it locks out safe mode.** *Robustness: robust.*
- **Trigger:** a CLAIM-bearing hostile is in an owned room within range of the controller (`hostile_warrants_defender` flags CLAIM).
- **Behavior:** elevate the CLAIM creep to TOP defender/tower target priority — above even an armed breacher — because one successful `attackController` sets `upgradeBlocked=now+1000`, a hard block on `activateSafeMode`. Killing it before the intent lands preserves the safe-mode option. If unreachable, pre-emptively consider safe mode (T-DEF-5) while upgrade_blocked is still 0.
- **Params:** `claim_target_priority` = highest (above ATTACK/RANGED/WORK); `engage_range` = controller.pos within 4 tiles triggers the bump.
- **Metric:** sim — controller `upgrade_blocked` stays 0 (creep killed in transit) in >90% of runs where a defender/tower can reach the approach tile; safe mode remains activatable.
- Source: https://screeps.fandom.com/wiki/Controller; https://docs.screeps.com/defense.html.

**T-DEF-5 — Predictive safe-mode activation when breach is imminent and no defense holds.** *Robustness: mixed.*
- **Trigger:** add a predictive arm to the existing reactive floor (`total_hostile_dps > 300` AND a critical structure < 5000 hits): the innermost rampart/wall protecting a spawn/storage is below `breach_hits` AND `projected_ticks_to_breach < ticks_to_kill_all_breachers`, AND `upgrade_blocked == 0`, a charge is available, and not on cooldown.
- **Behavior:** activate safe mode the tick before the last protective rampart breaks rather than after a spawn is already chewed. `projected_ticks_to_breach = rampart_hits / breach_dps`; `defense_kill_time` from `attack_parts_to_kill` / tower net damage; if defense can't hold, activate. Never activate while `upgrade_blocked > 0` (wasted attempt) — that's why T-DEF-4 prioritizes the CLAIM creep.
- **Params:** `breach_hits` floor = 10_000 (start watching); `predictive_margin` = activate when `projected_ticks_to_breach < defense_kill_time · 1.0` (sweep 0.8-1.5); keep `SAFE_MODE_DPS_THRESHOLD` = 300 and `CRITICAL_STRUCTURE_MIN_HITS` = 5000 as the reactive floor.
- **Metric:** sim — with the predictive trigger, safe mode fires while the spawn is still full (0 structures lost); false-positive activations (burned when defense would have held) → 0 across a sweep of attacker sizes.
- Source: https://support.screeps.com/hc/en-us/articles/212239225; https://docs.screeps.com/defense.html.

**T-DEF-6 — Remote/reserved-room defense: cheap mobile RANGED interceptor.** *Robustness: robust.*
- **Trigger:** a `hostile_warrants_defender` creep in a RESERVED/outpost remote (no ramparts to anchor on). Owned rooms use T-DEF-1; remotes use this.
- **Behavior:** spawn a cheap kiting RANGED+MOVE interceptor (no TOUGH/HEAL vs unboosted invaders) and kite at range 3, RMA only when ≥2 hostiles cluster. Against NPC invaders this suffices: invaders are unboosted, can't move between rooms, can't follow into a controlled neighbor. Keep `is_room_safe` keyed on `militarily_active` (the post-`4fae295` fix) so inert husks don't false-trigger.
- **Params:** interceptor body = RA:MOVE 1:1 to budget (5RA+5MOVE ~1250e baseline); `kite_range` = 3; mass-attack when hostiles_within_3 ≥ 2; `MAX_DEFENSE_SOURCE_DISTANCE` = 10.
- **Metric:** sim — a standard melee invader (10 ATTACK + 10 MOVE) vs a 5RA+5MOVE interceptor kiting at range 3: interceptor takes 0 damage, kills in `ceil(invader_hp/250)`; remote miners resume within the interceptor's ttl; interceptor deaths → 0 vs unboosted invaders.
- Source: https://docs.screeps.com/invaders.html; https://wiki.screepspl.us/Invader/.

**T-DEF-7 — Drop HEAL parts on defenders that fight only from a rampart.** *Robustness: robust.*
- **Trigger:** `sized_defender_body` is being built for an owned-room defender that will anchor on a rampart (T-DEF-1) AND `incoming_dps` assumes open-field exposure.
- **Behavior:** when the defender fights exclusively from rampart cover, pass `incoming_dps = 0` to `defender_heal_parts_for_dps` (it already returns 0 at 0 DPS) so the body spends all parts on offense (RANGED/ATTACK) — the rampart provides survivability, HEAL is dead weight on-cover. Keep HEAL sizing for the mobile remote interceptor and for defenders that must leave cover.
- **Params:** `anchored_incoming_dps_override` = 0 when the target is reachable from a maintained rampart; else use real `threat.estimated_dps`. Guard: if the only reachable rampart is below `MIN_RAMPART_HOLD`, revert to a HEAL-sized mobile body.
- **Metric:** sim — a budget-B anchored defender sized without HEAL has strictly more offense and identical survival (0 damage on the rampart), so kills the breacher faster; defender survival unchanged.
- Source: https://wiki.screepspl.us/Combat/.

**T-DEF-8 — Pre-position a standing defender at high-threat ramparts.** *Robustness: mixed.*
- **Trigger:** a room is a repeated/expected attack target (recent history, border room, or threat data shows scouting), OR a Defend squad just entered Rallying (the rally timeout is latency the room pays while structures take damage).
- **Behavior:** keep a single cheap standing defender pre-anchored on the most-exposed maintained rampart (the rampart nearest the likely breach corridor, reusing `breach_path_blockers`). It costs upkeep but eliminates spawn+travel+rally latency at the moment of attack. On escalation (Solo→Duo→Quad) it becomes the squad seed so the first responder is already on-station.
- **Params:** pre-position only when `threat_recency < THREAT_MEMORY` (sweep 1000-5000) to avoid quiet-room upkeep; anchor tile = `breach_path_blockers` highest-betweenness rampart; body = small RANGED+TOUGH on cover.
- **Metric:** sim — damage taken in ticks 0-50 of a surprise siege → near 0 with pre-position vs without; standing-defender energy/tick upkeep < tower repair savings.
- Source: https://www.jonwinsley.com/screeps/2021/08/17/screeps-patrolling-perimeter/.

---

## J. NPC playbooks

**T-NPC-1 — Reserver-core ATTACK snipe (lvl-0 invader core).** *Robustness: robust.*
- **Trigger:** `invader_core_attack_score` returns Some for a level-0 core in a room we mine/reserve, past the invulnerable deploy window (`EFFECT_INVULNERABILITY` cleared).
- **Behavior:** one `core_attacker` (10-20 ATTACK + matching MOVE), path to range 1, ATTACK every tick. No healer, no formation — a bare reserver core has no rampart and no hit-back. **dismantle is engine-rejected on cores — ATTACK only.** On death it stops re-reserving and the outpost gate clears.
- **Params:** `attack_parts` = 10 (300 DPS, ttk = 100000/300 = 334t; sweep 10-20; 20 = 167t); healer = none; `deploy_condition` = Immediate; re-recon if still invulnerable.
- **Metric:** sim — core dead within `100000/(parts·30)` ticks of arrival; reservation reverts within ~50 ticks; miners resume.
- Source: https://docs.screeps.com/invaders.html; https://docs.screeps.com/api/#StructureInvaderCore.

**T-NPC-2 — SK range-3 kite-kill.** *Robustness: robust.*
- **Trigger:** `AttackReason::SourceKeeper` for an SK room; a keeper is alive OR a lair respawn (300t) is near zero.
- **Behavior:** `duo_sk_farmer`: SK ranged attacker (10RA+10MOVE+1HEAL at range 3) + SK healer (10HEAL). Kiter does move(maintain range 3) + rangedAttack(keeper) every tick; NEVER enter range 1 (keeper melee 300 + ranged 100 if caught). Healer trails adjacent covering the 100 ranged DPS the keeper lands. After a keeper dies, pre-position at the NEXT lair before its 300t respawn to out-cycle respawns across all 3 lairs.
- **Params:** `ra_parts` = 10 (100 DPS); `heal_parts` = 10 (120/tick > 100 incoming); `kite_range` = 3 (hard); `retreat_threshold` = 0.5; `lair_prestage_lead` = keeper_ttk + travel.
- **Metric:** sim — keeper (5000 HP) dead in ~50 ticks with zero melee intake; duo survives a full 3-lair rotation.
- Source: https://docs.screeps.com/source.html; https://wiki.screepspl.us/Combat/.

**T-NPC-3 — Power-bank decay-window go/no-go gate.** *Robustness: robust.*
- **Trigger:** a power bank is sighted; evaluate BEFORE committing a duo.
- **Behavior:** launch ONLY if `ticks_to_decay > power_bank_min_ticks_needed(min_distance) = 3334 (kill) + dist·50 (travel) + 270 (serial duo spawn) + 200 (margin)`, AND under the concurrency cap, AND power ≥ a min-ROI floor. A fresh 5000-decay bank is farmable only within ~24 tiles; a half-decayed bank is a no-go at any distance. This is the existing `war.rs` gate — keep kill_time IN the window (the pre-D5 bug green-lit unfinishable banks).
- **Params:** `BANK_HITS` = 2_000_000; `DUO_DPS` = 600 (20-ATTACK cap); `DUO_SPAWN_TICKS` = 270; `MARGIN_TICKS` = 200; `min_roi_power` = 2000 (sweep 1000-3000); `max_concurrent` = min(2, room_count).
- **Metric:** sim — zero abandoned half-killed banks; launched-bank completion rate ~100% across dist 0-30; the gate matches actual completion.
- Source: https://docs.screeps.com/power.html; https://wiki.screepspl.us/Power/.

**T-NPC-4 — Hit-back-matched power-bank duo.** *Robustness: robust.*
- **Trigger:** the gate (T-NPC-3) passes; spawn the assault duo.
- **Behavior:** `power_bank_attacker` = **20 ATTACK** + 20 MOVE (CAP at 20: 50% hit-back of 600 reflects 300/tick = the 25-HEAL healer's 300/tick; 25 ATTACK would reflect 375 and out-damage its own healer). Healer = 25 HEAL + 25 MOVE. Two-phase netting cancels the 300 reflect with the 300 heal each tick. ttk = 2,000,000/600 = 3334 ticks.
- **Params:** `attack_parts` = 20 (hard cap, hit-back-matched); `heal_parts` = 25; boosted variant 20 XUH2O ATTACK (2400 DPS, ttk 834t) + 25 XLHO2 HEAL (1200/tick vs 1200 reflect) for distant/short-window banks.
- **Metric:** sim — attacker HP flat (intake 300 = heal 300) for the full kill; attacker min-HP never trends down.
- Source: https://wiki.screepspl.us/Power/; https://docs.screeps.com/power.html.

**T-NPC-5 — Hauler-arrival-timed power drop.** *Robustness: mixed.*
- **Trigger:** the attacker projects bank HP → 0 within (hauler spawn + travel).
- **Behavior:** spawn `power_bank_haulers` (total CARRY = `ceil(power/50)`; 5000 power → 100 CARRY = two 25-CARRY/25-MOVE haulers) timed to ARRIVE within a few ticks of the kill. On death the full power drops (then decays ~5/tick, or sits in a ruin); haulers scoop and return. Late haulers cede the drop to rivals — timing is the whole game.
- **Params:** `carry_total` = ceil(power/50); `hauler_count` = ceil(carry_total/25); spawn at `T_kill − (travel + buffer)`; `buffer` = 10 (sweep 0-30).
- **Metric:** sim — >95% of dropped power recovered (not decayed/stolen); haulers idle <50 ticks at the bank.
- Source: https://docs.screeps.com/power.html.

**T-NPC-6 — Stronghold core-snipe (one-rampart breach).** *Robustness: mixed.*
- **Trigger:** `AttackReason::InvaderCore{level≥1}` affordable per `invader_core_attack_score` (energy gates L1@30K, L3@100K, L5@200K), stronghold deployed (not invulnerable).
- **Behavior:** pick the corner rampart with the LOWEST aggregate tower DPS on its approach tile (range >5 from as many towers as possible — exploit the 600→150 falloff). Out-heal that tower DPS with a cross-healing formation, breach THAT ONE rampart (`STRONGHOLD_RAMPART_HITS[level]` = 100K/200K/500K/1M/2M), then focus the 100K core — its death collapses the whole bunker (all towers go inert). Never breach all ramparts; reuse `breach_path_blockers` to pick the rampart.
- **Params:** `approach_tile` = argmin(Σ tower dmg); breach = dismantle (`W·50/tick`, ×4 boosted) vs snipe-through; L1-2 unboosted duo, L3-4 boosted quad, L5 boosted quad or nuke.
- **Metric:** sim — one rampart down then core dead with the force surviving (intake at the chosen tile < pooled heal throughout); per bunker template, measure best-corner tower DPS and breach+core ttk.
- Source: https://screeps.com/forum/topic/2826/; https://docs.screeps.com/api/#StructureInvaderCore.

**T-NPC-7 — Per-level stronghold force ladder.** *Robustness: mixed.*
- **Trigger:** stronghold level resolved during recon; select force plan.
- **Behavior:** L1 (1 tower=600, 100K rampart): unboosted duo out-heals 600 (50 HEAL pooled) while dismantling. L2 (2=1200, 200K): unboosted-to-light-boosted duo/trio. L3 (3=1800, 500K): boosted quad cross-heal (4·25 XLHO2 = 4800 > 1800) + 20-WORK boosted dismantler (4000/tick → 125t). L4 (4=2400, 1M): boosted quad, longer breach. L5 (6=3600, 2M, auto-fortifies vs nukes every 10t): boosted quad cross-heal + heavy dismantle, OR stacked nukes on the core. Gate each level behind the stored-energy check.
- **Params:** L1 duo_drain/duo_attack_heal; L2 duo+; L3-4 quad_ranged boosted; L5 boosted quad + dismantler or nuke; `pooled_heal_floor` = N·600 · 1.3 margin.
- **Metric:** sim — each level cleared with the cheapest comp keeping `pooled_heal > tower_DPS` at the breach tile; no force wipes.
- Source: https://screeps.com/forum/topic/2826/; https://screeps.com/forum/topic/2794/.

**T-NPC-8 — Stronghold safe-window check (deploy + decay).** *Robustness: robust.*
- **Trigger:** before committing a stronghold assault.
- **Behavior:** skip if still in the INVULNERABLE deploy stage (core un-killable). Weigh `STRONGHOLD_DECAY_TICKS` (75000 ±10%): a near-end-of-life stronghold collapses on its own; only assault if loot (coreAmounts up to 3M at L5) justifies the boosted-force cost and enough lifetime remains to breach+kill+haul. Same discipline as power banks.
- **Params:** deploy gate = require `!EFFECT_INVULNERABILITY` on core; `min_remaining_lifetime` ≈ breach_ttk + core_ttk + haul + 500; loot floor by level via coreAmounts (1K/16K/60K/400K/3M).
- **Metric:** sim — no assaults against invulnerable-deploy or about-to-collapse strongholds; loot-per-energy positive on every committed assault.
- Source: https://docs.screeps.com/api/#StructureInvaderCore; https://screeps.com/forum/topic/2794/.

**T-NPC-9 — Invader-creep remote cleanup (non-core).** *Robustness: robust.*
- **Trigger:** `AttackReason::InvaderCreeps` — hostile NPC invader creeps in a remote (no core present).
- **Behavior:** deploy `solo_ranged` DefendRoom; ranged-kite the invaders (weaker than keepers, no rampart). Short economy patience (100t) since invaders despawn/finish quickly. Match offense to detected invader DPS via the `sized_defender` path; escalate to a duo if `detected_heal > 0`.
- **Params:** composition = solo_ranged (escalate to duo if detected_heal>0); `patience` = 100; size offense to detected enemy dps/heal.
- **Metric:** sim — invaders cleared before they kill the outpost miner; miner downtime <100 ticks.
- Source: https://docs.screeps.com/invaders.html; https://wiki.screepspl.us/index.php/Invader.

---

## Anti-overfitting

The catalog is deliberately built on **fixed engine constants** so it generalizes across a dynamic MMO opponent set rather than beating one defender.

**Robust (engine-math, opponent-agnostic — trust these as written):** all of section A (the kill inequality), B (range/fatigue kiting), the core tower handling (T-TOWER-1/2/4/5/6), breach selection and dismantle math (T-BREACH-1/2/3/5/7/8), heal adjacency (T-HEAL-1/2), the winnability/escalate-abandon gates (T-ENGAGE-1/2/3), the deterministic body math in every T-COMP entry, controller-warfare mechanics (T-CTRL-1/2/3/4/5), rampart-anchoring + tower coordination + CLAIM-priority defense (T-DEF-1/2/4/6/7), and the NPC playbooks where the target AI is fixed by the processor (T-NPC-1/2/3/4/6/8/9). These derive from verified constants — they do not encode an assumption about how any opponent behaves.

**Brittle / mixed (encode an opponent *behavioral read* — keep revocable and sweep against a roster):**
- **T-TOWER-7 (drain-then-attrition)** and **T-TOWER-5/T-DEF-3 (hold-fire)** are a pair of mirrored bets on the *enemy's* tower discipline: if their towers naively fire on a confirmed drain we win the energy exchange; if they hold fire the pure-drain economic attrition stalls and we must escalate to a breach. Keep the drain commitment revocable (abort when net ≤0).
- **T-FOCUS-3 (kill-healer-first)** crossover is comp-dependent (tank+healer vs mass-ranged vs boosted-quad).
- **T-CTRL-6 (spawn-kill)**, **T-BREACH-4 (breach-pour timing)**, **T-DEF-5 (predictive safe-mode)**, **T-DEF-8 (pre-position)**, **T-NPC-5/7 (hauler timing / stronghold ladder)**, **T-POS-6 (mirror-Y rotation)**, **T-POS-8 (cornered commit)**, **T-HEAL-3 (eHP estimation)** all depend on timing/estimation that must be validated against real bodies and behavior, not assumed.

**Mitigation (per ADR 0006 / 0015):** (1) **seed diversity** — every engagement gate is an N=9 paired-seed diff (terrain perturbation, body jitter, start-offset) against a stored (scenario, seed, SHA) baseline; the sim seeds are perfectly reproducible, so the distributional gate is buildable for combat. (2) **opponent roster** — scripted comps (tank+healer, mass-ranged, boosted quad, drain) + self-play (the bot's own code through the tactical seam, no tactics fork to overfit) + recorded-from-MMO opponents. (3) **MMO canary** — the cohesion/orphan/kill-efficiency metrics the sim optimizes are also emitted to seg-57 on the live bot; if the sim says "fixed" and MMO says "still scattering," the parity budget tightens and the missing mechanic is found. No opponent-specific constant appears anywhere in the catalog; threat is measured at runtime (`threatmap.rs`, conservative ×4-boost assumption), force is sized from that measurement (`damage.rs` + `sized_*_body`), and the give-up backoff (`UnwinnableTarget`, T-TOWER-4) degrades gracefully against an unexpectedly strong defender instead of feeding a death-spiral.

---

## Tunable parameters (the knobs to sweep)

These are the experimental **shell** (ADR 0015): tuned by sim/server iteration, never unit-tested. Thresholds live in the F19 one-config-file per the flake policy.

| Param | Tactic(s) | Default-guess | Sweep range | Notes |
|---|---|---|---|---|
| `kill_window_ticks` | T-FOCUS-1, T-ENGAGE-1/2 | 25 | 15-40 | reuse `KILL_WINDOW_TICKS` (`damage.rs`); longer = more patient kills |
| `heal_relief_weight` (w) | T-FOCUS-1 | 0.05 tick/HP | 0.0-0.15 | how much heal-relief floats a healer up the target ranking |
| `range_penalty` | T-FOCUS-1 | 10 /tile | 0-20 | Overmind value; biases focus toward closer targets |
| `shooter_order` | T-FOCUS-2 | by_dps_desc | {by_dps_desc, by_range_asc} | predicted-hits chaining order |
| `over_book_margin` | T-FOCUS-2 | Hb·1 tick | +0%..+5% | overkill safety buffer |
| `healer_relief_threshold` | T-FOCUS-3 | 0.4·softest_H_eff | 0.2-0.8 | when kill-healer-first wins over kill-softest |
| `rma_vs_single_threshold` | T-FOCUS-4, T-POS-4 | ≥2 hostiles r≤1 OR ≥4 r=2 | cluster 2-4 | falloff table itself is engine-fixed |
| `drain_confirm_cycles` | T-FOCUS-5, T-DEF-3 | 1 | 1-3 | reuse tower `DRAIN_CONFIRM_CYCLES` |
| `probe_budget_ticks` | T-FOCUS-5 | 3 | 2-5 | offensive edge-drain disengage |
| `hold_range` | T-POS-1 | 3 | 2-3 | range-3 = max RANGED reach, 0 melee return |
| `flee_trigger_range` | T-POS-2 | 2 | 2 | break-contact before melee connects |
| `melee_avoid_radius` | T-POS-2 | 3 | 3-4 | Overmind +1 (ATTACK) / +3 (RANGED) cost ring |
| `retreat_lookahead` | T-POS-2 | 2 tiles | 1-3 | avoid backing into a dead-end |
| `plain_move_ratio` / `road` / `boosted_t3` | T-POS-3 | 1.0 / 0.5 / 0.25 | engine-derived | MOVE-parity; assert in kiter builders |
| `part_order` | T-POS-3 | [TOUGH, combat, HEAL, MOVE] | fixed | MOVE-back keeps speed under attrition |
| `exit_buffer` | T-POS-5 | 2 tiles | 2-3 | vs boosted-MOVE chasers may need 3 |
| `rotation_cooldown` | T-POS-6 | 2 ticks | 2-4 | anti-thrash on armour rotation |
| `operating_range` (drain) | T-TOWER-1, T-POS-7 | 20 | 18-23 | lower = more drain, more incoming |
| `push_when_target_energy_below` | T-TOWER-1 | 200 | 0-500 | handoff to siege squad |
| `retreat_hp_fraction` (drain) | T-TOWER-1 | 0.3 | 0.2-0.5 | |
| `min_dismantle_range` | T-TOWER-2 | 6 | 6-12 | operate above tower-optimal range 5 |
| `accept_extra_path_ticks_for_range` | T-TOWER-2 | 8 | 0-12 | trade approach length for lower DPS |
| `healers_per_2_towers` | T-TOWER-3 | ceil(N/2) | per-healer HEAL 20-33 | in-bunker heal-train sizing |
| `tough_parts` (dismantler) | T-TOWER-3, T-COMP-5 | 8-12 | 4-12 | one-time eHP buffer, not a rate |
| `no_go_margin` | T-TOWER-4 | 1.0 | 1.0-1.3 | buffer vs tower re-focus |
| `reconsider_after_ticks` | T-TOWER-4 | 3000 | 2000-20000 | `UnwinnableTarget` backoff window |
| `reserve_energy_for_assault_fraction` | T-TOWER-5 | 0.5 | 0.3-0.7 | defender keeps energy for the real wave |
| `PROBE_COOLDOWN` | T-DEF-3 | 20 | 10-40 | bounded drainer probe spacing |
| `MIN_PROBE_PROGRESS` | T-DEF-3 | 200 hits | 100-400 | press-to-kill threshold |
| `MAX_PROBE_STRIKES` | T-DEF-3 | 3 | 2-4 | caps wasted energy on a drainer |
| `rotate_at_remaining_life` (drain) | T-TOWER-7 | 200 | 100-300 | rotate fresh drain before death |
| `safety_margin` m (breach) | T-BREACH-3 | 2.0 | 1.3-3.0 | net-repair commit margin |
| `max_structure_hits` horizon | T-BREACH-1/8 | 2_000_000 | 1e6-3e7 | salvage low, war override high |
| `hold_open_ticks` | T-BREACH-4 | member_count+2 | n..n+4 | keep gap open while squad pours |
| `stack_size` (dismantlers) | T-BREACH-7 | derived | by H,R,b_mult | continuity stagger ~50t lead |
| `preheal_lookahead` | T-HEAL-1 | 1 tick | EMA 1-3 | predictive pre-heal horizon |
| `rotate_trigger` (heal capacity) | T-HEAL-1 | 0.8·heal_cap | 0.6-1.0 | rotate before HP drops |
| `heal_range` | T-HEAL-2 | 1 (strict) | fixed | rangedHeal is ⅓ throughput |
| `tough_eHP_per_part` | T-HEAL-3 | 333 (T3) | validate | mis-estimate biases abandon gate |
| `retreat_threshold` | T-ENGAGE-3, T-COMP-1 | enemy-DPS-aware (~0.4 avg) | 0.3-0.6 | not a flat 0.3 |
| `reengage_band` | T-ENGAGE-3 | retreat+0.2 | +0.1..+0.3 | coupled hysteresis |
| `hard_floor` (per-member HP) | T-ENGAGE-3 | 0.2 | 0.15-0.3 | individual-critical requests retreat |
| `abandon_hysteresis_ticks` | T-ENGAGE-2 | 5 | 3-10 | anti-flap on abandon |
| `max_waves` | T-ENGAGE-1/4 | 3 | 2-5 | `AttackOperation` cap |
| `renew_ttl_threshold` / `renew_min_room_energy` | T-ENGAGE-4 | 1200 / 10000 | — | renew only small/cheap survivors |
| quad per-member TOUGH / RA / HEAL / MOVE | T-COMP-1 | 6 / 16 / 14 / 14 | TOUGH 4-8, RA 14-18, HEAL 12-16 | uniform brick vs current 2+2 split |
| `claim_parts` (siege opener) | T-CTRL-1 | 2 | 1-5 | only the upgradeBlocked flag matters |
| `claim_parts` (forced downgrade) | T-CTRL-2 | 25 | 10-25 | max strike −7500/tick |
| `give_up_if_clock_not_trending_after` | T-CTRL-2 | 5000 ticks | 3000-8000 | they out-upgrade → abandon |
| `claim_parts` (reserve denial) | T-CTRL-3 | enemy+2 | 2-25 | any N>M wins the parts race |
| `declaimer_count` | T-CTRL-4 | 1 | hard | only one strike/1000t lands |
| `wait_ticks` (declaim) | T-CTRL-4 | 25 | 15-50 | re-check cadence |
| `abort_persistence_ticks` / `establishment_stall_ticks` | T-CTRL-5 | 20 / 3000 | 10-50 / 2000-4000 | anti-flap self-room abort |
| `MIN_RAMPART_HOLD` | T-DEF-1/7 | 10_000 hits | sweep vs siege DPS | don't anchor on a breaking rampart |
| `claim_target_priority` | T-DEF-4 | highest | fixed | above armed breacher |
| `breach_hits` (safe-mode watch) | T-DEF-5 | 10_000 | — | predictive arm start |
| `predictive_margin` | T-DEF-5 | 1.0 | 0.8-1.5 | activate before defense fails |
| `SAFE_MODE_DPS_THRESHOLD` / `CRITICAL_STRUCTURE_MIN_HITS` | T-DEF-5 | 300 / 5000 | fixed floor | reactive floor kept |
| interceptor RA:MOVE | T-DEF-6 | 1:1 (5RA+5MOVE) | budget-scaled | unboosted invaders |
| `THREAT_MEMORY` (pre-position) | T-DEF-8 | 1000-5000 ticks | sweep | avoid quiet-room upkeep |
| `attack_parts` (core) | T-NPC-1 | 10 | 10-20 | 334t vs 167t kill |
| `kite_range` (SK) / `lair_prestage_lead` | T-NPC-2 | 3 / keeper_ttk+travel | hard / — | never enter range 1 |
| `min_roi_power` / `max_concurrent` (bank) | T-NPC-3 | 2000 / min(2,rooms) | 1000-3000 | decay-window gate |
| `attack_parts` / `heal_parts` (bank duo) | T-NPC-4 | 20 / 25 | hard cap / — | hit-back-matched (600→300) |
| `hauler buffer` (bank) | T-NPC-5 | 10 ticks | 0-30 | arrive within ~5t of kill |
| `approach_tile` (stronghold) | T-NPC-6 | argmin Σ tower dmg | — | exploit 600→150 falloff |
| `pooled_heal_floor` (stronghold) | T-NPC-7 | N·600·1.3 | margin 1.1-1.5 | per-level force ladder |
| `min_remaining_lifetime` (stronghold) | T-NPC-8 | breach+core+haul+500 | — | vs 75000±10% decay |
| `patience` (invader cleanup) | T-NPC-9 | 100 ticks | 50-150 | invaders despawn fast |

---

## Experiment register (sim per-change, server at acceptance)

Ordered so foundations (1v1 arithmetic, focus-fire, kiting) validate before composites (drain, breach, quad self-play, defense). **Sim** = the deterministic combat micro-sim driving the bot's own decision code (ADR 0006 Part B); per-change, hard-exact conformance vectors + N=9 paired-seed engagement diffs. **Server** = the Docker private-server acceptance gate (ADR 0006 Part A), nightly N-seed confirmation. Gates use the seg-57 metrics emitted in both sim and live (the MMO canary).

1. **EXP-FOUND-1 — Kill inequality conformance.** *Hypothesis:* `attack_parts_to_kill` / `should_towers_fire` correctly predict kill-or-not under two-phase netting. *Scenario (sim):* 1 attacker vs 1 target with parameterized self-heal; sweep D around Hb. *Metric:* predicted-kill label vs actual death; netting order (damage-then-heal) matches the engine. *Gate:* hard-exact conformance, 100% agreement; foundation for all of section A.

2. **EXP-FOUND-2 — Per-part degradation & TOUGH eHP.** *Hypothesis:* output degrades front-to-back and a boosted XGHO2 part = ~333 eHP consumed front-first. *Scenario (sim):* fixed-DPS fire on a TOUGH-front body; record DPS/heal output and survival per tick. *Metric:* eHP-consumed curve and output-decay curve vs the model. *Gate:* within 5% of the closed-form; validates T-HEAL-3 / T-TOWER-3 TOUGH-buffer assumptions before any abandon decision trusts them.

3. **EXP-KITE-1 — MOVE-parity kiting (1v1).** *Hypothesis:* a range-3 kiter at MOVE-parity takes 0 melee damage from an equal-speed melee chaser on plain. *Scenario (sim):* 7RA+7MOVE kiter vs 10ATTACK+10MOVE chaser, plain; then under-parity and swamp variants. *Metric:* melee hits taken; our-HP-lost/enemy-HP-lost ratio. *Gate:* 0 melee hits at parity; >0 only when parity breaks. Validates T-POS-1/2/3.

4. **EXP-KITE-2 — Exit-tile & cornered discipline.** *Hypothesis:* T-POS-5/8 prevent involuntary room transitions and convert a corner into the higher-EV commit-or-eject. *Scenario (sim):* kiter pressured toward an edge; cornered against terrain. *Metric:* unplanned room exits; corner survival/kill rate vs the die-in-place baseline. *Gate:* 0 unplanned exits; corner outcome ≥ baseline.

5. **EXP-FOCUS-1 — Net-heal-gated focus + chaining.** *Hypothesis:* T-FOCUS-1/2 reduce ticks-to-clear and overkill vs the current "healer-first then min-hits." *Scenario (sim):* Duo/Quad vs tank+healer and vs a 3-creep line. *Metric:* ticks-to-clear, overkill ratio, distinct kills/25t. *Gate:* clear time ≤ both fixed policies; overkill → ~0. Then *Server:* kills/energy in scripted duels rises.

6. **EXP-FOCUS-2 — Conditional kill-healer-first crossover.** *Hypothesis:* T-FOCUS-3 (kill-healer-first only when the healer is killable) ≤ min(always-healer-first, always-softest) across comps. *Scenario (sim):* tank+healer, mass-ranged, boosted-quad; sweep `healer_relief_threshold`. *Metric:* total-clear time per comp; fixation ticks on unkillable rampart-healers. *Gate:* never worse than either fixed policy; 0 fixation >3 ticks at net≤0. Picks the default `w` / threshold.

7. **EXP-TOWER-1 — Edge drain sustain + NO-GO gate.** *Hypothesis:* `drain_body_for_tower_dps` sustains at the edge for the N it claims, and T-TOWER-4 flags exactly the un-drainable rooms. *Scenario (sim):* drain pair vs N=1..6 towers at the edge, boosted and unboosted; then siege squads vs NO-GO-flagged rooms. *Metric:* drain net HP non-decreasing; squads-spawned-then-wiped against NO-GO rooms. *Gate:* sustain matches the heal math; 0 wipes on NO-GO rooms; false-NO-GO rate tracked. *Server:* target tower energy → 0.

8. **EXP-TOWER-2 — Hold-fire vs drain (defender side).** *Hypothesis:* T-TOWER-5/T-DEF-3 keep tower energy above reserve while a confirmed drain sits at the edge. *Scenario (sim):* a 20-HEAL edge drainer vs our 6-tower room. *Metric:* tower energy spent on the drainer over 1000 ticks (bounded to ≤180 via the probe); reserve preserved for the follow-on wave. *Gate:* energy bound met; a real attacker whose healer dies is still finished once a probe succeeds.

9. **EXP-BREACH-1 — Min-cut corridor + net-repair sizing.** *Hypothesis:* `breach_path_blockers` picks the min-HP corridor and T-BREACH-3 commits only when net DPS beats repair. *Scenario (sim):* walled room with towers + repairers; vary W and boost. *Metric:* total HP cleared = min over gaps; breach completes within one dismantler life when `net·1500 ≥ H`; aborts (no wasted creep) when net≤0. *Gate:* completion/abort matches the sign of net; first-blocker targeting doesn't raise total HP cleared.

10. **EXP-BREACH-2 — Breach-then-pour + drain handoff.** *Hypothesis:* T-BREACH-4/6 + stacking (T-BREACH-7) get the squad inside without gap deaths after towers are drained. *Scenario (sim):* drain pair drains towers, then siege quad breaches one rampart and pours single-file. *Metric:* members inside with 0 gap deaths; breach doesn't reseal before the last creep passes. *Gate:* 0 gap deaths; *Server:* breach force reaches spawns.

11. **EXP-COMP-1 — Quad composition self-play (uniform brick vs 2+2 split).** *Hypothesis:* the uniform RA+HEAL brick quad (T-COMP-1) kills a 25-HEAL boosted defender faster AND survives a 6-tower nest longer than the current 2-ranged/2-healer split. *Scenario (sim, self-play):* both quads vs a boosted defender stack and vs a 6-tower nest; sweep per-member TOUGH 4/6/8. *Metric:* time-to-kill, member survival, pool-heal vs net tower DPS. *Gate:* uniform ≥ split on both; pick the default TOUGH count. Resolves the open question on default quad comp.

12. **EXP-DEF-1 — Rampart-anchored defender + tower coordination.** *Hypothesis:* T-DEF-1/2/7 make an anchored defender survive a heavy siege indefinitely and stack DPS with towers. *Scenario (sim):* a 10× boosted-ATTACK siege vs a rampart-anchored defender + 6 towers, focus-firing the breacher. *Metric:* defender deaths, structures lost, breacher time-to-kill (coordinated vs split). *Gate:* 0 defender deaths while ramparts hold; 0 structures lost; coordinated kill < split. Then *Server:* the seg-57 canary confirms no scatter/deaths live.

13. **EXP-DEF-2 — CLAIM-priority + predictive safe-mode.** *Hypothesis:* T-DEF-4 keeps `upgrade_blocked` at 0 and T-DEF-5 fires safe mode before the last protective rampart breaks. *Scenario (sim):* a CLAIM creep walking to the controller; a dismantler siege approaching a spawn. *Metric:* fraction of runs where upgrade_blocked stays 0; structures lost; false-positive safe-mode activations across attacker sizes. *Gate:* >90% upgrade_blocked-stays-0 when reachable; 0 structures lost on the dismantler siege; 0 false positives.

14. **EXP-ENGAGE-1 — Escalate/abandon + coupled hysteresis.** *Hypothesis:* T-ENGAGE-2/3 escalate to exactly the count crossing D* on winnable comps and abandon provably-unwinnable heal walls without yo-yo. *Scenario (sim):* sweep enemy heal from winnable to unwinnable; vary squad count. *Metric:* over-spawn count into unwinnable walls; retreat/re-engage oscillations per engagement. *Gate:* 0 over-spawn into unwinnable; oscillations → near 0; correct minimal escalation count.

15. **EXP-NPC-1 — NPC playbook conformance.** *Hypothesis:* core/SK/power-bank/stronghold kill-times and gates match the engine constants. *Scenario (sim):* lvl-0 core (T-NPC-1), 3-source SK room (T-NPC-2), 2M power bank at varying decay/distance (T-NPC-3/4/5), bunker1-5 templates (T-NPC-6/7/8). *Metric:* kill-time vs closed form; bank completion rate vs the decay-window gate; stronghold survival per the force ladder. *Gate:* kill-times within 5%; 100% bank completion when gated; no force wipes on the mapped comps. *Server:* SK farming + bank farming run 0-death over a full cycle.

16. **EXP-CTRL-1 — Controller-warfare timing.** *Hypothesis:* the same-tick CLAIM-strike beats safe-mode (T-CTRL-1) and the reserve-denial parts race matches `−(N−M+1)/tick` (T-CTRL-3). *Scenario (server, needs the controller tick):* open a siege with a CLAIM strike on a room with safe_mode_available; out-claim an enemy reserver. *Metric:* safe_mode activation prevented; reservation decay rate. *Gate:* safe mode never activates during the opener; reservation reaches 0 on schedule. (Controller-tick mechanics are best confirmed on the server; sim conformance covers the arithmetic.)

17. **EXP-PARITY — Sim-to-real divergence budget (nightly).** *Hypothesis:* the sim's per-tick state (positions/hits/deaths/intent stream) stays within the divergence budget vs the server on the named combat scenarios. *Scenario:* run every gated scenario above through both sim and Docker server. *Metric:* per-tick divergence vs the tracked budget. *Gate:* within budget; sim scores trusted only within it. This is the anti-overfit backstop — if "sim says fixed, MMO says scattering," the budget tightens and the missing mechanic is found (the seg-57 canary).

**Register status (2026-06-18):** the only experiment run so far is **EXP-SQUAD-KITE-1** — *validated (self-play)*: a ranged duo + healer vs a high-HP melee keeper, asserting focus-fire + `max_pairwise ≤ 4` cohesion + survival (the G3-tail validation; lives in `screeps-combat-agent`, not originally in this 1–17 list). Items 1–17 above remain **pending** — they need the H4 harness (scenarios/opponents/self-play runner) before the register can run; tracked as EXP-REGISTER in the [master plan doc](../plans/combat-overhaul-plan.md) §5.

---

## Open behavioral questions (operator)

**Resolved (operator, 2026-06-17):**
- **Quad composition →** *let EXP-COMP-1 decide.* No upfront default — build both the uniform RA+HEAL brick (T-COMP-1) and the 2-ranged+2-healer split (T-COMP-2), run the self-play experiment, adopt the winner. (So G3/EXP-COMP-1 must implement both and gate on the comparison.)
- **Marginal-claim escort layer →** *build it* (phase-2 P2.W3 / T-CTRL-5 neighbor): the Escort{room} pre-clear objective ships in this overhaul; Marginal claims are no longer auto-rejected.
- **Reserve denial (T-CTRL-3) →** *build the proactive de-reservation capability, but gate it behind a feature flag defaulting OFF.* Reactive denial (when an enemy reserves our target) stays always-on; proactive de-reservation of enemy remotes is operator-enabled per the flag (the G-11 whitelist / `features.rs` pattern), not autonomous yet.
- **Boost-commit policy →** *conservative floor:* gate boosted assaults (T-TOWER-3, T-COMP-1/5, T-NPC-7) behind a stored-mineral / lab-throughput floor; when short, downgrade to unboosted (NO-GO collapses to N≥2 in-bunker, T-TOWER-4) or wait — never drain the economy for an assault. (Exact floor is a tunable; see the boost-commit row.)

**Still open (sim-tunable or pending):**
> (Quad composition and boost-commit policy moved to **Resolved** above on 2026-06-17; their stale "still open" duplicates were removed 2026-06-18.)
- Kill-healer-first-vs-softest default `w`: the crossover (T-FOCUS-3) is comp-dependent. Pick a single `heal_relief_weight` that maximizes kills/energy across the MMO opponent mix (tank+healer, mass-ranged, boosted-quad) rather than overfitting one — but the operator owns whether the bot biases aggressive (lower w, snap free kills) or healer-collapsing (higher w).
- Drain aggressiveness / the one opponent read: T-TOWER-7 (drain-then-attrition) wins the energy exchange only if the enemy's towers naively fire on a confirmed drain; if they hold fire it stalls and we must escalate to a breach. How aggressively should the bot commit to pure economic drain before escalating, and how long does it probe before concluding the defender is holding fire?
- Marginal-claim escort (ADR 0017 / T-CTRL-5 neighbor): the Securing/escort layer is DEFERRED to this overhaul — until it ships, Marginal claims are rejected outright. Operator call: build the escort/pre-clear layer (recovers some winnable contested expansions) or keep treating Marginal as Reject (simpler, loses them)?
- Proactive vs reactive reserve denial (T-CTRL-3): should the bot proactively de-reserve enemy remotes (robust engine-math, but invites escalation) or only react when an enemy reserves OUR target? This is a posture/diplomacy decision under ADR 0014.
- drain_body_heavy is undersized: it carries 20 HEAL (240/tick), below the 300/tick two edge towers deal, so it slowly dies unboosted. Raise the unboosted heavy drain to 25 HEAL, or gate 2+ tower drains behind XLHO2 boost (the `drain_body_for_tower_dps` switch fires at >13 required HEAL but the heavy body still under-delivers)? Operator picks the cost/safety tradeoff.
- Anti-flap persistence windows (escort-release 20t, abort-persistence 20t, establishment-stall 3000t, avoid-cooldown, abandon-hysteresis 5t) are tuned-live-only per ADR 0017 — too eager abandons winnable rooms, too patient bleeds creeps. The operator must set the live observation budget and acceptable false-abandon vs false-persist rates, since the sim cannot fully model a real attacker's cadence.
- Stronghold boosted-vs-unboosted economic break-even per level (T-NPC-7/8): at what stored-energy/loot ratio is a boosted quad worth it vs letting an L4/L5 collapse on its 75000-tick timer? The war.rs energy gates (30K/100K/200K) are coarse vs the actual loot value (coreAmounts 1K/16K/60K/400K/3M) — operator sets the loot floor.
