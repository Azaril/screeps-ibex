# Recon: room economy state, stress/bootstrap awareness, spawn/energy prioritization (screeps-ibex)

## Q1 — Room-level energy-health state today

**The only consolidated per-room economy state is `EconomySnapshot` / `RoomEconomyData`** (`screeps-ibex/src/military/economy.rs:27-51`), rebuilt every tick by `EconomyAssessmentSystem` (`economy.rs:154-239`), which runs in the pre-pass "Always" stage before missions (`game_loop.rs:82`). Fields per owned room:
- `stored_energy` = storage + terminal + ALL containers energy (`economy.rs:187-200`)
- `energy_income` = **static potential only**: `sources.len() × 3000/300 = 10 e/t per source` (`economy.rs:210-213`) — actual harvested income is NOT measured anywhere
- `spawn_energy` = `room.energy_available()`, `spawn_energy_capacity` = `room.energy_capacity_available()` (`economy.rs:220-221`)
- `spawn_count` / `free_spawns` (idle spawns, `economy.rs:201-206`)
- `prev_tick_queue_depth` — spawn queue depth snapshotted one tick stale by `SpawnQueueSystem` before clearing (`spawnsystem.rs:560-566`, struct `SpawnQueueSnapshot` at `economy.rs:14-19`)
- `military_spawns_claimed` (within-tick cooperative counter), `available_boosts` (**never populated** — hardcoded `HashMap::new()` at `economy.rs:226`)

**Serialization:** `EconomySnapshot` is explicitly ephemeral — "Rebuilt each tick (ephemeral -- not serialized)" (`economy.rs:58`). The serialized per-room state is `RoomData`'s ConvertSaveload: only `name`, `missions`, `static_visibility_data`, `dynamic_visibility_data` (`room/data.rs:407-422`). Static = controller/sources/minerals/terrain stats/exits/keeper lairs (`data.rs:72-93`); dynamic = owner/reservation/hostility flags/tower-dps/controller level+ttd/derelict clock (`data.rs:192-251`). **No energy-health field is serialized anywhere** — structure stores are read live via per-tick `RefCell` caches (`data.rs:400-404`, `get_structures()` at `data.rs:701-710`). So a new economy-health resource can stay ephemeral with **no WORLD_FORMAT_VERSION bump**; anything added to `RoomData`/mission saveload shapes requires bumping `WORLD_FORMAT_VERSION` (currently **23**, `game_loop.rs:679`).

**Ad-hoc, duplicated energy-health checks scattered in missions** (each with its own thresholds off the single constant `get_desired_storage_amount(Energy) = 200_000`, `missions/constants.rs:3-8`):
- LocalBuild `has_sufficient_energy` = any storage ≥ 50k (200k/4) OR (no storage AND any container >50%) (`localbuild.rs:225-239`)
- Upgrade `has_excess_energy` = storage ≥ 100k (200k/2) OR container >75%, **defaults TRUE when no storage AND no containers** (`upgrade.rs:183-200`)
- Repair's `available_energy` = sum of storage energy only (`localbuild.rs:193-197`, `jobs/utility/repair.rs:180-184`)
- HaulMission demand proxy: `unfufilled_hauling` from `TransferQueue::total_unfufilled_resources`, cached 20 ticks (`haul.rs:100-117,193-196`)

`room_economics.rs` (`room_net_roi`) and `claim_economics.rs` are **pure design-time valuation kernels** for war/claim target scoring (ADR 0032/0038) — they never read live room energy and play no role in runtime energy management.

Telemetry only: `metrics.rs:329-351` already exports per-room `rcl`, `rcl_progress`, `energy_available`, `energy_capacity_available`, `stored_energy` (+ GCL at 370-373) — useful reference shape for the requested bootstrap-recovery metric, but nothing consumes it for decisions.

## Q2 — Existing bootstrap/emergency/recovery mode

**There is NO explicit mode.** `grep bootstrap|emergency|starv` finds only: the acknowledged gap `//TODO: Make sure there is handling for starvation/bootstrap mode.` (`missions/haul.rs:299`), safe-mode combat emergency (`missions/safe_mode.rs:17`), and the upgrade-job comment "downgrade emergencies and room recovery scenarios" (`jobs/upgrade.rs:62`). What exists is a set of **implicit, per-mission fallbacks**:

