# Where "opportunistically repairing roads" comes from

The exact function is `tick_opportunistic_repair` in `screeps-ibex/src/jobs/utility/repairbehavior.rs:111-171`, called per-tick from inside the tick handlers of three creep jobs (harvest, haul, staticmine), plus non-opportunistic repair-as-a-state in `BuildJob`/`HarvestJob` and tower repair. Roads enter the pipeline via `LocalBuildMission::pre_run_mission`, which enqueues every damaged non-wall structure (roads included) into the global `RepairQueue` every tick.

## 1. Which roles run opportunistic repair, where in their tick, thresholds, and priority vs spawn/extension refill

**The opportunistic primitive** (`repairbehavior.rs:111-171`): fires when (a) the Pipeline-A intent flag `REPAIR` is not yet consumed this tick, (b) creep carries > 0 energy, (c) creep has ≥ 1 WORK part, (d) a repair target exists **within range 3** of the creep's current position at ≥ the caller's minimum `RepairPriority`. Walls/ramparts are excluded (`allow_walls=false`, comment "too expensive for a drive-by", repairbehavior.rs:127-136). There is **no room-energy or bootstrap-state check whatsoever** in this path.

**Priority thresholds that make ROADS eligible** (`repair.rs:23-37`, `map_normal_priority` — roads and all "other" structures): health < 25% → `High`, < 50% → `Medium`, < 75% → `Low`, else `VeryLow`. Containers/spawns/towers use `map_high_value_priority` (`repair.rs:39-53`): < 50% → `Critical`, < 75% → `High`, < 95% → `Low`. These mappings **ignore room energy entirely**.

**Call sites per role:**

- **Harvester (`HarvestJob` — the bootstrap generalist creep, spawned by `missions/localsupply/source_mining.rs:199` with `allow_haul=true`)** — the worst offender for the operator's symptom:
  - `jobs/harvest.rs:225` — `Harvest` state: `tick_opportunistic_repair(ctx, Some(RepairPriority::Medium))` runs **before** `tick_harvest` every tick. Because `REPAIR` and `HARVEST` share intent Pipeline A (`jobs/actions.rs:28-31`), a fired repair **skips that tick's harvest** (engine-accurate, but income lost).
  - `jobs/harvest.rs:264-266` — `Delivery` state: while delivering — **including HIGH-priority deliveries to spawns/extensions** — any structure ≤ 50% health within range 3 (min `Medium`) gets repaired, and the consumed energy is deducted from the delivery tickets via `consume_resource_from_deposits` (`transfer/transfersystem.rs:1124-1134`). Repair (Pipeline A) and transfer (Pipeline D, `actions.rs:44-48`) fire the same tick, so the trip isn't slowed — but the spawn receives less energy than the ticket promised.
  - `jobs/harvest.rs:177-185` — `Idle` also selects a full **Repair state** (walk-to-and-repair, min `Medium`, walls allowed via `get_new_repair_state`→`select_repair_structure(..., true)` at repairbehavior.rs:28). Idle priority order (harvest.rs:165-211): (1) deliver carried resources at `TransferPriority::High` — spawns/extensions register at exactly `High` (`missions/localsupply/room_transfer.rs:426-462`), (2) upgrade if RCL < 2, (3) build, (4) **repair ≥ Medium**, (5) deliver Medium/Low/None, (6) upgrade uncapped. So the full repair state ranks *below* spawn/extension refill but *above* all lower-priority deliveries — and `FinishedRepair` (harvest.rs:331-338) chains repair-after-repair until no Medium+ target remains, keeping the creep in repair mode.

- **Hauler (`HaulJob`)** — gated by constructor param `allow_repair`:
  - `jobs/haul.rs:229-233` — `Delivery` state, min `Low` (roads < 75% health!), deducts from deposit tickets; `jobs/haul.rs:246-248` — `MoveToRoom` state, min `Low`, consumed energy not deducted from anything.
  - **BUT** `missions/haul.rs:295-296`: `allow_repair = max_distance > 0` and `storage_delivery_only = max_distance > 0` — only **remote/outpost haulers** repair, and their deliveries go to storage, not spawns. Home spawn-refill haulers (`max_distance == 0`) have `allow_repair=false`. `missions/salvage.rs:190` also passes `false`. So the hauler leak burns remote-road energy but does not directly divert a spawn-bound carry.
  - `missions/haul.rs:299` carries the literal TODO: `"Make sure there is handling for starvation/bootstrap mode."`

