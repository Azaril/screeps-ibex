# Combat-Systems Review — 2026-07-09 (WFV-27 live-but-unproven)

- **Trigger:** WFV-27 deployed to MMO (shardX) today. The combat/squad stack (Defend objectives, G4 offense O1–O6, SquadManager formation travel/orientation/breach, objectives queue, ADR 0040 economy integration) is freshly live and lightly exercised. This review weights toward latent defects, fragile assumptions, and things likely to misfire in **real MMO combat** (boosted players, multi-tower bunkers, edge-kiting, CPU/bucket pressure).
- **Method:** five parallel deep-read passes over the full combat surface (~31k lines: `military/squad_manager.rs`+`formation.rs`+`squad.rs`, `operations/war.rs`+`objective_queue.rs`+defense missions, the `screeps-combat-decision` crate in full, `jobs/squad_combat.rs`+`threatmap.rs`, and the combat-relevant `screeps-rover` paths), followed by an adversarial verification pass re-reading the cited code for every load-bearing defect. Findings marked **VERIFIED** were independently re-confirmed at the cited lines; **REPORTED** findings carry quoted-code evidence from a single deep-read pass.
- **Relationship to ADR 0008a's readiness plan:** the 2026-06-29 "Implementation Recommendations & Plan" section of [0008a](../design/0008a-combat-tactics.md) already catalogs the known tactic gaps (T-HEAL-3 inputs, T-BREACH-3 repair stub, T-DEF-1 ramparts, T-DEF-5 predictive safe-mode, boost layer, etc.). Those are **not re-reported** here. This review finds a different class: defects in machinery 0008a scored as **DONE**, plus live-adapter seams the pure-kernel tests cannot see. §6 reconciles the two into one build order.
- **This is a review, not a change.** No combat/runtime code was modified. Each finding is a recommendation for a follow-up decision.

---

## 0. Executive summary

The lifecycle machinery (leases, budgets, give-ups, retire precedence, serialization hygiene) is in good shape — several past bug classes are verifiably closed (no silent template fallbacks remain, no raw-`Entity` serialization, no result-affecting HashMap iteration in the decision crate, reconcile races guarded). The exposure is concentrated in four clusters:

1. **The roster-churn cluster (D1, D4, D5, D6)** — the single most common MMO event, *a member dying mid-fight*, triggers three interacting defects (engaged squads re-anchor and stop kiting; refilled seats get a phantom `(0,0)` offset; engaged squads count as "forming" and choke new claims), and the assault latch **bypasses the ADR 0037 winnability veto entirely** (D1).
2. **The live-adapter divergence cluster (D9, D10, D11)** — three cases where the sim-validated behavior is not what live creeps do: the engaged friendly-avoid ladder exists only in the sim driver; the rover discards best-effort flee paths (retreating creeps freeze); squad movement orders inherit `HostileBehavior::Deny` and can fail to route into the very room they are attacking.
3. **Anti-player blindness (R1, R2, R3, R4, D13)** — the decision layer is well-calibrated against NPCs and self-play but structurally blind to standard MMO player tactics: enemy boosts (heal ×4 / TOUGH eHP dropped on the floor), tower refill (drain verdicts assume a cooperating defender), cross-healed drain squads against our towers, and ramparted defenders.
4. **CPU/bucket pressure (R9)** — the per-squad-per-tick tactical pipeline (fresh cost-matrix caches, 2500-tile overlays, kernel floods, per-member DTO rebuilds, per-tick breach Dijkstra) is the main bucket-collapse exposure on a 20-CPU account, and it spikes exactly when fights get big.

Also documented outside the code: the ADR 0044 cross-sim analysis (2026-07-09) found the combat kernel tuning **does not generalize to realistic chokepoint terrain** (R19), and the phase-2 **M4 "Combat-Effective" exit criteria are all still `pending`** — the architecture shipped; effectiveness was never formally validated.

**Top 5 defects:** D1 (latch bypasses winnability veto), D2/D3 (safe-mode hair-trigger + fires-once-ever), D4 (slot refill disables kiting mid-fight), D9 (engaged stuck ladder never wired live), D10 (flee freeze).
**Top 5 MMO risks:** R1 (enemy-boost blindness in the hard gates), R2 (drain has no refill model and no give-up valve), R9 (CPU pipeline), R6 (defense feed-the-kill loop), R19 (kernel tuned on open terrain).
**Top 3 opportunities:** O1 (room-level shared DTO/decision caching), O2 (tower healer-priority + scout-fire gating), O5 (nuke defense → ADR 0040 repair-market bids).

---

## 1. DEFECTS

### P0 — wrong behavior in shipped, live machinery

#### D1 — Assault latch is set from the un-vetoed quorum; the ADR 0037 T2 winnability veto is dead code on the live path — **VERIFIED**
- **Impact/likelihood:** High / High. A squad whose present force has a real-intel LOSING verdict still advances its box anchor across the border into towers the moment the bare count quorum fires — exactly the border-crossing-before-abandon T2 was landed to prevent.
- **Location:** [squad_manager.rs:3273-3292](../../screeps-ibex/src/military/squad_manager.rs) vs the pure gate at [squad_manager.rs:297-305](../../screeps-ibex/src/military/squad_manager.rs).
- **Detail:** the pure predicate carries the veto (`count_quorum_advances = gather_quorum_met && present_wins_or_stalls`, :301), but the live site computes `quorum_now = fast_path_allowed || count_quorum_met` (:3273, **no** `present_wins_or_stalls`) and inserts `assault_latched` from it (:3279-3281) *before* calling `squad_is_gathered` (:3287) — which returns true via the just-set latch. The veto can never bind. The unit tests pass because they exercise `squad_is_gathered` with `assault_latched=false`.
- **Fix:** latch from the vetoed value (`fast_path_allowed || (count_quorum_met && present_wins_or_stalls)`), or compute the gate's internal `quorum_now` once and use it for both the latch write and the branch. Add a pin test where a losing verdict + met count quorum does NOT latch.

#### D2 — Safe-mode hair trigger: `CRITICAL_STRUCTURE_MIN_HITS = 5000` equals spawn max hits — any spawn scratch arms the trigger — **VERIFIED**
- **Impact/likelihood:** High / High. Safe-mode charges are scarce; `spawn.hits() < 5000` is true after *one* hit (spawn max = 5,000), the damage persists until repaired, and the presence gate is the raw hostile list — so a 4,970/5,000 spawn plus a harmless Move-only scout later fires `activate_safe_mode()`. Baitable by an adversary (poke once, retreat, send a scout).
- **Location:** [safe_mode.rs:18](../../screeps-ibex/src/missions/safe_mode.rs), activation path :128-169.
- **Fix:** threshold as a fraction of `hits_max` (e.g. <50%) **and** require active threat (hostiles passing `hostile_warrants_defender`, or incoming DPS on the structure), not mere presence + stale damage.

