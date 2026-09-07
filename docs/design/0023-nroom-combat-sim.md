# 0023 — N-room combat sim (P-ENGINE design)

- **Status:** Decided

Implements ADR 0022 **P-ENGINE** (the full N-room engine, chosen over a movement-only harness).
Extends the single-room `screeps-combat-engine` to N rooms so integrated multi-room flee / attack /
group-up and the auction's objective-bed composition tournament (ADR 0022 P-AUCTION) are simulatable
end-to-end.

## Architecture finding (what the sim does — and doesn't)

Verified by reading the crate:

- **The engine is a deterministic tick over a `CombatWorld`.** `resolve.rs::resolve_tick` takes an `Intents` (per-creep combat actions **+ move `Direction`s**, tower actions) and resolves it (two-phase accumulate-then-apply combat + the `movement.rs` same-tile-contention port). It does **not** pathfind.
- **Move *directions* are produced by the agent/harness layer** (`screeps-combat-agent`) from the bot's decision output (`decide_combat`/`decide_squad`) — the engine just resolves the given `Direction`s.
- **The sim runs the live mover.** The rover `MovementSystem` + resolver (traffic management) is wired in via `resolve_moves_via_system` (`CombatMovementExternal` + `CombatCreepHandle` + a multi-room `CombatWorldCostSource`), so **sim ≡ live: one traffic-managed mover**, not a sim-only reimplementation. What the sim does *not* do is *measure* rover quality (route optimality / fatigue / congestion / ops) — it scores combat. That measurement gap is [ADR 0033](0033-rover-pathing-sim-and-benchmark.md), which also **extracts** this engine's combat-agnostic movement mechanism (world / terrain / body / contention / fatigue / edge-exit / recording) into a shared `screeps-sim-core` crate both sims depend on, landing two fidelity fixes there (roads + loaded-CARRY). After that extraction `screeps-combat-engine` = `screeps-sim-core` + the combat layer (`CombatState` + `resolve_combat_tick`), and the engine's movement types are renamed `Combat*`→`Sim*` (`CombatWorld`→`SimWorld`, `CombatTerrain`→`SimTerrain`, `resolve_tick`→`resolve_combat_tick`); the movement claims here hold unchanged, just from the lower crate under the new names.
- **Single-room assumptions the N-room design lifts:** `CombatWorld.terrain` is one `CombatTerrain`; `movement.rs::step` returns `None` at room edges; `rampart_at(x,y)`/contention key by `(x,y)` not `(room,x,y)`; combat range uses `(x,y)`. Positions are **already** `screeps::Position` (room-qualified) — the data model is half-ready.
- **Cross-room movement is NOT a move intent (verified against the real engine).** A creep cannot move off a room edge: `move.js:32` rejects the intent and `movement.js:88` clamps to the edge. Crossing is an **automatic edge-exit** (`creeps/tick.js:52-78`): after movement, any non-NPC creep (engine skips users '2'/'3') *standing on* an exit tile is relocated to the adjacent room's mirror tile (`x==0↔49`, `y==0↔49`), applied in the global stage (`global.js:42`). It is same-tick (engine `bulk.update` mutates in place). Modelling crossing as a `step` (the RTS model) is wrong — see the S1 layer below.

## N-room design

