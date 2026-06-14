# ADR 0008 — Combat & Squad Architecture (objective-driven missions + a generic Squad Manager)

- **Status:** Proposed
- **Date:** 2026-06-09
- **Deciders:** William Archbell
- **Related:** Field Report A (war/squad cohesion), Field Report B (operation/mission lifecycle hangs); IBEX-001, IBEX-002, IBEX-002b, IBEX-026, IBEX-027, IBEX-028, IBEX-029, IBEX-041, IBEX-042, IBEX-043 (review report §1). Sibling ADRs: **0001** (entity model / `SquadStore` / `SquadId`), **0003** (behavior modeling — owns the FSM + cohesion *movement* and the SquadDefense-wiring; this ADR owns the *layering*), **0004** (CPU governance / budgeted pathfinding facade), **0005** (runtime/scheduling + tick-level containment), **0006** (eval harness). Prior art: Overmind Overlord/Directive/Swarm (`../references/external-references.md`).

> **Scope boundary (read first).** This ADR is the **architecture / layering** decision for the combat system: *which* combat missions should exist, and the seam between an objective-bearing mission, a generic squad manager, and the spawn system. It does **not** re-decide the behavior model. ADR 0003 owns: the data-driven FSM, the one-guarded-intent-sink (IBEX-029), the cohesion *movement* model (lead-follower + hard in-range wait-gates replacing the virtual-anchor mover), and the first concrete step "wire `SquadDefenseMission` onto `SquadContext`". ADR 0001 owns squad *identity* (`SquadId`/`SquadStore`, replacing the raw-u32 `squad_entity` of IBEX-002b). This ADR consumes those decisions; it does not duplicate or contradict them. Where this ADR says "the manager hard-waits / replaces / retires", the *mechanism* of movement and the *key* of identity are ADR 0003 / ADR 0001 respectively.

## Context

The combat subsystem is the review's most fragile area after serialization (review §Exec, "3 most fragile subsystems #2: two incompatible squad models, broken cohesion, dead economy-abort, no lifecycle watchdog"). Reading the code confirms a structural cause that the per-finding view fragments: **there is no layer that owns squad lifecycle.** Each combat mission re-implements — inconsistently and partially — spawning, replacement, rally, formation, and teardown, and there are *three* squad behavior models (one live, one half-wired, two dead).

Forces / constraints (per AGENTS.md + the mechanics digest):
- **Single-threaded WASM**, cooperative only. No threads/locks; squad coordination is per-tick recompute, not messaging.
- **CPU = execution + intents** (0.2 CPU/intent). Renew, move, attack, heal, transfer are each intents. A squad of 4 large creeps each renewing + moving + attacking is ~2.4+ CPU/tick of intents alone; an attack that *never engages but keeps renewing* (Field Report B) is a permanent intent drain.
- **CREEP LIFETIME IS FINITE.** 1500 ticks (CLAIM 600), decremented each tick; parts decay; renew adds `floor(600/body.length)` TTL/tick (a 40-part quad member regains only ~15 TTL/renew-tick while costing energy + an intent + spawn occupancy) and is hard-capped at 1500, and a CLAIM-part creep cannot be renewed. **Any squad design MUST plan replacement/pre-spawn and retirement, not persistence** — a 4-creep quad assembled over ~150 ticks of travel will start losing members to TTL during a long siege, so the manager must pre-spawn successors, not endlessly renew.
- **VM-reset resilience.** A squad's identity and roster must survive a reset; today it does not reliably (IBEX-002b).
- **Incremental, strangler-fig.** One stable seam per step; back-compat not required at a *labelled* cutover but never break the running bot mid-increment.

### What exists today (verified against code)

The current war system spans `operations/war.rs` (singleton coordinator) → `operations/attack.rs` (per-target campaign) → `missions/attack_mission.rs` (the offense squad lifecycle, 2040 lines) / `missions/squad_defense.rs` (the defense path) → `military/squad.rs` (`SquadContext` ECS component + `PreRun/RunSquadUpdateSystem`) → `jobs/squad_combat.rs` (per-creep combat). Spawning is direct: a mission calls `spawn_queue.token()` + `spawn_queue.request(home_room_entity, SpawnRequest{..callback})` and `spawn_queue.request_renew(...)` (`spawnsystem.rs:74/82/98`); the callback runs inside `SpawnQueueSystem::process_room_spawns` (`spawnsystem.rs:253`) and itself constructs the `JobData::SquadCombat` and registers the creep onto the `SquadContext` via `add_member` (`attack_mission.rs:553–559`, `squad_defense.rs:207/224`).

