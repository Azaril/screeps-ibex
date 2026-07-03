# Recon: where a per-room "EnergyPosture" arbiter would live in screeps-ibex

## 1. Exact dispatcher order

The tick system list is defined ONCE in the `for_each_system!` macro (`screeps-ibex/src/game_loop.rs:60-149`); **declaration order IS execution order**. `run_systems` (`game_loop.rs:188-211`) expands the macro, runs each system via `run_now` followed by `world.maintain()`, and reads the CPU-governor tier once (`game_loop.rs:189`). Each system carries a `StageClass` (`Always` never sheds; `SkipUnderCritical` sheds under CPU pressure, `game_loop.rs:157-170`).

Full order (label, file:line of registration):

**Pre-pass (inputs):**
1. `WaitForSpawnSystem` — game_loop.rs:71
2. `CleanupCreepsSystem` — :72
3. `EntityCleanupSystem` (prepass) — :77
4. `CreateRoomDataSystem` — :78
5. `UpdateRoomDataSystem` — :79
6. `EntityMappingSystem` — :80 (builds RoomName→Entity map)
7. `ThreatAssessmentSystem` — :81
8. **`EconomyAssessmentSystem` — :82** ← the existing room-level economy snapshot is computed HERE, before everything strategic. A new EnergyPosture system slots naturally right after this line (class `Always`).

**Cleanup:** `RepairQueueClearSystem` :84 (clears RepairQueue at tick START), `ClearVisualizationSystem` :85, `VisibilityQueueCleanupSystem` :86, `CombatObjectiveCleanupSystem` :87, `CostMatrixClearSystem` :88, `RoomStatusCacheClearSystem` :89.

**Pre-run:** `OperationManagerSystem` :91 → `PreRunOperationSystem` :92 → `PreRunMissionSystem` :93 → `PreRunSquadUpdateSystem` :94 → `PreRunJobSystem` :95.

**Execution:** `RunOperationSystem` :97 → `RunMissionSystem` :98 → `SquadManagerSystem` :101 → `RunSquadUpdateSystem` :102 → **`RunJobSystem` :103** → `EntityCleanupSystem` :105 → `MovementUpdateSystem` :106.

**Observer/summarize (shed-able):** :108-128.

