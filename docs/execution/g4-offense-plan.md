# P2.G4 — Offense migration (`AttackMission` → objectives) + final legacy removal

> Companion to [`g3-tail-plan.md`](g3-tail-plan.md). Authored 2026-06-18 after the **defense
> half** of the legacy removal landed (`SquadDefenseMission` deleted; all defense now routes
> through `Defend` objectives — see phase-2 §2.0). This is the **offense half**: it is the
> remaining work to retire `AttackMission`/`AttackOperation` and reach "no legacy combat-driving
> mission code." Rationale: [ADR 0008 §6 steps 4–5](../design/0008-combat-and-squad-architecture.md).
>
> **Why this is sequenced, not done in one shot (operator decision 2026-06-18):** the `SquadManager`
> + `decide_squad` is a *single-room engagement* lifecycle; `AttackMission` is a *multi-room siege
> state machine*. Deleting `AttackMission` before the manager can drive a full offense regresses the
> live (MMO) bot — and offense is the **hardest path to validate headless** (multi-room siege isn't
> in the combat sim). So the capabilities below are built + validated first, then `AttackMission`
> is deleted last. Each step is independently committable + green (host + wasm + clippy).

## Capability gap (the manager-gap audit, `wf_1d0ec829`, 2026-06-18)

`AttackMission` does five things the manager does **not** yet do (file:line refs are pre-removal):
1. **Squad-level anchor path for multi-room travel** — `AttackMission` pathfinds a squad anchor to
   the target room (`nearest_home_room_to_target` + `advance_squad_virtual_position`) and advances it
   in lockstep. The manager has **no squad anchor**: jobs self-navigate per-creep (`squad_combat.rs`
   `tick_move_to_room`), so the formation strings out crossing rooms. This is the **M2 anchor mover**
   (ADR 0003 §B.2 / ADR 0008 §6 step 2) — the gating prerequisite.
2. **Formation orientation toward the threat** — `threat_direction` + `reassign_slots` /
   `threat_facing_slots` (dormant in `formation.rs`/`squad.rs`) keep tanks front / healers back and
   present fresh armor. `decide_squad` has no threat-orientation input.
3. **Layered dismantle target selection** — `AttackMission`'s Engaging phase tracks structure HP and
   stages rampart→core destruction; `decide_squad`'s focus is "lowest-hits hostile / best-rank
   structure", with no "the rampart blocks the path to the core, break it first" staging.
4. **Wave retry / unwinnable backoff** — `AttackMission::handle_wave_wipe` retries a failed assault up
   to `max_waves`; the queue already has `mark_unwinnable`/`is_unwinnable_now` but the **manager does
   not call it**, and it has no wave-wipe retire-and-rebuild.
5. **Power-bank exploit phase** — `AttackReason::PowerBank` cracks a bank then deploys haulers timed to
   ~20% bank HP (`DeployCondition::AfterTargetHPPercent`). The manager has no exploit/coordinated-hauler
   phase. (Power bank is arguably a `Farm{PowerBank}` objective + a hauler sub-mission.)

`AttackOperation` reasons to map: `Flag`, `ThreatResponse`, `Expansion`, `ResourceDenial`,
`InvaderCore{level}`, `InvaderCreeps`, `SourceKeeper`(dead — confirm/remove), `PowerBank{power}`,
`ProactiveDefense`. Tactics already covered by G3: focus-fire, heal-assignment, kiting, cohesion.

## Steps

- [x] **O1 — Squad anchor mover (M2) wired into the manager.** The mechanism was already built + tested
  (rover `AnchorPath` footprint-aware cached pathfind via `moving_maximum` + holds-on-blocked; `formation.rs`
  `advance_squad_virtual_position` cohesion gate + corridor collapse/re-form; the job's `MoveToRoom`
  formation-travel + `squad_has_anchor`→`execute_formation_movement` branch). The gap was that the
  `SquadManager` never set up/advanced the squad anchor. **DONE (`a0e4284`):** `compute_squad_orders`
  advances the squad's footprint anchor toward the target-room centre while the squad is still converging
  (the job reads `virtual_pos` + issues each member's `move_to` — manager decides the frame, job moves,
  §5), and DROPS the anchor on arrival so the `Engaged` state kites via `decide_movement` (engaged
  formation-follow is O2 — keeps G3 kiting intact). No WFV bump (reuses `SquadContext.squad_path`). Host
  165 + wasm + clippy green; mechanism rover-tested; **deployed to Docker (WFV 11→12 reset clean, no thrash,
  bot ticking)**. Multi-room cohesion + the trickle-in formation-hold validate on the soak under a real
  threat. *(Note: the manager's travel anchor uses the live multi-room `ScreepsPathfinder`, so it is
  Docker-validated, not combat-sim-validated — the sim is single-room; the AnchorPath mechanism is the
  sim-runnable part and is rover-tested.)*
