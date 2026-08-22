# ADR 0027 — Combat Objective/Squad Lifecycle Rework (P-OBJ #23)

- **Status:** Decided

- **Date:** 2026-06-27
- **Scope:** the combat objective/squad lifecycle — commitment lease, resolve-vs-give-up,
  whole-squad **Reassign**, **threat-centric defense**, and the uniform producer model in which every
  combat objective is produced by a kernel, queued, and fielded by the `SquadManager`.
- **Related:** ADR 0008 (squad lifecycle) and ADR 0026 §9 (doctrine selection/sizing) — the companions;
  ADR 0031 (force sizing / drain), ADR 0032 (the EV auction that chooses reassign / claim / StayPut /
  Recycle / Merge in one global solve — the Merge column is this ADR's transfer/merge phase),
  ADR 0034 (rally/travel/convergence robustness), ADR 0035 (scout-before-commit +
  abandon-on-unwinnable-contact), ADR 0036 (opportunistic RAZE structure targeting), ADR 0037
  (tower-aware neighbour defense). Task #23 / #25.

## Problem (Docker soak, 2026-06-27)

A force-sized offense squad "did nothing / scattered" against level-0 invader cores.
Root cause was a **three-system fatal coupling**, not a tactics bug:

1. Remote-core intel goes **stale ~every 200t** (no creep keeps eyes on the room) →
   the `war.rs` producer hits its stale gate (`war.rs:720`) and **stops re-pushing**
   the objective.
2. The objective **TTL-lapses** (`OFFENSE_OBJECTIVE_TTL=100`) — *shorter than the
   ~150–250t a squad needs to form + travel*.
3. The `SquadManager` retires the **still-forming** squad on `objective_gone`
   (`squad_manager.rs:241`), reading nothing about squad `state` → members orphan and
   **scatter**, often stranded mid-cross-edge (the "stuck on a room edge" reports).

Squad survival was coupled 1:1 to intel freshness. The offense was a spawn → orphan →
die conveyor that cleared nothing.

## Design

Reuse the **already-serialized but dormant** `CombatObjective.deadline: Option<u32>`
(was written by `request`, never read) as a manager-owned commitment lease.

### (a) Commitment — `objective_queue.rs`, `squad_manager.rs`
- `expire()` keeps an objective past its TTL while it is **CLAIMED** (`claimed_by`,
  a within-session resource) OR its **`deadline`** lease is in the future (serialized,
  bridges a VM reset / the cross-system ordering gap). Dies only on explicit
  `withdraw()` or once both lapse. (+`set_deadline`.)
- `field_new_squad` stamps `deadline = now + COMMITMENT_BUDGET (400)`; Phase A
  **refreshes** it every tick the squad has a `focus` (actively closing/fighting), so
  a long clear or a brief vision gap never drops the objective.

### (b) Resolve vs give-up — `squad_manager.rs` Phase A (new `SquadContext.engaged_once`)
`engaged_once` latches on the first `Engaged` tick — the signal that distinguishes a
squad that *fought and cleared* from one *just arriving* (Phase A runs before Phase B2
sets the focus) or *stuck en route*.
- **RESOLVE**: `engaged_once && in-target-room && no-focus` → target cleared →
  `withdraw()` the objective (clean win, no backoff) and retire.
- **GIVE-UP**: `deadline` lapsed with no focus and no clean clear → stuck/abandoned →
  `mark_unwinnable` (non-Defend) so we don't immediately re-field into a dead end.

### (c) Intel coverage — `squad_manager.rs`
For every live objective the manager pins **OBSERVE-only, HIGH** visibility on its
room, so an in-range RCL8 observer keeps `last_seen` fresh for free (no scout burned on
a walled target). Commitment + the lease cover rooms with no in-range observer.

### (d) Zero-orphan recall — `jobs/squad_combat.rs`
A retired squad's surviving members **recall themselves**: in the orphan fallback
(squad gone — `get_squad_state == None` — and nothing to fight) a combat creep moves to
the nearest home spawn and **recycles** instead of idling/scattering.

## Resolutions against the original 6-step plan

The design was authored as six steps; three of them resolved differently than first drafted, and those
resolutions are load-bearing:

| Plan step | Resolution |
|-----------|------------|
| 1 expire immunity | §(a) — the claim/deadline immunity in `expire()`. |
| 2 deadline heartbeat | §(a)/§(b) — the lease stamp + refresh, with resolve/give-up keyed on `engaged_once`. |
| 3 producer re-assert from last-known | **Subsumed** by (1): a claimed objective can't lapse underneath its squad, so producer silence on stale intel is already harmless. No re-assert path is built — it would be dead code. |
| 4 intel coverage pin | §(c) — OBSERVE/HIGH for live objectives. |
| 5 manager-side member cleanup + integrity sweep | **Rejected in that form**: `EntityCleanupQueue::delete_creep` only deletes the ECS entity (a live creep is re-discovered next tick), so disposing a live member must be an **in-game** action by its own job → §(d). `retire_squad`'s raw squad-entity delete stays (generation-safe `SquadRef` → an orphan resolves to `None`, never aliases); `repair_entity_integrity`'s member-scrub plus the Phase-A `objective_gone` retire prevent leaks without a new sweep. |
| 6 recall terminal state | §(d) — the orphan-fallback recall. No new job-FSM variant is needed. |

`SquadContext.engaged_once` is the one serialized-shape addition the design makes.

## Why one fresh look now suffices

Frontier-core intel is *chronically* intermittent (refreshed only by a passing scout).
Before: the squad had to **win a race** against the 100t TTL — usually lost → churn.
After: a single fresh look creates the candidate; the squad is then **committed** and
the objective survives the 400t form+travel window regardless of intel going stale, so
it arrives, clears, and resolves. Candidate *discovery* still rides the existing
intermittent scouting (a `requested re-scout` on the central visibility queue).

## Invariants the lifecycle rests on

Two non-obvious invariants, each established by a live failure or an attempted "fix" that regressed:

- **Rescout cadence must be DERIVED from intel lifetime, never set independently.** With
  `STRONGHOLD_RESCOUT_INTERVAL` (1500) longer than `THREAT_DATA_MAX_AGE` (500), intel expires before the
  re-probe, a known-stronghold SK room drops out of `threat_rooms`, and that stops BOTH the offense
  evaluation AND the in-join `>200t has_known_core` backstop — the room is silently abandoned (never
  farmed, never cleared). So `STRONGHOLD_RESCOUT_INTERVAL = THREAT_DATA_MAX_AGE / 2` (a 2× margin,
  = 250 — one peek per 250t, no scout storm), guarded by a `const _` compile-time assert that hard-fails
  the build if the relation ever inverts. (Leaving an *unaffordable* high-level stronghold to
  self-collapse remains a deliberate economic gate, not a coverage gap.)
- **`estimated_dps == 0` does NOT mean harmless — the harmlessness signal is `hostile_warrants_defender`.**
  `project_intel`'s `danger.max(priority_implied_danger(priority))` floor looks like it would make a
  harmless scout pull a CRITICAL defender, but a truly harmless scout (MOVE-only, no
  Attack/RangedAttack/Work/Claim/Heal) is rejected by `hostile_warrants_defender` (war_decision.rs:221)
  at the PRODUCER, so no `Defend`/`Secure` objective is ever emitted for it and it never reaches
  `project_intel`. The floor is load-bearing for what *does* reach it: a warranted threat whose
  `estimated_dps == 0` because it carries no Attack/RangedAttack parts but is dangerous anyway — a CLAIM
  declaimer, a WORK dismantler, a HEAL creep (`estimated_dps` counts only Attack/RangedAttack,
  threatmap.rs). Trusting `dps==0` there starves the objective's value to zero
  (`value_e(Defend) = asset · defense_risk(0) = 0`), the EV claim filter drops it, and a declaimer takes
  an owned room unopposed. Cross-refs the dual-DPS / `estimated_dps`-overload concern in ADR 0031.

## Reach, form, and travel — the end-to-end pipeline

The lifecycle is only useful if the squad actually gets there. The pipeline it drives is
scout → commit → size → multi-home spawn → solo-travel to a shared rally → gather → formation assault →
arrive → focus → engage → clear. Its load-bearing elements:

- **Scout/offense reach.** Priority-driven **Chebyshev** scout reach (a Manhattan-5 reach excludes
  BFS≤10 offense targets), no `.take(slots)` truncation ahead of the range filter, HIGH re-scout priority
  for offense rooms, lvl0 cores exempt from the offense concurrency cap, and no blanket early-return when
  `total_free_spawns == 0` — candidate *discovery* is what feeds the commitment lease, so starving it
  starves everything downstream.
- **Spawn priority + efficient sizing.** Forming combat members spawn at
  `SPAWN_PRIORITY_COMBAT_FORMING = 85` — above economy bulk, below CRITICAL miners. The EV optimizer
  sizes **undefended, zero-attrition** structure targets to the *minimal* effective force (binary p_kill:
  no over-power ladder where P(win) ≈ 1); defended sizing keeps the calibrated ladder. A related tuning
  edge: floor distance-0 miner priority ≥ 90 so a forming squad can never out-prioritize a marginal
  top-up miner.
- **Quorum rally + forming/travel lease.** Uncontested targets deploy at a min-viable quorum, and the
  commitment lease refreshes through both the forming-in-flight banking gap AND the travel phase, each
  bounded (`MAX_FORMING_BUDGET = 3000`, `MAX_TRAVEL_BUDGET = 1000`) so a stuck squad still terminates.
  Without those refreshes the lease lapses mid-spawn or mid-travel and the Generation churn returns.
- **Focus-on-arrival.** The arrival tick sees an empty room DTO; bridge it by reading hostiles/structures
  directly from `game::rooms()` when `mapping.get_room` is `None`, so a squad focuses the tick it lands
  rather than idling one tick in contact.
- **Fighter-first spawn order.** A partially-spawned roster must already be combat-capable. This is a
  bot-side spawn-*order* concern only — the assembled force must stay byte-identical (reordering the
  assembler itself regresses `assembler_kills_across_defended_regimes`).
- **Shared-rally traverse.** Multi-home spawn is preserved; members solo-travel to ONE shared rally
  (`rally::shared_rally_point`, derived fresh each tick so it carries no serialized state — uncontested →
  target centre, contested → one room short, out of tower range), gather via the **unified
  `rally::gather_quorum_met` kernel called by BOTH the bot and the sim** (a second copy drifts and freezes
  the box anchor), then assault rally→target in box formation.
- **`[SquadTrace]` introspection.** A debug-gated (`features.military.debug_log`) per-squad
  STATE/MEMBER/DEPLOY/TRAVEL/ARRIVED/FOCUS/ENGAGED/GIVEUP trace: the lifecycle is otherwise unobservable
  live, and every root cause in this ADR was found through it.
- **Latch discipline.** `engaged_once` latches only on real in-room presence (never en route); an assault
  latches once gathered (so `in_room` ↔ `travel` cannot oscillate); a Defend squad holds station on a
  clear owned room instead of `GaveUp`-then-re-field churn.

Rally/travel convergence beyond this (renew-in-transit, scatter-robust rally, movement-failure
escalation, majority-progress lease) is designed in **ADR 0034**; fight-through vs route-around on
incidental contact and abandon-on-unwinnable-contact are **ADR 0035**; the sizing/composition
side (defense right-sizing, drain mode, budget-free `emit_requirement`) is **ADR 0031**.

## Design — Squad reassignment

Subsumes defense targeting and survivor reassignment, and removes the
retire→re-field churn for non-loss terminals. Chosen shape: **threat-centric defense (Option B)**
+ **whole-squad reassignment** + **Lanchester-guarded creep transfer/merge** (the pending-slot rule
below; only *dilutive* splitting is rejected). Reviewed against the existing objective model for
cohesion; lands in the same pure-kernel + thin-adapter seam as the rest of 0027.

### Problem
A squad that **Resolves** (target cleared) or hits **ObjectiveGone** (target vanished)
retires → members recycle or a fresh squad re-fields next tick (Generation churn), wasting
the invested spawn energy. And a garrisoning defender holds its now-clear owned room while
the threat roams a neighbor (the `holding_station` fix *bounds* the waste but doesn't make
the squad useful).

### Decision — reassign-on-terminal, in-place rebind, composition-gated
- **Kernel** (`screeps-combat-decision/src/lifecycle.rs`): new `ReconcileAction::Reassign {
  withdraw_old: bool }`, returned in place of `Retire{Resolved}` / `Retire{ObjectiveGone}`
  **iff** a manager-computed `reassign_available: bool` (a new `ReconcileSnapshot` input, fed
  in exactly like `holding_station` so the kernel stays pure/deterministic) is true. Resolved
  → `withdraw_old=true` (record the clean win); ObjectiveGone → `withdraw_old=false`.
  **`Wiped`/`Duplicate`/`GaveUp` still retire** (no members / unwinnable-backoff — don't chain
  a tired squad straight into another fight).
- **Manager** (`squad_manager.rs` Phase A): compute `reassign_available` + the target via a new
  `best_reassignment` = `best_unclaimed_near_excluding(exclude=[current_id])` + a **capability
  gate** (v1: same broad class — defender→`Defend`/`Secure`, offense→offense; full ADR-0031
  capability match later). On `Reassign`, **rebind in place — no `retire_squad`/`field_new_squad`,
  no Generation churn, bodies reused**: release/withdraw old claim → `claim(new)` (and add it to
  the Phase-A `covered` set so a second reassigner can't double-claim) → rewrite
  `SquadContext.objective_id`+`target` → reset `engaged_once=false`/`focus_target=None`/
  `state`/`squad_path` → **clear + re-key the `SquadFormingProgress` clocks** under the new id
  (reuse the existing re-field cleanup block, then stamp fresh `forming_started_at`) →
  `set_deadline(new, now+COMMITMENT_BUDGET)`.
- **Reassignment is ATOMIC + resets rally/lease/renew as ONE step.** A squad is one unit with one
  rally, one commitment lease, one renew-state; `Reassign` resets all three together for the new
  objective — re-gather at the new `shared_rally_point` (reset `engaged_once`/`focus_target`/
  `squad_path`), reopen the `COMMITMENT_BUDGET` lease (`set_deadline`), and let the Phase-B renew
  pass follow the new rally (renew only if it's near a spawn). No partial/per-creep reassignment
  (see *Deferred / rejected* below) — atomicity is what keeps those three coordinated.
- **WFV:** the in-place rebind only rewrites the already-serialized `objective_id` (no shape
  change), so reassignment alone needs no bump — but a bump is acceptable where it buys a cleaner
  model (we don't dodge it with ephemeral hacks). Option B's threat-centric `Defend` semantics +
  any new producer fields land under one deliberate bump rather than contortions.

### Creep transfer & merge — the pending-slot Lanchester guard
Whole-squad reassignment is one move; a squad can also **merge** (wholly) or **partially merge** into
another. The guard that keeps this Lanchester-safe: **a creep may transfer to another squad ONLY to
fill that squad's PENDING SPAWN SLOT (compatible role).** Then the receiver's *target* force is
unchanged (it fills the slot by transfer instead of by spawn) — no new under-strength force is created,
the donor only sheds creeps it no longer needs, and the move is **concentration, not dilution.** Three
ops collapse to one safe primitive:
- **Reassign** — the whole squad takes a new objective (atomic; above).
- **Merge** — squad A's members fill squad B's pending slots; A empties → retires, B fields fuller/sooner.
- **Partial merge / reinforce** — some of A's members fill some of B's pending slots; leftover A keeps
  its objective or recycles.

It is strictly positive: reuses A's invested creeps, **saves B's spawn energy + time** (B fields
sooner), and the transferred creep can **RENEW with the forming receiver** (recover lifetime vs
recycling). It also eases the spawn-starve forming tail: two squads stuck at 1/4 each can
**merge into one at 2/4** instead of both churning.

**When it fires (donor sheds, never weakens mid-fight):** A is terminal (Resolved/ObjectiveGone) with
survivors, OR A is over-rostered (objective needs fewer), OR two forming squads consolidate. The
receiver B is **forming** (has pending slots); prefer a **nearby** B (minimize transfer travel).

**Coordination (the renew/rally concern) is clean because the RECEIVER is the coordination unit:** B
already owns its rally, commitment lease, renew-state, and the *defined* pending slot — a transferred
creep just sets its squad-ref + slot to B's pending slot, joins B's gather, and renews under B; the
spawn queue drops the now-filled slot. The pending slot is the handoff point, so nothing fragments
(contrast a dilutive split, which would have to *invent* a new rally/lease/renew unit). The
(creep → pending slot) match is deterministic (role-matched, nearest-B, `BTreeMap`/`Vec` order).

**Phasing:** v1 = whole-squad **Reassign** (the foundation, provable alone). v2 = the **transfer/merge**
pending-slot primitive (reinforce survivors + consolidate forming squads) — subsumes the old
"survivor-reinforcement pool".

### "Defending the wrong room" — threat-centric defense (Option B, chosen)
Reassignment can only re-point a freed defender to objectives that **exist**, and today `war.rs`
emits only `Defend{owned_room}` (the offense scan `continue`s on owned rooms — `war.rs:759`), so a
freed defender has nothing at the neighbor to go to. **Make defense threat-centric:** the defense
scan emits the clear objective at the **threat's CURRENT room** as `ObjectiveKind::Secure{room}`.
**No new `Intercept` kind** — an intercept is mechanically *"go to room X and clear its hostiles"*,
which is exactly `Secure`; the only differences (priority, TTL, doctrine, HUD label) ride existing
fields, not a parallel variant. When the threat is in an owned room the `Secure` objective sits
there (today's defend behavior); when it roams a neighbor the objective **moves with it** (re-emitted
each ~2-tick scan at the threat's room; the stale one TTL-lapses), and the squad **reassigns to
follow** (`ObjectiveGone` on the old → `Reassign` to the new). Two policy guards: an **asset-priority
boost** when the threat is in/adjacent to a valuable owned room (base defense outranks chasing a
distant roamer), and a **leash** so a squad doesn't over-extend away from its base. This **deletes
the empty-room-garrison as the default** — `Defend{owned}` survives only as an *optional preemptive
rampart-hold* for high-value bases, and `holding_station` becomes the bounded fallback, not the norm.
**Rejected: objective-follows-threat** (mutating one objective's `room` breaks the queue's
`kind == identity` upsert/claim invariant — `objective_queue.rs:225-260`); the producer re-emits per
the threat's room and reassignment + TTL handle the hand-off instead.

### Deferred / rejected
- **DILUTIVE split — REJECTED** (only the dilutive case; the *concentrating* transfer/merge is
  first-class above). Peeling creeps into a NEW, smaller squad that does **not** fill an existing
  pending slot creates an under-strength force (Lanchester loss) and would have to invent a fresh
  rally/lease/renew unit. The **pending-slot rule is the line**: fill an existing slot = concentration
  (allowed); spawn a new weaker force = dilution (rejected).
- **Preemption** (reassign to a higher-EV target mid-flight) — deferred (thrash; the
  `assault_latched`/`engaged_once` latches exist precisely to stop un-committing). Behind a flag
  if ever pursued.
- **GaveUp-reassign** — excluded for v1 (the squad just `mark_unwinnable`'d its room).

### Cohesion risks → mitigations
claim race → reassign-claim immediately + add to `covered`; lease/Generation accounting →
reuse the re-field cleanup + re-key the per-id clocks; ping-pong → terminal-only + `exclude=
[old_id]`; composition mismatch (defender onto an uncrackable core → `IN_ROOM_NO_FOCUS` stall →
poisons the room unwinnable) → the capability gate; unwinnable poisoning → `best_unclaimed_near`
already skips backoff rooms; determinism → selection is `max_by` over a `Vec` + the capability
gate is a pure fn over sorted roles (no `HashMap`).

### Offline repro / tests (extend `screeps-combat-eval/src/harness/lifecycle.rs`)
New `ChurnOutcome::Reassigned { from_gen, to, reuse_tick }` + cases over the shared kernel:
(1) reassign-on-resolve (assert **same generation** = reuse, vs churn's climbing generations);
(2) reassign-on-expire (+ a no-sibling control that still falls back to retire — reassign is
strictly additive); (3) defender-reassigns-to-threat (a neighbor `Secure` appears on owned
`ObjectiveGone` → `Reassigned{neighbor}` not `Garrisoned`; + a capability-mismatch control that
holds/recycles). Plus pure-kernel unit tests: resolved/gone→`Reassign` with correct
`withdraw_old`; wiped/gaveup never reassign; `reassign_available=false`→existing retire.

### Seams
`lifecycle.rs` (new action + snapshot input + tests) · `squad_manager.rs` (Phase-A rebind +
re-key) · `objective_queue.rs` (capability-aware selection helper over
`best_unclaimed_near_excluding`) · `war.rs` (threat-centric defense: emit `Secure{threat_room}` +
asset-priority boost + leash, demote `Defend{owned}` to an optional preemptive hold — the "wrong
room" half) · eval harness (the churn cases). Cross-ref ADR 0026 (threat-centric targeting is a
doctrine/targeting change).

## Producer unification + the sim-able production layer

Objective assignment is **uniform**: one pipeline, `producer kernel → objective_queue → squad_manager`,
for every piece of combat work — and every layer of it is drivable offline.

### The producers

- `war.rs:run_defense_scan` → `Secure{owned/neighbour threat room}` + `Defend{flag/remote}`
  (owner=Defense) via `emit_defense` + `neighbour_threats`.
- `war.rs:run_offense_evaluation` → `Secure{room}` (AttackFlag) / `Dismantle{room,pos}` (InvaderCore)
  / `Harass{room}` (ResourceDenial = GatedPlayerRaid), owner=Attack. There is no parallel legacy
  offense path.
- `SourceKeeperFarmMission` → `Farm{SourceKeeper}` (owner=SourceKeeper) → `duo_sk_farmer`.
- `SalvageMission` is a **thin producer**, not a combat manager: a breach producer emits
  `Dismantle{room, breach-blocker pos}` (owner=Attack, LOW) when a breach is possible and there is
  surplus — which is what gives the `SiegeBreach` doctrine a producer at all — and a declaim producer
  emits `Declaim{room, controller}` when the controller is `ReachableNow`. Declaim carries its own
  role/body/doctrine (`SquadRole::Declaimer`, a CLAIM body, the always-field `DeclaimAttack` doctrine,
  `SquadTarget::AttackController`) and a `declaiming` lease-hold that persists across the 1000-tick
  `attackController` cadence; the EV gate stays in `SalvageOperation`.
- **Economy work stays off the combat pipeline by design:** salvage raiders (`HaulJob` — hauling, not
  combat), claimers/builders, scouts, reservers/miners.

The declaim path depends on the breach path (a breach is what opens the route to a walled controller),
and the whole pipeline depends on the pure-kernel production layer below.

### Sim-able layers (the "sim the layers" requirement)
The combat stack, each layer with a pure kernel a harness drives:
1. **Production / observation** — `emit_defense`, `neighbour_threats`, `observe_neighbours` (the
   `game::*` hostile-fold lifted out of `run_defense_scan` so observation is a pure kernel), the
   offense candidate→objective map; `objective_value::value_e` (ADR 0032).
2. **Assignment / lifecycle** — `reconcile`, the objective queue, **the EV-global assignment
   (ADR 0032)**.
3. **Sizing** — `emit_requirement` + `optimize_composition` (ADR 0031).
4. **Spawn** — the spawn-priority / forming model.
5. **Movement / rally** — `shared_rally_point` + `gather_quorum_met` + the traverse kernels.
6. **Combat / tactics** — `decide_squad_with_pathing` + the engine sim.

Every layer is driven offline by `run_v1_flow` (the full production chain
observe_neighbours → neighbour_threats → emit_defense → queue → reconcile), `run_offense_flow`, and
`run_lifecycle_churn[_spatial]` / the agent sim / the eval. The requirement is that the WHOLE stack be
offline-provable — pieces like neighbour-observation are proven in the sim, not discovered broken on
Docker.

### Cross-references
The EV-positive, globally-optimal ASSIGNMENT (replacing the greedy claim/reassign — both defects
the operator named) is **ADR 0032 (P-AUCTION, #28)**: an energy-equivalent `value_e` currency + a
deterministic Hungarian matching over squads × {objectives + StayPut + Merge + Recycle}, with the
v2 transfer/merge as an EV-scored column. The new `DeclaimAttack` doctrine / `SquadRole::Declaimer`
/ CLAIM body cross-ref the ADR 0026/0031 doctrine corpus; the sim-layering taxonomy cross-refs ADR
0019/0020. Task #28 / #23 / #25.

## Combat objective inventory — add/remove sites

Every `ObjectiveKind` (objective_queue.rs:81-93), where it is ADDED
(`combat_objective_queue.request`) and where it is REMOVED. Generic remove paths apply to all: the
lifecycle `reconcile` **Resolved → withdraw** (clean win) and **`mark_unwinnable`** (give-up), plus the
queue's **`expire`** (TTL elapsed AND not claimed AND no live deadline lease — objective_queue.rs:348).

**The Resolved gate (design of record, as refined).** A clean clear requires in-room members with no
focus and no lose-verdict, plus ONE of two evidence forms: (a) **`engaged_once`** — the squad fought
and the room is now clear (the original rule; ADR 0035 D4 added the `retreated_from_contact`
exclusion so a LOST fight cannot mis-resolve); or (b) **the vacuous clear** (combat review D28) —
the target room is LIVE-visible this tick with zero hostile creeps. (b) exists because an
uncontested clear of an ALREADY-EMPTY room can never latch `engaged_once` (the latch needs an
in-room focus and an empty room offers none), so a successfully-massed squad used to hold until the
budgets forced a GaveUp. Two boundaries are load-bearing: live visibility (a cached-empty DTO must
never resolve — the R10 vacuous-intel class), and `is_defend` exclusion (a defend garrison's quiet
hold is deliberate — FIX B2 — and its terminal is `objective_gone` when the producer stops
asserting). Kernel: `lifecycle::reconcile`; the evidence is manager-computed
(`ReconcileSnapshot.vacuous_clear`), keeping the kernel pure.

| Kind | ADD (producer → owner) | Dedicated REMOVE (beyond resolve/expire) |
|------|------------------------|------------------------------------------|
| `Secure{room}` | war.rs:534 owned-room threat (Defense), war.rs:589 neighbour threat (Defense), war.rs:1294→1427 AttackFlag (Attack) | — (resolve/expire) |
| `Defend{room}` | war.rs:682 defend-flag (Defense), war.rs:792 remote-invader (Defense) | — (resolve/expire) |
| `Dismantle{room,pos}` | war.rs:1286→1427 InvaderCore (Attack), salvage.rs:574 breach (Attack) | salvage.rs:492 breach withdraw (pos-scoped via `SalvageBreachTracker`, on standdown/re-arm) |
| `Harass{room}` | war.rs:1305→1427 ResourceDenial / GatedPlayerRaid (Attack) | — (resolve/expire) |
| `Farm{SourceKeeper,room}` | sourcekeeperfarm.rs:414 (SourceKeeper) | sourcekeeperfarm.rs:349 stronghold-present / stand-down withdraw |
| `Declaim{room,controller}` | salvage.rs:465 (Attack) | salvage.rs:403 declaim withdraw (pos-scoped, on standdown/re-arm/neutralize) |
| `Escort{room}` | **no producer** — the variant is defined + manager-handled; its intended producer is a claim/build escort (claimers travel unescorted without it) | — |
| `Farm{Core}` / `Farm{PowerBank}` | **no producer** — the `value_e`/value-kind arms exist (squad_manager.rs:335/337) but nothing requests them. `Farm{Core}` means denied-reservation *income*, which is distinct from razing a core via `Dismantle`; both are kept as planned-future kinds, to be given producers or dropped deliberately | — |

**Combat-adjacent work that is deliberately NOT objective-based** (mission-owned spawning; produces no
`ObjectiveKind`):
- `SalvageMission` **teardown dismantlers** (raze-for-salvage within the horizon) — a candidate to
  express as `Dismantle`, but distinct from the breach corridor, which is objective-driven.
- `SalvageMission` **raiders** (`HaulJob`) — **economy by design**: hauling, not combat.
- Room-safety for the mining-outpost gate is a pure predicate, `is_remote_room_safe`
  (missions/utility.rs), not a mission — there is no `DefendMission`.

## Landed

- e4bbf0f v1 lifecycle base — commitment lease, resolve-vs-give-up, zero-orphan recall (2026-06-27)
- d3352f2 scout/offense reach unblock — priority-driven Chebyshev reach, no slot truncation (2026-06-28)
- 4f41da8 whole-squad Reassign + threat-centric defense (`Secure{threat_room}`) (2026-06-28)
- d301324 stronghold rescout interval derived from `THREAT_DATA_MAX_AGE` + compile-time assert (2026-06-28)