Three things are structurally wrong with this:

1. **Two incompatible squad behavior models + two dead ones (IBEX-001, IBEX-043).**
   - `AttackMission` (offense) *does* use `SquadContext` and the virtual-anchor formation mover (`attack_mission.rs:553` `SquadCombatJob::new_with_squad`; `formation.rs:201` `advance_squad_virtual_position`).
   - `SquadDefenseMission` (the **only live defense path**, built from `war.rs:319/326/333/432/524/531` for ALL owned-room, defend-flag, and remote-invader defense) sets `squad_entity: None.into()` (`squad_defense.rs:455`) and builds creeps with the squad-**less** `SquadCombatJob::new` (`squad_defense.rs:207/224`). So every defense creep runs solo fallback — scatter **by construction**.
   - `SquadAssaultMission` (`squad_assault.rs`) and `SquadHarassMission` (`squad_harass.rs`) are **orphaned dead code**: a crate-wide search finds **zero** `SquadAssaultMission::build` / `SquadHarassMission::build` call sites; both also use the squad-less `SquadCombatJob::new` (`squad_assault.rs:240/255/270`). They are registered in `missions/data.rs:33–34` purely as serialized variants.

2. **No lifecycle owner / no watchdog (IBEX-002, Field Report B).** `WarOperation::run_operation` returns `Ok(OperationResult::Running)` forever (`war.rs:1442`) and only drops *already-dead* children (`child_complete` → `remove_active_attack`, `war.rs:1330`). `AttackMission` Rallying gates `→Engaging` on `squad_is_cohesive` (a scattered squad never satisfies it; `attack_mission.rs:687`) **while** Rallying renews members below TTL 1200 (`attack_mission.rs:644–648`, threshold `:22`) — so the only natural terminator (`all_squads_wiped`, `:588/:1044`) is *actively prevented*, and the campaign neither engages nor dies. `SquadDefenseMission` Defending exits only on `!has_hostiles && all_dead` (`squad_defense.rs:313`), so a *surviving* defense squad idles forever after the threat clears. `AttackOperation::should_abort`'s economy branch is dead code (`attack.rs:402`, `total_energy_invested` never written — IBEX-026), and `WarOperation` never force-aborts excess attacks when the cap shrinks (IBEX-028, `war.rs:559–569` gate blocks *new* launches only).

3. **Identity & intent hazards (IBEX-002b, IBEX-029) — owned elsewhere but they bite the combat layer.** The creep→squad link is a raw `Entity::id()` u32 (`jobs/squad_combat.rs:18`, resolved `attack_mission.rs:550` `world.entities().entity(squad_entity_id)`), which after a reset/recycle resolves `None` or a *foreign* squad → solo scatter (ADR 0001 fixes the key). Combat intents are unguarded (`squad_combat.rs`, IBEX-029; ADR 0003 routes them through the sink). The boost pipeline is inert (IBEX-027): `BoostQueue` is plumbed into `MissionExecutionSystemData` (`missionsystem.rs:36/61`) but no combat mission populates a request, so boosted compositions (`composition.rs` `BoostedQuadMember`/`BoostedTank`/…) spawn unboosted.

**Why now:** the war system "cannot reliably convert an economic lead into territory" (review competitive verdict) precisely because lifecycle is everyone's job and therefore no one's. Fixing this per-mission would re-spread the same logic; the leverage is to **extract one squad-lifecycle owner** so missions shrink to objectives.

## Decision

Adopt a **three-layer combat architecture** in which **missions declare objectives + force requirements and own nothing else**, a **generic Squad Manager owns all squad lifecycle**, and the **spawn system stays a dumb fulfiller**. This is the Overmind inversion (the *mission* is the spawn-requesting owner via a declarative reconciler, not the per-creep role) specialized to Ibex's operation→mission→job stack and to the finite-lifetime / VM-reset constraints (`../references/external-references.md` §1, §4, §6).

### 1. Mission inventory — KEEP / MERGE / DELETE

> "Instantiated?" = has a live `::build*` call site (verified by crate-wide search). "Squad?" = participates in the combat-squad layer (`SquadContext` + `SquadCombatJob`).