- [x] **O2 — Formation orientation.** DONE (`0c79fa7`). Pure: `decide_squad` outputs
  `orientation: Option<Direction>` (centroid → focus, Engaged-only). Application: `slots_front_to_back`
  projects each slot offset onto `threat_direction` (front = toward the threat) and `reassign_slots`
  lands tanks/high-HP in the leading slots — replacing the min-Y placeholder + dropping the per-death
  `orient_toward` (orientation lives in slot assignment, no double-rotation; layout stays base so
  footprint/travel are stable). Manager derives combat style from the objective kind
  (`is_formation_objective`: Dismantle → Formation/siege keeps the anchor + advances to the focus +
  orients; everything else → Skirmish/kite, O1). Separation upheld (pure decide → manager applies →
  job moves). **No WFV bump** (combat style derived per-tick, not serialized; reuses
  `threat_direction`). Inert live until O6 (no Dismantle objectives yet — all current squads Skirmish);
  unit-tested (decision 40, bot 166); full formation-orient behavior validates in the sim + soak when
  O6 fields siege squads.
- [ ] **O3 — Layered dismantle targeting (pure).** A pure `select_dismantle_target` (rampart/wall on
  the path to the ranked structure → core/spawn/tower) added to `decide_squad`. Self-play vs a ramparted
  target. Reuse the rover cost-matrix to know "what blocks the path".
- [ ] **O4 — Wave retry / unwinnable in the manager.** Generalize `handle_wave_wipe` into the manager:
  on a wiped squad with the objective still wanted, retire-and-rebuild; after K failures call
  `objective_queue.mark_unwinnable(room)` (backoff already exists). Delete the combat `request_renew`
  sites (pre-spawn replacement, ADR 0011 D4/D8).
- [ ] **O5 — Power-bank as `Farm{PowerBank}`.** Producer upserts `Farm{PowerBank}` (the `power_bank_duo`
  composition exists); the coordinator owns the timed `power_bank_haulers` deploy (mirrors the SK
  coordinator owning mining children, P2.K). Replaces `AttackReason::PowerBank`.
- [ ] **O6 — Offense producers.** `WarOperation::run_offense_evaluation` upserts `Secure`/`Harass`
  (player offense gated by ADR 0014 `WarDecl`; NPC `InvaderCore`/`InvaderCreeps` policing autonomous);
  `AttackOperation` → `Dismantle` for blocking structures. Map every live `AttackReason` → objective
  kind + composition + priority + (re-assert-until-clear) lifetime. (`InvaderCreeps` in a reserved
  remote is already handled by the migrated remote-defense `Defend` path — fold/avoid overlap.)
- [ ] **O7 — Parity + DELETE the legacy.** Once O1–O6 reach parity (the bot clears an invader core +
  takes/sieges a target on the private-server soak): delete `AttackMission` (`attack_mission.rs`),
  `AttackOperation` (`operations/attack.rs`), the `AttackReason` enum, the `MissionData::AttackMission`
  + `OperationData::Attack` variants + dispatch arms + `mission_type!`/`operation_type!` lines + mod
  exports + tests. **Bump `WORLD_FORMAT_VERSION`** (removed enum variants → loud reset). Then NO legacy
  combat-driving mission code remains (`SquadCombatJob` shrinks to pure order-execution + `Recall`).

## Validation gate

Offense is the path the combat sim covers least (single 50×50 room, no inter-room). So O1–O3 self-play
where possible, but **O7's delete is gated on a private-server soak** (Docker): the bot must clear a
real invader core and lay siege to a target with the squad staying cohesive across rooms, no CPU
spiral, no orphan/idle. Do not delete `AttackMission` on faith — keep it until the manager path is
soak-proven at parity. (Docker was down at authoring; this is the first thing to stand up on resume.)