#### D3 — SafeModeMission `activated` latch is permanent — a room auto-safe-modes at most once, ever — **VERIFIED**
- **Impact/likelihood:** High / certain after first activation. `activated` is set true on activation *or on merely observing* an active safe mode (including operator-triggered) and is never cleared; the mission never returns Success and war.rs only creates one when none exists — so after the 20k-tick safe mode expires, the room has no reactive safe mode forever.
- **Location:** [safe_mode.rs:40,78-101,206,245](../../screeps-ibex/src/missions/safe_mode.rs) (set-only; no reset path — verified by grep).
- **Fix:** clear `activated` when `controller.safe_mode()` reads 0, or drop the flag and use the live `safe_mode() > 0` check as the re-trigger guard.

#### D4 — Mid-fight slot refill re-anchors an engaged skirmish squad — kiting is disabled for the whole replacement window — **VERIFIED (trigger); REPORTED (full chain)**
- **Impact/likelihood:** High / High — member deaths mid-fight are routine and Phase B re-queues every unfilled slot automatically.
- **Location:** [squad_manager.rs:2996-2998](../../screeps-ibex/src/military/squad_manager.rs) (`all_arrived` requires `m.pos` — a freshly registered spawning member has `pos: None`), travel branch :3137, assault re-anchor :3294-3297; consumer [squad_combat.rs:624-629](../../screeps-ibex/src/jobs/squad_combat.rs) (`squad_has_anchor` ⇒ `execute_formation_movement`, which ignores the kite directive).
- **Detail:** the moment a replacement is *registered* (not arrived), `all_arrived` flips false, the latched assault re-establishes `squad_path`, and every in-room member formation-follows the anchor at the focus instead of kiting — for the full spawn+travel window (hundreds of ticks). Only Retreating/drain/structure-siege drop the anchor; the skirmish anchor-drop requires `all_arrived`.
- **Fix:** gate the travel/re-anchor branch on `!in_room_any`, or drop the anchor in the Engaged arm for non-formation objectives regardless of `all_arrived`.

#### D5 — Formation layout only ever shrinks; a refilled seat resolves to offset `(0,0)` — phantom seat stacked on the anchor — **VERIFIED**
- **Impact/likelihood:** Medium / High (die → degrade → refill is the standard casualty cycle).
- **Location:** [squad.rs:825-834](../../screeps-ibex/src/military/squad.rs) (`if living_count < slot_count` — shrink only), `add_member` :466 (`formation_slot = self.members.len()` — exceeds the shrunken layout), `get_offset` :182-184 (`.unwrap_or((0,0))`).
- **Detail:** 4-box, one dies → layout compacts to 3 slots; refill assigns `formation_slot = 3` → offset `(0,0)`, permanently sharing slot 0's tile target. Strict cohesion (`all_in_formation`) becomes unsatisfiable → hold-tick churn + ratchet to Loose every march.
- **Fix:** `living_count != slot_count`, or call `update_formation_for_living_count()` from the member-registration path.

#### D6 — Phase C's "forming" count includes engaged, battle-damaged squads — `MAX_FORMING_SQUADS` silently becomes an offense-concurrency reducer — **VERIFIED**
- **Impact/likelihood:** Medium / High. Two deployed squads carrying casualties (`filled < requested`, still fighting) saturate the cap (=2) and block all new offense claims indefinitely — contradicting the const's own contract ("complete squads out fighting do NOT count") and the `forming_state()` lifecycle definition, which requires `!engaged_once`.
- **Location:** [squad_manager.rs:2349-2366](../../screeps-ibex/src/military/squad_manager.rs) — the filter is `requested > 0 && filled < requested` with no `engaged_once` check (defense is exempted, offense is not).
- **Fix:** add `&& !ctx.engaged_once` to the Phase C filter, mirroring `forming_state`.

#### D7 — `assign_focus_fire` books kill budgets against shooters that cannot reach the target — premature spill fragments focus fire — **VERIFIED**
- **Impact/likelihood:** High / High (squads are spread during approach/kiting in every fight).
- **Location:** [lib.rs:454-481](../../screeps-combat-decision/src/lib.rs) — `remaining -= dps` with **no range check**; the consumer (:676-683) silently shoots something else when out of range of its booked target.
- **Detail:** a distant member's full `melee_power + ranged_power` is deducted from target 1's kill budget; the next shooter spills to target 2; target 1 receives partial damage its healer out-heals; nothing dies. Against healed player forces this converts winnable focus-kills into stalemates. Melee power is also booked at any range (only lands at ≤1).
- **Fix:** deduct only landable DPS (`ranged_power` if range ≤3, `+melee_power` if ≤1 — mirroring `tower_fire::creep_dps_on_focus`); shooters with 0 landable DPS get the target as an approach order, not budget coverage.

#### D8 — Neutral constructed walls are unattackable by every pipeline — breach squads stall at the wall and then retire as "harmless turtle" — **VERIFIED (both halves)**
- **Impact/likelihood:** High / High — real player bunkers are ringed with constructed walls, not just ramparts.
- **Location:** decision crate: `breach_redirect` deliberately targets the first blocking **Neutral** wall ([lib.rs:2042-2044](../../screeps-combat-decision/src/lib.rs)) but the kernel ledger admits only `Ownership::Hostile` ([kernel.rs:373](../../screeps-combat-decision/src/kernel.rs)) and so does `best_hostile_structure_within` (lib.rs:635). Job seam: `get_hostile_structures` filters `.as_owned().map(|o| !o.my()).unwrap_or(false)` — **drops all unowned structures** ([squad_combat.rs:1536-1553](../../screeps-ibex/src/jobs/squad_combat.rs)), and the game-API fallback `find(HOSTILE_STRUCTURES)` also excludes walls. Meanwhile the threatmap's breach costing counts walls (`breach_blockers`, threatmap.rs:434), so force sizing admits objectives the fielded squad literally cannot see.
- **Detail:** the squad marches to the wall, emits no Attack/Dismantle against it, accrues 40 flat `structure_stalled` ticks with `incoming_dps == 0`, and trips the `harmless_turtle` disengage (lib.rs:1682) — retiring from a base it had the dismantle power to breach. The only pinned breach test uses a hostile rampart.
- **Fix:** admit the focus structure into the damage ledger regardless of ownership when it *is* the breach focus (or admit `StructureType::Wall`), and include unowned blocking structures in the job seam's structure list. Add a neutral-wall breach pin test.

