# ADR 0008 — Combat & Squad Architecture (scout-style objective queue + a generic Squad Manager + a real tactics model)

- **Status:** Accepted (Partially-Implemented as of 2026-06-18 — live status in [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md), the master status/plan doc)
- **Date:** 2026-06-09 · **Revised:** 2026-06-16 (overhaul pass: corrected stale inventory; adopted the scout pull model; added the tactics/orders model and lead-follower movement; pinned harness-first sequencing) · 2026-06-18 (status reconciled; design is now substantially implemented on master — implementation tracked in the master plan doc, NOT here)
- **Deciders:** William Archbell
- **Related:** Field Report A (war/squad cohesion), Field Report B (operation/mission lifecycle hangs); IBEX-001, IBEX-002, IBEX-002b, IBEX-015, IBEX-026, IBEX-027, IBEX-028, IBEX-029, IBEX-041, IBEX-042. Operator brief 2026-06-16: squads "very ineffective" — *they just stand and ranged-mass-attack* (null tactics), get **orphaned** when an objective completes, sit **idle**, and **scatter** (group pathfinding). The operator's chosen reference is the **scouting** subsystem (request→claim→fulfill→release), and the chosen sequencing is **harness-first**. Sibling ADRs: **0006** (eval harness — now hosts the *combat micro-sim* this ADR is validated on; **0008 work is gated behind it**), **0001** (entity model / `SquadStore` / `SquadId`), **0003** (behavior modeling — owns the FSM + lead-follower cohesion *movement*), **0011** (spawn orchestration — `GroupId` synchronized spawning + pre-spawn), **0014** (empire posture — declares WAR/objectives), **0015** (testing — registers the combat seams), **0017** (threat-aware expansion — defers escort/pre-clear to this ADR). Reference design in-repo: the `VisibilityQueue` scout pull system (`screeps-ibex/src/room/visibilitysystem.rs`). Prior art: Overmind Overlord/Directive + `autoMelee`/`autoRanged`/`autoSkirmish` (`../references/external-references.md`).

> **Scope boundary (read first).** This ADR is the **architecture + tactics** decision for combat: *which* combat objectives exist, the seam between an objective producer, a generic squad manager, the spawn system, and the per-tick tactical orders a squad executes. ADR 0003 owns the *mechanism* of the data-driven FSM and the lead-follower *movement primitive*; ADR 0001 owns squad *identity* (`SquadId`/`SquadStore`); ADR 0011 owns *spawn scheduling* (`GroupId`/align-finish/pre-spawn); ADR 0014 owns *strategic intent* (the `WarDecl` that gates player offense); ADR 0006 owns the *harness* (the combat micro-sim + cohesion metrics that make this work measurable). This ADR consumes those and decides the combat-specific layering, the goal-management model, and the tactics.

## Context

The combat subsystem is the review's most fragile area after serialization. The earlier (2026-06-09) version of this ADR diagnosed "no layer owns squad lifecycle." The 2026-06-16 mapping pass confirms that root cause and adds two findings the original ADR missed or got wrong:

1. **The inventory was stale.** `SquadAssaultMission`, `SquadHarassMission`, and `raid.rs` **no longer exist in the tree** — the DELETE rows in the old ADR are moot. The live combat reality is exactly three modules: one `SquadCombatJob` (`jobs/squad_combat.rs`), one offense mission `AttackMission` (`missions/attack_mission.rs`), and one defense mission `SquadDefenseMission` (`missions/squad_defense.rs`). (Looting is now economic via `salvage.rs`, not a combat squad.)
2. **Tactics are effectively absent**, not merely "smeared." This is the operator's lead symptom and the original ADR under-weighted it (it framed the problem as *lifecycle*). See §What exists today.

Forces / constraints (per AGENTS.md + the engine ground-truth at `C:\code\screeps-engine`, verified — see ADR 0006 §combat-sim and `../references/engine-mechanics.md`):
- **Single-threaded WASM**, cooperative only. Squad coordination is per-tick recompute, not messaging.
- **CPU = execution + intents** (~0.2 CPU/intent). A squad of 4 large creeps renewing + moving + attacking is ~2.4+ CPU/tick of intents; a campaign that *never engages but keeps renewing* (Field Report B) is a permanent intent drain.
- **CREEP LIFETIME IS FINITE.** 1500 ticks (CLAIM 600). Renew adds `floor(600/body.length)` TTL/tick (a 40-part quad member regains only ~12–16 TTL/renew-tick), is hard-capped at 1500, and a CLAIM-part creep cannot be renewed. **Any squad design MUST plan pre-spawn replacement and retirement, not persistence.**
- **Combat resolution is two-phase and deterministic** (engine `processor.js`): damage and heal accumulate into per-target pools during the intent phase and are netted (damage then heal) at each object's own tick — so simultaneous heal can save a creep, and focus-fire must out-DPS the *aggregate* enemy heal. Per-part 100-hit pools mean a creep's DPS/heal degrades as its front parts die. These facts make tactics (focus-fire, kiting by range/fatigue, heal assignment) *the* lever and make them faithfully simulatable (ADR 0006).
- **VM-reset resilience.** Squad identity, roster, and especially *assignment* state must survive a reset; today they do not reliably (IBEX-002b). The scout subsystem solves this by keeping assignment **ephemeral**.
- **Incremental, strangler-fig.** One stable seam per step; never break the running bot mid-increment.

### What exists today (verified against code, 2026-06-16)

> **NOTE (2026-06-18):** the five defects below describe the **PRE-OVERHAUL** state and are retained as the motivating diagnosis — they are **no longer the current state**. Defects 1–4 are substantially fixed on master: `SquadDefenseMission` deleted + defense on the queue (G4-defense); orphaning addressed by `Recall` + manager retask; cohesion via the anchor mover + pure `decide_squad` (M1–M3, G3); tactics live via G3/G3-tail focus-fire / kiting / coupled-hysteresis / heal-assignment. `AttackMission` now **coexists as legacy** behind the live objective producers (G4-O6) and is scheduled for deletion in O7 — it is no longer the live combat driver. **Current status: [master plan doc](../plans/combat-overhaul-plan.md) §3.**

Call graph: `WarOperation` (singleton) → `AttackOperation` (per target) → `AttackMission` (per wave) / `SquadDefenseMission` → `SquadContext` (ECS component) + `PreRun/RunSquadUpdateSystem` → `jobs/squad_combat.rs` (per-creep FSM). Five structural defects:

1. **Two divergent combat paths.** Only `AttackMission` creates a `SquadContext`. `SquadDefenseMission` builds `SquadCombatJob::new()` with `squad_entity: None` (`squad_defense.rs`), so **every defender runs the order-less fallback path — scatter by construction** (IBEX-001 L1). Defense, the more common case, has *zero* coordination, focus-fire, or cohesion.

