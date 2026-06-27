# ADR 0026 — Objective/Information-Dependent Combat Strategy-Selection Layer

- **Status:** IMPLEMENTED (2026-06-26) — pluggable `CombatStrategy` trait registry + `decide_strategy(ctx, &collection)` in `screeps-combat-decision/src/strategy.rs`; wired into the bot at the one seam (`compute_squad_orders`, `squad_manager.rs`) via `classify_objective` + `StrategyInfo`; per-objective gate in `tournament.rs`. NO kill-switch (operator: target the final state). **Implementation finding (determinism follow-on now LANDED, 2026-06-26):** base-attack absolute scoring WAS noise-dominated (~1% cross-process), so the objective split rests on the ROBUST open-combat self-play win (`open_combat()` = `a1-i6-tight`, exploitability 0) + the dismantle-needs-range-1 PRINCIPLE, not a measured base-attack lead. The noise was two seed-ordered hash iterations in `screeps-rover`'s resolver (the topological move-order budget consumption + `current_pos_to_entity` last-write-wins on a two-creep tile stack); both are now fixed and the sim is **bit-deterministic** (`sim_is_deterministic_over_rounds`, spread 0 over 5 fresh-seed rounds), so base-attack is now a reliable tuning signal and a clean re-tune is possible.
- **Builds on (unchanged):** ADR 0008/0008a (squad FSM + `SquadManager` lifecycle), ADR 0019 (`KiteScoreParams` term math), ADR 0020 §12 (force-sizing oracle, `DefenseProfile`/`assess`), ADR 0025 (the EV-of-(position×action) kernel + `KernelParams` tuning seam), ADR 0025 §12 (the realistic re-tune that motivates this ADR).
- **Crates touched:** `screeps-ibex` (bot: `military/squad_manager.rs`, a new `military/strategy.rs`), `screeps-combat-decision` (a single pure `strategy_for` selector + its input enum — host-shared), `screeps-combat-eval` (`tournament.rs` per-objective profile harness).
- **Serialization:** none (per-tick decision; no `WORLD_FORMAT_VERSION` bump — see §6).

---

## 1. Context & motivation

The combat decision crate's EV kernel (`plan_squad_ev`, `screeps-combat-decision/src/kernel.rs:339`) is driven by a five-coefficient `KernelParams` struct (`kernel.rs:281-293`):

```rust
pub struct KernelParams {
    pub approach_coef: i64,      // default 2 — downhill pull toward the objective
    pub incumbency_coef: i64,    // default 3 — dead-band holding a firing tile
    pub discohesion_coef: i64,   // default 10 — centroid-cohesion pull past K
    pub cohesion_k: u32,         // default 3
    pub spacing_coef: i64,       // default 1 — anti-stack penalty
}
```

This `KernelParams` is wrapped in `SquadTacticParams` (`kite.rs:241-262`) alongside the kite/engage/healer `KiteScoreParams` presets, and flows to the kernel through exactly one seam: `decide_squad_with_pathing(view, shared, tactics, …)` (`lib.rs:1453`) calls `plan_squad_ev(…, &tactics.kernel)` (`lib.rs:1645-1659`).

**The problem: the bot ships ONE fixed profile for every squad, every objective.** The live adapter `compute_squad_orders` hardcodes `SquadTacticParams::default()` at both call sites (`screeps-ibex/src/military/squad_manager.rs:653` and `:657`), regardless of whether the squad is razing a bunkered base or skirmishing in open field.

**The realistic re-tune (ADR 0025 §12 Stage 4) proves no single global `KernelParams` wins everything.** The host tournament — foreman-planned bases over real imported terrain, plus open-combat self-play — produced two findings that point in opposite directions (`0025-ev-position-action.md:331-334`):

- **Open combat (21 beds):** the shipped default `k-default` (`approach_coef=2`) is **robust — exploitability 431 net HP ≪ GROSS 1500** (no hard counter). The field/Nash leaders (`k-spread`, `k-tight-coh`) each *regress* base attack, so the §12 adoption protocol (no base-attack regression) keeps the default for open combat.
- **Base attack (26 real foreman + imported `Raze` bases):** strongly position-**sensitive**. `k-approach-hot` (`approach_coef=4`) **dominates: +21154 net, vs every other config deeply negative (~−25k to −29k)** — the default kernel *chips at the rampart ring and bleeds creeps instead of breaching*; approaching hard cracks the ring. But `k-approach-hot` is the **worst** config in open combat (−118 mean payoff).

The ADR 0025 §12 verdict (`0025-ev-position-action.md:334`):

> **Adoption:** no single `KernelParams` wins both lenses → the principled fix is an **objective-aware approach coefficient** (weight `approach_coef` up when the objective is a STRUCTURE/base, default for open-creep combat)… the shipped default is unchanged (robust + the breach gap is closed by objective-awareness, not a global bump that would forfeit open-combat robustness).

This ADR is that fix: a thin **strategy-selection layer** that picks the per-squad weight profile from the squad's **objective** and the **information** the bot already has about the target room, slotted into the existing FSM at the one seam that flows weights to the kernel. It is a strategic layer over the kernel — it does **not** change the kernel's math, the FSM, or any serialized state.

---

## 2. Decision

Introduce a pure selection function

```
strategy_for(objective_kind, info) -> SquadTacticParams
```

that maps `(objective kind × information signals)` to a `SquadTacticParams` weight profile, and **call it at the single existing weight-injection seam** in the FSM's tactical-orders phase, replacing the hardcoded `SquadTacticParams::default()`:

- **Integration point (the one and only seam):** `compute_squad_orders` in `screeps-ibex/src/military/squad_manager.rs:650-659`. Today both branches pass `SquadTacticParams::default()` into `decide_squad_with_pathing`. This ADR replaces those two literals with `strategy_for(kind, info)`, where `kind` is already threaded into `compute_squad_orders` (via `objective_target`/`is_formation_objective`, `squad_manager.rs:286-289`) and `info` is assembled from intel the adapter already reads.
- **Phase placement (FSM-respecting):** this is **Phase B2 — compute per-squad tactical orders** (`squad_manager.rs:275-300`), which already runs `decide_squad_with_pathing` for every live squad every tick. The engage/retreat gate (`assess_engage` + hysteresis) runs **first, inside `decide_squad`, unchanged**; this layer only chooses *how to fight* once the gate has committed (exactly as ADR 0025 §2.3 frames the kernel). It reads the objective and room state; it **does not** modify the FSM, the squad lifecycle, membership, or the engage/retreat decision.

**This AUGMENTS the FSM; it does not replace it.** The squad lifecycle (Forming → Moving → Engaged → Retreating), `CombatObjectiveQueue` reconciliation (Phase A, `:207-249`), roster fielding (Phase B, `:251-273`), and objective claiming (Phase C, `:302-340`) are untouched. The only change is *which constants* the already-present per-tick decision call uses. Because the profile is recomputed each tick from live state, the layer is also self-correcting (a squad that arrives at a room and discovers a tower picks the breach profile on the tick it sees it — no latched per-squad state, consistent with [[prefer-per-tick-optimal-over-hysteresis]]).

---

## 3. Design

### 3.1 Inputs

**(a) The objective kind** — the primary discriminator. `ObjectiveKind` (`screeps-ibex/src/military/objective_queue.rs:81-94`): `Secure`, `Defend`, `Dismantle { pos }`, `Harass`, `Farm { kind }`, `Escort`. Read at `squad_manager.rs:286` off `data.objective_queue.get(*obj_id)`. This already drives the `formation` flag (`is_formation_objective`, `:89-91` — only `Dismantle` fights as an oriented box); the strategy layer extends that binary split into a weight profile.

