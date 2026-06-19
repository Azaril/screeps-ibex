# P2.G4 — Offense migration (`AttackMission` → objectives) + final legacy removal

> **⚠ ARCHIVED (2026-06-18).** O1/O2/O3/O4/O6 are landed; only **O5** (power-bank `Farm`) + **O7** (delete the legacy) + the deferred **heavy multi-squad assault** remain. For the current plan see the master doc [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) §5–6; for the landed-increment history with SHAs see [`phase-2.md`](phase-2.md) §2.0 (at…ay). This file is preserved as the authoritative record of the O-series sequencing rationale, the `AttackReason→ObjectiveKind` mapping table, the manager-gap audit, and the O7 deletion checklist + soak gate.

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
- [x] **O3 — Layered dismantle targeting.** DONE (`9780df9`; rover submodule `448c2d4`). **Pathfinding
  move:** `room_grid_dijkstra` + `reaches_room_edge` relocated from the bot's `pathing::gridsearch` into
  `screeps-rover` (general pure-std primitives → the decision crate can reuse them; no-one-off-pathfinding
  rule). **Pure combat breach:** `breach_redirect` in `decide_squad_with_pathing` — when the focus is a
  hostile structure, build the breach pricing from the view's structures (walls + hostile ramparts =
  dismantlable by hits; other structures + terrain = impassable), run `room_grid_dijkstra` centroid→target,
  and redirect the focus + Advance goal to the first dismantlable blocker on the cheapest corridor. Bounded
  to the structure-siege phase (a structure focus ⇒ no hostile creeps left). Separation upheld (search in
  the pathfinding system, pricing/decision pure, manager applies, job moves). No WFV bump. +1 unit test.
  Inert live until O6; sim-/unit-validated now (rover 27, decision 41, bot 158 [8 gridsearch moved to rover]).
- [x] **O4 — Wave retry / unwinnable in the manager.** DONE (commit after `9780df9`). Generalized
  `AttackMission::handle_wave_wipe` into the `SquadManager` Phase A; `squad_is_wiped` + the non-`Defend`
  `mark_unwinnable` backoff are wired + host-tested (bot 160). `request_renew` audit: none in the manager
  (it never renews — Phase B replacement; only the legacy AttackMission has it → O7). **Design (as built):**
  - **Wipe detection (pure):** a managed squad is *wiped* when it had members but all are now dead —
    `total_members_added > 0 && members.is_empty()`. (Gradual losses are refilled by Phase B's
    unfilled-slot spawns and never hit all-empty; only an overwhelmed squad does.) Extract a tiny pure
    `squad_is_wiped(total_added, member_count)` so it's host-testable without an ECS `World`.
  - **On wipe:** retire the (empty) squad entity + `objective_queue.release_entity`. For a **non-`Defend`**
    objective also `objective_queue.mark_unwinnable(room, now)` — the queue's existing exponential
    backoff (`UNWINNABLE_BACKOFF_BASE`, doubling per call, capped) then makes `best_unclaimed_near` skip
    that room until `retry_after`, so the manager stops feeding squads into an unwinnable siege and the
    producer's re-assert is ignored meanwhile. The **retry** is automatic: when the backoff lapses and
    the producer still wants it, a fresh squad is fielded.
  - **`Defend` stays persistent** — never `mark_unwinnable` an owned-room defense (we don't abandon our
    own room; a wiped defense squad is simply re-staffed by Phase B). This is the one kind exempt.
  - **No `clear_unwinnable` wiring yet** (the backoff self-expires); revisit if a winnable-again signal
    is needed (e.g. the threat weakens) — out of O4 scope.
  - Delete the combat `request_renew` sites (pre-spawn replacement, ADR 0011 D4/D8) — *audit:* the
    legacy renew lives in `AttackMission`/`squad_defense` (already removed); the manager already does
    replacement via Phase B unfilled-slot spawns, so confirm no live combat `request_renew` remains.
  - **Validation:** host-test `squad_is_wiped` + the queue backoff is already tested; the manager
    wiring builds + ticks clean. Improves **live** robustness now (a wiped SK-farm duo / future offense
    backs off instead of feeding); inert for defense by design.
- [ ] **O5 — Power-bank as `Farm{PowerBank}` — DEFERRED to the O6/O7 window (operator 2026-06-18).**
  Mapping (`wf_bd77e5d0`): `FarmKind::PowerBank` + `power_bank_duo`/`power_bank_haulers` already exist,
  and the **crack** maps cleanly to `Farm{PowerBank}` + the manager. **But the haul is special:** the
  power drops on the ground in a HIGHWAY room, which is NOT a transfer-queue source, so `HaulMission`
  (the SK-farm reuse) can't collect it — a **bespoke dropped-power collector** is needed, and that logic
  lives only inside `AttackMission`'s `Exploiting` phase (deleted in O7). Power banks are also niche +
  intermittent (hard to soak-test), and the crack-alone would *regress* (lose the power collection).
  So O5 lands **with O6/O7**, building the dropped-power haul as `AttackMission`'s `Exploiting` is
  removed/replaced. (Engine note: dropped power decays `ceil(amount/1000)`/tick ≈ 5/tick for a 5000
  pile → hundreds of ticks, so hauler timing is forgiving — no razor-edge 20%-HP pre-deploy needed.)