| Mission | Instantiated? / by whom | Squad layer today | Overlap / duplicate | Verdict |
|---|---|---|---|---|
| **AttackMission** (`attack_mission.rs`, 2040 L) | ✅ `attack.rs:772` (per wave, from `AttackOperation`) | ✅ `SquadContext` + `new_with_squad`; virtual-anchor mover | The *one* live multi-squad lifecycle; reimplements spawn/rally/renew/formation/wave-wipe inline | **MERGE** — becomes a thin objective declarer (`secure room X`); lifecycle moves to the Squad Manager. Shrinks from 2040 L to a force-plan + objective + success-poll. |
| **SquadDefenseMission** (`squad_defense.rs`) | ✅ `war.rs:319/326/333/432/524/531` (owned-room, defend-flag, remote-invader) | ⚠️ **squad-LESS** (`squad_entity=None` `:455`; `SquadCombatJob::new`) — scatter by construction (IBEX-001 L1) | Duplicates AttackMission's spawn/rally/escalate FSM with a *different*, broken model | **MERGE** — becomes the `defend room Z` objective on the *same* manager (ADR 0003 §B.1 wires it onto `SquadContext` first as the quick-win). Defense and offense converge on one squad model. |
| **SquadAssaultMission** (`squad_assault.rs`, 592 L) | ❌ **zero** `::build` call sites | ⚠️ squad-LESS (`SquadCombatJob::new` `:240/255/270`) | Dead re-implementation of AttackMission's offense; superseded by AttackMission's force-plan | **DELETE** (confirmed orphan, IBEX-043 / ADR 0003 §B.4). Remove the `MissionData::SquadAssault` variant (`data.rs:33`). |
| **SquadHarassMission** (`squad_harass.rs`, 357 L) | ❌ **zero** `::build` call sites | ⚠️ squad-LESS | Dead; harass is already an `AttackReason::ResourceDenial` force-plan (`attack.rs:193`, `SquadTarget::HarassRoom`) | **DELETE** (orphan). Remove the `MissionData::SquadHarass` variant (`data.rs:34`). |
| **DefendMission** (`defend.rs`) | ✅ `colony.rs:273`, `miningoutpost.rs:434/588` | ❌ **not a squad mission** — spawns nothing; Idle/Active flag observer exposing `is_room_safe()` (`defend.rs:92/132`) | None — it is a *threat-state signal* consumed by economic missions, not a combat unit | **KEEP** (rename-candidate `RoomSafetyMission`). Out of scope for the squad manager; it produces no creeps. Note its name collides conceptually with the `defend room Z` objective — disambiguate in docs. |
| **RaidMission** (`raid.rs`) | ✅ `miningoutpost.rs:306` | ❌ uses `JobData::Haul` (`raid.rs:75`), not combat | Post-conquest *looting*; economic, not war | **KEEP** as-is. (Holds the IBEX-010 nuker-withdraw panic via its transfer registration — fixed in ADR 0005, not here.) |
| **DismantleMission** (`dismantle.rs`) | ✅ `miningoutpost.rs:374` | ❌ dismantle job; economic (clearing blocking structures for remotes) | None with the war path | **KEEP** as-is (economic). A *combat* dismantle is instead a `dismantle target Y` objective force-plan on the manager (`SquadComposition::duo_drain`/siege). |
| **NukeDefenseMission** (`nuke_defense.rs`) | ✅ `war.rs:363` | ❌ no spawns; manages ramparts/structure HP vs incoming nukes | None | **KEEP** as-is (utility). |
| **SafeModeMission** (`safe_mode.rs`) | ✅ `war.rs:374` | ❌ no spawns; activates controller safe mode under threat | None | **KEEP** as-is (utility). |
| **WallRepairMission** (`wall_repair.rs`) | ✅ `war.rs:385` | ❌ repair job; economic/defensive | None | **KEEP** as-is (utility). |

**Net:** the combat-squad layer collapses from **four** mission types (1 live-correct, 1 live-broken, 2 dead) to **one objective-bearing mission shape** served by **one** manager. `NukeDefense`/`SafeMode`/`WallRepair`/`Defend`/`Raid`/`Dismantle` are *not* squad missions and remain untouched (they are defensive-utility or economic and already work through their own non-squad job paths).

### 2. The unified design — objective-driven missions + a generic Squad Manager

Three layers with a hard seam between each:

```
Operation (campaign)        AttackOperation / WarOperation
        │  declares
        ▼
Objective / Mission         "secure room X" / "dismantle target Y" / "defend room Z"
        │  registers an OBJECTIVE { kind, force_requirement, deploy_policy }
        ▼
Squad Manager  (NEW)        owns: SquadId minting, composition→roster, spawn-request
        │                   generation, replacement / pre-spawn, retirement (force-abort)
        ▼
Squad  (SquadStore/SquadId) SquadContext: roster, formation state, tick orders
        │  per-member tick orders
        ▼
Job (jobs/squad_combat.rs)  executes ONE creep's intents this tick (no spawn/lifecycle logic)
```

