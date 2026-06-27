# ADR 0017 — Threat-Aware Expansion Lifecycle

- **Status:** Proposed
- **Date:** 2026-06-14
- **Deciders:** Lead architect (pending operator sign-off)
- **Related:** ADR 0008 (combat & squad architecture), ADR 0014 (empire strategy & posture), ADR 0002 (serialization), ADR 0004 (CPU governance), the derelict-rooms work (`jobs/declaim.rs`, `SalvageMission` declaimers, M12–M16). Supersedes the implicit expansion pipeline behavior in `operations/claim.rs`, `missions/claim.rs`, `missions/colony.rs`, `operations/war.rs`.

---

## 1. Context & the failure being fixed

Today expansion is a detached, threat-blind pipeline: `ClaimOperation` selects a target on **economy only** (`gather_candidate_room_data` at `operations/claim.rs:109` computes viability from `owner().neutral()` + reservation + `!source_keeper`; `try_score_candidates` at `:390` rejects only `owner().hostile()` controller-ownership at `:404` — never a parked hostile combat creep, hostile towers, or `RoomThreatData`), so a neutral room with an enemy combat creep in it scores and ranks normally; `ClaimMission` then spawns a lone `[Claim,Move]` claimer that, on death, is silently re-requested next tick with **no backoff and no abort** (`missions/claim.rs`, `self.claimers.is_empty() => respawn`); the moment the room becomes `owner().mine()`, `ColonyMission::can_run` (`missions/colony.rs:329`) fires its single, no-abort `Incubate` state; `ClaimOperation::spawn_remote_build` (`:906`) feeds builders in (travel-time-gated only) where they die; and `WarOperation` defense is reactive, spawn-sourced from distant homes, and **excludes the spawnless RCL1 claim from the SafeMode/WallRepair `home_set`** (`operations/war.rs:372–385`). No component ever un-claims — the `DeclaimJob`/`attackController` primitive is wired only into `SalvageOperation` for foreign/derelict rooms. Net: a threat-blind commit gate, a no-abort claimer loop, reactive-only defense, and an unwired exit produce a **contested-claim drain** — the bot pours claimers, builders, haulers, CPU, and single-RCL1-spawn time into an undefendable room it can neither hold nor leave, indefinitely (the sunk-cost trap).

## 2. Design principles

- **Assess before committing; re-validate at the moment of commitment.** A claimer is never dispatched into a room that current, *fresh* intel shows as contested, and the gate is re-checked when the claimer is adjacent (intel can change over the ~600-tick claimer life / travel).
- **Absence of intel is not safety.** Stale or missing threat data reads as *not-safe*, not safe — the opposite of `run_reclaim`'s `.unwrap_or(true)`. The root failure is "a single stale clean scout looked fine."
- **Protection comes from pre-clear / escort and racing to RCL3 — never from safe-mode heroics on a fresh claim.** A freshly *claimed* RCL1 controller has `safeModeAvailable == 0` and no tower until RCL3 (load-bearing, §5).
- **Abort is cheap and is the default for a losing spawnless claim.** GCL is never refunded *and never lost* on a revert; the room becomes neutral and re-claimable. Continuing to feed a contested RCL1 is the expensive choice. The abort rule is the robust community heuristic: *spawnless owned room + sustained player-hostile presence → abandon.*
- **Minimal serialization blast radius, maximal reuse.** No new `OperationData`/`MissionData` variant. The lifecycle lives in the existing `ColonyState` `machine!` and the existing `ClaimOperation`/`ClaimMission` structs; it reuses `SquadDefenseMission`, `RemoteBuildMission`, `SafeModeMission`, `VisibilityQueue`, the defender-selection registry (`defense_doctrines()`/`GarrisonDefense`), and the governor/economy gates. **(Superseded 2026-06-26, ADR 0026 §9.10 L3a: the originally-cited `DefenseEscalation::from_threat` was DELETED — defender selection now lives on the doctrine registry, so the escort sizes via `defense_doctrines()` and the M3 "make `from_threat` `pub`" step below is obsolete.)**
- **Lifecycle hygiene against the operator's known failure class.** Every new `Entity` reference is covered by `repair_entity_refs` and the `machine!` `get_children_internal` plumbing, validated-before-use at each transition, with a unit test — dangling-ref / lifecycle-hang is the documented historical failure mode.

## 3. The expansion lifecycle (state machine)

State for a single target is split across two existing serialized owners — `ClaimOperation` (selection + claimer + pre-clear escort) and the per-room `ColonyMission`'s `ColonyState` `machine!` (establishment + defense + abort). This is deliberate: `ClaimOperation` is the only place `RoomThreatData` is in scope (`OperationExecutionSystemData.threat_data`), and `ColonyMission` already owns the room post-claim and already has a `machine!`. The two halves hand off at the `owner().mine()` boundary.