**Queues (never shed):** **`SpawnQueueSystem` :130** (consumes the tick's spawn requests) → **`TransferQueueUpdateSystem` :131** (this system only CLEARS the TransferQueue — transfersystem.rs:2457-2466) → `OrderQueueSystem` :132.

**Planning/stats/persistence:** :134-148, ending with `MemoryArbiterSystem` :148.

So the answer to "mission → jobs → transfer → spawn?": **missions (pre_run :93 / run :98) → jobs (run :103) → movement :106 → spawn :130 → transfer-queue clear :131.** Missions enqueue `SpawnRequest`s and transfer requests during their run; jobs consume/act on transfer tickets the same tick; the spawn queue is drained the same tick at :130 and cleared+snapshotted (`spawnsystem.rs:558-568`, snapshot read next tick by EconomyAssessment via `SpawnQueueSnapshot`, economy.rs:11-19). The RepairQueue lifecycle is inverse: cleared at tick start (:84), populated by missions during :93/:98, consumed by jobs at :103.

Spawn priority within a room is a plain f32 band system: `SPAWN_PRIORITY_CRITICAL=100` (miners), `SPAWN_PRIORITY_COMBAT_FORMING=85`, `HIGH=75` (haulers/upgraders), `MEDIUM=50`, `LOW=25`, `NONE=0` (`spawnsystem.rs:22-39`); the queue is descending-sorted (`spawnsystem.rs:212-225`, comparator deliberately reversed — REFUTED review seed, see test :594) and `process_room_spawns` has a head-of-line energy-banking break: a request affordable-by-capacity but not by current energy `break`s the rest of the queue, reserving the room's energy for it (`spawnsystem.rs:434-436`). That banking mechanism is directly relevant to stress rebootstrap: whatever is highest priority monopolizes incoming energy.

## 2. What a job's tick can see (per-room context)

Both `PreRunJobSystem` and `RunJobSystem` use `JobSystemData` (`jobs/jobsystem.rs:17-34`): `creep_owners`, `jobs`, `updater`, `entities`, `transfer_queue: Write<TransferQueue>`, `room_data: ReadStorage<RoomData>`, `movement`, `movement_results`, `mapping: Read<EntityMappingData>`, `squad_contexts`, `repair_queue: Read<RepairQueue>`, `visibility_queue`, `pathfinder`, `intent_recorder`, `breach_cache`. It is split per-creep into `JobExecutionSystemData` (jobsystem.rs:36-42: updater/entities/room_data/squad_contexts/repair_queue) and `JobExecutionRuntimeData` (jobsystem.rs:44-55: creep_entity, `owner: &Creep`, mapping, transfer_queue, movement, visibility_queue, pathfinder, intent_recorder, breach_cache), wrapped as `JobTickContext` (`jobs/context.rs:4-8`).

**Room-level energy path available to a haul job today:** `runtime_data.mapping.get_room(&room_name)` (`entitymappingsystem.rs:12-14`, map rebuilt each tick at game_loop.rs:80) → `Entity` → `system_data.room_data.get(entity)` → `RoomData` (cached `get_structures()` incl. storages/containers/spawns, `get_dynamic_visibility_data()`). This is exactly what `tick_opportunistic_repair` does (`jobs/utility/repairbehavior.rs:123-124`), and the repair room-scan even computes `available_energy` from the room's storages itself (`jobs/utility/repair.rs:180-184`) — i.e. each consumer re-derives "room energy state" ad hoc.

**What jobs can NOT see today:** `EconomySnapshot` and `Features` are NOT in `JobSystemData` — missions get both (`missions/missionsystem.rs:45,48`), jobs get neither. So a room-stress signal currently cannot reach a haul job except by re-scanning RoomData.

**Where the operator's complaint lives concretely:** `HaulJob` has a serialized `allow_repair: bool` (`jobs/haul.rs:24`, set true for remote hauls at `missions/haul.rs:295`); its `Delivery` and `MoveToRoom` states call `tick_opportunistic_repair(tick_context, Some(RepairPriority::Low))` unconditionally when `allow_repair` (`jobs/haul.rs:229-233, 246-248`), burning carried energy (deducted from the deposit ticket via `consume_resource_from_deposits`, haul.rs:230-232) on any structure <75% hits within range 3 (`repair.rs:23-37` maps ≥50%→Low; roads qualify) — with no knowledge of room stress. Extension/spawn refill demand is a flat `TransferPriority::High` deposit request from `RoomTransferMission` (`missions/localsupply/room_transfer.rs:426-462`); priorities are the 4-level `TransferPriority` enum High/Medium/Low/None (`transfer/transfersystem.rs:16-23`). There is no room-level arbiter: the effective ordering emerges from hard-coded per-request priorities plus the haul Idle state's or_else chain (`jobs/haul.rs:86-139`).

## 3. Adding a new specs Resource (per-room EnergyPosture) — end-to-end analog

The sanctioned pattern (statics→Resource migration done; bot crate = sanctioned statics only) has two complete analogs; **`EconomySnapshot` is the closest** and **`RepairQueue` is the cleanest full lifecycle**:

- **Register:** explicit `world.insert(...)` in `create_environment`: `world.insert(EconomySnapshot::default())` at `game_loop.rs:893`; `world.insert(crate::repairqueue::RepairQueue::default())` at `game_loop.rs:904`; `SupplyStructureCache` at :910. (Resources implementing `Default` also auto-register via `setup_systems(&mut world)` at game_loop.rs:913, since `Write<'a, T>`'s setup inserts the default — `TransferQueue` relies on this. Explicit insert is used when the resource must exist before setup, see the MetricsState comment at :897-899.)
- **Compute:** a dedicated pre-pass system. `EconomyAssessmentSystem` (`military/economy.rs:154-239`) rebuilds `EconomySnapshot { rooms: HashMap<Entity, RoomEconomyData>, ... }` every tick from RoomData joins — per-room `stored_energy`, `energy_income`, `spawn_energy`, `spawn_energy_capacity`, `free_spawns`, `prev_tick_queue_depth` (economy.rs:26-51). It is ordered at `game_loop.rs:82`, after room-data update (:79) and threat (:81), before all operations/missions/jobs. An `EnergyPostureSystem` would be registered one line below (StageClass::Always) reading `EconomySnapshot` + `RoomData` and writing `Write<EnergyPosture>`.
- **Consume (missions):** add to `MissionSystemData` (missionsystem.rs:27-55) and plumb into `MissionExecutionSystemData` (missionsystem.rs:57-95), exactly as `economy: Write<'a, EconomySnapshot>` is at missionsystem.rs:45→75. Missions already read `governor: GovernorSnapshot` and `features: Features` as Copy fields (missionsystem.rs:78-80).
- **Consume (jobs):** add `energy_posture: Read<'a, EnergyPosture>` to `JobSystemData` (jobsystem.rs:17-34) and a `&'a EnergyPosture` field to `JobExecutionSystemData` (jobsystem.rs:36-42) — identical to how `repair_queue: Read<'a, RepairQueue>` flows (jobsystem.rs:29 → 41 → consumed in `tick_opportunistic_repair` via `tick_context.system_data.repair_queue`, repairbehavior.rs:131). Gating the haul.rs:229/246 opportunistic-repair calls on posture is then a two-line read.
- **Consume (spawn):** `SpawnQueueSystemData` already reads `economy: Read<'a, EconomySnapshot>` (spawnsystem.rs:256) for the renew energy gate (spawnsystem.rs:407-412) — an EnergyPosture read slots in identically.
- **Keying:** `EconomySnapshot.rooms` keys by room `Entity`; `RepairQueue.rooms` keys by `RoomName` (repairqueue.rs:34-37). Jobs hold RoomName (creep pos) and resolve Entity via `EntityMappingData`; either key works.
- A pure-math home already exists for economy kernels: `room_economics.rs` (pure, world-free net-ROI kernel, room_economics.rs:1-29) — the precedent for keeping posture-threshold math unit-testable outside the ECS.

## 4. Feature-flag kill-switch convention

`features.rs`: nested `#[derive(Serialize, Deserialize)] #[serde(default)]` structs with explicit `Default` impls, round-tripped to `Memory._features` via serde_wasm_bindgen. `load()` runs every tick (features.rs:791-813): read → fill defaults for missing keys → **write the fully-resolved struct back to Memory** so operators always see every knob in the console; the game loop inserts the result as a world Resource each tick (`env.world.insert(features)`, game_loop.rs:969). Missing/malformed → `Features::default()` (features.rs:753-762). Field-level defaults use `#[serde(default = "fn")]` (e.g. features.rs:104-120).

Kill-switch examples: `DerelictFeatures.declaim: bool` default true (features.rs:583/616), `SourceKeeperFeatures.farming` (features.rs:633/645), `MilitaryFeatures.offense`/`attack_players` (features.rs:341-344), top-level `raid`/`dismantle` bools (features.rs:701/708), `ClaimFeatures.safety_gate` (features.rs:467). A new `EnergyPostureFeatures { on: bool, ... }` (or a bool on an existing group) is purely additive — serde defaults mean no Memory migration and no WFV interaction. Reset flags are separate (`Memory._features.reset.*`, features.rs:15-31).

## 5. Serialized state vs. per-tick recomputation (WFV impact)

The persisted component tuple is exactly: `creep_spawnings, creep_owners, creep_movement_data(CreepRoverData), room_data, room_plan_data, job_data, operation_data, mission_data, squad_context, visibility_queue_data, combat_objective_data, room_threat_data` (`game_loop.rs:494-512`), fingerprinted by `WORLD_FORMAT_VERSION = 23` (game_loop.rs:679; bincode is positional — any shape change to any component in the tuple needs a bump, per the doc block :577-678).

- **All four priority queues are ephemeral Resources, never serialized:** `SpawnQueue` (cleared after processing each tick, spawnsystem.rs:568), `TransferQueue` (cleared by TransferQueueUpdateSystem, transfersystem.rs:2463-2465), `RepairQueue` (cleared at tick start, repairqueue.rs:137-153, "ephemeral -- rebuilt each tick" :31-33), `OrderQueue`. Spawn priorities are f32 constants recomputed by missions every tick (spawnsystem.rs:22-39). **A redesign that only changes how priorities are computed per tick — including a new EnergyPosture Resource and posture-conditional request priorities — requires NO WFV bump.**
- **BUT job internals ARE serialized:** `JobData` rides the tuple (game_loop.rs:501, jobs/data.rs:9-23 `ConvertSaveload`). `HaulJob` persists `HaulJobContext` (incl. `allow_repair`, haul.rs:20-26) and the in-flight `HaulState` (haul.rs:28-39), whose `Pickup`/`Delivery` variants embed `TransferWithdrawTicket`/`TransferDepositTicket` (transfersystem.rs:934-938, 1055-1058) whose resource entries carry `priority: TransferPriority` (transfersystem.rs:1029-1034). So: reordering/adding/removing `TransferPriority` variants, reshaping the tickets, or adding fields to `HaulJobContext`/`HaulState` (e.g. a per-job posture override) **WOULD force a WFV bump** (one loud reset). Same for adding fields to `RoomData`/`MissionData`. The clean pattern is therefore: keep posture entirely in the ephemeral Resource + read-side gates, and priorities stay recomputed per tick — reset-free.

## Bonus observations relevant to the operator's goal

- The head-of-line energy-banking break in `process_room_spawns` (spawnsystem.rs:434-436) already implements "save energy for the top-priority spawn" — the stress problem is upstream: energy leaks out via drive-by repair (haul.rs:229) and via missions requesting repair/build work at normal priority with no room-stress modulation, before it ever reaches extensions.
- `EconomyAssessmentSystem` already computes everything a bootstrap-stress classifier needs (`stored_energy`, `spawn_energy` vs `spawn_energy_capacity`, `free_spawns`, `prev_tick_queue_depth`) — a posture arbiter can be a near-pure function over `RoomEconomyData`, testable host-side, matching the `room_economics.rs`/`claim_economics` pure-kernel precedent.
- The CPU-governor `StageClass` seam (game_loop.rs:60-70) means the new system must be classified `Always` (it feeds spawn/haul, the never-shed set).

## Citations
- screeps-ibex/src/game_loop.rs:60 — for_each_system! macro — single definition of tick system order; declaration order IS execution order
- screeps-ibex/src/game_loop.rs:82 — EconomyAssessmentSystem registered in pre-pass (after ThreatAssessment :81, before operations/missions/jobs) — the slot for an EnergyPosture system
- screeps-ibex/src/game_loop.rs:93 — PreRunMissionSystem :93 / PreRunJobSystem :95 / RunOperationSystem :97 / RunMissionSystem :98 / RunJobSystem :103 / MovementUpdateSystem :106 ordering
- screeps-ibex/src/game_loop.rs:130 — SpawnQueueSystem :130, TransferQueueUpdateSystem :131 (queue clear), OrderQueueSystem :132 — queues run after jobs, never shed
- screeps-ibex/src/game_loop.rs:188 — run_systems: sequential run_now + world.maintain() per system; governor tier read once at :189
- screeps-ibex/src/game_loop.rs:893 — world.insert(EconomySnapshot::default()) — explicit Resource registration in create_environment
- screeps-ibex/src/game_loop.rs:904 — world.insert(RepairQueue::default()) — ephemeral rebuilt-per-tick resource registration
- screeps-ibex/src/game_loop.rs:913 — setup_systems(&mut world) — auto-registers Default resources declared in any system's SystemData
- screeps-ibex/src/game_loop.rs:969 — env.world.insert(features) — Features loaded from Memory._features each tick and inserted as a Resource
- screeps-ibex/src/game_loop.rs:494 — serialize tuple: the ONLY persisted components (job_data at :501); queues/resources are not in it
- screeps-ibex/src/game_loop.rs:679 — WORLD_FORMAT_VERSION = 23; doc block :577-678 explains bincode-positional shape-change rule
- screeps-ibex/src/jobs/jobsystem.rs:17 — JobSystemData — everything a job tick can see (no EconomySnapshot, no Features)
- screeps-ibex/src/jobs/jobsystem.rs:29 — repair_queue: Read<RepairQueue> in JobSystemData, plumbed to JobExecutionSystemData :41 — the exact pattern for adding Read<EnergyPosture>
- screeps-ibex/src/jobs/jobsystem.rs:44 — JobExecutionRuntimeData — per-creep context incl. mapping + transfer_queue
- screeps-ibex/src/entitymappingsystem.rs:12 — EntityMappingData.get_room(RoomName)->Entity — the job-side path to RoomData; rebuilt each tick
- screeps-ibex/src/jobs/haul.rs:24 — HaulJobContext.allow_repair — SERIALIZED (ConvertSaveload) job field gating opportunistic repair
- screeps-ibex/src/jobs/haul.rs:229 — Delivery state calls tick_opportunistic_repair(RepairPriority::Low) unconditionally when allow_repair — the operator-reported energy leak; also MoveToRoom at :246
- screeps-ibex/src/jobs/utility/repairbehavior.rs:111 — tick_opportunistic_repair — drive-by repair via mapping->RoomData + RepairQueue, no room-stress awareness
- screeps-ibex/src/jobs/utility/repair.rs:23 — map_normal_priority — any structure <75% hits gets at least RepairPriority::Low (roads qualify for drive-by repair)
- screeps-ibex/src/repairqueue.rs:29 — RepairQueue global resource: missions write, jobs read, ephemeral rebuilt each tick — cleanest full-lifecycle analog
- screeps-ibex/src/repairqueue.rs:144 — RepairQueueClearSystem — tick-start clear system (ordered at game_loop.rs:84)
- screeps-ibex/src/military/economy.rs:26 — RoomEconomyData — per-room stored_energy/energy_income/spawn_energy/free_spawns/prev_tick_queue_depth, rebuilt per tick
- screeps-ibex/src/military/economy.rs:154 — EconomyAssessmentSystem — computes EconomySnapshot in pre-pass; the end-to-end Resource analog for EnergyPosture
- screeps-ibex/src/missions/missionsystem.rs:27 — MissionSystemData — mission tick context incl. spawn_queue :34, transfer_queue :38, repair_queue :42, economy :45, features :48
- screeps-ibex/src/missions/missionsystem.rs:78 — MissionExecutionSystemData carries governor + features as Copy fields — pattern for handing posture to missions
- screeps-ibex/src/spawnsystem.rs:22 — SPAWN_PRIORITY_* f32 bands (CRITICAL 100 / COMBAT_FORMING 85 / HIGH 75 / MEDIUM 50 / LOW 25 / NONE 0) — recomputed per tick, not serialized
- screeps-ibex/src/spawnsystem.rs:434 — head-of-line energy-banking break: unaffordable-now top request reserves room energy and breaks the queue
- screeps-ibex/src/spawnsystem.rs:256 — SpawnQueueSystemData reads EconomySnapshot (renew gate :407-412) — precedent for a posture read in the spawn system
- screeps-ibex/src/spawnsystem.rs:558 — SpawnQueueSystem snapshots queue depth to SpawnQueueSnapshot then clears — one-tick-stale feedback into EconomyAssessment
- screeps-ibex/src/transfer/transfersystem.rs:16 — TransferPriority enum High/Medium/Low/None — serialized inside persisted haul tickets
- screeps-ibex/src/transfer/transfersystem.rs:1029 — TransferDepositTicketResourceEntry embeds priority: TransferPriority — priority values ride the persisted JobData via in-flight tickets
- screeps-ibex/src/transfer/transfersystem.rs:2457 — TransferQueueUpdateSystem = clear-only (queue is ephemeral, lazily generated per tick)
- screeps-ibex/src/missions/localsupply/room_transfer.rs:426 — spawn/extension refill = flat TransferPriority::High deposit requests (spawns :426-443, extensions :445-462) — no stress modulation
- screeps-ibex/src/missions/localsupply/room_transfer.rs:717 — RoomTransferMission registers TransferQueue generators (lazy per-room request population)
- screeps-ibex/src/features.rs:583 — DerelictFeatures.declaim — canonical kill-switch example (bool, default true, serde(default))
- screeps-ibex/src/features.rs:791 — load(): per-tick read-merge-writeback of Memory._features; result inserted as world Resource (statics-review M5)
- screeps-ibex/src/jobs/data.rs:9 — JobData enum derives ConvertSaveload — jobs are serialized components; enum shape changes need WFV bump
- screeps-ibex/src/room_economics.rs:1 — pure world-free room-economics kernel — precedent for keeping posture math as a pure, testable module in the bot crate
- screeps-ibex/src/missions/haul.rs:295 — allow_repair = max_distance > 0 — remote haulers get drive-by repair enabled at mission-creation time

## Gaps
- Did not trace OrderQueue/OrderQueueSystem (market) internals — out of scope for energy posture; it runs at game_loop.rs:132 after the transfer-queue clear.
- Did not enumerate every mission that submits RepairQueue requests or every job with an opportunistic-repair call site (harvest.rs, build.rs, staticmine.rs also match 'repair' — the haul.rs Delivery/MoveToRoom sites were verified as the canonical pattern; the others use the same tick_opportunistic_repair helper).
- Did not verify whether BuildJob/UpgradeJob withdraw-from-storage paths have their own stress gates (jobs/utility/haulbehavior.rs get_new_* helpers not deep-read); the TransferPriority::None storage withdraw at room_transfer.rs:464-499 suggests upgraders/builders pull at passive priority, but the consumer-side selection chain was only read for HaulJob.
- RoomData internals (get_structures caching, dynamic visibility) cited but not deep-read (room/data.rs is 1323 lines); the accessor usage in economy.rs/repair.rs confirms the shape.