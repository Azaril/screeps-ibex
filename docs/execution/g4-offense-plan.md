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

- [ ] **O1 — Squad anchor mover (M2).** Footprint-aware ("moving-maximum") anchor pathfind in the
  pathfinding/rover layer + lockstep block advance + hard cohesion gate, consumed by the manager so a
  squad travels multi-room *as a formation*. Corridor relaxation + loose-centroid for N>4. This is the
  prerequisite for any siege; **build + sim-validate first** (cohesion metric on a multi-room move).
- [ ] **O2 — Formation orientation (pure).** Port `threat_direction`/`reassign_slots` into
  `decide_squad` (or a pure `orient_formation`) so the block faces the threat with tanks front. Wire
  through `apply_squad_decision`. Self-play scenario.
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