1. **Harvester fallback** (SourceMiningMission, the de-facto bootstrap): spawns up to 4 mobile harvesters per source when `(no containers && no links) || (local && total_harvesting_creeps == 0) || (remote && no home storage)` (`source_mining.rs:371-373`). The FIRST harvester's body is sized from `energy_available().max(SPAWN_ENERGY_CAPACITY=300)` instead of capacity (`source_mining.rs:394-398`); local priority lerps CRITICAL(100)→HIGH(75) by count (`source_mining.rs:401-410`). Desired count is hardcoded 4 (`//TODO: Compute correct number`, `source_mining.rs:390-391`).
2. **First-builder fallback**: body from `energy_available().max(300)` when `builders.is_empty() && priority >= HIGH` (`localbuild.rs:255-259`); builders get `allow_harvest = room.storage().is_none()` (`localbuild.rs:273`).
3. **Downgrade-risk upgrader**: when controller ttd < max/2 and no upgraders → `SPAWN_PRIORITY_CRITICAL` + `energy_available()`-sized body, WORK parts solved to restore the timer in one lifetime (`upgrade.rs:209-222, 259-263, 292-296, 319-322`). Slow upgraders may harvest directly only when the room has no storage AND no containers (`jobs/upgrade.rs:59-73`).
4. **First-hauler fallback**: `energy_available().max(300)` when `haulers.is_empty()` (`haul.rs:229-237`).
5. **Renew gate**: creep renewal only when room `stored_energy >= 10_000` (`spawnsystem.rs:20, 407-412`) — an implicit "don't renew when poor".

