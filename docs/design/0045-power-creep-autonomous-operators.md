# ADR 0045 — Autonomous Power-Creep Operators: reset-proof reconciliation, EV-driven power selection, and the full unattended lifecycle

- **Status:** Decided
- **Date:** 2026-07-10
- **Deciders:** William Archbell
- **Related:** ADR [0013](0013-power-economy-and-power-creeps.md) (the power pillar; this ADR is its **spending-half implementation spec, re-priced in the ADR 0040 currency** — 0013's engine rulebook, bank-acquisition D1, enable-policy D4 and doctrine D5 stand; its D3 operator design is refined here and superseded where the two conflict), [0040](0040-stress-energy-prioritization-and-economy-sim.md) (the milli-e/t market + `screeps-econ-decision` seam this plugs into), [0042](0042-unified-forming-pricing.md) (the R_net band-containment reference pattern; Flaws 3/4 bind this ADR's pricing), [0043](0043-pricing-normalization-ledger.md) (B1 is the bid shape; **this ADR converts B4's REFUTED "`powerspawn.rs` sanctioned coarse non-market lane — no EV kernel exists" by minting the kernel**), [0044](0044-transfer-market-min-cost-flow.md) (stage-1 haul admission governs ops/power long hauls), [0018](0018-source-keeper-room-exploitation.md) (the operation/coordinator-mission shape; "exploited, never owned" discipline), [0011](0011-spawn-orchestration.md) (OPERATE_SPAWN/OPERATE_EXTENSION become throughput *inputs* per 0013 D6), [0010](0010-boost-lab-factory-pipeline.md) (the OPERATE_FACTORY branding invariant), [0012](0012-market-and-risk.md) (ops/power market valve), 0014 (empire arbitration — declared owner of the processing energy trade), [0006](0006-eval-and-iteration-harness.md) (the `PowerFixture` GPL-seeding requirement); memory [[wfv-fine-clean-design-no-debt]], [[prefer-per-tick-optimal-over-hysteresis]], [[entity-marker-serialization]], [[ecs-dangling-ref-serialize]], [[deploy-use-screeps-pack]].
- **One line:** A **zero-serialized-state** power-creep layer that **reconciles the account's `game::power_creeps()` by name every tick** (so a WFV loud reset, a VM reset, and a fresh deploy are all the same no-op — orphaning and duplicate creation are impossible **by construction**), drives **creation → GPL-level allocation → spawn → cast scheduling → renew → relocation → death-wait** from pure kernels in `screeps-econ-decision`, prices **power processing** as a real market bid (converting ADR 0043 B4's sanctioned-coarse lane into EV), and rolls out **critical economy powers first** — GENERATE_OPS → OPERATE_SPAWN → OPERATE_EXTENSION → REGEN_SOURCE → OPERATE_POWER — with every irreversible act (enableRoom, OPERATE_FACTORY branding, delete) a **recorded veto-class policy decision, never a price**.