2. **Orphaning → idle.** Combat creeps are **not** children of the mission (`get_children()` returns only squad entities, `attack_mission.rs:1892`). On mission complete the `SquadContext` entity is deleted but the live creeps keep a now-dangling `squad_entity: Option<u32>` (`jobs/squad_combat.rs:18`); every lookup returns `None`, the FSM drops into Engaged-without-orders, and `fallback_movement` does nothing once no hostiles remain. **The creep idles in the conquered room until TTL (~1500t).** No system reclaims a living creep whose squad is dead (`creep.rs:71–90` deletes only on `hits()==0`). This is the operator's "orphaned / sit idle when the objective is complete."

3. **Cohesion is N independent solo pathfinds.** Each member issues `move_to(formation_tile).range(0)` against a "virtual anchor" that advances ≤1 tile/tick only on a 75%/15-tick quorum and ratchets permanently into Loose (`formation.rs`, `squad_combat.rs`). **The rover's purpose-built `Follow { desired_offset }` / `pull()` group-movement API (`screeps-rover/src/movementsystem.rs:531–567,1072–1156`) is fully implemented and 100% unused** — the strongest available primitive is dead code. So is `room_route`, `threat_direction`, `reassign_slots`, `apply_quad_cost_overlay`, and `check_movement_failure` (IBEX-015). Fatigue mismatch (one slow member) scatters the rest.

4. **Tactics are near-null (the lead symptom).** Focus fire is *best-effort*: the mission picks one target id, but a creep out of range of it silently re-scans and retargets its own nearest-lowest-HP, defeating concentration against aggregate enemy heal. **There is no kiting in the ordered Engaged path** — kite logic exists only in the unreachable `fallback_movement`, so an ordered ranged quad sits at melee range and ranged-mass-attacks in place (exactly "just stands and RMAs"). Retreat *oscillates* because the squad-level (`any member <25%`) and per-creep (`<50% out / >80% in`) thresholds are decoupled with no shared hysteresis. The runtime heal/retreat math uses a flat 12 HP/part and never consults the threat/damage model.

5. **Trickle spawning.** Each slot is an independent `spawn_queue.token()` at `SPAWN_PRIORITY_MEDIUM`, broadcast to every home room, fulfilled in non-deterministic `HashSet` iteration order — so members of one squad spawn in different rooms many ticks apart, and the mission walks early members toward the rally while later siblings are still queued. Members are scattered *before the fight even starts*.

Also confirmed dead/inert: `WarOperation::run_operation` returns `Running` forever with no child age-abort (IBEX-002/028); `AttackOperation::should_abort`'s economy branch never fires (`total_energy_invested` unwritten, IBEX-026); the `BoostQueue` is plumbed but no combat mission populates a request, so boosted compositions spawn unboosted (IBEX-027); combat intents fire bare `creep.attack(...)` outside the guarded sink (IBEX-029).

**Why now:** the war system "cannot reliably convert an economic lead into territory" because there is no goal owner, no tactics, and no cohesion — and, critically, **no way to measure or iterate on any of it** (the harness scores `military: None`, ADR 0006). Fixing this per-mission would re-spread the logic. The leverage is (a) a harness that makes combat measurable and fast to iterate (ADR 0006, **first**), then (b) a single goal/lifecycle owner modeled on the scout pull system, with real tactics on top.

## Decision

Adopt a **scout-style, queue-decoupled combat architecture** with three coordinated pieces, all gated behind the combat harness (ADR 0006) and `SquadStore` (ADR 0001):

1. A **`CombatObjectiveQueue`** — a global, persistent, priority/TTL request queue of *objectives* (per-room/target, not per-creep), modeled directly on the `VisibilityQueue` scout pattern. Producers (war/defense-scan/claim/attack) upsert idempotently; the manager pulls. **This is the seam** that makes work queue-owned-and-pulled instead of mission-owned-and-pushed, which is precisely why a completed or aborted producer never strands a squad.
2. A **`SquadManager`** — a single perpetual ECS system (like `ScoutOperation`) that claims objectives for `SquadId`s, reconciles desired-vs-live rosters into spawn demand, pre-spawns replacements (never renews), enforces cohesion-as-invariant + deadlines, **computes the per-tick tactical orders**, and retires/retasks squads.
3. A **real tactics model** — authoritative focus-fire, kiting, centralized heal assignment, and engage/disengage with coupled hysteresis — computed once by the manager and merely *executed* by the per-creep FSM through the one guarded intent sink.

This refines the original three-layer "objective-driven missions + Squad Manager" decision by (a) inserting the queue as the decoupling seam (matching the scout reference the operator wants, rather than a direct mission↔manager API), (b) making *assignment* ephemeral (the scout `claimed_by` discipline, killing the dangling-ref class for the goal layer), and (c) elevating *tactics* to a first-class layer.

### Layering

```
WarOperation / AttackOperation / ClaimOperation     strategy: what & why (ADR 0014 posture/WarDecl)
        │  produce / refresh (idempotent upsert)
        ▼
CombatObjectiveQueue   (NEW global resource)         request → claim → complete → release → retire
        │  persistent: durable ObjectiveData + give-up backoff
        │  ephemeral:  assignment (claimed_by), this-tick status  ── NEVER serialized
        ▼  manager claims an objective for a SquadId
SquadManager   (NEW global system)                   objectives → rosters → spawn demand → ORDERS → retire
        │  reconciles desired vs live; mints SquadId; pre-spawn; force-abort; tactics
        ▼  per-tick TickOrders + Follow targets
Squad   (SquadContext, keyed by SquadId; ADR 0001)   roster + formation + lead/anchor + tick orders
        │  per-member orders
        ▼
SquadCombatJob   (jobs/squad_combat.rs)              ONE creep's intents this tick (guarded sink);
        │                                            on dead squad → Recall escape valve (never idle)
        ▼
SpawnOrchestrator → spawnsystem.rs executor          synchronized align-finish group spawning (ADR 0011)
```

The key inversion vs. today: **work is queue-owned and pulled, not mission-owned and pushed.** A producer that completes or dies leaves its objective in (or lets it TTL-expire from) the queue; the manager — a perpetual reconciler — observes the change and retasks or retires squads. A squad whose objective vanishes is never stranded because the manager owns it; a creep whose squad vanishes self-detaches into a Recall behavior.

### 1. Mission / module inventory — corrected (KEEP / MERGE; no DELETE)

> "Squad?" = participates in the combat-squad layer (`SquadContext` + `SquadCombatJob`).