- **Static miner (`StaticMineJob`, container miners)** — `jobs/staticmine.rs:144`: `tick_opportunistic_repair(ctx, Some(RepairPriority::Low))` inside `try_harvest_mine_target`, **before** the harvest intent, source arm only. Shared Pipeline A means a repairing miner skips that tick's harvest. Its own container is `Low` priority already at < 95% health (`map_high_value_priority`), so miners repair their container (correct) but also any road < 75% within range 3 using energy in carry.

- **Builder (`BuildJob`)** — not the range-3 primitive but full repair states in `Idle` (`jobs/build.rs:56-104`): (1) **repair ≥ High FIRST — ahead of building**, (2) build, (3) **repair at ANY priority (`None` minimum — includes `VeryLow`, i.e. roads at 75-99% health)**, (4) pickup energy (HAUL|USE, ALL priorities), (5) harvest if the room has no storage. So an energy-carrying builder always prefers topping off roads to going idle.

- **Towers** (`missions/tower.rs:426-453`): no hostiles → heal weakest friendly, else repair min `Low` (roads < 75%); hostiles present → min `Medium`. No tower-energy floor and no room-energy check; each tower repair action costs 10 energy. Tower refill deposit priority is `High` with hostiles, `Low` otherwise (tower.rs:176-191).

## 2. Repair queue prioritization + energy awareness

- `RepairQueue::get_best_target` (`repairqueue.rs:54-78`): max by `RepairPriority` enum (`VeryLow < Low < Medium < High < Critical`, repair.rs:7-13), tie-broken by **lowest HP fraction**. No structure-type weighting beyond what the priority mapping already encodes — a 40%-health road (`Medium`) outranks a 60%-health container (`Low` is wrong here — container at 60% is `High` via high-value mapping; but a 96%-health container is below any 49%-health road).
- Populated each tick by: `LocalBuildMission::pre_run_mission` (`missions/localbuild.rs:181-217`) — **all damaged non-wall structures including roads** (`get_repair_targets(structures.all(), false)` at line 200); and `WallRepairMission` (`missions/wall_repair.rs:186-239`) — walls/ramparts only, only while hostiles are present (thresholds: < 100_000 hits → `Critical`, < 1_000_000 → `High`, else `Medium`; constants at wall_repair.rs:19-22).
- Consumers: `select_repair_structure[_and_priority]` (`repair.rs:162-191`) — queue first, room-scan fallback; `select_repair_structure_in_range` (`repair.rs:208-232`) for the opportunistic range-3 case.
- **Room-energy-awareness gate: exists in exactly ONE place** — `map_defense_priority` (`repair.rs:84`): walls/ramparts at ≥ 10% health are only `VeryLow` if `available_energy > 10_000`, else `None`. `available_energy` = **sum of STORAGE energy only** (repair.rs:180-184; localbuild.rs:192-196) — containers/spawn energy not counted, and under collapse (no storage) it is 0. **Roads, containers, spawns, and towers have zero energy gating** (`map_normal_priority` and `map_high_value_priority` never receive `available_energy`). The in-range fallback scan even passes `available_energy: None` (repair.rs:227). There is no bootstrap/collapse/starvation mode anywhere in the repair path.

## 3. Energy-leak mechanism, quantified