> **Scope boundary (read first).** This ADR owns the **operator (power-creep) layer end-to-end**: account reconciliation and reset recovery, creation/upgrade (GPL spend), spawning/assignment/relocation, the per-tick cast scheduler and ops ledger, renew/retreat/death handling, the processing-bid EV kernel, and the incremental power rollout. It does **not** re-decide: power-bank acquisition (0013 D1 — the squads stay `Farm{PowerBank}` objectives on the Squad Manager), the enable-room *policy rationale* (0013 D4 — adopted verbatim, executed here), combat doctrine for DISRUPT/FORTIFY/SHIELD (0013 D5 — activated in a later phase on 0013's terms), hauling mechanics (0007/0044 — ops and power are ordinary resources), or cross-pillar energy arbitration (ADR 0014; this ADR states its demand and ships an interim gate). Where this ADR quantifies a mechanic, the number is engine-cited, not folklore.

## Context

### Why now, and what exists

GPL accrues and is never spent: a crate-wide `PowerCreep` search finds **zero operator code** (0013 "G-2c: nothing"; the only matches are sim constants and a rover impl detail). The accrual plumbing exists — `PowerSpawnMission` (`missions/powerspawn.rs`, launched by `ColonyMission` at `missions/colony.rs:246-253`) hauls energy+power and calls `process_power()`; `TransferTarget::PowerSpawn` is frozen end-to-end; GPL level/progress are already in stats (`statssystem.rs:211,234-238,312`). Since 0013 was written, the economy grew a real currency (ADR 0040: integer **milli-e/t**, storage = par = 1000), a pure decision seam (`screeps-econ-decision`), a numeric-bid ticket lane, and a reduced-cost haul-admission model (0044). 0013's operator half (D3) predates all of that; this ADR is the market-era spec that makes it buildable — and adds the piece 0013 never designed: **surviving our own WFV resets**.

The reset problem is specific to power creeps. The bot's declared policy is reset-anytime ([[wfv-fine-clean-design-no-debt]]): a WFV bump wipes the serialized world and ordinary creeps are simply abandoned to run out their 1500-tick TTL — the accepted cost of a loud reset. A power creep is different in kind: it is an **account-level, effectively immortal asset** (renew resets the full 5000-tick life for free) whose death costs **8 real-time hours of every effect** and whose creation consumes **capped GPL allowance**. An orphaned operator after a reset is not a 1500-tick write-off; it is a multiplier-bearing asset idling toward an 8-hour funeral, and a naive "create what memory says we lack" would try to double-mint against the GPL cap. Orphan recovery must be first-class (§D1).

### Mechanics ground truth

0013's engine-cited power rulebook (its "Engine ground truth" section and POWER_INFO table, verified against `C:\code\screeps-engine` / `screeps-common`) is adopted wholesale and **not restated** — cooldown/duration/ops/effect tables, the level-gate families **[0,2,7,14,22]** (most powers), **[10,11,12,14,22]** (REGEN_SOURCE/REGEN_MINERAL/OPERATE_POWER), **[20,21,22,23,24]** (OPERATE_CONTROLLER/DISRUPT_TERMINAL), one `usePower` per creep per tick, per-power cooldowns, higher-rank active effects not overwritable by lower ranks. External verification of the same numbers: engine constants ([screeps/common constants.js](https://github.com/screeps/common/blob/master/lib/constants.js)), [usePower.js](https://github.com/screeps/engine/blob/master/src/processor/intents/power-creeps/usePower.js), [docs.screeps.com/power.html](https://docs.screeps.com/power.html), [wiki.screepspl.us/Power](https://wiki.screepspl.us/Power/). The lifecycle facts this ADR's design load-bears on:

1. **Creation is account-global and name-keyed.** `PowerCreep::create(name, class)` works any tick, costs one free GPL level; the engine's allowance check is `#creeps + Σlevels < GPL` (`createPowerCreep.js:11-13`). Only class: Operator. `upgrade(power)` spends one more level; max creep level 25, max rank 5/power; **no respec** — a learned power is permanent ([PowerCreep API](https://docs.screeps.com/api/#PowerCreep)).
2. **`game::power_creeps()` is the durable account truth** — it lists every power creep by **name**, alive, unspawned, or cooling down, with `level`, `powers` (the full build incl. per-power cooldowns), `store`, `ticks_to_live`, `shard`/`room`, `spawn_cooldown_time`, `delete_time`. Everything the layer needs is re-derivable from it every tick.
3. **Any death costs 8 real-time hours** (`POWER_CREEP_SPAWN_COOLDOWN = 8·3600·1000` ms, wall clock). Engine-verified: `_diePowerCreep.js` sets the cooldown **unconditionally**, and [`suicidePowerCreep.js`](https://github.com/screeps/engine/blob/master/src/processor/global-intents/power/suicidePowerCreep.js) delegates to it — **suicide is not exempt** (a 2019 PTR note claiming old-age exemption is stale; design to the engine). Levels and powers are never lost; recovery is waiting, not regrinding.
4. **Renew is free and full** — at an own power spawn *or any power bank*, range 1, resets TTL to 5000 (`renew.js`). `delete()` takes 24 cancellable hours and costs 1 net GPL level; accounts also hold 30 free "experimentation periods" ([power.html](https://docs.screeps.com/power.html)) — both are **manual-operator tools, never autonomous** (§D5).
5. **enableRoom** (range 1 to any controller, symmetric, effectively permanent) and **safe mode** semantics per 0013 D4: with safe mode active, *non-owner* operators can use no powers, while the owner's are unaffected (`usePower.js:13-15` — the engine citation wins over the wiki's "both sides" wording). Controller-less rooms (highways/SK) have no gate.
6. **No fatigue, no body parts** — 1 tile/tick on any terrain; roads wear as if 100 parts. **Cross-shard portals are impassable** to power creeps ([forum](https://screeps.com/forum/topic/2685/power-creep-can-t-use-an-intershard-portal-to-switch-shard)); shard migration = death (8h) + respawn, or an experimentation period — out of scope for automation.
7. **GPL is brutally superlinear:** `GPL = floor(sqrt(processed/1000))`; level n costs `1000·n²` cumulative power = `50,000·n²` processing energy; the marginal level costs `(2n−1)·1000` power.

### Competitive practice (what autonomous power layers actually do)

- **Hivemind** ([Mirroar/hivemind](https://github.com/Mirroar/hivemind) — the reference open-source implementation; Overmind and TooAngel ship no substantive power-creep automation) auto-creates an Operator whenever free GPL permits, auto-upgrades down a static priority list (`settings.default.ts` `powerPriorities`), runs a task-priority operator loop (`role/power-creep/operator.ts`): GENERATE_OPS passively when idle (capped at a 15k room ops stockpile), ops banked to storage above 90% carry / withdrawn below 10%, renew opportunistically near the power spawn and critically below TTL 200, REGEN_SOURCE re-cast at <20% remaining duration, OPERATE_STORAGE only when storage ≥90% full, one operator per mature room, and distributes OPERATE_FACTORY *ranks* across the fleet because factory branding is permanent.
- **The International** ([repo](https://github.com/The-International-Screeps-Bot/The-International-Open-Source)) runs a central `powerCreepOrganizer.ts` that **reconciles memory against `Game.powerCreeps` every tick** — pruning stale memory, tracking unspawned names, respawning when cooldowns lapse — exactly the adoption discipline §D1 adopts; its operator renews below 10% of `POWER_CREEP_LIFE_TIME`.
- Community EV consensus ([sleeplesshacker power analysis](https://sleeplesshacker.com/articles/screeps-power-analysis/), [forum fleet math](https://screeps.com/forum/topic/2183/power-creeps-update/88)): GENERATE_OPS first ("15 of the 19 powers require ops"), then economy powers; REGEN_SOURCE rank ladder ≈ unreserved-remote / reserved-remote / SK-source / "better than any single-source room" at r2–r5; OPERATE_SPAWN is the war/logistics multiplier because RCL8 rooms are spawn-throughput-bound; ops-per-GPL favors mid-level creeps (8 L11 creeps generate 66 ops vs 4 L24 creeps' 32) while deep ranks need deep creeps — a fleet-mix question this ADR defers (§D8 #3).

This matches 0013 D3.1's independently-derived build order — treated here as convergent validation, not a source.

### The two pricing traps (named up front, per ADR 0042's adversarial table)

- **Flaw 3 — band saturation.** An operator's headline EV is genuinely enormous (OPERATE_SPAWN r5 = ×5 room spawn throughput; REGEN_SOURCE r5 = +16.7 e/t per source). Raw `value·1000/horizon` numbers of that size cannot be expressed inside any existing band and **must not be cross-lane-arbitrated** until the reserved `value_e`-to-par normalization lands (0042 §Consequences (4)). This ADR's answer: the power layer **competes in almost no existing lane** — it consumes GPL, ops, and cast slots, which nothing else wants. The single genuine cross-lane contention is **processing energy**, and only that gets a market bid (§D3.2), on the transfer lane, at par scale.
- **Flaw 4 — horizon mismatch.** Every civilian rate amortizes over `CREEP_LIFE_TIME = 1500`; a renewed operator's horizon is effectively infinite and its 0042-style burn term (`cost/1500`) is **zero** — an operator has no body cost, no spawn lane, and free renewal. Any formula comparing operator value against a 1500-normalized figure must horizon-normalize explicitly or not compare at all. §D3 keeps operator-internal decisions (upgrade choice, cast choice) in their own scarce-resource currencies precisely to avoid this trap.

## Decision

Adopt a **`PowerCreepSystem`** in the bot plus a pure **`power_policy`** kernel module in `screeps-econ-decision`, with the following properties: **zero serialized state** (per-tick reconciliation from the account API is the whole persistence story, §D1); a **derived lifecycle state machine** (§D2); **EV kernels for the four real scarcities** — GPL levels, ops, cast slots, processing energy (§D3); the **critical-economy-first rollout** (§D4); **irreversibles as recorded vetoes** (§D5); and wiring that reuses the existing operation/mission/job seams and market lanes without inventing parallel machinery (§D6).

### §D1 Zero serialized state: the reconciler IS the orphan recovery

**D1.1 The invariant.** The power layer persists **nothing** in the WFV payload — no components with power-creep fields, no minted ids, no assignment memory. Every tick, a reconciliation pass reads `game::power_creeps()` (a handful of entries; trivial CPU) and (re)derives the entire layer state:

- **build** ← `creep.powers` (ranks and per-power cooldowns are server-side truth);
- **lifecycle state** ← `shard`/`room`/`ticks_to_live`/`spawn_cooldown_time`/`delete_time` (§D2);
- **assignment** ← a pure function of (fleet, owned-room scores) recomputed per tick (§D2.4);
- **standing effects** ← target structures' `effects` (read, never remembered);
- **GPL allowance** ← `game::gpl()` level minus `#creeps + Σlevels` (the engine's own creation check, mirrored read-only).

This is the ratified discipline (0013 D3's "no serialized state" claim, honored; ADR 0018's "a stored flag would duplicate derivable state and go stale"), applied where it pays most: **a WFV loud reset, a VM reset, and a mid-tick global reset are all the same no-op for this layer.** There is no orphan state to recover *from* — the account API is the store.

**D1.2 Adoption (the WFV-reset scenario, made concrete).** After a reset the ECS world is empty but `game::power_creeps()` still lists every operator, alive or not. The reconciler's per-tick sweep:

1. For each account power creep with no live ECS entity: **adopt** — create the entity (`OperatorMission`/`OperatorJob` wiring per §D6), regardless of name, class, or origin (manually-created creeps are adopted identically; adoption is **name-agnostic** — no naming convention is load-bearing).
2. An adopted *deployed* creep's first derived priorities are exactly the dangerous ones: check TTL against the renew deadline (§D2.3) **before** resuming casts — the failure mode being defended is "orphan ages out during the operator's absence."
3. An adopted *unspawned/cooling* creep enters the spawn scheduler (§D2.2).
4. ECS entities whose named account creep no longer exists (operator deleted a creep manually) are reaped through the standard `EntityCleanupQueue` path — never `entities.delete()` directly ([[ecs-dangling-ref-serialize]]).

**D1.3 Duplicate creation is impossible by construction.** The creation decision (§D3.3) never consults bot memory for "what we own" — it reads the live account list and the live GPL allowance arithmetic each tick. A reset therefore cannot cause a double-mint: the tick after a wipe, the reconciler sees the same account state the pre-reset bot saw. The engine's own `ERR_NAME_EXISTS`/allowance check is the backstop, not the mechanism. Creation names are deterministic (`ibex-op-<k>` for the first free k) purely for log legibility — see D1.2's name-agnostic adoption.

**D1.4 What may never be serialized, and the two apparent exceptions.** The two "state-like" facts the layer produces — a room's power-enabled status and a factory's branded level — are both **readable from the game** (`controller.is_power_enabled()`, `factory.level()`) and are therefore re-derived, not persisted. The action *log* for these one-way acts (§D5) goes to console/seg-57 telemetry, which carries no WFV obligation (segments are explicitly outside `WORLD_FORMAT_VERSION` — `segments.rs:62`).

### §D2 The lifecycle state machine (derived per creep, per tick)

```
              create() [D3.3]                    spawn(power_spawn) [D2.2]
   Unborn ────────────────► Unspawned ───────────────────────────────► Deployed
 (allowance>0,              (in account list;                             │
  plan says create)          gate: spawn_cooldown_time)                   │ sub-states, priority-ordered:
      ▲                          ▲                                        │  1. Retreating   (threat; defense event)
      │                          │  death — ANY death, incl. old age:     │  2. Renewing     (TTL ≤ renew deadline)
      │                          │  8h wall-clock cooldown; loud alert    │  3. Enabling     (room !is_power_enabled)
      │                          └────────────────────────────────────────┤  4. Relocating   (assignment changed)
      └── delete(): NEVER autonomous (§D5)                                └  5. Working      (cast loop + ops banking)
```

**D2.1 States are derived, not stored:** `Unborn` = a creation the plan wants that doesn't exist; `Unspawned` = in the account list with `room == None` (further split by `spawn_cooldown_time` into *ready* and *cooling*); `Deployed` = live in a room. Sub-state selection inside `Deployed` is a strict priority order evaluated fresh each tick — no latches, per [[prefer-per-tick-optimal-over-hysteresis]].

**D2.2 Spawning.** A ready `Unspawned` operator spawns at the power spawn of its assigned room (§D2.4) the tick the assignment resolves. There is no queue contention: `spawnPowerCreep` costs no energy and no spawn lane — the power layer **never bids in the spawn band** (this dissolves the Flaw-3 exposure for spawning entirely).

**D2.3 Renew: the never-die doctrine.** Death costs 8 wall-clock hours of every standing multiplier, so renewal is paranoid and layered: (a) **opportunistic** — whenever the operator is adjacent to its power spawn (the anchor tile is Chebyshev ≤3 from the hub per 0013 D3.3, so this is nearly free) and TTL < `RENEW_OPPORTUNISTIC` (v0 4500), renew; (b) **deadline errand** — when `TTL ≤ dist_to_power_spawn + RENEW_MARGIN` (v0 200, 0013 D3.4's number), the Renewing sub-state preempts all casting; (c) field operators (bank escort, deferred phase) renew at the bank itself. `operator_deaths` is a seg-57 metric whose target is **zero, forever**; any death logs a loud alert with the derived cause. Threat response: a hostile player creep within range 5, or hits < 50%, flips Retreating (ramparted anchor tile; the room's defense objective treats "operator at risk" as a first-class event per 0013 D5.3 — wired in the defensive phase).

**D2.4 Assignment & relocation.** Assignment is a pure per-tick function: rank owned rooms with a power spawn by `room_power_score` (v0: spawn-pressure percentile from seg-57 spawn-uptime, + war exposure, + source count for the REGEN era — constants sim-swept); operator k anchors in room k of the ranking. Relocation is per-tick optimal **with the switching cost priced in, not hysteresis**: relocate iff `ΔEV_rooms_milli · H_RELOC > travel_ticks · cast_value_forgone_milli` where both sides are integer milli-e/t over the same explicit horizon `H_RELOC` (v0 10,000 ticks) — deterministic, tie-broken by room name. Travel is cheap (1 tile/tick, no fatigue) so this fires rarely and correctly. Multi-shard: out of scope (mechanics fact 6); the layer operates on the active shard only.

**D2.5 Death/respawn.** A dead operator (cooldown running) parks in *cooling* `Unspawned`; the reconciler re-schedules the spawn for the tick the wall-clock gate opens, at the then-recomputed top room. Nothing else changes — levels/powers persist server-side.

### §D3 The EV model: four scarcities, each priced in its own lane

The power layer's scarce resources are **GPL levels** (capped, superlinear to earn), **ops** (0.16/t generation ceiling per operator), **cast slots** (one `usePower` per creep-tick), and **processing energy** (50 e/power, competes with the whole economy). Only the last is commensurable with the existing market and only it gets a market bid. The first three are allocated by dedicated pure kernels in `screeps-econ-decision/src/power_policy.rs`, unit-tested in-crate, DTO-in/intent-out, per the seam rule.

**D3.1 Scale declarations (the 0043 mixed-scale trap, pre-empted).** `power_process_bid` lives on the **transfer lane at par scale** (storage = 1000 milli-e/t) — the same lane and scale as `refill_bid`/`upgrade_bid`. Nothing in this ADR posts to the spawn band (§D2.2). Operator-internal kernels (D3.3–D3.5) rank in **e/t-per-unit-of-their-own-scarcity** and never cross lanes.

**D3.2 `power_process_bid` — the kernel that converts ADR 0043 B4.** `powerspawn.rs:87-147` registers its energy demand through the coarse `TransferDepositRequest::new_tier` band path, sanctioned by 0043 B4 *only because* "no EV kernel exists." Mint it:

```rust
/// Marginal e/t-equivalent value of feeding this power spawn, at par scale.
/// value chain: 50 energy + 1 power → GPL progress → the next planned upgrade's
/// Δeffect e/t (from the D4 build table), amortized over the GPL marginal cost.
pub fn power_process_bid(consts: &PowerConsts, gpl: u32, next_upgrade_ev_milli: u32,
                         power_stock: u32, storage_energy: u32) -> u32
```

v0 shape (constants named, sim/live-swept per EP-4.6): `bid = clamp(next_upgrade_ev_milli · H_AMORT / ((2·gpl+1) · 1000 · 50), BID_FLOOR, BID_CAP)` — the next GPL level's planned upgrade value (D3.3), spread over that level's marginal power cost `(2n+1)·1000` and its 50 e/power conversion, horizon-amortized like every stock→flow conversion in the market. Properties by construction: the bid **decays superlinearly with GPL** (early GPL is cheap and valuable → processing outbids more; deep GPL is a marathon → processing yields to refill under stress), reproducing 0013 D2's "start the accrual early, throttle under pressure" from pricing rather than decree. The deposit generator migrates from `new_tier` to the numeric `TransferDepositRequest::new(...)` path (the refill idiom, `room_transfer.rs:375-377`), its inverted fullness-priority map fixed in passing (0013's noted defect). Two guardrails stay **outside** the bid, in the C-part veto idiom: the interim surplus gate (process only above the room's posture threshold — 0013 D2, handed to ADR 0014 when it lands, §D8 #7), and `process_power` itself remains free to *not* fire even with a fed spawn. Pricing `power`/`ops` as tradeable resources reuses the trust-gated `mineral_value_e` template (market price when trustworthy → cost-of-production floor, which for power is exactly `POWER_SPAWN_ENERGY_RATIO = 50` e/power → constant).

**D3.3 Creation & GPL-level allocation (the upgrade planner).** GPL spends are **irreversible** (no respec), so the allocation is a **recorded static plan** — the D4 build table — executed by an autopilot, not a per-tick argmax: `next_gpl_action(fleet, gpl_free) → Create(name) | Upgrade(creep, power) | Hold`. The kernel walks the table, respects the engine level gates, skips ranks whose fleet-wide target is already met (the Hivemind factory-rank distribution idea, generalized), and **creates operator #2 only when the allowance permits without delaying operator #1's L22 OPERATE_SPAWN-r5 milestone** (0013 D3.1, retained). The *derivation* of the table is EV (each row justifies its Δeffect-e/t-per-level against alternatives — 0013 D3.1's argued table plus §Context's community convergence); the *execution* is deterministic and auditable. Re-ranking the table is an ADR edit, not a runtime decision (§D8 #3 tracks the fleet-mix question). `Hold` is legitimate: banking free GPL costs nothing and an unspendable level (gate-locked) must not force a bad spend.

**D3.4 The cast scheduler (per-tick ops/power application).** Pure kernel, the 0013 D3.3 EDF design made concrete:

```rust
pub fn next_cast(powers: &BuildState, cooldowns: &CooldownState, effects: &EffectState,
                 ops: &OpsLedger, room: &PowerRoomView, posture: Posture) -> Option<CastIntent>
```

Ranking: for each ready (cooldown-clear, ops-affordable, rank>0, in-range) cast, `cast_value_milli = Δeffect_value_milli(power, target) − ops_cost · ops_price_milli`, with an **EDF refresh override**: a standing effect inside its refresh slack (duration remaining < cooldown + travel) jumps the queue — sustained effects must never lapse from greedy value-chasing. Effect values are the engine-derived e/t deltas (REGEN_SOURCE rank r = `+{50,100,150,200,250}[r]/15` e/t; OPERATE_SPAWN = the room's spawn-throughput shadow value from the 0011 lane model; OPERATE_EXTENSION = refill-latency e/t recovered), each duty-cycled. `ops_price_milli` is the ops shadow price: the max marginal e/t-per-ops among currently-unfunded sinks (v0: a constant derived from OPERATE_SPAWN r3's value/ops, sim-swept). GENERATE_OPS is the scheduler's idle default whenever no positive-value cast is ready and the room ops stockpile is below `OPS_STOCKPILE_CAP` (v0 15,000 — the Hivemind-validated number; 0013's 5,000 floor is the *sustained-effects admission* threshold, distinct and kept). Scheduling arithmetic to honor (0013 D3.3, engine math): one operator sustains exactly 3 OPERATE_SPAWN effects (cd 300/dur 1000) — a full RCL8 room; REGEN_SOURCE cd 100/dur 300 sustains both home sources; the whole rotation is ≤ ~40 casts/kilotick against 1000 slots.

**D3.5 The ops ledger.** Ops are a stockpiled resource (0013 D3.2's honest budget: a full RCL8 suite costs ~0.8 ops/t against 0.16/t generation — sustained effects are a *late* state). Ledger policy, all deterministic thresholds: bank to storage above 90% carry, withdraw below 10% (carry is 100·(level+1) — the working buffer, not the vault); sustained-effect classes admit in fixed priority (defense > OPERATE_SPAWN > OPERATE_EXTENSION > REGEN-era extras) only while `room_ops_stock ≥ OPS_FLOOR` (v0 5,000); below the floor, casts are burst-only (0011 group-admission windows, defense spikes). Ops purchases/sales ride ADR 0012's fair-value machinery when that lands (§D8 #5); ops hauls storage↔terminal are ordinary 0044-admitted traffic.

### §D4 Power scope: critical-economy-first (the recorded rollout)

The build order — 0013 D3.1's table, re-validated against competitive practice, adopted as the recorded GPL-allocation plan (execution per D3.3):

| Creep level | Take | EV rationale (why this beats alternatives at that level) |
|---|---|---|
| L1 | **GENERATE_OPS r1** | The faucet: 15 of 19 powers consume ops; an ops-less operator can only haul. |
| L2–L3 | **OPERATE_SPAWN r1→r2** | RCL8 rooms are spawn-throughput-bound (0011); r2 = ×0.7 spawn time = +43% room throughput — the steepest early marginal gain. |
| L4 | **OPERATE_EXTENSION r1** | 2 ops per instant 20%-capacity refill — the cheapest ops→spawn-uptime conversion in the table. |
| L5–L6 | GENERATE_OPS r2, OPERATE_TOWER r1 | Fund the habit; first defensive power on the shelf (cast policy waits for the defensive phase). |
| L7–L9 | **OPERATE_SPAWN r3** (×0.5), GENERATE_OPS r3, OPERATE_LAB r1 | r3 doubles throughput. |
| L10–L13 | **REGEN_SOURCE r1→r3, OPERATE_POWER r1** | The L10 gate opens the zero-ops income powers (+3.3→10 e/t per source) and the GPL accelerator. |
| L14–L21 | OPERATE_SPAWN r4, GENERATE_OPS r4, REGEN_SOURCE r4, OPERATE_LAB r2–r3, OPERATE_TOWER r2 | |
| L22–L25 | **OPERATE_SPAWN r5** (×0.2), GENERATE_OPS r5, then OPERATE_CONTROLLER r1 / OPERATE_FACTORY r1 per posture | The ×5 milestone; the L20/L22 gates open. |

**In scope for autonomous casting from phase P1–P3** (§Migration): GENERATE_OPS, OPERATE_SPAWN, OPERATE_EXTENSION, REGEN_SOURCE, OPERATE_POWER — the powers whose EV is grounded in already-measured bot signals (spawn uptime, refill latency, source income, GPL rate). **Learned early but cast-dormant:** OPERATE_TOWER/OPERATE_LAB (activate with 0013 D5.1 defense doctrine / 0010 lab pipeline respectively). **Deferred entirely:** OPERATE_FACTORY (until 0010 lands and the §D5 branding decision is recorded), OPERATE_STORAGE/TERMINAL/OBSERVER (positive but small; priced when their consumers exist), OPERATE_CONTROLLER (L20+, GCL-push posture), REGEN_MINERAL (with 0010's mineral demand), all DISRUPT/SHIELD/FORTIFY (0013 D5.2 offensive doctrine, post-maturity). Second operator: the REGEN/field specialist per 0013, gated per D3.3.

### §D5 Irreversibles veto, they do not bid

Four acts are one-way and are **recorded policy decisions outside every market** (the 0043 Part-C idiom):

1. **`enableRoom`** — executed once per owned room *when its first operator deploys* (0013 D4's argued policy, adopted): the symmetric-enablement downside is smaller than forfeiting defensive powers in exactly the siege that decides the room, and refusing is not a shield (the attacker enables your room themselves). Logged loud; re-derived from `controller.is_power_enabled()` thereafter (§D1.4).
2. **OPERATE_FACTORY branding** — permanent factory level assignment. Never autonomous in this ADR's phases: stays behind 0010's debug-assert until a recorded (room, level, tick) go/no-go per 0013 D6, then the *cast* automates.
3. **`delete()` / experimentation periods** — never autonomous, full stop. A wrong build is 24h + 1 GPL to fix; the autopilot must never hold that trigger. Console-command territory.
4. **Creation/upgrade** — irreversible GPL spends, governed by the D4 recorded table (D3.3); the table is the veto boundary, the autopilot just walks it.

### §D6 Architecture wiring (reuse, no parallel machinery)

- **`PowerCreepOperation`** (new `OperationData` variant — serialization-transparent, no WFV): the account-scoped owner. Runs the §D1 reconciler, the D3.3 GPL autopilot, and launches one **`OperatorMission`** per deployed/assigned operator (new `MissionData` variant; coordinator shape per `SourceKeeperFarmMission`). The mission provides context (assignment, anchor tile, renew target); the creep's **`OperatorJob`** (new `JobData` variant) owns intents — movement via the rover facade ([[no-one-off-pathfinding-algorithms]]), casts from the D3.4 kernel, ops `withdraw`/`transfer` as ordinary store intents ("missions provide context; the creep's job owns its intent," ADR 0018 §2 #8 — the squad-path inversion is not replicated).
- **Kernels** in `screeps-econ-decision/src/power_policy.rs`: `power_process_bid`, `next_gpl_action`, `next_cast`, `ops_ledger_admit`, `assignment` + the `PowerConsts` struct — pure, integer, BTreeMap-ordered, unit-tested, shared verbatim with any future sim consumer (the 0040 §D5 seam discipline).
- **`powerspawn.rs`** keeps its mission role; its deposit generator migrates to the numeric-bid path priced by `power_process_bid` (D3.2). `TransferTarget::PowerSpawn` and the haul lanes are untouched.
- **0011 retrofit** (declared by 0013 D6, consumed here): the spawn lane model reads **effective** `needTime` from standing OPERATE_SPAWN effects, and OPERATE_EXTENSION's step-function refills are excluded from the refill-trend stall detector.
- **Foreman**: the operator anchor tile (Chebyshev ≤3 to spawns + storage, inside the rampart shell) joins the plan like 0010's boost tile — 0013 D3.3, unchanged.
- **Observability**: seg-57 gains `gpl`, `power_processed`, `ops_stock`, `operator_uptime` (fraction of ticks each sustained effect is live — the metric that proves the multipliers), `operator_deaths` (target 0), `power_income` (0013 D6's list); one console line per operator-room (state, TTL, next cast, ops). Labelled Memory/format addition per 0006 segment discipline — **segments, not WFV**.
- **Governor** (0004): defense casts never shed; economy casts and the reconciler's non-adoption work shed at Conserve. **Kill-switch**: `features.power.operators` (default off until P1 validates), plus `features.power.process_bid` for the D3.2 migration — each stops new actions without stranding a deployed operator (renew/retreat never shed or gate).

### §D7 CPU & tick-safety

Reconciler: O(#account creeps) ≤ a handful/tick. Cast rotation: ≤ ~40 intents/kilotick/operator (~0.008 CPU/t, 0013's arithmetic). Kernels are integer table-walks. No new panic surface: all game-object resolution is handled-`None`; entity teardown routes through `EntityCleanupQueue`; nothing in the layer serializes, so the dangling-ref class ([[ecs-dangling-ref-serialize]]) cannot reach it. Runs inside 0005's tick containment.

### §D8 Open decisions (numbered, operator-veto-pending)

1. **v0 constants** — `RENEW_OPPORTUNISTIC=4500`, `RENEW_MARGIN=200`, `OPS_FLOOR=5000`, `OPS_STOCKPILE_CAP=15000`, carry bank thresholds 10%/90%, `H_AMORT`, `H_RELOC=10000`, `room_power_score` weights, `ops_price_milli` v0. Approve the shapes now; numbers land as a reviewed diff, swept per EP-4.6.
2. **Cross-lane power-EV normalization** — expressing operator value against `value_e` lanes is blocked on the reserved par-rescale (0042 §Consequences (4), Flaw 3). Until then the power layer's only market surface is D3.2. Revisit when the normalization ADR lands.
3. **Fleet mix beyond operator #2** — ops-throughput-per-GPL favors many mid-level creeps; deep ranks need deep creeps (§Context). The D4 table covers creeps #1–#2; a fleet-mix policy for GPL > ~50 is future work.
4. **OPERATE_FACTORY branding go/no-go** — decided with 0010+0012 per 0013 D6; until recorded, the power stays uncast (§D5).
5. **Ops market valve** — buy ops under 0012 price guards (they trade cheap relative to spawn-throughput value per the community analysis); sell never while the flywheel is hungry; wartime embargo per 0013 D6. Lands with 0012 consumption.
6. **Sim scope** — `screeps-econ-engine` has no power systems; faithful T_recover-style power scenarios need the 0006 `PowerFixture` (mongo-seeded GPL/creeps/enabled flags) plus engine-side ops/effect modeling. Deferred: P0–P3 validate via kernel tests + private-server soak; the sim buildout is its own later increment.
7. **Processing-energy arbitration** — the D3.2 surplus veto is the interim stand-in; when ADR 0014 lands, posture owns the gate (0013 D2, unchanged). Declared here so 0014 inherits the demand: 50 e/t base, up to 300 e/t with OPERATE_POWER r5.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Zero-state per-tick reconciler + pure kernels + recorded build table (chosen)** | WFV-reset/VM-reset/deploy all no-ops; duplicates impossible by construction; kernels host-testable; matches the codebase's strongest discipline | Per-tick rederivation cost (measured trivial); assignment must be a pure function (it is) |
| Serialize power-creep state in the WFV payload | "Feels" like the other layers | Creates the orphan problem this ADR exists to prevent; duplicates server-side truth; every field is derivable — pure downside |
| Adopt only creeps matching our naming convention | Simpler matching | Orphans manually-created or renamed creeps — the exact failure mode; name-agnostic adoption costs nothing |
| Operators as Squad-Manager squad members | One lifecycle owner | Rejected in 0013: wrong lifetime model (5000t + free renew + 8h wall-clock death vs 1500t prespawn doctrine); no spawn-lane contention to manage |
| Per-tick argmax GPL spending (no recorded table) | Maximally adaptive | GPL spends are irreversible with no respec; a transient mis-signal permanently malforms the build. Veto-class decisions get recorded plans (§D5) |
| Full market bids for casts/upgrades in existing lanes now | One uniform market | Flaw 3 band saturation — operator EV cannot be expressed pre-normalization; forcing it collapses lanes to ties (0042's blocker, verbatim) |
| Defer all of this until 0013's bank-farming half is proven | One dependency chain | Processing + operators don't require bank kills (slow accrual + 0012 power purchases feed GPL); gating on the combat stack idles free GPL. The halves are independent by 0013's own design |
| Hysteresis latches on relocation/renew | Fewer transitions | No observed oscillation; switching-cost-priced per-tick optimum is the ratified default ([[prefer-per-tick-optimal-over-hysteresis]]) |

## Consequences

**Positive.** The bot gains its first fully-autonomous account-level asset layer, orphan-proof under the reset-anytime policy — the prerequisite for running power on MMO where WFV resets are routine. The 0043 B4 REFUTED lane converts to a real EV kernel (one fewer sanctioned-coarse exception). The engine multipliers 0013 quantified (×5 spawn, +33 e/t/room, refill-stall elimination, GPL acceleration) become reachable on the existing seams with zero new serialized state and zero WFV bumps. Everything decision-bearing is a pure kernel.

**Negative / new risks.** Real-time coupling enters the design (8h/24h wall-clock — the harness compresses ticks, not wall-clock; death-avoidance is kernel-tested, not scenario-proven). Ops mis-scheduling (sustaining OPERATE_SPAWN while the defense floor is empty) is the new mis-tiering risk — mitigated by the fixed admission priority + the ops floor + a harness invariant. Enabled rooms are power-attackable (0013 D4's accepted, mitigated trade). Value concentrates in 1–2 by-policy-unkillable units; degradation is made visible by `operator_uptime`/`operator_deaths`. One more system with tunables — bounded: no serialized state, pure kernels, config-driven, kill-switched.

**CPU & tick-safety:** §D7 — noise-level.

## Incremental Migration Path

Stable seams: the account API (the store), the `OperationData`/`MissionData`/`JobData` variant seam (serialization-transparent), the numeric-bid transfer lane, the `power_policy` kernel boundary. **No step bumps WFV.**

1. **P0 — Reconciler + lifecycle core (Breaking: Behavioral).** `PowerCreepOperation` + reconciler (§D1), creation/upgrade autopilot walking the D4 table (§D3.3), spawn/assignment, renew discipline, enableRoom-on-deploy, GENERATE_OPS + ops banking as the only casts. Feature-gated default-off. **Validate:** kernel tests — adoption from a synthetic account list (incl. foreign-named creeps), duplicate-impossibility property (creation decision under a simulated wipe re-derives identically), table-conformance of `next_gpl_action` across GPL 1→60, renew-deadline math; live: private server, forced `deserialize_world` reset mid-run → operator re-adopted, renews on schedule, zero duplicate `create` intents, `operator_deaths == 0`.
2. **P1 — Cast scheduler + spawn powers (Breaking: Behavioral).** `next_cast` EDF kernel; OPERATE_SPAWN + OPERATE_EXTENSION live; 0011 effective-`needTime` retrofit + stall-detector exclusion. **Validate:** kernel — EDF refresh never lets a sustained effect lapse (property test over cooldown/duration grids); live soak — 3 spawn effects ≥95% uptime, spawn-throughput KPI ≥1.9× baseline at r3 (0013 P3's gate, inherited).
3. **P2 — The processing bid (Breaking: Behavioral).** `power_process_bid` + numeric-path deposit generator (retiring `new_tier` in `powerspawn.rs`, un-inverting its priority map); OPERATE_POWER at L10. **Validate:** kernel — bid monotonicity (↓ in GPL, ↑ in next-upgrade EV), scale audit (par-lane values only); live — processing yields to refill under stress and resumes on surplus (the 0013 P1 oscillation scenario), seg-57 `power_processed`/`gpl` flowing. Cite-and-close 0043 B4's REFUTED line.
4. **P3 — REGEN_SOURCE era + relocation polish (Breaking: Behavioral).** L10 powers cast; `room_power_score` + switching-cost relocation; full seg-57 battery + console line. **Validate:** live — source income delta matches the rank table (+3.3 e/t r1) within CI; relocation fires only on priced deltas (flap counter 0).
5. **P4 — Deferred menu (each evidence-gated, separately decided):** defensive casts (0013 D5.1) with the threat model's `HostileOperator` class; OPERATE_FACTORY per §D8 #4; second-operator specialization; ops market valve (§D8 #5); DISRUPT doctrine (0013 D5.2); the sim buildout (§D8 #6).

**Breaking-change summary:** every step is **Behavioral** (EP-5.2); no serialized shape is touched anywhere — the layer's defining property. Seg-57 additions are labelled segment-side Memory/format (additive, no reset, outside WFV). `enableRoom` and (later) factory branding are irreversible **in-game** actions, each executed once, logged, and governed by §D5 — not code breaks. The running bot never breaks mid-increment: P0 runs dark behind `features.power.operators`.

## Harness validation (red → green)

(a) **Adoption:** synthetic account snapshots (alive/unspawned/cooling/foreign-named/deleted) → reconciler produces the correct entity set, idempotent across repeated ticks. (b) **No-duplicate property:** for all fleet states, `next_gpl_action` after a simulated memory wipe equals its pre-wipe output. (c) **Never-die:** renew-deadline kernel over (TTL × distance × threat) grids — no reachable state where casting outranks a due renewal. (d) **EDF:** no sustained-effect lapse across cooldown/duration/ops-shortage matrices; GENERATE_OPS fills genuine idle only. (e) **Bid audit:** `power_process_bid` monotonicity + par-scale bounds; the D3.5 admission priority (defense before economy) asserted as an invariant. (f) **Table conformance:** the D4 build order reproduced exactly from GPL 1→60 including gate-locked `Hold`s. **Matrix axes:** GPL ∈ {1, 8, 15, 23, 40}; fleet ∈ {0, 1, 2 operators}; reset ∈ {none, wiped}; posture ∈ {peace, stress}. **Determinism:** all kernels integer + BTreeMap-ordered; the existing fences extend to `power_policy` (spread-0 over 5 seeded runs). Live gates ride the private-server soak per phase; MMO only on explicit go-ahead ([[deploy-use-screeps-pack]]).

## Cross-references

- Reconciler idiom to mirror: `screeps-ibex/src/creep.rs:52` (`WaitForSpawnSystem` by-name lookup — run unconditionally, not only for pending spawns).
- Deposit generator to migrate: `screeps-ibex/src/missions/powerspawn.rs:87-147` (band path `new_tier`); numeric-path example `missions/localsupply/room_transfer.rs:375-377`.
- Mission launch site: `screeps-ibex/src/missions/colony.rs:246-253`; coordinator shape `missions/sourcekeeperfarm.rs:171-181`.
- Kernel home: `screeps-econ-decision/src/` (`sink_economics.rs`, `spawn_policy.rs`, `mineral_value.rs` — the trust-gated template); new `power_policy.rs`.
- Band/currency: `spawn_policy.rs:39-58` (`SPAWN_BID_*`, `BID_SCALE`); ADR 0043's scale-migration constraint — the mixed-scale trap.
- WFV mechanics: `screeps-ibex/src/game_loop.rs:746` (`WORLD_FORMAT_VERSION`), `:753+` (`deserialize_world` loud reset), `segments.rs:62` (segments outside WFV).
- Inert variants awaiting producers: `military/objective_queue.rs:71-77` (`FarmKind::PowerBank`), `game_loop.rs:701`.
- Engine rulebook: ADR 0013 "Engine ground truth" + POWER_INFO table; external sources cited inline in §Context.