**(b) The information signals** — each with where it is read. All are *already computed* bot-side; the layer reads them, it does not create new intel. v1 keeps the set deliberately small (the two the re-tune proved decisive plus the hard vetoes):

| Signal | Type / source | Where read | Why |
|---|---|---|---|
| **target_is_structure** | `bool` — `matches!(kind, Dismantle{..})`, or "the room has hostile structures and no killable hostile creeps" from `view.structures` | `ObjectiveKind` (`:286`); `view.structures` (`squad_manager.rs:619`, built by `build_room_combat_dtos` `:606`) | THE re-tune lever: structure/base objective ⇒ `approach_coef` high; creep objective ⇒ default. |
| **enemy_safe_mode** | `bool` | already computed at `squad_manager.rs:610-614` and on `view.enemy_safe_mode` | Safe mode ⇒ no damage possible ⇒ never spend approach risk; force the conservative profile (the `assess` hard veto, `force_sizing.rs:134`). |
| **tower_pressure** | `bool`/small enum from energized hostile towers | `RoomThreatData.hostile_tower_positions` + `.tower_energy` (`military/threatmap.rs:76,99`), already on the room entity | A towered base needs the approach-hot breach profile (the re-tune's foreman bases all have tower rings); an open skirmish does not. |
| **winnability mode** | `Option<AssaultMode>` (`Breach`/`Drain`) | `force_sizing::assess` (`force_sizing.rs:124`) output — already produced by war.rs at field time (`war.rs:960`) | The force-sizing oracle ALREADY classifies the assault: `Breach` ⇒ approach-hot + dismantle-through; `Drain` ⇒ tank-soaks-then-breaches (patience/cohesion). Free, exact signal. |
| **threat_level** | `ThreatLevel` (`threatmap.rs:42`) | `RoomThreatData.threat_level` | `Defend` against a `PlayerSiege` may want a different posture than against a lone `Invader`; v1 uses it only as a coarse gate, reserved for tuning. |

The signals deliberately **excluded from v1** (gaps noted, deferred as tuning surface, not v1 machinery): per-creep weight variation (the kernel takes one `SquadTacticParams` for the whole squad — `lib.rs:1456`), multi-room context (intel is single-room — `threatmap.rs` is per-room), RCL-graduated profiles, intel-confidence/staleness weighting, and cross-squad coordination. These are §11-style follow-ons; v1 ships the smallest set the re-tune proved decisive.

### 3.2 Output

A `SquadTacticParams` (`kite.rs:241-262`) — the **existing** container, unchanged. It flows down the **existing** seam: `strategy_for(kind, info)` returns it, `compute_squad_orders` passes it to `decide_squad_with_pathing(&view, …, profile, …)` (`squad_manager.rs:653/657`), which routes `&profile.kernel` into `plan_squad_ev` (`lib.rs:1658`). **No new output type, no new plumbing** — the layer substitutes a value at a call site that already takes that exact type. The kite/engage/healer `KiteScoreParams` fields ride along unchanged in v1 (only `kernel` varies); leaving them as tuning surface for later objectives (e.g. a future `Harass` profile that reweights the kite preset).

### 3.3 The selection mechanism — a pure table/rules function (recommended)

**Recommendation: a small, explicit rule table over `(kind, info)` → named profile, NOT a learned/continuous policy.** Rationale:

1. **The re-tune already produced discrete winners per regime** (`0025-ev-position-action.md:332-334`): `k-default` for open combat, `k-approach-hot` for base breach. The decision surface the data supports is *categorical* (open-creep vs structure-breach vs safe-mode-veto), not a smooth function — a lookup table is the faithful encoding of the evidence we have.
2. **Determinism + parity** (ADR 0020 §6, ADR 0025 §7): the kernel is integer-only and deterministic; a table-lookup selector is trivially deterministic and wasm-safe (no floats in the *selection*, no `game::*` calls — it lives in the pure decision crate). A learned/continuous policy adds an inference path, float weights, and a model artifact to serialize/version — all debt this layer is explicitly trying to avoid.
3. **Tournament-tunable per profile** (§4): each named profile is one `KernelParams` constant set the harness tunes independently. A table of named profiles maps 1:1 onto the tournament's existing `Strategy` population (`tournament.rs:46-49`) — the harness already constructs and ranks named profiles; the table is just "which named profile per objective".
4. **Least debt, fits the FSM**: it is a `match` returning a `const`-derived struct. No state, no allocation, O(1) per squad per tick (the CPU constraint at `squad_manager.rs`'s linear loop), no serialization.

The continuous/learned alternative is evaluated and rejected in §5.

### 3.4 Concrete new types / functions / files

**New — in `screeps-combat-decision` (pure, host-shared so the tournament and the bot select identically):**

```rust
// screeps-combat-decision/src/strategy.rs  (new file)

/// The strategic objective class the selector keys on — a kind-agnostic projection of the bot's
/// `ObjectiveKind` (the decision crate must stay JS/bot-free, so it gets the *class*, not the bot enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatObjectiveClass {
    /// Open-creep combat: clear/deny/defend against creeps, no rampart ring to crack
    /// (Secure / Defend / Harass / Farm with no structure objective).
    OpenCombat,
    /// Break a defended structure objective behind a rampart/wall ring (Dismantle / a base raze).
    StructureBreach,
}

/// The information signals the selector reads (all pre-computed bot-side; the crate just reads them).
#[derive(Clone, Copy, Debug, Default)]
pub struct StrategyInfo {
    /// Enemy safe mode active in the target room → zero damage possible (the assess hard veto).
    pub enemy_safe_mode: bool,
    /// At least one ENERGIZED hostile tower covers the objective tile.
    pub tower_pressure: bool,
    /// The force-sizing oracle's chosen assault mode, when the producer ran it.
    pub assault_mode: Option<AssaultMode>,   // from force_sizing::AssaultMode
}

/// THE strategic layer: objective class × information → the weight profile the kernel fights with.
/// Pure, deterministic, integer-only — the ONLY new decision logic this ADR adds. Each arm returns a
/// named, tournament-tuned `SquadTacticParams` (the constants are the §4 tuning surface).
pub fn strategy_for(class: CombatObjectiveClass, info: StrategyInfo) -> SquadTacticParams {
    // Hard veto first: nothing is winnable under safe mode → never spend approach risk.
    if info.enemy_safe_mode {
        return SquadTacticParams::default(); // robust/conservative; the engage gate retreats anyway
    }
    match class {
        CombatObjectiveClass::StructureBreach => match info.assault_mode {
            // Drain: tank soaks the towers dry, THEN breach — patience over a hot approach.
            Some(AssaultMode::Drain) => SquadTacticParams::breach_drain(),
            // Breach (or unknown mode but a structure objective): approach hard to crack the ring.
            _ => SquadTacticParams::breach_hot(),
        },
        // Open-creep combat: the robust, low-exploitability shipped default.
        CombatObjectiveClass::OpenCombat => SquadTacticParams::default(),
    }
}
```

**New profile constructors on `SquadTacticParams` (`kite.rs`, beside `default()` at `:253`):**

```rust
impl SquadTacticParams {
    /// Base-breach profile (ADR 0025 §12: `k-approach-hot` dominates real foreman rampart rings,
    /// +21154 vs ~−25k for the default). Only `kernel.approach_coef` differs from default in the
    /// v1 seed; the tournament tunes the full kernel per objective (§4).
    pub fn breach_hot() -> Self {
        Self { kernel: KernelParams { approach_coef: 4, ..KernelParams::default() }, ..Self::default() }
    }
    /// Drain-then-breach profile: a tank soaks tower fire until the towers run dry, then the squad
    /// breaches. Patience + cohesion over a hot approach (seed = default until the tournament tunes it).
    pub fn breach_drain() -> Self { Self::default() }   // seed; §4 tunes
}
```

**Bot-side mapping (one small adapter fn — keeps the bot enum out of the pure crate):**

```rust
// screeps-ibex/src/military/strategy.rs  (new, small)

/// Project the bot's ObjectiveKind + live room intel into the decision crate's selector inputs.
pub fn classify(kind: &ObjectiveKind, structures: &[CombatStructureDto], creeps_present: bool) -> CombatObjectiveClass {
    match kind {
        ObjectiveKind::Dismantle { .. } => CombatObjectiveClass::StructureBreach,
        // Any objective whose room presents valuable hostile structures and no killable creeps is a
        // structure breach in practice (e.g. Secure of a deployed stronghold) — the kernel is already
        // shooting the core/ramparts at that point.
        _ if !creeps_present && has_valuable_structures(structures) => CombatObjectiveClass::StructureBreach,
        _ => CombatObjectiveClass::OpenCombat,
    }
}
```

**Files to touch:**

| File | Change |
|---|---|
| `screeps-combat-decision/src/strategy.rs` | **NEW.** `CombatObjectiveClass`, `StrategyInfo`, `strategy_for`. Pure, unit-tested. |
| `screeps-combat-decision/src/kite.rs` (`:253`) | Add `breach_hot()` / `breach_drain()` constructors beside `default()`. |
| `screeps-combat-decision/src/lib.rs` | `pub mod strategy;` re-export. |
| `screeps-ibex/src/military/strategy.rs` | **NEW.** `classify()` + `build_strategy_info()` (assemble `StrategyInfo` from `RoomThreatData` + `enemy_safe_mode` + the `AssaultMode` carried on the objective — see §6). |
| `screeps-ibex/src/military/squad_manager.rs` (`:650-659`) | Replace `SquadTacticParams::default()` with `strategy_for(classify(kind, …), info)`. Thread `kind` (already at `:286`) + `info` into `compute_squad_orders`. |
| `screeps-ibex/src/military/squad_manager.rs` (`:157-167`) | Add `threat_data: ReadStorage<'a, RoomThreatData>` to `SquadManagerSystemData` so the adapter can read `tower_pressure`/`threat_level` for the target room (the component already lives on room entities). |
| `screeps-combat-eval/src/tournament.rs` | Per-objective profile harness (§4). |

---

## 4. Tuning integration

The realistic harness already proves the open ↔ base-attack divergence (`0025-ev-position-action.md:331-334`). This ADR ties each **named profile** to its **own per-objective tournament**, so the harness tunes per-objective profiles, not one global config.

**The harness seam already supports this.** `tournament.rs` constructs named strategies (`Strategy { name, tactics }`, `:46-49`), injects them via `ManagedSimSquad::with_tactics` (`screeps-combat-agent/src/squad.rs:268`), and ranks them by mean payoff + exploitability + meta-Nash (`run_tournament_over_comps`, `:261`). The two lenses already exist:

- **Open-combat profile** (`OpenCombat` → `default()`): tuned and validated by the existing `realistic_comp_basket` (`tournament.rs:221`) open-combat tournament. The gate is **robustness**: `exploitability ≤ GROSS` (the shipped default scores 431 ≪ 1500, `0025-ev-position-action.md:332`). This profile is the §12 default — already adopted, no change.
- **Breach profiles** (`StructureBreach` → `breach_hot()` / `breach_drain()`): tuned by `realistic_base_scenarios` (`tournament.rs:237`) — the foreman + imported `Raze`/`Breach` bases — scored by `assault_score` (HP razed + destroyed bonus + attacker survival, `harness/validate.rs`). The gate is **per-objective best**: the profile that maximizes `assault_score` over the realistic base set (today `k-approach-hot` at +21154).

**New harness fns (extend `tournament.rs`, build no new mechanism):**

```rust
/// Tune the StructureBreach profile: rank a KernelParams population over `realistic_base_scenarios`
/// by `assault_score`. Returns the best (the per-objective adoption candidate). Mirrors the existing
/// `base_attack_ranking`, but its OUTPUT is "the profile to bake into breach_hot()", not a dashboard row.
pub fn tune_breach_profile(pop: &[Strategy]) -> (&'static str, KernelParams, /*score*/ i64) { … }

/// Per-objective robustness check: a breach profile MUST NOT be wildly exploitable in open combat
/// (a squad that mistakenly fields it in a creep fight should not get hard-countered). Run the chosen
/// breach profile through the open-combat exploitability gate; record its open-combat exploitability
/// alongside its base-attack score. (It need not WIN open combat — it won't be selected there — only
/// not be a free kill, since a mid-fight reclassification can switch a live squad into it.)
pub fn validate_breach_profile_open_robustness(breach: SquadTacticParams, pop: &[Strategy]) -> i64 { … }
```

**Per-objective validation contract (the adoption protocol, per profile):**

1. **OpenCombat:** lowest-exploitability config over `realistic_comp_basket`; re-run `exploitability ≤ GROSS`. (Unchanged — the shipped default.)
2. **StructureBreach/Breach:** highest `assault_score` over `realistic_base_scenarios` (`tune_breach_profile`); **plus** a bounded-exploitability check in open combat (`validate_breach_profile_open_robustness`) so a reclassification mid-fight is not a free kill.
3. **StructureBreach/Drain:** seed = default until the harness adds a tower-energy-bounded drain scenario; tune against `assault_score` on the `Drain`-mode bases once the scenario lands (deferred, seed shipped).
4. **Adoption** (per ADR 0025 §12 step 5): record each adopted profile's constants + its per-objective ranking + its cross-objective robustness in this ADR's ledger (§8). A `KernelParams` change is a decision-crate constant — **no `WORLD_FORMAT_VERSION` bump** (`0025-ev-position-action.md:348`).

A new CI gate `per_objective_profiles_are_each_best_in_class` asserts: `strategy_for(StructureBreach, Breach)` beats `default()` on `realistic_base_scenarios`, and `default()` beats `breach_hot()` on `realistic_comp_basket`. This is the regression fence — it would have caught "we globally bumped `approach_coef` and forfeited open-combat robustness", the exact failure §12 warns against.

---

## 5. Alternatives considered

**(a) Keep a single global `KernelParams` (status quo).** *Rejected.* The re-tune is decisive: no single config wins both lenses. `k-default` cannot breach real foreman rampart rings (chips and bleeds, ~−25k), and `k-approach-hot` is the worst open-combat config (−118 mean, and exploitable) (`0025-ev-position-action.md:333`). A global bump trades one failure for another. This is precisely what motivates the ADR.

**(b) Bake objective-awareness INTO the kernel's EV math directly** (e.g. make `approach_coef` a function of "is the focus a structure?" inside `plan_squad_ev`). *Rejected as the mechanism, with one nuance.* Trade-offs:
   - *Against:* it couples the kernel's pure per-(tile×action) math to objective semantics the kernel deliberately does not know — ADR 0025's whole thesis is "no role archetype, no objective branching in the kernel; formation emerges from one currency" (`0025-ev-position-action.md:27`). Threading objective kind into the kernel re-introduces exactly the strategic conditioning ADR 0025 pushed *out*. It also makes the tuning surface harder to reason about (the coefficient is now data-dependent inside the hot loop) and the tournament can no longer A/B named profiles cleanly (`tournament.rs:46`).
   - *Nuance kept:* the kernel *already* prices structures vs creeps (`V_struct`, breach-inherited value, ADR 0025 §2.4) — so "the kernel knows it's hitting a structure" is true at the *value* level. What it must NOT do is condition its *position-shaping coefficients* on that. The clean separation: **the kernel prices outcomes; the strategic layer picks the position-shaping weights.** Keeping `approach_coef` selection in a layer above `plan_squad_ev` preserves the kernel as a pure, tournament-comparable function of its `KernelParams`.

**(c) A learned/continuous policy** (map a feature vector → continuous `KernelParams` via a small learned model). *Rejected for v1.* Trade-offs:
   - *Against:* adds a model artifact (weights to serialize + version → a `WORLD_FORMAT_VERSION` concern this design otherwise avoids), a float inference path on a deterministic integer hot path (parity risk, ADR 0020 §6 / ADR 0025 §7), and training/eval infra — heavy machinery for a decision surface the data shows is essentially categorical (two regimes). It also undermines tournament comparability (a continuous policy is not a discrete `Strategy` the population gate ranks).
   - *When it'd be right:* once the input set grows (RCL graduation, multi-room, intel confidence, enemy composition) the categorical table may get unwieldy and a learned selector over `realistic_*` scenarios becomes attractive. The table is the v1 floor; the meta-Nash mixed strategy the tournament already produces (`tournament.rs:277`, the §11-#8 adaptivity bridge) is the natural upgrade path. Deferred, not foreclosed.

**Chosen: (the table, §3.3).** Faithful to the evidence (categorical regimes), zero serialization, deterministic/wasm-safe, O(1), and 1:1 with the existing tournament `Strategy` population. Least debt, fits the FSM, tournament-tunable per objective.

---

## 6. Consequences

**Technical debt.** Minimal and bounded. One pure function + two profile constructors + one small bot adapter + one new `SystemData` field. No new FSM states, no new lifecycle, no new persistent component. The selector is a `match`; adding an objective profile later is one arm + one tuned constant set. The main *latent* debt is the `StrategyInfo` plumbing (getting `AssaultMode`/`tower_pressure` to the seam) — see below.

**Serialization / `WORLD_FORMAT_VERSION`.** **No bump.** The selected `SquadTacticParams` is per-tick, recomputed in Phase B2 each tick, never stored (consistent with ADR 0025 §6: "no `WORLD_FORMAT_VERSION` bump — pure per-tick decision"). One sub-decision on where `AssaultMode` comes from:
   - *v1 (no serialization):* re-derive `tower_pressure` live from `RoomThreatData` (already serialized, already on the room entity) at the seam, and treat `assault_mode` as `None` (the `StructureBreach` arm falls back to `breach_hot()` when mode is unknown — the correct default for a towered base). This needs **zero new serialized fields**.
   - *Optional follow-on (one serialized field, justified separately):* if telemetry shows the `Breach`/`Drain` distinction materially changes outcomes, carry the producer's `AssaultMode` on `CombatObjective` (`objective_queue.rs:147` — already `Serialize`) so the squad uses the *producer's* oracle verdict rather than re-deriving. `CombatObjectiveData` is `#[serde(default)]` (`objective_queue.rs:182`), so adding an `Option<AssaultMode>` is forward-compatible — **but bincode is positional, so it would still gate a `WORLD_FORMAT_VERSION` bump** (cf. the `tower_energy` 14→15 note, `threatmap.rs:96`). v1 deliberately avoids this; the field is added only if the drain distinction earns it.

**CPU.** O(1) per squad per tick — a `match` + a few `bool` reads. Negligible against the per-squad target-flood (`TARGET_FLOOD_OPS = 2500`, ADR 0025 §6). `RoomThreatData` is read once per target room (it is already build-once-per-room-shared alongside `PositionLayers`, `squad_manager.rs:284`).

**Testability.** The selector is a pure function — host-unit-tested with no ECS (the same pattern as `is_formation_objective`/`objective_target`, which have unit tests at `squad_manager.rs:784-828`). Tests assert: `StructureBreach + Breach → breach_hot`, `StructureBreach + safe_mode → default`, `OpenCombat → default`, and the per-objective tournament gate (§4). The decision crate already host-tests `KernelParams` variations via the tournament.

**Migration / rollout.**
   - **Default profile = today's behavior.** `OpenCombat → default()` and safe-mode → `default()` mean every objective that is *not* a structure breach gets byte-identical behavior to today. The only behavioral change is `Dismantle`/breach objectives switch to `breach_hot()` — the exact case the re-tune shows the default *loses*. So the change is strictly a fix to a known-failing case, with no regression surface on the working cases.
   - **Kill-switch.** Add `pub strategy_selection: bool` to `MilitaryFeatures` (`features.rs:336-360`, default `true`), gating the selector. When `false`, `compute_squad_orders` passes `SquadTacticParams::default()` exactly as today — instant revert via `Memory._features` without a redeploy (the same override path the existing military flags use, `features.rs:368`). This is cheap insurance for a combat change; it is removed once the profiles are proven on a soak (per the [[combat-overhaul-initiative]] deploy-and-watch discipline).
   - **Deploy gating.** Same as any combat change: ADR 0020 §10 Docker-soak → operator go-ahead; never deploy MMO without explicit go-ahead.

---

## 7. Implementation plan

Ordered, minimal-debt increments. Each leaves the workspace compiling with the relevant tests green so the harness stays a usable gate. The new code is gated behind the kill-switch until the per-objective tournament gate is green.

**Step 1 — Pure selector + profiles (decision crate).** Add `screeps-combat-decision/src/strategy.rs` (`CombatObjectiveClass`, `StrategyInfo`, `strategy_for`) and `SquadTacticParams::breach_hot()` / `breach_drain()` (`kite.rs:253`). Unit tests: each arm returns the expected named profile; safe-mode forces default. `cargo test -p screeps-combat-decision`. *No bot change yet — pure, isolated, host-green.*

**Step 2 — Per-objective tournament gate (harness).** Add `tune_breach_profile` + `validate_breach_profile_open_robustness` to `screeps-combat-eval/src/tournament.rs` and the CI test `per_objective_profiles_are_each_best_in_class` (§4). Confirm `breach_hot()` beats `default()` on `realistic_base_scenarios` and `default()` beats `breach_hot()` on `realistic_comp_basket` — i.e. re-confirm the §12 finding with the *named* profiles. `cargo test -p screeps-combat-eval --lib`. *This is the regression fence; it must be green before the bot wires it in.*

**Step 3 — Bot adapter + seam swap (gated).** Add `screeps-ibex/src/military/strategy.rs` (`classify`, `build_strategy_info`). Add `threat_data: ReadStorage<RoomThreatData>` to `SquadManagerSystemData` (`squad_manager.rs:157`). Thread `kind` + `StrategyInfo` into `compute_squad_orders` and replace `SquadTacticParams::default()` at `:653`/`:657` with `strategy_for(classify(…), info)` **behind `features.military.strategy_selection`** (default-off-equivalent until proven: when the flag is off, pass `default()`). Add `strategy_selection: bool` to `MilitaryFeatures` (`features.rs:336`, default `true`). Existing `squad_manager` unit tests stay green; add a test that `classify(Dismantle) == StructureBreach` and `classify(Defend) == OpenCombat`. `cargo test -p screeps-ibex --lib military`.

**Step 4 — Tune + adopt the breach profile.** Run the realistic re-tune machinery (ADR 0025 §12 Stage 4, already reusable) to confirm/refine the `breach_hot()` constants; bake the adopted `KernelParams` into `breach_hot()`; record the per-objective ranking + cross-objective robustness in the §8 ledger. Re-green oracle-calibration / single-room-oscillation / self-play-decisive / Lanchester-floor / action-oscillation (the ADR 0025 §12 step-4 gate set). **No `WORLD_FORMAT_VERSION` bump.**

**Step 5 — Soak + (optional) `AssaultMode` plumbing.** Docker soak A–D (per [[combat-overhaul-initiative]]) watching the breach-objective outcomes + the seg-57 cohesion canary. If the soak shows the `Breach`/`Drain` distinction matters, do the optional `CombatObjective.assault_mode` follow-on (§6) as a *separate, WFV-gated* change. Operator go-ahead, then MMO deploy. Remove the kill-switch once proven.

**Gating summary:** every step gated on the existing decision/agent/eval/bot test suites **plus** the new per-objective tournament gate (`per_objective_profiles_are_each_best_in_class`, Step 2). The bot path is inert (kill-switch / default-equivalent) until Step 4's tournament adoption is green.

---

## 8. Adoption ledger (filled at Step 4)

**Implementation note:** §3.3 specified a `match`-based table; the shipped implementation is a **pluggable `CombatStrategy` trait registry** (operator refinement) — each strategy is an activator + a profile, `decide_strategy(ctx, &collection)` takes the collection (first-match-by-priority), so strategies are added/removed by editing the collection. Standard registry: `SafeModeHold` (veto) → `DrainBreach` → `Breach` → `OpenCombat`.

| Objective class | Mode | Profile (`KernelParams`: approach/incumbency/discoh/K/spacing) | Basis | Adopted |
|---|---|---|---|---|
| OpenCombat | — | `open_combat()` = **a1/i6/d20/K2/s2** (`a1-i6-tight-s2`) | spacing re-tune (2026-06-26) winner: +169 vs the real-opponent field, beats the old spacing-1 (+135) at equal exploit. The original grid fixed spacing=1, so its "exploit 0" was a blind spot — Screeps AoE is pure Chebyshev, so a tight blob eats stacked RMA/tower fire; spacing 2 sheds it (see ADR 0026a) | ✅ |
| StructureBreach | Breach / unknown | `breach()` = **a1/i4/d10/K3/s1** (`a1-i4-def`) | low approach (don't over-commit — winnable force breaches anyway) + LOWER incumbency than open ⇒ move in to range-1 and dismantle. Rests on the dismantle PRINCIPLE + the open win (base-attack was noise-dominated when chosen; now bit-deterministic, re-tunable) | ✅ |
| StructureBreach | Drain | `breach_drain()` = **a1/i6/d10/K3/s1** | breach + hold longer through the tower-drain soak (incumbency 6) | ✅ seed |
| StructureBreach | + safe mode | `open_combat()` (veto) | a shielded base takes zero damage — never spend approach risk | ✅ |

> **Why approach stays LOW (thorough re-tune, ADR 0025 §12):** the original approach=4 `breach_hot` seed (a 6-config quick run) did NOT replicate at 48-config scale — with a winnable-sized force, base-attack is weakly discriminating and a hot approach just bleeds creeps. The open-combat optimum is low-approach/high-incumbency/tight (`a1-i6-tight`, unexploitable). Base-attack absolute scores carried a ~1% cross-process noise floor WHEN the breach profile was chosen, so it is NOT chosen by a base-attack lead — it is the principled "move in to dismantle" variant of the open winner. **The determinism follow-on LANDED (2026-06-26):** the noise was two seed-ordered hash iterations in `screeps-rover`'s resolver (topological move-order budget + `current_pos_to_entity` tile-stack collision); both fixed, sim now bit-deterministic (`sim_is_deterministic_over_rounds`), so a clean base-attack re-tune is now possible.

---

## 9. Extension — objective & force-composition selection (the *doctrine* registry)

- **Status:** RUNG 1 IMPLEMENTED (2026-06-26) — `screeps-combat-decision/src/doctrine.rs` (`ForceDoctrine` trait registry + `decide_doctrine` + the `NpcCore`/`SiegeBreach`/`SecureRoom`/`HarassRemote` doctrines + `EngagementContext`/`EnemyCoordination`/`ForcePlan`); the bot's `war.rs` offense `match` and the eval's 3 `assess`+`siege_quad().sized_for` sites BOTH route through `decide_doctrine` → `plan` (the **parity** the operator required — one selection+sizing path, shared budget via `SquadComposition::force_budget`). Behaviorally a no-op (same compositions, same sizing); decision 138 / bot 150 / eval 48 green, wasm clippy clean, sim still bit-deterministic (spread 0), **no WFV**. **No kill-switch** — shipped to the final state per the operator's strategy-layer precedent (a verified no-op). The `Coordinated` square-law sizing primitive (`force_sizing::clear_force`) is also **built + host-tested** (the keystone for rungs 2–3, still unwired). **L3a (2026-06-26): defender selection is now UNIFIED onto the registry** — both defense sites route through `defense_doctrines()` → `GarrisonDefense`, and the parallel `DefenseEscalation` 3-bucket enum + `from_threat` are **deleted** (debt removed; behavior-preserving selection, spawn-path sizing unchanged). All remaining work is tracked in the **§9.10 deferred-work ledger**. Design + Q1–Q3 resolved below.

### 9.1 Motivation — the same activator-registry, one layer up

§3–§8 select the kernel **weight profile** (*how* a squad fights) from a pluggable `CombatStrategy` registry. Two adjacent decisions are still **hardcoded**; the operator's ask is to give them the same treatment:

1. **Objective selection** — *what* to do in a target room (clear / breach / suppress / harass / deny / hold). Today `war.rs`'s offense loop `match`es `TargetSource` → an `ObjectiveKind` + priority inline.
2. **Force-composition selection** — *who* fights (solo / duo / quad / blob) and at *what size*. Today the same `match` returns a hardcoded `SquadComposition`; only the `InvaderCore` arm runs the force-sizing oracle (ADR 0020 §12). `DefenseEscalation::from_threat` (`war.rs:101`) is a coarse threshold precursor on the defense side (dps/heal/count → Solo/Duo/Quad — a rule, but un-sized, three-bucket, enemy-blind).

This is **ADR 0020 §12.7(A)** ("archetype selector — *which* roles, not just how many parts") made concrete, and it carries the **one axis §12.7 does not yet model: how the enemy fights.** The current oracle *aggregates* `enemy_dps` — correct for a player whose creeps focus-fire **together**, but it over-sizes against NPCs (invaders, three SK keepers) that are fought **individually, one at a time**. The sizing math must branch on that — it is the crux of this section.

### 9.2 Three sibling registries, one chain

The doctrine registry is a structural **twin** of the strategy registry — same activator-first-match shape, same pure-decision-crate home, same bot-agnostic context projection. It runs **cold** (once per target/candidate), and its output's objective class **feeds** the strategy registry's per-tick `class` input:

```
intel ─► decide_doctrine(EngagementContext) ─► ForcePlan { objective, sized composition, winnable }
                                                     │ objective class
            manager spawns the sized composition     ▼
            each tick:  decide_strategy(StrategyContext{class}) ─► weight profile ─► kernel
```

No layer re-enters another's hot loop: doctrine = once per target (cold, may run the oracle), strategy = once per squad per tick (hot, O(1)), kernel = per creep per tick. The doctrine is the missing *first* link — today the offense `match` hardcodes what it should decide.

### 9.3 The doctrine trait (mirror of `CombatStrategy`)

```rust
// screeps-combat-decision/src/doctrine.rs  (new — pure, host-shared so bot + tournament decide identically)

/// How the opposing force fights — the axis that selects the sizing math (operator 2026-06-26).
pub enum EnemyCoordination {
    /// NPCs (invaders, SK keepers) + scattered defenders: engaged ONE AT A TIME. The binding
    /// constraint is the WORST SINGLE unit (out-heal its dps, out-last its hits); the squad never
    /// faces the SUM of their dps at once. Sizing target = max-single; kill-time = serial.
    Individual,
    /// A player's combat creeps fight TOGETHER (focus-fire + mutual heal). The binding constraint is
    /// the AGGREGATE under a square-law Lanchester — our force must OVER-match theirs (the ratio
    /// counts quadratically), not merely match it. Sizing target = Σ dps / Σ heal.
    Coordinated,
}

/// What a doctrine activator reads — the objective intent + expected opposing force + budget. Bot-
/// agnostic (the bot projects its enums/intel into this, §9.6), exactly as StrategyContext is.
pub struct EngagementContext {
    pub objective: CombatObjectiveClass,    // §3.4 — extended to the full ObjectiveKind projection
    pub coordination: EnemyCoordination,    // ← the new axis
    pub defense: DefenseProfile,            // towers/breach_hits/objective_hits/enemy_dps/heal/safe_mode (§12)
    pub worst_single: Option<UnitThreat>,   // for Individual: the strongest single enemy (dps/heal/hits)
    pub importance: u8,                     // OBJECTIVE_PRIORITY_* → investment scale (R5)
    pub home_energy: u32,                   // strongest in-range spawn energy (the sizing ceiling)
    pub time_budget: u32,                   // CREEP_LIFE_TIME − spawn − travel
}

/// A pluggable engagement doctrine: a named ACTIVATOR + the FORCE PLAN it fields. Add/remove = one
/// entry in the decide_doctrine collection (order = priority). Pure, deterministic, Sync — so the
/// tournament can rank a collection across parallel matches.
pub trait ForceDoctrine: Sync {
    fn name(&self) -> &'static str;
    fn applies(&self, ctx: &EngagementContext) -> bool;     // the classifier
    fn plan(&self, ctx: &EngagementContext) -> ForcePlan;   // runs the oracle + sizing internally
}

pub struct ForcePlan {
    pub objective: CombatObjectiveClass,
    pub composition: Option<SquadComposition>,  // already sized (assess → sized_for at ctx.home_energy); None = defer
    pub winnable: bool,                         // oracle go/no-go — skip if false, like the InvaderCore gate
}

/// First doctrine whose activator fires (collection order = priority) — the twin of decide_strategy.
pub fn decide_doctrine<'a>(ctx: &EngagementContext, doctrines: &'a [Box<dyn ForceDoctrine>])
    -> Option<&'a dyn ForceDoctrine> { doctrines.iter().map(|d| d.as_ref()).find(|d| d.applies(ctx)) }
```

`plan()` is self-contained (it calls `assess` + `sized_for` with `ctx.home_energy`), so a doctrine is a pure `ctx → ForcePlan` function — host-unit-testable and tournament-rankable with no ECS, exactly like a strategy's `profile()`.

**Special-case to *select*, size from *observed* intel (operator 2026-06-26).** A doctrine's `applies` may key on owner type (Invader / SourceKeeper) to *select* the coordination class + archetype — that is cheap and unambiguous. But `plan()` must *size* from the **observed force** in `ctx` (creep bodies/parts → dps/heal/hits; structures → breach/objective hits), never from type-keyed magic numbers — so the same doctrine is robust to boosted / modded / variant enemies and shares **one** sizing path with the player doctrines. `worst_single` and `defense` are therefore *derived from live intel*, not looked up by type. (The just-landed `SK_KEEPER_HP` / `SK_KEEPER_MELEE_DPS` constants are an acceptable shortcut *only* because NPC bodies are engine-fixed; **rung 1 derives them from the observed keeper body** so no sizing is type-pinned and the SK path is the same code as a player kiter duel.)

**Composition is *computed*, and an N-blob is first-class (operator 2026-06-26).** `ForcePlan.composition` is not a fixed registry pick — the registry templates (`quad_ranged`, `duo_sk_farmer`, …) are *seeds*; `sized_for` already grows the member **count** when one creep can't hold the required parts, and that growth **is** a blob. So the output is a *blob of N sized creeps* whenever the force demands it — a quad is just the N = 4 **efficient-formation** case (the 2×2 that paths and holds as one unit), **not a cap**. N is dynamic on **both** sides of the fight:
- we **spawn** an N-blob when sizing calls for it (the `SquadManager` + the agent formation must support arbitrary N, not just the quad layout — a build requirement, not just a sizing one);
- we **size against** an enemy N-blob — the Coordinated square-law (§9.4) scales with *their* N, read from the observed creep set.

R8's role auction is the limit form — *compute* the best (role-mix × N) by marginal EV — with the templates as its warm start (the §12.7 R5.5 → R8 ladder). The doctrine layer is the heuristic precursor; the blob-of-N is the shape that makes "the best composition" expressible at both rungs.

### 9.4 The coordination-driven sizing math (what `assess` branches on)

The oracle gains a coordination branch — the SAME inputs, two aggregation rules:

| | **Individual** (NPC) | **Coordinated** (player) |
|---|---|---|
| DPS to out-heal | `worst_single.dps` (one at a time) | `Σ enemy_dps` (all at once) |
| Their HP to grind | serial → kill-time `Σ hits / our_dps`, heal need bounded by the single | concentrated under their focus-fire |
| Win condition | beat the strongest single + survive serial attrition | square law: our combat power must **exceed** theirs by a `√margin` factor (ratio counts quadratically), not just match |
| Typical output | the *minimum favorable* force (cheap: SK duo, core quad) | the *over-matching* force (quad → blob with margin) |

So three SK keepers size a **duo** (beat one 168-dps / 5000-hp keeper — R6 + R-attack, already built), where a naïve `Σ` would size a needless trio+. A player's 4-creep focus-fire squad sizes a **quad/blob with square-law margin**, where `worst_single` would fatally under-size. **That divergence is the whole reason the axis exists.** `DefenseProfile` already carries the aggregate; the only new data are `EnemyCoordination` + (for Individual) `worst_single` — both cheap bot-side from `RoomThreatData.hostile_creeps` / the keeper body / the core.

**Built (2026-06-26): the creep-clear sizing primitive `force_sizing::clear_force` (the keystone for rungs 2–3).** A creep-clear is NOT a structure breach: where `assess` sizes a structure's kill-DPS to the squad's *gross* (so rampart repair can't stall it), `clear_force` sizes to the **enemy** — kill-DPS = enough to grind their HP net of their heal within the on-site window **and** to out-power them by a `dps_margin`, plus heal to out-heal the incoming. The coordination axis is the caller's: **Individual** passes the worst single + `dps_margin = 1.0` (beat that one); **Coordinated** passes the aggregate + `dps_margin = COORDINATED_DPS_MARGIN` (= 1.5 seed — the square-law over-match, §9.8-tunable). The margin scales the KILL parts only (heal is sized to the incoming either way). `(ForceAssessment, RequiredForce)` out; unwinnable ⇒ all-zero. **Pure + host-tested (4 tests), NOT yet wired** — the `PlayerDefend`/`PlayerRaid` doctrines that call it are the §9.10 ledger's next rungs.

### 9.5 The starter doctrine set (the named rules)

Collection order = priority; first activator wins (the §8 registry shape):

| Doctrine | `applies` (classifier) | Coordination | ForcePlan | Status |
|---|---|---|---|---|
| `SafeModeSkip` | `defense.safe_mode` | — | not winnable → skip (hard veto; mirrors `SafeModeHold`) | design |
| `SkSuppression` | `Farm{SourceKeeper}` | Individual | sized `duo_sk_farmer` (heal out-heals one keeper; ranged kills it) | ✅ built (R6 + R-attack) |
| `NpcCore` | `InvaderCore{level}` | Individual | oracle-sized `quad_ranged` (ranged ceiling kills the dismantle-immune core) | ✅ built (R-attack) |
| `InvaderCreeps` | `InvaderCreeps` | Individual | sized duo/solo vs the worst single wave creep | partial (templated) |
| `PowerBankFarm` | `PowerBank` | Individual (bank is inert) | ROI-gated duo + hauler(s) | existing mission |
| `ResourceDenial` | `ResourceDenial` | — | opportunistic `solo_harasser`, LOW priority, no gate (throwaway) | ✅ built (hardcoded) |
| `PlayerRaid` | `AttackFlag` / `Expansion` vs an owned base | **Coordinated** | quad → blob, oracle-sized to the aggregate with square-law margin; objective = `Secure` (clear creeps) or `Dismantle` (raze) by what's present | **NEW — value** |
| `PlayerDefend` | `Defend` / `ThreatResponse` | **Coordinated** | sized defender squad — **subsumes `DefenseEscalation::from_threat`** | design (replaces from_threat) |

The two ✅ rows are the current sized arms re-expressed as doctrines (so they land first as a **no-op refactor**); `ResourceDenial` is the current hardcoded arm. `PlayerRaid` / `PlayerDefend` are the new value — and the reason the coordination axis is needed. **`PlayerRaid` requires the §12.7(B) creep-target oracle path** (an `enemy_creep_hits` field + a `clear_creeps` Lanchester branch) that the AttackFlag/Harass re-adjudication (ADR 0020 §12.6, 2026-06-26) deferred to R8 — this section is its design home, and the deferral's stated reason (the oracle is structure-shaped and `candidate.defense` is `None` for those arms) is exactly what `EngagementContext` + the Coordinated branch fix.

### 9.6 The seam

`war.rs`'s offense `match` (and the SK / defense producers) become: project the candidate's intel into an `EngagementContext`, call `decide_doctrine`, field the `ForcePlan`. One adapter per producer (bot enums stay out of the pure crate, exactly like `classify` in §3.4):

```rust
// screeps-ibex/src/military/doctrine.rs (new) — project a candidate into the pure context
pub fn engagement_context(c: &AttackCandidate, threat: &RoomThreatData, home_energy: u32) -> EngagementContext { … }
```

`decide_doctrine` replaces the hardcoded `(objective, priority, composition)` tuple the offense loop returns today; `DefenseEscalation::from_threat` is replaced by the `PlayerDefend` doctrine. Adding / retiring a doctrine = one collection entry — no `war.rs` surgery, the §2 win the operator asked to extend.

### 9.7 Files / rungs / gating

| File | Change |
|---|---|
| `screeps-combat-decision/src/doctrine.rs` | **NEW.** `EnemyCoordination`, `EngagementContext`, `UnitThreat`, `ForceDoctrine`, `ForcePlan`, `decide_doctrine`, the starter doctrines. Pure, unit-tested. |
| `screeps-combat-decision/src/force_sizing.rs` | `assess` gains the Individual/Coordinated branch (§9.4); `DefenseProfile` (or `EngagementContext`) carries `worst_single`. For `PlayerRaid`: the §12.7(B) `enemy_creep_hits` + `clear_creeps` branch (R8). |
| `screeps-combat-decision/src/lib.rs` | `pub mod doctrine;`. |
| `screeps-ibex/src/military/doctrine.rs` | **NEW.** `engagement_context()` adapter (projects `AttackCandidate` + `RoomThreatData` + home energy; derives `EnemyCoordination` from owner/body signals). |
| `screeps-ibex/src/operations/war.rs` | Offense `match` → `decide_doctrine` (✅ done, L1); defense sites → `defense_doctrines()`/`GarrisonDefense` + `DefenseEscalation::from_threat` **deleted** (✅ done, L3a). |
| `screeps-combat-eval/src/tournament.rs` | Per-doctrine beds (an Individual NPC bed + a Coordinated player-squad bed) + the `doctrines_are_each_best_in_class` gate. |

**Rungs** (map onto ADR 0020 §12.7 R5.5 → R8):
1. **Refactor-to-registry (no-op).** Re-express `SafeModeSkip` + `NpcCore` + `SkSuppression` + `ResourceDenial` as doctrines; swap the offense `match` for `decide_doctrine`. Behavior byte-identical (the built sizing is unchanged); the win is the seam. Kill-switch `features.military.doctrine_selection` (default true), `default()`-equivalent off. **No WFV.**
2. **`PlayerDefend`.** Replace `from_threat`'s 3-bucket escalation with a Coordinated-sized defender; gate on a Coordinated defense bed.
3. **`PlayerRaid` (R8).** Build the §12.7(B) creep-target oracle path, then the doctrine; gate on a Coordinated raid bed. This is the deferred AttackFlag/Harass work, now with a home. **Prerequisite — N-blob spawning + formation:** the `SquadManager` spawn path and the agent formation/movement must field an **arbitrary-N** blob (not just the quad 2×2 layout), since `sized_for` can grow past 4 and the square-law raid wants it. Quad stays the efficient-formation special case; the blob is the general one.

**Serialization:** none — `ForcePlan` is a per-target decision, recomputed, never stored (like §6). **No `WORLD_FORMAT_VERSION` bump** at any rung. **Deploy gating:** ADR 0020 §10 Docker-soak → operator go-ahead; never MMO without it.

### 9.8 Tuning integration — the dynamic weights

Doctrine **selection** stays discrete (the §3.3/§5 categorical decision stands — `applies` is a classifier, not a continuous/learned policy). But the **weights inside each doctrine's sizing are continuous, and the tournament tunes them** — exactly the §4 pattern (discrete named profiles, tuned `KernelParams` *within*). So "dynamic" here = tuned boundaries + margins, **not** a learned end-to-end policy; §5's rejection of a continuous *selection* policy is untouched. This is the operator's point (2026-06-26): wherever squad selection needs a continuous knob, the harness should *discover* its value, not have it hand-set.

**The tuning surface** — a `DoctrineParams` constant set (the twin of `KernelParams` / `SquadTacticParams`), pure + host-shared so the bot and the tournament read identically:

| Weight | Drives | Replaces (hand-set today) |
|---|---|---|
| `coordination_dps_threshold` | the Individual ↔ Coordinated boundary | Q1's hand default (the safety prior becomes the *floor*, not the value) |
| `coordinated_margin` (square-law over-match) | Coordinated force size | a slice of the single `HOLD_MARGIN` |
| `individual_margin` (out-heal / out-last the single) | Individual force size | the other slice of `HOLD_MARGIN` |
| `blob_escalation_parts` | quad → blob escalation for a Coordinated raid | Q2's hand cap |
| `investment_scale` (importance · P(win) curve) | force vs objective priority | R5's fixed scale |
| `defend_size_curve` | `PlayerDefend` sizing | `DefenseEscalation::from_threat`'s three hardcoded thresholds (`war.rs:101`) |

**The harness** (mirror §4 — build no new mechanism). Two beds, each **near the winnability boundary** — §8's lesson that trivially-winnable beds don't discriminate, so they can't tune:
- an **Individual NPC bed** (cores / keepers / invader waves at graded strength) — confirms the cheap min-favorable sizing holds (no over-spend);
- a **Coordinated player-squad bed** (player comps at graded strength + composition) — the bed that actually exercises the square-law margin + the blob escalation.

The tournament sweeps `DoctrineParams` over each bed and adopts the payoff-maximizing set (won objectives − creeps lost − energy spent — the EV currency, ADR 0020-S5), the same per-regime adoption as §4 / §8. The **bit-deterministic sim (2026-06-26)** makes these margins cleanly tunable — the same enablement that unblocked the base-attack re-tune (§8) — so the boundaries are *discovered*, not ideated (the "tournament-discovery beat ideation" finding, [[sim-determinism-fence]]). **Gate:** `doctrines_are_each_best_in_class` **plus** the tuned weights beat their hand-set priors on both beds. A doctrine that mis-classifies coordination or under-sizes *loses self-play*, so the gate is self-policing.

A concrete near-term win: `PlayerDefend`'s `defend_size_curve` replaces `from_threat`'s three magic thresholds (`200`/`150`/`60` dps etc.) with a curve the Coordinated bed tunes — the first hand-set combat constants this layer retires.

### 9.9 Open questions — all resolved (design locked 2026-06-26)

- **Q1 — RESOLVED ✅ (operator 2026-06-26): coordinated unless a positive NPC signal.** Mis-classification is asymmetric: calling a player "Individual" *under-sizes and loses creeps*; calling an NPC "Coordinated" merely *over-spends*. ⇒ the prior is **`Coordinated` unless a positive NPC signal** (owner ∈ {Invader, SourceKeeper, unowned}), and `coordination_dps_threshold` (§9.8) is swept *from* that prior and must beat it. The classifier defaults to the safe (over-spend) side and only asserts `Individual` on a definite NPC owner.
- **Q2 — RESOLVED → tunable.** Blob vs quad for a Coordinated raid is `blob_escalation_parts`, swept on the player-squad bed — not a hand decision.
- **Q3 — RESOLVED → independent objectives, guarded against bleed.** A player base *with* an NPC core is separate candidates → separate doctrines/objectives (the registry stays per-candidate; no blended coordination value). The operator's one condition (2026-06-26): independence must not *bleed energy when a call is wrong*. That guard is **existing mechanism, not new** — each objective is held by the **winnability gate** (`plan().winnable == false` → skip, the InvaderCore-arm pattern) **+** the `ObjectiveKind` **give-up backoff** (`objective_queue` proximity/backoff), so a mis-sized or mis-classified engagement *backs off and stops re-spawning* rather than feeding creeps into a continued loss. A wrong independent call costs at most one bounded, backed-off attempt — acceptable per the condition.

### 9.10 Deferred-work ledger (the path from rung 1 → the full registry)

Rung 1 (the registry + the current arms re-expressed) and the creep-clear sizing primitive (`clear_force`) are **built**. Everything else is tracked here so nothing is lost — status, what it depends on, and the open call.

| # | Item | Status | Depends on | Notes / open call |
|---|---|---|---|---|
| L1 | Doctrine registry + rung-1 arms (`NpcCore`/`SiegeBreach`/`SecureRoom`/`HarassRemote`) | ✅ **built** (decision `8efa32e`, bot `da0756d`, eval `c574bc3`) | — | no-op, parity-verified |
| L2 | `clear_force` — creep-clear sizing primitive (Individual + Coordinated square-law) | ✅ **built + host-tested** | — | unwired; `COORDINATED_DPS_MARGIN` = 1.5 seed |
| L3a | **`GarrisonDefense`** doctrine — UNIFY defender selection onto the registry; delete `DefenseEscalation` | ✅ **built** (decision, bot) | — | both defense sites (owned-room + remote-invader) route through `defense_doctrines()` → `GarrisonDefense`; the 3-bucket `DefenseEscalation` enum + `from_threat` are deleted. Behavior-preserving selection (the former thresholds, kept; remote site harmonized to them); spawn-path sizing unchanged → no defensive regression. The shape thresholds are the §9.8 `defend_size_curve` (L6-tunable) |
| L3b | clear_force-based **threat-proportional defender sizing** (replace spawn-path max-sizing) | deferred | L2 + L6 | wire `GarrisonDefense` to `clear_force` so a defender is sized to the THREAT, not the room's max energy. Behavior change (energy efficiency) → wants the Coordinated-defense bed (L6) first |
| L4 | **Rung 3 — `PlayerRaid`** doctrine; make the offense `AttackFlag`/`Expansion` arm sized | deferred | L2 + L5 + bot wiring | §9.5 **size-but-always-field** (never gate-skip operator intent). Two bot subtleties to settle: making `AttackFlag` sized adds a **reachability** gate (it currently fields unconditionally) and the **ROI** gate — decide whether operator intent overrides them. Quad-capped until L5 |
| L5 | **N-blob spawning + formation** (arbitrary N, not just the quad 2×2) | deferred | `SquadManager` spawn path + agent formation/movement | `sized_for` already grows member COUNT past 4; the spawn + formation/movement layers must field + drive an N-blob. Rung-3 prereq for the blob escalation |
| L6a | **Creep-clear validation bed + gate** (`CreepClearBed` + `CreepClearWins`) | ✅ **built** (eval) | L2 | an open-room Secure bed with a graded GROUPED defender force; sizes the attacker via `clear_force`, fields the real moving brain, scores `SideWiped(defender)` = cleared. `creep_clear_sizing_clears_the_bed` (#[ignore] dashboard): **100% (4/4)**. **Finding:** grouped forces fight as `Coordinated` — at margin 1.0 a lean ranged squad *can't close on open-field kiters* (stall/timeout); the square-law over-match (`COORDINATED_DPS_MARGIN`) is what lets it close + clear. So **grouping is a Coordinated signal**, not just mutual-heal (refines §9.4). |
| L6b | **`COORDINATED_DPS_MARGIN` sweep** (`creep_clear_margin_sweep`) | ✅ **built + run; seed validated** | L6a | swept 1.0–2.0 on the bed (payoff = winning dominates, then leanest cost): the **4/4 win plateau starts at 1.4** (≤1.3 → 3/4, the ranged-weak stall), and ≥1.75 only adds cost. **Kept the 1.50 seed** = the 1.4 cliff + a ~7% safety buffer (the HOLD_MARGIN "hold through variance" philosophy; adopting the exact 4-scenario cliff would overfit). The sweep is the reusable instrument; a richer/terrain bed would refine it. |
| L6c | tune the **other `DoctrineParams` weights** (`coordination_dps_threshold`, `blob_escalation_parts`, `defend_size_curve`) | deferred | their consumers (L3b/L4/L5) + a true-Individual (separate-enemy) bed | each weight is tuned with the rung that introduces it (no consumer yet → nothing to sweep) |
| L7 | **SK keeper-suppression as a `SkSuppression` doctrine** | ✅ **built** (decision, bot) | — | `sourcekeeperfarm.rs`'s inline `RequiredForce` sizing folded onto the registry: the SK mission builds the keeper as an `EnemyForce` + calls `decide_doctrine(sk_doctrines())` → `SkSuppression` (out-heal the keeper's melee × HOLD_MARGIN + size the kiter's RANGED to kill its HP in the kill window). Behavior-identical (15 ranged / R6 heal). NOT `clear_force` — the SK kites + out-heals (no square-law over-power). `DoctrineObjective::Suppress` added. |
| L8 | **Coordination from observed bodies** (not the candidate's `source`) | deferred | — | `classify_coordination` keys on `TargetSource` today; the §9.3 principle wants it derived from observed owner/body signals. Q1 prior (Coordinated unless a positive NPC signal) holds |
| L9 | `managed_assault_comp` (eval traversal lens) | **not a gap** | — | a different concern (the squad-brain traversal lens fields a drivable `quad_ranged`), not selection/sizing — intentionally separate |

**Done so far (unification + debt):** L1 (registry + parity), L2 (`clear_force`), **L3a** (defender selection unified; `DefenseEscalation` deleted), **L6a** (creep-clear bed + gate — `clear_force` clears 4/4), **L6b** (margin sweep — `COORDINATED_DPS_MARGIN` = 1.5 validated), **L7** (SK suppression folded onto the registry — `SkSuppression`). **All three combat producers — war offense, war defense, and the SK farm — now select + size on the doctrine registry; the only parallel-system debt left is the fixed AttackFlag/Harass arms (their sizing is L4) and the PowerBank mission.** **Next:** **L8** (coordination from observed bodies — best done WITH L3b/L4, which add the consumer + the richer intel), then the behavior-changing rungs that wire `clear_force` live — **L3b** (defender sizing), **L4** (`PlayerRaid`, also needs **L5** N-blob) — each bed-validated before live, deploy-gated. **L6c** tunes the remaining `DoctrineParams` weights alongside their rungs.