Engine costs (matching the code's own arithmetic at repairbehavior.rs:144-148): a repair action repairs `REPAIR_POWER = 100` hits per WORK part per tick and costs **1 energy per WORK part per tick** (= `REPAIR_COST 0.01` energy per hit repaired). `tick_opportunistic_repair` returns `energy_consumed = min(work_parts, carried_energy, ceil(missing_hits / 100))`.

- **Direct spawn-refill diversion (HarvestJob only):** `harvest.rs:264-266` — the returned consumption is subtracted from the creep's deposit tickets (`consume_resource_from_deposits`, transfersystem.rs:1124-1134), which at bootstrap are the `TransferPriority::High` spawn/extension tickets (room_transfer.rs:434, 453). A harvester with W WORK parts loses **W energy per tick** for every tick any ≤ 50%-health structure is within range 3 of its delivery path — and since one action only restores 100·W hits, a decayed road stays under-threshold for many consecutive ticks, so the leak repeats along the whole corridor, trip after trip. Bootstrap bodies are WORK-heavy, making W (and the leak) proportionally large exactly when energy matters most.
- **Income loss on top:** in `Harvest`/`StaticMine` states the repair consumes the shared Pipeline-A flag, so the creep forfeits that tick's harvest (2 energy/WORK/tick foregone) in addition to the repair energy.
- **Remote hauler leak:** haul.rs:230 deducts from storage-bound tickets; haul.rs:247 (MoveToRoom) spends carry energy with no ticket accounting at all. Not spawn-bound energy, but still drains the pool during a crash.
- **Builder leak:** builders pull energy at ALL transfer priorities (build.rs:84-93) and will spend it repairing roads down to `VeryLow` priority (any damage) when no construction sites exist, competing for the same scarce energy the spawn needs.
- **Tower leak:** 10 energy per repair shot on any road < 75% whenever the room is peaceful (tower.rs:436-453), with hauler effort then diverted to refilling towers.

## 4. Dedicated repairers?

There is **no dedicated repairer job type** — repair is (a) opportunistic (harvest/haul/staticmine), (b) builder-driven (`BuildJob` repair states), and (c) tower-driven. Sizing: `LocalBuildMission::get_repairer_priority` (`localbuild.rs:112-122`) adds **1** builder at `SPAWN_PRIORITY_HIGH` if the best available repair is ≥ `High` (e.g. any road < 25% health — trivially true after a collapse) or **1** at `SPAWN_PRIORITY_MEDIUM` if ≥ `Medium` (road < 50%); merged via `max` with the construction-driven builder count (localbuild.rs:249-252). Note the perverse effect under collapse: badly decayed roads *raise* the spawn priority of a repair-builder, spending spawn energy on a `[Carry,Work,Move,Move]`-repeat body (localbuild.rs:263-270) whose Idle logic then repairs roads ahead of everything except High-priority repairs and construction. `WallRepairMission` spawns **no creeps** — it populates the queue and registers a High-priority tower-energy transfer generator during sieges (wall_repair.rs:95-145). Repair demand is otherwise never sized against energy income or state.

## 5. Feature flags

**There is no repair-specific feature flag.** The only adjacent gate is `features.military.defense` (`features.rs:336-338`, default `true`), which gates `run_defense_scan` (`operations/war.rs:240-242`) and therefore `WallRepairMission` creation (war.rs:729-738, hostiles + home room only). `LocalBuildMission` (the road-repair queue population and repairer spawning) is created unconditionally by `ColonyMission` (`missions/colony.rs:256-267`), and every opportunistic call site is ungated — `HaulJob.allow_repair` is a per-job constructor parameter (missions/haul.rs:295), not a feature. **Nothing in `features.rs` can turn off road repair or make it energy-aware.**

## Net diagnosis for the operator's symptom

Under collapse: roads decay below 50-75% everywhere → `LocalBuildMission` floods the `RepairQueue` with Medium/High road entries every tick (no energy gate for roads) → (1) bootstrap harvesters leak `WORK` energy/tick from spawn-bound carries along the whole delivery path and forfeit harvest ticks, (2) harvesters that finish a HIGH delivery drop into chained full-repair states before Medium/Low deliveries, (3) `get_repairer_priority` sees `High` road repairs and requests an extra builder at HIGH spawn priority, (4) builders repair roads at ANY damage level with energy drawn at all priorities, (5) towers burn 10/shot on roads and demand refill hauling. The single energy-awareness gate (storage > 10k) covers only walls/ramparts and reads a storage that may not exist post-wipe.

## Citations
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repairbehavior.rs:111 — tick_opportunistic_repair: the drive-by repair primitive — energy>0, WORK>0, range 3, walls excluded, no room-energy check; returns energy consumed = min(work_parts, carried, ceil(missing_hits/100))
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repairbehavior.rs:144 — energy accounting: 1 energy per WORK part per repair action, REPAIR_POWER=100 hits per WORK
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repairbehavior.rs:24 — get_new_repair_state requires only carried energy > 0; allow_walls=true for the full repair state
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repair.rs:23 — map_normal_priority (roads): <25% High, <50% Medium, <75% Low, else VeryLow — no energy input
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repair.rs:39 — map_high_value_priority (spawn/tower/container): <50% Critical, <75% High, <95% Low — no energy input
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repair.rs:84 — the ONLY room-energy gate: walls/ramparts VeryLow only if available_energy > 10_000 (storage sum), else None
- C:\code\screeps-ibex\screeps-ibex\src\jobs\utility\repair.rs:180 — available_energy = sum of STORAGE energy only (0 post-wipe); in-range fallback passes None (line 227)
- C:\code\screeps-ibex\screeps-ibex\src\repairqueue.rs:54 — RepairQueue::get_best_target — max by priority enum then lowest HP fraction; ephemeral per-tick resource
- C:\code\screeps-ibex\screeps-ibex\src\jobs\harvest.rs:225 — Harvest state: opportunistic repair min Medium BEFORE harvest; shared Pipeline A skips the harvest tick
- C:\code\screeps-ibex\screeps-ibex\src\jobs\harvest.rs:264 — Delivery state: opportunistic repair min Medium during spawn/extension deliveries; consumed energy deducted from deposit tickets
- C:\code\screeps-ibex\screeps-ibex\src\jobs\harvest.rs:177 — Idle priority chain: High deliveries > upgrade(RCL<2) > build > repair(Medium+) > Medium/Low/None deliveries; FinishedRepair chains repairs (line 331)
- C:\code\screeps-ibex\screeps-ibex\src\jobs\haul.rs:229 — HaulJob Delivery: if allow_repair, opportunistic repair min Low, deducts from tickets; MoveToRoom variant at line 246 deducts nothing
- C:\code\screeps-ibex\screeps-ibex\src\missions\haul.rs:295 — allow_repair = max_distance > 0 (remote haulers only; home spawn-refill haulers never repair); line 299 TODO: no starvation/bootstrap handling
- C:\code\screeps-ibex\screeps-ibex\src\jobs\staticmine.rs:144 — StaticMineJob source arm: opportunistic repair min Low before harvest intent
- C:\code\screeps-ibex\screeps-ibex\src\jobs\build.rs:61 — BuildJob Idle: repair>=High FIRST, then build, then repair at ANY priority (None minimum, incl. VeryLow roads), then energy pickup
- C:\code\screeps-ibex\screeps-ibex\src\missions\localbuild.rs:181 — pre_run_mission enqueues ALL damaged non-wall structures (roads included) into RepairQueue every tick, storage-only energy sum
- C:\code\screeps-ibex\screeps-ibex\src\missions\localbuild.rs:112 — get_repairer_priority: 1 builder at SPAWN_PRIORITY_HIGH if best repair >= High, 1 at MEDIUM if >= Medium — repair demand sizing is 'at most one creep'
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\room_transfer.rs:426 — spawns and extensions register energy deposits at TransferPriority::High
- C:\code\screeps-ibex\screeps-ibex\src\missions\tower.rs:433 — towers repair min Low (roads <75%) when peaceful, min Medium with hostiles; no energy floor; tower refill priority High/Low at line 176
- C:\code\screeps-ibex\screeps-ibex\src\missions\wall_repair.rs:19 — WallRepairMission: siege-only, walls/ramparts only (100k/1M thresholds), spawns no creeps, boosts tower energy priority
- C:\code\screeps-ibex\screeps-ibex\src\jobs\actions.rs:27 — SimultaneousActionFlags: REPAIR shares Pipeline A with HARVEST/BUILD (bit 1<<1); TRANSFER is Pipeline D (1<<4) so repair+transfer co-fire
- C:\code\screeps-ibex\screeps-ibex\src\transfer\transfersystem.rs:1124 — consume_resource_from_deposits: repair energy is subtracted from delivery tickets (spawn receives less)
- C:\code\screeps-ibex\screeps-ibex\src\features.rs:336 — MilitaryFeatures.defense (default true) is the only adjacent flag; no repair-specific feature flag exists
- C:\code\screeps-ibex\screeps-ibex\src\operations\war.rs:240 — run_defense_scan early-returns on !features.military.defense, gating WallRepairMission creation (line 729, hostiles + home rooms)
- C:\code\screeps-ibex\screeps-ibex\src\missions\colony.rs:256 — ColonyMission creates LocalBuildMission unconditionally — road-repair queueing cannot be feature-flagged off
- C:\code\screeps-ibex\screeps-ibex\src\missions\localsupply\source_mining.rs:199 — HarvestJob (the leaking bootstrap generalist role) is spawned here with allow_haul=true

## Gaps
- Exact TransferPriority of container/storage deposit requests (the Medium/Low/None delivery arms harvesters fall through to) was not traced — only spawn/extension (High) and tower (High/Low) registrations were verified.
- get_new_build_state internals (construction-site selection) were not read; only its position in the priority chains was verified.
- Whether ColonyMission itself is gated by any feature was not fully verified beyond the LocalBuildMission creation site (colony.rs:256 shows no gate in the surrounding context).