```
                        ┌─────────────────────────────────────────────┐
                        │   ClaimOperation (RoomThreatData in scope)    │
                        └─────────────────────────────────────────────┘

   discover/select cadence
   (Tier::Normal, GCL room, below cap)
            │
            ▼
      ┌───────────┐   hard veto (hostile owner/foreign-reservation,
      │  ASSESS   │── PlayerRaid+, hostile towers, militarily_active,
      │(pre-gate) │   stale/absent intel) ─────► PRUNE + avoid-cooldown
      └───────────┘
        │       │ marginal (transient/weak threat, EV-positive)
  clean │       └──────────────► ┌──────────┐
        │                        │ SECURING │ escort (SquadDefenseMission)
        │                        │(pre-clear│ pre-clears/holds room;
        │                        │ /escort) │ claimer held in queue
        │                        └──────────┘
        │             held N ticks │      │ escort lost / persists past budget
        │             (anti-flap)  │      └──► ABORT (prune + avoid-cooldown)
        ▼                          ▼
      ┌──────────────────────────────────┐
      │             CLAIMING              │  re-validate gate at adjacency;
      │  (ClaimMission, hardened: death   │  exponential respawn backoff;
      │   counter + backoff + abort)      │  abort on re-arm / death budget
      └──────────────────────────────────┘
        │ owner().mine()                  │ death budget exceeded
        │ (claimController = 1 tick)      │  or target re-armed
        ▼                                 └──► ABORT (prune + avoid-cooldown)
   ═════════════════════════ handoff at owner().mine() ═════════════════════════
        │
        ▼                       ┌──────────────────────────────────────────────┐
      ┌──────────┐              │  ColonyMission / ColonyState machine!         │
      │ INCUBATE │              └──────────────────────────────────────────────┘
      │(threat-  │◄───────────────── clear for N ticks (anti-flap) ──────┐
      │instrument)│                                                       │
      └──────────┘                                                       │
        │   │ sustained hostile (persistence > 20 ticks)            ┌──────────┐
        │   └──────────────────────────────────────────────────────►│CONTESTED │
        │ RCL3 reached + clear                                       │ (defend) │
        ▼                                                            └──────────┘
   ┌─────────────┐                                                        │
   │ ESTABLISHED │ (marker; normal TowerMission/DefendMission             │ no-win
   │             │  own defense; lifecycle complete)                      │ predicate
   └─────────────┘                                                        ▼
                                                              ┌────────────────────┐
                                                              │     ABANDONING     │
                                                              │ stop child spend,  │
                                                              │ controller.unclaim,│
                                                              │ avoid-cooldown tag │
                                                              └────────────────────┘
                                                                   │ !owner().mine()
                                                                   ▼  (one tick)
                                                              MissionResult::Success
                                                              (teardown; room neutral,
                                                               re-claimable after cooldown)
```