**Cold-start / post-wipe behavior:** ColonyMission unconditionally re-creates Construction/LocalSupply/LocalBuild/Haul(/Tower/Upgrade...) children (`colony.rs:229-346`); each SourceMiningMission fires a CRITICAL small harvester from available energy — that part works. The failure mode: every **replacement** body (harvester #2+, miners, haulers, builders) is sized from `energy_capacity_available()` (`source_mining.rs:397, 452, 488`; `localbuild.rs:258`; `haul.rs:235`), which after a wipe still counts all standing (empty) extensions — producing expensive requests that head-of-line-block the queue (Q3) while income is a trickle, and the little energy in flight leaks into road repair (Q4).

## Q3 — Spawn queue prioritization and mission modulation

**Priorities are f32 constants, not an enum** (`spawnsystem.rs:22-39`): `CRITICAL=100` (local first harvesters, downgrade-risk upgrader), `COMBAT_FORMING=85` (forming squads only), `HIGH=75` (miners, link/container miners, spawn/storage-site builders, local haulers under-quota, first upgrader), `MEDIUM=50`, `LOW=25`, `NONE=0`. Missions interpolate between bands with `lerp_bounded` by alive-count fraction (e.g. `source_mining.rs:409-410`, `upgrade.rs:325-332`).

**Mechanics** (`spawnsystem.rs`):
- Requests are per-room, inserted sorted **descending** by priority (`SpawnQueue::request`, `spawnsystem.rs:212-225`; deliberately-reversed comparator pinned by test at 594 — refuted review seed, do not "fix").
- The queue is **cleared every tick** (`spawnsystem.rs:568`); missions re-request each tick from scratch. No persistence, no aging.
- `process_room_spawns` walks descending: `body_cost > energy_capacity` → `continue` (skip forever-unaffordable); `body_cost > available_energy` (but ≤ capacity) → **`break`** — the head request "banks" the room's energy and blocks every lower-priority request that tick (`spawnsystem.rs:428-436`). This head-of-line energy-banking is load-bearing (the whole `COMBAT_FORMING` band exists to exploit/avoid it, comment at `spawnsystem.rs:23-35`).
- Renew pass runs strictly after spawn requests (`spawnsystem.rs:477-513`).

**Do missions modulate by room energy state?** Only coarsely and locally: body size switches between `energy_available()` (first creep / emergencies) and `energy_capacity_available()` (replacements); upgrader count 1→5 via `has_excess_energy` + CPU governor + hostiles (`upgrade.rs:227-241`); builder count via `has_sufficient_energy` (`localbuild.rs:86`). **Nothing consumes `EconomySnapshot` for civilian spawn decisions** — its only consumers are the renew gate (`spawnsystem.rs:407-412`) and military affordability (`can_afford_military`, `economy.rs:82-101`).

**Tiny-income behavior:** all missions still enqueue every tick (cheap — the queue is rebuilt anyway, so no literal thrash), but each independently requests capacity-sized bodies. The top capacity-sized request (e.g. a HIGH 700-energy miner in a room with 1800 capacity and 300 available) banks energy and stalls the ladder below it; there is no demand-shedding, no global "we are poor, everyone request minimum bodies" signal. `prev_tick_queue_depth` is recorded but never acted on.

## Q4 — Upgrade/build/repair energy consumption; is there an arbiter?

**No global energy budget arbiter exists.** Two per-tick queues arbitrate *logistics ordering*, not budgets:
- **TransferQueue** (`transfer/transfersystem.rs`): `TransferPriority { High=0, Medium, Low, None }` (`transfersystem.rs:16-23`). Deposit priorities registered by RoomTransferMission: **spawns HIGH** (`room_transfer.rs:428-441`), **extensions HIGH** (`room_transfer.rs:447-460`), towers HIGH-when-hostiles else LOW (`tower.rs:176-180`), storage/most containers `None`, controller link escalating High→None by fill (`room_transfer.rs:56-93`). So extension/spawn refill already wins the *haul* lane on priority.
- **RepairQueue**: populated in LocalBuild's `pre_run_mission` for all non-wall structures (`localbuild.rs:181-216`) with `map_structure_repair_priority` (`jobs/utility/repair.rs:93-110`): roads use `map_normal_priority` — **<25% health = High, <50% = Medium, <75% = Low, else VeryLow** (`repair.rs:23-37`). Energy-budget gating exists ONLY for walls/ramparts (`available_energy > 10k` for VeryLow, `repair.rs:84`).

**Consumption is independent pull, uncoordinated:**
- Upgrade job Idle: picks up energy at **ALL transfer priorities** from HAUL|USE lanes (`jobs/upgrade.rs:112-122`) — competes with refill logistics for the same withdrawable stock; upgrader WORK sizing when not excess = half of theoretical source income (`upgrade.rs:282-289`), 20 WORK each when "excess" (`upgrade.rs:279-280`).
- Build job Idle: **repair(≥High) → build → repair(ANY priority incl. VeryLow) → pickup(ALL priorities) → harvest** (`jobs/build.rs:61-102`) — an idle builder becomes a full-time road/structure repairer with no floor.
- **The operator-observed road-repair leak, precisely:** (a) harvesters run `tick_opportunistic_repair(≥Medium)` every tick while harvesting AND while delivering (`jobs/harvest.rs:225, 264`) — drive-by repairing any road <50% within range 3 with the energy they are carrying toward spawns/extensions (energy deducted from the delivery at `harvest.rs:264-266`); (b) Harvest Idle enters a dedicated `Repair` state at ≥Medium **before** Medium/Low/None delivery (only HIGH delivery outranks it) (`jobs/harvest.rs:165-202`); (c) roads <25% map to High → `get_repairer_priority` requests a repairer-builder at **SPAWN_PRIORITY_HIGH**, tied with haulers (`localbuild.rs:112-122`); (d) multi-room haulers (`allow_repair = max_distance > 0`, `haul.rs:295`) opportunistically repair at ≥Low (`jobs/haul.rs:229-247`); static miners at ≥Low (`jobs/staticmine.rs:144`). After a collapse (decayed roads everywhere), Medium/High road-repair targets are abundant, so bootstrap energy bleeds into roads at exactly the moment extensions need it — there is no stress gate on any of these paths.

## Q5 — What "collapse" looks like in current state terms; detection signals

No detector exists; collapse is emergent. **Signals already available (all in `RoomEconomyData`, one place):** `free_spawns == spawn_count` persisting (idle spawns), `spawn_energy` ≪ `spawn_energy_capacity` persisting (empty extensions), `stored_energy ≈ 0` (< the 10k renew bar), `prev_tick_queue_depth > 0` persisting (unmet spawn demand) — computed at `economy.rs:187-227`. Complementary signals not centralized: per-mission alive-creep counts (mission `EntityVec`s), controller ttd < max/2 (`upgrade.rs:209-220`), unfulfilled-haul volume (`haul.rs:109-116`), dropped/tombstone energy (`data.rs:1074-1096`). Missing entirely: actual (not potential) energy income, energy spent by sink (repair vs refill vs upgrade), time-to-refill. Since `EconomySnapshot` is ephemeral, a stress/bootstrap classifier over these fields needs no WFV bump unless persisted.

## Offline sim gap (context for the operator's ask)

`screeps-sim-core` contains only movement/combat primitives — `body/constants/intents/movement/rng/sim/terrain/tick/world` (`screeps-sim-core/src/lib.rs:14-28`), with **no spawn, no sources/minerals, no energy, no labs** (world.rs/tick.rs have zero energy/harvest references). `screeps-rover-eval` is movement/haul-efficiency benching. The requested sim spawn/economy/lab/source-mineral systems and a bootstrap-recovery/RCL-rush metric do not exist anywhere in the workspace; the closest analytic model is the pure `room_net_roi` kernel (`room_economics.rs:174-202`) and the body/cost mirrors in `localsupply/body_helpers.rs` (travel/lead-time math at 109-138, `source_work_parts` at 142-150), which are natural seeds for a sim's constants.

## Citations
- C:\code\screeps-ibex\screeps-ibex\src\military\economy.rs:27 — RoomEconomyData: the only consolidated per-room energy-health state (stored_energy, energy_income, spawn_energy, capacity, free_spawns, queue depth)
- C:\code\screeps-ibex\screeps-ibex\src\military\economy.rs:58 — EconomySnapshot explicitly ephemeral — rebuilt each tick, not serialized (no WFV impact)
- C:\code\screeps-ibex\screeps-ibex\src\military\economy.rs:210 — energy_income is a static potential estimate (sources × 3000/300); actual income never measured
- C:\code\screeps-ibex\screeps-ibex\src\military\economy.rs:82 — can_afford_military: 20% stored-energy reserve clamped 5k-30k — the only strategic energy budget check, military-only
- C:\code\screeps-ibex\screeps-ibex\src\room\data.rs:407 — RoomDataSaveloadData: serialized room state = name/missions/static/dynamic visibility only; no energy fields
- C:\code\screeps-ibex\screeps-ibex\src\game_loop.rs:679 — WORLD_FORMAT_VERSION = 23; bump required for any serialized-shape change
- C:\code\screeps-ibex\screeps-ibex\src\spawnsystem.rs:22 — Spawn priority f32 bands: CRITICAL 100 / COMBAT_FORMING 85 / HIGH 75 / MEDIUM 50 / LOW 25 / NONE 0
- C:\code\screeps-ibex\screeps-ibex\src\spawnsystem.rs:428 — Head-of-line energy banking: cost>capacity continue; cost>available break (blocks all lower requests)
- C:\code\screeps-ibex\screeps-ibex\src\spawnsystem.rs:568 — Spawn queue cleared every tick; missions re-request each tick
- C:\code\screeps-ibex\screeps-ibex\src\spawnsystem.rs:20 — RENEW_MIN_ROOM_ENERGY = 10_000: renewal gated on stored energy — implicit poverty behavior
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\source_mining.rs:371 — Implicit bootstrap: harvester fallback condition (no containers/links, or local with zero harvesting creeps)
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\source_mining.rs:394 — First harvester body sized from energy_available().max(300); replacements from energy_capacity_available()
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\source_mining.rs:402 — Local harvester priority lerps CRITICAL→HIGH by alive count
- C:\code\screeps-ibex\screeps-ibex\src\missions\haul.rs:299 — TODO acknowledging missing starvation/bootstrap handling in hauler spawning
- C:\code\screeps-ibex\screeps-ibex\src\missions\upgrade.rs:209 — Downgrade-risk detection (ttd < max/2) → CRITICAL upkeep upgrader sized to restore timer in one lifetime
- C:\code\screeps-ibex\screeps-ibex\src\missions\upgrade.rs:183 — has_excess_energy: storage ≥100k or container >75%, defaults TRUE with no storage/containers
- C:\code\screeps-ibex\screeps-ibex\src\missions\localbuild.rs:225 — has_sufficient_energy: storage ≥50k or container >50% — gates builder count only
- C:\code\screeps-ibex\screeps-ibex\src\missions\localbuild.rs:112 — get_repairer_priority: any ≥High repair target (road <25%) spawns a repairer-builder at SPAWN_PRIORITY_HIGH
- C:\code\screeps-ibex\screeps-ibex\src\missions\localbuild.rs:181 — LocalBuild pre_run populates the RepairQueue for all non-wall structures (roads included)
- C:\code\screeps-ibex\screeps-ibex\src\jobs\harvest.rs:264 — Harvester Delivery tick: opportunistic repair ≥Medium burns carried energy en route to spawn/extensions
- C:\code\screeps-ibex\screeps-ibex\src\jobs\harvest.rs:177 — Harvest Idle: dedicated Repair state at ≥Medium outranks Medium/Low/None delivery (only HIGH delivery first)
- C:\code\screeps-ibex\screeps-ibex\src\jobs\build.rs:61 — Build Idle order: repair(≥High) → build → repair(ANY priority) → pickup(ALL priorities) → harvest
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repair.rs:23 — map_normal_priority: roads <25%=High, <50%=Medium — abundant Medium targets after collapse
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repairbehavior.rs:111 — tick_opportunistic_repair: drive-by repair within range 3, returns energy consumed
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\room_transfer.rs:428 — Spawns and extensions register TransferPriority::High deposit requests (refill already wins the haul lane)
- C:\code\screeps-ibex\screeps-ibex\src\transfer\transfersystem.rs:18 — TransferPriority enum High/Medium/Low/None — logistics ordering, not an energy budget
- C:\code\screeps-ibex\screeps-ibex\src\jobs\upgrade.rs:112 — Upgrade job pulls energy at ALL transfer priorities from HAUL|USE — uncoordinated independent pull
- C:\code\screeps-ibex\screeps-ibex\src\missions\colony.rs:229 — ColonyMission Incubate: unconditionally re-creates the 9 child missions (cold-start path)
- C:\code\screeps-ibex\screeps-ibex\src\missions\constants.rs:3 — get_desired_storage_amount(Energy)=200k — the single constant all storage-fullness thresholds derive from
- C:\code\screeps-ibex\screeps-ibex\src\room_economics.rs:174 — room_net_roi: pure valuation kernel (war/claim scoring), not runtime energy management; seed constants for a sim
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\body_helpers.rs:109 — estimate_travel_ticks/miner_lead_ticks/source_work_parts — existing analytic body/economy math a sim can reuse
- C:\code\screeps-ibex\screeps-sim-core\src\lib.rs:14 — sim-core modules: movement/combat only — no spawn, source/mineral, energy, or lab systems exist
- C:\code\screeps-ibex\screeps-ibex\src\metrics.rs:346 — Telemetry already exports rcl/rcl_progress/energy_available/capacity/stored_energy per room (shape for recovery metric)

## Gaps
- Did not deep-read missions/labs.rs (742+ lines) beyond can_run — lab energy/mineral flows (input/output orders via terminal) not mapped; relevant to the sim-lab-system ask but outside the listed recon files.
- Did not trace the TransferQueue's internal value/distance ranking (transfersystem.rs is 2498 lines) — priority classes are mapped, but the tie-break economics (finite_transfer_value per distance) within a priority class were not verified in detail.
- Did not verify whether any operations-layer code (operations/*.rs) reads EconomySnapshot for civilian (non-military) purposes — grep showed consumers only in spawnsystem renew gate and military code, but operations were not exhaustively read.
- MiningOutpost allow_spawning gating (DefendMission is_room_safe) was confirmed to exist for outposts only; whether any path ever disables spawning for the home colony's own LocalSupply/Haul missions was checked only by absence in colony.rs (none found).