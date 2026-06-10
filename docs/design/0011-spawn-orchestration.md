# ADR 0011 — Spawn Orchestration & Group Spawning

- **Status:** Proposed
- **Date:** 2026-06-09
- **Deciders:** William Archbell
- **Related:** Field Report A (squads deploy staggered/scattered), Field Report B (lifecycle hangs sustained by renew), Field Report C (CPU governance — spawn is pinned always-on). Findings: spawn-ordering INFO guard (review §1 — ordering is CORRECT, do **not** re-flag), IBEX-022 (body-sizing min-cost — **REFUTED**, clamp present; do not re-flag), IBEX-046 (latent NaN priority), IBEX-002/IBEX-041 (renew-as-glue prevents teardown / renew waste), IBEX-026/IBEX-028 (economy gating & trim — homes are in ADR 0008), IBEX-027 (inert BoostQueue), IBEX-034 (flat military reserve), IBEX-035 (per-spawn×target pathfinder spikes), IBEX-047 (reactive `remove_creep` — replacement only after death), IBEX-032 (route-blindness). Review prompt §6.11(b) (break-on-unaffordable starvation question — this ADR answers it). Competitive analysis: **G4** (declarative wishlist reconciler, prespawn-replace-not-renew), **G3** (incubation), G2(d) (synchronized squad spawning). Sibling ADRs: **0001** (stable ids — `DemandId`/`GroupId` keying, `SquadId`), **0002** (serialization discipline — this ADR adds **no** persisted state), **0003** (behavior modeling — jobs stay statically assigned at spawn), **0004** (CPU governor — spawn is never-shed; travel estimates charge the budgeted facade), **0005** (scheduler placement/containment), **0006** (eval harness + seg-57 metrics — spawn-uptime KPI lands there), **0007** (hauling — extension refill is the orchestrator's energy-recovery signal), **0008** (Squad Manager — the single combat demand producer; pre-spawn doctrine §2.3), **0009** (room graph travel distances; spawn-exit/lab adjacency), **0010** (boost pipeline — lab-side fulfillment of the boost-on-spawn handoff). Prior art: Winsley "spawn uptime as the master KPI" / "Spawning Budget", Overmind `wishlist`/`lifetimeFilter`/`swarmWishlist`, KasamiBot priority spawn queues (`../references/external-references.md`, prior-art digest).

> **Scope boundary (read first).** This ADR owns the **spawn-time and spawn-energy budget**: how demands for creeps are declared, arbitrated globally, scheduled onto 1–3 spawns per room (and across rooms), synchronized for groups, pre-spawned against TTL, and renewed (rarely). It does **not** re-decide what forces a war needs (ADR 0008 owns objective→composition; its Squad Manager is *one producer of demands here*), how creeps behave after spawning (ADR 0003), or CPU tiering (ADR 0004). `spawnsystem.rs`'s verified-correct priority executor is **kept** as the per-room fulfiller — ADR 0008 §2 already pins "spawn stays a dumb fulfiller"; this ADR builds the brain that feeds it.

## Context

### Spawn throughput is a hard, budgetable resource — the engine math

Every design choice below is denominated in **part-ticks of spawn time**, because the engine makes spawn throughput as hard a ceiling as CPU:

- A spawn produces **one creep at a time** (`engine\processor\intents\spawns\create-creep.js:11-13`), taking `needTime = CREEP_SPAWN_TIME(3) × body.length` ticks (`create-creep.js:48`; body silently truncated to `MAX_CREEP_SIZE(50)`, `:36`). A room fields **1/2/3 spawns at RCL ≤6/7/8** (`common\lib\constants.js:215`), excess spawns are switched off (`_calc_spawns.js:9-22`).
- Therefore a room's spawn capacity is **0.33 / 0.67 / 1.0 parts/tick**, and — at `CREEP_LIFE_TIME = 1500` — its **sustained living-parts ceiling is 500 / 1000 / 1500 parts** (≈10/20/30 max-size creeps at RCL8). A CLAIM creep (600-tick life, `constants.js:111-112`) consumes **2.5× spawn-time per sustained part**. `PWR_OPERATE_SPAWN` multiplies `needTime` down to ×0.2 (`create-creep.js:51-54`) — a future lever the model must *read* (actual `needTime`), never assume.
- Energy is the **joint budget**: cost is charged atomically at intent processing, drawing **all spawns first (closest to the spawning spawn), then extensions closest-first** (`spawns\_charge-energy.js:6-39` — folklore correction #5: not interleaved). Two spawns the same tick draw sequentially against the mutated in-memory pool — **N spawns can start N creeps in one tick iff the pool covers the sum**. Capacity: 300/spawn + extensions (12,900 total at RCL8, `constants.js:153`).
- A blocked spawn exit slips emergence **+1 tick/tick** (`spawns\tick.js:17-27`) — group-emergence windows must tolerate slip, and ADR 0009's reachability rules (clear exits, lab-adjacent spawn placement) matter here.
- The trickle charge (+1 energy/tick while room spawn+extension energy < 300, `spawns\tick.js:44-47`) means a room is **never permanently energy-dead** — the recovery floor every starvation argument below stands on.
- `cancelSpawning` **refunds nothing** (`spawns\cancel-spawning.js:6-13` — folklore correction #4); renew is `floor(600/body_size)` TTL per intent at cost `ceil(1.2 × creepCost / 3 / size)`, **rejected entirely if it would exceed 1500 TTL**, never for CLAIM, and **strips all boosts with zero refund** (`spawns\renew-creep.js:28-33,45-53`); recycle refunds `floor(min(125, part_cost × ttl/lifeTime))` per part into a container on the creep's tile if present (`recycle-creep.js:22`, `creeps\_die.js:39-94`).

The prior-art consensus is unambiguous (digest §5.1): a mature bot is **spawn-blocked**, and dominant bots schedule spawn-time as an explicit budget (Winsley's spawn-uptime KPI and franchise-budget model; KasamiBot's priority queues; Overmind's `wishlist` reconciler). Ibex today has a correct *queue* but no *budget*.

### What exists today (verified against code)

**The queue and executor (`spawnsystem.rs`, 310 lines).** `SpawnQueue` is an ephemeral per-tick resource: missions push `SpawnRequest { description, body, priority, token, callback }` (`spawnsystem.rs:25-31`) into a per-room vec kept **descending** by a binary-search insert (`:82-95` — the review-verified-CORRECT comparator; the false-positive guard stands). `SpawnQueueSystem::run` iterates rooms from a `HashSet` (**nondeterministic room order**, `:284-289`), and per room (`process_room_spawns`, `:170-275`):

1. **Renew first.** If room stored energy ≥ 10k (`RENEW_MIN_ROOM_ENERGY`, `:14`), renew requests sorted by ascending TTL are served **before any spawn request**, each consuming an idle adjacent spawn (`:199-232`). Renew is gated by `renew_ttl_threshold = next_spawn_duration + 50` (`:196-197`) — i.e. renew only creeps that would die before the next queued spawn completes — but when the queue is empty (`next_spawn_ticks == 0`) **every** requested creep is renewed regardless of TTL (`:204`).
2. **Consume loop** (`:236-272`): for each request, **`continue`** if `body_cost > energy_capacity` (permanently unaffordable at this RCL — skip, `:243-245`), **`break`** if `body_cost > available_energy` (temporarily unaffordable — *reserve all remaining energy for this request*, `:247-249`), **`break`** when no idle spawn remains (`:268-270`). Tokens dedup multi-room submissions: the same `SpawnToken` queued in several rooms spawns **once**, in whichever room the `HashSet` happens to visit first (`:237`, `:257-259`).
3. A `SpawnQueueSnapshot` (queue depth per room) is written for next tick's `EconomyAssessmentSystem`, then the queue is **cleared** (`:300-308`) — every request is re-computed from scratch every tick.

**Demand production is per-mission, imperative, and ad-hoc.** Every mission re-derives its shortfall and re-pushes requests each tick: `source_mining.rs:392-419` (hard-coded `desired_harvesters = 4`, `//TODO` at `:389`; priority lerp CRITICAL→HIGH local, MEDIUM/LOW remote `:400-409`; the **empty-fleet clamp** `energy_available().max(SPAWN_ENERGY_CAPACITY)` at `:393-397` — the IBEX-022 refutation), `haul.rs:277-310`, `upgrade.rs:317-340`, `localbuild.rs:238-279`, `claim.rs:158-166`, `scout.rs:215-232`, `reserve.rs:202-215`, `attack_mission.rs:381-431` (one token per squad slot, submitted to every home room at flat `SPAWN_PRIORITY_MEDIUM`, `:419`), `squad_defense.rs:114-172` (HIGH). Bodies come from `create_body(SpawnBodyDefinition)` (`creep.rs:123-179` — pre/repeat/post with min/max repeat, cost- and 50-part-capped), via per-mission body helpers or the military `BodyType::body_definition` table (`composition.rs:41-64`).

**Global spawn arbitration is scaffolding, not a system.** `EconomySnapshot` computes `free_spawns`/`total_free_spawns` and a one-tick-stale `prev_tick_queue_depth` (`economy.rs:37-43,326-331,340`), but the only consumer is a single gate — `war.rs:572` blocks *new* attacks when `total_free_spawns == 0`. The within-tick coordination field `military_spawns_claimed` (`economy.rs:44-47`) is **dead**: initialized to 0 (`:350`) and never incremented — `room_mut` (`:118`) has zero callers. `SquadComposition` already has the right *estimators* — `estimated_cost` (`composition.rs:479-481`), `estimated_spawn_time` with a correct multi-lane simulation (`:486-509`), `is_viable_from` (≥40% residual TTL after spawn+travel, `:537-549`) — but they are consulted only at attack-launch scoring, never by anything that schedules spawns.

### The pathologies this ADR exists to fix

- **P1 — No demand model.** Requests are single-shot imperatives rebuilt every tick; the spawn layer cannot see *future* demand (a successor needed in 120 ticks), only *current* shortfall. Pre-spawn is therefore unrepresentable, and replacement is reactive-after-death (IBEX-047's `remove_creep` pattern).
- **P2 — No throughput model.** Nothing accounts parts-in-flight vs. parts/tick capacity, or demand backlog in part-ticks. A room with 3 idle spawns and a room with 0 look identical to a requesting mission; the multi-room token pattern picks the fulfilling room by **HashSet iteration order**, not by headroom, surplus, or travel time (`spawnsystem.rs:284-289`).
- **P3 — No group spawning.** A quad's 4 slots are 4 independent MEDIUM requests (`attack_mission.rs:416-423`). A 4×40-part quad is 480 part-ticks ≈ **480 spawn-ticks on one lane**: the first member burns ~30% of its TTL idling at rally before the last emerges — feeding exactly the Rallying-forever + renew-as-glue hang (Field Report A/B; `attack_mission.rs:646`).
- **P4 — Renew is the primary lifetime mechanism.** `request_renew` (`spawnsystem.rs:98-106`) is used as cohesion glue (`attack_mission.rs:484-488/646`, `squad_defense.rs:175-195` at TTL<1200) — the IBEX-002 pathology ADR 0008 §2.3 replaces with pre-spawn. Renew also runs **before** the consume loop, so a renew-eligible creep consumes a spawn lane ahead of even a CRITICAL request (`:199-232` vs `:236`) — an un-prioritized inversion.
- **P5 — The §6.11(b) starvation wedge is real (and has an amplifier).** The `break`-on-unaffordable is *deliberate energy reservation* for the top request — correct in intent (see D7) — but it can wedge: e.g. all haulers dead while harvesters live → `source_mining` sizes its CRITICAL harvester at `energy_capacity_available()` (the clamp at `:393-397` applies only when `total_harvesting_creeps == 0`), extensions never refill (static miners drop to containers; haulers are the refillers — ADR 0007), the trickle floor recovers only to 300, and the affordable HIGH hauler behind the 12,900-cost harvester **never spawns**. The queue is rebuilt identically every tick — a steady-state deadlock, not a transient. *Amplifier (new observation):* the renew energy decrement `(body_cost * 2) / 5` (`spawnsystem.rs:221-223`) over-charges by a factor of `body.length` versus the engine's `ceil(1.2 × cost / 3 / size)` (`renew-creep.js:33`) — after one large-creep renew the executor believes the room is far poorer than it is and `break`s earlier than necessary. Conservative direction, but it widens the wedge.
- **P6 — No cross-room assist.** The token-to-many-rooms pattern only covers rooms a mission already lists in `home_room_datas`; there is no incubation (a mature room spawning a newborn colony's first creeps — gap G3, "the review's stated weakness: cannot convert an economic lead into territory") and no war-support spillover when a front-line room's lanes saturate or its spawns are destroyed.
- **P7 — Static priorities, latent NaN.** Five f32 constants + ad-hoc lerps (`spawnsystem.rs:16-20`, `upgrade.rs:317-329`); no deadline dimension; `partial_cmp(...).unwrap_or(Equal)` coalesces a future NaN silently (IBEX-046 — latent, guarded by convention only).

**Why now:** ADR 0008's Squad Manager (Increment 4) needs a spawn seam that can express "4 creeps, synchronized, pre-spawned, possibly cross-room" — bolting that onto the imperative per-tick queue would smear scheduling logic back into the manager. And the economic ceiling argument is independent: at RCL8 the binding resource *is* the 1.0 parts/tick lane, and a bot that doesn't budget it under-fields its economy ("spawn uptime" prior art, digest §5.1).

## Decision

Adopt a **two-layer spawn architecture**: a new global **`SpawnOrchestrator`** (the brain: declarative demands in, scheduled per-room `SpawnRequest`s out) feeding the existing **`spawnsystem.rs` executor** (the hands: per-room priority consume loop, kept verbatim minus two targeted fixes). Missions and the Squad Manager stop pushing imperative requests and instead **declare demands**; the orchestrator owns throughput accounting, room placement, group synchronization, pre-spawn timing, renew policy, and the boost handoff.

```
Producers                 economic missions · Squad Manager (ADR 0008, sole combat producer)
        │ declare (re-assert per tick)
        ▼
SpawnDemand store         { demand_id, owner, body_spec, desired/current, deadline,
        │                   room_affinity, group, prespawn, renewable, boosts }
        ▼
SpawnOrchestrator (NEW)   global, once/tick: throughput+energy budget model →
        │                 placement (which room) → schedule (when/which lane) →
        │                 group sync (align-finish) → renew/boost/preemption policy
        ▼ emits SpawnRequests (priority-encoded) + renew requests
spawnsystem.rs executor   UNCHANGED consume loop: descending priority, capacity-skip,
        │                 energy-reserve break, token dedup, callbacks
        ▼
WaitForSpawnSystem        creep.rs:37-69 — unchanged registration on emergence
```

### D1 — Declarative demands, ephemeral with re-assertion (the G4 wishlist seam)

`SpawnDemand` is data, not an action:

```rust
SpawnDemand {
    demand_id: DemandId,            // stable, owner-derived (ADR 0001 discipline: minted id, never an Entity index)
    owner: OwnerId,                 // mission/manager id for callbacks + attribution
    body: BodySpec,                 // SpawnBodyDefinition params or BodyType (composition.rs:41) — sized BY THE ORCHESTRATOR at schedule time
    desired: u32, current: u32,     // owner-counted roster incl. spawning+in-flight (the wishlist diff)
    class: DemandClass,             // Defense | Bootstrap | Economy | Military | Expansion | Utility
    deadline: Option<u32>,          // absolute tick by which the creep must EMERGE (EDF input)
    room_affinity: Affinity,        // preferred room(s) + target position (travel-time input)
    group: Option<GroupId>,         // synchronized-emergence group (SquadId-derived for combat)
    prespawn: Option<Prespawn>,     // incumbent TTLs → successor timing (D4)
    renewable: bool,                // hint only; orchestrator decides (D8)
    boosts: Vec<(ResourceType,u32)> // from composition.required_boosts() (composition.rs:552) → ADR 0010 handoff (D9)
}
```

**The store is ephemeral and rebuilt by re-assertion every tick** — the same reset-proof contract as today's queue and the transfer queue (ADR 0007): owners re-declare their demands each tick (they already recompute shortfall each tick today, so this is a shape change, not a cost change), and `current` is owner-counted from their serialized rosters plus `CreepSpawning` entities. **No new persisted state** — in-flight spawns are recoverable from game state (`spawn.spawning()` + the `WaitForSpawnSystem` name registry, `creep.rs:43-62`), so a VM reset reconstructs the world without a new segment (ADR 0002 discipline; breaking label **None** for format). What changes vs. today is the *information content*: deadlines, groups, TTL schedules, and affinities make future demand visible, which is what P1 blocks.

**Explicitly declined:** Overmind's idle-creep reassignment (`recalculateCreeps`). Ibex jobs are statically assigned at spawn (`JobData` per creep, ADR 0003) and the competitive analysis already weighed this (§1 job row); the reconciler nets out live+spawning creeps but never re-roles them. Revisit only if the harness shows material idle waste.

### D2 — The joint budget model (throughput × energy)

Once per tick the orchestrator computes, per owned room (extending `RoomEconomyData`, `economy.rs:27-51` — replacing the dead `military_spawns_claimed` field with used accounting):

- **Lane state:** per-spawn busy-until tick (from `spawn.spawning().{needTime,remainingTime}`), idle lanes, parts/tick capacity (= `spawn_count / 3`).
- **Committed load:** part-ticks of already-spawning bodies + part-ticks of demands already placed in this room this tick.
- **Backlog:** Σ part-ticks of unscheduled demands affine to this room, vs. capacity — the **spawn-pressure ratio** (backlog ÷ capacity), the metric that decides cross-room spill (D5) and feeds the seg-57 spawn-uptime KPI (ADR 0006).
- **Energy:** `energy_available`/`capacity` (already there), refill trend (Δ`energy_available` over a short window — the D7 stall detector), stored surplus above the military reserve (`can_rooms_afford_military`, `economy.rs:91-101`; IBEX-034's RCL-scaling fix lands in that function, not here).

Travel estimates use `RoomRouteCache::travel_ticks` (`economy.rs:263-271`, hops×50) **through the ADR 0004 budgeted facade** — never a fresh `pathfinder::search` (IBEX-035's lesson). The orchestrator reads the `CpuGovernor`: under Conserve/Critical it freezes *re-planning* (placement scoring, group re-scheduling) and only emits already-scheduled requests — spawning itself is pinned **always-on** by ADR 0004's shed order.

### D3 — Synchronized group spawning (align-finish)

For a demand set sharing a `GroupId` (a quad's 4 slots; ADR 0008 §2.2's reconciler emits them; Overmind `swarmWishlist` G2(d)):

1. **Admission:** schedule the group only when the chosen room set has (a) enough lanes to complete all members within the **emergence window W** (default ~25 ticks, ≥ slip tolerance for blocked exits), and (b) projected energy for the whole group (engine: same-tick multi-spawn draws are sequential against one pool — all succeed iff the pool covers the sum, `_charge-energy.js`). Until admission the group consumes **zero** lanes — no half-spawned quads idling at rally (the P3 fix).
2. **Align finish, not start:** with lane free-times `f_j` and member spawn times `s_i = 3 × parts_i`, assign longest-first to earliest-free lanes (the simulation already in `composition.rs:486-509`, promoted from estimator to scheduler) and **delay shorter members' starts** so completions align: `start_i = T_target − s_i`. All members emerge within W at full 1500 TTL — the squad deploys together instead of the first member aging while the last spawns.
3. **Cross-room groups:** members may be split across rooms only if `|travel_i − travel_j|`-adjusted arrival times at the rally point stay within W (reusing `is_viable_from`'s spawn+travel model, `composition.rs:521-549`). Otherwise prefer one room and accept later `T_target`.
4. **Failure:** if a member's spawn fails (room lost, energy collapse), the group reverts to unadmitted; the Squad Manager sees `Forming` stall and applies its own deadline policy (ADR 0008 §2.4) — the orchestrator never silently fields a partial group.

Concretely: today a 4×40-part quad on one lane = 480 ticks of staggered emergence; on an RCL8 room's 3 lanes with align-finish it is ≤ 240 ticks with **zero** stagger — and the orchestrator can see that and prefer the 3-spawn room.

### D4 — Pre-spawn replacement (the lifetime answer; ties ADR 0008 §2.3)

For demands with `prespawn` (incumbent TTLs supplied): schedule the successor so it **emerges** at `incumbent_death − margin` and **arrives** as the incumbent expires:

`start_by = death_tick − (3 × parts(successor) + travel_ticks(spawn_room → post) + PRESPAWN_MARGIN)` — margin default 25–50 ticks (Overmind `DEFAULT_PRESPAWN = 50`), travel from the budgeted route cache. This replaces renew as the default lifetime mechanism for **both** combat (ADR 0008 §2.3 — the manager supplies the TTLs) and economy (static miners anchored to containers get a seamless handover instead of a harvest gap; haulers likewise — closing the IBEX-047 reactive-replacement pattern). Pre-spawn demands are ordinary demands with an *earliest-useful* and *latest-useful* start; the scheduler treats `start_by` as the deadline. Double-spawn protection: the successor is attributed to the incumbent's slot (`demand_id` + slot), so `current` counts it and the reconciler never emits two.

### D5 — Cross-room assist (placement is the orchestrator's, not the mission's)

Placement of every demand scores candidate rooms by: **spawn-pressure headroom × energy surplus × travel viability** (route-distance via the facade; residual-TTL floor from `is_viable_from` — a remote-spawned creep that arrives at 40% TTL is usually worse than waiting for a local lane; CLAIM bodies at 600 life are the extreme case and effectively must spawn near-target). This generalizes — and makes deterministic — the accidental token-to-many-rooms pattern (P6). Three assist classes, in priority order:

1. **Defense spillover:** a room under attack whose lanes/energy can't field its defense demand before the projected breach spills to the nearest room by *route* distance (never linear — IBEX-032 discipline), accepting the travel delay only when it still beats local feasibility.
2. **War support:** a front-line room hosting a campaign exports its economic demands (haulers/upgraders) to neighbors so its own lanes are free for combat bodies — spawn-pressure arbitrage, the Winsley budget idea applied across rooms.
3. **Incubation (gap G3, Increment 7):** a newborn colony (claimed, RCL1–3) registers `Bootstrap`-class demands with affinity to itself; mature rooms within viable range fulfill them (spawn there, walk over) until the newborn's own capacity (lanes × energy) crosses self-sufficiency. This is the *spawn half* of incubation; the *energy half* (terminal/hauler feeding) stays with the Colony operation design and ADR 0007.

### D6 — Preemption policy: queue-jump yes, mid-spawn cancel (almost) never

Engine facts decide this: `cancelSpawning` refunds **nothing** (`cancel-spawning.js:6-13`) and the canceled creep is simply deleted — preempting a mid-spawn body burns its full energy cost and the part-ticks already invested. Therefore:

- **Default: no cancellation.** Defense demands preempt by **priority queue-jump only**: they take the next free lane (worst case wait = remaining `needTime` of the longest in-flight body, ≤150 ticks; typically far less with 2–3 lanes) plus D5 spillover to a neighbor, which is usually faster *and* free.
- **Single cancel exception:** predicted breach (threat classification, `threatmap.rs`, with IBEX-033's over-trigger fixed) **before** any lane frees up **and** no viable spillover **and** the victim is the lane's lowest-value non-defense body with the most remaining spawn time. The orchestrator logs the sunk cost; the harness asserts this path stays rare (a counter, not a code path exercised in normal play).
- Renew never blocks defense: D8 moves renew behind the priority gate, fixing P4's inversion.

### D7 — Energy-reservation semantics: keep reserve-for-top; cure starvation on the demand side (the §6.11(b) decision)

**Decision: the executor's `break`-on-unaffordable (`spawnsystem.rs:247-249`) is kept verbatim.** Evidence for keeping it: the alternative (opportunistic fill — `continue` past unaffordable requests) re-introduces a strict-priority inversion with an *unbounded* failure mode — a steady stream of cheap LOW bodies can hold `energy_available` below a CRITICAL body's cost indefinitely, so the room's most important creep **never** spawns while its least important ones always do. Reservation makes the top request's wait bounded by refill rate; opportunistic fill makes it potentially infinite. The engine's trickle floor (`spawns\tick.js:44-47`) plus the refill loop (haulers, ADR 0007) guarantee reservation normally clears.

The genuine §6.11(b) wedge (P5) is a **demand-sizing** problem, not an ordering problem — the top request is sized at `energy_capacity_available()` while the refill machinery it is waiting on is dead. So the cure lives in the orchestrator, which now owns sizing (D1):

- **Refill-stall rule (generalizing the IBEX-022 clamp, which stays):** if the top demand of a room has been energy-blocked for T ticks (default ~20) **and** the refill trend (D2) is non-positive, re-size that demand to `energy_available().max(SPAWN_ENERGY_CAPACITY)` — the same remedy `source_mining.rs:393-397` / `haul.rs` / `localbuild.rs` already apply on the empty-fleet path, extended from "my fleet is empty" to "the room's refill is stalled". A small harvester/hauler now beats a never-spawning big one; the room bootstraps back to capacity sizing automatically once refill resumes.
- **Bootstrap class is always affordable-sized:** `DemandClass::Bootstrap` (minimal miner + minimal hauler per room) is sized to `energy_available` *by definition*, so the recovery pair can never sit behind a reservation.
- **Fix the renew over-charge amplifier:** `spawnsystem.rs:222` becomes the engine-true `ceil(1.2 × body_cost / 3 / body.len())` (`renew-creep.js:33`) so a renew no longer fakes a large energy deficit into the consume loop. (Quick-win; see migration Step 0.)
- Add `debug_assert!(priority.is_finite())` at the request seam (IBEX-046) and the descending-order unit test the review asks for (review §4) — guarding the comparator against ever being "fixed" into an inversion.

### D8 — Renew policy: a narrow, orchestrator-owned optimization

Engine math defines exactly when renew beats replace: renew yields `floor(600/size)` TTL/intent at the *same energy per TTL* as spawning ÷1.2 — i.e. **~1.2× better spawn-time efficiency** (≈600 part-ticks of life restored per tick of spawn attention vs. 500 for spawning), at the price of: no boost survival (`renew-creep.js:45-53`), no CLAIM, the creep must stand adjacent, and a spawn lane is occupied per intent. Per-creep yield collapses with size: a 10-part utility creep regains 60 TTL/intent; a 48-part quad member regains 12 — renewing big combat bodies is the Field Report B drain ADR 0008 §2.3 abolishes.

Therefore: **renew is legal only when (all of):** body ≤ ~15 parts, unboosted, non-CLAIM, the creep is already ≤1 from a spawn whose lane would otherwise be **idle this tick** (no scheduled demand wants it within the renew window), room energy above the renew reserve, and the owner declared `renewable: true`. In practice that is small static utility creeps parked at the hub (a queen/filler shape, link tenders) — and nothing else. Mechanically, `request_renew` moves behind the orchestrator: missions never call it directly; the executor's renew pass runs **after** priority dispatch decides lane allocation (fixing P4's renew-before-CRITICAL inversion at `spawnsystem.rs:199-232`). Combat renew requests (`attack_mission.rs:484-488/646`, `squad_defense.rs:175-195`) are deleted with the ADR 0008 Step-1/2 migration — pre-spawn replaces them.

### D9 — Boost-on-spawn handoff (with ADR 0010)

Engine correction first: the folkloric "boost while still spawning" does **not** hold — `boostCreep` requires the creep *not* be spawning (`labs\boost-creep.js:15-23`). The realizable version: when a demand with `boosts` is **scheduled** (spawn intent issued), the orchestrator emits a boost reservation to the ADR 0010 lab pipeline so compounds + energy (30 mineral + 20 energy **per part**, `constants.js:281-282`) are loaded into boost labs **during the 3×parts spawn window**; the spawn uses `directions` to emerge toward the lab cluster (ADR 0009's lab stamp adjacency), and the creep boosts on its first ticks of life — maximizing boosted TTL without the dead-time of post-hoc lab logistics. Group deploy gates on all members boosted (ADR 0008 Step 4 / IBEX-027's wire-or-delete: this is the "wire" half's spawn-side seam; if 0010 resolves to "delete", `boosts` stays an empty vec and nothing here changes). Unboost-for-refund is *not* planned into retirement: lab cooldown ≈675 ticks per XGHO2 part (`labs\unboost-creep.js:47`) makes it strategically near-one-shot — retirement uses plain recycle (refund into a container, `_die.js:39-94`).

### D10 — Priority mapping: class + earliest-deadline-first, encoded to the executor's f32

The executor's descending-f32 contract is preserved. The orchestrator maps `(DemandClass, deadline_slack)` onto bands: Defense > Bootstrap > Economy-critical (miner/hauler shortfall) > Military > Economy-growth (upgrader/builder) > Expansion > Utility — within a band, earlier deadline ⇒ higher value (EDF). The five static constants (`spawnsystem.rs:16-20`) survive as band anchors during migration so unmigrated missions interleave correctly with orchestrated demands (the strangler-fig property). This answers the review prompt's "is static prioritization expressive enough?" — *bands* are; the deadline dimension inside a band is what was missing (a HIGH due-in-300-ticks must not outrank a HIGH due-now).

## Spawn-system layering vs ADR 0008 (who owns what)

| Layer | Owns | Does NOT own |
|---|---|---|
| **Operations / missions** (econ) | desired counts, rosters, `renewable` hints, deadlines | bodies' final sizing, room placement, renew calls, tokens |
| **Squad Manager** (ADR 0008) | objective→composition, roster slots, member TTLs, group membership (`GroupId` = `SquadId`-derived), deploy gating | scheduling, lane assignment, emergence sync mechanics, boost-lab timing |
| **SpawnOrchestrator** (this ADR) | demand store, throughput+energy budgets, placement, align-finish group scheduling, pre-spawn timing, renew/preemption/boost policy, priority encoding | combat strategy, creep behavior, lab chemistry (0010), CPU tiers (0004) |
| **`spawnsystem.rs` executor** | per-room descending-priority consume loop, token dedup, callbacks, the actual `spawn_creep`/`renew_creep` intents | everything else — it stays the dumb fulfiller ADR 0008 §2 already promised |

**Kept, not replaced.** The executor survives because it is verified-correct (the review's false-positive guard), small, and its semantics (capacity-skip / energy-reserve / token dedup) are exactly the right *room-local* contract; the orchestrator simply becomes its only producer. ADR 0008 §2.2's "the manager emits spawn requests to `spawn_queue` (the existing API, unchanged)" is **specialized**, not contradicted: during Increment 4 the manager targets the demand seam, which *is* the existing API plus the demand envelope — the manager's reconciler emits `SpawnDemand`s and the orchestrator translates them to `SpawnRequest`s. (0008 Step 1's "single producer of combat spawn requests" becomes "single producer of combat *demands*".)

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Global orchestrator over the kept per-room executor; declarative re-asserted demands** (chosen) | One owner for throughput/groups/pre-spawn/assist; executor's verified semantics preserved; no new persisted state; strangler-fig friendly (bands interleave with legacy requests) | New system; owners must be migrated producer-by-producer; placement scoring is a new tunable surface |
| **Status quo + spot fixes** (renew accounting, starvation clamp, per-mission group hacks) | Smallest diff | P1–P3 structurally unrepresentable (no future demand, no groups, no budget); every mission keeps re-implementing sizing/priorities; ADR 0008's manager would have to grow its own scheduler |
| **Replace `spawnsystem.rs` wholesale** with the orchestrator emitting intents directly | One layer | Discards the verified-correct executor (review §1 guard: the real risk is "fixing" it); larger blast radius; loses the clean per-room seam that lets legacy missions coexist mid-migration |
| **Per-mission wishlist reconcilers** (Overmind-literal: each mission owns its own `wishlist`) | Closest to prior art; no global component | Spawn-time is a *global* resource — per-mission reconcilers cannot see cross-room headroom, group lanes, or empire pressure; re-smears P2 |
| **Persisted demand store** (segment-backed queue with cross-tick scheduling state) | Scheduler state survives resets exactly | New Memory/format surface on the brittle substrate ADR 0002 is still hardening; unnecessary — demands are cheap to re-assert and in-flight spawns are recoverable from game state (same verdict as ADR 0007's "persist the matcher state" rejection) |
| **Auction / price-based lane allocation** (missions bid energy for spawn-time) | Elegant arbitration | Over-engineered; non-deterministic emergent priorities are hostile to replay-diff validation (ADR 0006); bands+EDF is sufficient and testable |
| **Renew-centric lifetime** (maximize renew, minimize spawning) | Saves replacement walk; 1.2× spawn-time ratio | Engine math kills it for the bodies that matter (12 TTL/intent at 48 parts), strips boosts, blocks CLAIM, and it is the documented Field Report B hang mechanism — D8 confines it to the narrow case where it genuinely wins |

## Consequences

**Positive**
- **Spawn-time becomes a measured, scheduled budget.** Parts/tick capacity, committed load, backlog, and spawn-uptime land in seg-57 (ADR 0006); "are we spawn-blocked?" becomes a dashboard read, and the colony-health ECONOMIC term gains the KPI prior art says predicts maturity (digest §5.1).
- **Groups emerge together at full TTL** (P3 closed): align-finish + admission gating remove the staggered-quad rally hang upstream of ADR 0003's movement cohesion and ADR 0008's lifecycle deadlines — Field Report A attacked at its *third* root (spawn stagger, after movement and identity).
- **Pre-spawn-over-renew lands for the whole bot** (P1/P4 closed; G4): combat per ADR 0008 §2.3, economy seamless-handover miners/haulers; the renew-before-CRITICAL inversion and the renew energy over-charge are gone; renew survives only where the engine math says it wins.
- **The §6.11(b) question is answered with evidence** (P5): reservation kept (priority-inversion-proof), starvation cured by refill-stall demand re-sizing + always-affordable Bootstrap class — and the wedge scenario becomes a harness regression test.
- **Cross-room assist exists** (P6): defense spillover, war support, and the G3 incubation seam — the "convert economic lead into territory" capability the competitive verdict demands.
- **Demand ids + pure scheduling logic are host-target testable** (ADR 0006): the budget model, align-finish assignment, refill-stall rule, and renew predicate are pure functions over snapshots — kernel tests, no game API.

**Negative / new risks**
- **A new global system with tunables** (window W, prespawn margin, stall T, placement weights). Mitigated: every tunable in config, validated against the colony-health score; the orchestrator is pure over `EconomySnapshot`+demands, so mis-tuning shows as a measurable regression, not a mystery.
- **Migration is producer-by-producer** — until complete, two demand styles coexist. Mitigated by D10's band anchoring (legacy `SPAWN_PRIORITY_*` requests interleave correctly) and per-step replay/intent-diff parity (ADR 0006).
- **Placement scoring could thrash** (demands bouncing between rooms tick-to-tick as headroom shifts). Mitigated: placement is sticky per `demand_id` once scheduled (committed-plan discipline, same shape as ADR 0007's committed-delivery guard); re-placement only on room loss or admission failure.
- **Group admission can over-wait** (holding a quad for a 3-lane window while the war needs *something* now). The Squad Manager's deploy policy owns that trade (it can split the force requirement into smaller groups); the orchestrator exposes projected `T_target` so the manager decides with data.

**CPU & tick-safety**
- The orchestrator is O(demands × candidate rooms) — tens × small — once per tick, no pathfinding of its own (travel estimates are cache reads through the 0004 facade; cold misses are charged there). It emits spawn/renew intents only via the executor (~0.2 CPU each, few per tick). Under Conserve/Critical it freezes re-planning and replays the committed schedule; **spawning itself is never shed** (ADR 0004's always-on list). Net intent count *falls*: the perpetual combat-renew drain (Field Report B) is removed.
- No new panic surface: demand resolution is id-keyed lookup-miss-handled (ADR 0001 discipline), the executor's error handling is unchanged, and the whole pass sits inside ADR 0005's tick containment.
- VM-reset: zero persisted additions; first post-reset tick re-asserts demands and reconstructs lane state from `spawn.spawning()` — degraded only in that uncommitted group schedules are recomputed (cheap, bounded).

## Incremental Migration Path

Stable seams: the **`SpawnQueue::request` executor API** (frozen — legacy missions keep using it mid-migration) and the new **`SpawnDemand` registration seam** (the wishlist boundary). Each step harness-validated (ADR 0006) before the next; never break the running bot mid-increment. Placement follows the plan's §3 mapping: quick-wins ride Increment 1, the demand seam + groups + combat pre-spawn ride Increment 4 (with ADR 0008's Squad Manager), economic migration rides Increments 5–6 opportunistically, and the full global layer is **Increment 7** capability work.

1. **Step 0 — Executor quick-wins (Increment 1 vicinity). Breaking: None.**
   Fix the renew energy decrement to engine-true `ceil(1.2 × cost / 3 / len)` (`spawnsystem.rs:222` vs `renew-creep.js:33`); move the renew pass after a priority check so renew never consumes a lane a CRITICAL/HIGH request wants this tick (`:199-232`); add `debug_assert!(priority.is_finite())` (IBEX-046) and the descending-order unit test + clarifying comment (review §4 — the anti-re-flag guard). **Validate:** ordering test green; harness scenario "large-creep renew + queued CRITICAL" — the CRITICAL spawns the same tick.
2. **Step 1 — Demand seam + orchestrator skeleton, Squad Manager as first producer (Increment 4, co-staged with ADR 0008 Step 1; gated on ADR 0001's stable ids). Breaking: Behavioral.**
   Introduce `SpawnDemand` + the orchestrator translating demands → `SpawnRequest`s (initially: same placement and priorities a mission would have produced — parity mode). ADR 0008's Squad Manager registers demands instead of raw requests. **Validate:** replay intent-diff parity (same spawn stream as the legacy path on a recorded engagement), then enable D2 accounting and assert seg-57 spawn-pressure metrics populate.
3. **Step 2 — Group spawning + combat pre-spawn (Increment 4, with ADR 0008 Steps 1–2). Breaking: Behavioral.**
   Enable `GroupId` align-finish scheduling and `prespawn` for squad demands; delete combat `request_renew` call sites with the 0008 migration. **Validate:** harness engagement — all quad members emerge within W (assert via emergence-tick spread), squad cohesion-rate metric improves vs. baseline; kill a member mid-siege — successor emerges before incumbent death − margin; assert zero combat renew intents.
4. **Step 3 — Starvation cure + bootstrap class (Increment 4/5, independent of combat steps). Breaking: Behavioral.**
   Refill-stall re-sizing + `Bootstrap` always-affordable sizing in the orchestrator (legacy IBEX-022 clamps stay until each mission migrates). **Validate (the §6.11(b) regression test):** scenario "all haulers die at full extensions-empty" — assert the room spawns a minimal hauler within T+spawn-time ticks and recovers to capacity-sized bodies; assert a CRITICAL big-body demand still pre-empts cheap LOW spam (reservation intact).
5. **Step 4 — Economic producers migrate; renew policy centralization; boost handoff (Increments 5–6, riding mission/FSM touches). Breaking: Behavioral.**
   Migrate `source_mining`/`haul`/`upgrade`/`localbuild`/`claim`/`scout`/`reserve` to demands (each is a mechanical shortfall→demand rewrite, one mission per change); `request_renew` becomes orchestrator-internal (D8 predicate); wire the D9 boost reservation when ADR 0010's Increment-7 pipeline lands (no-op until then). **Validate per mission:** replay parity on the economy-bringup scenario; pre-spawn handover — zero-gap container-miner replacement (energy-throughput non-regression); renew intents only for ≤15-part adjacent utility creeps at idle lanes.
6. **Step 5 — Full global layer: placement scoring, cross-room assist, incubation, preemption (Increment 7). Breaking: Behavioral.**
   Enable D5 scoring (replacing token-to-many-rooms placement-by-HashSet-order), defense spillover, war support, and the G3 incubation class; enable the D6 cancel exception behind its counters. **Validate:** newborn-colony scenario — incubated room reaches self-sufficiency ≥30% faster than un-assisted baseline with no donor spawn-uptime regression below threshold; siege scenario — defense demand fulfilled within bound via spillover with zero cancels in the normal case; assert deterministic placement (same snapshot ⇒ same room) for replayability.

**Breaking-change summary:** Step 0 — **None**. Steps 1–5 — **Behavioral** only (when/where creeps spawn changes; no serialized field is added, removed, or reordered — the demand store is ephemeral by design, and the only Memory/format-adjacent item is the seg-57 metric *addition*, already labelled in ADR 0006). No state drop is introduced by this pillar; legacy `SpawnRequest` producers keep working at every step via the frozen executor seam, so the running bot never breaks mid-increment.