- **World:** `CombatWorld` keeps `terrain: CombatTerrain` as the default/common room terrain (so single-room builders stay unchanged — zero churn) **plus** `rooms: HashMap<RoomName, CombatTerrain>` per-room overrides, read via `terrain_for(room) -> &CombatTerrain` (override if present, else the default). No explicit room graph needed — adjacency is `Position` world-coord arithmetic (`Position::checked_add((dx,dy))` crosses rooms), and `Position::get_range_to` is already room-aware (Chebyshev over global coords). *(Caveat of the hybrid: the default terrain applies to every override-less room, so a true multi-room scenario with distinct walls per room gives each room its own override via `terrain_mut`.)*
- **Movement:** crossing a room boundary is **not a move intent** — the real engine *rejects* an off-edge move (`move.js:32`) / *clamps* it (`movement.js:88`). So `step()` returns `None` at an edge (in-room only); `resolve_moves` + fatigue read `terrain_for(dest.room())`. The boundary cross is a separate **edge-exit relocation** in `resolve_tick`'s Phase D (engine `creeps/tick.js:52-78` + `global.js:42`): a **non-NPC** creep standing on an exit tile is moved to the adjacent room's mirror tile (the tile one `checked_add` step across that edge). It is **same-tick** — a creep that *moves* onto the edge crosses that tick (engine `bulk.update` mutates in place, so `tick.js:52` reads the post-move position), so a border cross is effectively "free" of one tile. NPC creeps (keepers/invaders, engine users '2'/'3') never auto-exit → `CombatWorld.npc_owners`. Fatigue zeroes on entering an edge tile (`is_edge`).
- **Combat (per-room):** tower fire only reaches creeps in the **tower's room** (the engine gates EVERY creep and tower action to the actor's room — `in_range` same-room check in `resolve.rs`, WS-CLOSE 2026-09-07; world-coordinate range alone was NOT enough: a seam-adjacent tower could hit the next room); `rampart_at`/redirect are room-aware. Most range checks already use `get_range_to` (room-aware) so this is mostly fixing the `(x,y)` helpers.
- **Agent (`screeps-combat-agent`):** N-room `ScenarioBuilder` (multiple rooms, per-room terrain/structures, exits); **cross-room direction production** — the harness pathfinds a creep toward a target in another room and emits the per-tick `Direction` (the engine doesn't pathfind). **Objective beds:** core + towers + ramparts + defender creeps with active rampart-repair + tower-heal-of-defenders (the D5 fight model), with a win condition.
- **Eval (`screeps-combat-eval`):** metrics/cohesion room-aware; the tournament extends to attacker-vs-objective + composition space (ADR 0022 P-AUCTION).

### Pathing is the rover's, and the mover must be deterministic

All pathing goes through rover — the sim never grows a private pathfinder. Two properties this
demands, both discovered by the sim and both fixes that help live equally:

- **Deterministic conflict resolution.** `resolver.rs::resolve_conflicts` picks the contested-tile
  winner by `max_by(priority → stuck_ticks)`; without a further tie-break a true tie resolves by
  std-HashMap per-process seed order, so moves become seed-flaky. The winner is therefore tie-broken
  on `Handle`, and `losers` are sorted — deterministic in every process.
- **Combat creeps take `High` movement priority.** With a duo sharing one kite goal, a neutral
  tie-break parks the *healer* on the forward shooting tile and leaves the *shooter* one tile out of
  range. Giving combat creeps `High` priority makes the shooter win the forward tile, reach range 3,
  and focus-fire.

The squad layers (`SimSquad`, the formation squad; `ManagedSimSquad`, the kiting self-play squad) both
resolve every member's move in one traffic-managed `resolve_moves_via_system` pass, with a persisted
`move_cache` carrying the resolver's path-reuse + stuck-escalation across ticks. The folded-slot
geometry stays as the *target* layer and the resolver deconflicts the rest. The mover carries `Flee`
support + a per-request `MovementPriority` for this. Single-creep scenario/siege callers may use
`resolve_move_direction` directly — with one creep there is no traffic to resolve.

`LocalPathfinder::search` is a **true multi-room A\*** (over `Position` nodes, per-room grids via
`room_callback`), so cross-room routing is native: no per-room MoveToRoom workarounds
(`in_room_goal` projection in `resolve_move_direction`, an anchor find_route+exit+cross-step, an
orchestrator pre-projection) are needed. `AnchorPath` routes multi-room through the same search, so a
squad pre-groups at the exit and crosses as a bloc — distinct folded slots, `SimView`-decoupled member
moves — and reforms a tight box on the far side.

## Build layers (each independently testable)

- **S1 — multi-room terrain + edge-exit movement** (the foundational substrate): `terrain` default + `rooms` override map + `terrain_for`/`terrain_mut`; `resolve_moves` + the fatigue loop read the destination room's terrain. `step()` is in-room-only; the edge-exit relocation lives in Phase D and `CombatWorld.npc_owners` marks the NPC-exempt owners. Invariants under test: `step` rejects off-edge; `resolve_moves` does not carry a creep off an edge; the wall check reads the creep's own (non-default) room; edge-exit relocates an idle creep, is same-tick on move-onto-edge, skips NPCs, and a move-inward escapes the exit. Single-room builders stay source-compatible via `..Default::default()`.
- **S2 — per-room combat:** rampart shield/redirect keyed by full `Position` (`rampart_tiles`/`rampart_id_at`/`on_rampart`/`redirect` in resolve.rs); movement contention keyed by `Position` (`Mover.current_pos`/`dest_pos` + all `want_count`/`matrix`/`want_idx`/`creep_at`/swap/chain-block maps); no `(x,y)`-keyed `CombatWorld::rampart_at`. An adversarial audit confirmed every *other* combat site is already room-safe via `get_range_to`: tower fire is room-scoped by the engine's explicit same-room gate (`resolve.rs` `in_range`, 2026-09-07 — the earlier "naturally out-of-range, no scoping needed" claim was wrong for seam-adjacent towers and creeps), damage/heal pools are id-keyed, and Phase-D edge/fatigue is room-local. Invariants under test: a rampart only shields within its own room; two creeps at the same `(x,y)` in different rooms don't contend.
- **S3 — N-room ScenarioBuilder + cross-room direction production** (agent): `ScenarioBuilder::in_room(room)` switches the current room so terrain/structures/towers are placed per-room, with terrain methods writing to that room's override (`terrain_mut`). `CombatCostSource` is **room-scoped** — only obstacles *in* the queried room populate its matrix, and terrain reads `terrain_for(room)`. (Overlaying every room's obstacles at the same `(x,y)` makes cross-border routing impossible.) Invariants under test: a creep paths across a room boundary end-to-end (`resolve_move_direction` + `resolve_tick` loop — the creep crosses and arrives).
- **S4 — objective beds with active repair** (the D5 fight model, `objective_bed.rs` in combat-agent): `defense_intents` mirrors the engine stronghold AI (`stronghold.js`) — **focusClosest** (all energized towers + in-range defenders hit the hostile closest to the core) + **towersMaintenance** (one spare tower heals the most-damaged defender, else repairs the most-damaged rampart, which is the active repair). The engine has **no creep-repair action**, so active repair is tower-based — which is exactly how strongholds hold: defenders don't repair; towers heal them and static high-HP ramparts wall the core. `run_siege` runs an attacker (any `FnMut(&CombatWorld) -> Intents` — `dismantler_intents` for tests, a real attacker AI for the auction) vs the defended core with a win condition: `CoreBreached` / `AttackersWiped` / `Held` (timeout). The bed composes with the `ScenarioBuilder` (S3). Invariants under test: an under-gunned attacker is repelled (repair out-holds its DPS); a TOUGH-buffered sufficient-DPS dismantler breaches; the defense heals a damaged defender while focusing the attacker. **Engine-mechanics ground truth:** body parts are destroyed **from index 0** (front), so a front-loaded WORK body loses dismantle power to tower fire — TOUGH-first is mandatory for a sieger. Design extensions this bed anticipates: a dedicated dismantle-immune `Core` StructureKind (so the sim requires *attack*, not dismantle, per the invader-core rule), defenders that reposition (`assignDefenders`), and `focusMax` for L4–5; the auction (P-AUCTION) plugs a real attacker composition into `run_siege`.
- **S5 — scenarios + integration gate** (`scenarios.rs` in combat-agent — the offline whole-stack gate, ADR 0022 PROVE-1's offline half; run via `cargo test … scenarios`): each scenario reuses the real `resolve_move_direction` + `resolve_tick` + `run_siege`, never mocks. **CROSS-ROOM-TRAVEL** (a creep routes across *two* borders to a target); **FLEE-ACROSS-ROOMS** (a creep flees a far threat to the room edge and the edge-exit carries it across — flee + edge-exit, no special cross-room flee); **ATTACKER-VS-OBJECTIVE** (a sieger crosses the border, breaks the rampart, and damages the core under tower fire/repair — the full S1→S4 stack the auction scores); **GROUP-UP-THEN-ENGAGE-ACROSS-BORDER** and **STUCK-MEMBER-TIMEOUT** (the squad-anchor cross-room cases). Two constraints these scenarios establish: (1) the S1 edge-exit means a target reachable *only* from an exit tile is un-besiegeable — the attacker is auto-exited off it every tick — so a bed must sit ≥1 tile inside the border (the attacker then naturally moves off the exit because the target is range > 1). (2) A *lone* dismantler is under-gunned for a towered core (the tower wears it down during the open core phase); force adequacy is the auction's job (P-FORCE / P-AUCTION), so the gate asserts the stack *fired* (crossed + breached rampart + damaged core), not a win.

## Design deltas (2026-09-07 — WS-CLOSE lane (b2), decision D4)

- **S5 is complete.** GROUP-UP-THEN-ENGAGE-ACROSS-BORDER and STUCK-MEMBER-TIMEOUT landed in
  `scenarios.rs` (`squad_groups_up_at_the_border_then_engages_the_objective_across_it`,
  `squad_does_not_hold_forever_at_a_border_whose_far_side_is_walled`); `integration_gate_marker`
  now names all five scenarios as fn items (a missing one is a build break). Both drive the real
  `ManagedSimSquad` with a rally (the live `SquadManager` path — rally-gated bloc crossing +
  traffic-managed mover), not the anchor `SimSquad`: the anchor's "single-room" deferral is moot
  since `AnchorPath` went multi-room, and the managed squad is what live fields.
- **The squad-anchor cases are shaped by three sim findings, recorded here so nobody re-tunes
  them into the beds:** (1) the engine's tower resolution + `objective_bed::defense_intents` are
  room-agnostic (range over world coordinates), so a border-adjacent tower fires ACROSS the seam
  into the staging room — the real `tower.attack` is same-room only. GROUP-UP runs its siege loop
  over a same-room-gated defense (`same_room_defense`, a FOLLOW-UP owned by the H5 parity lane: fix
  it at the source in the engine + `defense_intents`, then delete the shim). Creep attacks/RMA/heal
  have the same gap; it only bites the gauntlet at the exit tiles (range ≤3), which mirrors a
  camper adjacent to the exit and is left as is. (2) The bloc gate's gather target is the
  traveller centroid projected onto the exit band and is recomputed per tick, so a long approach
  drifts along the border (the A* tie-break goes diagonal, the centroid follows, the target
  follows the centroid); where the bloc crosses decides how the kernel meets the bed. (3) On the
  tower's flank the kernel has been seen to assign two members the SAME goal tile; the dance
  damper then converts both to `Immovable` holds and the squad parks Engaged without acting — a
  kernel (ADR 0025) finding, not a crossing one. The GROUP-UP staging is chosen so the bloc crosses
  north of the bed and takes the core; keep it when re-tuning.
- **STUCK-MEMBER-TIMEOUT walls BOTH sides of the seam** (as the real terrain generator guarantees —
  exits mirror). The sim's edge-exit is wall-blind by engine fidelity, so a one-sided far-side wall
  does not block a crosser, it teleports it onto the wall tile; an asymmetric fixture is an
  impossible world. The bed pins: every member reroutes through the one open column and stands in
  the objective room off the edge band within 150 ticks — no eternal gather on dead-end files.
- **Cross-room `Flee`, squad side (D4 closes it on the single-creep bed + the squad rout bed).** The
  rout bed (`combat-eval` `stronghold::run_border_rout` / `RoutOutcome`, pinned by
  `border_rout_withdraws_across_the_seam_but_never_reaches_the_rally_yet`) forces a quad across the
  g2 gauntlet camp and grades the retreat leg. What holds: after contact the crossers withdraw back
  across the seam within ≤4 ticks, and there is no lone-survivor `Timeout` stall. What does NOT hold
  yet, pinned as the honest baseline: the rally is never REACHED — `Retreating` decays to `Forming`
  the moment no member stands in the fight room, the bloc gate re-releases, and the single open
  column funnels members back in one at a time (trickle-and-die to `SideWiped`). The rally is a
  direction, not a destination, until a re-entry/give-up rule exists (ADR 0027 Retreating-decay
  write-back; the M23 economic give-up is the designed answer). The seam-stitched threat/approach
  field (ADR 0024 §follow-up, ADR 0025 open item; the ~93% cross-room oscillation pinned at ≤0.97
  in `positioning_oscillation_stays_low_across_designed`) stays a kernel design item — not harness
  closeout.

## Design deltas (2026-09-07 — WS-CLOSE Phase B lane B(ii), engine seam fidelity)

- **S2's "tower fire is naturally out-of-range across rooms" claim was false and is now true by
  construction.** `Position::get_range_to` is a WORLD-coordinate Chebyshev distance, so a tile one
  step across a seam reads as range 1: a border-adjacent tower fired 600/tick into the neighbouring
  room and exit campers at x=45–48 hit travellers at x=0–2 of the staging room. The real engine
  rejects every cross-room target (each creep action's `target.room != object.room` guard,
  `towers/{attack,heal,repair}.js`, and `rangedMassAttack.js` ranging over the actor's own room).
  `resolve.rs` now gates every creep action (Attack/RangedAttack/RangedMassAttack/Heal/RangedHeal/
  Dismantle/AttackStructure/RangedAttackStructure/AttackController) and every tower action
  (Attack/Heal/Repair) on `target.room == actor.room` via one `in_range` helper — a cross-room intent
  is a no-op (no damage, no heal, no tower energy), pinned RED-verified by the seven `*_seam` /
  `tower_heal_and_repair_stay_in_the_towers_room` engine tests with same-room positive controls.
  `objective_bed::defense_intents` picks its focus (and its in-range defenders) in the core's room;
  the `same_room_defense` shim in `scenarios.rs` (S5 finding (1) above) is deleted and GROUP-UP runs
  on the bed's own defense. The five conformance placeholders are single-room and unchanged.

## Cross-references
ADR 0022 (P-ENGINE / P-AUCTION / PROVE-1), 0006 (eval harness), 0008 (combat arch). The anchor movement fix is rover `fd977f2` (validated by rover unit tests + live, not in this sim).