#### D9 — Live squad members never get the engaged stuck-threshold ladder — the sim's heal-cluster fix was never wired into the live bot — **VERIFIED (grep)**
- **Impact/likelihood:** High / High. An Engaged live member immobile ≥2 ticks (routine inside a formation) repaths with tier-1 friendly-avoid, which prices every friendly within 5 tiles at `u8::MAX` — the live layer IS populated (`screeps-rover/src/screeps_impl.rs:250-253`). Result: engaged members detour *around their own squad*, prying the heal-the-focus block apart (measured in sim before the fix: focused member's received heal ~800 → ~300/t, sequential pick-off).
- **Location:** the fix exists only in the sim driver (`screeps-combat-agent/src/squad.rs:53-59`, `engaged_stuck_thresholds()` with `avoid_friendly_creeps: u16::MAX`); `grep stuck_thresholds` across `screeps-ibex/src` returns **zero matches** — no live movement request overrides `StuckThresholds::default()` (`avoid_friendly_creeps: 2`), though `MovementRequestBuilder::stuck_thresholds` exists for exactly this.
- **Detail:** this inverts the 2026-07-03 live-parity intent: live got the *layer* (u8::MAX friendlies in cost matrices) but not the *ladder* exemption for engaged members — so live now has the pathology the sim measured and fixed, and self-play overstates live cohesion.
- **Fix:** in `squad_combat.rs`, apply the engaged ladder on every Engaged/Formation member request (mirroring the combat-agent split); keep the default ladder for MoveToRoom travellers.

#### D10 — Rover discards incomplete flee results — retreating creeps freeze in place under a swarm — **VERIFIED**
- **Impact/likelihood:** High / High in exactly the scenario Retreating exists for (outnumbered, surrounded).
- **Location:** [movementsystem.rs:1645-1682](../../screeps-rover/src/movementsystem.rs) — `flee_ops = 2000`, then `if result.incomplete || result.path.is_empty() { return Err(PathNotFound) }`. Caller builds a goal per hostile at range 8 ([squad_combat.rs:1196-1201](../../screeps-ibex/src/jobs/squad_combat.rs)); in a 30–50-hostile siege the union of range-8 discs covers most of the room, so the search is *routinely* incomplete — the best-effort step is thrown away and the creep stands still until dead.
- **Fix:** for flee, accept an incomplete result's first step when `!path.is_empty()` (best-effort withdrawal strictly beats freezing); optionally scale `flee_ops` with threat count.

#### D13 — Tower drain model is cross-heal blind, and the "conserve" branch fires at unconfirmed drainers — an in-room drain squad drains our towers indefinitely — **REPORTED**
- **Impact/likelihood:** High / High — tank + separate healer staying in-room near the edge is the canonical MMO drain tactic, and haulers refill towers at `SURVIVAL_BID` priority (tower.rs:184), so the attacker controls an economy bleed.
- **Location:** [tower.rs:377-457](../../screeps-ibex/src/missions/tower.rs) — per-creep heal is computed from the creep's *own body only*, so a 0-HEAL tank cross-healed by a nearby hostile healer passes the `total_damage > heal` filter and towers volley it forever; the sawtooth `DrainTracker` confirms only on exit/re-entry (a squad that never leaves is never "confirmed"); and the conserve branch's `non_drainer_target` has **no net-damage filter** (:431-436).
- **Fix:** room-level heal pool (hostile HEAL within range of the candidate — threatmap already computes this) in the net-damage filter; apply the filter inside the `is_drain` branch; extend confirmation to "hits not decreasing across N volleys" without requiring exit.

#### D14 — Room-keyed `mark_unwinnable` cross-poisons unrelated objective kinds; the promised producer-side `clear_unwinnable` does not exist — **VERIFIED (clear gap); REPORTED (poisoning paths)**
- **Impact/likelihood:** High / Medium-high. The backoff (2k–20k ticks) is keyed by room and filters **all** claims: an offense give-up on an SK room (e.g. a stronghold `Dismantle`) leaves `Farm{SourceKeeper}` unclaimable for the rest of the backoff even after the stronghold collapses — the farm looks alive and produces nothing; a `Harass` give-up on a remote later blocks the *defense* claim for invaders raiding that same remote (defense never *sets* the latch but is still *filtered* by it).
- **Location:** [objective_queue.rs:497-526](../../screeps-ibex/src/military/objective_queue.rs) (`best_unclaimed_*` skip `is_unwinnable_now(o.kind.room())` for every kind); [war.rs:1796,2817-2834](../../screeps-ibex/src/operations/war.rs) — the "clear on a winnable re-scan" story exists only in comments and a queue-mechanics **test**; the sole production caller is the squad-manager clean-win path (squad_manager.rs:2009), which cannot fire while the room is unclaimable.
- **Fix:** key the backoff by `(room, objective class)` — or at minimum exempt `ObjectiveOwner::Defense` and `Farm{..}` claims — and implement the producer-side `clear_unwinnable` when a re-scan shows the blocker gone.

### P1 — wrong behavior, narrower trigger or bounded blast radius

#### D11 — Squad tick-order `MoveTo` requests default to `HostileBehavior::Deny` — cross-room legs into hostile-flagged rooms fail silently — **VERIFIED (default); REPORTED (call sites)**
- **Impact/likelihood:** Medium-high / Medium-high. `RoomOptions::default()` is `Deny` ([movementrequest.rs:30-36](../../screeps-rover/src/movementrequest.rs)) and ibex's route callback returns `None` (→ ∞) for hostile-flagged rooms — which includes the attack target room itself. The rally `MoveTo` (squad_combat.rs:271), CombatResponse `MoveTo`/`kite_toward_objective` (:533, :551-561), and Engaged/Retreating `MoveTo` (:633, :1101-1105) set no room options; any that needs a cross-room route into/through a hostile-flagged room gets `InternalError` and the member stands still. Only the no-orders fallback passes `HighCost` explicitly (:372-379). Near-invisible: the MOVE-BLOCKED warn latches once per VM (D-min2).
- **Fix:** stamp `RoomOptions::new(HostileBehavior::HighCost)` (or `Allow` for the target room) on every squad-order movement request.

#### D12 — Orphan recall gate inverts for wiped/gave-up squads — survivors solo-charge the hostile room instead of recycling — **REPORTED**
- **Impact/likelihood:** Medium / Medium. The P-OBJ #23 recall triggers only when the room is clear of hostiles; an orphan whose squad retired *because* the room is defended sees hostiles, skips recall, and falls into the solo behaviors — pure melee "close to range 1" against the force that just killed its squad. Recreates the historical orphans-feed-kills failure for exactly the loss case.
- **Location:** [squad_combat.rs:904-917](../../screeps-ibex/src/jobs/squad_combat.rs) (`if orphaned && hostiles.is_empty()`); same shape en route at :204-210.
- **Fix:** an orphan recalls unconditionally (fight-while-withdrawing at most); flee-toward-home if the exit is blocked.

#### D15 — Defense scan hard-exits when no owned room has a spawn — all defense turns off exactly when the base is dying — **REPORTED**
- **Impact/likelihood:** High per-event / Medium (most relevant to young single-room colonies — the current fresh-deploy state on shardX). If the attacker kills the (only) spawn first, the next scan finds `home_rooms` empty and returns before any defense work: no `Secure` objectives, no SafeMode/WallRepair/NukeDefense mission creation, no remote defense.
- **Location:** [war.rs:329-338](../../screeps-ibex/src/operations/war.rs).
- **Fix:** only the fielding paths need a spawn-bearing home — move the early return below mission creation, and let objective emission use globally-available spawn capacity.

#### D16 — Two authoritative producers share one `Secure{room}` objective — priority/force/owner ping-pong — **REPORTED**
- **Impact/likelihood:** Medium / Medium (an operator `attack` flag on a room that also shows hostiles within the defense leash). `ObjectiveKind` equality is the upsert key; the defense scan (every 2t, defense-sized, `.authoritative()`) and the offense AttackFlag path (every 10t, raid-sized, `.authoritative()`) overwrite each other's priority/force/owner — and the owner flip corrupts `offense_count` (counts `owner == Attack`), skewing the offense cap.
- **Location:** [objective_queue.rs:290-316](../../screeps-ibex/src/military/objective_queue.rs); producers war.rs:706-718 vs :1615-1620.
- **Fix:** owner-scoped authoritative upserts (or owner in the dedup key); base `offense_count` on kind+room, not the overwritable owner hint.

#### D17 — `heal_power` is initialized once and never decays — destroyed HEAL parts keep full weight — **REPORTED**
- **Impact/likelihood:** Low-medium / Medium (focusing healers is standard player play). `if member.heal_power == 0 { … }` ([squad.rs:804-806](../../screeps-ibex/src/military/squad.rs)) means one-time computation; a healer whose HEAL parts were shot off keeps full weight in heal triage and the win-or-stall balance — overestimating sustain exactly when most damaged. `caps_from_members` *does* recount live; the two disagree.
- **Fix:** recompute per tick (the body is already being iterated), or on `damage_taken_last_tick > 0`.

#### D18 — Retreating fires `ranged_mass_attack` unconditionally — starves `ranged_heal` (same intent pipeline) and wastes an intent — **REPORTED**
- **Impact/likelihood:** Medium / certain for hybrid RA+HEAL bodies while retreating. RMA and rangedHeal share pipeline B; Retreating consumes B first with no range check, so a withdrawing hybrid can never ranged-heal its assigned target at range 2-3, and RMA is issued even with zero hostiles in range (0.2 CPU + phantom digest intent per tick).
- **Location:** [squad_combat.rs:1050-1053](../../screeps-ibex/src/jobs/squad_combat.rs) vs :1069-1096.
- **Fix:** gate RMA on a hostile within range 3; order heal first while Retreating.

#### D19 — Kernel `Act::Declaim` prices CLAIM output as structure damage and emits it as `CombatIntent::Attack` — **REPORTED (latent)**
- **Impact/likelihood:** Medium / latent (fires when doctrine fields CLAIM members; the plumbing exists). A CLAIM member adjacent to any hostile structure drains phantom `claim_power` from the shared damage residual (spilling other members off a target not actually being damaged) and emits an `Attack` intent the engine drops (no ATTACK part). There is no Declaim/AttackController variant in `CombatIntent`.
- **Location:** [kernel.rs:596-616](../../screeps-combat-decision/src/kernel.rs); `CombatIntent` at lib.rs:238-251.
- **Fix:** gate Declaim to controller targets, add `CombatIntent::AttackController`, never drain structure residuals with claim output.

#### D20 — `drain_standoff_range` tank-range clamp is anti-conservative — sustain check underestimates spread-nest tower DPS — **REPORTED**
- **Impact/likelihood:** Medium-high / Medium-high vs non-point tower nests. `(r − d).unsigned_abs().max(r)` always uses the *farther* range → lower assumed damage — the opposite of the comment's "conservative"; near-side towers of a spread bunker hit from `r − d` in the steeper falloff band while sustain is priced at `r`. Feeds both the standoff hold and `assess_engage`'s unwinnable veto.
- **Location:** [lib.rs:1248](../../screeps-combat-decision/src/lib.rs) (in :1232-1258).
- **Fix:** use the closest possible approach `r.saturating_sub(d)`, or price per-tower from the actual tank goal tile.

#### D23 — Per-creep melee-evade keys on `is_melee_only` — hybrid melee+ranged hostiles, **including the Source Keepers the guard was built for**, never trigger it — **REPORTED**
- **Impact/likelihood:** High / High (SK rooms; player attack+ranged hybrids). Keepers carry ATTACK and RANGED_ATTACK, so `is_melee_only` is false and the documented "SK-duo guard" never fires — a squad directive can march a kiter adjacent to a keeper (300 melee DPS). Inconsistent within the same file: `kite_threats` (lib.rs:1926-1935) correctly classifies keepers as melee-capable; only the ranged kiter's own guard uses the narrow predicate.
- **Location:** [lib.rs:733-735, 802-812, 876-885](../../screeps-combat-decision/src/lib.rs).
- **Fix:** evade any hostile with working ATTACK within range 2 (matching `kite_threats`).

### P2 — latent / bounded

- **D21 — `FormationLayout::orient_toward` rotation table mirrors left/right** (dead code today; `Right=>1`/`Left=>3` swapped for a south-extending layout under `rotate_cw=(x,y)→(-y,x)`); `is_full` measures against the *shrunken* layout (a 3/5 squad reads "full"); `mirror_y`, `should_retreat`, `issue_virtual_anchor_movement` also dead. [squad.rs:158-184, 503, 526](../../screeps-ibex/src/military/squad.rs). Delete or fix+pin before anyone wires O2 orientation through them. — **REPORTED**
- **D22 — `estimated_combat_time` u32 wrap when `available_spawns == 0`**: `spawn_time + travel_ticks` wraps in release (`u32::MAX + travel`), making an unspawnable composition read maximally viable. The sole live caller clamps `.max(1)`, but the kernel is the shared sim seam and the clamp is a caller convention. [composition.rs:235-238](../../screeps-combat-decision/src/composition.rs). Fix: `saturating_add`. — **REPORTED**
- **D-min1 — raw tick subtractions** at tower.rs:316-317 (debug-build-only panic class; release wraps benignly). — **REPORTED**
- **D-min2 — `warn_move_blocked_once` latches once per VM lifetime** — the first blocked member anywhere consumes the rally-unreachable diagnostic for days. [squad_combat.rs:129-146](../../screeps-ibex/src/jobs/squad_combat.rs). Rate-limit per (squad, window) instead — this is the signal you want during a live-unproven deploy. — **REPORTED**

---

## 2. RISKS — fragile assumptions / unproven-on-MMO hazards

#### R1 — Enemy-boost blindness inverts the hard gates (killability, winnability, tower hold-fire) — **REPORTED; top MMO risk**
Everything enemy-side is priced unboosted in the decision layer, and the pieces the threatmap *does* price boosted never reach the sizers:
- `heal_reaching` uses raw 12/4 per part (lib.rs:389-399) — a T3 healer heals 48/part, so `ev_target_order` marks out-healed creeps **killable** and squads commit fire that can never finish.
- `assess_engage`'s `creep_dps` (lib.rs:1445-1447) prices boosted weapons ×1 → Lanchester overestimates our odds up to 4× → `present_force_wins_or_stalls` deploys into losses.
- `decide_towers` gap-sizing ignores boosted-TOUGH ×0.3 (tower_fire.rs:247-252) → "one tower finishes it" lands ~30% and the freed towers redirect while the target survives.
- `EnemyForce.hits = Σ raw hits` (war.rs:188) — the threatmap's computed boosted-TOUGH eHP (`100/0.3`, threatmap.rs:174) is **dropped on the floor**, and `EnemyForce.boosted` has **no consumer** in the crate (doctrine.rs:87-94) — kill pools under-sized up to ~3× vs boosted tanks.
- `threat_step_ticks` ignores move boosts (lib.rs:1903-1917) → chaser-reachability under-predicts closure and kiters get caught.

This is materially worse than the known 0008a gaps (which cover *our* heal output and the ThreatField): the enemy-side blindness feeds the **hard gates**. Fix direction: per-part boost multipliers on `CombatBodyPart` (the live adapter can read `body[i].boost`), threaded into heal/dps/TOUGH/mobility; interim, a pessimism factor on enemy heal when any enemy part is boosted. Complementary inconsistency: the threatmap *classifier* over-escalates in the other direction — any boosted part (a move-boosted scout) ⇒ `PlayerSiege`, 4 creeps with one armed ⇒ `PlayerSiege`, every boost assumed T3 ×4 (threatmap.rs:150-158, 243-253) — so the bot simultaneously over-spends on phantom sieges and under-sizes against real ones.

#### R2 — Drain verdicts assume a cooperating defender: no tower-refill model, no fire-discretion model, and no drain give-up valve — **REPORTED**
Every real tower (cap 1000) reads "finite/drainable" against the 50k infinite sentinel (force_sizing.rs:266-299), so `assess` returns winnable/Drain for any base a breach can't out-heal. An active player refills from storage (drain never completes) or holds fire (energy never drains). Meanwhile every disengage valve is disabled for drain stance: `harmless_turtle` requires `!drain_stance` (lib.rs:1682) and `below_band_stalemate` can't fire (creepless base clamps balance to +1000, lib.rs:1516-1518). The squad soaks for its whole lease → GaveUp — a systematic energy bleed against any awake player, repeated after each backoff expiry. Fix: gate the Drain arm on ownership/activity intel (player-owned + storage ⇒ towers infinite); make "tower energy not falling under our soak" a drain-abort condition with a bounded budget.

#### R3 — Stale drained-tower snapshots flip a room to the "undefended, p_kill=1.0" certain-win path — **REPORTED**
`Seen` tower intel is never re-scout-deferred (only `ScoutedEmpty` is, force_sizing.rs:126-128); towers with `energy < TOWER_ENERGY_COST` are filtered from DPS (:242-245); `undefended = tower_dps == 0 && incoming == 0` ⇒ binary `p_kill = 1.0` (composition.rs:643). A room scouted mid-drain (exactly the state a *previous drain attempt leaves behind*) fields a minimal certain-win squad into towers that refilled on arrival — the ADR 0035 vacuous-intel cascade class, one field over. Fix: age the energy content (floor stale-`Seen` towers at full, matching the existing `unwrap_or(1000)` conservative convention); exclude drained-but-present towers from the `undefended` predicate.

#### R4 — `clear_force` has no rampart/structure channel — creep-clear raids stall against ramparted defenders — **REPORTED**
A creep-clear composition is RANGED+HEAL only (no `dismantle_parts` in the emitted `RequiredForce`, force_sizing.rs:473-538; doctrine.rs:237-242 routes only towers + creep stats). A defender parked on a rampart takes redirected damage through rampart HP (the engine's real shelter), so the fielded DPS grinds millions of hits it was never sized for — verdict says winnable, fight stalls, squad burns its lifetime to GaveUp. Applies to `RaidCreeps`/`ClearCreeps` vs nearly all player rooms. Fix: thread a defenders-under-ramparts signal into the creep-clear arm (add sheltered rampart HP to the kill pool, or route to the SiegeBreach/assess arm which owns `breach_hits`).

#### R5 — `win_probability` floors at ~0.0067 and gated doctrines commit on EV, not the verdict — high-value targets buy hopeless commits — **REPORTED**
`surplus ≥ −1` always ⇒ `p_survive ≥ 1/(1+e^5)`; `ev = p_win·target_value − cost > threshold` has no `assessment.winnable` check (force_sizing.rs:726-736; composition.rs:680-706). Any objective valued ≳150× comp cost EV-clears at a ~0.7% win estimate — and re-fields after each backoff expiry. Fix: clamp `p_survive` to 0 below a hard surplus floor, or AND the commit with the verdict.

#### R6 — Defense feed-the-kill loop: always-field floor + never-back-off + instant retire has no escalation terminal — **REPORTED**
Threat exceeds the one-squad ceiling → `clear_force` unwinnable → always-field driver raises the 4-HEAL/4-RANGED floor (doctrine.rs:614-643) → floor defender arrives, gets the in-room LOSE verdict, retires GaveUp with `mark_unwinnable=false` (Defend exemption, lifecycle.rs:289-300) → objective persists → Phase C re-fields the same floor defender. Each cycle donates spawn energy to the attacker; nothing escalates (no safe-mode signal, no multi-squad, no stop-feeding bound). Fires exactly when a strong player hits an owned room. Fix: an explicit "over-run" signal instead of the floor when unwinnable at ceiling — hold spawning, trigger safe-mode evaluation, or request multi-room reinforcement.

#### R7 — No defense preemption at `MAX_CONCURRENT_SQUADS` — 4 active offense squads means a base under attack cannot field a defender — **REPORTED (known-open REC-023, now live exposure)**
`while active < MAX_CONCURRENT_SQUADS` gates defense claims too; defense bypasses only the *forming* cap (squad_manager.rs:2414, 2436). Fix: retire/reassign the lowest-EV offense squad when a CRITICAL defense objective is claimable at cap, or let defense exceed the cap by 1.

#### R8 — `ready_to_depart` has no latch — attrition below quorum flips a deployed squad back to RALLY and freezes mid-corridor survivors with `Hold` — **REPORTED**
Members in hostile intermediate rooms get `TickMovement::Hold` (a no-op) until the lease lapses (~400t GaveUp) — free kills. The assault latch protects gather→assault but `ready_to_depart` is evaluated per-tick upstream of it (squad_manager.rs:3114-3129). Related residual: a defense squad with real losing intel and an incomplete roster holds at home indefinitely while its room burns, bounded only by the forming budget → GaveUp → re-field churn. Fix: for members away from home, degrade Hold to return-home/flee; latch departure like assault; let Defend-class squads release at `MIN_VIABLE_GROUP` when the objective room has friendly towers (which change the in-room Lanchester the at-home view can't see).

#### R9 — CPU/bucket: the per-squad-per-tick tactical pipeline is the main bucket-collapse exposure on a 20-CPU account — **REPORTED (cluster)**
Bucket-drain collapse is a documented historical failure mode of this bot (June 24). The steady-state and spike costs:
- **Manager:** up to 4 squads × (DTO build + `build_room_threat_field` + `build_target_matrix` two 50×50 loops + flood fill + `decide_squad_with_pathing`) — all **before** the rally gate, so a squad forming at home for 3000 ticks pays it every tick; `CostMatrixCache::default()` constructed fresh at every call site (squad_manager.rs:2952-2986; formation.rs:329-353), and the anchor destination is rewritten every tick to a moving focus, forcing PathFinder re-paths (formation.rs:129-131).
- **Kernel:** `assess_engage`/`ev_target_order` recomputed 3× per squad-tick; a 2500-op `search_scored_set` flood every engaged tick; `best_tile` × 9 tiles × ~12 action-sets × targets with two Vec clones per set (lib.rs:2241-2335; kernel.rs:499-526); `breach_redirect` Dijkstra every tick of a structure siege.
- **Jobs:** per-member double full-room DTO rebuild — O(members × hostiles) `creep.body()` JS crossings; a 4-member squad in a 50-hostile room ≈ 400+ crossings/tick (squad_combat.rs:677-684, 806-812).
- **Threatmap:** per-tick invader-core breach Dijkstra + fresh 2500-byte terrain buffer copy per visible core room — continuous while SK farming keeps core rooms visible (threatmap.rs:373-439).
- **Rover accounting:** ops charged at `allowed_ops` max, never actual (movementsystem.rs:1777-1785) — sieges self-throttle stuck repaths on paper; and low-bucket "normal mode" sets pathfinding headroom = the whole movement cap, effectively disabling new paths under sustained CPU pressure — newly spawned defenders stand at spawn during the siege that drained the bucket (pathing/movementsystem.rs:539-556).

Fixes are in §4 O1 (shared room-level caching + gate-first ordering) plus: cadence the breach Dijkstra, refund unused ops, exempt `needs_path` from the low-bucket headroom gate.

#### R10 — `uncontested_intel`/`have_target_intel` satisfied by ANY cached structure (own/neutral roads included) — the RC-11/D3 no-intel guards rarely engage — **REPORTED**
`!hostiles.is_empty() || !structures.is_empty() || LiveVisible` (squad_manager.rs:3065-3066) over `rd.get_structures().all()` — any once-scouted controller room has "intel" forever, so a stale cache yields a vacuous win → fast-path assault + rally at the room centre in tower range. Related: `classify_objective`'s `has_structures` counts *our own* base structures for a Defend garrison → breach weight profile at home. Fix: count only hostile-owned structures; optionally require cache freshness for the fast path.

#### R11 — Multi-squad in one room: kill budgets, heal triage, and residual ledgers are all per-squad — **REPORTED**
Two friendly squads engaging the same defenders double-book kill budgets (inter-squad overkill), each excludes the other from `our_dps` (combined-killable targets read unkillable to both), and healers never cross-heal (lib.rs:1633-1792; kernel.rs:335-458). Normal war state once defense emissions and offense overlap. Fix: pass co-located friendly squads' landable DPS/positions into the view; longer-term one room-level ledger drained in squad-id order.

#### R12 — Retreat hysteresis can latch `Retreating` in exactly the parity band the proceed gate deploys into; and the stalemate valve is blind to a winning-but-uncatchable enemy — **REPORTED**
(a) `can_reengage` requires balance ≥ +200 while `present_force_wins_or_stalls` deploys anything > −200 (lib.rs:1700-1718 vs 1575-1578): a parity-band squad that trips one retreat kites in-room forever (still win-or-stall, so lifecycle doesn't retire it); `any_critical` also counts a dead-but-rostered member (hits 0) as permanently critical. (b) `below_band_stalemate` fires only when balance < +200 (lib.rs:1680-1684): a cheap fast edge-kiter our expensive squad massively out-strengths but can never catch trips neither arm — the classic MMO bleed. Fix: re-engage bar = "would not lose" (> −200) with HP as the sole hysteresis axis; exclude hits==0 from `any_critical`; add an uncatchable-mobility disengage arm.

#### R13 — Sizing-input gaps at the war.rs seam — **REPORTED**
(a) Neighbour-threat defenders sized with `heal: 0, hits: 0, boosted: false` (war.rs:737-758) — healer-backed raiding pairs get a defender that plinks forever (the full bodies are collected at :563-611 and discarded). (b) Owned-room defenders sized to the *attacked* room's shrinking `energy_capacity_available()` (war.rs:657-675) — a degradation spiral as extensions die (see O3). (c) The auction prices tower threat at room-centre range while the commit path prices at the objective tile (squad_manager.rs:797-808 vs war.rs:1071-1076) — up to 4× disagreement about death-traps between reassignment and fielding. (d) Multi-home claim EV anchors on `homes.first()` (ECS join order) for travel and energy (squad_manager.rs:2367-2391) while the actual spawn uses the strongest in-range home — mispriced claims for a spread empire.

#### R14 — Offense discovery is still intel-horizon-bound — the "No attack candidates" class is structurally open — **REPORTED**
Candidates require a live `RoomThreatData` (expires at 500t); the only war-owned visibility feed is 1-hop-adjacent, opportunistic weight 0.5 (war.rs:1054-1127, 1896-1934) — the 10-hop candidate radius is mostly dead. Fix: extend the visibility ring to the offense radius (throttled BFS, MEDIUM priority near threat-data expiry).

#### R15 — `can_afford_military`'s 5k-per-room reserve floor starves young colonies of the lvl0-core clears their remotes depend on — **REPORTED**
A room storing <5k can afford *nothing* (economy.rs:87-90 → war.rs:1756), including the cheap reserver-core `Dismantle` whose scoring deliberately allows an empty war chest — remotes stay NPC-reserved for the ~75k collapse timer during exactly the growth phase that needs them. High likelihood in the current fresh-colony shardX state. Fix: scale the floor with RCL/storage presence, or exempt the lvl0-core class under a small absolute ceiling.

#### R16 — Shove can land a creep on room-border tiles — engine-rejected moves and silent room ejection — **REPORTED**
`try_shove` allows `0..=49` while avoidance paths restrict to `1..=48` (resolver.rs:1042-1044), and the live walkability predicate blanket-approves the whole border ring including terrain walls (screeps_impl.rs:142-155) — a formation fighting near an exit can have a member shoved out of the room by its own resolver. Fix: clamp shove landings to `1..=48`; consult terrain on border tiles.

#### R17 — Guarded-sink pipeline model diverges from the engine's simultaneous-actions matrix — **REPORTED**
The sink pins attack/heal as independent (intents.rs:181-191 + test), but the engine conflicts `attack`⊗`heal` and `rangedHeal`⊗`attack` — a hybrid issuing both has one silently dropped while the decision layer, recorder digest, and (if unmodeled) the sim believe both landed. Low-medium today (comps mostly separate roles) but the sizer can emit hybrids. Fix: encode the real conflict pairs so first-caller-wins actually decides.

#### R18 — Stall/staleness accrual gated on member presence, not intel liveness — **REPORTED (low)**
`enemy_stall`/`structure_stall` streaks can grow from frozen cached snapshots when `in_room_any` is true but the room isn't live-visible that tick (squad_manager.rs:2892-2921) — a spurious stalemate disengage of a winning grind. Fix: gate accrual on live visibility.

#### R19 — Combat kernel tuning does not generalize to realistic chokepoint terrain (ADR 0044 cross-sim finding, 2026-07-09) — **doc-grounded**
The shared terrain generator wired into `combat-eval` exposed: mirror-symmetric self-play net swings ±~500 on cave seeds (≈0 on open — a position/order bias trivial rooms hid), and the tuned `open_combat` edge over `default` ranges **−750..+890** across cave seeds (consistent −835 on OpenField). MMO rooms are chokepoint-heavy (bunkers, walls, corridors). The re-tune (add `Bed::Generated` to the sweep basket, re-run the kernel grid, re-pin EXP-KITE-1/EXP-COHESION-1/EXP-POS-* canaries) is specified in [ADR 0044](../design/0044-transfer-market-min-cost-flow.md) and outstanding while combat is live. This compounds every tactical finding above: live MMO fights are being fought with parameters proven only on terrain easier than the real map.

#### R20 — Cross-crate serialized shapes: rover's `AnchorPath`/`CreepPathData`/`StuckState` are embedded in ibex's world format with no in-tree WFV tripwire — **REPORTED**
A rover submodule field append silently changes ibex's serialized shape (the exact REC-001 class that forced WFV 23→24). Bincode is positional: the many `#[serde(default)]`s do not protect it. Fix: a comment-fence + CI check (e.g. a layout-hash test over the embedded rover types) so a rover bump fails loudly in-tree.

#### R21 — Standing process risks (doc-grounded)
- **M4 "Combat-Effective" exit criteria: all 11 still `pending`** ([phase-2.md §2.10](../execution/phase-2.md)) — the architecture is complete; effectiveness on a live opponent was never formally closed. WFV-27 on MMO *is* the soak; the seg-57 cohesion/orphan/kill-efficiency canary should be actively watched, not passively collected.
- **I1/I2 identity open** — `squad_entity: Option<u32>`-era dangling refs are mitigated (EntityOption + Recall + repair pass, verified clean in §5) but the minted-`SquadId` end-state (ADR 0001) remains unbuilt; multi-squad assault and synchronized spawning are gated on it.
- **Boost layer (ADR 0041): accepted, zero code** — both directions (our forces unboosted; R1's enemy blindness). The single largest capability gap vs established MMO players.
- **Heavy multi-squad assault: deliberately deferred** — one-squad-per-objective caps offense below what real bunkers require; the D10/#38 escalate-vs-abandon terminal currently just abandons.
- **Squad dismantle seam fix (2026-07-03) live offense-soak verification still pending** per the memory ledger — WFV-27 includes it; watch the first real `Dismantle` engagement.

---

## 3. OPPORTUNITIES

#### O1 — Room-level shared combat caching + gate-first ordering (the R9 fix, and a consistency win)
`build_room_combat_dtos` is rebuilt per *squad* (and again per *member* in the job seam); `decide_squad_with_pathing` runs for squads whose orders are then overwritten by the RALLY hold. Cache `(hostiles, structures, intel_source)` per room per tick (the `room_layers` map already exists as the pattern); compute `ready_to_depart` first and short-circuit the kernel/kite pipeline for at-home squads; persist one `CostMatrixCache` across squads (ideally ticks); quantize the anchor destination (re-target only when the focus moved >2 tiles). Cuts the steady-state manager cost to near-zero for forming squads and removes the per-member JS-crossing amplification — the cheapest insurance against a bucket collapse during the first real war.

#### O2 — Tower targeting: healer-priority tier + scout-fire gating
The no-squad tower path prefers dangerous-then-lowest-hits — enemy healers (the force multiplier D13 shows the model can't otherwise beat) sort last; and the final fallback volleys full tower fire at Move-only scouts (a self-inflicted mini-drain, 10 energy × towers × ticks) (tower.rs:401-417, 465-475). Both fixes reuse existing kernels (`hostile_warrants_defender`, `estimated_ticks_to_kill`).

#### O3 — Defender sizing: size at the strongest in-range home, not the attacked room
`member_energy = attacked_room.energy_capacity_available()` shrinks as the attacker destroys extensions — each defender generation is smaller against an undiminished enemy (war.rs:657-675). The remote/neighbour paths already borrow `max_home_energy`; apply the same REC-015 rule to owned-room defense: `max(attacked_room_capacity, max_home_energy_within_spawn_range)`.

#### O4 — `EconomySnapshot.available_boosts`: populate or delete
Declared, aggregated, queried — and always `HashMap::new()` (economy.rs:231). When ADR 0041 lands against this snapshot it will silently conclude "no boosts anywhere." Populate from labs/storage/terminal now (the structures are already iterated) or delete the accessors so a future consumer can't bind to a dead field.

#### O5 — Nuke defense → ADR 0040 repair-market bids
The mission computes exact blast-zone rampart deficits (10.5M/5.5M) and then only logs (nuke_defense.rs:159-174); normal repair never targets multi-million-hit ramparts. ADR 0040 M5a's sink market is the natural vehicle: bid the deficits at survival-lane priority budgeted against `ticks_to_land`. Turns a paper mission into a real capability with mostly-existing machinery.

#### O6 — Un-dead-end the importance/over-power ladder
For defended structures the base requirement already sits at the 8-member fielding ceiling, so every ladder rung k>1.0 overflows and is skipped, and the 1.5× importance margin can push a *winnable-per-assessment* target to assemble at NO rung → silent defer (composition.rs:645-737; doctrine.rs:221; force_sizing.rs:360-379). Apply importance inside the EV term (scale `target_value`, not the part vector), or clamp the scaled requirement to the assemblable ceiling. Longer-term this is the multi-squad escalation seam.

#### O7 — Heal-triage completeness: route idle healers by uncovered risk
The idle-healer pre-heal pass ignores coverage already booked — two healers stack on one tank while a wounded member at range 3 gets nothing (lib.rs:1871-1888). Track residual need across both passes.

#### O8 — Rover ops-budget refund + flee-ops scaling
Refund `allowed_ops − ops_used` after each search (the result exposes ops used); scale `flee_ops` with threat count. Cheap, strictly increases real search throughput under siege load (pairs with R9's accounting findings).

---

## 4. Serialized combat shapes (WFV inventory)

All persisted via the bincode component segments behind `WORLD_FORMAT_VERSION` ([game_loop.rs:746](../../screeps-ibex/src/game_loop.rs), currently **27**). Bincode is positional/variant-indexed — `#[serde(default)]` is decoration; **any** field add/remove/reorder or non-appended enum variant ⇒ WFV bump.

| Shape | Location | Notes |
|---|---|---|
| `SquadCombatJobContext` | jobs/squad_combat.rs:24-38 | ConvertSaveload; `squad_entity: EntityOption<Entity>` marker-remapped + scrubbed (REC-009b) — **clean** |
| `SquadCombatState` | jobs/squad_combat.rs:49-62 | variant-indexed `machine!` enum — new states append-only |
| `SquadContext` / `SquadMember` | military/squad.rs:333-434 | ConvertSaveload; `EntityVec`/`EntityOption` throughout — **clean**; `focus_target_id` was the WFV 25→26 field |
| `TickOrders`/`TickMovement` | military/squad.rs:195-328 | ephemeral fields `#[serde(skip)]`; `movement` IS serialized — variants append-only (`Recycle` correctly appended) |
| `SquadPath` → rover `AnchorPath` | squad.rs:61-67; rover anchor.rs:47-58 | **cross-crate hazard** — see R20 |
| `CreepRoverData(CreepMovementData)` | pathing/movementsystem.rs:16-19 | transparent; contains `CreepPathData` + `StuckState` (the WFV-24 fields) — same hazard |
| `RoomThreatData` (+ HostileCreepInfo, NukeInfo, ThreatLevel) | military/threatmap.rs:11-120 | the file documents its own WFV 14→15 gate; future fields (e.g. T-BREACH-3's `repair_per_tick`) ⇒ WFV |
| `CombatObjectiveQueue` + objectives | military/objective_queue.rs:66-208 | `#[serde(default)]` container; runtime intel (claimed_by/assault_mode/est_ticks) correctly ephemeral |
| `SafeModeMission.activated` | missions/safe_mode.rs:40 | persisted — D3's permanent latch survives resets short of a WFV bump |

**Raw-`Entity`-without-marker audit: clean.** The REC-009b regression test pins both the round-trip and the dead-ref scrub.

---

## 5. Verified-clean (checked, no finding — do not re-flag)

- No silent `.unwrap_or(template)` composition fallbacks remain (ADR 0030/0031 cleanup held); `no_dynamic_doctrine_silently_fields_static` pin test present.
- No result-affecting HashMap/HashSet iteration in the decision crate's scoped files; the Hungarian is integer-quantized with a lexicographic tie-break + brute-force cross-check.
- Engine-math pins verified: tower falloff (600/450/150), RMA falloff {1, 1, 0.4, 0.1}, heal bands 12/4 (unboosted), two-phase damage-then-heal netting, rampart-shield redirect + RMA shielded-skip on both focus and RMA-gate paths.
- Lifecycle reconcile races guarded: proximity-gated `engaging` refresh blocks give-up-while-engaged; `resolved` dominates same-tick `retreat_budget_exhausted` (REC-061); retire-reason precedence matches spec; wiped/gave-up never reassign.
- The unsound 0.75 ratio-quorum revert (`9705b6a`) holds; `deploy_then_retreat_allowed` correctly restricts the quorum to no-intel targets — **but see D1**, which bypasses the gather-advance veto by a different route.
- Division/underflow guards in the sizing kernels (`ticks_for`, `parts_for_rate`, `HEAL_OVERCOME_MARGIN` compile-time assert) — only D22 survived.

---

## 6. Reconciliation with the ADR 0008a readiness plan + recommended sequencing

The 0008a Tier 0–3 build order stands, with these amendments from this review:

1. **A new Tier −1 ahead of 0008a's Tier 0: fix the shipped machinery before improving its inputs.** 0008a Tier 0 (T-HEAL-3/T-BREACH-3) fixes *inputs* to gates assumed correct; D1 shows the central gate is bypassed outright, and D4/D5/D6 break the casualty cycle every fight will exercise. Recommended first wave (small, contained diffs, no WFV): **D1, D4, D5, D6, D9, D10, D11** — the roster-churn + live-adapter clusters — plus **D2/D3** (safe-mode, two-line class fixes protecting a scarce irreversible resource).
2. **T-HEAL-3 (0008a Tier 0 #1) should be widened into R1.** Fixing `estimated_heal` reachability without the enemy-boost multipliers still leaves the killability/winnability gates wrong by 4× against the opponents that matter. Do the `CombatBodyPart` boost threading as one workstream.
3. **The drain/tower group (D13, D20, R2, R3) supersedes 0008a's T-TOWER-7 deferral rationale** — the review shows drain is broken in both directions *today* (our towers drainable by the standard comp; our drains bleed against refilling defenders), not a second-order improvement.
4. **T-DEF-5 (predictive safe-mode) is now the third safe-mode work item, not the first** — D2/D3 mean the *reactive* trigger is both hair-triggered and fires-once-ever; fix those before adding prediction.
5. **D8 (neutral walls) belongs with T-BREACH-1's owner** — the breach corridor selection is fine; the execution layer can't hit what it selects.
6. **R19 (chokepoint re-tune) should gate any further kernel-parameter work** — re-tune on `Bed::Generated` before sweeping anything else, per ADR 0044.
7. **Watch items for the live soak** (no code): seg-57 cohesion/orphan canaries vs the D4/D5 churn signatures (hold-tick spikes, Loose ratchets after casualties); tower energy trends vs D13; `[Lifecycle] RETIRE reason=GaveUp` clusters on drain objectives vs R2; bucket trend during the first multi-squad engagement vs R9.

---

*Review artifacts: five deep-read passes + adversarial verification, 2026-07-09. Finding IDs (D=defect, R=risk, O=opportunity) are stable for follow-up tracking.*