- [x] **O6 — Offense producers (DONE — single-squad cases).** `WarOperation::run_offense_evaluation` upserts
  `Secure` / `Dismantle` / `Harass` objectives instead of launching `AttackOperation`s — the **live consumer +
  validation gate** for O1/O2/O3 (a real offense squad now travels in formation, orients, breaches).
  Each live single-squad `AttackReason` maps to an objective kind + composition + priority, all routed
  through the launch loop's source→objective branch (so dedup + the unified offense cap apply uniformly).
  - [x] **O6.1 — `InvaderCore` → `Dismantle { room, pos: core_pos }` (DONE; no WFV bump; deployed to Docker).**
    `run_offense_evaluation`'s invader-core block now captures the highest-level core's level **and
    position** and, when `invader_core_attack_score` says it's worth attacking (affordability/interest
    gate preserved), **upserts** a `Dismantle` objective (`ForceRequirement::single(siege_quad())`,
    `OBJECTIVE_PRIORITY_MEDIUM` — above SK-farm LOW, below all defense; `owner(Attack)`;
    `OFFENSE_OBJECTIVE_TTL=100`) instead of pushing an `AttackCandidate` / launching an
    `AttackOperation`. The room is re-evaluated each scan (no `active_attacks` entry), so the upsert is
    idempotent + re-asserts the TTL; core dies ⇒ room drops out ⇒ TTL lapses ⇒ manager retires the
    siege squad. The consumer side was already complete (manager `objective_target` maps `Dismantle`→
    `SquadTarget::AttackStructure`; `is_formation_objective(Dismantle)`=true ⇒ O1 travel + O2 orient +
    O3 breach). **Composition is standardized `siege_quad` for all levels** (level-aware sizing — e.g.
    `solo_core_attacker` for a level-0 reserver — is a future refinement, consistent with the W1
    accepted trade-off). **Cap:** no war-side cap on this increment — the manager's
    `MAX_CONCURRENT_SQUADS` gates fielding; a dedicated active-offense-objective count reconciles the
    war cap when the remaining reasons migrate. Bot host 160, wasm + clippy clean; deployed to Docker
    (hot swap, no reset, ticking clean; behavior validates when a core appears — world peaceful now).
  - [x] **O6.2 — the rest, single-squad (DONE; no WFV bump; deployed to Docker).** `AttackFlag` →
    `Secure { room }` (`quad_ranged`, `HIGH` — explicit operator intent); `ResourceDenial` →
    `Harass { room }` (`solo_harasser`, `LOW` — opportunistic). **`InvaderCreeps` RECONCILED = dropped:**
    the reserved-remote-invader `Defend` context in `run_defense_scan` already covers the identical
    trigger (`reservation().mine() && visible() && hostile invaders`), so a `Secure` here would
    double-field the same room — the offense path is removed (the `all_npc` computation is kept for the
    resource-denial gate). **Unification:** O6.1's `InvaderCore` upsert was refactored out of the in-scan
    block into the launch loop too (an `AttackCandidate.target_pos: Option<Position>` carries the core
    tile), so ALL migrated reasons now flow through one source→objective branch — respecting the dedup
    (highest-scored per room) and one combined cap. **War-cap reconcile:** the launch loop's budget is
    `active_attacks.len() + (Attack-owned objectives in the queue)`, capped at `max_concurrent_attacks`;
    an EXISTING objective is always re-asserted (TTL refresh), only NEW offense is gated (score-sorted,
    so the skipped one is lowest-value). `ProactiveDefense`/`ThreatResponse`/`Expansion` are in the enum
    but not produced by the offense scan → nothing to migrate. Bot host 160, wasm + clippy clean.
  - **The HEAVY multi-squad player assault** (`plan_by_detected_threat` towers≥4 → drain-duo + quad with
    `DeployCondition` sequencing) does NOT map to the one-squad-per-objective model → **deferred** (needs a
    multi-squad / sequenced-objective mechanism; stays on `AttackMission` until then, so O7's delete is
    gated on that too). `PowerBank` stays on `AttackOperation` until O5. Validate on Docker (up): the bot
    clears an invader core / honors an attack flag via a manager squad (peaceful world ⇒ validates when a
    target appears).
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