**Layer responsibilities (the new contract):**

- **Mission declares an OBJECTIVE, nothing else.** An objective is data:
  `Objective { kind: Secure{room} | Dismantle{target} | Defend{room} | Harass{room} | Farm{powerbank|sk|core}, force: ForceRequirement, deploy: DeployPolicy, success: SuccessPredicate }`.
  (Planned `kind` extensions on the same manager, new variants only: `Downgrade{room}` / `Claim{room}` for controller warfare — downgrade-timer and claim/attackController mechanics per [`../references/engine-mechanics.md`](../references/engine-mechanics.md) §controller warfare.)
  `ForceRequirement` is the existing `Vec<PlannedSquad>` (`attack_mission.rs:42` — `composition` + `target` + `deploy_condition`), unchanged in shape. The mission **does not** call `spawn_queue`, **does not** hold `squad_entities`, **does not** renew, **does not** run the rally/wave FSM, and **does not** clean up squads. It registers the objective with the manager, polls `manager.objective_status(id)` for `{Forming, Engaged, Succeeded, Failed, Impossible}`, and reports `MissionResult` accordingly (using `MissionResult::Wait` from ADR 0003 §A.6 for transient visibility loss instead of tearing down). AttackMission's 2040 lines reduce to this declaration + the success poll it already does (`attack.rs:743` `mission_succeeded`).

- **Squad Manager owns the full squad lifecycle.** A single ECS resource/system (`SquadManager`) that, each tick, reconciles **declared objectives** against **live squads**:
  1. **Composition → roster.** For each objective force requirement, map each `PlannedSquad.composition` to a roster of `(SquadRole, slot_index, BodyType)` via the existing `SquadComposition` (`composition.rs`). Mint a `SquadId` (ADR 0001 A2) and create the `SquadContext` in the `SquadStore`.
  2. **Spawn-request generation (declarative reconciler — the Overmind `wishlist`).** Compute *desired vs. current* roster per squad and emit only the shortfall as `SpawnRequest`s to `spawn_queue` (the existing API, unchanged), with the registration callback writing back the new member's `SquadId` + slot (the callback is now owned by the manager, not the mission). This is the single producer of combat spawn requests.
  3. **Replacement / pre-spawn (finite lifetime — the core constraint).** Instead of the renew-forever loop (`attack_mission.rs:644`), the manager **pre-spawns a successor** for any member whose `ticksToLive < spawn_time(body) + travel_estimate + PRESPAWN_MARGIN` so the replacement arrives as the incumbent expires (Overmind `lifetimeFilter`, §4). Renew is **never a squad mechanism**: ADR 0011 D8 owns renew policy and confines it to orchestrator-internal use on small (≤~15-part) **unboosted** utility creeps at otherwise-idle spawns — combat bodies (36–48 parts, regaining only ~12–16 TTL/renew-intent) and CLAIM-part creeps are categorically renew-ineligible, so **pre-spawn is the squad's only lifetime mechanism**. This directly answers the digest's renew-economics warning: pre-spawn-and-retire beats renew-and-decay.
  4. **Retirement / force-abort (closes Field Report B).** The manager force-retires a squad when ANY of: (a) the objective reports `Succeeded`/`Impossible`; (b) the objective is withdrawn (mission deleted, operation aborted); (c) **cohesion-as-invariant** — the squad is non-cohesive for `> N` ticks (ADR 0003 §B.3 supplies the cohesion metric and the lead-follower movement; this ADR makes the manager the *enforcer* that converts "non-cohesive > N" into a clean teardown); (d) the squad is orphaned (its objective/owner no longer exists); (e) a per-state **wall-clock deadline** is exceeded (Forming/Rallying ~150t, Engaged ~400t, Defending-no-hostiles ~50t — IBEX-002). Retirement releases the roster (recycle near spawn if cheap, else release to the job pool / suicide), deletes the `SquadContext`, and frees the `SquadId`. With squad renew abolished outright (§2.3 / ADR 0011 D8), nothing sustains a non-cohesive squad past its deadline and the all-dead terminator can finally fire.

- **Spawn system stays a dumb fulfiller.** `spawnsystem.rs` is unchanged: it fulfills `SpawnRequest`s by priority and runs the callback. The manager is the only combat caller; missions no longer touch it. (This preserves the verified-correct priority ordering, review §1 false-positive guard.)

**The seams (explicit, testable):**

