# ADR 0041 — Combat boost layer (boost-aware force sizing + composition)

- **Status:** Decided
- **Date:** 2026-07-02
- **Deciders:** William Archbell
- **One line:** Make the LIVE capability-driven force-composition pipeline (ADR 0031) **boost-aware on our own side** — a boost tier becomes an EV-search axis (like the existing TOUGH ladder), boost multipliers thread into the part↔capability pricing, and a *colony-supply availability gate* keeps a boosted comp affordable-or-degrades-gracefully — closing the asymmetry where we already model enemy boosts (×4, ADR-0031 threatmap) but field none of our own. This is the reserved v2 `boost_tier` lever (ADR 0031a §5/§4.6, 0031 §5 "Deferred: the v2 `boost_tier` axis"). The five design questions this ADR opened are answered in §8.
- **Related:** [0031](0031-capability-driven-force-composition.md) (the pipeline this extends — `RequiredForce`/`emit_requirement`/`assemble_force`/`optimize_composition`/`ForceDoctrine`; §5 "Deferred" names the boost axis + the `assemble_force`=`None` escalate-vs-abandon that boosts partially answer), [0031a](0031a-force-composition-tunable-params.md) §4.6/§5(6) (`boost_tier` reserved as "the single biggest lever the moment v2 lands", grounded boost gates), [0031b](0031b-force-composition-tuning-results.md) (the sweep this re-opens), [0020 §12](0020-ev-adaptive-blob-combat.md) (the force-sizing oracle + the EV framing), [0010](0010-boost-lab-factory-pipeline.md) (the boost/lab/factory economy pipeline this *consumes from* — the demand this ADR emits is exactly the consumer ADR 0010's BoostQueue wire was designed for; ADR 0010 L1/L2 remain the supply side), [0026 §9](0026-combat-strategy-selection.md) (the doctrine registry), [0029](0029-generalized-force-composition.md) (one-oracle generalization). **Engine ground truth:** [`../references/engine-mechanics.md`](../references/engine-mechanics.md) + the boost effect table recomputed below (EP-7.1/EP-7.2).

---

## 1. Context — why boosts, and why now

The reconciliation backlog names the **"0008a boost layer"** the highest-leverage missing combat capability: it blocks **T-COMP-1/5** (boosted offense/defense compositions), **T-TOWER-3** (out-heal / out-armor a towered room point-blank), **T-NPC-7** (boosted NPC/stronghold clears), and **L3+ strongholds** (which are unbeatable by an unboosted single squad — the force-sizing oracle correctly defers them today).

### 1.1 The mechanics that make boosts decisive (engine-recomputed, EP-7.1/EP-7.2)

A boost is a mineral compound applied to a body part via `boostCreep` (30 mineral + 20 energy **per part**; engine `labs/boost-creep.js`, cited in ADR 0010 §"Engine ground truth"). The compound multiplies that part's output for the creep's whole life. The military-relevant T3 (catalyzed) effects (`common/lib/constants.js:617-731`, mirrored in `bodies::boosts` — `bodies.rs:130-143`):

| Part | Boost (T3) | Effect | Unboosted | Boosted |
|---|---|---|---|---|
| ATTACK | XUH2O | ×4 damage | 30/part | **120/part** |
| RANGED_ATTACK | XKHO2 | ×4 damage | 10/part | **40/part** |
| HEAL | XLHO2 | ×4 heal | 12/part | **48/part** |
| WORK (dismantle) | XZH2O | ×4 dismantle | 50/part | **200/part** |
| TOUGH | XGHO2 | ×0.3 damage taken | 100 EHP/part | **~333 EHP/part** |
| MOVE | XZHO2 | ×4 fatigue-decrease | 1 MOVE:2 non-MOVE (road) | ~1 MOVE:8 (frees slots) |

Two consequences drive the whole design:

- **A boosted part is 4× the force per part** (or ~3.3× survivability for TOUGH). A boosted squad delivers the same required capability in **~¼ the parts** → fewer, cheaper members OR the same member-count at 4× power. This is exactly what turns an oracle-`None` (a target no unboosted single squad can take, deferred at ADR 0031's `MAX_SIZED_MEMBERS=8` ceiling) into a winnable one — the partial answer to ADR 0031 §5's "escalate-vs-abandon on `assemble_force`=`None`" (#38).
- **Unboosted, you cannot out-heal a tower point-blank** (~50 HEAL parts for ONE tower — ADR 0031a §5, infeasible solo), which is why the LIVE bot only takes multi-tower rooms via the **drain** tactic (ADR 0031 §2(g)). A T3-HEAL squad (48/part) out-heals **4×** the incoming, and T3-TOUGH absorbs **~3.3×** — together they make a *direct breach* of a towered room feasible where only drain was before (T-TOWER-3).

### 1.2 The cost side (why availability must gate, not just EV)

Boosts are not free force — they cost **energy + minerals + lab-time**, and (critically) they must be **stockpiled predictively** because the tick-snapshot concurrency means you cannot brew them in an attack window (ADR 0010 §"The arithmetic that forces the doctrine": one boosted quad member ≈ 3,150 full-cluster lab-ticks, > 2 creep lifetimes). So a boost the colony cannot supply from stock **right now** is not an option this tick — a comp sized *assuming* a boost it can't apply would be sized to a force it can't field, the exact class of silent under-fielding ADR 0031 exists to kill. **Availability is a hard input to the sizing, not a post-hoc filter.**

### 1.3 The asymmetry this closes (grounded in the current code)

Today the two sides are inconsistent:

- **Enemy boosts: fully modeled.** `analyze_hostile_creep` (`military/threatmap.rs:150-177`) reads each hostile part's real `boost()` state and applies a **conservative flat ×4** to ATTACK/RANGED/HEAL (`30/10/12 × 4`) and `100/0.3 ≈ 333` EHP to boosted TOUGH; `RoomThreatData.estimated_attack_dps`/`estimated_heal` carry the boosted values into `EnemyForce` (`military/squad_manager.rs:754-763`), read by BOTH the structure-breach `assess` and the EV path (ADR 0031 §2(f), the single `EnemyForce` channel). `war.rs` mirrors it (REC-066 `dismantle_boost`, `war.rs:202-209`; REC-067 `boost_multipliers`, `war.rs:573-587`).
- **Our boosts: modeled *nowhere it fields force*.** `defender_heal_parts_for_dps(incoming_dps, boosted: bool)` (`bodies.rs:121-127`) *already has a `boosted` parameter* that switches HEAL/part 12→48 — but **every caller passes `false`** (`force_sizing.rs:533,673`, `doctrine.rs:249`). `parts_for_rate` (`force_sizing.rs:753`), `SquadComposition::capabilities` (`composition.rs:265-283`), and `single_role_cap` (`composition.rs:319`) price parts with the **unboosted** engine constants only. `BodyType::required_boosts()` returns an **empty Vec unconditionally** (`composition.rs:131-133`, comment: "Force-`Sized` bodies are unboosted v1"). `CombatBodySpec` has **no boost field** (`bodies.rs:19-33`). `EconomySnapshot.available_boosts` is a **hollow HashMap**, initialized empty and never populated (`military/economy.rs:50,131,138,226`); `BoostQueue` (`military/boostqueue.rs`) is **inert end-to-end** — zero `request`/`mark_ready`/`is_ready`/`boost_creep` call sites (confirmed by tree-wide grep; ADR 0010 §"What is DEAD" still holds).

So we field forces sized as if we will *always* be unboosted, against enemies we (correctly, conservatively) assume *may* be boosted. That biases every comparison against us, over-defers winnable targets, and leaves the labs brewing 10k of every compound (`labs.rs`, ADR 0010) for **no combat consumer**. This ADR makes our own sizing boost-aware on **both** sides consistently.

### 1.4 Constraints every part of this must respect (EP-*)

- **The combat kernel is LIVE on MMO.** Any change must **degrade gracefully** — a boost is always an *upgrade path*, never a precondition; the unboosted comp is always the fallback (EP-3.8 "fail toward the safe posture").
- **No serialized-shape change without a WFV bump (EP-5.2/EP-5.3).** The one persisted surface a boost touches is `CombatBodySpec` (Serialize, rides in `BodyType::Sized` on the persisted `SquadComposition`, `composition.rs:76`), tracked by `WORLD_FORMAT_VERSION` (`game_loop.rs`). Everything else in the sizing path — `RequiredForce`, `ForceBudget`, `ForceAssessment`, `CompositionParams`, `EnemyForce` — is transient / not `Serialize`, so it is WFV-neutral (this is the same discipline ADR 0031 used to add the whole capability vector for free).
- **Keep shared abstractions generic (EP-2.4).** A boost tier is *data on the requirement + a knob on the search*, not an `is_boosted()` method on a generic trait. The multiplier lives with the body/part model (its owner), following the AGENTS.md §8 ladder.
- **Determinism (EP-6.13).** The boost-tier axis is a Vec-ordered integer ladder folded exactly like the TOUGH ladder — no HashMap reaches the decision.
- **Arithmetic before taste (EP-7.2).** Every ×4 is engine-cited; the availability numbers are `30 mineral + 20 energy/part`.

---

## 2. Decision — a boost tier is an EV-search axis with a hard availability gate

Add a **boost tier** to the force-composition pipeline as a first-class *dimension of the EV search*, priced into `fighting_strength` on both sides, gated by what the colony can actually supply, and carried to spawn on the one persisted body DTO. The unboosted path is the tier-0 floor and the always-available fallback.

The pipeline gains exactly one new concept threaded through the existing seams:

```
BoostTier  (a small enum: T0 unboosted … T3 catalyzed)
   │  chosen per-candidate by optimize_composition's new BOOST ladder
   │  clamped to what EconomySnapshot can supply for the fielding home (D2)
   ▼
part↔capability pricing (single_role_cap / capabilities / parts_for_rate / defender_heal_parts_for_dps)
   │  all take a BoostTier → apply the engine multiplier (D3)
   ▼
CombatBodySpec.boost: BoostTier   (the ONE new persisted field → one WFV bump, D4)
   ▼
SpawnRequest → SquadCombatJob "AwaitBoost" pre-deploy state → boostCreep at the labs (D5)
   ▲
enemy side already ×4 (threatmap) — sizing now boost-aware on BOTH sides consistently (D6)
```

### 2(a) — D1: The boost tier is an EV-search axis, not a fixed pick

**Decision.** `optimize_composition` (`composition.rs:542`) gains a **`BOOST_LADDER`** dimension, a sibling of the existing `OVER_POWER_LADDER` (`[1.0,1.25,1.5,2.0]`) and `TOUGH_LADDER` (`[0.0,0.1,0.2]`). The search becomes `over-power × TOUGH × boost`, and each `(k, t, tier)` candidate is:

1. built by `assemble_force` from `emit_requirement`'s per-objective requirement **priced at `tier`** (so a boosted candidate needs *fewer parts* for the same required capability — the ¼-parts effect of §1.1);
2. scored `EV(C) = P(win | C) · target_value − cost(C)` where **`cost(C)` now includes the boost cost** — `Σ boosted_parts × (BOOST_MINERAL_COST·w_mineral + BOOST_ENERGY_COST·w_energy)` — so the optimizer only pays for a boost when the P(win) it buys is worth its mineral+energy (the operator's smallest-favorable-force directive, ADR 0031 REC-011);
3. clamped to the **availability gate** (D2) — a tier the fielding home can't supply is skipped, exactly like an over-`MAX_SIZED_MEMBERS` candidate is skipped today.

The deterministic tie-break extends: **max EV, then lowest boost tier, then lowest k, then lowest tough, then fewest members** (as built in `composition.rs` — tier is compared FIRST so an unboosted tie never spends minerals) — so at equal EV we prefer the *cheaper, unboosted* comp (never spend minerals for no P(win) gain). This is *why* it's a search axis and not a threshold: `EV(C)` itself decides "is this fight worth boosting for?" — reproducing the grounded Overmind gate (boost when enemy dmg > 1500/tick or heal > 1000/tick, ADR 0031a §5) as an *emergent* outcome of the cost/benefit math rather than a hardcoded cutoff (the same "let EV decide, don't hardcode the archetype" principle ADR 0031 D17 applied to weapon selection).

**Why an axis and not a per-objective flag:** the same target is worth boosting or not depending on its `target_value`, the colony's mineral surplus, and the enemy's own boosts — all already inputs to `optimize_composition`. A flag would re-introduce the presumed-shape smell ADR 0031 deleted; the ladder lets the value/cost balance pick the tier per-fight.

### 2(b) — D2: Availability gating — supply-clamped, degrades to unboosted, never stalls

**Decision.** A candidate at `tier > T0` is only considered if the **fielding home's** lab/mineral/terminal stock can supply the compounds its body demands. The optimizer receives a `BoostSupply` (the boost-availability view for the home(s) in range) and, per candidate, computes the compound demand (`Σ parts_of_boosted_type × 30 mineral`, per compound) and **clamps the tier down** to the highest fully-suppliable tier — a T3 comp the colony can only supply T1 for is re-priced at T1, not skipped. If even T0 (unboosted) is the only suppliable tier, the search still returns the unboosted comp: **a boost is always an upgrade, its absence is never a stall** (EP-3.8, and the ADR 0010 §4 "boosts NEVER gate defense" rule — this ADR keeps that: defense fields immediately at T0 and *upgrades* if stock exists).

`BoostSupply` is populated from the now-real `EconomySnapshot.available_boosts` (the hollow field at `economy.rs:50` — populating it from lab+storage+terminal stock is a prerequisite this ADR shares with ADR 0010 L0; see §7 Phasing). The gate is **conservative**: it reserves the compounds against concurrent requests (so two squads don't both size to the same 3000-unit lab fill), reusing the reservation discipline ADR 0010 §4 designed for the BoostQueue.

**Consequence for graceful degradation:** because the availability clamp happens *inside* the search and the unboosted rung is always present, the LIVE bot's behavior at empty stock is **byte-identical to today** (T0 wins every candidate; no boost field is set; no `boostCreep` step). The boosted path only activates once the supply side (ADR 0010 L1/L2) actually stocks compounds — this is the safe incremental turn-on (EP-2.2 "behavior changes turn on only after parity is verified").

### 2(c) — D3: The sizing kernel — a `BoostTier` threaded into `fighting_strength`

**Decision.** Introduce a small `BoostTier` (an enum `T0 | T1 | T2 | T3`, or a per-part multiplier struct if the sweep later wants mixed tiers — see Open Question O2) that every part↔capability function takes and applies the engine multiplier for. The touch-list, all already the single pricing seams (EP-2.6, "one implementation per concern"):

- **`defender_heal_parts_for_dps(incoming_dps, boosted)`** (`bodies.rs:121`) — *already boost-aware* (12→48). Generalize `boosted: bool` → `tier: BoostTier` (T0=×1, T3=×4). This is the smallest possible change and the proof the seam was designed for it.
- **`parts_for_rate(rate, power)`** (`force_sizing.rs:753`) — the DPS→parts inverse. A boosted part delivers `power × mult`, so fewer parts satisfy `rate`. Thread the tier's multiplier for the relevant part (RANGED/WORK/ATTACK).
- **`SquadComposition::capabilities(max_energy)`** (`composition.rs:265`) — the forward parts→capability (`heal_per_tick`, `structure_dps`, `tank_effective_hp`). Multiply per boosted part-type; the `tank_effective_hp` uses `×3.3` for boosted TOUGH (matching the threatmap's `100/0.3` enemy model, §1.3). This makes a candidate's `P(win)` reflect its *actual boosted* strength.
- **`single_role_cap(role, probe_energy)`** (`composition.rs:319`) — the per-member part ceiling. A boosted member needs fewer parts for a given capability *and* a boosted MOVE frees fatigue budget (§1.1), so more weapon parts fit under the 50-part cap. Thread the tier so the cap reflects the boosted body.
- **`RequiredForce`** (`force_sizing.rs:638`) — **unchanged in shape** (it's *capability* demand, tier-agnostic — "field 200 dismantle-DPS worth of WORK"); the *tier* is applied when the requirement is turned into parts (`parts_for_rate` / `assemble_force`). Keeping `RequiredForce` tier-free preserves its "not Serialize, WFV-neutral" property and keeps the capability/parts split clean.

`fighting_strength` (the `win_probability` inputs — `caps.heal_per_tick`, `caps.structure_dps`, incoming, `required_kill`) is therefore boost-aware **by construction** once `capabilities()` prices boosted parts: no new strength formula, just the existing one reading correctly-priced parts. Bit-determinism holds (integer/ceil multiplies over the Vec-ordered ladder; the tier enum is `Copy`).

### 2(d) — D4: The spawn→lab→boost pipeline (the one persisted field + a pre-deploy lifecycle state)

**Decision.** The fielded body must record which tier it was sized for, so the spawn side knows which compounds to apply. Add **one field** to the DTO that already rides the persisted composition:

```rust
// bodies.rs — CombatBodySpec (already Serialize, in BodyType::Sized on the persisted SquadComposition)
pub struct CombatBodySpec {
    pub tough: u32, pub attack: u32, pub ranged_attack: u32,
    pub work: u32, pub carry: u32, pub heal: u32, pub claim: u32,
    #[serde(default)]                 // ← default T0 keeps every existing serialized body byte-compatible on read
    pub boost: BoostTier,             // ← THE ONE new persisted field → one WFV bump (D-WFV below)
}
```

`BodyType::required_boosts()` (`composition.rs:131`, today always empty) becomes real: it reads `spec.boost` and returns `[(compound, boosted_part_count)]`; `SquadComposition::required_boosts()` (`composition.rs:242`, the already-built aggregator that has had *zero callers*) finally gets its consumer — the spawn side and the availability gate.

**The lifecycle** (the spawn→lab→deploy cost, EP-2.7 "one lifecycle owner"):

1. **Spawn** — `squad_manager.rs:2259` `build_body` produces the `Vec<Part>` (unchanged); the boost demand is attached to the spawn callback (`create_spawn_callback`, `squad_manager.rs:2318`) so the new creep's `SquadCombatJob` knows its tier. The body itself is boost-agnostic (boosts apply post-spawn; `boostCreep` rejects a spawning creep — engine, ADR 0010 §3).
2. **`AwaitBoost` pre-deploy state** — `SquadCombatJob` (`jobs/squad_combat.rs`, today `MoveToRoom → Engaged/Retreating`) gains an **`AwaitBoost`** state between spawn and `MoveToRoom`: if the creep's tier > T0, route to the room-plan **boost tile** (ADR 0010 §2 — the stamp tile adjacent to ≤6 labs, one `boostCreep`/lab/tick), apply boosts, then proceed. This is the ADR 0010 §3 flow ("emerge → boost tile → boosted in ≤ walk + 1 tick"). The state is **bounded** (EP-4.5): a boost unfulfillable within a deadline **falls through to `MoveToRoom` unboosted** (the creep was already sized to a suppliable tier by D2, so this only fires on a mid-flight stock loss — degrade, don't hang).
3. **Boost application** is the *only* place a `boostCreep` intent is issued — routed through a boost station on the labs (ADR 0010 §4 fulfiller). This ADR is the **consumer** side; the *producer/fulfiller* (labs load the compounds) is ADR 0010's BoostQueue wire, which this ADR gives its first real caller (`SquadComposition::required_boosts()` → a `DemandId`-keyed request, EP-1.7 — **not** the `Entity`-keyed `BoostRequest` the inert queue has today, `boostqueue.rs:18-30`; keying is fixed as part of wiring, per ADR 0010 §4).

**End-of-life** rides ADR 0010 §5 unchanged (recycle-not-renew for boosted members; never renew — renew strips boosts with zero refund).

### 2(e) — D5: Enemy-boost symmetry — one consistent model on both sides

**Decision.** Our sizing now prices **our** boosts from the **same boost table** the enemy side reads, so a boosted-vs-boosted engagement is scored consistently, and — critically — the optimizer can *respond* to an observed boosted enemy by boosting up a rung (the `EnemyForce.boosted` flag, `doctrine.rs:93`, and the boost-inflated `EnemyForce.dps`/`heal` feed `incoming`/`required_kill`; a boosted enemy raises the required capability, and the boost ladder is how we meet it without blowing `MAX_SIZED_MEMBERS`). **As built (2026-08-23 write-back, see Design deltas):** the one table is `screeps-combat-decision::bodies::boosts` — the exact per-compound OUTPUT map (`output_multiplier_for`: tier-1 ×2 / tier-2 ×3 / catalyzed ×4 over the six military families) stamped per part onto `CombatBodyPart.boost_mult` for **both** sides of the tactical seam, and `BoostTier::output_multiplier()` (1/2/3/4) on our sizing side. The *strategic* enemy channel (`threatmap.rs::analyze_hostile_creep` → `EnemyForce`) keeps its conservative flat-×4 assumption for any boosted part (EP-8.2, don't "fix" correct-but-conservative code). The boosted-TOUGH damage-reduction curve (30/50/70%; the threatmap's `100/0.3 ≈ 333` EHP) is modeled on the enemy side only — our own TOUGH is priced unboosted (the 0019 boosted-TOUGH follow-up; conservative direction).

### 2(f) — D6: What it unblocks + the graceful-degradation contract

**Decision.** The boost layer directly unblocks:

- **T-COMP-1/5** — boosted offense/defense comps become *selectable* (the optimizer fields them when EV + supply justify).
- **T-TOWER-3** — a T3 HEAL+TOUGH comp can *direct-breach* a towered room (out-heal 4×, out-armor 3.3×) where only drain was viable unboosted (ADR 0031 §2(g)); drain remains the fallback when supply can't reach the needed tier.
- **T-NPC-7 / L3+ strongholds** — a boosted single squad clears targets the unboosted oracle correctly `None`-defers today (the "escalate-vs-abandon on `None`" partial answer, ADR 0031 §5 #38: boost *before* multi-squad — cheaper and already-single-squad).

**The degradation contract (the LIVE-safety spine):** at every layer, T0/unboosted is the floor and the fallback. Empty stock → T0 wins the search → byte-identical to today. Partial stock → clamp to the suppliable tier. Mid-flight stock loss → `AwaitBoost` falls through unboosted. A boost is *never* a precondition for fielding or defending — only an upgrade the EV+supply math opts into. This is what makes it safe to land on the live kernel incrementally (§7).

---

## 3. Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| **Boost tier as an EV-search axis + supply-clamp gate + one persisted `CombatBodySpec.boost` (chosen)** | Reuses the entire ADR-0031 pipeline (ladder pattern, `assemble_force`, EV cost/benefit, determinism); the tier *emerges* from value/cost like the weapon archetype (D17); degrades to T0 by construction so it's LIVE-safe; closes the enemy/own asymmetry with the same multiplier table; gives ADR 0010's BoostQueue its first real consumer | One WFV bump (the `boost` field); depends on the supply side (ADR 0010 L0/L1) being wired to actually stock compounds; adds a `BoostSupply` input + an `AwaitBoost` lifecycle state |
| **Hardcode boosted templates (a "boosted quad" body catalog)** | Simple to write | Re-introduces the exact static-template + `is_sized` smell ADR 0031 **deleted** (Phase 4b); can't size the *tier* to the fight or the supply; a mismatch (boosted body, no stock) silently under-fields — the failure class ADR 0031 exists to kill. Rejected on EP-2.6/EP-2.9. |
| **Threshold flag: "boost when enemy dmg > 1500/tick" (Overmind's literal gate)** | Grounded number (ADR 0031a §5); trivial | A fixed cutoff is a proxy for the real quantity (is the P(win) gain worth the mineral cost?); ignores `target_value` and supply; EP-7.5 "gate on the precise distinguishing property, not a proxy". We *reproduce* this gate as an emergent EV outcome instead. |
| **Model our boosts in `capabilities()` only, skip the search axis** | Smaller; makes existing comps read correctly if ever boosted | Nothing ever *sets* a boost, so `capabilities()` always reads T0 — inert, like `defender_heal_parts_for_dps(_, false)` today. Reading boosted strength is necessary but not sufficient; the *decision* to boost must live somewhere, and the EV search is where every other force-shaping decision lives (D16/D17). |
| **Do nothing (stay unboosted v1)** | Zero risk; drain already handles towers | Permanently cedes T-COMP/T-TOWER/T-NPC/L3+; leaves the labs brewing 10k of every compound (ADR 0010) for no combat consumer; keeps the enemy/own asymmetry that biases every engagement decision against us. The reconciliation calls this the highest-leverage gap. |
| **Boost application during spawn (Overmind "spawn-time boost")** | Zero unboosted-travel overhead | **Engine-impossible** — `boostCreep` rejects a spawning creep (`labs/boost-creep.js:20`, ADR 0010 §3, EP-7.1). The `AwaitBoost`-at-labs flow is the feasible equivalent (~one in-base walk of 1500t lifetime). |

---

## 4. Consequences

**Positive.**
- **The asymmetry is closed** — our sizing reads the same ×4/×3.3 boost table as the enemy model, so every engage/target/size decision is scored consistently (§2(e)); we stop conceding force we could field.
- **Winnable set grows** — boosted comps take T-COMP/T-TOWER/T-NPC/L3+ targets the unboosted oracle correctly defers; the "escalate on `None`" gap (#38) is partially closed *before* the heavier multi-squad path (cheaper).
- **The labs get a combat consumer** — `SquadComposition::required_boosts()` (a built-but-callerless aggregator, `composition.rs:242`) and `EconomySnapshot.available_boosts`/`has_boost` (`economy.rs:131`) finally get real callers; ADR 0010's inert BoostQueue gets its first producer. The demand this emits is the "direction" ADR 0010 §"Net" said the pipeline was missing.
- **LIVE-safe by construction** — the T0 floor + supply clamp + `AwaitBoost` fallthrough mean the change is byte-identical to today at empty stock and turns on only as supply materializes (EP-2.2 parity-first).
- **Reuses the pipeline** — no new sizing math; the boost axis is a ladder folded exactly like TOUGH (bit-deterministic, tournament-tunable via `CompositionParams` — the reserved `boost_tier` knob, ADR 0031a §4.6).

**Negative / costs.**
- **~~One WFV bump~~ — none.** The `CombatBodySpec.boost` field (D4) is additive with `#[serde(default)]` (T0), and under ADR 0047's tolerant stream that is reset-free (see D-WFV). The cost this ADR budgeted here did not materialize.
- **A hard dependency on the supply side.** The boost axis is *inert without stock*. The producer (labs stocking compounds on a military demand signal) is ADR 0010 L1/L2 — this ADR is the consumer and does not build the reaction planner. Absent that supply, the layer runs *dark* (the code exists, T0 always wins because `BoostSupply` is empty) — which is exactly the safe way to land it (§7 P0/P1).
- **New lifecycle state + tunables.** `AwaitBoost` adds a bounded pre-deploy hop; the boost cost weights (`w_mineral`, `BOOST_MINERAL_COST=30`, `BOOST_ENERGY_COST=20`) and the `BOOST_LADDER` join the `CompositionParams` sweep surface — the 0031b tuning re-opens (already flagged: the boost tier "reshapes every ratio", ADR 0031a §5(6)).
- **Value concentration risk.** A boosted member that dies loses ~30 mineral × its boosted parts (ADR 0010 §Consequences). Bounded by the existing retreat thresholds + the recycle-not-renew end-of-life (ADR 0010 §5) — and the EV cost term prices this into the decision (a fragile boosted comp scores worse).

**CPU / tick-safety.** The search grows by the `BOOST_LADDER` length (×4 at most: T0–T3) — a bounded constant-factor increase in an already-bounded (`4 × 3`) integer search, negligible. `boostCreep` intents are cheap (~0.2 each, ≤ boosted-parts once per creep, ADR 0010 §7). No pathfinding introduced (the boost tile is plan metadata, ADR 0010 §2). No new panic surface (the tier enum is total; `#[serde(default)]` covers decode).

---

## 5. Invariants (carried from ADR 0031 §3, extended)

- **Bit-determinism.** The boost axis is a Vec-ordered `BOOST_LADDER` folded with integer/ceil multiplies; the tie-break is total (max EV → **lowest tier** → lowest k → lowest tough → fewest members). No HashMap reaches the decision. The `emit_requirement`/`optimize_composition` run-twice fences (ADR 0031 §4) extend to cover the tier.
- **T0 is the floor and the fallback, everywhere.** No layer may make a boost a precondition for fielding or defending (the degradation contract, D6). A grep for a boost check *gating* a spawn or a defense field returns empty (the ADR-0010 "boosts never gate defense" rule, made a combat-layer invariant).
- **Capability/parts split preserved.** `RequiredForce` stays tier-agnostic (capability demand); the tier is applied only at the parts boundary (`parts_for_rate`/`assemble_force`/`capabilities`), keeping `RequiredForce` non-`Serialize` / WFV-neutral.
- **One boost multiplier table.** The output multipliers live in one place — `bodies::boosts` (`output_multiplier_for` per compound, `BoostTier::output_multiplier` per tier, `tier_compounds` per family) — read by the tactical seam for both sides and by the sizing ladder for ours (EP-2.9); a second, drifting boost table is a design smell. (The threatmap's flat ×4 and `100/0.3` TOUGH EHP are the strategic enemy channel's deliberate conservative over-estimate, not a second table of engine truth.)
- **Availability is a sizing input, not a filter.** A tier is *chosen* against supply inside the search (clamped down), never fielded-then-checked; a boosted comp is never assembled that the fielding home can't supply (the "sized to a force we can't field" class stays impossible, ADR 0031's whole point).
- **Boost application is the sole `boostCreep` site.** Exactly one place issues the intent (the `AwaitBoost`/boost-station step, D4); an out-of-lifecycle `boost_creep` is a smell (EP-2.7 one owner).

---

## 6. Decisions (D1–D6 + D-WFV)

- **D1 — Boost tier is an EV-search axis.** Add `BOOST_LADDER` to `optimize_composition` (sibling of `OVER_POWER_LADDER`/`TOUGH_LADDER`); each candidate is priced at its tier, `cost(C)` includes the boost mineral+energy, and the EV math decides whether to boost. Emergent gate, not a hardcoded threshold. Tie-break prefers the lowest (cheapest) tier at equal EV.
- **D2 — Supply-clamped availability gate; degrades to T0.** A candidate's tier is clamped down to the highest fully-suppliable tier for the fielding home (`BoostSupply` from the now-populated `available_boosts`), reserved against concurrent requests; T0 is always present, so absence of stock is never a stall. Defense fields at T0 immediately and upgrades if stock exists (ADR 0010 §4 rule kept).
- **D3 — One `BoostTier` threaded into the pricing seams.** `defender_heal_parts_for_dps` (generalize its existing `boosted: bool`), `parts_for_rate`, `capabilities`, `single_role_cap` all take a `BoostTier` and apply the engine multiplier. `RequiredForce` stays tier-agnostic. No new strength formula — `fighting_strength`/`win_probability` become boost-aware because `capabilities()` prices boosted parts.
- **D4 — One persisted field + a bounded pre-deploy lifecycle state.** `CombatBodySpec.boost: BoostTier` (`#[serde(default)]` = T0); `required_boosts()` reads it and gets its first caller; `SquadCombatJob` gains a bounded `AwaitBoost { tier }` state that walks to whichever home lab the `LabsMission` has loaded for it (as built — not the plan boost tile; the tile is a later routing refinement), applies boosts one compound per visit, and falls through unboosted on the age deadline (`AWAIT_BOOST_DEADLINE` = 300) / stock-loss. The sole `boostCreep` site. BoostQueue keyed by the **creep name** (the stable per-member identity that exists today; ADR 0011's minted `DemandId` supersedes it when the spawn orchestrator lands), never the inert `Entity` key.
- **D5 — Enemy-boost symmetry.** Price our boosts from the same `bodies::boosts` table the tactical seam reads for the enemy (exact per-compound ×2/×3/×4 via `CombatBodyPart.boost_mult`; `BoostTier::output_multiplier` on the sizing ladder); the optimizer *responds* to `EnemyForce.boosted` by climbing the boost ladder. Strategic enemy model unchanged (stays conservative flat-×4). Own-side TOUGH reduction not yet modeled (0019).
- **D6 — Unblocks T-COMP/T-TOWER-3/T-NPC-7/L3+, with the T0 degradation contract as the LIVE-safety spine.** Boost is always an upgrade path; unboosted is always the fallback.
- **D-WFV — no `WORLD_FORMAT_VERSION` bump.** Originally "one loud reset for the `CombatBodySpec.boost` write shape"; **obviated by ADR [0047](0047-reset-tolerant-serialization.md)** (which post-dates this ADR): an additive `#[serde(default)]` field decodes every pre-field payload as T0 with no reset — the T-HEAL-3a `effective_hits` precedent. Host landing was never gated on a bump (EP-5.2 / ADR 0031 D12).

---

## 7. Phasing / what lands first (incremental, LIVE-safe — EP-2.1/EP-2.2)

The stable seams hidden behind each step: the ADR-0031 composition pipeline (ladders + `assemble_force`), the `CombatBodySpec` DTO, `SquadCombatJob`'s state machine, and ADR 0010's BoostQueue/boost-station. **Sizing first, gated on availability** — nothing boosts until supply is real, so the early phases are *dark* (present in code, T0 always wins) and turn on only when stock materializes.

1. **P0 — Sizing kernel, boost-aware, dark (Breaking: None; WFV: none).** Thread `BoostTier` into the pricing seams (D3) + add `BOOST_LADDER` to `optimize_composition` (D1) with `BoostSupply` **hardwired empty** (T0 always wins). `RequiredForce` untouched. **Validate (host):** every existing calibration gate (`OracleCalibration`/`SizingWins`/`CreepClearWins`, ADR 0031 §4) is **byte-unchanged** (empty supply ⇒ T0 ⇒ identical fielding); a new kernel test asserts that *given synthetic T3 supply*, a towered bed the unboosted oracle `None`-defers becomes winnable at T3 (the T-TOWER-3 proof). Determinism fences extended to the tier. Deployable on its own — dark, zero behavior change.
2. **P1 — Populate `available_boosts` + the supply clamp (Breaking: None; WFV: none).** Fill `EconomySnapshot.available_boosts` (`economy.rs:226`, the hollow field) from lab+storage+terminal stock (shared prerequisite with ADR 0010 L0), wire `BoostSupply` into the optimizer (D2). Now the boost axis *can* fire — but only for a home that already holds stock (the labs brew 10k of everything today, ADR 0010, so some T3 may already be on hand). **Validate:** a home with synthetic stock fields a boosted comp for a high-value towered target; a home with no stock is byte-identical to P0; the reservation prevents two squads double-booking one lab fill.
3. **P2 — The persisted field + spawn attach (Breaking: None as built — `#[serde(default)]` under ADR 0047; the "one WFV bump" this phase planned is obviated).** Add `CombatBodySpec.boost` (D4); `required_boosts()` reads it; attach the tier to the spawn callback. **Validate:** a boosted spec round-trips through serialize/deserialize; `#[serde(default)]` decodes an old (boost-less) body as T0; the acceptance test `oracle_sized_force_forms_and_kills_a_defended_core` (ADR 0031 §4) still passes at T0.
4. **P3 — The `AwaitBoost` lifecycle + `boostCreep` via the boost station (Breaking: Behavioral).** `SquadCombatJob` gains the bounded `AwaitBoost` state (D4); the boost station (ADR 0010 §4 fulfiller) loads compounds and the creep applies `boostCreep` at the plan boost tile; deadline fallthrough unboosted. This is where a boost *actually applies* live. **Validate (offline lifecycle harness, ADR 0028):** a boosted squad forms → routes to the boost tile → boosts → deploys → kills a towered core that the unboosted comp cannot; a stock-loss mid-`AwaitBoost` falls through to an unboosted deploy (no hang — the bounded-attempt proof, EP-4.5).
5. **P4 — Tournament re-sweep (Breaking: None).** Re-run the 0031b `CompositionParams` sweep with the boost axis + cost weights (the sweep the boost tier "reshapes every ratio" for, ADR 0031a §5(6)); record the emergent boost-gate thresholds and compare to the grounded Overmind numbers (dmg>1500 / heal>1000). **Validate:** the sweep is Pareto-improving over unboosted on the towered/L3+ beds and neutral on the unboosted beds; seeds recorded in a 0041-companion results note if they change.

**Breaking-change summary:** P0/P1 — **None** (dark, WFV-neutral). P2 — **None as built** (additive `#[serde(default)]` field, reset-free under ADR 0047; the ADR's original "Memory-format / one WFV bump" line is superseded). P3 — **Behavioral** (boosts actually apply, behind `features.military.boost_military`). P4 — **None**. **No state-drop anywhere; the supply dependency (ADR 0010 L0/L1) is the real gate on P1+ having any effect.**

---

## 8. The five design questions, and how they were decided

The ADR opened five questions (O1–O5); each is decided below. The options and the ADR's own
recommendations are retained under §8.1 as the rationale the decisions were taken against — where a
decision differs from the recommendation, the "delta" column says why.

| # | Question | **Decision** | Notes / delta from recommendation |
|---|---|---|---|
| O1 | Supply sequencing | **Dark-first consumer** (rec. (b)) | Land P0–P2 consumer-side, WFV-neutral + byte-identical until the labs stock compounds; the demand-driven supply planner (ADR 0010 L1/L2) follows once the consumer is proven. |
| O2 | Tier granularity | **Uniform tier per body** (rec.) | One `BoostTier` per body; the search axis stays 4-wide (T0…T3). Per-part mixing is a Tier-3 follow-up only if a sweep shows it pays. |
| O3 | Which tiers | **FULL ladder T0→T1→T2→T3** (⟂ rec. of T0→T3-only) | Operator wants the whole ladder as the EV-search axis so T1/T2 can be *picked* where they EV-win, not pre-excluded. **Consequence:** the supply side (O1's later phase) must stock the intermediate reagent chain (T1/T2 compounds), not just catalyzed T3; the `BOOST_LADDER` in `optimize_composition` carries all four rungs; P4's sweep validates which rungs actually win per bed. Uniform granularity (O2) keeps the search cost bounded (4-wide, not tier^part). |
| O4 | Cost currency | **MARKET price WITH a validated-approximation fallback** (⟂ rec. of fixed constant; refined by the operator: "there may be no trustworthy market price so a validated approximation may still be needed") | Price the boost's mineral cost at live compound market value **when that price is trustworthy**, else fall back to a **validated approximation** — because Screeps markets are frequently thin/absent/manipulated (no orders, one-sided books, cold-start, outlier spikes). So the producer computes `mineral_value_e` per compound via a **trust-gated resolver**: (1) use the live market price only if it passes a validity check (sufficient order depth / recent volume / within a sane band vs the reagent's base-mineral floor); (2) else fall back to the validated approximation — a **cost-of-production floor** derived from the base-mineral market values + the reagent-chain energy (what it costs the colony to *make* the compound), clamped to a conservative constant band; (3) a fixed conservative constant as the final cold-start default. This is a small **pure `mineral_value_e(compound) -> f64` valuation kernel** (base-mineral prices + chain recipe in, energy-equivalent out — game-free, deterministic, unit-testable per EP-6.2/8.3); the war.rs producer feeds it the resolved base-mineral prices + the trust verdict, then threads the single scalar into `optimize_composition` so the decision crate stays game-free (EP-6.13). **Consequence:** the cost term is robust to an untrustworthy market (never wildly over/under-values a boost because the order book is empty), and the approximation is itself validated in P4's sweep (the boost cost must not distort the EV-selection surface the 0031b seeds were tuned on). |
| O5 | Defense-boost scope | **Offense-first; defense T0-immediate** (rec.) | Defense keeps the always-field T0 floor (never waits on labs); it gains the boost-upgrade path only once pre-loaded-lab supply (ADR 0010 L1) is proven, so defense responsiveness is never traded for a boost. |

**Net v2 design:** boost-aware sizing on the offense path, a **full T0→T3 uniform-per-body ladder** as an EV-search axis, cost priced at **market value with a validated-approximation fallback** (a pure `mineral_value_e` valuation kernel, trust-gated, threaded as a resolved scalar so the decision crate stays game-free), **supply-clamped with a T0 floor** so empty stock is byte-identical to the unboosted bot, landing **dark-first** (WFV-neutral P0; the persisted `CombatBodySpec.boost` field brings the one loud reset at **P2**, not P0). Three operator refinements (O3 full-ladder, O4 market-priced-with-validated-fallback, and the untrustworthy-market caveat) trade more optimality/robustness for the lean defaults — the phasing (dark-first, T0 floor, kernel purity) preserves live-safety regardless.

### 8.1 The questions as posed (options + the ADR's recommendations)

1. **Supply-side ownership & sequencing.** This ADR is the *consumer*; ADR 0010 L1/L2 (the empire ReagentPlanner + boost stations that actually stock compounds on a combat demand signal) is the *producer*. Do we (a) build the minimal ADR-0010 L0/L1 supply slice as part of this initiative (so P1+ has real stock), or (b) land P0–P2 dark against the labs' existing autonomous 10k-of-everything brew (which already stocks *some* T3) and defer the demand-driven planner? Recommendation: **(b) first** — land the consumer dark, prove it fires against whatever the autonomous labs already hold, then build the demand signal when the consumer is proven.
2. **`BoostTier` granularity: uniform vs per-part.** Model the tier as one enum for the whole body (T0–T3, simplest, matches how the labs stock catalyzed T3), or per-part (a body could be T3-HEAL + T1-MOVE)? Uniform is the ADR-0031a §5 reserved shape and the simplest sweep; per-part is more optimal but multiplies the search. Recommendation: **uniform T0–T3 for v2**, per-part as a Tier-3 follow-up if the sweep shows it pays (mirrors the archetype/tier deferral discipline, ADR 0031 D17).
3. **Which tier first (the "T1 first" cost/benefit).** The prompt says T1 first — but T1 boosts are a *weaker* multiplier for *less* mineral refinement (the T3 catalyzed ×4 is the decisive one; T1 is a smaller bump). Do we ladder T0→T1→T2→T3 (gradual, cheaper stock) or T0→T3 (only the decisive tier, simpler stock, skip the marginal middle)? Recommendation: **T0→T3 only for v2** (the middle tiers rarely pay for their extra lab chain-time — ADR 0010 §arithmetic; the ×4 is what unblocks the blocked targets); expose the full ladder only if the sweep (P4) shows T1/T2 EV-wins on some bed.
4. **The boost cost currency.** `target_value` is energy-equivalent (`value_e`, `objective_value.rs`); the boost mineral cost needs a mineral→energy exchange rate to enter `cost(C)` on the same axis. Use the market price of the compound (ADR 0012's price feed) or a fixed conservative constant? Recommendation: a **fixed conservative `BOOST_MINERAL_VALUE_E` constant** for v2 (the market-priced version is an ADR-0012-coupled follow-up), tuned in P4.
5. **Defense-boost latency.** ADR 0010 §4 keeps a *pre-loaded defense-boost lab* so the first defense wave boosts with zero haul latency. Is a boosted *defense* comp in scope for v2, or is v2 offense-only (defense stays T0-immediate, the current always-field floor)? Recommendation: **offense-first**; defense stays T0-immediate and gains the boost-upgrade path only once the pre-loaded-lab supply (ADR 0010 L1) is proven, so defense responsiveness is never traded for a boost.

---

## Design deltas (2026-09-07)

- **§7 P3 validated offline — the lifecycle half (WS-CLOSE Phase B, ADR 0028 harness).**
  `screeps-combat-eval/src/harness/lifecycle.rs::run_boosted_forming(BoostedFormingScenario)` mirrors
  the as-built P3 state machine as harness state, in the live tick order: the PRODUCER (`SquadManager`
  re-files each boosted-tier member's `remaining_for_parts` into the ephemeral `BoostQueue` while it is
  at home, inside the age window, with unboosted parts) → the FULFILLER (`LabsMission::service_boosts`:
  one lab per distinct compound in file order, READY only when the lab can hold the creep's full
  30/part from the home's stock) → the CONSUMER (`AwaitBoost::tick`: feature/T0 release, fully-boosted
  release, the age deadline, else walk to the ready lab and apply one compound per visit). The harness
  does not depend on the bot crate: `AWAIT_BOOST_DEADLINE` (300) is MIRRORED (`pub(crate)` in
  `jobs/squad_combat.rs` — the mirror carries a MUST-mirror note), the stock is a plain ledger, and the
  boost tile is a walk distance. Pins (each RED-verified by flipping the fixture):
  `boosted_roster_routes_to_the_labs_and_departs_boosted` (T3 duo, stock = `required_boosts()` →
  departs fully boosted, one `boostCreep` per family compound, a few ticks after its T0 twin);
  `stock_loss_mid_await_boost_falls_through_at_the_deadline` (stock lost mid-`AwaitBoost` → both members
  depart unboosted at the last member's `AWAIT_BOOST_DEADLINE`, no hold — the EP-4.5 bounded contract)
  with its RED twin `without_the_deadline_a_stock_loss_holds_the_roster_forever`;
  `t0_rosters_never_touch_the_boost_stage` (no request, no visit, byte-identical to `run_forming`);
  `boost_stage_is_inert_while_boost_military_is_off`; `boosted_forming_is_deterministic`.
- **The "kills a towered core the unboosted comp cannot" half of the P3 validate line** is the engine
  gate `t3_twin_decisively_beats_unboosted_twin` (`screeps-combat-eval/src/tournament.rs`) — the bed
  above covers forms → routes to the labs → boosts → deploys; the two together are the §7 P3 proof.
  Not covered offline: the transfer-system haul latency into the lab (modelled as "stock ⇒ loaded"),
  the plan boost-tile geometry, and the one-lab-per-compound overflow beyond six families.

### WS-CLOSE write-back (ws-5 boost pipeline + WS-VAL boost seam) — the layer as built

Written back from the WS-5 / WS-VAL implementation docs on 2026-09-07 (docs/implementation/README.md rule 5) and verified
against the code. Body sentences corrected in place the same day: §2(e)/§5/§6 D5 (the "same ×4/×3.3
model … shared EHP constant" framing), §6 D4 (plan boost tile; `DemandId` key), §4/§6/§7 (the WFV bump).

- **The own-side pricing seam (WS-VAL 2026-08-23 — the boost-blind fix, the parity keystone).** The
  tactical kernels never priced *our* boosts: an EV-sized T3 squad read its own heal at ¼ value,
  `assess_engage` called every towered fight unwinnable, and the squad kited forever (the stronghold-
  gauntlet freeze — live and sim identically). The fix is ONE primitive on both sides of the seam:
  `CombatBodyPart.boost_mult` (`screeps-combat-decision/src/lib.rs`; ctors `new` = ×1, `boosted`) and
  `CombatCreepDto::effective_output(part, per_part)` = Σ working parts × `per_part × boost_mult`
  (the engine's `calcBodyEffectiveness`). Every power consumer reads it: `heal_reaching`,
  `threat_value`, `best_heal_target`, `assess_engage`'s creep dps, `kite_threats`. The compound→multiplier
  map is `bodies::boosts::output_multiplier_for` — exact over the 18 military compounds (six families ×
  T1 ×2 / T2 ×3 / T3 ×4; any other compound ×1). Live adapters: `jobs/squad_combat.rs::creep_to_dto`
  stamps the multiplier from `BodyPart::boost()`; `military/squad.rs` `heal_power` is recomputed
  **every tick** and boost-multiplied (it was computed once and latched — destroyed HEAL parts kept
  counting for the creep's whole life; parity H4/M16); `squad_manager.rs` member views sum effective
  parts. Sim adapter: `screeps-combat-agent::creep_dto` stamps the `SimBody` tier. Pins:
  `effective_output_prices_boosts_and_skips_dead_parts`, `heal_reaching_reads_boosted_healers_at_real_strength`,
  `output_multiplier_map_matches_engine_tiers` (decision), `creep_dto_stamps_boost_multipliers` (agent),
  `t3_twin_decisively_beats_unboosted_twin` (eval, end-to-end).
- **P0a — the tier axis, as built (`composition.rs`).** `bodies::BoostTier { T0, T1, T2, T3 }`
  (`Ord`, `Default` = T0, serde) with `output_multiplier()` 1/2/3/4; `BOOST_LADDER` is the outermost
  loop of `optimize_composition`, filtered to `<= params.boost_max_tier`. Per tier the WINNABILITY
  assessment is re-emitted against a tier-scaled ceiling (`tier_budget`: `max_heal_per_tick` and
  `max_dismantle_dps` × m into `emit_requirement`) — so a target the T0 ceiling calls unwinnable is
  genuinely re-assessed at each suppliable tier (the T-TOWER-3 unlock); `boost_scaled` divides the
  requirement's output + heal channels by m (ceil, floored at 1; TOUGH/CLAIM pass through); the
  boost cost is priced over the REAL built body (`comp_total_parts` × `CompositionParams.boost_cost_e_per_part[m-2]`,
  cold-start constants `[120, 200, 300]` e per part = O4 stage 3 — the market-fed `mineral_value_e`
  resolver is still unbuilt; over-pricing is the safe direction); tie-break max EV → lowest tier →
  lowest k → lowest TOUGH → fewest members. `CompositionParams.boost_max_tier` defaults to T0, which
  is the DARK mode: byte-identical to the unboosted optimizer. `tank_effective_hp` is deliberately not
  tier-scaled (own-side TOUGH reduction = the 0019 boosted-TOUGH item). Debug: `SZ_DEBUG=1` prints the
  per-tier requirement + per-candidate `p_surv`/`p_kill`/EV.
- **P0b re-scoped.** The optimizer-internal per-tier emit already delivers D3's winnability effect;
  the remaining seam-signature churn (`boosted: bool` → tier on `defender_heal_parts_for_dps` and
  friends) waits for a consumer, and TOUGH-reduction is the 0019 item. Nothing blocked P3 on it.
- **P1 — supply, as built.** `EconomyAssessmentSystem` (`military/economy.rs`) populates the
  previously-hollow `RoomEconomyData.available_boosts` per home from storage + terminal + every lab,
  over `tier_compounds(T1..T3)` (the 18 compounds). The pure clamp `bodies::boosts::max_supplied_tier(stock, MIN_BOOST_STOCK)`
  returns the highest tier whose ENTIRE six-family set has ≥ `MIN_BOOST_STOCK` (1 000 units ≈ one
  ~33-part member) — fully-suppliable per family, T0 floor, never a stall. `operations/war.rs` derives
  the offense clamp as the **max over homes** (per-home, not empire-pooled: scattered compounds cannot
  boost one body uniformly) and threads it as `CompositionParams.boost_max_tier`; the caller-side
  skip honors the boosted verdict (`doctrine.honor_verdict() && !plan.winnable() && plan.composition.is_none()`
  — a fielded comp at a suppliable tier is not vetoed by the T0 assessment). Defense stays T0 (O5).
  **Not built:** D2's reservation against concurrent requests — the physical fulfiller (one lab per
  compound, per-creep readiness) is the only serialization today.
- **P2 — the persisted tier.** `CombatBodySpec.boost: BoostTier` with `#[serde(default)]`; the
  optimizer stamps the winning rung onto every sized slot's spec (`BodyType::Sized`) — a T0 win leaves
  the comp byte-identical to pre-P2. `BodyType::required_boosts()` is real: per fielded family,
  `30 × parts` of the tier compound, MOVE counted at the derived Plains-parity count the builder uses;
  `SquadComposition::required_boosts()` aggregates. No spawn-side field: the job re-derives the tier from
  its slot's spec (`create_spawn_callback(…, slot_boost_tier)` → `SquadCombatJob::new_with_squad_boosted`).
- **P3 — the apply wire (producer → fulfiller → consumer), all behind `features.military.boost_military`.**
  `BoostQueue` (`military/boostqueue.rs`) is ephemeral, keyed by creep name, with **owner-staged
  clears** matching the tick order PreRun → RunMission (labs fulfil) → SquadManager (produces) →
  RunJob (consumes): `clear_ready` at tick start in `game_loop`, `clear_requests` by the producer right
  before re-filing. Producer: the `SquadManager` re-files, every tick, for each member whose slot tier
  > T0 that is still in a home room, inside `AWAIT_BOOST_DEADLINE`, with unboosted parts, the
  body-derived remaining set `bodies::boosts::remaining_for_parts(parts, tier)` at `BoostPriority::High`
  (no serialized progress — a boosted part is done). Fulfiller: `LabsMission::service_boosts` assigns
  one lab per distinct compound (labs sorted by id, compounds in priority-file order), files the
  transfer-system loads (withdraw foreign contents; deposit `30/part` compound + `20/part` energy),
  `mark_ready`s each request whose lab already holds that creep's full need, and pauses the reaction
  FSM (`LabsState::wait(5)`) while requests pend — the ADR 0010 §2 "temporary re-role" of reaction labs
  (a reacting lab may boost; there is no lab-cooldown check). More compounds than labs ⇒ the overflow
  waits, bounded by the requester's deadline. Consumer: the `AwaitBoost { tier }` job state (appended
  variant of the serde-plain state enum) releases immediately when the flag is off or the tier is T0,
  departs once `remaining_for_parts` is empty, departs unboosted past the age deadline (300 ticks),
  else walks to the ready lab and calls `boost_creep` (the sole site), one compound per visit;
  with nothing ready it loiters clear of the spawns on the rally leash. Renew skips any creep with a
  boosted part (ADR 0010 §5; `squad_manager.rs` renew filter). Offline proof: the ADR 0028 bed above.
- **P4 — the sweep, as merged (WS-VAL Phase 4.5 item 6).** The §7 P4 line asked for a
  `CompositionParams` re-sweep with emergent boost-gate thresholds compared to the Overmind numbers;
  what was built instead grades the **tactical kernel profile** under boosts, because the corpus
  showed the unboosted tuning did not generalize (default tactics ≈ best at T0 but lost to
  `focus_ball` at T2 and to `ranged_duel_kite`/`anti_aoe_spread` at T3). Instruments
  (`screeps-combat-eval/src/tournament.rs`, `#[ignore]`): `boosted_tier_retune` (regimes = T0/T2/T3
  mirror baskets, maximin) and the ADOPTION instrument `joint_boosted_terrain_retune` — 3 boost tiers ×
  3 terrain classes (synthetic / imported / generated) = 9 cells over `chokepoint_comp_basket` boosted
  per tier, ranked by min-over-cells. Caveat recorded in the code: `comp_basket`/`boosted_comp_basket`
  are synthetic-bed-only, so a tier-only run has empty terrain cells — the joint instrument builds
  from the chokepoint basket. Result under the RULING-9 one-currency EV: the previous R19 winner
  `a0-i3-d14-K3-s2` regressed (worst cell −852, mean +39, rank 16/57); no config is positive in all
  nine cells (the literal maximin is the untuned default), so adoption used **maximin-with-a-noise-band**
  and `SquadTacticParams::open_combat()` became `a2-i6-tight` (approach 2, incumbency 6, cohesion k 2,
  discohesion 20, spacing 1; two noise-scale negative cells −26/−18 vs mean +652). The sizing-side
  re-sweep (0031b seeds + emergent boost thresholds) remains open.
- **Activation.** `features.military.boost_military` (`features.rs`, default `false`) is the single
  switch for the sizing clamp, the producer and the consumer; effects are demand-driven (nothing
  changes until war sizing picks a tier and labs fill the queue). Operational note carried from the
  workstream (provenance only): the flag was flipped ON live on 2026-08-24 via the rest-api console
  example (`screeps-rest-api/examples/console.rs`) — the live `Memory._features` tree is persisted and
  shadows code defaults, so any features change must be applied there too — and no boosted engagement
  had occurred by this write-back; the first one is the watch item.
- **What remains of this ADR's design.** O4's trust-gated market `mineral_value_e` resolver (constants
  govern); D2's supply reservation; the 0031b sizing re-sweep half of P4; own-side boosted TOUGH
  (0019) and the seam-signature generalization (P0b); the plan boost-tile routing; and the supply
  side itself — ADR 0010 L1/L2 (the labs still brew the autonomous flat 10k of every compound).
