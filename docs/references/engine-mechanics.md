# Screeps Engine Mechanics — Ground-Truth Reference

**This is the single place to look before guessing a game mechanic.** Every claim cites engine source. If a mechanic you need is not here, read the engine, then add it here with a citation — do not design from folklore or from docs.screeps.com alone (the official docs are frequently imprecise; see §8).

**Where citations point.** The engine is cloned locally:

- `engine/…` = `C:\code\screeps-engine\src\…` (clone of [screeps/engine](https://github.com/screeps/engine))
- `common/…` = `C:\code\screeps-common\lib\…` (clone of [screeps/common](https://github.com/screeps/common)) — `common/constants.js` is the canonical constants file; `common/strongholds.js` defines NPC stronghold templates.
- `driver/…` = `C:\code\screeps-driver\lib\…` (clone of [screeps/driver](https://github.com/screeps/driver)) — the isolated-vm runtime where Memory, RawMemory segments, intent-CPU pricing, and the bucket actually live (§9).

Load-bearing numbers below were spot-verified against these clones on 2026-06-09 (files opened: `creeps/tick.js`, `creeps/intents.js`, `creeps/rangedMassAttack.js`, `towers/attack.js`, `towers/intents.js`, `_damage.js`, `movement.js`, `spawns/renew-creep.js`, `spawns/_charge-energy.js`, `creeps/upgradeController.js`, `global-intents/market.js`, `constants.js`; segments/power/controller pass: `driver/runtime/runtime.js`, `driver/runtime/make.js`, `driver/runtime/data.js`, `driver/index.js`, `driver/bulk.js`, `power-creeps/*.js`, `global-intents/power/*.js`, `creeps/claimController.js`, `creeps/reserveController.js`, `creeps/attackController.js`, `creeps/generateSafeMode.js`, `creeps/signController.js`, `controllers/tick.js`, `controllers/activateSafeMode.js`, `controllers/unclaim.js`, `engine/utils.js`). Remaining citations come from the same research pass; all carry file:line so they can be re-checked in seconds.

**Relation to the design docs.** ADRs 0001–0014 (`../design/`) take these mechanics as given. In particular: the tick-concurrency model (§1) underpins ADR 0008's predictive-combat stance and ADR 0005's scheduling model; spawn throughput (§3) bounds ADR 0008's force planning and ADR 0007's hauler sizing; container/road decay (§7) feeds ADR 0007/0009 remote-cost math. The 2026-06-09 extension adds §2.12 (controller warfare — grounds ADR 0008's `Downgrade{room}`/`Claim{room}` objectives), §6.3/§6.6 (the power rulebook — grounds ADR 0013; doctrine lives there, mechanics here), and §9 (segments & intent CPU — grounds ADR 0002's segment registry and ADR 0004's budget math). Behavioral claims here are testable on the ADR 0006 private-server harness — when a mechanic is disputed, write a harness scenario, don't argue.

---

## 1. Tick & intent-resolution model (concurrency semantics)

This section is the foundation for all combat and scheduling reasoning. **HARD CONSTRAINT: you can never observe and react to a same-tick enemy action — only predict.**

### 1.1 Global tick sequence (`engine/main.js`)

1. **All players' code runs first.** Users are queued; the tick waits for every user to finish (`engine/main.js:32-41`). The runner executes one user's code and persists Memory + the intent list (`engine/runner.js:9-51`, intents saved at `:42-44`). Player code for tick T therefore sees the world state written at the end of tick T−1.
2. **All rooms are processed** independently, in parallel workers off a rooms queue (`engine/main.js:42-56`; `engine/processor.js:549-592`). Each room loads its intents/objects/terrain fresh (`processor.js:555-578`).
3. **Global stage**: inter-room creep transfers, power, and ALL market deals/orders resolve once, after every room (`engine/main.js:62-66`, `engine/processor/global.js:32-49`). Note: terminal `send` is a *room* intent; market `deal` is *global* — they land at different points in the tick.
4. `incrementGameTime` (`engine/main.js:72-76`).

### 1.2 Per-room processing order (`engine/processor.js` `processRoom`)

1. **Pre-pass** (`processor.js:43-171`): actionLogs reset, `_ticksToLive` computed, keepers/invaders/cores collected.
2. **NPC AI generates intents inside the processor** — nuke pretick (cancels spawn intents, `:176-178`), keeper creeps (`:180-192`), invaders (`:194-206`), invader core (`:208-219`). NPC AI runs with same-tick knowledge — it is *not* subject to the snapshot constraint players have.
3. `_calc_spawns` disables over-RCL spawns/extensions (`processor.js:221-223`, `engine/processor/intents/_calc_spawns.js:8-23`).
4. **Intent resolution loop** (`processor.js:227-322`): per user, per object — creeps (`:250-251`), towers (`:256-257`), spawns, labs, links, etc. **Iteration order across users/objects is JS hash order — treat it as unordered; never design a mechanic that depends on it.**
5. **Movement conflict resolution**: `movement.check()` (`processor.js:324`) — after ALL intents are registered.
6. **Object tick loop** (`processor.js:342-483`): `creeps/tick` per creep — moves execute and damage/heal apply here (§1.3).
7. Bulk DB writes (`processor.js:494-497`).

**All intent validation (range, hits, energy, safe mode) happens in step 4 against tick-start positions** — movement has not executed yet. This is what makes kiting work (§2.10).

### 1.3 Deferred damage/heal and the netting order (`engine/processor/intents/creeps/tick.js`) — VERIFIED

Damage to creeps/powerCreeps accumulates in `target._damageToApply` (`engine/processor/intents/_damage.js:16-24`); heals accumulate in `_healToApply` (`creeps/heal.js:28`, `creeps/rangedHeal.js:28`, `towers/heal.js`). Damage to **structures** applies immediately and sequentially during step 4 (`_damage.js:32`) — a destroyed structure rejects later intents that same tick (`!target.hits` guards, e.g. `creeps/attack.js:27-29`).

Per creep, in its tick handler, the exact order is (`creeps/tick.js:118-150`):

| Step | Line | Action |
|---|---|---|
| 1 | `:118` | `oldHits = hits` |
| 2 | `:120-123` | apply `_damageToApply` via `_applyDamage` (tough-boost reduction happens here, §2.4) |
| 3 | `:125-128` | add `_healToApply` |
| 4 | `:130-132` | clamp to `hitsMax` |
| 5 | `:134-135` | death check |
| 6 | `:137-149` | `_recalc-body` + drop overflow resources |

**Damage first, then heal, then the death check.** A creep at 100 hits taking 1,000 damage and 950 heal the same tick ends at 50 and lives. Pre-heal at full HP is never wasted against same-tick damage (clamping happens only after netting). Tower heal flows into the same `_healToApply`, so it nets identically.

Also in `creeps/tick.js`: `movement.execute` (`:50`), edge-exit to the next room (`:52-78`), age-death at `gameTime >= ageTime-1` (`:93-95`), fatigue regen −2×MOVE-parts (boost-multiplied) every tick (`:105-108`).

### 1.4 Per-creep intent conflict matrix (`engine/processor/intents/creeps/intents.js:3-13`) — VERIFIED

An intent is **silently dropped** if any intent in its blocker list was also issued (`creeps/intents.js:21-23`):

| Intent | Dropped if also issued |
|---|---|
| `heal` | nothing — always wins |
| `rangedHeal` | heal |
| `attackController` | rangedHeal, heal |
| `dismantle` | attackController, rangedHeal, heal |
| `repair` | dismantle, attackController, rangedHeal, heal |
| `build` | repair, dismantle, attackController, rangedHeal, heal |
| `attack` | build, repair, dismantle, attackController, rangedHeal, **heal** |
| `harvest` | attack, build, repair, dismantle, attackController, rangedHeal, heal |
| `rangedMassAttack` | build, repair, rangedHeal |
| `rangedAttack` | rangedMassAttack, build, repair, rangedHeal |

`move`, `drop`, `transfer`, `withdraw`, `pickup`, `pull`, `say`, `suicide` have **no** conflicts.

**Legal world-class same-tick combos:**
- `move + heal + rangedAttack` (heal does NOT block rangedAttack — only rangedHeal does)
- `move + heal + rangedMassAttack`
- `move + attack + rangedAttack` (or `attack + rangedMassAttack`) — hybrid melee/ranged fires both
- **NOT** `attack + heal`; **NOT** `heal + rangedHeal`; **NOT** `rangedAttack + rangedMassAttack` (rMA wins)

### 1.5 Towers act once per tick — VERIFIED

A tower performs exactly ONE action per tick, priority **heal > repair > attack** if multiple are issued (`engine/processor/intents/towers/intents.js:4-9`).

### 1.6 Movement resolution (`engine/processor/intents/movement.js`) — VERIFIED

- A `move` intent only registers a desired tile (`creeps/move.js` → `movement.add`, `movement.js:84-98`). Moving requires `fatigue == 0` and a live MOVE part, or being pulled (`canMove`, `movement.js:11-14`).
- **Tile contention** (`movement.js:115-141`): winner by `rate1` = number of creeps moving into the contender's vacated tile (forced to 100 if it's a swap, `:126-127`), then pulled status, then pulling status, then **moves/weight ratio** (`:135,139`). Heavier/slower creeps lose ties.
- **Stationary creeps are hard obstacles — there is no pushing** (`checkObstacleAtXY`, `movement.js:22`). But a creep vacating a tile lets a follower in the same tick (train movement — the `!objects[i._id]` check at `:22`); a failed leader cascades failure down the whole train (`removeFromMatrix`, `:154-165`).
- During **safe mode**, hostile stationary creeps do NOT block the owner's creeps (`movement.js:22` safe-mode clause; invoked at `processor.js:324`).
- **Fatigue gained per step** = (#non-MOVE, non-CARRY parts + loaded-CARRY count) × terrain rate; rate = 1 road / 2 plain / 10 swamp (`movement.js:204-240`). Stepping onto a room edge zeroes fatigue (`:242-246`). Each MOVE part removes 2×(boost multiplier) fatigue per tick (`creeps/tick.js:105-107`). Pull chains shift fatigue to the chain head after the puller's own MOVE regen (`creeps/_add-fatigue.js`).
- **Sustained 1 tile/tick** iff `2 × Σ(MOVE_boost_mult) ≥ (otherParts + loadedCarry) × terrainRate`.
- Bonus: stepping onto a **hostile construction site** destroys it, dropping `floor(progress/2)` energy (`movement.js:224-233`) — blocked while the room's safe mode is active and the mover isn't the owner.

### 1.7 Design implications (cross-ref ADR 0008, ADR 0005)

- **Predictive play is mandatory**: pre-heal the member you predict will be focused; path melee to where the kiter *will be*; pre-spawn replacements (creeps die on a 1500-tick clock, §3).
- **Reactive healing is structurally one tick late** — that one tick is exactly the burst window towers + boosted attackers exploit.
- Same-tick ordering *within* a step is hash order: never rely on "my tower fires before his creep moves" style reasoning. The only reliable orderings are the numbered stages above.

---

## 2. Combat math

### 2.1 Base action powers (`common/constants.js`) — VERIFIED

| Action | Power/part | Range (Chebyshev) | Constant line |
|---|---|---|---|
| attack | 30 | ≤1 (`creeps/attack.js:21`) | `:123` |
| rangedAttack | 10 | ≤3 (`creeps/rangedAttack.js:21`) | `:125` |
| rangedMassAttack | 10/10/4/1 at range 0/1/2/3 (§2.7) | ≤3 | `:125` + rate table |
| heal | 12 | ≤1 | `:126` |
| rangedHeal | 4 | ≤3 | `:127` |
| dismantle | 50 | ≤1 | `:121` |
| repair | 100 | ≤3 | `:120` |

Per-part power = base × boost multiplier; only parts with `hits > 0` count (`engine/utils.js:623-636` `calcBodyEffectiveness`).

### 2.2 Tower damage/heal/repair falloff — VERIFIED

`amount = POWER × (1 − 0.75 × (clamp(range,5,20) − 5) / 15)`, floored after power-creep effects (`engine/processor/intents/towers/attack.js:32-46`). **Falloff is linear from range 5 to 20, not stepped.** Constants: `TOWER_POWER_ATTACK 600 / HEAL 400 / REPAIR 800`, `TOWER_OPTIMAL_RANGE 5`, `TOWER_FALLOFF_RANGE 20`, `TOWER_FALLOFF 0.75`, `TOWER_ENERGY_COST 10` per action, capacity 1000 (`common/constants.js:247-254`).

| Range | Attack | Heal | Repair |
|---|---|---|---|
| ≤5 | 600 | 400 | 800 |
| 10 | 450 | 300 | 600 |
| 15 | 300 | 200 | 400 |
| ≥20 | 150 | 100 | 200 |
| per tile (5→20) | −30 | −20 | −40 |

Towers cannot target spawning creeps (`towers/attack.js:18-20`); shots redirect to a rampart on the target tile (§2.6). Energy cost is flat 10/action at any range — there is no reduced-power-when-low-energy mechanic (`towers/attack.js:24,54`).

### 2.3 Boost multipliers (`common/constants.js:617-731`) — full table

| Part | Action affected | T1 | T2 | T3 |
|---|---|---|---|---|
| WORK | harvest | UO ×3 | UHO2 ×5 | XUHO2 ×7 |
| WORK | build/repair | LH ×1.5 | LH2O ×1.8 | XLH2O ×2 |
| WORK | dismantle | ZH ×2 | ZH2O ×3 | XZH2O ×4 |
| WORK | upgradeController | GH ×1.5 | GH2O ×1.8 | XGH2O ×2 |
| ATTACK | attack | UH ×2 | UH2O ×3 | XUH2O ×4 |
| RANGED_ATTACK | rangedAttack + rMA | KO ×2 | KHO2 ×3 | XKHO2 ×4 |
| HEAL | heal + rangedHeal | LO ×2 | LHO2 ×3 | XLHO2 ×4 |
| CARRY | capacity | KH ×2 | KH2O ×3 | XKH2O ×4 |
| MOVE | fatigue removal | ZO ×2 | ZHO2 ×3 | XZHO2 ×4 |
| TOUGH | damage taken (multiplier) | GO ×0.7 | GHO2 ×0.5 | XGHO2 ×0.3 |

### 2.4 TOUGH damage-reduction semantics (`creeps/tick.js:7-29` `_applyDamage`) — VERIFIED

Applied at netting time (NOT at intent time), walking the body **from index 0**: each boosted part with a `damage` ratio absorbs `part.hits / ratio` of incoming damage while negating `(1 − ratio)` of the absorbed share; unboosted parts pass damage through at ratio 1 (`:16-23`). Final: `hits -= damage − round(damageReduce)` (`:26-28`). One full-hits XGHO2 TOUGH part (100 hits) eats 333 incoming damage and negates 233 of it. Reduction only happens while boosted parts still have hits, and the loop runs only if ANY body part is boosted (`:11`).

### 2.5 Body-part damage order — parts die front-to-back

`creeps/_recalc-body.js:8-18` redistributes total hits filling **from the last body part backwards**, so index 0 is destroyed first. This is why TOUGH-first body ordering works: `_applyDamage` walks front-to-back (§2.4), matching the death order.

### 2.6 Melee hit-back, ramparts, redirects — VERIFIED

- Melee-attacking a creep with live ATTACK parts triggers automatic counter-damage equal to the target's full melee power — **unless the attacker stands on a rampart (ANY rampart, ownership not checked)** (`engine/processor/intents/_damage.js:14-21`), or the room controller belongs to the attacker's user and safe mode is active (`:39-41`). PowerBanks return 50% of damage dealt (`:35-37`; `POWER_BANK_HIT_BACK 0.5`, `common/constants.js:264`).
- Creep `attack`, `rangedAttack`, tower `attack`, and `dismantle` ALL redirect to a rampart on the target's tile (`creeps/attack.js:33-36`, `creeps/rangedAttack.js:33-36`, `towers/attack.js:27-30`, `creeps/dismantle.js:27-29`). **The redirect does not check rampart ownership** — attacking an enemy creep camped on YOUR rampart damages your own rampart.
- Heal never redirects. Spawning creeps cannot be targeted (`creeps/attack.js:24-26` and equivalents).
- Rampart hitsMax by RCL: 300K / 1M / 3M / 10M / 30M / 100M / 300M for RCL 2–8; decay 300 hits per 100 ticks (`engine/processor/intents/ramparts/tick.js:26-40`, `common/constants.js:131-134`). Walls: max 300M, no decay (`:149-150`).

### 2.7 rangedMassAttack (`creeps/rangedMassAttack.js`) — VERIFIED

- Per-target damage = `round(totalRangedPower × {0:1, 1:1, 2:0.4, 3:0.1}[range])` (`:32,53`) — 10/10/4/1 per unboosted part at range 0–3.
- Targets only objects **with an owner** (any owner ≠ self — allies included; there is no engine ally concept) plus powerBanks (`:26-30`). **It cannot damage neutral walls, roads, or containers.**
- Skips ANY non-rampart object standing on a rampart-covered tile — creeps *and* structures — but can target the rampart itself (`:38-40`). Skips spawning creeps (`:44-46`) and FORTIFY/INVULNERABILITY-affected targets (`:47-49`).

### 2.8 Nukes (`engine/processor/intents/nukes/`)

- Flight 50,000 ticks, range 10 rooms; nuker load: 300K energy + 5K ghodium, 100K cooldown (`common/constants.js:345-354`).
- The tick before landing, all `createCreep` intents in the room are cancelled (`nukes/pretick.js:8-14`).
- On landing (`nukes/tick.js:16-78`): **EVERY creep in the room dies** — whole room, ramparts do not save creeps, no tombstones (`:23`; `creeps/_die.js` skips tombstones for `EVENT_ATTACK_TYPE_NUKE`); powerCreeps zeroed; construction sites / dropped energy / tombstones / ruins wiped; in-progress spawns cancelled.
- Structure damage 5×5: 10M at ground zero, 5M at range 1–2 (`:44`; `NUKE_DAMAGE`, `common/constants.js:351-354`). A rampart on a tile absorbs first: the structure under it takes `damage − rampart_pre-damage_hits` (`:49-52`) — a 10M rampart fully absorbs a range-1-2 hit; stacked nukes are the classic counter. FORTIFY/invulnerability does NOT block nuke damage (`_damage.js:26`).
- Landing **cancels active safe mode** and blocks controller upgrades for 200 ticks (`:63-75`; `CONTROLLER_NUKE_BLOCKED_UPGRADE 200`, `common/constants.js:240`).

### 2.9 Safe mode and spawn-blocking

- During safe mode, hostiles in the room cannot attack/rangedAttack/rMA/heal/dismantle at all (guard in every combat intent, e.g. `creeps/attack.js:30-32`) and do not block the owner's movement (`movement.js:22`). Safe-mode **charges** are bought by a creep with `generateSafeMode` (1,000 ghodium per charge, `creeps/generateSafeMode.js:19-24`) and granted free on each controller level-up (`creeps/upgradeController.js:77`); **activation** consumes a charge and is gated by cooldown, upgrade-block, and the downgrade clock — §2.12 covers the gates an attacker can force shut. Duration 20,000, cooldown 50,000 (`engine/processor/intents/controllers/activateSafeMode.js:11-22`, `common/constants.js:242-244`).
- **You cannot spawn-block with creeps**: if every exit tile is blocked and a hostile occupies a candidate tile, the newborn **kills the hostile and spawns on its tile** ("spawnstomp", `engine/processor/intents/spawns/_born-creep.js:53-77`). If some non-chosen direction is free, the spawn waits instead (`:55-67`); blocked spawns retry every tick (`spawns/tick.js:26`).

### 2.10 Derived sustain math (kiting, drain, quads)

These are derivations from the verified constants above — the arithmetic, not the constants, is ours:

- **Kiting**: all attack range checks use tick-start positions (§1.2 step 4); a ranged kiter at range 3 issuing `move`(away) + `rangedAttack` + `heal`(self) every tick is untouchable by an equal-speed melee chaser. Chasers must path to the *predicted* destination.
- **Tower drain**: a shot costs 10 energy for 150 damage at range ≥20. Healing 150 hits costs 12.5 unboosted HEAL part-ticks (≈3.1 XLHO2). One creep with 13 boosted HEAL parts (13×48 = 624 hp/tick self-heal) out-sustains four towers at max falloff (600/tick), forcing 40 energy/tick of drain. A full RCL8 6-tower volley: 900/tick at range 20, 3,600/tick at range ≤5 — never park inside optimal range.
- **Quad vs towers**: max solo heal = 50 × 12 × 4 = 2,400 hp/tick < 3,600 (6 towers @ optimal) — **no solo creep tanks an RCL8 core**. A 4-creep quad cross-healing the focused member (e.g. 4 × 25 boosted HEAL = 4,800 hp/tick pooled) out-sustains 6 towers at ANY range, before XGHO2 tough mitigation (×0.3 while tough lives → required heal ~1,080). This is why quads are the meta unit, and why ADR 0008 treats cohesion as an invariant, not a preference. Defenders must spike above the pooled heal in one pre-positioned tick and/or focus the member with least heal coverage.

### 2.11 NPC opponents (predictable by design)

NPC AI runs inside the processor with same-tick knowledge (§1.2 step 2) — it reacts faster than any player can, but it is deterministic and readable:

- **Invader cores / strongholds**: core 100K hits (`common/constants.js:842`); deploys at `deployTime` (`engine/processor/intents/invader-core/stronghold/stronghold.js:26`); lifetime 75,000 ±10% via `EFFECT_COLLAPSE_TIMER` (`:27-35`; `STRONGHOLD_DECAY_TICKS`, `constants.js:850`). Templates bunker1–5 (`common/strongholds.js:3-246`): towers per level 1/2/3/4/6, fully ramparted; rampart hits by level 100K/200K/500K/1M/2M (`constants.js:849`). Tower AI focuses closest (L1–3) or the target taking maximum computed damage (L4–5 `focusMax`, `stronghold.js`); L5 auto-fortifies ramparts against incoming nukes every 10 ticks (`stronghold.js:332-360`). Defender spawn time is level-keyed ticks/part {2:6, 3:3, 4:2, 5:1} (`invader-core/create-creep.js:60-64`, `constants.js:843-845`). Loot tables: `strongholds.js:248-271`.
- **Keeper lairs** (`engine/processor/intents/keeper-lairs/tick.js`): 300-tick respawn timer starts once the keeper is dead or damaged below its full 5,000 hits (`:12-16`); body 17×TOUGH, 13×MOVE, 10×(ATTACK+RANGED_ATTACK) (`:25-50`). Keeper AI (`creeps/keepers/pretick.js`): melee lowest-hits hostile at range 1; rMA when aggregate mass damage >13, else rangedAttack lowest-hits in range 3; camps its assigned source. Pre-spawn keeper-killers timed to the 300-tick respawn.

### 2.12 Controller warfare (claim / reserve / attackController / downgrade) — VERIFIED

Grounds ADR 0008's planned `Downgrade{room}` / `Claim{room}` objectives. Each fact carries its tactical implication.

**claimController** (`engine/processor/intents/creeps/claimController.js`):
- Needs ≥1 live CLAIM part (`:28-30`), range ≤1 (`:19`), controller at level 0 (`:25`), and no foreign reservation (`:31-33`) → an enemy reservation must be attacked down or allowed to expire before a claim can land.
- GCL gate inside the processor: `user.gcl ≥ calcNeededGcl(claimedRooms + 1)` = `1,000,000 × claimedRooms^2.4` (`:41-43`; `engine/utils.js:661-663`) → an over-GCL claim fails **silently** server-side; the bot must pre-check GCL itself.
- Success sets level 1, clears reservation and downgradeTime (`:47-53`). CLAIM bodies live 600 ticks and cost 600 energy/part (`common/constants.js:105,112`).

**reserveController** (`creeps/reserveController.js`):
- +1 reservation tick per live CLAIM part per intent-tick (`CONTROLLER_RESERVE 1`, `constants.js:236`; `:30`); a fresh reservation starts at `gameTime+1` (`:35-40`); rejected on owned controllers or foreign reservations (`:26-28`).
- Cap 5,000 (`CONTROLLER_RESERVE_MAX`, `constants.js:237`) and **an overshooting intent is rejected entirely** — no partial credit, same pattern as renew (`:43-45`) → size/schedule reservers so ticks aren't wasted bouncing off the cap.
- The reservation decays 1/tick and is cleared at endTime (`controllers/tick.js:10-12`); reserved sources hold 3,000 (§7.3) → a 2-CLAIM reserver (net +1/tick while present) sustains a remote indefinitely.

**attackController** (`creeps/attackController.js`):
- Gates: range ≤1 (`:19`); target must be owned **or** reserved (`:22-24`); rejected while the room's safe mode is active (for hostiles) **or while `upgradeBlocked` is running** (`:25-28`); `EFFECT_INVULNERABILITY` blocks it (invader-core deploy window) (`:29-31`).
- vs a **reservation**: `endTime −1` per live CLAIM part per tick (`:33-40`) — symmetric with reserving, **no cooldown** → a bigger CLAIM body strips a remote's reservation faster than its owner rebuilds it, every tick.
- vs an **owned controller**: `downgradeTime −300` per CLAIM part (`CONTROLLER_CLAIM_DOWNGRADE`, `constants.js:235`) and sets `upgradeBlocked = now + 1000` (`CONTROLLER_ATTACK_BLOCKED_UPGRADE`, `constants.js:239`) (`:41-48`).
- That 1,000-tick block also rejects the **next attackController** (`:25-28`) → one strike per controller per 1,000 ticks; and a CLAIM body lives only 600 ticks → **each attacker body delivers exactly one strike** — sustained pressure costs one fresh CLAIM body per 1,000 ticks.
- Magnitude: max practical 25×CLAIM+25×MOVE = −7,500/strike; against RCL8's 200K clock that is ~27 bodies over ~27K ticks → attackController is a **denial** tool (upgrade-block, safe-mode lockout, un-reserving), not a demolition tool.

**What `upgradeBlocked` denies** (the strike's real payload):
- `upgradeController` is rejected while it runs (`creeps/upgradeController.js:27-29`) and the downgrade clock's +100/tick restore stops (`controllers/tick.js:38`) → sustained strikes freeze RCL progress *and* recovery — a downgrade-blocked room cannot buy back its clock.
- **`activateSafeMode` is rejected** while upgrade-blocked (`controllers/activateSafeMode.js:17-19`). Resolution order inside the controller's tick applies `_upgradeBlocked` *before* checking `_safeModeActivated` (`controllers/tick.js:20-31`; `bulk.update` mutates in-memory, `driver/bulk.js:51`) → **a same-tick attackController hit beats a same-tick safe-mode pop**. Opening a siege with a CLAIM strike denies safe mode for the next 1,000 ticks.
- The nuke's 200-tick upgrade block (§2.8; `CONTROLLER_NUKE_BLOCKED_UPGRADE`, `constants.js:240`) flows through the same gate → a fresh landing also denies safe-mode *re*-activation for 200 ticks.
- `activateSafeMode` additionally requires the downgrade clock near full: rejected if `downgradeTime < now + CONTROLLER_DOWNGRADE[level]/2 − 5000` (`activateSafeMode.js:20-22`; `CONTROLLER_DOWNGRADE_SAFEMODE_THRESHOLD`, `constants.js:234`) → enough accumulated CLAIM strikes lock safe mode out entirely until the victim re-upgrades the clock back up.

**The downgrade clock & what a downgrade does** (`controllers/tick.js`):
- Full clock per RCL: 20K/10K/20K/40K/80K/120K/150K/200K for RCL 1–8 (`CONTROLLER_DOWNGRADE`, `constants.js:232`). Upgrading restores +100+1/tick capped at full (`:38-43`); a level-up resets it to half-max (§7.2).
- On hitting zero: level −1; new clock = half the new level's max (`:65`); **progress is set to 90% of the new level's requirement** (`:66`) → climbing back is fast; the lasting damage is below:
- **Every downgrade wipes stored safe-mode charges (`safeModeAvailable = 0`) and starts the 50K safe-mode cooldown** (`:67-68`) → the canonical siege sequence: deny upgrades → force one downgrade → breach during the 50,000-tick safe-mode blackout.
- Structures above the now-lower RCL allowance deactivate, keeping those Chebyshev-closest to the controller (`engine/utils.js:456-505` `checkStructureAgainstController`; spawns/extensions via `_calc_spawns.js`, §1.2) → RCL8→7 turns off 3 of 6 towers — the three *furthest* from the controller; plan tower placement with downgrade in mind (ADR 0009).
- At level 0: ownership cleared, `safeMode`/`upgradeBlocked` nulled, `safeModeAvailable = 0`, **`isPowerEnabled = false`** (`:52-62`) — the room is claimable and its power-enable flag is gone (§6.6). `unclaim` does the same instantly (`controllers/unclaim.js:18-27`).

**Signs** (`creeps/signController.js`): range ≤1, works on any controller — **no ownership or safe-mode gate** (`:15-28`) → signs are pure intel/diplomacy signal; never treat one as a mechanical claim or deterrent.

---

## 3. Spawn mechanics

### 3.1 Spawn time and the throughput ceiling — VERIFIED constants

`needTime = CREEP_SPAWN_TIME(3) × body.length`; body silently truncated to `MAX_CREEP_SIZE(50)` (`engine/processor/intents/spawns/create-creep.js:36,49`). One creep at a time per spawn (`:11-13`). The creep emerges when `gameTime >= spawning.spawnTime−1`; a blocked exit slips spawnTime +1/tick (`spawns/tick.js:17-27`). `PWR_OPERATE_SPAWN` multiplies needTime ×[0.9, 0.7, 0.5, 0.35, 0.2] (`create-creep.js:51-54`); `PWR_DISRUPT_SPAWN` freezes progress (`spawns/tick.js:13-15`).

**The hard production ceiling** (3 ticks/part × 1500-tick lifetime):

| Spawns (RCL) | Parts/tick | Sustained living parts | ≈ max-size creeps |
|---|---|---|---|
| 1 (RCL 1–6) | 0.33 | 500 | 10 |
| 2 (RCL 7) | 0.67 | 1,000 | 20 |
| 3 (RCL 8) | 1.00 | 1,500 | 30 |
| 3 + OPERATE_SPAWN lvl 5 | 5.00 | 7,500 | 150 |

CLAIM creeps (600-tick life, `common/constants.js:112`) consume 2.5× spawn-time per sustained part. Every body in ADR 0008's force plans and ADR 0007's hauler fleets is a withdrawal against this budget — economy and war share the same parts/tick pool.

### 3.2 Energy draw order (`spawns/_charge-energy.js`) — VERIFIED

Default (no `energyStructures`): **ALL spawns first** (sorted closest to the spawning spawn — itself first), **then extensions closest-first** (`:6-39`). With `energyStructures`: the exact caller-given order after filtering to own, active spawns/extensions and dedup (`:41-70`). The charge fails atomically if total available < cost (`:11-13,52-54`). Two spawns the same tick draw sequentially against the mutated in-memory store — both succeed if the pool covers both.

### 3.3 renewCreep (`spawns/renew-creep.js`) — VERIFIED

| Property | Value | Line |
|---|---|---|
| TTL gained per intent | `floor(1.2 × 1500 / 3 / body.length)` = `floor(600/size)` | `:28` |
| Overshoot | **intent rejected entirely** (no partial, no charge) if it would exceed 1500 TTL | `:29-31` |
| Energy cost | `ceil(1.2 × creepBodyCost / 3 / size)` | `:33` |
| CLAIM bodies | cannot renew | `:24-26` |
| Boosts | **ALL stripped, zero refund** | `:45-53` |
| Other gates | spawn not spawning; own creep; range ≤1; target not spawning | `:13-23` |

Energy per TTL-tick of renew ≈ spawn cost / 1500 — identical energy efficiency to respawning; the 1.2 ratio buys ~1.2× spawn-*time* efficiency (≈600 part-ticks per spawn-tick vs 500) and skips replacement travel. The price: no boosts, no CLAIM, and a busy spawn. ADR 0008 (Context, Field Report B) builds on exactly this: renewing a 40-part quad member regains only 15 TTL/intent — pre-spawning successors beats renew for combat bodies.

### 3.4 recycleCreep and death drops

`recycleCreep`: range ≤1, own non-spawning creep; calls `_die(target, dropRate=1.0)` (`spawns/recycle-creep.js:22`). Refund per part = `floor(min(125, BODYPART_COST × ttl/lifeTime))`, plus per boosted part `30 × ttl/lifeTime` mineral + `20 × ttl/lifeTime` energy; carried store returned in full; **deposited into a container on the creep's tile if it has space, else a tombstone** (`creeps/_die.js:39-94`). Natural death / kill uses `CREEP_CORPSE_RATE 0.2` instead of 1.0 (`_die.js:9`; `common/constants.js:113`).

### 3.5 Misc spawn facts

- **cancelSpawning refunds nothing** — creep deleted, spawn cleared, no credit (`spawns/cancel-spawning.js:6-13`).
- Each spawn self-charges +1 energy/tick while room spawn+extension energy < 300 (`spawns/tick.js:43-47`) — a drained room always recovers spawn ability.
- Excess spawns/extensions beyond the RCL allowance are switched `off`, keeping those closest to the controller (`_calc_spawns.js:9-22`); `CONTROLLER_STRUCTURES.spawn = {…, 7:2, 8:3}` (`common/constants.js:215`).
- Spawnstomp: §2.9.

---

## 4. Labs & boosts

### 4.1 runReaction (`engine/processor/intents/labs/run-reaction.js`)

- Output lab must be within **range 2 (Chebyshev) of BOTH input labs** (`:26,38`) — this is the geometric constraint room planning (ADR 0009) must satisfy.
- Inputs need ≥ `LAB_REACTION_AMOUNT 5` each; produces 5 (+2/4/6/8/10 with `PWR_OPERATE_LAB` → up to 15) (`:12-16`).
- Sets `cooldownTime = gameTime + REACTION_TIME[product]` (`:56-57`). Output lab may hold only one mineral type (`:42-51`). `reverseReaction` is symmetric, cooldown keyed on the decomposed compound (`labs/reverse-reaction.js`).
- Lab capacity: 3,000 mineral + 2,000 energy (`common/constants.js:275-276`). `LAB_COOLDOWN 10` is annotated "not used" in constants (`:279`) — the real cooldown is per-compound `REACTION_TIME`.

### 4.2 Reaction tree to T3 (`common/constants.js:484-615`; times `:733-768`)

- **Base pairs**: H+O→OH, Z+K→ZK, U+L→UL, ZK+UL→G.
- **T1** = mineral + H or O (UH, UO, KH, KO, LH, LO, ZH, ZO, GH, GO).
- **T2** = T1 + OH (e.g. UH2O).
- **T3** = X + T2 (e.g. XUH2O).
- Reaction times worth knowing: OH 20t; ZK/UL/G 5t; slow chains: ZH2O 40t, XZH2O 160t, GHO2 30t, XGHO2 150t, XGH2O 80t. Full per-compound table: `common/constants.js:733-768` — read it, don't guess.

### 4.3 boostCreep / unboostCreep (`labs/boost-creep.js`, `labs/unboost-creep.js`)

| Property | boostCreep | unboostCreep |
|---|---|---|
| Cost | 30 mineral + 20 energy **per part** (`boost-creep.js:15-23`) | — |
| Refund | — | 15 mineral/part (50%) **dropped on the ground**; energy 0 (`unboost-creep.js:32-48`; `common/constants.js:281-282`) |
| Scope | partial OK (until lab runs dry; optional `bodyPartsCount`) (`:37-46`) | **ALL boosts removed at once** |
| Part order | tail-of-body first, EXCEPT TOUGH boosts apply head-first (`:33-35`) | — |
| Gates | range ≤1; not spawning; **no owner check, no lab-cooldown check** (`:15-23`) | own creep; range ≤1; lab active and NOT on cooldown |
| Cooldown caused | none | `Σ parts[r] × totalReactionChainTime(r) × 15/5` (`:47`; `engine/utils.js:665-669`) — one XGHO2 part = 675 ticks |

Implications: you can boost **allies'** creeps and boost from a lab that is mid-reaction-cooldown; unboost is strategically near-one-shot per lab (a fully boosted creep can cost tens of thousands of lab-cooldown ticks to strip). Boost effects table: §2.3.

---

## 5. Market & terminals

### 5.1 Fees (`engine/processor/global-intents/market.js`)

| Event | Cost | Line |
|---|---|---|
| createOrder | `ceil(price × totalAmount × 0.05)` charged immediately | `:127-133` |
| changeOrderPrice up | `ceil(Δprice × remainingAmount × 0.05)`; downward free, nothing refunded | `:183-211` |
| extendOrder | `ceil(price × addAmount × 0.05)` | `:225` |
| **cancelOrder** | **forfeits the remaining fee** — no refund | `:256-263, 502-505` |
| natural expiry (30 days real time) | **refunds** `remainingAmount × price × 0.05` | `:507-533`; `MARKET_ORDER_LIFE_TIME`, `common/constants.js:375` |

Cap: 300 orders/player, enforced API-side (`engine/game/market.js:90-93`). Max **10 `deal()` intents queued per player per tick** (`game/market.js:149-151`).

### 5.2 deal() resolution — VERIFIED

All deals across all players are collected globally (§1.1 step 3), then **sorted by wrapped linear room distance dealer→order, closest first** (`global-intents/market.js:289`); intershard-resource deals are `_.shuffle`d instead (`:400`). Partial fills cascade: amount clipped to remainingAmount → seller stock → buyer terminal free space → buyer credits-at-execution (`:315-337`). **The dealer's terminal pays the energy cost and takes the 10-tick cooldown regardless of direction** (`:339,391-396`); a terminal on cooldown can't deal (`:300-302`) → max one deal per terminal per tick. Orders repriced this tick are skipped for dealing (`_skip`, `:179-181,269`) — no same-tick reprice sniping.

### 5.3 Transaction cost — VERIFIED formula

`cost = ceil(amount × (1 − e^(−range/30)))`, range = wrapped linear room distance (`engine/utils.js:644-659`; `engine/game/market.js:31-34`). The same formula applies to `Terminal.send` (`engine/processor/intents/terminal/send.js:19-20`); sending energy requires `store.energy ≥ amount + cost` (`:27-29`). `PWR_OPERATE_TERMINAL` multiplies cost and cooldown ×[0.9…0.5] (`send.js:22-25`; `market.js:37-39,85-88`). `TERMINAL_COOLDOWN 10` applies to send and deal; capacity 300,000 (`common/constants.js:333,337`). **`TERMINAL_SEND_COST (0.1)` is a dead constant — never referenced by the engine.**

### 5.4 Manipulation surface (for trust modeling — plan Increment 7 market hardening)

1. **Spoof walls must be backed**: displayed `amount` is recomputed every tick from actual terminal stock (sell) or actual credits + terminal space (buy) (`market.js:542-587`); unbacked orders show 0/inactive — but the 5% fee is sunk unless the order survives 30 days unfilled.
2. **Wash trades** between one player's own terminals/orders are not blocked (no same-user check in the terminal-deal path). Painting price history costs 5% of painted notional + transfer energy + cooldowns; `getHistory` is poisonable at that price — trust trailing windows, not single ticks.
3. **Races are won by distance, not submission time** (`:289`) — pre-positioned terminals near popular order rooms structurally win fills. You cannot react same-tick (§1.1).
4. **Buy orders escrow nothing**; credits are checked at execution (`:330-336`) — a whale buy wall can be hollow the tick it would fill.

---

## 6. Power, factory, deposits

### 6.1 processPower & GPL

1 power + 50 energy per intent-tick (`engine/processor/intents/power-spawns/process-power.js:16-37`; `POWER_SPAWN_ENERGY_RATIO`, `common/constants.js:269`). `PWR_OPERATE_POWER` adds +1..5 power/tick, each still ×50 energy. GPL: `level = floor(sqrt(totalProcessed/1000))` — **GPL n costs 1000·n² lifetime power** (`engine/game/game.js:133-134`; `common/constants.js:808-809`).

### 6.2 Power banks

2M hits (`common/constants.js:259`); melee attackers reflect 50% damage (`_damage.js:35-37`); capacity 500–5,000; despawns at decayTime 5,000 ticks (`processor.js:421-426`; `constants.js:259-264`).

### 6.3 Operator powers — POWER_INFO quick reference (`common/constants.js:854-1016`; application `engine/processor/intents/power-creeps/usePower.js`) — VERIFIED

All powers require a power-enabled room when a controller exists (§6.6 gates); a higher-rank still-active effect cannot be overwritten by a lower one (`usePower.js:47-50`). Effects replace same-power effects, `endTime = now + duration[rank]`; cast sets `cooldownTime = now + cooldown` per power per creep and deducts ops (`usePower.js:277-298`). **Level gates** are when each rank r1→r5 unlocks (the creep-level arrays): most powers gate at **[0, 2, 7, 14, 22]**; REGEN_SOURCE / REGEN_MINERAL / OPERATE_POWER at **[10, 11, 12, 14, 22]**; OPERATE_CONTROLLER / DISRUPT_TERMINAL at **[20, 21, 22, 23, 24]**. All OPERATE/REGEN/FORTIFY targeting is range 3 unless noted.

| Power | Gates | cd / duration | ops | Effect r1→r5 | Constants line / note |
|---|---|---|---|---|---|
| GENERATE_OPS | 0,2,7,14,22 | 50 / — | 0 | +1/2/4/6/8 ops | `:855-860`; the only in-game ops faucet (≤0.16 ops/t at r5); overflow above store drops to ground (`usePower.js:57-68`) |
| OPERATE_SPAWN | 0,2,7,14,22 | 300 / 1000 | 100 | spawn time ×0.9/0.7/0.5/0.35/0.2 | `:861-868`; applied at `spawns/create-creep.js:51-54` — the 5× throughput multiplier (§3.1) |
| OPERATE_TOWER | 0,2,7,14,22 | 10 / 100 | 10 | tower effect ×1.1–1.5 | `:870-877`; applied pre-floor in `towers/attack.js:40-45` |
| OPERATE_STORAGE | 0,2,7,14,22 | 800 / 1000 | 100 | +0.5M–7M capacity | `:879-886` |
| OPERATE_LAB | 0,2,7,14,22 | 50 / 1000 | 10 | +2/4/6/8/10 reaction output | `:888-895` |
| OPERATE_EXTENSION | 0,2,7,14,22 | 50 / instant | 2 | refill ≤20–100% of total extension capacity, closest-first, from one storage/terminal/factory/container | `:897-903`; source must belong to the controller owner (`usePower.js:104-132`) |
| OPERATE_OBSERVER | 0,2,7,14,22 | 400 / 200–1000 | 10 | unlimited-range observe | `:905-911` |
| OPERATE_TERMINAL | 0,2,7,14,22 | 500 / 1000 | 100 | send/deal cost & cooldown ×0.9–0.5 | `:913-920` |
| DISRUPT_SPAWN | 0,2,7,14,22 | 5 / 1–5 | 10 | freezes spawn progress, **range 20** | `:922-928`; effect in `spawns/tick.js:13-15` |
| DISRUPT_TOWER | 0,2,7,14,22 | **0** / 5 | 10 | tower effect ×0.9–0.5, **range 50** | `:930-937`; zero cooldown → one operator rotates 5 towers at ×0.5 |
| DISRUPT_SOURCE | 0,2,7,14,22 | 100 / 100–500 | 100 | halts source regeneration | `:939-945`; effect in `sources/tick.js:17-22` |
| SHIELD | 0,2,7,14,22 | 20 / 50 | 100 **energy** | 5K/10K/15K/20K/25K-hit rampart on own tile | `:947-953`; `usePower.js:228-256` |
| REGEN_SOURCE | **10**,11,12,14,22 | 100 / 300 | **0** | +50–250 energy per 15t | `:955-962`; capped at source capacity (`sources/tick.js:31-39`) |
| REGEN_MINERAL | **10**,11,12,14,22 | 100 / 100 | **0** | +2–10 per 10t | `:964-971` |
| DISRUPT_TERMINAL | **20**,21,22,23,24 | 8 / 10 | 50→10 | blocks terminal, **range 50** | `:973-980` |
| OPERATE_POWER | **10**,11,12,14,22 | 800 / 1000 | 200 | +1–5 power/tick processed (each ×50 energy) | `:990-997`; §6.1 |
| FORTIFY | 0,2,7,14,22 | 5 / 1–5 | 5 | rampart/wall **invulnerable to non-nuke damage** | `:982-988`; `_damage.js:26-31`; rMA skips fortified targets (`rangedMassAttack.js:47-49`) |
| OPERATE_CONTROLLER | **20**,21,22,23,24 | 800 / 1000 | 200 | +10–50 e/t over the RCL8 15/tick upgrade cap | `:999-1006` |
| OPERATE_FACTORY | 0,2,7,14,22 | 800 / 1000 | 100 | enables leveled `produce`; **first use permanently brands the factory with the cast rank** | `:1008-1014`; `usePower.js:259-274` |

### 6.4 Factory & commodities

`produce` checks recipe level vs factory level; **leveled commodities additionally require an ACTIVE OPERATE_FACTORY effect of exactly that level on every produce** (`engine/processor/intents/factories/produce.js:9,21-23`); per-recipe cooldown (`:43`); capacity 50,000 (`common/constants.js:357`). Level-0 chains (no power creep needed): 500 mineral + 200 energy → 100 bar (20cd) and the reverse; 600 energy → 50 battery (10cd); 50 battery → 500 energy (`constants.js:1144-1286`). Regional chains (wire/switch/…, levels 1–5) consume deposit resources + bars (`:1320+`). Commodity value is purely market-side (NPC buy orders) — no engine constant.

### 6.5 Deposits

Harvest 1/WORK/intent (boostable ×3/5/7); after each harvest `cooldown = ceil(0.001 × totalHarvested^1.2)` and `decayTime` refreshes to +50,000 (`engine/processor/intents/creeps/harvest.js:114-138`; `common/constants.js:329-331`).

### 6.6 Power-creep lifecycle (account-level mechanics; grounds ADR 0013) — VERIFIED

A power creep is **account state** (a `users.power_creeps` record) that is only sometimes also a room object. Doctrine (build orders, scheduling, enable policy) lives in ADR 0013 D3/D4; the engine facts:

- **GPL & allowance.** `GPL = floor((user.power / 1000)^(1/2))` (`POWER_LEVEL_MULTIPLY 1000`, `POWER_LEVEL_POW 2`, `common/constants.js:808-809`) — GPL n therefore costs 1000·n² lifetime processed power (§6.1). The spend check is `#creeps + Σ creep levels < GPL` (`global-intents/power/createPowerCreep.js:11-13`, same in `upgradePowerCreep.js:11-13`) — every creep *and* every level each consume one GPL of allowance.
- **Creation.** `createPowerCreep` mints a level-0 creep: 1,000 hits, 100 store, class `operator` — the only class that exists (`POWER_CLASS`, `constants.js:815-817`; `createPowerCreep.js:27-39`). Name ≤50 chars, must be unique per account (`:21-25`).
- **Leveling.** `upgradePowerCreep`: +1 level, +1,000 hitsMax, +100 store capacity, +1 rank in ONE chosen power, gated by that power's level array (§6.3); max creep level **25**, max power rank **5** (`upgradePowerCreep.js:22,37,41-48`; `POWER_CREEP_MAX_LEVEL 25`, `constants.js:812`). Levels are permanent — there is no respec except delete (below).
- **Spawning.** At an **own** power spawn: sets `ageTime = now + POWER_CREEP_LIFE_TIME (5,000)` and full hits (`spawnPowerCreep.js:30-39`; `constants.js:813`); blocked while `spawnCooldownTime` is in the future (`:16`) or another power creep stands on the spawn's tile (`:20-22`).
- **Renew.** Range ≤1 to a power spawn **or a power bank** → full 5,000-tick reset (`power-creeps/renew.js:8-20`) — a bank-escort operator self-renews in the field; renewal at home is a short errand, so age-death is always a policy failure.
- **Death.** Age (`gameTime ≥ ageTime−1`) or hits ≤ 0, checked in the **global stage**, not the room tick (`global-intents/power.js:30-36`; damage/heal netting itself is creep-like — damage, then heal, then clamp — `power-creeps/tick.js:49-67`). Death drops the store into a same-tile container else a tombstone and sets `spawnCooldownTime = Date.now() + 8 real-time hours` (`_diePowerCreep.js:26-58`; `POWER_CREEP_SPAWN_COOLDOWN`, `constants.js:810`). **Levels and powers are never lost** — death costs wall-clock downtime, not progress.
- **Delete / respec.** `deletePowerCreep` takes **24 real-time hours** and is cancellable until then (`deletePowerCreep.js:15-27`; `constants.js:811`) → builds are effectively permanent decisions; record them (ADR 0013 D3.1).
- **Movement.** Power creeps always pass `canMove` — **no fatigue, 1 tile/tick on any terrain including swamp** (`movement.js:11-14`); they wear roads like a 100-part creep (`movement.js:215-219`).
- **Intents.** No conflict matrix: `move + usePower + withdraw + transfer + say` in one tick is legal (`power-creeps/intents.js:3`); the per-object intent store holds one `usePower` per tick.
- **usePower gates** (`power-creeps/usePower.js`). In any room **with a controller**, EVERY power — including GENERATE_OPS — requires `controller.isPowerEnabled` (`:9-12`); a **non-owner** is additionally blocked while the room's safe mode is active (`:13-15`). The flag is room-wide and user-blind: it gates own and enemy powers identically. Controller-less rooms (highways, SK/center) have no gate. Per cast: rank > 0 and per-power-per-creep cooldown (`:25`), ops paid from the creep's own store (`:29-37`), range (`:39-46`), and a higher-rank still-active effect rejects a lower-rank overwrite (`:47-50`).
- **enableRoom** (`power-creeps/enableRoom.js`). ANY power creep at range ≤1 to ANY controller sets `isPowerEnabled: true` (`:19`); the only block is `target.user != creep.user && safeMode active` (`:12-14`) → **an enemy operator can enable YOUR room** unless safe mode is up; refusing to enable is not a shield. One-way: the flag persists until controller downgrade-to-0 or unclaim (`controllers/tick.js:52-62`; `controllers/unclaim.js:26`; §2.12).
- **Ops.** GENERATE_OPS yields [1,2,4,6,8] per cast on a 50-tick cooldown — hard ceiling ≈0.16 ops/t per operator; generation above store capacity **drops to the ground** (`usePower.js:57-68`). Ops are an ordinary store resource (haulable, tradeable, transferable like any mineral).

---

## 7. Misc economy mechanics & constants

### 7.1 Decay & upkeep (drives ADR 0007 remote costing, ADR 0009 layout)

| Object | Rule | Source |
|---|---|---|
| Link | receiver loses `ceil(amount × 0.03)`; **sender** cooldown = 1 × Chebyshev distance; capacity 800 | `engine/processor/intents/links/transfer.js:42-51`; `common/constants.js:163-165` |
| Container | −5,000 hits per **100** ticks where controller level is 0 — **including reserved remotes** — per **500** ticks at RCL ≥1; 250K hits | `engine/processor/intents/containers/tick.js:10-31`; `constants.js:339-343` |
| Road | −(100 × terrain ratio) hits per 1,000 ticks (swamp ×5, tunnel ×150); each creep step pulls `nextDecayTime` forward by 1 × body.length (power creep 100); build cost 300 × 1/5/150 plain/swamp/tunnel | `engine/processor/intents/roads/tick.js:10-31`; `movement.js:216-219` (VERIFIED); `constants.js:155-159,192-211` |
| Dropped resource | decays `ceil(amount/1000)` per tick | `engine/processor/intents/energy/tick.js:12` |
| Tombstone | 5 × bodyParts ticks, store spills to ground on decay | `engine/processor/intents/tombstones/tick.js`; `constants.js:359` |
| Ruin | 500 ticks | `constants.js:362` |
| Rampart | −300 hits per 100 ticks | `engine/processor/intents/ramparts/tick.js:26-40` |

### 7.2 Controller / GCL — VERIFIED

- RCL8 cap is **15 energy/tick room-wide, shared across all upgraders via the `target._upgraded` accumulator** (`engine/processor/intents/creeps/upgradeController.js:42-52,88`), raised by OPERATE_CONTROLLER. Range 3.
- **GCL is credited the boosted amount even at RCL8** (`upgradeController.js:57,84-86`): the 15/tick cap binds *energy*, the GCL increment is `boostedEffect` — XGH2O doubles GCL per energy at the cap (30 GCL/tick; 130 with lvl-5 power).
- Any upgrade adds +100+1 ticks to downgradeTime, capped at full (`engine/processor/intents/controllers/tick.js:38-43`); level-up sets downgradeTime to half-max (`upgradeController.js:72`).
- GCL level n→n+1 costs `1,000,000 × n^2.4` (`engine/utils.js:661-663`; `common/constants.js:284-285`).

### 7.3 Sources & minerals

- Sources refill when `gameTime >= nextRegenerationTime−1`; the 300-tick timer starts at the first harvest below cap (`engine/processor/intents/sources/tick.js:10-29`). Capacity 3,000 owned **or reserved**, 1,500 neutral-unreserved, **4,000 in controller-less rooms (SK/center)** (`:46-59`). `invaderHarvested` accumulates toward `INVADERS_ENERGY_GOAL 100,000` (`creeps/harvest.js:45`; `constants.js:776`).
- Minerals: regen 50,000 ticks after exhaustion; density amounts {1:15K, 2:35K, 3:70K, 4:100K}; density rerolls on regen with p=0.05 (always for LOW/ULTRA) (`engine/processor/intents/minerals/tick.js:14-36`; `constants.js:298-327`). Extractor cooldown 5; 1/WORK/intent.

### 7.4 Constants quick table (`common/constants.js`) — line-checked

| Constant | Value | Line |
|---|---|---|
| BODYPART_COST | move/carry 50, work 100, attack 80, ranged 150, heal 250, tough 10, claim 600 | 96-105 |
| CREEP_LIFE_TIME / CLAIM / MAX_CREEP_SIZE | 1500 / 600 / 50 | 111-112, 296 |
| CREEP_SPAWN_TIME / SPAWN_RENEW_RATIO | 3 / 1.2 | 142-143 |
| CREEP_CORPSE_RATE / CREEP_PART_MAX_ENERGY | 0.2 / 125 | 113-114 |
| EXTENSION_ENERGY_CAPACITY | 50 (RCL≤6), 100 (7), 200 (8) → 12,900 room max @RCL8 | 153 |
| HARVEST_POWER / MINERAL / DEPOSIT | 2 / 1 / 1 per WORK | 117-119 |
| UPGRADE_CONTROLLER_POWER / RCL8 cap | 1 / 15 | 124, 238 |
| TOWER: capacity / cost / attack / heal / repair / optimal / falloff-range / falloff | 1000 / 10 / 600 / 400 / 800 / 5 / 20 / 0.75 | 247-254 |
| LAB: reaction / boost mineral / boost energy / unboost refund / caps | 5 / 30 / 20 / 15 / 3000+2000 | 275-282 |
| MARKET_FEE / max orders / order lifetime | 0.05 / 300 / 30 days | 372-375 |
| TERMINAL_CAPACITY / COOLDOWN | 300,000 / 10 | 333, 337 |
| LINK capacity / cooldown / loss | 800 / 1 / 0.03 | 163-165 |
| GCL_MULTIPLY / GCL_POW | 1,000,000 / 2.4 | 284-285 |
| POWER_BANK hits / hit-back / decay | 2M / 0.5 / 5000 | 259-264 |
| SOURCE capacity owned / neutral / keeper; regen | 3000 / 1500 / 4000; 300 | 136-147 |
| CONTROLLER_LEVELS / DOWNGRADE | {1:200 … 7:10.935M} / {1:20K … 8:200K} | 213, 232-233 |
| REACTION_TIME / BOOSTS / COMMODITIES tables | — | 733-768 / 617-731 / 1144+ |

---

## 8. Folklore vs fact

Community beliefs the engine source **contradicts**. Cite this section when reviewing design docs.

| # | Folklore | Fact | Source |
|---|---|---|---|
| 1 | "Heal can't save a creep from lethal same-tick damage" | FALSE — heal applies after damage, before the death check | `creeps/tick.js:120-135` (VERIFIED) |
| 2 | "heal blocks all attacks that tick" | Only melee `attack`; `move + heal + rangedAttack` is legal | `creeps/intents.js:3-13` (VERIFIED) |
| 3 | "rangedMassAttack damages walls/roads/containers" | FALSE — only owned objects + powerBanks | `creeps/rangedMassAttack.js:26-30` (VERIFIED) |
| 4 | "Creeps can block enemy spawns" | FALSE — spawnstomp kills the blocker | `spawns/_born-creep.js:53-77` |
| 5 | "Nuke damage is area-limited" | Structure damage is 5×5, but **all creeps in the entire room die** | `nukes/tick.js:16-26` |
| 6 | "A rampart only redirects its owner's enemies' attacks" | Redirect is ownership-blind — you chew your own rampart attacking a creep standing on it | `creeps/attack.js:33-36`; `towers/attack.js:27-30` (VERIFIED) |
| 26 | "`store.getUsedCapacity()` (no arg) double-counts — sum per-resource instead" (the Ibex `HasExpensiveStore` workaround's premise) | FALSE in the current engine: the no-arg path is a **memoized single `_.sum(object.store)` over the raw DB store** — no double counting is possible. The REAL trap is different: on `storeCapacityResource` structures (lab/nuker/powerSpawn), the no-arg call returns **`null`**, which the Rust binding's `Option<u32> → unwrap_or(0)` turns into **0** (not a total!) — per-resource summation remains the only way to total a special store through the binding. General stores (creeps/containers/storage/terminal) are safe to query directly. Live-probed 2026-06-10 on the eval server (empty-store creeps: `none === sum`). | `engine src/game/store.js:36-50` (VERIFIED); fork `screeps-game-api/src/objects/impls/store.rs:39-60` |
| 7 | "Towers hit weaker when low on energy" | No such mechanic — flat 10 energy/action, full power until empty | `towers/attack.js:24,54` (VERIFIED) |
| 8 | "Tower falloff steps at ranges 5/10/20" | Linear from 5 to 20 | `towers/attack.js:32-39` (VERIFIED) |
| 9 | "Renew is energy-efficient for boosted creeps" | Renew silently strips ALL boosts, zero refund | `spawns/renew-creep.js:45-53` (VERIFIED) |
| 10 | "Renew partially applies when near the 1500 cap" | Overshooting renew is rejected entirely — no partial, no charge | `renew-creep.js:29-31` (VERIFIED) |
| 11 | "cancelSpawning refunds part of the energy" | Refunds nothing | `spawns/cancel-spawning.js:6-13` |
| 12 | "Terminal send costs 0.1 energy/unit/room" | `TERMINAL_SEND_COST` is dead; real cost `ceil(amount × (1 − e^(−dist/30)))` | `engine/utils.js:657-659` |
| 13 | "Cancel a market order to get the fee back" | Backwards: cancel forfeits; 30-day expiry refunds the unfilled portion's fee | `global-intents/market.js:502-533` |
| 14 | "Spawn energy draws closest-structure-first" | All spawns first, then extensions closest-first (default path) | `spawns/_charge-energy.js:15-36` (VERIFIED) |
| 15 | "boostCreep needs your own creep / an idle lab" | No owner check, no cooldown check — allied boosting works; a mid-cooldown lab can boost | `labs/boost-creep.js:15-23` |
| 16 | "RCL8 hard-caps GCL gain at 15/tick" | Energy is capped at 15 room-wide; **GCL credits the boosted amount** | `creeps/upgradeController.js:42-52,84-86` (VERIFIED) |
| 17 | "Reserved-remote containers decay at the slow owned rate" | Only RCL ≥1 gets 500-tick decay; reserved remotes decay at the fast 100-tick rate | `containers/tick.js:26` |
| 18 | "Market deals fill in submission order" | Sorted by room distance, closest dealer first (intershard: shuffled) | `global-intents/market.js:289,400` (VERIFIED) |
| 19 | "The order owner pays deal energy when bought from" | The **dealer** always pays the energy and eats the cooldown, both directions | `market.js:339,391-396` (VERIFIED) |
| 20 | "Every intent costs 0.2 CPU" | `say` and `pull` are free; re-issuing the same intent name on the same object charges once; billed = `ceil(cleanTime + 0.2 × count)` | `driver/runtime/runtime.js:60-71`; `driver/runtime/make.js:98-110,193-194` (VERIFIED, §9.2) |
| 21 | "Segments hold 50 KB" | **100 KB** per segment — and exceeding it **throws**, killing the whole end-of-tick save; Ibex's 50 KiB chunking is a self-imposed half-cap | `driver/runtime/runtime.js:264-265`; `memorysystem.rs:95` (VERIFIED, §9.1) |
| 22 | "Never enableRoom — it keeps enemy powers out of your rooms" | Backwards both ways: the flag is user-blind, and an enemy operator can enable your controller himself (active safe mode is the only block) | `power-creeps/enableRoom.js:12-19`; `usePower.js:9-15` (VERIFIED) |
| 23 | "A dead power creep loses levels/powers" | Levels and powers are never lost; death costs an 8-real-hour spawn cooldown plus the dropped store | `global-intents/power/_diePowerCreep.js:51-58` (VERIFIED) |
| 24 | "attackController can be spammed every tick" | Owned controllers: a hit starts a 1,000-tick upgrade-block that also rejects further attackController — one strike per 1,000t (and CLAIM bodies live 600t → one strike per body); **reservations** CAN be drained every tick | `creeps/attackController.js:25-28,41-48` (VERIFIED, §2.12) |
| 25 | "Safe mode can always be popped the moment the attack starts" | Activation is rejected while upgrade-blocked or with a drained downgrade clock; a same-tick attackController hit beats a same-tick activateSafeMode | `controllers/activateSafeMode.js:17-22`; `controllers/tick.js:20-31` (VERIFIED, §2.12) |

### Verify before relying (not pinned to engine source in this pass)

These came up in research but are backend/MMO-side, annotated-dead, or otherwise not provable from the three cloned repos (engine, common, driver). Verify (engine read or ADR 0006 harness scenario) before any design depends on them:

1. ~~**CPU accounting, including the ~0.2 CPU/intent cost**~~ — **RESOLVED 2026-06-09** by the driver clone: `intentCpu = 0.2` with `say`/`pull` free and per-(object, intent-name) dedup; billed `ceil(cleanTime + 0.2 × count)`. See §9.2. The AGENTS.md/ADR 0004 figure is engine fact, with those two refinements.
2. **Pathfinding internals** (native PathFinder costs, `findRoute` behavior) — driver-side **native C++ module**, still unread (the JS in `driver/lib` only binds it); out of scope here.
3. `INVADER_CORE_EXPAND_TIME` (`common/constants.js:846`) — consumed by the MMO backend, not by engine room-processing code; private servers may differ.
4. **Invader raid spawning cadence** (when the 100K `invaderHarvested` goal triggers what) — the counter is engine-side (`creeps/harvest.js:45`) but raid generation is backend logic.
5. `LAB_COOLDOWN 10` — annotated "not used" (`constants.js:279`); trust per-compound `REACTION_TIME` instead.
6. **Market order books / `getHistory` aggregation windows** — storage/API-side; only the resolution mechanics in §5.2 are engine-pinned.
7. **Intershard portals/transfers** beyond the deal-shuffle (`market.js:400`) — multi-shard plumbing not examined.
8. **Hash-order details** in the per-room intent loop (§1.2 step 4): that it is *unreliable* is certain; any specific observed ordering is an implementation accident — never depend on one even if measured.
9. Stronghold `focusMax` targeting details and keeper micro thresholds (§2.11) — cited from research pass; re-read `stronghold.js` / `keepers/pretick.js` before building a stronghold-killer around them.
10. **MMO server version drift** — these clones are a snapshot. For tournament-critical mechanics (nuke interactions, market races), re-verify against the live engine repo HEAD and/or harness-test on the private server, which runs this same engine code (ADR 0006).

---

## 9. RawMemory, segments & CPU accounting (driver ground truth)

Pinned 2026-06-09 from the `screeps/driver` clone (`driver/…` = `C:\code\screeps-driver\lib\…`) — the isolated-vm runtime layer the engine clones don't contain. This section resolves verify-item 1 above and folklore rows 20–21.

### 9.1 Memory & segment mechanics — VERIFIED

| Fact | Value | Source |
|---|---|---|
| `Memory` / `RawMemory` main blob | **2 MB hard cap** — `RawMemory.set` over 2×1024×1024 chars **throws** | `driver/runtime/runtime.js:113-114` |
| Segment ID space | **0–99** (100 segments); out-of-range ids throw | `runtime.js:140,258` |
| Per-segment size | **100 KB** — enforced at save; exceeding it **throws**, aborting the whole end-of-tick save (every segment write that tick is lost) | `runtime.js:264-265` |
| Active segments per tick | **max 10** — `setActiveSegments` throws on an 11th id | `runtime.js:134-136` |
| Segments saved per tick | **max 10** — and the save loop counts **every key in `RawMemory.segments`**, i.e. loaded-active segments are written back unconditionally; reads consume the same 10-slot budget as writes | `runtime.js:191-195` (load populates the object) → `:250-268` (save-all-keys) |
| `setActiveSegments` latency | **one tick**: the requested id list is persisted to the user record after the tick ends and the *next* tick's data load fetches those segments | `driver/runtime/make.js:200-201` → `driver/runtime/data.js:228-229,249-252` → `runtime.js:191-195` |
| Public / foreign segments | `setPublicSegments` persists a comma-list (`make.js:249-250`); foreign reading is **one segment at a time** — `setActiveForeignSegment` stores a single `{username, id}` (`runtime.js:174-188`), honored next tick **only if** the target's public list contains the id (`data.js:231-238,254-260`); `id`-less requests fall back to the target's `defaultPublicSegment` (`make.js:233-238`) | — |
| Segment persistence | committed end-of-tick via `hmset` into the per-user segment hash | `driver/index.js:162-168` |
| Isolate heap | 256 MB + static terrain size | `driver/runtime/user-vm.js:30` |

Implications: a fresh VM reset cannot read any segment its *previous* tick didn't request — the post-reset tick is segment-blind until the request round-trips (Ibex already handles this by gating dispatch, `game_loop.rs:699-709`, at the cost of one idle tick). And because loaded segments are rewritten on save, "active" and "written" share one 10-id budget — the cap below is a per-tick *touch* cap, not just a read cap.

### 9.2 Intent CPU pricing — VERIFIED (the constant ADR 0004 budgets against)

- **`intentCpu = 0.2`** (`driver/runtime/runtime.js:60`). Not folklore, not community-measured — a driver constant.
- **`say` and `pull` are free** (`runtime.js:61`; re-checked at billing time, `make.js:99`).
- In-tick accounting (what `Game.cpu.getUsed()` reflects): one 0.2 charge per **(object, intent name)** — re-issuing the same intent on the same object the same tick charges once (`runtime.js:66-72`); a cancelled intent refunds (`runtime.js:92-99`); array-style intents (e.g. the ≤10 queued `deal`s, §5.1) charge per element (`runtime.js:73-91`).
- End-of-tick billing: `cpuUsed = ceil(usedCleanTime + 0.2 × intentsCount)` (`make.js:193-194`, count logic `:98-110`) — note the **ceil**: billed CPU is an integer.
- Bucket: `cpuAvailable += cpu_limit − cpuUsed`, clamped to the configured bucket size (`make.js:206-211`).

### 9.3 WARNING — Ibex's segment budget is already at the engine cap

Ibex's named segments today (file:line, `screeps-ibex/src/`):

| Segment(s) | Owner | Source |
|---|---|---|
| 50–55 | ECS components (`COMPONENT_SEGMENTS`) | `game_loop.rs:554` |
| 55 | cost matrix (`COST_MATRIX_SEGMENT`) — **collides with the ECS range**: Critical IBEX-013 | `pathing/costmatrixsystem.rs:6` |
| 56 | stats history | `stats_history.rs:17` |
| 57 | always-on metrics (**proposed**, ADR 0006 / plan Inc 0) | — |
| 60 | room planner | `room/roomplansystem.rs:241` |
| 99 | live stats | `statssystem.rs:340-348` |

That is **ten ids**. Ibex requests every registered segment every tick and blocks dispatch until all are active (`game_loop.rs:699-709`; `memorysystem.rs:128-147`); the stats system adds 99 each tick it runs (`statssystem.rs:340`) and the planner adds 60 while a plan is in flight (`roomplansystem.rs:319`). With seg-57 live, the steady-state active set is **10 of 10 — exactly the `setActiveSegments` cap**; one more concurrent id throws (`runtime.js:134-136`), and §9.1's save-all-keys rule means there is no "write-only" escape hatch. The cap binds hardest on the **post-reset tick**: everything needed to rebuild the world — components, cost matrix, planner resume state, stats history — must be readable inside a single 10-segment activation, after the one-tick request latency.

**[ADR 0002](../design/0002-serialization.md) owns the segment registry**; every new segment proposal must clear it. The pending claims: 0002 itself (a dedicated cost-matrix segment), [0009](../design/0009-room-planning-and-multiroom-layout.md) (a `RoomGraph` segment — its own text already says "not 50-55/56/57/60/99"), and [0012](../design/0012-market-and-risk.md) (a market-ledger block). None of them fit without one of: (a) **freeing an id** — 0002's interim shrink of `COMPONENT_SEGMENTS` to 50–54 frees exactly one; (b) **time-multiplexing off the gating set** — planner/stats-class segments don't need to be active every tick, only the world-rebuild set does; or (c) **packing** — the 100 KB real per-segment cap (§9.1) vs Ibex's self-imposed 50 KiB chunks (`memorysystem.rs:95-97`) means the ECS payload could halve its segment count before the id budget needs to grow. What is *not* an option is adding an eleventh always-active id.