**Abort edges** (any state → Abort/Abandoning): foreign claim/reservation appears, hostile tower appears, `threat_level >= PlayerRaid` sustained, claimer/builder death budget exceeded, sustained-CPU-Critical on a non-essential contested claim, or the no-win predicate (§7). Abort is a normal serializable transition, never a crash path (per Overmind issue #107: removal must validate the room object exists and must not re-create what it just removed).

## 4. Pre-claim safety gate

Two tiers, because the two intel sources live at different seams (verified):

- **BFS tier (cheap, `RoomDynamicVisibilityData` only).** `GatherSystemData` (`room/gather.rs:101`) does **not** carry `RoomThreatData` — only `room_data`. So `gather_candidate_room_data` (`operations/claim.rs:109`) folds the cheap, always-cached dynamic-visibility signals into `viable`/`can_expand`: reject if `hostile_creeps()` (combat parts present) **or** `hostile_towers()` (active+funded) **or** `tower_dps_at_edge().is_some()` **or** `militarily_active()` **or** `reservation()` is `Hostile/Friendly` by another player (the engine blocks `claimController` with `ERR_INVALID_TARGET` on a foreign reservation — see §10 must-fix). `hostile_threat_creeps()` (true for any non-Move/Tough part, incl. haulers) is **not** used as a veto here — too coarse — but downweights the soft score.
- **Score/commit tier (rich, `RoomThreatData` in scope).** `try_score_candidates` (`:390`) and `run_select` (before `ClaimMission::build` at `:789`) read `system_data.threat_data.get(candidate_entity)` (`RoomThreatData`, fields verified at `military/threatmap.rs:68–89`):
  - **Hard veto** if `threat_level >= ThreatLevel::PlayerRaid` (a parked enemy combat creep classifies as `PlayerRaid`), or `estimated_dps > 0`, or `!hostile_tower_positions.is_empty()`, or `incoming_nukes` non-empty. This mirrors the proven `ColonyOperation::run_reclaim` gate (`operations/colony.rs:83`, `threat_level <= Invader`) the forward path lacks — **but without** that gate's `.unwrap_or(true)` (Principle 2).
  - **Intel-freshness gate.** Require `game::time() - threat_data.last_seen <= features.claim.intel_freshness_ticks` (default ~50). If absent or stale → treat as **not-safe**: keep a `VISIBILITY_PRIORITY_HIGH` request alive (`VisibilityQueue`) and re-scout instead of committing. `RoomThreatData` persists 500 ticks after `last_seen`, so a single confirmed sighting is actionable, but a *clean* read older than the freshness window is not trusted.
  - **Marginal → Securing.** Economically strong rooms failing *only* on a transient/weak signal (a single unboosted combat creep, recent `PlayerScout`, low `estimated_dps`) route to **Securing** (escort/pre-clear) rather than being thrown away.
  - **GCL / Novice.** `GCL >= owned_rooms + 1` (engine: `user.gcl < calcNeededGcl(claimed+1) → ERR_GCL_NOT_ENOUGH`) and the Novice 3-room cap (`ERR_FULL`) — already partly enforced by `compute_maximum_rooms`; the gate must not admit a target that would `ERR_GCL_NOT_ENOUGH`.
- **Avoid-cooldown map.** A plain-serde `HashMap<RoomName, u32>` (room → until-tick) on `ClaimOperation`, consulted at all three points, mirroring `SalvageRejection { room_name, until_tick }` (`operations/salvage.rs:98`) and pruned with the same `retain(|_, until| *until > game::time())` pattern (`salvage.rs:382`). No entity refs → no `repair_entity_refs` cost. Bounded by pruning each scan.

## 5. Claimer protection & the just-claimed room

The binding mechanic: `claimController` sets only `{user, level:1, progress:0, downgradeTime:null, reservation:null}` — it grants **no** `safeModeAvailable` charge (verified against engine `claimController.js`). The free 20 000-tick safe mode is a respawn-only grant on the *initial* spawn room. A fresh RCL1 claim therefore has `safeModeAvailable == 0` and `activateSafeMode` returns `ERR_NOT_ENOUGH_RESOURCES`; no tower is legal until RCL3 (`CONTROLLER_STRUCTURES.tower = {1:0, 2:0, 3:1, …}`). So the policy is **pre-clear / escort-or-abort, not safe-mode heroics**. Three layers, by *when* each applies:

1. **Don't-send (default, clean rooms).** The §4 gate stops the claimer ever being requested into a contested room. Cheapest protection.
2. **Escort / pre-clear (marginal rooms, the `Securing` sub-state).** `ClaimOperation` proactively builds a `SquadDefenseMission` (`build`/`build_duo`/`build_quad(defend_room_data, home_room_datas)`, sized by `DefenseEscalation::from_threat` — **must be made `pub`**, currently private at `war.rs:90`) sourced from genuine spawn-bearing home rooms, and holds the `[Claim,Move]` request (`SPAWN_PRIORITY_CRITICAL` for the escort, ≥ the claimer's `SPAWN_PRIORITY_HIGH`) until the room is observed clear. For a hostile remnant spawn/tower that must come down first, the salvage `DismantleJob` role is the pre-clear primitive. **"Room held" is not an API on `SquadDefenseMission`** (its `Defending → Cleanup` exit only fires on no-hostiles-AND-all-members-dead). The release predicate is therefore a *direct* read: `!militarily_active() && !hostile_creeps()` on intel fresher than the freshness window, **persisted for N ticks** (anti-flap, default 20) so a one-tick gap between hostile waves does not release the claimer into a re-arming room. The escort's existence is necessary-but-not-sufficient; a hard `Securing` TTL → Abort prevents a stall waiting for a clear that never comes.
3. **After claim, before RCL3 (`Incubate`/`Contested`).** Defenders are spawned for *this* room from real spawn-bearing home rooms (fixing the `war.rs:372–385` `home_set` exclusion — §6). `SafeModeMission` is requested **only** when `controller.safe_mode_available() > 0` (so realistically only at RCL2+, after a level-up earns the first charge) **and** `controller.upgrade_blocked() == 0` — note an enemy `attackController` sets `upgradeBlocked`, which *also* blocks `activateSafeMode` (`safe_mode.rs:227`), so a fresh contested claim's "lifeline" is structurally unavailable; the design treats it as a bonus at RCL2+, never the plan. **Re-validation at adjacency** (`CLAIMING`): on the tick the claimer is in the target room, re-read the gate and abort the claim intent if a combat creep appeared during travel. This is honest but weak (the adjacency tick is the same tick the claimer can be shot); the real protection is the **death counter + exponential backoff** replacing the unconditional `claimers.is_empty() => respawn`: a killed claimer increments `claimer_deaths` in `remove_creep` (`missions/claim.rs:89`), respawn is gated on `claimer_deaths < features.claim.max_claimer_deaths` (default 2) AND the gate still passing, with `last_spawn_tick`-keyed backoff; exceeding the budget → Abort + avoid-cooldown.

## 6. Nascent-colony defense & establishment through RCL3

The RCL1→RCL3 window is the kill zone: no towers, ramparts/walls start at 1 HP, the single RCL1 spawn is a SPOF, and `safeModeAvailable == 0`. Establishment reuses the existing pipeline, made threat-aware:

- **Incubation from a mature home room.** `RemoteBuildMission` (`missions/remotebuild.rs`) is fed from genuine spawn-bearing home rooms (kept pointed at strong homes via `home_room_datas`), so the new colony's spawn-build does not bottleneck on the slow RCL1 spawn — the standard Overmind/Kasami "incubator races the room to RCL3 (first tower) before the attacker can wreck it" pattern. No new mission: `RemoteBuildMission`/`DefendMission` already accept `home_room_datas` arrays. A cheap pre-spawn threat guard is added to `RemoteBuildMission` (`hostile_creeps()` or `threat_level >= PlayerRaid` → stop spawning builders) to cover the window before top-down `Abandoning` teardown fires (closes failure-path step 8).
- **`Incubate` (threat-instrumented).** The existing single `Incubate` state keeps lazily spawning `Construction`/`LocalBuild`/`Haul`/`Tower`/`Defend` children. At the top of its tick it reads the room's threat (see the threat-read note below) + an `EstablishmentRisk` tally (builder/defender deaths, ticks-since-claim with no RCL2 progress, `controller_ticks_to_downgrade` trend). On sustained threat (persistence > 20 ticks) → `Contested`. On RCL3 reached and clear → set the `Established` marker; thereafter the normal `TowerMission` child owns defense and the lifecycle is done.
- **`Contested` (new `ColonyState` variant).** Proactively ensures a `SquadDefenseMission` exists for *this* room, sourced from real homes (fixing both the `home_set` exclusion and the fact that a spawnless room cannot self-spawn defenders); requests `SafeModeMission` only under the §5 charge-and-not-blocked condition; keeps `RemoteBuild` going if survivable; keeps `VISIBILITY_PRIORITY_HIGH` alive so the abort decision uses fresh intel. Runs the no-win evaluation each cadence.
- **`WarOperation` is the reactive backstop.** `run_defense_scan` (`DEFENSE_CADENCE = 2`, never shed) already creates a `SquadDefenseMission` for any `owner().mine()` + visible + player-hostile room (`war.rs:241`), so even before `Contested` fires the colony gets reactive squad defense. The **minimal war change** is to drop the `my()`-spawn requirement from the SafeMode/WallRepair `home_set` eligibility *for owned rooms with a charge available* (`war.rs:372–385`), so the nascent colony is not excluded. To avoid double-spawn, `Contested` owns the *proactive* SquadDefense for the room; war remains the reactive layer (it already dedups by mission presence).

**Threat-read note (resolved up front, not deferred).** `MissionExecutionSystemData` does **not** carry `threat_data` (verified — only `OperationExecutionSystemData` does). The no-win predicate needs `estimated_dps`/`estimated_heal`/`upgrade_blocked` detail that `hostile_threat_creeps()` (a coarse bool, true for any hauler) cannot supply. Therefore **M-plan adds `threat_data: ReadStorage<RoomThreatData>` to `MissionSystemData` and surfaces it on `MissionExecutionSystemData`** (a borrowed-storage addition — **not** a serialized-shape change, no version bump for this alone) before the `Contested`/`Abandoning` predicate ships. We do **not** ship the coarse-bool predicate as the abort trigger.

## 7. Abort & sunk-cost rules

**The abort decision (robust form).** Primary rule, evaluated in `Contested`: **spawnless owned room (no `my()` spawn yet) + sustained player-hostile presence (`threat_level >= PlayerRaid` persisting past the anti-flap window) → Abandon.** This is the community heuristic ("defend only if it has a Spawn or is your only room, else unclaim + avoid") and it is a *single* condition an attacker cannot game by trickling RCL progress or dipping out of vision for a tick — the earlier 3-way-AND framing was rejected for exactly that reason. Richer signals are **accelerators that abort sooner**, never additional AND-conditions:
- `controller.upgrade_blocked() > 0` (an enemy is actively `attackController`-ing us): near-decisive. `attackController` sets a 1000-tick `upgradeBlocked` per strike, which *also* freezes our upgrade to RCL2 (so we can never earn a safe-mode charge) — a "cannot win" signal that aborts immediately.
- death budget (`~2` claimers or `~3` builders), `controller_ticks_to_downgrade` trending to zero, or no RCL2 progress within `features.claim.establishment_stall_ticks` (default ~3000).
- A CPU/energy-stressed empire (`GovernorSnapshot.tier`/`EconomySnapshot`) trips abort sooner (smaller death budget, shorter persistence).

Exceptions that keep `Contested` (don't abort): the room **already has a `my()` spawn**, or it is **our only colony**.

**The un-claim (`Abandoning`).** Use **`StructureController::unclaim()`** — free, instant, one tick, **no creep, no body, no travel, no `upgradeBlocked` dance** (verified present in screeps-game-api 0.23: `objects/impls/structure_controller.rs:120`, `pub fn unclaim(&self) -> Result<(), UnclaimErrorCode>`). This is a **deliberate correction of the candidate designs**, which all proposed reusing `DeclaimJob`/`attackController` on our own controller: `attackController` targets a controller *owned/reserved by another player* — it is the wrong primitive (and slow: −300/CLAIM/tick with a 1000-tick block between strikes ≈ tens of thousands of ticks to neutralize a fresh 20 000-tick RCL1 timer). `DeclaimJob` reuse is **dropped** for self-owned rooms. `Abandoning` therefore: (1) halts/queues-abort for child establishment missions via the existing `child_complete`/`queue_mission_abort` machinery so builders/haulers stop dying; (2) calls `controller.unclaim()` on a creep-less intent the tick the colony is in `Abandoning` (or, if no creep/object access path exists at the mission seam, the smallest possible new one-shot intent — there is **no** `unclaim()` usage anywhere today, so this is a tiny new primitive, not "pure reuse"); (3) tags the room in the avoid-cooldown map; (4) returns `MissionResult::Success` once `!owner().mine()` (the controller reverted to neutral), so the colony mission cleans up. **GCL is preserved** (engine: not refunded on loss, but also not lost) — the room is freely re-claimable after the cooldown decays. `unclaim()` reverts in one tick, so there is no orphan window where the room is `owner().mine()` with neither lifecycle owning it.

## 8. Dynamic re-evaluation

The lifecycle re-evaluates every cadence against live posture, reusing existing gates:
- **CPU.** `ClaimOperation` already sheds discovery under `Tier::Critical` (`claim.rs:273`) and vetoes new claims unless `tier == Normal` (`claim.rs:607`); the escort/pre-clear spend is gated identically (don't open a contested claim while CPU is stressed). `Contested`/`Abandoning` read `MissionExecutionSystemData.governor` and `EconomySnapshot`; a stressed empire trips abort sooner. `WarOperation`'s `DEFENSE_CADENCE = 2` (never shed) keeps the reactive backstop responsive even under Critical.
- **Economy.** `compute_maximum_rooms` (`claim.rs:437`) bounds concurrent claims; escort/pre-clear is only afforded when `EconomySnapshot` supports it (the inputs war already uses).
- **Intel.** Each active target keeps a `VISIBILITY_PRIORITY_HIGH` `VisibilityQueue` request alive on its room (the loop `ClaimOperation` already runs during scouting); `ThreatAssessmentSystem` repopulates `RoomThreatData` each visible tick. `clear_unreachable` on fresh data (as claim already does) prevents a permanently-unscoutable target from being committed; a `Securing`/`Assess` wall-clock TTL → Abort prevents an unscoutable target holding a `compute_maximum_rooms` slot forever.
- **Anti-flap.** Both the `Securing` release and the `Incubate ↔ Contested` boundary are hysteresis-gated (persistence ≥ 20 ticks in *both* directions) so an attacker re-entering after the escort departs does not cause state thrash.

## 9. Components

| Component | Status | File | Responsibility / serialization note |
|---|---|---|---|
| `is_claim_safe()` / `establishment_risk()` | **new** | `src/missions/utility.rs` | Pure, host-testable helpers (like `salvage_worthwhile`) paralleling `is_claim_feasible`. `is_claim_safe(threat_data, dynamic_vis) -> Verdict {Clean, Marginal(escort_size), Reject}` from `threat_level`, `estimated_dps`, `hostile_tower_positions`, `tower_dps_at_edge`, `militarily_active`, reservation/owner, and intel freshness. The only substantial new logic. No serialized state. |
| `ClaimOperation` pre-commit gate + avoid-cooldown | **modified** | `src/operations/claim.rs` | Add dynamic-vis veto to `gather_candidate_room_data` (`:109`); add `RoomThreatData` hard veto + freshness + `Marginal→Securing` routing to `try_score_candidates`/`run_select` (`:390`/`:789`). Add `avoid_cooldown: HashMap<RoomName, u32>` (plain serde, mirrors `SalvageRejection`, pruned each scan). **Serialized-shape change** (new field on `ClaimOperation`). |
| `ClaimMission` hardening (`Securing` + abort/backoff) | **modified** | `src/missions/claim.rs` | Add `claimer_deaths: u32`, `last_spawn_tick: Option<u32>`, `escort_mission: EntityOption<Entity>`. `remove_creep` (`:89`) increments `claimer_deaths`. Death-counter + exponential backoff replacing unconditional respawn; adjacency re-validation; abort on re-arm/budget. **Fix the buggy reservation comment at `:135–143`** ("proceeding anyway") — a foreign reservation *does* block claim (`ERR_INVALID_TARGET`); reject it. **Serialized-shape change**; `escort_mission` needs `repair_entity_refs` coverage (extend `:93`). |
| `ColonyState` machine extension (`Contested`, `Abandoning`) | **modified** | `src/missions/colony.rs` | Add two variants to the `machine!` enum (`:31`). `Incubate` gains `EstablishmentRisk` + `Established` flag; `Contested` holds a self-sourced `SquadDefense` `EntityOption` + conditional `SafeMode` `EntityOption`; `Abandoning` drives `unclaim()` + teardown. **Every new per-state `Entity` slot must be added to `get_children_internal`/`get_children_internal_mut`/`clear_stale_children` (`:88–`) or it leaks.** Gate `can_run` (`:329`) so a freshly-claimed spawnless room is owned by the FSM (it already is, via `owner().mine()`) but `Contested`/`Abandoning` are reachable. **Serialized-shape change.** |
| `unclaim()` abort primitive | **new (tiny)** | `src/missions/colony.rs` (or a one-shot intent helper) | `controller.unclaim()` in `Abandoning`. No `unclaim()` usage exists today; this is a small new primitive, **not** `DeclaimJob` reuse. No serialized state. |
| `MissionSystemData` + `MissionExecutionSystemData` threat wiring | **modified** | `src/missions/missionsystem.rs` | Add `threat_data: ReadStorage<RoomThreatData>` and surface it, so `Contested`/`Abandoning` compute the no-win predicate from real DPS/heal/`upgrade_blocked`. **Borrowed-storage change — NOT a serialized-shape change** (no version bump for this alone). |
| `SquadDefenseMission` (escort + nascent defense) | **reused** | `src/missions/squad_defense.rs` | `build`/`build_duo`/`build_quad(defend_room_data, home_room_datas)` instantiated proactively by `Securing` and `Contested`. No change to the mission. |
| `DefenseEscalation::from_threat` | **modified (visibility)** | `src/operations/war.rs` | Make `pub` (currently private, `:90`) so escort sizing can reuse it. No behavior change. |
| `WarOperation::run_defense_scan` | **modified** | `src/operations/war.rs` | Drop the `my()`-spawn requirement from the SafeMode/WallRepair `home_set` for owned rooms with a charge (`:372–385`); leave reactive SquadDefense as the backstop. No serialized state. |
| `RemoteBuildMission` threat guard | **modified** | `src/missions/remotebuild.rs` | Pre-spawn guard: stop builders if `hostile_creeps()` / `threat_level >= PlayerRaid`. No serialized state. |
| `ClaimFeatures` thresholds + kill-switches | **modified** | `src/features.rs` | Extend `ClaimFeatures` (`:373`) with `safety_gate: bool` (default TRUE), `escort_enabled: bool` (TRUE), `intel_freshness_ticks: u32` (~50), `max_claimer_deaths: u32` (2), `establishment_stall_ticks: u32` (~3000), `avoid_cooldown_ticks: u32`, `abort_persistence_ticks: u32` (20). Loads from `Memory._features` — generally **not** part of the world fingerprint (verify against the `features.rs` load path; if persisted in world state, the version bump below covers it). |
| `DeclaimJob` / salvage declaimers | **untouched** | `src/jobs/declaim.rs` | Explicitly **not** reused for self-rooms (wrong primitive — §7). Remains the foreign/derelict de-claim path. |
| `WORLD_FORMAT_VERSION` | **modified** | `src/game_loop.rs` | Bump **6 → 7** (`:564`). Drivers: `Contested`/`Abandoning` variants + new `Incubate` fields, `ClaimMission` risk fields + `escort_mission`, `ClaimOperation.avoid_cooldown`. One loud, clean reset on deploy (fingerprint path `:628`). Update the version doc-comment block (`:561`). |

## 10. Incremental implementation plan

Each increment is shippable and warning-free; all behavior gated by `features.claim.*` kill-switches (default TRUE per repo convention). **Only M5 bumps `WORLD_FORMAT_VERSION`** — sequence the serialized-shape changes so the version bump lands once.

- **M1 — Read-only safety gate (no new serialized state).** Add `is_claim_safe()`/`establishment_risk()` to `missions/utility.rs` (host-tested). Wire the dynamic-vis veto into `gather_candidate_room_data` and the `RoomThreatData` hard veto + freshness into `try_score_candidates`/`run_select`. **No new fields** — pure gating using data already in scope. Gated by `features.claim.safety_gate`. This alone closes failure-path steps 1–4 (no claimer dispatched blind). *Smallest, highest value, no reset.*
- **M2 — Fix the reservation bug + `RemoteBuildMission` threat guard.** Reject foreign reservations in `ClaimMission` (drop the "proceeding anyway" path at `:135`); add the builder pre-spawn threat guard. No serialized state. Closes step 8's leading edge.
- **M3 — War coverage fix + `DefenseEscalation::from_threat` `pub`.** Relax the `home_set` SafeMode/WallRepair exclusion for owned-spawnless rooms with a charge; make the escalation helper `pub`. No serialized state. Gives the nascent colony reactive coverage immediately.
- **M4 — Mission-layer threat wiring.** Add `threat_data` to `MissionSystemData`/`MissionExecutionSystemData` (borrowed storage, **no version bump**). Prerequisite for the no-win predicate; ships behavior-neutral (nothing reads it yet).
- **M5 — Claimer hardening + `Securing` escort + avoid-cooldown (version bump 6→7).** Add `ClaimMission` risk fields + `escort_mission` + `ClaimOperation.avoid_cooldown`; death counter / backoff / adjacency re-validation; proactive `SquadDefenseMission` escort with the persistence-gated release predicate + `Securing` TTL. Extend `repair_entity_refs` + add the repair unit test. **Bump `WORLD_FORMAT_VERSION` here** (the first serialized-shape change). Gated by `features.claim.escort_enabled`. Closes step 5 (the no-abort claimer loop).
- **M6 — `Contested` state.** Add the `ColonyState::Contested` variant + `EstablishmentRisk`/`Established` on `Incubate`; threat-instrument `Incubate`; self-sourced SquadDefense + conditional SafeMode; anti-flap on both boundaries. Extend `get_children_internal`/`clear_stale_children` for the new slots. (Rides the M5 version; if M6 ships separately, it is the version bump instead — land M5+M6 together if practical to bump once.) Closes step 9 (proactive nascent defense).
- **M7 — `Abandoning` state + `unclaim()`.** Add the `Abandoning` variant; implement the robust abort predicate (spawnless + sustained-hostile, with accelerators); `controller.unclaim()` + child teardown + avoid-cooldown tag + `Success` on revert. Extend `repair_entity_refs`/`clear_stale_children`. Closes step 10 (the unwired exit) — completing the lifecycle. Gated by a `features.claim.declaim_on_abort`-style kill-switch.

## 11. Open risks & questions for the human

- **`unclaim()` at the mission seam.** `controller.unclaim()` needs a live `StructureController` object (room vision). A room being abandoned because it's contested *is* visible (our creeps/structures are there), so this should hold — but confirm the cleanest place to issue the intent (a one-shot job vs. a direct intent in the colony tick). **Q: is a creep-less controller intent reachable from the `ColonyState` tick, or do we need a tiny `UnclaimJob`?**
- **Threshold tuning is live-only.** `intel_freshness_ticks`, `max_claimer_deaths`, `establishment_stall_ticks`, `abort_persistence_ticks`, `avoid_cooldown_ticks` need observation against real attackers. Too eager abandons winnable rooms; too patient bleeds. All config-gated; **Q: acceptable to ship M1–M4 and observe before tuning M5–M7 abort thresholds?**
- **Escort can lose.** Against a committed, healed player combat creep an RCL1-sourced escort can be wiped. The safety valve is the no-win predicate + Abandoning. **Q: confirm the empire should *abort* rather than escalate to a full war squad for a single contested expansion — i.e. expansion never out-commits the home economy.**
- **`repair_entity_refs` surface.** M5–M7 add `escort_mission` + `Contested` SquadDefense/SafeMode + (no creep ref for unclaim) entity slots. This is the operator's documented dangling-ref/hang class. Mitigation: extend `get_children_internal`/`clear_stale_children` + a unit test mirroring `ClaimMission`'s repair test, **before** M5 ships.
- **One-time reset re-runs every colony's `Incubate`.** The 6→7 bump resets every serialized `ColonyMission` to fresh `Incubate`, transiently re-running all child-mission creation. Standard per policy, but **Q: confirm deploy window / operator sign-off (Phase-1 process).**
- **Avoid-cooldown lockout.** A camped-then-departed room stays avoided until the cooldown decays; mis-tuned decay starves expansion of a good room. Bounded by pruning; decay value is a tuning question.

## 13. Defense staleness — defense is subordinate to ownership

A separate but adjacent failure the operator hit live: a `SquadDefenseMission`
stuck holding a room that had already been **manually de-claimed** while losing
— it kept respawning defenders (and other home rooms kept sourcing creeps into
it) because its only exit (`Defending → Cleanup`) fires on *no-hostiles AND
all-members-dead*; a de-claimed room with a hostile still inside never reaches
it. The war/squad system needs a broader overhaul (out of scope here), but the
anti-stuck invariant is cheap and lands now:

- **A `SquadDefenseMission` self-terminates the moment fresh intel shows the
  defended room is not `owner().mine()`** (`missions/squad_defense.rs`,
  `run_mission` guard, `DEFENSE_OWNERSHIP_STALE_TICKS = 100`). Defense exists to
  protect an *owned* room; a de-claimed / lost / abandoned room is not ours to
  defend, so we stop spawning into it and other homes stop sourcing creeps.
- **This is also the abort cascade.** The colony no-win abort (§7) just calls
  `controller.unclaim()`; the room flips to neutral, and *every* defense mission
  for it (war-reactive or future-escort) self-terminates on the next tick via
  the same ownership guard — no cross-operation teardown signal needed. Defense
  is strictly subordinate to ownership, which is the clean integration with
  overall base protection: `WarOperation::run_defense_scan` only *creates*
  `SquadDefenseMission` for `owner().mine() && visible()` rooms, and the mission
  now also *destroys* itself the moment that stops holding.
- A room under active defense has our creeps in it, so it is seen every few
  ticks; the 100-tick freshness window only avoids reacting to a one-off stale
  read (not a permanent grace).

## 14. Implementation status & deviations (shipped 2026-06-14)

Shipped M1–M7 **except the escort/Securing layer**, with these grounded
simplifications vs. §3–§9 (all verified in-tree, all builds warning-free, world
fingerprint bumped 6 → 7 by the concurrent planner-spawn change so this rides
one reset):

- **No new `ColonyState` variants.** The `Contested`/`Abandoning` behavior is
  folded into the existing `Incubate` state: it gains a single `contested_since:
  Option<u32>` field and, at the top of its tick, evaluates the no-win abort
  (`should_abandon_claim`, `missions/utility.rs`, host-tested) and, on a verdict,
  calls `controller.unclaim()` + tags avoid-cooldown + returns `Err` (top-down
  teardown of children via the standard mission-failure path). This is strictly
  smaller than adding machine states and needs no `get_children_internal` change
  (the new field is not an entity ref).
- **Defense is delegated to war, not self-sourced.** `WarOperation`'s reactive
  `run_defense_scan` already creates `SquadDefenseMission` for any owned + visible
  + player-hostile room **including a spawnless nascent colony** (no spawn
  requirement on the scan), so the colony does not own a proactive SquadDefense
  child. Combined with §13's ownership-subordinate self-termination, defense
  ramps up and winds down automatically around the claim's lifetime.
- **The SafeMode `home_set` fix was dropped as moot.** A nascent colony has
  `safeModeAvailable == 0` and no ghodium to `generateSafeMode`, so SafeMode
  cannot fire regardless of the `home_set` membership; including the spawnless
  colony buys nothing. `DefenseEscalation::from_threat` was still made `pub`
  for the future escort.
- **Avoid-cooldown is an ephemeral `ExpansionAvoidance` Resource** (`expansion.rs`),
  not a serialized field on `ClaimOperation` — written by the claimer abort and
  the colony abort, read by the pre-claim gate. It only needs to prevent
  re-claim thrash within a VM lifetime; after a reset the safety gate re-vetoes
  a still-contested room anyway.
- **Claimer hardening shipped (anti-stuck), escort deferred.** The
  death-counter + exponential respawn backoff + abort-on-budget
  (`max_claimer_deaths`) shipped because it has no squad dependency and is the
  user's "don't get stuck." The **Securing escort / pre-clear** (a proactive
  `SquadDefenseMission` gating the claimer) is **DEFERRED to the squad/combat
  overhaul** — see ADR 0008. Until then a *marginal* room is conservatively
  treated as unsafe (rejected), not escorted.

**Updated M-plan status:** M1 (safety gate) ✅ · M2 (reservation reject +
builder threat guard) ✅ · M3 (`from_threat` pub; war already covers spawnless,
SafeMode fix dropped) ✅ · M4 (`threat_data` + `ExpansionAvoidance` on the
mission/op execution data) ✅ · M5a (avoid-cooldown + claimer abort/backoff) ✅ ·
**M5b (escort/Securing) DEFERRED → ADR 0008 overhaul** · M6+M7 (folded into
`Incubate`: no-win abort + `unclaim()` + avoid tag) ✅ · defense-staleness
self-termination (§13) ✅. Everything is gated by `features.claim.safety_gate`
and `features.claim.abort_on_contest` (both default TRUE).

## 12. References

Bot/community: Overmind ExpansionEvaluator/Planner (economy-only scoring, adjacency-only `avoid`) — https://github.com/bencbartlett/Overmind/blob/master/src/strategy/ExpansionEvaluator.ts , https://github.com/bencbartlett/Overmind/blob/master/src/strategy/ExpansionPlanner.ts ; colonize directive (claim+pioneer, no threat-abort, no safe mode) — https://github.com/bencbartlett/Overmind/blob/master/src/directives/colony/colonize.ts ; claimer/pioneer overlords (zero combat/escort) — https://github.com/bencbartlett/Overmind/blob/master/src/overlords/colonization/claimer.ts , .../pioneer.ts ; Overseer auto-safe-mode (structurally can't fire for a nascent colony) — https://github.com/bencbartlett/Overmind/blob/master/src/Overseer.ts ; incubate directive — https://github.com/bencbartlett/Overmind/blob/master/src/directives/colony/incubate.ts ; clearRoom (claim→demolish→**unclaim** as a normal transition) — https://github.com/bencbartlett/Overmind/blob/master/src/directives/colony/clearRoom.ts ; lifecycle-hang cautionary tale — https://github.com/bencbartlett/Overmind/issues/107 ; TooAngel "new rooms should auto-safe-mode" — https://github.com/TooAngel/screeps/issues/131 ; KasamiBot (self-renew expansion workers, support-to-RCL3) — https://kasami.github.io/kasamibot/features.html ; community defend-or-unclaim rule + pre-claim safety heuristics — https://github.com/NobodysNightmare/screeps-ai , https://wiki.screepspl.us/Claiming_new_room/ , https://wiki.screepspl.us/index.php/Great_Filters , https://screeps.com/forum/topic/1942 .

Screeps mechanics (load-bearing): `claimController` 1-tick / single-CLAIM / neutral-only / `ERR_INVALID_TARGET` on foreign reservation / `ERR_GCL_NOT_ENOUGH` — https://raw.githubusercontent.com/screeps/engine/master/src/processor/intents/creeps/claimController.js , https://raw.githubusercontent.com/screeps/docs/master/api/source/Creep.md ; fresh claim grants **no** safe-mode charge + `activateSafeMode` block conditions (`safeModeAvailable`, `safeModeCooldown`, `upgradeBlocked`, downgrade threshold; one room/shard) — https://raw.githubusercontent.com/screeps/engine/master/src/processor/intents/controllers/activateSafeMode.js , https://docs.screeps.com/defense.html , https://docs.screeps.com/respawn.html ; towers only at RCL3 (`CONTROLLER_STRUCTURES`), downgrade timers, `attackController` −300/CLAIM/tick + 1000-tick `upgradeBlocked` (freezes our upgrade too) — https://raw.githubusercontent.com/screeps/common/master/lib/constants.js , https://raw.githubusercontent.com/screeps/engine/master/src/processor/intents/controllers/tick.js ; GCL not refunded / room reverts to neutral — https://docs.screeps.com/respawn.html ; `StructureController.unclaim()` (free, instant, no creep) — https://docs.screeps.com/api/#StructureController.unclaim , https://screeps.com/forum/topic/1897/unclaiming-a-room-triggers-safemode . Verified in-tree against `screeps-game-api 0.23` (`objects/impls/structure_controller.rs:120`).