| Module | Instantiated? | Squad layer today | Verdict |
|---|---|---|---|
| **`SquadCombatJob`** (`jobs/squad_combat.rs`) | ✅ the only per-creep combat job | the executor | **KEEP** — shrinks to pure order execution + a `Recall` escape valve; loses all targeting-of-last-resort once it always has orders. |
| **AttackMission** (`attack_mission.rs`) | ✅ `attack.rs` (per wave) | ✅ `SquadContext` + virtual anchor | **MERGE — IN PROGRESS (P2.G4-offense, sequenced).** Becomes an objective *producer* (`Secure`/`Dismantle`/`Harass`/`Farm{PowerBank}`); lifecycle + tactics move to the `SquadManager`. Manager siege capabilities built (O1 anchor-mover / O2 orient / O3 breach / O4 wave-retry). **Single-squad offense producers live (O6, 2026-06-18): `InvaderCore`→`Dismantle`, `AttackFlag`→`Secure`, `ResourceDenial`→`Harass`; `InvaderCreeps` reconciled into remote-defense `Defend`. Coexists with `AttackOperation` for `PowerBank` + the deferred heavy assault.** Remaining: O5 (power-bank) → O7 delete. See [`../execution/g4-offense-plan.md`](../execution/g4-offense-plan.md). NOT yet removed. |
| **SquadDefenseMission** (`squad_defense.rs`) | ~~`war.rs` defense scan~~ | ~~squad-LESS~~ | **DONE — REMOVED (P2.G4-defense, 2026-06-18).** All three war.rs defense contexts (owned reactive / defend-flag / remote-invader) now upsert `Defend` objectives fielded by the `SquadManager` with G3 tactics; the file + `MissionData::SquadDefense` + the `manager_defense` flag are deleted (WFV 11→12). Producer-scoping preserves the §13 ownership invariant; fixed the W7N1 reserved-remote thrash as a bonus. |
| **DefendMission** (`defend.rs`) | ✅ economic missions | ❌ not a squad mission — a room-safety *signal* (`is_room_safe()`), spawns nothing | **KEEP** (rename-candidate `RoomSafetyMission`). Out of scope — produces no creeps. |
| **NukeDefense / SafeMode / WallRepair** | ✅ `war.rs` | ❌ utility, no squads | **KEEP** as-is. |

`SquadAssaultMission`/`SquadHarassMission`/`raid.rs` referenced by the 2026-06-09 ADR **do not exist** — no DELETE needed. **Net:** the combat-squad layer is *already* one job + one offense + one defense; the work is to converge offense & defense onto the queue+manager+tactics, not to prune dead missions.

### 2. The global combat-goal layer — `CombatObjectiveQueue`

Modeled directly on `VisibilityQueue` (two-layer split `visibilitysystem.rs:121–175`; upsert `:191–217`; claim/release `:220–231,284–292`; selection `:301–324`; TTL expire `:269–274`; unreachable backoff `:96–114,242–266`). The six anti-orphan properties of scouting are adopted wholesale:

| Scout property (ref) | Combat adoption |
|---|---|
| Global priority/TTL queue, idempotent upsert (priority max-merge, flags OR, TTL extend) | `CombatObjectiveQueue::request(CombatObjective)` — many producers, no duplicates. |
| Two-layer state: persistent durable facts + **ephemeral** assignment (`claimed_by`) | Persistent: the objective + give-up backoff. Ephemeral (never serialized): which `SquadId` claims it — self-heals on reset, **cannot dangle** (kills IBEX-002b for the goal layer). |
| Self-claiming worker that releases on completion and re-pulls | The **squad** (not the creep) self-claims via the manager; releases on `SuccessPredicate`; the manager re-pulls the next-best next tick. |
| Mission = pure spawner, completes on **observed world-state**, decoupled from creep | `SuccessPredicate` is an observable predicate, not a creep flag — decoupling objective lifetime from creep lifetime. |
| Idle escape valve (bounded idle → proactive useful work; idle creeps marked shovable) | A released-but-healthy squad **retasks**, never idles; an orphaned creep enters **Recall** (return home + volunteer-defend), marked `mark_idle` shovable in transit. |
| Graceful give-up: persistent exponential backoff, cleared on success | `UnwinnableTarget` backoff (base ~2000t, cap ~20000t) — stops throwing squads at a safe-moded/over-towered room forever; cleared when the target becomes winnable. |

```rust
// Persistent (serialized component, like VisibilityQueueData) — durable FACTS only
struct CombatObjectiveData { objectives: Vec<CombatObjective>, unwinnable: Vec<UnwinnableTarget> }

struct CombatObjective {
    id: ObjectiveId,            // minted monotonic id (ADR 0001), never an Entity index
    kind: ObjectiveKind,        // Secure{room} | Defend{room} | Dismantle{target} | Harass{room}
                                //  | Farm{powerbank|sk|core} | Escort{room}  (Downgrade/Claim later)
    priority: f32,              // max-merged on re-request
    force: ForceRequirement,    // = existing Vec<PlannedSquad> (composition + deploy_condition)
    success: SuccessPredicate,  // OBSERVABLE world-state (see below)
    deadline: Option<u32>,      // per-kind wall-clock (Forming ~150t, Engaged ~400t, Defend-clear ~50t)
    expires_at: u32,            // TTL; kept alive by re-request, dies if the producer stops
    owner: OwnerHint,           // producer id for status reporting (NOT ownership of the squad)
}
```

`SuccessPredicate` examples (observable, decoupled from creep state):
- `Secure{room}` → `room_has_hostile_threats(room) == false` AND room visible.
- `Defend{room}` → `!militarily_active(room)` (the post-`4fae295` predicate) AND `owner().mine()`. **If the room stops being ours, the objective is withdrawn immediately** (preserves the ADR 0017 §13 ownership-subordinate invariant).
- `Dismantle{target}` → target gone. `Farm` → resource depleted / core dead.

