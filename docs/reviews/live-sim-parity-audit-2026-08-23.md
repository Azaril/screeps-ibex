# Live↔Sim parity audit — 2026-08-23 (ultracode)

**Trigger:** operator directive (2026-08-23): "make sure the live code uses all the same behavior
as simulation, so we know live will behave correctly and is not getting bad input."
**Method:** 7-seam multi-agent survey (DTO construction, threat field, SquadView, movement/mover
config, boost model, tower kernel, lifecycle inputs) + adversarial per-finding verification.
43 of 47 raw findings CONFIRMED against code; 10 HIGH. Raw evidence per finding below (verbatim
from the verified audit output).

## Triage (what happened to the findings)

**FIXED in the WS-VAL batch (this commit series):**
- **H4 / M16** — live `heal_power` latched stale → now recomputed every tick, boosted
  (`screeps-ibex/src/military/squad.rs`).
- **H7 / M15 / M19 (the boost-blind seam)** — `CombatBodyPart.boost_mult` +
  `CombatCreepDto::effective_output` now price boosts in every shared kernel consumer
  (`heal_reaching`, `threat_value`, `best_heal_target`, `assess_engage`, `kite_threats`); live
  adapter stamps `output_multiplier_for(compound)`; sim adapter stamps the SimBody tier. This was
  also the stronghold-gauntlet freeze root cause (an EV-sized T3 squad read its own heal at ¼ and
  refused its own fight).
- **M15-adjacent harness gap** — the eval harness now FIELDS boosted bodies (stronghold corpus +
  boosted self-play basket; `place_at_entry` honors the stamped comp tier).

**BACKLOG (unfixed — ranked; each is a real live/sim behavior divergence):**
1. ~~**H0**~~ **FIXED 2026-08-24** (`is_combat_targetable` in squad_combat.rs): the execution-side
   structure list now includes NEUTRAL constructed walls with hits (matching the decision layer,
   which always saw them), in BOTH the cached-RoomData arm and the live `find` fallback — the
   emitted Attack/Dismantle intents against a neutral wall ring now resolve instead of silently
   dropping. (No native unit pin — JS-backed types; verify on the next live neutral-wall breach.)
2. **H1/H2/H3 + M2/M3/M8/M13 (threat/traversal unification cluster)** — SIM SIDE FIXED
   2026-08-24 (agent `4e68de4`: the traversal field now delegates to the shared
   `build_room_threat_field` — cover-aware, hostile-energized-tower-gated, unboosted-stamped
   (H1/H2/M3/M8); own ramparts walkable (M2)). LIVE SIDE FIXED 2026-08-24 (super `1abcc8f` +
   rover `fae493b`): H3/M13 — the live mover overlays squad-manager-published per-tick threat
   tiles under the structure layer's hard blockers (`RoomThreatCosts` + `ThreatOverlayCostSource`);
   M4/M6 — the decide room-callback honors the requested room. THE WHOLE CLUSTER IS CLOSED.
3. ~~**H5 / M0**~~ **FIXED 2026-08-24** (agent `1fbff1b` + decision `b0b7ea0`): the sim now builds
   full-roster member views matching live (execution stays room-scoped), and the shared kernel's
   cross-room-centroid consequences are defused on BOTH sides — fight-room anchoring
   (`plan_squad_ev` room param), room-gated mover anchor, room-local tower assessment.
4. ~~**H6 / M10 / M11 / M12**~~ **FIXED 2026-08-24** (item 7 batch; sim-core + agent + live): sim
   shove depth 3→10 (10 IS live), flee knobs matched (High for every fleer, swap allowed, bid
   MoveTo-only), live retreat arm now carries the engaged stuck ladder (M11 — the sim's uniform
   in-room treatment; a withdrawing member needs squadmate-transparency most). M10 resolved in the
   OTHER direction: live `friendly_creep_distance` 15→5 — the 15 was a pre-tournament hand-tune,
   and matching sim to it made the cross-border assault bed arrive strung out and DIE; 5 is the
   value the whole validated envelope runs at. M14 (per-squad vs global resolver pass) REMAINS —
   architectural, queued with the multi-squad doctrine (Phase 4.5 item 8).
5. ~~**H8**~~ **FIXED 2026-08-24** (live tower.rs + decision `heal_reaching` export): the no-squad
   path prices sustain with the KERNEL's `heal_reaching` (adjacent healers + hostile towers,
   boost-aware) and the no-net-damage weakest-hostile chip fallback is DELETED (hold fire — the
   kernel discipline). M17 (full no-squad decide_towers unification) and M18 (threat_value has no
   WORK/CLAIM term — a DESIGN fork: it feeds focus EV too) REMAIN — queued below.
6. ~~**H9**~~ **FIXED 2026-08-24** (squad_manager): `forming_state` takes `departed` (stamped by
   the SquadTrace phase pass the tick the rally gate releases, cumulative per generation), and
   `traveling` accepts a released under-strength squad — the progress-gated travel lease, matching
   the harness drivers. M20–M23 (harness never exercises forming_in_flight/vacuous_clear/
   retreat-budget/economic-give-up inputs) REMAIN — harness-lane work, queued there.
7. **M4 / M6** — live `room_callback` returns the TARGET room's matrix for every requested room.

---


## HIGH (10)