- **Mission ↔ Manager:** `register_objective(owner, Objective) -> ObjectiveId` and `objective_status(ObjectiveId) -> ObjectiveStatus`. The mission never sees a `SquadId` or an `Entity`; it sees only status. This is the seam that lets AttackMission/SquadDefenseMission converge — both register objectives differing only in `kind` + `force`.
- **Manager ↔ Squad:** keyed by `SquadId` through the `SquadStore` (ADR 0001 A2). A lookup miss is a handled `None`, not a panic — which is *why* this ADR is gated behind ADR 0001 (the raw-u32 `squad_entity` cannot safely be a cross-tick owner key).
- **Manager ↔ Spawn:** the existing `spawn_queue.token()/request()/request_renew()` API (`spawnsystem.rs:74/82/98`). No change to the spawn system; the manager simply becomes the sole combat producer.

**Objective → composition mapping** is preserved from `AttackOperation::build_force_plan` / `plan_by_detected_threat` (`attack.rs:161/212`): detected DPS/heal/tower counts pick `solo_ranged` / `duo_attack_heal` / `quad_ranged` / `duo_drain + quad_ranged` (`composition.rs`). That logic stays in the *operation/mission* (it is strategy, "what force does this objective need"); the manager only *fulfills* the resulting `ForceRequirement` (it is mechanism, "keep this roster alive and cohesive"). This keeps strategy and lifecycle on opposite sides of the seam.