**Producers** (mirror the scout producers, each on its own cadence, all idempotent upsert):
- `WarOperation::run_offense_evaluation` → `Secure`/`Dismantle`/`Harass`/`Farm` (player offense gated by ADR 0014 `WarDecl`; NPC policing autonomous). **The offense-producer mapping (design):** each live `AttackReason` maps to one objective kind + a threat-/role-matched composition + a priority, mirroring `run_defense_scan`'s escalation→composition pattern:

  | `AttackReason` (legacy) | → `ObjectiveKind` | Composition | Priority | Notes |
  |---|---|---|---|---|
  | `InvaderCore { level }` | `Dismantle { room, pos }` (the core's tile) | `siege_quad` | `MEDIUM` | **DONE (P2.G4-O6).** Exercises O1 travel + O2 orient + O3 breach (a stronghold core is shielded by ramparts). Affordability gate (`invader_core_attack_score`) preserved. Standardized `siege_quad` for all levels (level-aware sizing is a future refinement). |
  | `AttackFlag` (operator) | `Secure { room }` | `quad_ranged` | `HIGH` | **DONE (P2.G4-O6).** Explicit operator intent — clear the flagged room. |
  | `ResourceDenial` | `Harass { room }` | `solo_harasser` | `LOW` | **DONE (P2.G4-O6).** Deny a hostile player's remote income; opportunistic, never starves core-clearing/defense. |
  | `InvaderCreeps` | — (RECONCILED) | — | — | **DONE (P2.G4-O6): dropped.** The reserved-remote-invader `Defend` context (`run_defense_scan`) already fields a squad on the identical trigger (`reservation().mine() && visible() && hostile invaders`) — a `Secure` here would double-field the same room. Defense owns it. |
  | `ProactiveDefense` / `ThreatResponse` / `Expansion` | (`Secure`/`Defend` when produced) | — | — | Enum variants the offense scan does not currently produce → nothing to migrate. |
  | `PowerBank { power }` | `Farm { PowerBank, room }` | `power_bank_duo` + bespoke dropped-power haul | `LOW` | O5 (deferred to the O6/O7 window — the highway dropped-power collector lives only in `AttackMission::Exploiting`). |
  | **Heavy multi-squad player assault** (`plan_by_detected_threat`, towers≥4 → drain-duo + quad with `DeployCondition` sequencing) | — | — | — | **DEFERRED** — does NOT fit one-squad-per-objective; stays on `AttackMission` until a multi-squad/sequenced-objective mechanism exists. O7's delete is gated on this too. |

  **Coexistence + cap (current state, O6 done):** all migrated reasons flow through one `source → (ObjectiveKind, priority, composition)` branch in the offense launch loop (so the dedup — highest-scored candidate per room — and one combined cap apply uniformly); the un-migrated reasons (`PowerBank`, the heavy assault) keep launching `AttackOperation`s, so the two paths run side by side until O7. A migrated room carries no `active_attacks` entry, so the per-scan re-evaluation re-asserts its objective idempotently (refreshing the TTL). **War-side cap (reconciled):** the launch-loop budget is `active_attacks.len() + (Attack-owned objectives in the queue)`, capped at `max_concurrent_attacks` — an existing objective is always re-asserted (only new offense is gated, score-sorted so the skipped one is lowest-value); this replaces the old `active_attacks`-only break. The manager's `MAX_CONCURRENT_SQUADS` is the downstream fielding limit.
- `WarOperation::run_defense_scan` → `Defend` (replaces `SquadDefenseMission` creation; **keep** the 2026-06-16 `hostile_warrants_defender` body-parts trigger and the owned-room-invader rule).
- `ClaimOperation` (marginal target) → `Escort{room}` (the ADR 0008/0017 deferred pre-clear escort; sizing via `DefenseEscalation::from_threat`).
- `AttackOperation` → `Dismantle` for blocking structures.

A producer that stops caring simply stops re-asserting; the TTL lapses, the objective dies, the squad is retasked/retired — exactly how a satisfied scout request disappears.

### 3. The squad lifecycle owner — `SquadManager`

A single ECS resource + system (a `Resource` threaded through execution-data, never a static), perpetual like `ScoutOperation`. Once per tick (sheddable re-planning under CPU Conserve/Critical per ADR 0004; the cheap status poll never sheds):

1. **Claim / retask.** Select `best_unclaimed` (priority then proximity to candidate home rooms; skip `unwinnable`-backoff and already-claimed), then `claim(objective_id, squad_id)`. Claims live **only in the ephemeral runtime map**; `release_dead`-equivalent frees claims whose squad no longer exists each tick.
2. **Composition → roster.** Mint a `SquadId` (ADR 0001), create `SquadContext` in the `SquadStore`. **Size members via the threat-matched `military/damage.rs` helpers + `bodies.rs sized_*_body`** (the 2026-06-16 learnings) — not fixed `BodyType` templates at `energy_capacity`. Preserve minimum-affordability, focus-heal (aggregate `enemy_focus_heal`), and energy-readiness (`SpawnNow`-vs-`Wait`).
3. **Spawn-demand generation (the only combat spawn producer).** Emit desired-minus-current as `SpawnDemand`s (ADR 0011 D1) tagged `GroupId = SquadId` for synchronized align-finish (§6). Replaces both missions' hand-rolled token broadcasts.
4. **Pre-spawn replacement, never renew** (ADR 0011 D4/D8). For any member with `ticksToLive < spawn_time + travel + PRESPAWN_MARGIN`, emit a successor demand for that slot. **Delete all combat `request_renew` call sites.** If a fresh-TTL successor would badly mismatch low-TTL survivors, prefer retire-and-rebuild the whole squad (generalize `handle_wave_wipe` into the manager).
5. **Compute per-tick tactical orders** (§4) — the brain that was smeared across `AttackMission::Engaging::tick` moves here so defense and offense share one model. **Built (P2.G4):** the manager drives the squad **anchor** for cohesive formation travel (O1, `advance_squad_virtual_position`), **formation orientation** toward the threat for box-fighting squads (O2, pure `decide_squad.orientation` → `reassign_slots`), and **breach-aware dismantle targeting** (O3, `breach_redirect` over the rover `room_grid_dijkstra` — break the rampart shielding the target first). Combat style (box-siege vs kite) is derived from the objective kind (`Dismantle` → Formation, else Skirmish).
6. **Enforce invariants, wave-retry & retire** (§5). A wiped squad (had members, all now dead) is retired; on a non-`Defend` target a wipe also calls `objective_queue.mark_unwinnable(room)` (exponential backoff) so the manager stops feeding squads into an unwinnable siege and the producer's re-assert is ignored until the backoff lapses — **defense stays persistent** (never abandon an owned room). This generalizes `AttackMission::handle_wave_wipe` into the manager (P2.G4-O4).

### 4. Tactics / orders model — the "just stands and RMAs" fix

> **The concrete tactics catalog is [ADR 0008a](0008a-combat-tactics.md)** — ~55 tactics (T-FOCUS/POS/TOWER/BREACH/HEAL/ENGAGE/COMP/CTRL/DEF/NPC) each as `trigger → behavior → tunable params → sim-measurable metric → robustness`, plus the per-composition playbooks, the tunable-parameter table, and the ordered experiment register (EXP-*) we iterate through on the sim (ADR 0006) to *find* effective tactics. This section states the *shape* of the orders model; 0008a is the behavior.

The manager computes, per squad, one authoritative order set into `SquadContext.members[*].tick_orders`. Jobs **execute, never re-decide** the *combat target/heal/kite-vs-engage decision*. **(⚑ But the actual *movement request* to the pathfinder belongs in the squad/job, not the mission/manager — see the deferred flag in §5; the manager supplies the goal/anchor/focus as context, the job issues the move.)** All combat intents route through the **one guarded sink** (ADR 0003 §A.2 / IBEX-029) — today combat fires bare `creep.attack(...)`; this closes it.

**§4.1 — The tactics are PURE and live in `screeps-combat-decision`, so they are simulated, not just live (operator 2026-06-18; ADR 0006 §B.2).** The decision functions are pure over JS-free DTOs and run **identically** in the live bot and the in-process micro-sim — no tactics fork, self-play-validated, and iterated via the [ADR 0008a](0008a-combat-tactics.md) EXP-* register on the sim. The layering is two pure tiers + thin game-coupled adapters, with **clear responsibilities** (treat the current squad-vs-job split as changeable toward this):

| Layer | Where | Responsibility | Purity |
|---|---|---|---|
| **Per-squad decision** `decide_squad(SquadView) → SquadDecision` | `screeps-combat-decision` (P2.G3) | shared focus (`select_focus_target`), engage/retreat **coupled hysteresis**; *next:* heal **assignment** + slot/orientation | **PURE** |
| **Per-creep decision** `decide_combat` / `decide_movement` | `screeps-combat-decision` (H2/M2) | one creep's attack/heal intents + its movement *goal* (kite/engage/flee) given the squad orders | **PURE** |
| **SquadManager** | `military/squad_manager.rs` (ECS) | LIFECYCLE: claim/spawn/retire; the **live adapter** — builds `SquadView` from `SquadContext`+room DTOs, calls `decide_squad`, writes the result to `tick_orders`/state. **No tactics math.** | game-coupled |
| **SquadCombatJob** | `jobs/squad_combat.rs` (ECS) | EXECUTION: read orders, call the per-creep pure decisions, **issue the intents + the rover movement request** (owns movement issuance — the §5 ⚑ fix) | game-coupled |
| **SquadContext** | `military/squad.rs` (component) | DATA carrier: roster, cached member status, layout, the per-tick orders/state. Its `compute_heal_assignments`/`should_retreat`/`reassign_slots` **migrate into `decide_squad`** (they are already pure over member data). | data |

So the SK duo / defense squads now run `decide_squad` (the SAME code a future sim scenario will exercise), and the manager carries zero tactics logic. **Status (P2.G3 + G3-tail, complete 2026-06-18):** the full pure surface is `decide_squad`/`decide_squad_with_pathing` (shared focus + engage/retreat coupled hysteresis + the heal assignment + the cohesive **pathfinding-scored kite goal**) feeding the per-creep `decide_combat`/`decide_movement`; the kite/flee search is rover's `LocalPathfinder::search_scored` (combat supplies only the pricing); wired live (no WFV bump) + self-play-validated (EXP-SQUAD-KITE-1). Cohesion is a *term in the tile score* (the block moves to one shared goal), the only individual break being a critical-HP flee.

**Two recorded items (operator 2026-06-18; details in [`../execution/g3-tail-plan.md`](../execution/g3-tail-plan.md) "Known limitations & future evaluations"):** **(L1)** the scored kite/flee search is **single-room** (`LocalPathfinder`), so a squad **will not flee to an adjacent room** — fine for an intra-room fight (cross-room travel is the separate phase) but a real edge for a squad cornered at a room boundary; the likely fix is a server-`PathFinder` multi-room-flee fallback when the local search is cornered (live-only, not sim-validated). **(L2)** evaluate replacing the per-tick **eager DTO copy** (`creep_to_dto`/`structure_to_dto`) on the live path with the **trait-based lazy-view** pattern the pathfinder already uses (`CreepHandle`/`CostMatrixDataSource`) — a `CombatCreep`-style trait the live adapter impls over `game::*` and the sim over `CombatWorld`, decisions generic over it — to drop the live copy/alloc; gate on a measured CPU win, since value-over-DTO keeps the digest + tests simple.

- **Authoritative focus-fire with ranked fallback.** Pick one focus target per squad from the **whole-squad centroid** (fixing today's bug where the anchor's "first living member's room" returns `None` for everyone). A creep *in range of the focus target MUST hit it* (concentration is required to out-DPS the aggregate enemy heal — the engine nets damage-then-heal per target); a creep *out of range* uses a **manager-supplied ranked fallback list** (one shared scan), never its own re-scan. This both restores concentration and removes the 4× redundant per-creep room scans.
- **Kiting in the ordered path (the missing piece).** `TickMovement` gains intent-bearing variants: `Engage{target}` (close to optimal weapon range), `Kite{from}` (ranged hold range 3, flee melee-only at ≤2), `Hold`. The correct kite logic already exists in `fallback_movement` but ordered creeps never reach it — promote it to a manager-driven order so a ranged quad stops sitting at melee range. Kiting is computed from MOVE/fatigue + range math (engine-accurate, opponent-agnostic), not an enemy list.
- **Centralized heal assignment (keep, fix the math).** Keep the greedy assignment (`squad.rs:515–647`) — a good pattern — but use the **runtime damage model** (boosted HEAL = 4×, `damage.rs`) instead of the flat 12 HP/part, and **recompute `heal_power` each tick** (a creep that loses HEAL parts is reassessed). Out-of-range healers fall back to `heal_best_nearby`.
- **Engage / disengage with coupled hysteresis.** Replace the two decoupled thresholds with one squad-owned policy: retreat when avg HP < `retreat_threshold` OR any member < a hard floor; re-engage only above a separated higher band. `retreat_threshold` is **enemy-DPS-aware** (sized from the threat model), not a flat 0.3. Per-creep states are *subordinate* to squad state (an individually-critical creep requests retreat but does not unilaterally flip the squad). This removes the yo-yo.

ADR 0003's "utility AI for SELECTION only" is honored: selection (target/heal/kite-or-engage) is scored once by the manager; the FSM is pure control flow.

### 5. Robust group movement, cohesion & lifecycle invariants

**The movement model is ANCHOR-PRIMARY with a column-collapse fallback** (corrected 2026-06-16 from an earlier "delete the anchor, use lead-follower" draft — see ADR 0003 §B.2 CORRECTION; lead-follower has a *fixed* offset with no facing, so it structurally cannot do orientation/rotation/present-armor/turn-from-tower, which are the tactically decisive maneuvers). This is the single biggest lever for "squads scatter." The current virtual-anchor is broken not because anchors are bad but because it advances in a **straight line** (no pathfind) while members path independently and the mode ratchets into Loose — all fixable. The bot already contains ~90% of the proper anchor machinery, dormant.

> **⚑ KNOWN ARCHITECTURE TENSION — the mission/squad/job movement split (DEFERRED; flagged per operator 2026-06-18, "fix later").** The split this ADR currently draws — the `SquadManager`/mission computes per-tick movement orders (`TickMovement` `Engage`/`Kite`/`Hold` in §4, the anchor advance) and the job *"executes, never re-decides"*, with mission-side flee (`issue_virtual_anchor_flee`) and mission-side anchor stepping (`advance_virtual_pos`, `formation.rs`) — **inverts the bot's normal pattern**, where the creep's **job** state machine issues its own movement requests to the pathfinder (`jobs/utility/movebehavior.rs`; the economic jobs already do this). **The intended split:** missions/manager set **goals, objectives, and context** (the objective, the shared anchor `virtual_pos` + orientation, the focus target, the per-source suppression signal); the **squad or job performs the actual movement request to the pathfinder**. The shared *context* (anchor frame, cohesion gate, focus) is legitimately squad/manager-level — only the *movement-request issuance* should move down. Consistent with [ADR 0018](0018-source-keeper-room-exploitation.md) principle 8 and the phase-2 §2.1 convention; lands with the Migration-Path **step 2** anchor-mover + the `SquadManager` work (M/G workstreams). Flagged here so the rewrite pushes movement-request issuance down to the squad/job rather than cementing the inversion.

- **Anchor as the squad's coordinate frame.** A `virtual_pos` + an **orientation**; each member's target = `anchor + rotate(base_offset, orientation)`. The anchor **follows a cached tile-path** — pathfound **once** through the pathfinding system, cached on `SquadPath` (the never-populated `room_route` is the vestigial intent), then followed step-by-step and re-pathed only on invalidation/stuck (the rover `CreepPathData.path` discipline). **Not** today's `advance_virtual_pos`, which advances by a straight-line `signum` step with no pathfind (`formation.rs:380-414`) — the straight line is the bug, not the anchor. The cached path is built with a **footprint-aware cost transform** (the Overmind "moving-maximum" `applyMovingMaximum(w,h)` recipe — generalize the existing `apply_quad_cost_overlay`/`apply_formation_cost_overlay`, parameterized by W×H so duo/quad/larger share one path) plus `apply_tower_avoidance_costs` pricing — so the block never routes where it can't fit and naturally drifts out of tower-optimal range. Members then **move in lockstep** (one direction from the anchor's next cached step) — one (cached) pathfind + N cheap direction-moves, cheaper than today's N per-member `move_to`. Honors *pathfinding lives in the pathfinding system; modules supply pricing only*.
- **Orientation & rotation (the anchor's payoff).** Wire the dormant `FormationLayout::{orient_toward, mirror_y, rotate_cw}` + `threat_direction` + `reassign_slots`/`threat_facing_slots`: face the block toward the threat (tanks/high-HP front, healers back), `mirror_y()` on retreat to keep the armored edge toward the enemy while kiting as a block, rotate-in-place (engine-confirmed 4-cycle, `movement.js:22`) to swap a damaged front creep for a fresh one. **This is exactly the "rotate away from damage range while keeping cohesion" capability — and lead-follower cannot do it.**
- **Corridor/edge = RELAX the same mover, don't switch primitives.** The scatter the anchor prevents only happens in *open* terrain; in a 1-wide corridor there is one path, so independent member moves *converge* (terrain enforces single-file). So when the footprint won't fit, keep the SAME anchor mover and relax two parameters: a **width-1** footprint pathfind + a travel-oriented `line`/loose tolerance. Self-mobile members file through; the hard gate **re-forms** the box on the open side automatically. **No separate follower mode and no `pull` are needed for self-mobile squads** — it's one mover on a continuum of tightness (exact offsets → line/loose → loose-centroid), switching on "does a footprint path exist".
- **`Follow`/`pull` reserved for its real niche.** `Follow`'s unique value is the `pull` integration for creeps that can't move themselves — **no-MOVE / under-MOVE'd compositions** (a pulled high-part attacker, a dedicated puller). Optional rover capability for such bodies, not the corridor mechanism. (Fix the rover line-510 fatigue short-circuit only if those compositions are fielded.)
- **Arbitrary N.** Rigid offsets for **N ≤ 4** (duo = 1×2 anchor — a *pull-pair* only if one member is under-MOVE'd; quad = 2×2; triangle = 3); **5+ "blobs"** use **loose-centroid cohesion** ("stay within N tiles of the squad centroid" / path-from-center) or split into multiple anchored sub-squads under one objective.
- **Cohesion as a HARD invariant.** Advance only when **every live member is on its oriented offset tile**, replacing the soft 75%/15-tick quorum that ratchets into Loose; gate the whole squad on aggregate fatigue. Fatigue cohesion for self-mobile squads = **MOVE-balanced bodies** (not `pull`).
- **Wait-for-stragglers + force-abort backstop.** A squad moves at the pace of its slowest live member; the N-tick non-cohesive **force-abort** converts a permanent block into a clean retirement (no hang).
- **Stuck recovery.** Wire the dead `check_movement_failure` (IBEX-015) into the job's move states: a rover-abandoned member drops target and regroups rather than re-issuing the same blocked move. Use lower military `StuckThresholds` so combat reacts faster than haulers.
- **Reset-survivable state.** Keep the *strategic* path/lead-slot on `SquadContext` (already serialized); mark regenerable per-creep `CreepPathData.path` `#[serde(skip)]` (IBEX-049). Assignment/claim state stays ephemeral.

**Lifecycle invariants & deadlines (closing Field Report B):** the manager force-retires a squad when ANY of: objective `Succeeded`/`Impossible`; objective withdrawn (producer stopped / TTL lapsed); **cohesion-as-invariant** non-cohesive > N ticks; orphaned (objective gone); per-state **wall-clock deadline** exceeded. With squad renew abolished outright, nothing sustains a non-cohesive squad past its deadline and the all-dead terminator can finally fire. `WarOperation` stays a perpetual supervisor but gains teeth: it withdraws low-value objectives when `max_concurrent_attacks` shrinks (IBEX-028) and feeds real per-squad spend so the economy-abort fires (IBEX-026).

**Orphan/idle GC at two levels:** *(squad)* the manager owns all squads — a vanished objective auto-releases its claim → retask or retire within one reconcile tick; there is no "stranded squad" state. *(creep, defense-in-depth)* `SquadCombatJob` gains a terminal **`Recall`** state: when `get_squad_state() == None` for K ticks the job self-detaches, marks itself shovable, and pulls a safe default (return to nearest owned room, register as a `Defend` volunteer, else recycle). This is the scout idle-escape-valve made safe (recall, not wander-into-danger).

### 6. Synchronized spawning

Adopt ADR 0011's `SpawnOrchestrator` + `GroupId`/align-finish (D3) + pre-spawn (D4); the `SquadManager` is the sole combat **demand** producer.

- **Group admission:** schedule a squad's slots only when the chosen room set has enough lanes to finish all members within an emergence window W (~25t) AND projected energy covers the group. Until admitted, the group consumes **zero** lanes — no half-spawned quads idling at rally.
- **Align finish, not start:** delay shorter members so all emerge within W at full 1500 TTL (promote the `composition.rs` simulation from estimator to scheduler).
- **Same-squad, same-room (or bounded split):** prefer one home room per squad to eliminate cross-room scatter; allow a split only if travel-adjusted rally arrivals stay within W. This makes `HashSet`-iteration nondeterminism irrelevant to cohesion.
- **Hold movement until roster-complete:** the manager does NOT advance toward the rally until the group is admitted/emerging together.
- **Priority:** offense escalates MEDIUM→HIGH **on commit** (after group admission) so a committed squad is not starved below economy; defense stays HIGH. Keep the descending-priority reservation comparator verbatim (pinned anti-re-flag invariant).
- **Boosts:** wire `composition.required_boosts()` → boost reservation at schedule time (ADR 0011 D9 / 0010), gate deploy on `is_ready`, behind the existing kill-switch (IBEX-027 wire-or-delete → **wire**, see Open Questions).

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Scout-style `CombatObjectiveQueue` + `SquadManager` + tactics model** (chosen) | Matches the operator's reference exactly; work is pulled, so a dead producer never strands a squad; assignment ephemeral (no dangling refs); one cohesive model for offense+defense; tactics finally exist; everything measurable via ADR 0006 | New queue + manager; gated on ADR 0001 (`SquadId`) + ADR 0003 (movement) + ADR 0006 (harness); a labelled migration |
| Direct `register_objective`/`objective_status` mission↔manager API (the 2026-06-09 framing) | Slightly less machinery | Keeps the *mission* as work-owner → re-creates the push coupling that orphans creeps today; diverges from the scout reference |
| Fix each mission in place (wire defense, add a watchdog, fix focus target) | Smallest diff; ADR 0003 §B.1 is already step 1 | Re-spreads lifecycle + tactics across missions; no home for pre-spawn/trim/economy-abort/kiting; the root (no goal owner) persists |
| **Movement: one anchor mover on a continuum of tightness** (chosen — §5) | Orientation/rotation/present-armor/turn-from-tower; one footprint-aware ("moving-maximum") pathfind; corridors handled by *relaxing* the same mover (width-1 pathfind + line/loose) — no second primitive; ~90% already built (dormant) | Most machinery; the rigid↔relaxed switch is unpublished work to build |
| Movement: pure lead-follower (rover `Follow` only — the rejected 2026-06-09 draft) | Simplest; naturally single-files through corridors | **No orientation/facing** → cannot present-armor / turn-from-tower / rotate; leader paths as a single creep (walks the block into a 1-wide trap). Redundant: corridors are handled by relaxing the anchor mover; `Follow`/`pull` is reserved for no-MOVE/under-MOVE compositions only |
| Movement: a separate "follower/column mode" with a handoff between primitives | Conceptually tidy two-mode model | Unnecessary extra subsystem — a 1-wide corridor *enforces* single-file, so relaxing the anchor (width-1 pathfind + line/loose) achieves the same with one mover and no handoff glue |
| Movement: pure rigid anchor, no corridor relaxation (Overmind block only) | Perfect cohesion + orientation | Stalls at 1-wide chokes/room-edges. The chosen design relaxes to line/loose to fix exactly this |
| Movement: keep the current straight-line virtual-anchor | No change | The actual bug source — anchor doesn't pathfind, members path independently, mode ratchets into Loose |
| Per-creep self-pull (max cohesion via shared per-target claims, pure scout model) | Simplest mental model | Abandons formation — the exact failure (Field Report A) we must fix. Squads, not creeps, claim objectives |

## Consequences

**Positive**
- **The operator's three symptoms close structurally.** Orphan/idle → queue ownership + `Recall`; scatter → lead-follower + hard cohesion + synchronized spawn; null tactics → the manager-computed focus-fire/kite/heal/hysteresis orders.
- **Defense and offense converge** on one cohesive, focus-firing, centrally-healed model — the single largest defensive-effectiveness lever.
- **Field Report B closed at the root:** a definite terminator (engage/succeed/retire) via deadlines + cohesion-invariant + give-up backoff + economy-abort, with pre-spawn replacing the renew drain.
- **No new dangling-ref surface:** assignment is ephemeral (scout discipline), proven in production.
- **Measurable & iterable:** with ADR 0006's combat sim + cohesion metrics, every tactic change moves the score and is inspectable ("why did slot 2 stop advancing on tick 14"). Self-play surfaces formation/focus bugs with the bot's own code.
- **Homes for the orphaned fixes:** economy-abort (IBEX-026), excess-attack trim (IBEX-028), boost demand (IBEX-027), guarded combat intents (IBEX-029).

**Negative / new risks**
- **New queue + manager + a tactics rewrite** in a fragile area. Mitigated by strangler-fig + behavior-parity-first validation on the harness, and by the queue being introduced *behind* the existing `SquadContext`/`SquadCombatJob`.
- **Hard gating:** unsafe before ADR 0001 (`SquadId`) and incomplete without ADR 0003 (movement) and ADR 0006 (the harness that proves each step). It therefore slots **after** the harness-first work.
- **Pre-spawn mis-estimation / lead-follower stall** — bounded by the cohesion/deadline force-abort and the give-up backoff.

**CPU & tick-safety**
- Reconciliation is O(objectives × squads × slots) — tens, not thousands — once per tick; it emits spawn *demand*, never creep intents. Authoritative focus-fire removes the 4× redundant per-creep scans (net intent/CPU win). Pre-spawn-over-renew removes the perpetual renew drain. Squad movement draws from the single budgeted pathfinding facade (ADR 0004), not a private path. Manager re-planning is sheddable; reading a stale-but-valid order set is the designed degraded mode.

## Robustness to a dynamic MMO (anti-overfitting)

No opponent-specific constants anywhere in the design. Threat is **measured at runtime** (`threatmap.rs` per-hostile body analysis, conservative ×4-boost assumption); force is sized from that measurement (`damage.rs` + `sized_*_body`); focus-fire targets aggregate enemy heal; kiting is range/fatigue policy; cohesion is a geometric invariant; the give-up backoff degrades gracefully against an unexpectedly strong defender (abandon + retry-later) instead of feeding a death-spiral. The harness reinforces this: the sim runs the bot's *real* decision code (no tactics fork to overfit), scenarios perturb terrain/positions/bodies across N seeds, opponents are a *roster* (scripted + self-play + recorded-from-MMO), and the live seg-57 cohesion canary is the final arbiter (ADR 0006 §anti-overfit).

## Incremental Migration Path

**Sequencing is harness-first** (operator decision, 2026-06-16). ADR 0006's combat micro-sim + cohesion metrics + military score term land **before** the behavior steps below, so every step is measurable and replay-diffable. The behavior steps are then gated behind ADR 0001 (`SquadStore`/`SquadId`) and ADR 0003 (lead-follower movement) per those ADRs' ordering. `WORLD_FORMAT_VERSION` in `game_loop.rs` MUST bump on any serialized-shape step.

| Step | What | Seam / files | Breaking |
|---|---|---|---|
| **H** | **Harness first** — combat micro-sim, cohesion/orphan metrics in seg-57, unblock the `military` score term (ADR 0006 Inc A–E). Combat changes become measurable + introspectable. | `screeps-combat-engine/agent/eval`, `screeps-ibex-metrics`, `screeps-ibex-eval/score.rs` | None (host-only) |
| **0** | **Quick wins** (today, pre-manager): whole-squad-centroid focus target; recompute `heal_power` + boost-aware heal math; wire `check_movement_failure` (IBEX-015); add a `Recall` terminal state so orphaned creeps recover now | `attack_mission.rs`, `squad.rs`, `squad_combat.rs` | Behavioral |
| **1** | ~~**Wire `SquadDefenseMission` onto `SquadContext`**~~ — **SUPERSEDED by Step 5** (`squad_defense.rs` was removed entirely in G4-defense; the interim `new_with_squad` wire was bypassed, defense went straight to the `Defend` objective). | ~~`squad_defense.rs`~~ | Moot |
| **2** | **Proper anchor mover** replaces the straight-line `advance_virtual_pos`: footprint-aware ("moving-maximum") anchor pathfind + lockstep block advance + hard cohesion gate + `pull`; wire dormant orientation (`threat_direction`/`orient_toward`/`reassign_slots`/`mirror_y`); add **column-collapse** single-file fallback (rover `Follow`/`pull`) for corridors/edges + **loose-centroid** for N>4 | `formation.rs`, `squad.rs`, `squad_combat.rs`, pathfinding system, `screeps-rover` | Behavioral — **mechanism + manager wiring DONE: formation travel (P2.G4-O1) + threat orientation (O2, projection-based, no layout double-rotation) + breach-aware dismantle targeting (O3, `room_grid_dijkstra` moved into `screeps-rover`).** |
| **3** | **`SquadId` key** replaces `squad_entity: Option<u32>` | `jobs/squad_combat.rs:18`, `jobs/data.rs`, `cleanup.rs`; ADR 0001 A1→A2 | Memory/format (one loud reset) |
| **4** | **`CombatObjectiveQueue` + `SquadManager`** behind the live offense path (parity), then migrate `AttackMission` to a producer; manager computes tactics; generalize `handle_wave_wipe`; delete combat `request_renew` | new `military/objective_queue.rs` + `military/squad_manager.rs`; `attack_mission.rs` shrinks; `war.rs` produces objectives | Memory/format + Behavioral — **queue+manager+tactics DONE (G1/G2/G3/G3-tail + G4-O1/O2/O3 + O4 wave-retry/unwinnable); migrate `AttackMission`→producers IN PROGRESS — single-squad producers live (O6: `InvaderCore`→`Dismantle`, `AttackFlag`→`Secure`, `ResourceDenial`→`Harass`; `InvaderCreeps` reconciled into remote-defense `Defend`), coexisting with `AttackOperation` for power-bank + the deferred heavy assault; power-bank `Farm` = O5, delete = O7 (soak-gated). Full offense checklist: [`../execution/g4-offense-plan.md`](../execution/g4-offense-plan.md).** |
| **5** | **Migrate defense + escort onto the queue**; `WarOperation` becomes a supervisor (withdraw/trim — IBEX-026/028); add `UnwinnableTarget` backoff | `war.rs`, `squad_defense.rs` removed, `claim.rs` escort producer | Behavioral — **defense DONE (P2.G4-defense, 2026-06-18: `squad_defense.rs` removed, all defense → `Defend` objectives, WFV 11→12); escort + supervisor still TODO (W2–W4).** |
| **6** | **Synchronized spawning** via `GroupId`/align-finish/pre-spawn; boost handoff (or kill-switch off) | `spawnsystem.rs` executor unchanged; ADR 0011 D3/D4/D9 | Behavioral |

**Validation per step (ADR 0006 sim + private-server gate):** Step 0/2 — cohesion-rate (fraction of combat ticks all-members-in-range) rises in the sim and on the seg-57 canary. Step 4 — replay intent-diff parity on a recorded engagement; kill a member mid-siege → successor pre-spawned; attack an unreachable room → torn down within the deadline. Step 5 — threat clears → defense squad retired within the deadline (closes the lingering-defense hang). Step 6 — group members emerge within W and rally cohesively, not trickled.

### Pulls from the expansion lifecycle (ADR 0017)

ADR 0017 (threat-aware expansion) shipped the safe-claim / abort half but **deferred two squad-dependent pieces to this overhaul**:

1. **Expansion escort / pre-clear — a new objective kind.** When a claim target is *marginal* (a transient/weak threat, economically worth taking), the claim pipeline declares an `Escort{room}` objective — pre-clear (the salvage `DismantleJob` for a remnant spawn/tower; a small squad for a weak combat creep) and hold it clear while the `[Claim,Move]` claimer commits. `DefenseEscalation::from_threat` is already `pub` for sizing it. Until this lands, ADR 0017 conservatively treats marginal rooms as unsafe (reject, never escort).
2. **The defense-staleness retirement already has an interim fix.** ADR 0017 §13 made `SquadDefenseMission` self-terminate the moment its room stops being `owner().mine()`. Step 5 must preserve this *ownership-subordinate* invariant on the queue: a `Defend` objective for a room we no longer own is **withdrawn immediately** (its `SuccessPredicate` already encodes this), not just when "threat clears + members dead." This is also the teardown cascade for ADR 0017's `unclaim()` abort — keep it.

### Implementation learnings — defender body sizing (interim defense path, 2026-06-16)

Surfaced by a live incident: an established but **towerless RCL2 room (W11N57)** was being declaimed by an enemy **CLAIM creep** with no defender ever spawning. Fixing it end-to-end on the pre-manager `SquadDefenseMission`/`bodies.rs` path produced combat-system learnings the `SquadManager` MUST carry forward — these are properties of *body sizing* and the *spawn/threat seam*, independent of whether lifecycle lives in a mission or the manager.

1. **The defense trigger must key on threat by body parts, not just combat parts.** `RoomDynamicVisibilityData::hostile_creeps()` flags only ATTACK/RANGED_ATTACK/WORK, so an enemy CLAIM creep neutralising the controller (or a lone dismantler/healer) was invisible and no defense was created. The trigger now also fires on `hostile_threat_creeps()` and commits a defender to hostiles bearing a "worth-defending" part (Attack/RangedAttack/Work/Claim/Heal) — `war.rs` `hostile_warrants_defender`. The manager's `Defend` trigger must inherit this: a controller-attacker is a defendable threat.

2. **A body's MINIMUM buildable cost must be affordable at the lowest RCL the role serves.** `solo_defender_body`'s repeat unit `[RangedAttack, Move, Heal, Move]` (500e) made `create_body` return `Err` below 500e, so a young RCL2 room spawned nothing and was declaimed undefended. **Rule: never force an expensive part (HEAL/TOUGH) into the minimum repeat — size it as a fixed, affordability-gated addition.** Same latent bug existed in `duo_healer_body`.

3. **Threat-matched sizing, built as a direct `Vec<Part>`.** Pure helpers in `military/damage.rs` (`attack_parts_to_kill`, `defender_heal_parts_for_dps`, `defender_spawn_readiness`) turn `RoomThreatData` + room energy into a body; the sized builders (`bodies.rs sized_defender_body`/`sized_healer_body`) construct the final `Vec<Part>` directly (offense → HEAL when affordable → TOUGH when affordable → MOVE, degrading to a `[RangedAttack, Move]` 200e floor), bypassing the `&'static [Part]` repeat-template constraint. The manager's composition→roster step should size members this way, not from fixed `BodyType` templates at `energy_capacity`.

4. **Offense must out-damage the AGGREGATE enemy heal (focus-fire), not per-target self-heal.** `attack_parts_to_kill` takes `enemy_focus_heal = RoomThreatData.estimated_heal`. When one defender can't out-DPS it (`None`), **escalate count** (Duo/Quad stack DPS and focus-fire one target), not build a bigger solo — the manager's `ForceRequirement` knob.

5. **Spawn-now-vs-wait needs `energy_available` (current) vs `energy_capacity_available` (max).** `defender_spawn_readiness` returns `Wait` for a capable room that just needs to refill, but `SpawnNow(available)` when nothing holds the line in a towerless room — a smaller defender now beats a perfect one too late.

6. **Multi-room sourcing via one shared spawn token.** A defender is sourced by broadcasting ONE `spawn_queue.token()` to every in-range home room (≤ `MAX_DEFENSE_SOURCE_DISTANCE`); the shared `spawned_tokens` set fulfils it at most once. The manager's reconciler should use this pattern for shortfall demand: one token per slot, several candidate rooms.

7. **Owned rooms must be defended against NPC invaders** (Source Keepers excluded). The manager's `Defend` trigger must not assume player-only threats.

When the `SquadManager` lands (Step 4–5), fold the sizing helpers (`damage.rs`) and the `sized_*_body` builders into its composition→roster + pre-spawn steps; do not regress the minimum-affordability (§2), focus-heal (§4), and energy-readiness (§5) properties.