### [H0] combat-dto-construction/structure-target-resolution (from dto)
CLAIM: The squad kernel can focus a NEUTRAL constructed wall (breach_redirect explicitly prices unowned walls as dismantlable blockers), but the live intent executor resolves structure targets only against a hostile-OWNED structure list, so the emitted Attack/Dismantle intent is silently dropped — live squads cannot damage neutral walls anywhere in the combat path, while the sim executes the same intent.
LIVE: screeps-ibex/src/jobs/squad_combat.rs:1674 get_hostile_structures filters `.filter(|s| s.as_owned().map(|o| !o.my()).unwrap_or(false))` (fallback :1682 `find(find::HOSTILE_STRUCTURES)` — also excludes unowned walls); translate_intents resolves structure intents via `struct_at` over that list only (squad_combat.rs:747, :756, :765, :799). The kernel ledger is hostile-only too (screeps-combat-decisio
SIM: screeps-combat-agent/src/lib.rs:145-160 — pos_to_struct indexes ALL structures including StructureKind::Wall (scenario.rs:144-146 builds walls with owner None); to_engine_action (lib.rs:303-312) resolves Attack{id:None}/Dismantle via view.structure_for(pos) → CombatAction::AttackStructure/Dismantle executes against the neutral wall.
CONSEQUENCE: A siege whose cheapest breach corridor runs through a neutral wall ring (e.g. a hostile spawn/core ringed by unowned constructed walls — common on MMO) advances to the wall, emits Attack each tick, and every intent is dropped at translate_intents: the squad stands adjacent doing zero damage until a stall/lease valve fires. Self-play validated the breach (sim razes the wall), so the live no-op is invisible to the harness — sim overstates live breach capability exactly where breach_redirect was de

### [H1] Seam 2 — threat field cover-awareness (T-DEF-1) (from threatfield)
CLAIM: The sim's traversal threat field is NOT cover-aware: it stamps threat onto the squad's own maintained rampart tiles, while the live traversal field zeroes them via ThreatField::build_covered + friendly_rampart_cover.
LIVE: screeps-combat-decision/src/lib.rs:2012-2016 — build_room_threat_field: "let cover = friendly_rampart_cover(structures); kite::ThreatField::build_covered(&kite_threats(hostiles), &kite_towers(structures), &cover)"; folded into the live matrix at screeps-ibex/src/military/squad_manager.rs:3273-3275.
SIM: screeps-combat-agent/src/pathing.rs:63 — room_threat_field ends with "ThreatField::build(&threats, &towers)" (build_covered with empty cover, kite.rs:50-51); consumed by both traversal sources at pathing.rs:151-152 and pathing.rs:414-415.
CONSEQUENCE: Live traversal prices its own maintained rampart line as a zero-threat safe corridor (the T-DEF-1 intent) while the sim keeps charging up to the full THREAT_PATH_CAP=8 penalty on those tiles — the cover-corridor routing behavior now live was never exercised by the sim that validated the traversal pricing, and any EXP-* sweep of THREAT_PATH_DIV/CAP runs against a field with different geometry than live's.

### [H2] Seam 2 — KiteTower construction for the traversal field (from threatfield)
CLAIM: The sim's traversal field stamps ALL towers — including the deciding side's OWN towers and energy-drained towers — while live stamps only hostile towers with enough energy to fire.
LIVE: screeps-combat-decision/src/lib.rs:1946-1957 — kite_towers filter: "s.ownership == Ownership::Hostile && s.structure_type == StructureType::Tower && s.hits > 0 && s.energy >= TOWER_ENERGY_COST".
SIM: screeps-combat-agent/src/pathing.rs:57-62 — "world.towers.iter().filter(|t| t.is_alive() && t.pos.room_name() == room).map(|t| KiteTower { pos: t.pos })" — no owner filter, no energy gate, even though SimTower carries both fields (screeps-combat-engine/src/state.rs:68-77).
CONSEQUENCE: In sim self-play a defender's own movement detours around its own towers' falloff (a bogus whole-room penalty since towers stamp every tile), and in drain scenarios the sim keeps the tower traversal penalty after the towers run dry while live drops it — so drain-validated approach routes/timings (ADR 0031 #39 'advance once they're dry') diverge from live, and the sim's traversal field disagrees with its own decision-path field, whose kite_towers correctly gates on energy via the DTOs (agent lib.

### [H3] Seam 2 — where the threat-weighted matrix is actually used (from threatfield)
CLAIM: In the sim, threat-weighted traversal cost is folded into EVERY movement matrix (per-creep pathing and the shared system mover all members resolve through); live folds it only into the squad_manager's decide-searches matrix — the anchor mover and rover MovementSystem that actually execute live squad travel use unpriced matrices.
LIVE: screeps-ibex/src/military/formation.rs:298-315 — advance_virtual_pos's room_cb bakes only terrain walls over ScreepsCostMatrixDataSource (no threat); screeps-rover/src/screeps_impl.rs:164-192 has no threat layer; grep shows build_room_threat_field/THREAT_PATH_DIV appear in no live file except squad_manager.rs (fold at 3062-3082, used only for decide_squad_with_pathing at 3275/3294).
SIM: screeps-combat-agent/src/pathing.rs:414-421 — CombatWorldCostSource::from_world folds threat_cost_tiles into every room's matrix for resolve_moves_via_system (pathing.rs:549), the mover ALL managed-squad members resolve through (squad.rs:843 comment); the per-creep CombatCostSource does the same at pathing.rs:147-155.
CONSEQUENCE: ADR 0024 Stage 1 'safest route' is execution-real in the sim (members and travellers physically route around tower/enemy kill-zones) but selection-only live (kite/EV goal scoring sees the field, yet the anchor path and member move requests take the raw shortest route) — live squads march through exposure corridors the sim-validated runs avoided, so en-route attrition live is worse than anything the sim measured.

### [H4] SquadView.members / squad_heal / drain sustain (from squadview)
CLAIM: Live member heal_power is computed once and latched (only recomputed while it is 0), while the sim recomputes working HEAL parts every tick, so a damaged healer's heal capacity goes stale live but degrades correctly in sim.
LIVE: screeps-ibex/src/military/squad.rs:804-806 — "if member.heal_power == 0 { member.heal_power = creep.body().iter().filter(|p| p.part() == Part::Heal && p.hits() > 0).count() as u32; }" (never re-run once nonzero; squad_manager.rs:3151 copies m.heal_power into the view unchanged, unlike melee/ranged/dismantle which ARE re-derived per tick at 3122-3145)
SIM: screeps-combat-agent/src/squad.rs:686 — "heal_power: f.working_parts(Part::Heal) as u32" (per-tick recount of hits>0 HEAL parts from the DTO body)
CONSEQUENCE: After a healer's HEAL parts are chopped, live still reports full heal capacity to the kernel: squad_heal (decision lib.rs:2198) and the drain-stance sustain check (hold-standoff-while heal >= falloff tower dps), heal assignments, and the Lanchester engage sustain all overestimate — a live drain/engage holds a standoff the sim-validated policy would retreat from, until the healer dies. Sim never exercises the stale-input case.

### [H5] SquadView.members roster scope (from squadview)
CLAIM: Live builds member_views from the ENTIRE roster wherever each member stands (no room filter), while the sim excludes out-of-room members from the view — and the sim's REC-053 comment explicitly (and incorrectly) claims this matches live.
LIVE: screeps-ibex/src/military/squad_manager.rs:3114-3161 — member_views maps ctx.members with m.position (global, set from creep.pos() in squad.rs:793 regardless of room) with no target-room filter; in_room_any (3189) is used only for stall gating, not to scope the roster.
SIM: screeps-combat-agent/src/squad.rs:657-663 — "decision reflects the PRESENT in-room force (matching live...)" then ".filter(|&&id| in_objective_room(id))" excludes crossed members from member_views entirely (they only get travel/flee requests).
CONSEQUENCE: Every roster-derived kernel input diverges when any member is out of the target room (border crossing, trickle-in, retreat): live's centroid is dragged toward/into the neighbor room, squad_avg_hp_fraction (retreat trigger) counts absent members' HP, fragile_hits/squad_heal (lib.rs:2197-2198) count a healer rooms away as present sustain, and focus_damage_inputs (lib.rs:1965-1966) sums melee/ranged power of shooters that cannot fire. Sim validated engage/retreat/kite behavior only over the in-room

### [H6] Seam 4 — movement/mover config parity: max_shove_depth (from movement)
CLAIM: The sim's combat mover resolves shove chains at depth 3 while live runs the rover resolver at depth 10, and the sim constant's own doc falsely claims 3 is the live default.
LIVE: C:\code\screeps-ibex\screeps-ibex\src\features.rs:289 `max_shove_depth: 10` (PathingFeatures::default), applied at C:\code\screeps-ibex\screeps-ibex\src\pathing\movementsystem.rs:445 `system.set_max_shove_depth(pathing_features.max_shove_depth)`; rover's own default agrees: C:\code\screeps-ibex\screeps-rover\src\resolver.rs:8 `pub(crate) const DEFAULT_MAX_SHOVE_DEPTH: u32 = 10`
SIM: C:\code\screeps-ibex\screeps-sim-core\src\rover_driver.rs:29 `pub const DEFAULT_SHOVE_DEPTH: u32 = 3;` with the stale doc at line 37 "Resolver shove-chain depth (live default 3)", wired into MoverConfig::default (line 62) which combat uses verbatim via `combat_mover_config()` (C:\code\screeps-ibex\screeps-combat-agent\src\pathing.rs:511-513, whose doc claims it is "identical to the kernel default 
CONSEQUENCE: In exactly the crowded-formation/corridor cases the combat sim validates, live can cascade displacement chains up to 10 creeps deep where the sim's resolver refuses past 3 and books a denial (feeding the stuck ladder instead). Live formations get shoved/reshuffled in ways no sim run ever exercised, and sim-measured cohesion/denial rates do not predict live ones.

### [H7] Seam 5 (a) — defensive tower kernel vs boosted drain healers (from boostmodel)
CLAIM: decide_towers sizes its heal-gap commit and its anti-drainer skip through the same boost-blind heal_reaching, so a live boosted drain tank's heal is under-read up to 4x and the 'skip out-healed drainer' guard fails to trip; the tower harness only ever fields unboosted attackers.
LIVE: screeps-combat-decision/src/tower_fire.rs:247-270 — `heal = heal_reaching(hostiles, structures, pos)` then `if squad_dps.saturating_add(full_tower_damage) <= heal { skip }`, with heal_reaching (lib.rs:387-398) counting raw HEAL parts × 12; live DTOs from squad_combat.rs:173-185 carry no boost, so a T3 XLHO2 healer (48/part real) reads 12/part.
SIM: screeps-combat-eval/src/harness/tower_fire.rs has no boosted bodies (no BoostTier usage anywhere in the eval harness; roster.rs:149 / generate.rs:259 build SimBody::unboosted), so the measured combined-fire win and the drainer-bait refusal were only validated against unboosted heal.
CONSEQUENCE: Against a boosted PvP drain squad (the canonical MMO tower-drain attack), towers compute the target as killable (believed heal 1/4 of real), commit and dogpile an actually out-healed tank, and bleed stored energy — precisely the drainer bait the kernel was built to refuse. The failure only exists in the boosted regime the sim never runs.

### [H8] Seam 6 — tower behavior (from towers)
CLAIM: The live no-squad path prices a hostile's sustain from its OWN body's HEAL parts only, and its final fallback fires all towers at the weakest hostile with no net-damage check — both directly violating the kernel's heal_reaching-based hold-fire discipline the sim validated (never dogpile the out-healed).
LIVE: screeps-ibex/src/missions/tower.rs:379-388 (heal_per_tick summed from the candidate creep's own body parts only — adjacent/nearby hostile healers contribute nothing) and tower.rs:466-477 ("Fall back to weakest non-drainer hostile" — reached when no target has total_damage > heal and is_drain is false, e.g. an out-healing tank mid-room away from the edge (is_likely_tower_drain in military/damage.rs
SIM: screeps-combat-decision/src/lib.rs:387-414 (heal_reaching counts every hostile healer within heal range plus energized hostile towers) and tower_fire.rs:268-272 (`if squad_dps + full_tower_damage <= heal { continue }` — hold rather than feed a drainer), pinned by the `holds_fire_against_an_out_healed_drainer` test at tower_fire.rs:392-400.
CONSEQUENCE: A tank-plus-adjacent-healer pair standing mid-room (never exiting, so the drain-sawtooth tracker never confirms them — confirmation requires a leave/re-enter cycle at tower.rs:261-271) is scored killable (heal=0 for the HEAL-less tank), or falls to the weakest-hostile branch, and the towers bleed 10 energy per tower per tick at it indefinitely — precisely the unbounded drain the kernel refuses and the sim proved the kernel refuses. Live behaves strictly worse than the behavior the sim validated.

### [H9] Seam 7 — lifecycle/reconcile inputs: forming vs traveling classification (from lifecycle)
CLAIM: A squad released below full roster (uncontested trickle, win-or-stall fast path, or deploy-then-retreat quorum) is snapshot-classified `forming` live, while every harness driver classifies any departed squad `traveling` — live has no `departed` gate on forming and requires FULL roster for traveling.
LIVE: screeps-ibex/src/military/squad_manager.rs:1954 `let traveling = full_roster && !engaged_once && !in_target_room && has_members;` with full_roster = present >= requested (1953); forming_state (418) is `has_members && !engaged_once && requested_slots > 0 && present_count < requested_slots` — no departed condition. Sub-full release paths: 3427-3430 (`fast_path_allowed || ready_to_depart_gate(..) || 
SIM: screeps-combat-eval/src/harness/lifecycle.rs:671 `let forming = has_members && !engaged_once && !departed && present < n_slots;` and :558 `traveling = true;` the tick `ready_to_depart_gate` releases (same shape at 1021/1108, 1615/1876) — the harness that validated the travel-lease fixes puts every departed squad on the progress-gated travel path.
CONSEQUENCE: Live, a quorum-departed under-strength squad is lease-refreshed through the FORMING path — `forming_in_flight = forming` is unconditionally true (1908) — so it holds its slot with ZERO positional-progress gating for up to MAX_FORMING_BUDGET (3000t) instead of the sim-validated travel path (progress-gated, MAX_TRAVEL_BUDGET 1000t). Worse, `departed_at` is only stamped when `traveling` (1959-1960), so when such a squad finally GaveUps, the R22 never-departed circuit breaker (2151-2166) counts a sq

## MEDIUM (24)

### [M0] combat-dto-construction/squad-member-view-population (from dto)
CLAIM: The live SquadView.members is built from the ENTIRE roster (including members still at home spawns or mid-travel, and unspawned members with pos None), while the sim builds member views from the in-objective-room subset only — and the sim code comments falsely assert live parity.
LIVE: screeps-ibex/src/military/squad_manager.rs:3114-3161 — `ctx.members.iter().map(|m| SquadMemberView { ... pos: m.position ... })` with no room filter; the full-roster view feeds decide_squad_with_pathing (:3297) and present_force_wins_or_stalls (:3407).
SIM: screeps-combat-agent/src/squad.rs:655-679 — `let living = self.members.iter().filter(|&&id| in_objective_room(id))...` with the comment "Out-of-room members are excluded from the combat brain... (matching live, where crossed members HOLD and only the in-room subset runs decide_squad)" — which is not what live does.
CONSEQUENCE: Live engage/retreat balance (assess_engage), avg-HP retreat fraction, centroid, and the win-or-stall deploy gate count melee/ranged/heal power of members that are rooms away (e.g. a mid-fight refill spawning at home), so live can hold an engagement or pass present_force_wins_or_stalls on force that is not physically present — a configuration the sim never evaluated because its kernel inputs only ever contain the in-room force. The centroid can also land between rooms, skewing focus-distance and 

### [M1] combat-dto-construction/member-heal-power-staleness (from dto)
CLAIM: Live member heal_power is computed once (only when the cached value is 0) and never re-read, so destroyed HEAL parts keep counting at full strength for the rest of the creep's life; the sim recomputes working HEAL parts every tick.
LIVE: screeps-ibex/src/military/squad.rs:804-806 — `if member.heal_power == 0 { member.heal_power = creep.body().iter().filter(|p| p.part() == Part::Heal && p.hits() > 0).count() as u32; }` inside the per-tick member sync (position/hits ARE refreshed each tick right above at :793-802, making the latch an outlier).
SIM: screeps-combat-agent/src/squad.rs:686 — `heal_power: f.working_parts(Part::Heal) as u32` rebuilt from the current body every tick (CombatCreepDto::working_parts counts hits>0 parts, screeps-combat-decision/src/lib.rs:141-145).
CONSEQUENCE: After a healer takes body damage (HEAL parts are destroyed front-to-back last, but tough/move losses precede them — once HEAL parts start dying live keeps the pre-damage count), assess_engage's our-heal term, assign_heals capacity, and the retreat decision all overestimate squad sustain; live fights on with heal the squad no longer has, in states the sim (which degrades heal correctly) would have retreated from.

### [M2] Seam 2 — own-rampart passability in the movement matrix (from threatfield)
CLAIM: The sim marks EVERY structure — including the deciding side's own ramparts — as an impassable blocker, while live leaves own/public ramparts walkable; combined with cover-zeroing this makes the live rampart-anchoring behavior unreachable in sim.
LIVE: screeps-rover/src/screeps_impl.rs:174-180 — "StructureObject::StructureRampart(r) => { if r.my() || r.is_public() { None } else { Some((u8::MAX, ...)) } }" (own ramparts passable).
SIM: screeps-combat-agent/src/pathing.rs:131-137 (CombatCostSource) and 358-364 (CombatWorldCostSource) — every alive structure is pushed into blockers/u8::MAX with no kind or ownership filter.
CONSEQUENCE: A sim creep can never stand on or path across its own rampart tile, so the T-DEF-1 cover behaviors (score_tile's covered→0 branch at kite.rs:492-500, the survival veto relaxation, the cover corridor) can never be selected by the sim's scored search — the tiles are walls in its matrix. Live defenders will anchor on ramparts under fire in a configuration no sim run has ever exercised or tuned.

### [M3] Seam 2 — KiteThreat power basis (boosted vs unboosted stamp) (from threatfield)
CLAIM: The sim's traversal field stamps BOOST-MULTIPLIED creep power while live stamps unboosted working-part count × constant, contradicting the ThreatField v1 'stamps unboosted' contract.
LIVE: screeps-combat-decision/src/lib.rs:1934-1939 — kite_threats: "attack_power: working(Part::Attack) * screeps_combat_engine::constants::ATTACK_POWER" (alive-part count, no boost term); kite.rs:40-42 documents "v1 stamps unboosted output".
SIM: screeps-combat-agent/src/pathing.rs:53-54 — "attack_power: c.body.attack_power()" → SimBodyCombat (body_combat.rs:31-36) → effective_power (screeps-sim-core/src/body.rs:124-131): "power += base as f64 * p.boost.action_mult()".
CONSEQUENCE: Against boosted hostiles the sim's per-tile traversal penalty is up to 4x live's (hitting THREAT_PATH_CAP much closer to the threat), so routes and DIV/CAP tuning validated in boosted sim scenarios don't transfer to live, and within the sim itself the traversal field disagrees with the decision-path field (built from DTOs via the shared unboosted kite_threats). Note this is NOT the documented threatmap flat-x4 approximation — that is a different module; this is the shared-seam traversal stamp it

### [M4] Seam 2 — room-scoping of decide-path threat inputs and matrices (from threatfield)
CLAIM: Live's room_callback ignores the requested room and always returns the TARGET room's threat-folded matrix/layers, while decide_squad_with_pathing keys its searches on the CENTROID's room; the sim callback is room-correct but its SimView hostiles are world-scoped — each side mis-scopes differently and neither exercises the other's condition.
LIVE: screeps-ibex/src/military/squad_manager.rs:3294 — "let mut room_cb = |_r: RoomName| Some(matrix.clone())" (target_room matrix for any room; layers cached per target_room at 3267-3278), vs lib.rs:2126 "let room = centroid.room_name()" and ThreatField::raw_at being coordinate-only/room-blind (kite.rs:105-107).
SIM: screeps-combat-agent/src/squad.rs:779 — "&mut |r| build_combat_matrix(world, r, self.owner)" (per-room-correct), but SimView::from_world at screeps-combat-agent/src/lib.rs:117-127 classifies ALL living non-owner creeps as hostiles with no room filter, so distant-room hostiles stamp phantom threat at their (x,y) into the centroid room's decision field.
CONSEQUENCE: Live: a squad whose centroid sits outside the target room (en route, border kiting) prices centroid-room tiles with the target room's walls and threat stamps — a wrong-room matrix the sim never feeds the kernel; sim (multi-room worlds): out-of-room hostiles bleed into the in-room threat/kite scoring live would never see. Kite/engage goal picks near room borders can differ between the sides under identical tactical situations.

### [M5] enemy_stalled / structure_stalled trackers (from squadview)
CLAIM: On a tick with no member in the target room, live CLEARS both stall trackers (streak and seed dropped) while the sim merely FREEZES them (early-return skips the update, state retained), so the streaks survive a full room exit in sim but not live.
LIVE: screeps-ibex/src/military/squad_manager.rs:3207-3213 and 3224-3234 — else-branch of in_room_any: "forming_progress.enemy_stall.remove(&obj_id)" / "forming_progress.structure_stall.remove(&obj_id)"
SIM: screeps-combat-agent/src/squad.rs:665-681 — "if living.is_empty()" (no in-room members) returns before the stall blocks at 705-760; self.stall_ticks/prev_enemy_hits/structure_stall_ticks persist across the exit and resume accrual on re-entry.
CONSEQUENCE: A squad that fully leaves the room (retreat across the border, recovery dip) restarts its 40-tick no-headway streak from zero live but keeps it in sim: sim trips the stalemate disengage up to ENEMY_STALL_TICKS sooner, and a live squad oscillating in/out of the room can never latch enemy_stalled at all — the sim-validated stalemate-valve timing does not reproduce live. (The per-tick step itself, advance_enemy_stall squad_manager.rs:476-494 vs squad.rs:717-733, IS identical: reset-on-decrease any 

### [M6] decide_squad_with_pathing room_callback + shared layers (from squadview)
CLAIM: Live's room callback ignores the requested room name and always returns the TARGET room's cost matrix (and passes target-room PositionLayers), while the sim builds the matrix for whatever room is actually requested.
LIVE: screeps-ibex/src/military/squad_manager.rs:3294 — "let mut room_cb = |_r: RoomName| Some(matrix.clone());" (matrix/layers built for target_room at 3267-3277)
SIM: screeps-combat-agent/src/squad.rs:778-779 — decide_squad_with_pathing(..., &mut |r| build_combat_matrix(world, r, self.owner), ...) honors the requested room.
CONSEQUENCE: Combined with the unfiltered live roster (finding 2), the centroid can sit outside the target room; decide_squad_with_pathing keys its search on centroid.room_name() (lib.rs:2126) yet live hands it the target room's walls/threat matrix and target-room-indexed layers (the 50x50 field arrays are room-agnostic by x,y), so kite/engage/kernel tile scoring runs over the wrong room's terrain and threat data. Sim never constructs this mismatch (single-room world, per-room callback).

### [M7] SquadMemberView.damage_taken_last_tick (from squadview)
CLAIM: The sim hardcodes damage_taken_last_tick to 0 for every member, while live feeds the real per-tick hits delta.
LIVE: screeps-ibex/src/military/squad.rs:798-802 — "member.damage_taken_last_tick = prev_hits - creep.hits()" (real delta), consumed at squad_manager.rs:3153
SIM: screeps-combat-agent/src/squad.rs:694 — "damage_taken_last_tick: 0,"
CONSEQUENCE: The kernel's heal assignment risk term (lib.rs:1811 risk_at = damage_taken_last_tick.max(predicted incoming)) has one of its two inputs permanently zero in sim: live reactively heals members hit by sources the threat model missed (unseen melee, unmodeled boost damage), a behavior branch the sim never exercises or validates — heal-triage tuning done in sim runs on predicted-incoming only.

### [M8] movement cost-matrix threat fold (room_callback input) (from squadview)
CLAIM: The sim's combat-matrix threat fold prices ALL alive towers (including the squad's OWN and energy-drained ones) and applies no friendly-rampart cover, while live folds only hostile energized towers and zeroes maintained-own-rampart tiles (T-DEF-1).
LIVE: screeps-combat-decision/src/lib.rs:2012-2017 — build_room_threat_field uses ThreatField::build_covered with friendly_rampart_cover, over kite_towers (lib.rs:1947-1955: ownership==Hostile && energy >= TOWER_ENERGY_COST); threaded into the live matrix at squad_manager.rs:3271-3277.
SIM: screeps-combat-agent/src/pathing.rs:57-63 — room_threat_field: "world.towers.iter().filter(|t| t.is_alive() && t.pos.room_name() == room)" (no owner filter, no energy gate) and "ThreatField::build(&threats, &towers)" (no cover).
CONSEQUENCE: In defense/owned-room beds the sim's traversal pricing penalizes tiles around the squad's own towers and treats the own rampart line as a kill-zone instead of the safe corridor, while live does the opposite — sim-tuned positioning/kite results on beds with friendly towers or ramparts do not transfer to live (T-DEF-1 was added on the live/shared side only; the sim matrix source was not updated).

### [M9] SquadTacticParams selection (tactics input to decide_squad_with_pathing) (from squadview)
CLAIM: Live re-selects the tactic profile every tick via classify_objective + decide_strategy — with has_structures computed from ALL structures including own/neutral ones — while ManagedSimSquad's tactics are fixed at construction (default() unless a bed overrides), so live's mid-fight profile flips and the misclassification are sim-unexercised.
LIVE: screeps-ibex/src/military/squad_manager.rs:3288-3290 — "let class = classify_objective(formation, !structures.is_empty(), !hostiles.is_empty());" where structures = structure_to_dto over rd.get_structures().all() (squad_manager.rs:3001-3004: every structure, Mine/Neutral included); tactics recomputed per tick.
SIM: screeps-combat-agent/src/squad.rs:502 — "tactics: SquadTacticParams::default()" set once in new(), used unchanged at squad.rs:778 for the whole run (no sim caller invokes decide_strategy; eval beds pass a single fixed profile via with_tactics).
CONSEQUENCE: Two effects: (a) since any room with a road/container/own structure has has_structures=true, a live squad that clears the creeps flips to the breach profile (approach_coef 1, incumbency 4) even when no hostile structure exists — e.g. defense in an owned room — a weight regime the beds that validated open combat never ran mid-fight; (b) the untuned KernelParams::default() (a2-i3) that ManagedSimSquad fields by default is a profile live can never select (the registry always returns open_combat/bre

### [M10] Seam 4 — movement/mover config parity: friendly_creep_distance (from movement)
CLAIM: Tier-1 friendly-avoid repricing uses a 15-tile proximity radius live but the rover default of 5 in the sim's combat mover.
LIVE: C:\code\screeps-ibex\screeps-ibex\src\features.rs:290 `friendly_creep_distance: 15`, applied at C:\code\screeps-ibex\screeps-ibex\src\pathing\movementsystem.rs:446 `system.set_friendly_creep_distance(pathing_features.friendly_creep_distance)`
SIM: C:\code\screeps-ibex\screeps-sim-core\src\rover_driver.rs:65 `friendly_creep_distance: screeps_rover::DEFAULT_FRIENDLY_CREEP_DISTANCE` = 5 (C:\code\screeps-ibex\screeps-rover\src\movementsystem.rs:375 `pub const DEFAULT_FRIENDLY_CREEP_DISTANCE: u32 = 5`)
CONSEQUENCE: A stuck live traveller (out-of-room squad member, rejoiner, retreat-to-home mover — engaged members are transparent on both sides) prices detours around friendlies within 15 tiles; the sim only within 5. Live escalation repaths take much wider detours than any sim-validated trajectory, changing arrival timing and route choice for the default-ladder movers the combat harness exercises.

### [M11] Seam 4 — engaged-ladder scoping: Retreating members (from movement)
CLAIM: The sim applies the squadmate-transparent engaged ladder to every in-room member uniformly (Retreating included); live applies it only when the squad is Engaged, so live Retreating members keep the default ladder with reachable friendly-avoid tiers.
LIVE: C:\code\screeps-ibex\screeps-ibex\src\jobs\squad_combat.rs:1229 `Engaged::execute_decide_movement(creep, creep_pos, orders, false, tick_context)` (the Retreating/Formation arm passes engaged=false) and squad_combat.rs:902-904 `if engaged { mr.stuck_thresholds(StuckThresholds::engaged()); }`
SIM: C:\code\screeps-ibex\screeps-combat-agent\src\squad.rs:876-881 "IN-ROOM members are the squad brain's choreography ... so applied uniformly" → `req = req.with_stuck_thresholds(engaged_stuck_thresholds());` unconditionally for every in-room member's request, in any SquadOrderState
CONSEQUENCE: A live withdrawing member stuck ≥2 ticks behind a squadmate reprices tier-1/1b friendly-avoid and detours around its own heal cluster mid-retreat — precisely the heal-collapse pathology (received heal ~800→~300/t) the engaged ladder was built to prevent — while sim retreats are structurally immune. Sim-validated retreat survival/cohesion overstates live.

### [M12] Seam 4 — flee request semantics (from movement)
CLAIM: Flee requests diverge on three knobs: sim flees can never swap (live flee default allow_swap=true), sim support/healer flees run at Normal priority (live flee is High for everyone), and the sim stamps the numeric priority bid on flees while live confines bids to MoveTo.
LIVE: C:\code\screeps-ibex\screeps-rover\src\movementrequest.rs:182-185 flee defaults `priority: MovementPriority::High, allow_shove: false, allow_swap: true`, used unmodified by the live squad flee at C:\code\screeps-ibex\screeps-ibex\src\jobs\squad_combat.rs:912; squad_combat.rs:870-871 "the binding member's numeric R_O bid replaces its enum tier on the MoveTo (only — flee keeps its own semantics)"
SIM: C:\code\screeps-ibex\screeps-combat-agent\src\pathing.rs:314-326 builds Flee at `priority: MovementPriority::Normal`; squad.rs:851-853 upgrades to High only for combat-bodied members and squad.rs:857-858/864-865 stamps the bid and forces shove=false on flees; the kernel driver then sets `mr.allow_shove(req.shove).allow_swap(req.shove)` (C:\code\screeps-ibex\screeps-sim-core\src\rover_driver.rs:379
CONSEQUENCE: In a packed retreat corridor a live fleeing creep can still swap tiles with a teammate (the escape valve) and a live fleeing healer outranks Normal traffic; the sim's fleeing creeps can do neither. Retreat throughput and who-yields-to-whom during disengage differ from what REC-055 claims is aligned, so sim drain/retreat outcomes don't transfer.

### [M13] Seam 4 — per-creep mover threat pricing (ADR 0024) (from movement)
CLAIM: The sim's system mover prices the ADR 0024 threat field into every per-creep path, but the live rover MovementSystem's cost source has no threat layer — live threat folding exists only in the squad-manager's separate kite/anchor planning matrix.
LIVE: C:\code\screeps-ibex\screeps-ibex\src\pathing\movementsystem.rs:435 `CostMatrixSystem::new(&mut data.cost_matrix_cache, Box::new(ScreepsCostMatrixDataSource))` — the game-API source (screeps-rover\src\screeps_impl.rs:245-301) stamps creeps/structures/SK-aggro but no threat cost; the only live threat fold is C:\code\screeps-ibex\screeps-ibex\src\military\squad_manager.rs:3062-3083 `build_target_mat
SIM: C:\code\screeps-ibex\screeps-combat-agent\src\pathing.rs:407-420 computes `threat_cost_tiles(&room_threat_field(...))` per room and CombatWorldCostSource::get_structure_costs stamps it into the matrix (pathing.rs:433-435) consumed by `resolve_moves_via_system` for every member step
CONSEQUENCE: Sim members' executed step paths route around tower/hostile kill-zones (add up to +8/tile); live rover-resolved paths (approach into the room, rejoin, retreat, any MoveTo) ignore threat entirely and can thread straight through kill-zones the sim's validated trajectories avoided — en-route pick-off risk live that sim soaks never measured.

### [M14] Seam 4 — resolver pass scope: per-squad vs global (from movement)
CLAIM: Live resolves ALL movement (every squad + all economy traffic) in one MovementSystem pass, while each sim squad runs its own resolve_moves_via_system pass with its own cache, in which other squads' members are registered as shoveable idles rather than their real Immovable holds / High movers.
LIVE: C:\code\screeps-ibex\screeps-ibex\src\pathing\movementsystem.rs:437-548 — one `system.process(&mut external, movement_data)` per tick over the whole bot's MovementData, with unrequested military creeps injected as `MovementPriority::Immovable` holds (lines 404-411)
SIM: C:\code\screeps-ibex\screeps-combat-agent\src\squad.rs:895-899 and 416-421 — each ManagedSimSquad/SimSquad calls `resolve_moves_via_system_with(world, self.owner, &move_reqs, &mut self.move_cache, ...)` for its members only; the kernel driver then registers any unrequested same-owner creep (i.e. another squad's members) as a SHOVEABLE idle (C:\code\screeps-ibex\screeps-sim-core\src\rover_driver.rs
CONSEQUENCE: Cross-squad and squad-vs-economy contention (priority ordering, shove/swap chains, denial bookkeeping across request sets) is never exercised in the sim: a sim squad plans through a neighboring squad's held tiles as displaceable Low idles, where live those tiles are Immovable holds that deny and stall it. Multi-squad live behavior (e.g. two squads staging through one corridor) has no sim coverage.

### [M15] Seam 5 (b) — own boosted creeps (ADR 0041 P3) under-reported to the tactical kernels (from boostmodel)
CLAIM: Live member views feed the shared kernels raw-part-count powers ignoring p.boost(), while the composition optimizer sized the boosted squad by DIVIDING part requirements by the tier multiplier — so a live T3 squad fields 1/4 the parts and then self-assesses at 1/4 its real dps/heal in decide_squad/assess_engage/present_force_wins_or_stalls; the sim never fields boosted own bodies at all.
LIVE: screeps-ibex/src/military/squad_manager.rs:3126-3143 — member view melee/ranged/dismantle power = raw alive part counts × base constants (`atk * ATTACK_POWER` etc., no boost read); screeps-ibex/src/military/squad.rs:804-806 heal_power = raw HEAL count; versus the sizing side screeps-combat-decision/src/composition.rs:505-518 `boost_scaled` (parts ÷ m at the chosen tier) and screeps-ibex/src/operat
SIM: screeps-combat-agent/src/squad.rs:686-692 builds SquadMemberView the same raw-count way, but no sim/eval scenario ever fields a boosted own body (roster.rs:149, generate.rs:259/918/1194 all SimBody::unboosted) — so in every validating run raw counts equal true output and the mismatch is unexercised.
CONSEQUENCE: The moment boost_military is enabled (the whole point of the shipped P3 layer), a tier-m squad reaches the proceed gate and the in-room Lanchester retreat gate at 1/m of the strength it was sized and boosted to have: present_force_wins_or_stalls holds/refuses deployment of squads the tier optimizer proved winnable, the retreat band trips on winnable fights, drain_sustains under-reads the tank's soak heal 4x so drain stances veto themselves, and heal assignments cap expected_heal at 1/4 of real t

### [M16] Seam 5 (b)-adjacent — live heal_power latched once vs sim per-tick recompute (from boostmodel)
CLAIM: Live SquadMemberView.heal_power is initialized once when the member is first seen alive and never recomputed, so destroyed HEAL parts keep counting; the sim adapter recomputes working HEAL parts every tick from per-part hits.
LIVE: screeps-ibex/src/military/squad.rs:804-806 — `if member.heal_power == 0 { member.heal_power = creep.body()...filter(hits() > 0).count() }` inside PreRunSquadUpdateSystem (doc: 'Initialize heal_power from body parts (once, when first seen alive)'), consumed by squad_manager.rs:3149 and assess_engage's our_heal (lib.rs:1449).
SIM: screeps-combat-agent/src/squad.rs:686 — `heal_power: f.working_parts(Part::Heal) as u32` rebuilt every tick from the DTO's live per-part hits, so sim-validated behavior includes heal degradation as parts die.
CONSEQUENCE: A mauled healer whose HEAL parts are destroyed still reports full heal capacity to the live kernel: assess_engage's bleed-out veto (`unkillable_dps + tower_dps > our_heal`) fails to trip so the squad holds a fight it is now bleeding out of, drain_sustains over-credits the soak, and the heal-assignment kernel assigns heals (expected_heal > 0) to a healer that can no longer heal — all divergent from the per-tick-degrading inputs the sim validated against.

### [M17] Seam 6 — tower behavior (from towers)
CLAIM: The live no-squad (passive-base) tower path never calls the shared decide_towers kernel at all — it runs a forked legacy heuristic, while every sim validation of tower fire (kernel unit tests incl. the squad_focus=None fallback, and the U7 harness) runs decide_towers; the sim's only non-kernel tower model is scripted nearest-enemy fire, a third behavior that matches neither.
LIVE: screeps-ibex/src/missions/tower.rs:375-478 — "No defending squad: the passive-base defensive path (drain-sawtooth / probe / chip)" builds its own hostile_infos/best_target/is_drain logic and issues `tower.attack(target)` for ALL towers on one target (lines 462-465), never constructing DTOs or calling decide_towers; the kernel path (line 353) is reached only when find_squad_focus_for_room returns S
SIM: screeps-combat-decision/src/tower_fire.rs:374-387 (`no_squad_fallback_targets_highest_threat_killable` validates the kernel's None-focus passive behavior live never runs) and tower_fire.rs:136-137 (kernel docs call None-focus "the common passive-base case"); screeps-combat-eval/src/harness/tower_fire.rs:187-199 (`towers_alone_intents` = nearest-enemy scripted fire, described as "the passive-base b
CONSEQUENCE: The most common live tower situation (hostiles present, no managed squad defending — every invader/harass event in a passive room) executes target selection, hold-fire, and commit-sizing logic that no sim ever measured: all towers dogpile one target with no gap-sizing (N×600 into a 150-HP scout = pure energy overkill the kernel's minimal-commit prevents), a threat order (dangerous-part flag then min-hits) different from the kernel's threat_value ranking, and no rampart-shield exclusion. The U7 "

### [M18] Seam 6 — tower behavior (from towers)
CLAIM: Recently introduced asymmetry (Wave A, ab692bd): CLAIM and WORK parts count as 'dangerous' in the live legacy tower target order (the T-DEF-4 'half'), but the shared threat_value used by the kernel's redirect ordering on the squad-defended path scores CLAIM and WORK as zero — so exactly when a squad is defending, a controller-attacker or dismantler is the LAST tower target instead of a priority one.
LIVE: screeps-ibex/src/missions/tower.rs:402-412 — comparator marks `Part::Attack | Part::RangedAttack | Part::Work | Part::Claim` dangerous, with the comment "CLAIM counts (D27/T-DEF-4 half)"; this ordering exists only on the no-squad legacy path.
SIM: screeps-combat-decision/src/lib.rs:416-423 — threat_value = ATTACK·30 + RANGED·10 + HEAL·12 only (no WORK, no CLAIM term); tower_fire.rs:231-237 sorts the kernel's redirect targets by this value descending, so a pure CLAIM/WORK creep sorts to the bottom (threat 0).
CONSEQUENCE: In a room with a defending squad (the kernel path), a declaimer walking to the controller or a dismantler team chewing a rampart is deprioritized behind any creep with a single ATTACK part — contradicting design item T-DEF-4 (docs/design/0008a-combat-tactics.md:480-485: CLAIM priority 'above even an armed breacher', because one landed attackController blocks safe-mode for 1000 ticks) and the dismantle-threat operator decision. The two live paths now disagree with each other, and the sim only eve

### [M19] Seam 6 — tower behavior (from towers)
CLAIM: The live DTO seam drops boost data entirely (CombatBodyPart has no boost field), so decide_towers' heal_reaching prices a boosted hostile healer at unboosted HEAL_POWER — up to 4x under — and the kernel's anti-drain hold-fire gate never trips against boosted drainers; the sim only ever feeds the tower kernel unboosted bodies, so this bad-input regime is never exercised.
LIVE: screeps-ibex/src/jobs/squad_combat.rs:173-185 — creep_to_dto maps body to `CombatBodyPart { part, hits }` (boost discarded); contrast the live legacy path which DOES estimate boosts (tower.rs:384: `if p.boost().is_some() { 48.0 } else { 12.0 }`), so the kernel path is strictly boost-blinder than the legacy path it replaced.
SIM: screeps-combat-decision/src/lib.rs:119-122 (CombatBodyPart carries part+hits only) with heal_reaching at lib.rs:391-401 using flat HEAL_POWER/RANGED_HEAL_POWER; screeps-combat-eval/src/harness/tower_fire.rs:50,67,326 — every U7 body is `SimBody::unboosted`, so no sim scenario measures decide_towers against boosted heal.
CONSEQUENCE: A T3-boosted drainer self-healing 48/part is modeled at 12/part: `squad_dps + full_tower_damage <= heal` (tower_fire.rs:270) evaluates false when the real heal exceeds our fire, so on the squad-defended path the towers commit and bleed energy into an actually-out-healed target — the exact failure mode the hold-fire discipline was built and sim-validated to prevent. This is distinct from the accepted threatmap flat-x4 note (that is the enemy-boost OVER-estimate in threat sizing; this is the tower

### [M20] Seam 7 — lifecycle/reconcile inputs: forming_in_flight (from lifecycle)
CLAIM: Live feeds the kernel a degenerate `forming_in_flight = forming` (constant true while forming), while the harness computes a real queued/in-flight signal — so the kernel's lapse-while-forming-with-nothing-in-flight path validated offline can never occur live, and the live constant-true feed was never validated offline.
LIVE: screeps-ibex/src/military/squad_manager.rs:1908 `let forming_in_flight = forming;` (comment: an unfilled slot is re-queued by Phase B every tick, so it is assumed in-flight whenever forming).
SIM: screeps-combat-eval/src/harness/lifecycle.rs:536 (also 992, 1594) `let forming_in_flight = !completing.is_empty() || !syncing.is_empty() || any_queued;` fed as `forming_in_flight: forming && forming_in_flight` (693/1136/1898).
CONSEQUENCE: A live forming squad whose spawn genuinely cannot be queued (dead/blocked spawns, slots_to_spawn empty) still refreshes its lease every tick until the 3000-tick MAX_FORMING_BUDGET or the economic give-up, whereas the harness-validated behavior lets the base +400 lease lapse in that gap. The live Phase-B assumption ('unfilled slot is always re-queued') is baked into the input instead of measured, so the two sides drive different kernel branches for the same world state.

### [M21] Seam 7 — lifecycle/reconcile inputs: D28 vacuous_clear (from lifecycle)
CLAIM: The live-default-ON D28 vacuous-clear input (live-visible empty room resolves without engagement) is fed `false` by every one of the six harness lifecycle drivers — no harness flow ever exercises the vacuous Resolve, only kernel unit tests do.
LIVE: screeps-ibex/src/military/squad_manager.rs:2049-2055 — `vacuous_clear` = objective room in `game::rooms()` AND cached-lazily-refreshed same-tick DTO (room/data.rs:724-732 refreshes when `game::time() != last_updated`) shows `hostile().is_empty()`.
SIM: screeps-combat-eval/src/harness/lifecycle.rs:688, 1131, 1893, 2374, 2683, 2926 — all six ReconcileSnapshot constructions hardcode `vacuous_clear: false` (comments: 'the pre-D28' behavior / 'not exercised').
CONSEQUENCE: The recently introduced (§7.2a) resolve-without-engagement terminal — including its interactions the kernel tests cannot see (withdraw-as-clean-win feeding Reassign's `withdraw_old`, racing the forming/travel refresh, or a stale-visibility flap re-fielding after a vacuous withdraw) — runs live-only with zero harness soak; a regression in the D28 gate would pass the whole lifecycle eval suite.

### [M22] Seam 7 — lifecycle/reconcile inputs: retreat_budget_exhausted / stall-aware retreat clock (from lifecycle)
CLAIM: The REC-003 retreat force-abort input, including the NEW stall-aware `retreat_clock_holds` extension (clock keeps running on Engaged ticks while the enemy-HP stall streak is latched), is fed `false` by every harness driver — the composed manager-side clock is validated only by isolated unit tests.
LIVE: screeps-ibex/src/military/squad_manager.rs:472-474 `fn retreat_clock_holds(state_retreating, stalemate_latched) { state_retreating || stalemate_latched }`; 1976-1988 stalemate latch from `enemy_stall` streak >= ENEMY_STALL_TICKS(40) and `retreating_since` clock vs MAX_RETREAT_BUDGET=600 (284).
SIM: screeps-combat-eval/src/harness/lifecycle.rs:702, 1147, 1907, 2388, 2697, 2940 — every driver hardcodes `retreat_budget_exhausted: false` ('not exercised by this driver'); the harness has no MAX_RETREAT_BUDGET constant at all (its const block at 267-278 stops at MAX_TRAVEL_BUDGET).
CONSEQUENCE: The period-2 Engaged/Retreating probe-bounce zombie the FU2 fix targets — the clock accruing across the bounce, the streak advancing only on engaged ticks (advance_enemy_stall, 476+), and the terminal dominating the in-room focus-refresh through the full retire+mark_unwinnable flow — has never run end-to-end in any offline scenario; live is the first integration test of the interaction between the enemy_stall tracker (also consumed by Phase B's `enemy_stalled` kite input, 3207-3213) and the Phas

### [M23] Seam 7 — lifecycle/reconcile inputs: forming_budget_remaining (economic give-up) (from lifecycle)
CLAIM: Live folds the ADR 0042/0043 ECONOMIC forming give-up (R_net rate vs roster burn, 20-reconcile streak, safe-mode skip) into `forming_budget_remaining`; every harness driver computes it as the pure MAX_FORMING_BUDGET clock — the economic early-abandon path is never exercised offline.
LIVE: screeps-ibex/src/military/squad_manager.rs:1919-1947 — `economic_giveup` via `should_abandon_forming(forming_objective_rate_milli, forming_burn_rate_milli, 0)` latched at FORMING_ABANDON_STREAK=20 (261), then 1947 `let forming_budget_remaining = budget_clock_remaining && !economic_giveup;`.
SIM: screeps-combat-eval/src/harness/lifecycle.rs:673 (also 1110, 1878, 2380, 2689, 2932) `let forming_budget_remaining = tick.saturating_sub(gen_start) < MAX_FORMING_BUDGET;` — clock only, no economic term.
CONSEQUENCE: Live a forming squad can lose its lease ~20 reconciles in, driven by market/economy inputs (roster cost, objective completed-rate) the lifecycle harness never constructs — so the harness-proven forming-churn envelope (give up only at 3000t) does not describe live, and a mispriced `forming_objective_rate_milli` would produce field/abandon churn no offline suite can catch.

## LOW (9)

### [L0] combat-dto-construction/damage-taken-input (from dto)
CLAIM: The sim always feeds damage_taken_last_tick = 0 while live feeds the real prev_hits - hits delta, so the heal-assignment's reactive risk term (`risk_at = max(damage_taken_last_tick, predicted)`) is a live-only input path the sim never exercises.
LIVE: screeps-ibex/src/military/squad.rs:797-802 — `member.damage_taken_last_tick = prev_hits - creep.hits()` each tick; consumed at squad_manager.rs:3154 into SquadMemberView.
SIM: screeps-combat-agent/src/squad.rs:694 — `damage_taken_last_tick: 0,` (hardcoded); the kernel's risk term screeps-combat-decision/src/lib.rs:1811 `m.damage_taken_last_tick.max(incoming_damage_at(...))` therefore only ever sees the predicted half in self-play.
CONSEQUENCE: Live heal assignments can be pulled toward whoever took a burst last tick (including tower alpha or damage the ThreatField's prediction disagrees with), producing heal distributions self-play never validated; any tuning of assign_heals against the harness silently assumed the reactive term is dead.

### [L1] combat-dto-construction/hostile-power-creeps-excluded (from dto)
CLAIM: Hostile PowerCreeps never enter the combat DTOs on any live branch (both the cached CreepData and the live-visible fallback use creep-only find constants; there is no HOSTILE_POWER_CREEPS/power_creep read anywhere in the combat adapter path), and the sim has no power-creep concept, so the kernel is both blind to them live and untested against them.
LIVE: screeps-ibex/src/room/data.rs:1012 CreepData::new uses `room.find(find::CREEPS, None)` (power creeps are a separate find constant); squad_manager.rs:3017 LiveVisible branch uses `find(find::HOSTILE_CREEPS, None)`; grep for POWER_CREEPS/power_creep across squad_combat.rs, squad_manager.rs, and room/data.rs returns nothing.
SIM: screeps-combat-agent/src/lib.rs:113-121 — SimView hostiles come solely from world.movement.creeps (SimCreep); the engine models no power creeps, so no scenario ever presents one to decide_squad/ThreatField.
CONSEQUENCE: On MMO, an enemy operator power creep in the target room contributes zero to the ThreatField, engage balance, focus selection, and the uncontested classifier (a room defended only by a power creep classifies uncontested → rally AT the target) — live-only blind spot the sim can neither reproduce nor regress-test.

### [L2] SquadView.engage_objective (from squadview)
CLAIM: Live hardcodes engage_objective to Destroy while the sim driver supports and tests Hold (with_intent), so the kernel's Hold branch (w_close zeroing, stall-ignore, pin-at-standoff) is sim-validated but live-unreachable.
LIVE: screeps-ibex/src/military/squad_manager.rs:3248 — "engage_objective: screeps_combat_decision::EngageObjective::Destroy," (comment: Hold 'is for a future pin/harass objective')
SIM: screeps-combat-agent/src/squad.rs:554-557 — with_intent(intent) sets self.intent, fed to the view at squad.rs:770.
CONSEQUENCE: One-sided coverage rather than a live bug: every live squad takes the Destroy stalemate-disengage path; any behavior proven in Hold-intent beds (deliberate pin, harass standoff) has no live producer, and a live Harass-class objective silently runs Destroy semantics including the enemy_stalled retreat.

### [L3] Seam 4 — formation/travel priority tiers (from movement)
CLAIM: Live formation-slot and rally-travel moves run at High priority with the numeric bid; the sim's formation-squad slot moves are plain Normal with no bid.
LIVE: C:\code\screeps-ibex\screeps-ibex\src\jobs\squad_combat.rs:1326-1333 `apply_squad_move_priority(&mut mr, MovementPriority::High, bid)` + engaged ladder for formation slots (same at 369-373, and rally travel at 290-292)
SIM: C:\code\screeps-ibex\screeps-combat-agent\src\squad.rs:402-405 `SimMoveRequest::move_to(member_id, target, range).with_stuck_thresholds(engaged_stuck_thresholds())` — priority left at the Normal default (screeps-sim-core\src\rover_driver.rs:114), no `.with_priority_value`; sim travellers (squad.rs:641-645) likewise carry only the bid at Normal
CONSEQUENCE: Resolver winner-selection and shove gates key on the priority lane: live formation members displace Normal/Low traffic to claim slots and win contested tiles; sim formation members contend as equals with anything Normal. Inert inside a lone all-Normal sim squad, but live behavior around any other traffic (and any mixed-priority scenario) diverges from what the formation soaks validated.

### [L4] Seam 4 — live-only budget/cost layers the sim never exercises (from movement)
CLAIM: Live movement runs under CPU-derived, governor-tier-scaled pathfinding budgets and prices SK-aggro/road/construction-site layers, none of which exist in the sim's mover (fixed 20k ops, unlimited CPU, empty road/SK/site layers).
LIVE: C:\code\screeps-ibex\screeps-ibex\src\pathing\movementsystem.rs:496-516 ops budget derived from remaining CPU, halved/quartered under Tier::Conserve/Critical, floor 2000, ceiling 50_000; :483-484 repath budget 5.0 CPU; :523-545 movement CPU caps + pathfinding headroom; screeps-rover\src\screeps_impl.rs:262-289 populates `source_keeper_agro` (plus real roads/structures/construction-site layers)
SIM: C:\code\screeps-ibex\screeps-sim-core\src\rover_driver.rs:288 fixed `set_pathfinding_ops_budget(config.pathfinding_ops_budget)` (20_000) and :299-300 `set_cpu_budget(|| 0.0, f64::MAX)` / `set_repath_budget(|| 0.0, f64::MAX)`; C:\code\screeps-ibex\screeps-combat-agent\src\pathing.rs:440-451,490-494 roads empty, construction sites None, `source_keeper_agro: LinearCostMatrix::new()`
CONSEQUENCE: Under bucket pressure live movement degrades in ways no sim run models (ops exhaustion → PathNotFound for stuck-escalation repaths, expiry repaths starved by the 5-CPU budget, tier-quartered budgets during Critical) — combat mobility silently worsens exactly when live fights drain CPU; and SK-aggro/road pricing shifts live route choice relative to sim (partly inherent to offline modeling, but the governor-tier degradation is a live-only behavioral regime worth a soak observation).

### [L5] Seam 6 — tower behavior (from towers)
CLAIM: The sim's combined-fire validation computes the squad focus and squad_dps in the SAME tick as the tower decision, while live sizes the tower gap against LAST tick's focus (tick-order lag: TowerMission runs before SquadManagerSystem) — the lag is documented, but the only lag the sim exercises is id re-resolution, never the gap-sizing consequence of a focus switch.
LIVE: screeps-ibex/src/missions/tower.rs:540-548 (module doc: TowerMission "reads LAST tick's" focus_target_id) with the order pinned in screeps-ibex/src/game_loop.rs:101 (RunMissionSystem) before :104 (SquadManagerSystem); squad_dps at tower.rs:589-597 is computed against that old focus's current position.
SIM: screeps-combat-eval/src/harness/tower_fire.rs:160-173 — combined_tower_intents rebuilds squad_focus_for_towers from the CURRENT world every tick (zero lag) inside the ticks-to-kill measurement; only run_u7_lag_reresolution (tower_fire.rs:316-381) models the lag, and solely for id→creep re-resolution, not commit sizing.
CONSEQUENCE: On any tick the squad switches focus (old focus dies, EV re-rank), live towers subtract squad_dps from the gap on a target the squad will NOT actually shoot this tick → systematic undercommit, leaving the old focus alive with sliver HP for a tick while the freed towers redirect; the U7 win margin was measured without this effect. Low because focus is stable in sustained engagements, but the sim number validating the live wiring is measured under a strictly more favorable (no-lag) regime.

### [L6] Seam 6 — tower behavior (from towers)
CLAIM: The entire live drain-conserve apparatus — the heal-while-away sawtooth confirmation, bounded probe strikes (MAX_PROBE_STRIKES/PROBE_COOLDOWN/MIN_PROBE_PROGRESS), and the probe_fired feedback loop — is live-only: every sim/harness invocation of decide_towers passes an EMPTY conserve set, so the conserve_ids input contract and the probe-driven engaged-drainer exception are never exercised end-to-end in the sim (only a single synthetic-id kernel unit test).
LIVE: screeps-ibex/src/missions/tower.rs:23-39 (probe constants), :249-322 (sawtooth + probe state machine), :337-341 (conserve = confirmed drainers MINUS actively-probed engaged_ids fed into decide_towers), :363-369 (probe_fired stamped from the kernel's chosen target).
SIM: screeps-combat-eval/src/harness/tower_fire.rs:173 and :353 — both decide_towers call sites pass `&std::collections::HashSet::new()`; the only conserve coverage is the kernel unit test `confirmed_drainers_are_never_targeted` (screeps-combat-decision/src/tower_fire.rs:404-412). docs/design/0008a-combat-tactics.md:477 specifies a sim metric ("total tower energy on the drainer over 1000 ticks bounded 
CONSEQUENCE: The bait-ceiling guarantee (a drainer can extract at most 3 probe volleys of energy) and the probe/kernel interaction (an engaged drainer re-entering the candidate list with threat_value ranking, probe_fired only set when the kernel HAPPENS to pick it — a probe can silently never fire if the kernel commits all towers elsewhere, stalling the strike counter) are asserted, not measured; the design doc's own sim gate for T-DEF-3 was never built. Reported per the task's request to enumerate live-only

### [L7] Seam 6 — tower behavior (from towers)
CLAIM: Live squad_dps for the tower gap-sizing is summed over ALL squad members wherever they stand, using world-coordinate Chebyshev range — a member just across the room border within world-range 3 of a near-exit focus contributes phantom ranged DPS it cannot actually land (cross-room attacks are impossible), a case the single-room sim world can never exercise.
LIVE: screeps-ibex/src/missions/tower.rs:589-597 — members are resolved and fed to creep_dps_on_focus with no same-room filter (only the FOCUS is checked to be in-room at :584-586); creep_dps_on_focus (screeps-combat-decision/src/tower_fire.rs:113-124) uses Position::get_range_to, which spans room borders on world coordinates.
SIM: screeps-combat-eval/src/harness/tower_fire.rs:107-129 — squad_focus_for_towers counts only members present in the single-room SimView (`sim.friend_index(id)`), so every counted member can genuinely hit the focus; the sim world is one room (SimView::from_world, screeps-combat-agent/src/lib.rs:107-127).
CONSEQUENCE: When a squad member sits on/over the exit adjacent to a border-hugging focus (a common drain/skirmish geometry), the towers subtract DPS that never lands and undercommit — the focus survives the tick with the gap-sized sliver. Narrow geometry, hence low, but it is a live-only input mis-scoping of the shared kernel's sizing input.

### [L8] Seam 7 — lifecycle/reconcile inputs: duplicated budget constants (from lifecycle)
CLAIM: The lease/budget constants the two sides use to build equivalent snapshot inputs are DUPLICATED private consts, not shared: COMMITMENT_BUDGET/MAX_FORMING_BUDGET/MAX_TRAVEL_BUDGET/SOLO_TRAVEL_STALL_WINDOW exist in both files (currently equal), while MAX_RETREAT_BUDGET, FORMING_ABANDON_STREAK, and NEVER_DEPARTED_GIVEUP_LIMIT exist live-only.
LIVE: screeps-ibex/src/military/squad_manager.rs:247 `const COMMITMENT_BUDGET: u32 = 400;`, 254 MAX_FORMING_BUDGET=3000, 268 MAX_TRAVEL_BUDGET=1000, 175 SOLO_TRAVEL_STALL_WINDOW=100, 284 MAX_RETREAT_BUDGET=600, 261 FORMING_ABANDON_STREAK=20, 275 NEVER_DEPARTED_GIVEUP_LIMIT=2.
SIM: screeps-combat-eval/src/harness/lifecycle.rs:267 `pub const COMMITMENT_BUDGET: u32 = 400;`, 273 MAX_FORMING_BUDGET=3000, 278 MAX_TRAVEL_BUDGET=1000, ~1402 its own SOLO_TRAVEL_STALL_WINDOW=100 — independent copies; no harness constant for the retreat/economic/never-departed bounds.
CONSEQUENCE: A retune of any budget on one side (the exact class of change the WFV-fine/retune-freely policy encourages) silently de-synchronizes the validated envelope from the deployed one — the harness would keep proving lease behavior at the old values while live runs the new ones, with no compile-time or test-time signal.