**Member dies mid-objective → replace vs. retire (the manager's decision, not the mission's):**
- **Replace** (default) if the objective is still `Forming`/`Engaged`, the economy can afford it (reuse `economy.can_rooms_afford_military`, `economy.rs:91`), AND the surviving roster is still viable (e.g. the squad is not down to lone healers). Pre-spawn the replacement per §2.3.
- **Retire-and-rebuild the whole squad** if a replacement would be wildly mismatched in TTL vs. survivors (Overmind `isExpired`, §3) — a fresh 1500-TTL creep joining three 200-TTL survivors is a coordination liability; better to let the wave wipe and respawn cohesively (the existing wave model, `attack_mission.rs:836` `handle_wave_wipe`, generalized into the manager).
- **Retire the squad** (no replace) if the objective is `Succeeded`/`Impossible` or the economy can no longer afford it (this is also where IBEX-026's dead economy-abort is *actually* wired: the manager tracks real per-squad spend and reports `Impossible` upward when surplus collapses).

**Objective completes / becomes impossible → release the squad.** When `SuccessPredicate` is met (no dangerous hostiles remain — `attack_mission.rs:1030` `room_has_hostile_threats`; or target structure destroyed) the manager marks `Succeeded`, releases the squad (loot/exploit is a *separate* `Farm`/`CollectResources` objective or the existing `RaidMission`, not a held combat squad), and the mission completes. When impossible (max waves, safe-mode wall, unreachable, economy collapse, deadline) it marks `Failed`/`Impossible`, force-retires, and the mission/operation tears down. This gives the war lifecycle the **definite terminator** it lacks today.

### 3. Layering & interaction analysis (in depth)

The current call graph (verified): `WarOperation`(singleton) → `AttackOperation`(per target) → `AttackMission`(per wave) → `SquadContext` + `jobs/squad_combat.rs`, with `SquadDefenseMission` hanging directly off `WarOperation`. The pain is that **lifecycle logic is smeared across all four** instead of living in one place:

- **Operation layer** (`war.rs`, `attack.rs`) — *strategy & supervision*. Picks targets, scores candidates (`attack.rs:212`), sizes force plans, allocates home rooms (`war.rs:1097`), and SHOULD supervise children with deadlines. Today `WarOperation` is a perpetual `Running` with no child-abort (IBEX-002, `war.rs:1442`) and no excess-attack trimming (IBEX-028). **Fix:** `WarOperation` becomes a true supervisor — it age-aborts children and trims attacks when `max_concurrent_attacks` shrinks, by *withdrawing the objective* (which the manager turns into a squad force-retire). Strategy stays here; lifecycle mechanism moves down to the manager.
- **Objective/mission layer** (`AttackMission`, ex-`SquadDefenseMission`) — *what to achieve*. Reduces to objective declaration + status polling + transient-fault tolerance (`MissionResult::Wait`, ADR 0003 §A.6 / IBEX-042). No spawn, no renew, no formation, no squad cleanup. This is where the **2040-line `attack_mission.rs` and the broken `squad_defense.rs` collapse** into one small shape.
- **Squad-manager layer** (NEW) — *keep the declared force alive, cohesive, and bounded*. The single owner of: `SquadId` minting, roster reconciliation, spawn-request generation, replacement/pre-spawn, cohesion-invariant enforcement, deadline/orphan force-abort, retirement. This is the layer that **did not exist** and whose absence is the root of Field Report B.
- **Squad layer** (`SquadContext`/`SquadStore`) — *roster + formation state*. Unchanged in role; re-keyed by `SquadId` (ADR 0001). The `PreRun/RunSquadUpdateSystem` (`squad.rs:936/1004`) that already prunes dead members and degrades formation is reused as-is.
- **Job layer** (`jobs/squad_combat.rs`) — *one creep's intents this tick*. Reads its `SquadContext` tick orders and emits attack/heal/move through the **one guarded sink** (ADR 0003 §A.2 / IBEX-029). No lifecycle, no spawn, no targeting-of-last-resort scatter once it always has a squad.

**Where this sits vs. the report:**

- **Field Report B lifecycle-hang ROOT (IBEX-002):** the root is "no layer owns 'is this squad still worth keeping alive?'". This ADR *creates* that layer. The renew-forever-while-never-cohesive deadlock (`attack_mission.rs:644` renew + `:687` cohesion gate) is broken because the manager (a) never renews squads at all (§2.3 / ADR 0011 D8) and (b) force-retires on the cohesion invariant and the per-state deadline — letting `all_squads_wiped` fire or the squad be torn down cleanly.
- **IBEX-001 (cohesion):** L1 (defense unwired) is closed by MERGE-ing `SquadDefenseMission` onto the manager's one squad model (ADR 0003 §B.1 does the concrete wiring; this ADR makes it architectural, not a special case). L2 (offense soft-quorum) is closed by ADR 0003's lead-follower movement; this ADR's contribution is making **cohesion an enforced retirement invariant** at the manager, not a self-relaxing gate inside the mission (`attack_mission.rs:752`).
- **IBEX-002b (raw-u32 squad ref):** the manager's `SquadId` key (ADR 0001 A2) replaces the `squad_entity: Option<u32>` (`squad_combat.rs:18`). This ADR is *gated* on that fix — a lifecycle owner keyed on a recyclable index would resurrect the aliasing.
- **IBEX-026 (dead economy-abort):** the manager tracks real per-squad spend and is the natural place to compute "objective impossible — economy collapsed", finally giving `should_abort`'s economy branch (`attack.rs:402`) a live producer.
- **IBEX-027 (inert BoostQueue):** with one combat demand producer, the manager is the single place combat boost *demand* originates — it declares `boosts` from `composition.required_boosts()` (`composition.rs:552`) on its demands and gates deploy on `is_ready`; the `BoostQueue` reservation itself is emitted by the spawn orchestrator at schedule time (ADR 0011 D9, fulfilled per ADR 0010 §4) — wiring the dead boost pipeline through the same reconciler (or deciding to drop boosted compositions). The architecture makes "wire-or-delete" a one-site decision.
- **IBEX-028 (no excess-attack force-abort):** `WarOperation`-as-supervisor withdraws low-value objectives when the cap shrinks; the manager force-retires their squads. The trimming has a home.
- **IBEX-029 (unguarded combat intents):** orthogonal but co-located — ADR 0003 routes them through the sink; this ADR ensures the job layer is the *only* place intents are emitted (the manager emits spawn requests, never creep intents), so the sink boundary is clean.
- **IBEX-041 / IBEX-042 / IBEX-043:** IBEX-043 (orphaned `SquadAssault`/`SquadHarass`) is the DELETE rows above. IBEX-042 (one `Err` tears down a long mission) is handled by the mission shrinking to status-polling + `MissionResult::Wait` (ADR 0003). IBEX-041 maps to the renew/lifecycle waste the manager eliminates by pre-spawning instead of renewing.
- **Field Report A:** convergence on one cohesive squad model (defense + offense) + cohesion-as-retirement-invariant is the architectural half; ADR 0003's movement model is the mechanical half.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Generic Squad Manager + objective-only missions** (chosen) | One lifecycle owner; missions shrink to data; defense & offense converge; replacement/pre-spawn & force-abort live in one place; closes Field Report B root + IBEX-001/026/028 structurally | New system + seam; gated on ADR 0001 (`SquadId`) and ADR 0003 (cohesion movement); a labelled migration |
| **Fix each mission in place** (wire SquadDefense, add a watchdog to AttackMission, delete orphans) | Smallest diff; ADR 0003 §B.1 already does the SquadDefense quick-win | Re-spreads lifecycle logic across missions; two FSMs to keep in sync; no home for pre-spawn/trim/economy-abort; the *root* (no owner) persists |
| **Per-mission squad lifecycle (status quo, deduped)** | Familiar | Same as above; the 2040-line AttackMission stays the reference everyone copies |
| **Overmind-style flag/Directive layer above operations** | Powerful, reactive directive placement | Heavy new abstraction; Ibex already has operations as the campaign layer; flag-cap risk (digest §1); out of proportion to the problem |
| **Per-creep roles with a global assigner (no squads)** | Simplest mental model | Abandons formation/cohesion entirely — the exact failure (Field Report A) we must fix |

*Resolution:* extract the manager. The "fix in place" option is strictly a *subset* of this one (it is ADR 0003 §B.1 step 1, which we keep as the Increment-4 quick-win), but it leaves the root — no lifecycle owner — unaddressed, so it cannot be the end state. The Overmind Directive layer is more machinery than Ibex's operation layer needs; we adopt Overmind's *ideas* (mission-as-spawn-owner via a declarative wishlist reconciler, pre-spawn over renew, synchronized squad spawning, retire-and-rebuild on TTL mismatch) without its flag framework.

## Consequences

**Positive**
- **One squad model, one lifecycle owner.** Four combat mission types (1 correct, 1 broken, 2 dead) collapse to one objective shape + one manager. `attack_mission.rs` (2040 L) and `squad_defense.rs` shrink to objective declarations; `squad_assault.rs` (592 L) and `squad_harass.rs` (357 L) are deleted (~950 L of dead code gone).
- **Field Report B closed at the root.** Lifecycle has an owner that pre-spawns, enforces cohesion-as-invariant, applies per-state deadlines, and force-retires orphaned/impossible squads — the campaign always reaches a definite terminator (engage, succeed, or tear down).
- **Finite-lifetime correctness.** Pre-spawn-and-retire replaces renew-forever (which the mechanics digest shows is a near-break-even energy drain for big bodies), so squads stay at strength through long sieges without the renew deadlock.
- **Defense and offense converge** (Field Report A architectural half): defense squads form up because they ride the *same* cohesive model as offense.
- **Homes for the orphaned fixes.** Economy-abort (IBEX-026), excess-attack trim (IBEX-028), and the inert BoostQueue (IBEX-027) each get exactly one place to live, instead of being absent or smeared.
- **Testable seams.** `register_objective`/`objective_status` and the `SquadId` store key are host-target constructible (unlike recycled `Entity` slots), enabling kernel tests: declare an objective, kill a member, assert a successor is pre-spawned; mark objective impossible, assert the squad is retired within the deadline (review §9).

**Negative / new risks**
- **New system + a new seam** during a fragile area. Mitigated by strangler-fig: the manager is introduced *behind* the existing `SquadContext`/`SquadCombatJob`, first serving the already-wired `AttackMission`, before `SquadDefenseMission` is migrated onto it.
- **Hard gating.** This is unsafe before ADR 0001 (a lifecycle owner keyed on a recyclable index reintroduces IBEX-002b aliasing across the whole roster) and incomplete without ADR 0003 (the cohesion *movement* the invariant enforces). It therefore slots *after* Increment 4's cohesion work, not before.
- **Pre-spawn mis-estimation** (travel estimate wrong → coverage gap or double-spawn). Mitigated by reusing the route cache for travel estimates and bounding with the existing wave model; the cohesion/deadline force-abort backstops a runaway.

**CPU & tick-safety**
- The manager's reconciliation is O(objectives × squads × slots) — tens, not thousands — and runs once per tick; it emits spawn requests, never creep intents, so it adds no intent cost itself. It must read the `CpuGovernor` (ADR 0004) and shed *re-planning* (not the cheap status poll) under Conserve/Critical; squad movement still draws from the single budgeted pathfinding facade (ADR 0004), not a private path.
- **Intent reduction is the net win:** pre-spawn-over-renew removes the perpetual renew-intent drain of hung campaigns (Field Report B), and one spawn producer removes duplicate-spawn races.
- No new panic surface: the manager resolves squads via the `SquadStore` (handled `None`, never a `ConvertSaveload` panic), under the ADR 0005 tick-level containment boundary.

## Incremental Migration Path

Slots **at/after Increment 4** (squad cohesion + lifecycle), depending on **ADR 0001's `SquadStore`/`SquadId`** (Increment 3) and **ADR 0003's cohesion movement + SquadDefense wiring** (Increment 4). It must NOT precede them. Hide each step behind the existing `SquadContext`/`SquadCombatJob` and the `Mission`/`Operation` trait seams; validate with the eval harness (ADR 0006) before each next step.

1. **Step 0 — DELETE the orphans (Increment 4, with ADR 0003 §B.4). Breaking: Memory/format.** Remove `SquadAssaultMission`/`SquadHarassMission` and their `MissionData::SquadAssault`/`SquadHarass` variants (`data.rs:33–34`). Because `MissionData` is positional bincode (ADR 0002), removing enum variants shifts discriminants — confine to the same labelled low-stakes reset as the other Increment-4/5 shape changes. Validate: build green; no live call sites (already verified); round-trip of a snapshot without those variants.
2. **Step 1 — Introduce `SquadManager` behind the live offense path (Increment 4+, gate: ADR 0001 `SquadStore` stable, ADR 0003 cohesion landed). Breaking: Behavioral.** Move `AttackMission`'s spawn-request generation, renew→pre-spawn, and squad creation/teardown into the manager; `AttackMission` becomes an objective declarer + success poll. Keep the wave model inside the manager (generalize `handle_wave_wipe`). Validate by **replay intent-diff parity** (ADR 0006): the same engagement produces an equivalent spawn/intent stream; assert the renew-forever loop is gone and a member death triggers a pre-spawned successor.
3. **Step 2 — Migrate `SquadDefenseMission` onto the manager. Breaking: Behavioral.** Re-express defense as a `defend room Z` objective; delete the duplicate spawn/rally/escalate FSM and the `squad_entity=None` path. (ADR 0003 §B.1's "wire SquadDefense onto SquadContext" is the precursor that lands *first*; this step generalizes it onto the manager so defense and offense share one model.) Validate: defense squads form up (cohesion metric above threshold) and a defense squad is *retired* within the deadline after the threat clears (closes the `squad_defense.rs:313` lingering-squad hang).
4. **Step 3 — `WarOperation` becomes a supervisor; wire economy-abort + excess-trim (Increment 5). Breaking: Behavioral.** Age-abort children, withdraw low-value objectives when `max_concurrent_attacks` shrinks (IBEX-028), and feed real per-squad spend so `should_abort`'s economy branch is live (IBEX-026). Validate: drop room_count → active attacks trimmed; launch at an unreachable room → torn down within the deadline (no perpetual `Running`).
5. **Step 4 (optional) — Boost pipeline through the manager (IBEX-027). Breaking: None or drop boosted compositions.** Declare `boosts` from `composition.required_boosts()` on the manager's demands (the spawn orchestrator emits the `BoostQueue` reservation at schedule time — ADR 0011 D9 / ADR 0010 §4) and gate deploy on `is_ready`; or delete the boosted compositions. One-site wire-or-delete decision.

**Breaking-change summary:** Step 0 = Memory/format (variant removal, one labelled low-stakes reset, co-staged with Increment 4/5 shape changes); Steps 1–4 = Behavioral (no serialized-shape change beyond Step 0 — `SquadContext` already serializes its roster/formation, and the `SquadId` field is introduced by ADR 0001 A2's cutover, not here). Never break the running bot mid-increment: the manager serves the already-correct offense path first, then defense, then supervision.

### Pulls from the expansion lifecycle (ADR 0017)

ADR 0017 (threat-aware expansion) shipped the safe-claim / abort half but
**deferred two squad-dependent pieces to this overhaul**:

1. **Expansion escort / pre-clear — a new SquadManager objective.** When a claim
   target is *marginal* (a transient/weak threat, economically worth taking),
   the claim pipeline should be able to declare an `escort/secure room Z` squad
   objective — pre-clear the room (the salvage `DismantleJob` for a remnant
   spawn/tower, a `SquadDefenseMission`-class squad for a weak combat creep) and
   hold it clear while the `[Claim,Move]` claimer commits. `DefenseEscalation::from_threat`
   is already `pub` for sizing it. Until this lands, ADR 0017 conservatively
   treats marginal rooms as unsafe (reject, never escort), so expansion is
   correct-but-timid against contested targets.
2. **The defense-staleness retirement (Step 2's "retired within the deadline")
   already has an interim fix.** ADR 0017 §13 made `SquadDefenseMission`
   self-terminate the moment its room stops being `owner().mine()` (de-claimed /
   lost / abandoned). Step 2 must preserve this *ownership-subordinate* invariant
   when defense moves onto the manager: a defend-objective for a room we no
   longer own is retired immediately, not just when "threat clears + members
   dead". This is also the teardown cascade for ADR 0017's `unclaim()` abort —
   keep it.
