# ADR 0026 — Objective/Information-Dependent Combat Strategy-Selection Layer

- **Status:** Decided
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

**The realistic re-tune (ADR 0025 §12 Stage 4) proves no single global `KernelParams` wins everything.** The host tournament — foreman-planned bases over real imported terrain, plus open-combat self-play — produced two findings that point in opposite directions (ADR 0025 §12):

- **Open combat:** robustness (low exploitability against the whole field) is the binding criterion, and the configs that top mean payoff tend to *regress* base attack — so the adoption protocol's no-base-attack-regression rule keeps open combat on the robust profile rather than the field leader.
- **Base attack (real foreman + imported `Raze` bases):** strongly position-**sensitive**, and the config that wins it is *not* the config that wins open combat — a default tuned for open fighting can chip at a rampart ring and bleed creeps instead of breaching, while a config that cracks the ring is the worst one in open combat. (The first small basket read that split as "approach_coef 4 dominates"; at grid scale the discriminating levers turned out to be incumbency and cohesion with approach LOW — ADR 0025 §12. The *divergence* replicated; only its lever changed.)

The ADR 0025 §12 verdict:

> **Adoption:** no single `KernelParams` wins both lenses → the principled fix is objective-awareness (a distinct weight profile when the objective is a STRUCTURE/base, the robust default for open-creep combat) … not a global bump that would forfeit open-combat robustness.

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
| **target_is_structure** | `bool` — `matches!(kind, Dismantle{..})`, or "the room has hostile structures and no killable hostile creeps" from `view.structures` | `ObjectiveKind` (`:286`); `view.structures` (`squad_manager.rs:619`, built by `build_room_combat_dtos` `:606`) | THE re-tune lever: a structure/base objective selects the breach profile; a creep objective the open profile. |
| **enemy_safe_mode** | `bool` | already computed at `squad_manager.rs:610-614` and on `view.enemy_safe_mode` | Safe mode ⇒ no damage possible ⇒ never spend approach risk; force the conservative profile (the `assess` hard veto, `force_sizing.rs:134`). |
| **tower_pressure** | `bool`/small enum from energized hostile towers | `RoomThreatData.hostile_tower_positions` + `.tower_energy` (`military/threatmap.rs:76,99`), already on the room entity | A towered base needs the breach profile (the re-tune's foreman bases all have tower rings); an open skirmish does not. |
| **winnability mode** | `Option<AssaultMode>` (`Breach`/`Drain`) | `force_sizing::assess` (`force_sizing.rs:124`) output — already produced by war.rs at field time (`war.rs:960`) | The force-sizing oracle ALREADY classifies the assault: `Breach` ⇒ approach-hot + dismantle-through; `Drain` ⇒ tank-soaks-then-breaches (patience/cohesion). Free, exact signal. |
| **threat_level** | `ThreatLevel` (`threatmap.rs:42`) | `RoomThreatData.threat_level` | `Defend` against a `PlayerSiege` may want a different posture than against a lone `Invader`; v1 uses it only as a coarse gate, reserved for tuning. |

The signals deliberately **excluded from v1** (gaps noted, deferred as tuning surface, not v1 machinery): per-creep weight variation (the kernel takes one `SquadTacticParams` for the whole squad — `lib.rs:1456`), multi-room context (intel is single-room — `threatmap.rs` is per-room), RCL-graduated profiles, intel-confidence/staleness weighting, and cross-squad coordination. These are §11-style follow-ons; v1 ships the smallest set the re-tune proved decisive.

### 3.2 Output

A `SquadTacticParams` (`kite.rs:241-262`) — the **existing** container, unchanged. It flows down the **existing** seam: `strategy_for(kind, info)` returns it, `compute_squad_orders` passes it to `decide_squad_with_pathing(&view, …, profile, …)` (`squad_manager.rs:653/657`), which routes `&profile.kernel` into `plan_squad_ev` (`lib.rs:1658`). **No new output type, no new plumbing** — the layer substitutes a value at a call site that already takes that exact type. The kite/engage/healer `KiteScoreParams` fields ride along unchanged in v1 (only `kernel` varies); leaving them as tuning surface for later objectives (e.g. a future `Harass` profile that reweights the kite preset).

### 3.3 The selection mechanism — a pure table/rules function

**A small, explicit rule table over `(kind, info)` → named profile, NOT a learned/continuous policy.** Rationale:

1. **The re-tune already produced discrete winners per regime** (ADR 0025 §12): `k-default` for open combat, `k-approach-hot` for base breach. The decision surface the data supports is *categorical* (open-creep vs structure-breach vs safe-mode-veto), not a smooth function — a lookup table is the faithful encoding of the evidence we have.
2. **Determinism + parity** (ADR 0020 §6, ADR 0025 §7): the kernel is integer-only and deterministic; a table-lookup selector is trivially deterministic and wasm-safe (no floats in the *selection*, no `game::*` calls — it lives in the pure decision crate). A learned/continuous policy adds an inference path, float weights, and a model artifact to serialize/version — all debt this layer is explicitly trying to avoid.
3. **Tournament-tunable per profile** (§4): each named profile is one `KernelParams` constant set the harness tunes independently. A table of named profiles maps 1:1 onto the tournament's existing `Strategy` population (`tournament.rs:46-49`) — the harness already constructs and ranks named profiles; the table is just "which named profile per objective".
4. **Least debt, fits the FSM**: it is a `match` returning a `const`-derived struct. No state, no allocation, O(1) per squad per tick (the CPU constraint at `squad_manager.rs`'s linear loop), no serialization.

The continuous/learned alternative is evaluated and rejected in §5.

**The table is realized as a pluggable activator registry.** Rather than one `match`, each rule is a `CombatStrategy` — a named **activator** (`applies(ctx)`) plus the **profile** it fights with — and `decide_strategy(ctx, &collection)` returns the first strategy in the collection whose activator fires (collection order = priority). Semantically identical to the `match` below (pure, deterministic, O(1), no state), but a strategy is added or retired by editing one collection entry instead of surgery on a growing `match`, and the collection is exactly the population the tournament ranks. The standard collection is `SafeModeHold` (the veto) → `DrainBreach` → `Breach` → `OpenCombat`. §9 applies the same shape one layer up.

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
        return SquadTacticParams::open_combat(); // robust/conservative; the engage gate retreats anyway
    }
    match class {
        CombatObjectiveClass::StructureBreach => match info.assault_mode {
            // Drain: tank soaks the towers dry, THEN breach — hold longer through the soak.
            Some(AssaultMode::Drain) => SquadTacticParams::breach_drain(),
            // Breach (or unknown mode but a structure objective): move in and dismantle.
            _ => SquadTacticParams::breach(),
        },
        // Open-creep combat: the robust, low-exploitability profile.
        CombatObjectiveClass::OpenCombat => SquadTacticParams::open_combat(),
    }
}
```

**Profile constructors on `SquadTacticParams` (`kite.rs`, beside `default()`)** — one per named profile, each a `KernelParams` constant set the tournament tunes independently (§4, adopted values in §8): `open_combat()`, `breach()`, `breach_drain()`. `KernelParams::default()` stays as the neutral seed and is not the adoption vehicle.

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
| `screeps-combat-decision/src/kite.rs` (`:253`) | Add `breach()` / `breach_drain()` constructors beside `default()`. |
| `screeps-combat-decision/src/lib.rs` | `pub mod strategy;` re-export. |
| `screeps-ibex/src/military/strategy.rs` | **NEW.** `classify()` + `build_strategy_info()` (assemble `StrategyInfo` from `RoomThreatData` + `enemy_safe_mode` + the `AssaultMode` carried on the objective — see §6). |
| `screeps-ibex/src/military/squad_manager.rs` (`:650-659`) | Replace `SquadTacticParams::default()` with `strategy_for(classify(kind, …), info)`. Thread `kind` (already at `:286`) + `info` into `compute_squad_orders`. |
| `screeps-ibex/src/military/squad_manager.rs` (`:157-167`) | Add `threat_data: ReadStorage<'a, RoomThreatData>` to `SquadManagerSystemData` so the adapter can read `tower_pressure`/`threat_level` for the target room (the component already lives on room entities). |
| `screeps-combat-eval/src/tournament.rs` | Per-objective profile harness (§4). |

---

## 4. Tuning integration

The realistic harness already proves the open ↔ base-attack divergence (ADR 0025 §12). This ADR ties each **named profile** to its **own per-objective tournament**, so the harness tunes per-objective profiles, not one global config.

**The harness seam already supports this.** `tournament.rs` constructs named strategies (`Strategy { name, tactics }`, `:46-49`), injects them via `ManagedSimSquad::with_tactics` (`screeps-combat-agent/src/squad.rs:268`), and ranks them by mean payoff + exploitability + meta-Nash (`run_tournament_over_comps`, `:261`). The two lenses already exist:

- **Open-combat profile** (`OpenCombat` → `open_combat()`): tuned and validated by the `realistic_comp_basket` (`tournament.rs:221`) open-combat tournament. The gate is **robustness**: `exploitability ≤ GROSS`.
- **Breach profiles** (`StructureBreach` → `breach()` / `breach_drain()`): tuned by `realistic_base_scenarios` (`tournament.rs:237`) — the foreman + imported `Raze`/`Breach` bases — scored by `assault_score` (HP razed + destroyed bonus + attacker survival, `harness/validate.rs`). The gate is **per-objective best**: the profile that maximizes `assault_score` over the realistic base set.

**New harness fns (extend `tournament.rs`, build no new mechanism):**

```rust
/// Tune the StructureBreach profile: rank a KernelParams population over `realistic_base_scenarios`
/// by `assault_score`. Returns the best (the per-objective adoption candidate). Mirrors the existing
/// `base_attack_ranking`, but its OUTPUT is "the profile to bake into breach()", not a dashboard row.
pub fn tune_breach_profile(pop: &[Strategy]) -> (&'static str, KernelParams, /*score*/ i64) { … }

/// Per-objective robustness check: a breach profile MUST NOT be wildly exploitable in open combat
/// (a squad that mistakenly fields it in a creep fight should not get hard-countered). Run the chosen
/// breach profile through the open-combat exploitability gate; record its open-combat exploitability
/// alongside its base-attack score. (It need not WIN open combat — it won't be selected there — only
/// not be a free kill, since a mid-fight reclassification can switch a live squad into it.)
pub fn validate_breach_profile_open_robustness(breach: SquadTacticParams, pop: &[Strategy]) -> i64 { … }
```

**Per-objective validation contract (the adoption protocol, per profile):**

1. **OpenCombat:** lowest-exploitability config over `realistic_comp_basket`; re-run `exploitability ≤ GROSS`.
2. **StructureBreach/Breach:** highest `assault_score` over `realistic_base_scenarios` (`tune_breach_profile`); **plus** a bounded-exploitability check in open combat (`validate_breach_profile_open_robustness`) so a reclassification mid-fight is not a free kill.
3. **StructureBreach/Drain:** the drain profile is seeded from the breach profile until the harness carries a tower-energy-bounded drain scenario; then it is tuned against `assault_score` on the `Drain`-mode bases.
4. **Adoption** (per ADR 0025 §12 step 5): record each adopted profile's constants + its per-objective ranking + its cross-objective robustness in §8. A `KernelParams` change is a decision-crate constant — **no `WORLD_FORMAT_VERSION` bump** (ADR 0025 §6).

A CI gate `per_objective_profiles_are_each_best_in_class` asserts: the breach profile beats the open profile on `realistic_base_scenarios`, and the open profile beats the breach profile on `realistic_comp_basket`. This is the regression fence — it would catch "we globally bumped `approach_coef` and forfeited open-combat robustness", the exact failure ADR 0025 §12 warns against.

---

## 5. Alternatives considered

**(a) Keep a single global `KernelParams` (status quo).** *Rejected.* The re-tune is decisive: no single config wins both lenses. `k-default` cannot breach real foreman rampart rings (chips and bleeds, ~−25k), and `k-approach-hot` is the worst open-combat config (−118 mean, and exploitable) (ADR 0025 §12). A global bump trades one failure for another. This is precisely what motivates the ADR.

**(b) Bake objective-awareness INTO the kernel's EV math directly** (e.g. make `approach_coef` a function of "is the focus a structure?" inside `plan_squad_ev`). *Rejected as the mechanism, with one nuance.* Trade-offs:
   - *Against:* it couples the kernel's pure per-(tile×action) math to objective semantics the kernel deliberately does not know — ADR 0025's whole thesis is "no role archetype, no objective branching in the kernel; formation emerges from one currency" (ADR 0025 §1). Threading objective kind into the kernel re-introduces exactly the strategic conditioning ADR 0025 pushed *out*. It also makes the tuning surface harder to reason about (the coefficient is now data-dependent inside the hot loop) and the tournament can no longer A/B named profiles cleanly (`tournament.rs:46`).
   - *Nuance kept:* the kernel *already* prices structures vs creeps (`V_struct`, breach-inherited value, ADR 0025 §2.4) — so "the kernel knows it's hitting a structure" is true at the *value* level. What it must NOT do is condition its *position-shaping coefficients* on that. The clean separation: **the kernel prices outcomes; the strategic layer picks the position-shaping weights.** Keeping `approach_coef` selection in a layer above `plan_squad_ev` preserves the kernel as a pure, tournament-comparable function of its `KernelParams`.

**(c) A learned/continuous policy** (map a feature vector → continuous `KernelParams` via a small learned model). *Rejected for v1.* Trade-offs:
   - *Against:* adds a model artifact (weights to serialize + version → a `WORLD_FORMAT_VERSION` concern this design otherwise avoids), a float inference path on a deterministic integer hot path (parity risk, ADR 0020 §6 / ADR 0025 §7), and training/eval infra — heavy machinery for a decision surface the data shows is essentially categorical (two regimes). It also undermines tournament comparability (a continuous policy is not a discrete `Strategy` the population gate ranks).
   - *When it'd be right:* once the input set grows (RCL graduation, multi-room, intel confidence, enemy composition) the categorical table may get unwieldy and a learned selector over `realistic_*` scenarios becomes attractive. The table is the v1 floor; the meta-Nash mixed strategy the tournament already produces (`tournament.rs:277`, the §11-#8 adaptivity bridge) is the natural upgrade path. Deferred, not foreclosed.

**Chosen: (the table, §3.3).** Faithful to the evidence (categorical regimes), zero serialization, deterministic/wasm-safe, O(1), and 1:1 with the existing tournament `Strategy` population. Least debt, fits the FSM, tournament-tunable per objective.

---

## 6. Consequences

**Technical debt.** Minimal and bounded. One pure function + two profile constructors + one small bot adapter + one new `SystemData` field. No new FSM states, no new lifecycle, no new persistent component. The selector is a `match`; adding an objective profile later is one arm + one tuned constant set. The main *latent* debt is the `StrategyInfo` plumbing (getting `AssaultMode`/`tower_pressure` to the seam) — see below.

**Serialization / `WORLD_FORMAT_VERSION`.** **No bump.** The selected `SquadTacticParams` is per-tick, recomputed in Phase B2 each tick, never stored (consistent with ADR 0025 §6: "no `WORLD_FORMAT_VERSION` bump — pure per-tick decision"). One sub-decision on where `AssaultMode` comes from:
   - *v1 (no serialization):* re-derive `tower_pressure` live from `RoomThreatData` (already serialized, already on the room entity) at the seam, and treat `assault_mode` as `None` (the `StructureBreach` arm falls back to `breach()` when mode is unknown — the correct default for a towered base). This needs **zero new serialized fields**.
   - *Optional follow-on (one serialized field, justified separately):* if telemetry shows the `Breach`/`Drain` distinction materially changes outcomes, carry the producer's `AssaultMode` on `CombatObjective` (`objective_queue.rs:147` — already `Serialize`) so the squad uses the *producer's* oracle verdict rather than re-deriving. `CombatObjectiveData` is `#[serde(default)]` (`objective_queue.rs:182`), so adding an `Option<AssaultMode>` is forward-compatible — **but bincode is positional, so it would still gate a `WORLD_FORMAT_VERSION` bump** (cf. the `tower_energy` 14→15 note, `threatmap.rs:96`). v1 deliberately avoids this; the field is added only if the drain distinction earns it.

**CPU.** O(1) per squad per tick — a `match` + a few `bool` reads. Negligible against the per-squad target-flood (`TARGET_FLOOD_OPS = 2500`, ADR 0025 §6). `RoomThreatData` is read once per target room (it is already build-once-per-room-shared alongside `PositionLayers`, `squad_manager.rs:284`).

**Testability.** The selector is a pure function — host-unit-tested with no ECS (the same pattern as `is_formation_objective`/`objective_target`, which have unit tests at `squad_manager.rs:784-828`). Tests assert: `StructureBreach + Breach → breach()`, `StructureBreach + safe_mode → open_combat()`, `OpenCombat → open_combat()`, and the per-objective tournament gate (§4). The decision crate already host-tests `KernelParams` variations via the tournament.

**Migration / rollout.**
   - **The open profile is the conservative one.** `OpenCombat` and the safe-mode veto both select `open_combat()`, so every objective that is *not* a structure breach keeps the robust profile. The behavioral change is confined to `Dismantle`/breach objectives switching to `breach()` — the exact case the re-tune shows a single global profile *loses*. So the change is a fix to a known-failing case, with no regression surface on the working cases.
   - **No kill-switch.** The layer ships as the final state rather than behind a `MilitaryFeatures` flag: the flag's "off" path is just "select the open profile everywhere", which is the very failure this ADR exists to remove, and a dark second selection path would drift from the tournament-tuned one. The regression fence is the per-objective tournament gate (§4), not a runtime toggle.
   - **Deploy gating.** Same as any combat change: ADR 0020 §10 Docker-soak → operator go-ahead; never deploy MMO without explicit go-ahead.

---

## 7. Implementation plan

Ordered, minimal-debt increments. Each leaves the workspace compiling with the relevant tests green so the harness stays a usable gate.

**Step 1 — Pure selector + profiles (decision crate).** `screeps-combat-decision/src/strategy.rs` (`CombatObjectiveClass`, `StrategyInfo`, the selection rule of §3.3/§3.4) and the `SquadTacticParams` profile constructors (`kite.rs`). Unit tests: each arm returns the expected named profile; safe-mode forces the open profile. *No bot change yet — pure, isolated, host-green.*

**Step 2 — Per-objective tournament gate (harness).** `tune_breach_profile` + `validate_breach_profile_open_robustness` in `screeps-combat-eval/src/tournament.rs` and the CI test `per_objective_profiles_are_each_best_in_class` (§4): the breach profile beats the open profile on `realistic_base_scenarios`, and the open profile beats the breach profile on `realistic_comp_basket` — the ADR 0025 §12 divergence re-confirmed with the *named* profiles. *This is the regression fence; it goes green before the bot wires the layer in.*

**Step 3 — Bot adapter + seam swap.** `screeps-ibex/src/military/strategy.rs` (`classify`, `build_strategy_info`); `threat_data: ReadStorage<RoomThreatData>` on `SquadManagerSystemData` (`squad_manager.rs:157`); thread `kind` + `StrategyInfo` into `compute_squad_orders` and replace the two `SquadTacticParams::default()` literals with the selector. Tests: `classify(Dismantle) == StructureBreach`, `classify(Defend) == OpenCombat`, and the existing `squad_manager` unit tests stay green.

**Step 4 — Tune + adopt the breach profile.** Run the realistic re-tune machinery (ADR 0025 §12 Stage 4) to refine the `breach()` constants; bake the adopted `KernelParams` in; record the per-objective ranking + cross-objective robustness in §8. Re-green oracle-calibration / single-room-oscillation / self-play-decisive / Lanchester-floor / action-oscillation (the ADR 0025 §12 step-4 gate set). **No `WORLD_FORMAT_VERSION` bump.**

**Step 5 — Soak + (optional) `AssaultMode` plumbing.** Docker soak A–D (per [[combat-overhaul-initiative]]) watching the breach-objective outcomes + the seg-57 cohesion canary. If the soak shows the `Breach`/`Drain` distinction matters, the optional `CombatObjective.assault_mode` follow-on (§6) is a *separate, WFV-gated* change. Operator go-ahead, then MMO deploy.

**Gating summary:** every step gated on the decision/agent/eval/bot test suites **plus** the per-objective tournament gate (`per_objective_profiles_are_each_best_in_class`, Step 2).

---

## 8. Adoption ledger

| Objective class | Mode | Profile (`KernelParams`: approach/incumbency/discoh/K/spacing) | Basis |
|---|---|---|---|
| OpenCombat | — | `open_combat()` = **a1/i6/d20/K2/s2** (`a1-i6-tight-s2`) | the spacing sweep's winner: best mean payoff against the real-opponent field, beating the otherwise-identical spacing-1 profile at equal exploitability. Spacing was the axis the original grid fixed at 1 — Screeps AoE is pure Chebyshev, so a tight blob eats stacked RMA and overlapping tower fire; spacing 2 sheds it (see ADR 0026a) |
| StructureBreach | Breach / unknown | `breach()` = **a1/i4/d10/K3/s1** (`a1-i4-def`) | low approach (don't over-commit — a winnable force breaches anyway) + LOWER incumbency than open ⇒ move in to range 1 and dismantle. It is the dismantle-needs-range-1 variant of the open winner |
| StructureBreach | Drain | `breach_drain()` = **a1/i6/d10/K3/s1** | breach, but hold longer through the tower-drain soak (incumbency 6) |
| StructureBreach | + safe mode | `open_combat()` (veto) | a shielded base takes zero damage — never spend approach risk |

> **Why approach stays LOW (ADR 0025 §12):** the first `approach = 4` breach seed came from a small quick run and did not replicate at grid scale — with a winnable-sized force, base attack is weakly discriminating and a hot approach just bleeds creeps. The open-combat optimum is low-approach / high-incumbency / tight. The breach profile is therefore *not* chosen by a base-attack lead (base-attack absolute scoring carried a cross-process noise floor at the time it was chosen, since root-caused and eliminated — ADR 0025 §12); it is the principled "move in to dismantle" variant of the open winner, and it is re-tunable now that the sim is bit-deterministic.

---

## 9. Extension — objective & force-composition selection (the *doctrine* registry)

- **Scope:** the same activator-registry shape, one layer up — `screeps-combat-decision/src/doctrine.rs` (`ForceDoctrine` trait registry + `decide_doctrine` + `EngagementContext`/`EnemyCoordination`/`ForcePlan`). Every force-producing site — the bot's `war.rs` offense and defense paths, the SK farm, and the eval's sizing sites — selects and sizes through this one path, so bot and harness field identical squads. Like §3–§8 it carries no kill-switch: selection has exactly one implementation.

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

**Special-case to *select*, size from *observed* intel.** A doctrine's `applies` may key on owner type (Invader / SourceKeeper) to *select* the coordination class + archetype — that is cheap and unambiguous. But `plan()` must *size* from the **observed force** in `ctx` (creep bodies/parts → dps/heal/hits; structures → breach/objective hits), never from type-keyed magic numbers — so the same doctrine is robust to boosted / modded / variant enemies and shares **one** sizing path with the player doctrines. `worst_single` and `defense` are therefore *derived from live intel*, not looked up by type. (Constants like `SK_KEEPER_HP` / `SK_KEEPER_MELEE_DPS` are an acceptable shortcut *only* because NPC bodies are engine-fixed; deriving them from the observed keeper body instead keeps no sizing type-pinned, and makes the SK path the same code as a player kiter duel.)

**Composition is *computed*, and an N-blob is first-class.** `ForcePlan.composition` is not a fixed registry pick — the registry templates (`quad_ranged`, `duo_sk_farmer`, …) are *seeds*; `sized_for` already grows the member **count** when one creep can't hold the required parts, and that growth **is** a blob. So the output is a *blob of N sized creeps* whenever the force demands it — a quad is just the N = 4 **efficient-formation** case (the 2×2 that paths and holds as one unit), **not a cap**. N is dynamic on **both** sides of the fight:
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

**The creep-clear sizing primitive `force_sizing::clear_force`.** A creep-clear is NOT a structure breach: where `assess` sizes a structure's kill-DPS to the squad's *gross* (so rampart repair can't stall it), `clear_force` sizes to the **enemy** — kill-DPS = enough to grind their HP net of their heal within the on-site window **and** to out-power them by a `dps_margin`, plus heal to out-heal the incoming. The coordination axis is the caller's: **Individual** passes the worst single + `dps_margin = 1.0` (beat that one); **Coordinated** passes the aggregate + `dps_margin = COORDINATED_DPS_MARGIN` (the square-law over-match, §9.8-tunable). The margin scales the KILL parts only (heal is sized to the incoming either way). `(ForceAssessment, RequiredForce)` out; unwinnable ⇒ all-zero.

**`COORDINATED_DPS_MARGIN = 1.5`, and why that number.** Swept 1.0–2.0 against a graded creep-clear bed (payoff = winning dominates, then leanest cost): the win plateau starts at **1.4** — below it (≤1.3) a lean ranged squad *stalls*, because at margin 1.0 it cannot close on open-field kiters at all — and ≥1.75 only adds cost. 1.50 is the 1.4 cliff plus a ~7% buffer, following the same "hold through variance" philosophy as `HOLD_MARGIN`; adopting the exact cliff would overfit a four-scenario bed. That sweep also **refines §9.4**: a *grouped* force fights as `Coordinated` even without mutual heal, because the square-law over-match is what lets the attacker close and clear at all. **Grouping is itself a Coordinated signal.**

### 9.5 The starter doctrine set (the named rules)

Collection order = priority; first activator wins (the §8 registry shape):

| Doctrine | `applies` (classifier) | Coordination | ForcePlan |
|---|---|---|---|
| `SafeModeSkip` | `defense.safe_mode` | — | not winnable → skip (hard veto; mirrors `SafeModeHold`) |
| `SkSuppression` | `Farm{SourceKeeper}` | Individual | sized SK-farm duo (heal out-heals one keeper; ranged kills it) |
| `NpcCore` | `InvaderCore{level}` | Individual | oracle-sized ranged force (a ranged weapon kills the dismantle-immune core) |
| `InvaderCreeps` | `InvaderCreeps` | Individual | sized against the worst single wave creep |
| `PowerBankFarm` | `PowerBank` | Individual (bank is inert) | ROI-gated duo + hauler(s) |
| `ResourceDenial` | `ResourceDenial` | — | opportunistic lone harasser, LOW priority, no gate (throwaway) |
| `PlayerRaid` | `AttackFlag` / `Expansion` vs an owned base | **Coordinated** | oracle-sized to the aggregate with the square-law margin, growing to an N-blob; objective = `Secure` (clear creeps) or `Dismantle` (raze) by what's present |
| `PlayerDefend` | `Defend` / `ThreatResponse` | **Coordinated** | sized defender squad — **subsumes `DefenseEscalation::from_threat`** |

`PlayerRaid` / `PlayerDefend` are why the coordination axis exists: they are the two arms whose enemy fights *together*. **`PlayerRaid` needs the §12.7(B) creep-target oracle path** (an `enemy_creep_hits` field + a `clear_creeps` Lanchester branch) that the AttackFlag/Harass re-adjudication (ADR 0020 §12.6) deferred — this section is its design home, and the deferral's stated reason (the oracle is structure-shaped and `candidate.defense` is `None` for those arms) is exactly what `EngagementContext` + the Coordinated branch fix.

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
| `screeps-ibex/src/operations/war.rs` | Offense `match` → `decide_doctrine`; defense sites → `defense_doctrines()`/`GarrisonDefense`, with `DefenseEscalation::from_threat` **deleted**. |
| `screeps-combat-eval/src/tournament.rs` | Per-doctrine beds (an Individual NPC bed + a Coordinated player-squad bed) + the `doctrines_are_each_best_in_class` gate. |

**Rungs** (map onto ADR 0020 §12.7 R5.5 → R8):
1. **Refactor-to-registry (no-op).** Re-express `SafeModeSkip` + `NpcCore` + `SkSuppression` + `ResourceDenial` as doctrines; swap the offense `match` for `decide_doctrine`. Behavior byte-identical (the existing sizing is unchanged); the win is the seam. No kill-switch — a verified no-op needs no second path. **No WFV.**
2. **`PlayerDefend`.** Replace `from_threat`'s 3-bucket escalation with a Coordinated-sized defender; gate on a Coordinated defense bed.
3. **`PlayerRaid` (R8).** Build the §12.7(B) creep-target oracle path, then the doctrine; gate on a Coordinated raid bed. This is the deferred AttackFlag/Harass work, now with a home. **Prerequisite — N-blob spawning + formation:** the `SquadManager` spawn path and the agent formation/movement must field an **arbitrary-N** blob (not just the quad 2×2 layout), since `sized_for` can grow past 4 and the square-law raid wants it. Quad stays the efficient-formation special case; the blob is the general one.

**Serialization:** none — `ForcePlan` is a per-target decision, recomputed, never stored (like §6). **No `WORLD_FORMAT_VERSION` bump** at any rung. **Deploy gating:** ADR 0020 §10 Docker-soak → operator go-ahead; never MMO without it.

### 9.8 Tuning integration — the dynamic weights

Doctrine **selection** stays discrete (the §3.3/§5 categorical decision stands — `applies` is a classifier, not a continuous/learned policy). But the **weights inside each doctrine's sizing are continuous, and the tournament tunes them** — exactly the §4 pattern (discrete named profiles, tuned `KernelParams` *within*). So "dynamic" here = tuned boundaries + margins, **not** a learned end-to-end policy; §5's rejection of a continuous *selection* policy is untouched. This is the governing point: wherever squad selection needs a continuous knob, the harness should *discover* its value, not have it hand-set.

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

The tournament sweeps `DoctrineParams` over each bed and adopts the payoff-maximizing set (won objectives − creeps lost − energy spent — the EV currency, ADR 0020-S5), the same per-regime adoption as §4 / §8. A **bit-deterministic sim** is what makes these margins cleanly tunable — the same enablement that unblocked the base-attack re-tune (§8) — so the boundaries are *discovered*, not ideated (the "tournament-discovery beat ideation" finding, [[sim-determinism-fence]]). **Gate:** `doctrines_are_each_best_in_class` **plus** the tuned weights beat their hand-set priors on both beds. A doctrine that mis-classifies coordination or under-sizes *loses self-play*, so the gate is self-policing.

A concrete near-term win: `PlayerDefend`'s `defend_size_curve` replaces `from_threat`'s three magic thresholds (`200`/`150`/`60` dps etc.) with a curve the Coordinated bed tunes — the first hand-set combat constants this layer retires.

### 9.9 Resolved design questions

- **Q1 — coordinated unless a positive NPC signal.** Mis-classification is asymmetric: calling a player "Individual" *under-sizes and loses creeps*; calling an NPC "Coordinated" merely *over-spends*. ⇒ the prior is **`Coordinated` unless a positive NPC signal** (owner ∈ {Invader, SourceKeeper, unowned}), and `coordination_dps_threshold` (§9.8) is swept *from* that prior and must beat it. The classifier defaults to the safe (over-spend) side and only asserts `Individual` on a definite NPC owner.
- **Q2 — blob-vs-quad is tunable, not decided by hand.** Blob vs quad for a Coordinated raid is `blob_escalation_parts`, swept on the player-squad bed — not a hand decision.
- **Q3 — independent objectives, guarded against bleed.** A player base *with* an NPC core is separate candidates → separate doctrines/objectives (the registry stays per-candidate; no blended coordination value). The binding condition: independence must not *bleed energy when a call is wrong*. That guard is **existing mechanism, not new** — each objective is held by the **winnability gate** (`plan().winnable == false` → skip, the InvaderCore-arm pattern) **+** the `ObjectiveKind` **give-up backoff** (`objective_queue` proximity/backoff), so a mis-sized or mis-classified engagement *backs off and stops re-spawning* rather than feeding creeps into a continued loss. A wrong independent call costs at most one bounded, backed-off attempt — acceptable per the condition.

### 9.10 Per-doctrine design notes

The rules each named doctrine encodes, beyond the one-line summaries in §9.5. The **L-ids are stable anchors** for the items other ADRs cite (L1 = the registry itself, §9.2–§9.3; L2 = the `clear_force` primitive, §9.4):

- **L3 / L3a — `GarrisonDefense` (defense, unified).** BOTH defense sites — owned-room and remote-invader — select through `defense_doctrines()`, and the parallel three-bucket `DefenseEscalation::from_threat` enum is **deleted**: one selection path, no second escalation ladder to drift. **L3b — threat-proportional sizing:** the doctrine picks the member COUNT from the observed threat and `clear_force`-sizes the PARTS to out-power and out-heal it (`COORDINATED_DPS_MARGIN` over-match, so it never under-defends). Defense passes **`hits = 0`**: a defender has no kill deadline, so the binding constraint is out-powering the incoming dps, not grinding a HP pool inside a window. The count thresholds are the §9.8 `defend_size_curve` — tunable, not hand-set for good.
- **L4 — `PlayerRaid` (offense, always-field).** `clear_force`-sizes the raid to out-power and out-heal the defenders, and **always fields** rather than gating: with no intel it degrades to the generic raid squad, so the doctrine is inert until the offense path supplies enemy-force intel. That intel comes from cross-referencing the flag's room against the threat scan (estimated enemy dps/heal) plus a `DefenseProfile` carrying the **towers** ranged to the flag tile — so a scouted raid is sized to out-heal creeps *and* towers, while an unscouted one is not under-fielded.
- **L7 — `SkSuppression` (SK farm).** Sized as the kited duel it is, NOT with `clear_force`: the keeper kites and out-heals, so there is no square-law over-power to buy. Out-heal the keeper's melee × `HOLD_MARGIN`, and size the kiter's RANGED to kill the keeper's HP inside the kill window. The SK farm builds the keeper as an `EnemyForce` and selects through `sk_doctrines()` like every other producer, rather than carrying its own inline sizing.
- **L5 — N-blob formation is a build requirement, not just a sizing one.** Sizing can grow a force past four members, so the formation layout must be **arbitrary-N**: a compact ⌈√N⌉ box (`FormationLayout::box_formation(count)`, where `count == 4` is exactly the 2×2), used by every layout site including the death-degrade re-layout and the corridor re-form. A fixed four-offset box silently stacks grown members on the anchor. `MAX_SIZED_MEMBERS = 8` caps N.
- **L6 / L6a — validation bed.** A `CreepClearBed` — an open-room Secure scenario with a graded *grouped* defender force, attacker sized by `clear_force`, driven by the real moving brain and scored on `SideWiped(defender)` — is the gate for every clear-sizing change (`CreepClearWins`). It is also the instrument that produced the margin finding in §9.4, and the host for the **L6b** `COORDINATED_DPS_MARGIN` sweep that fixes that margin's value. **L6c** — the remaining `DoctrineParams` weights (`coordination_dps_threshold`, `blob_escalation_parts`, `defend_size_curve`) are each swept alongside the rung that introduces their consumer, since a weight with no consumer has nothing to sweep over.
- **L8 — coordination should ultimately be read from observed bodies**, not from the candidate's `TargetSource` (§9.3's own principle). Until it is, the Q1 prior holds: Coordinated unless there is a positive NPC signal.
- **L9 — not part of this layer:** the eval's squad-brain traversal lens fields a drivable pre-set composition on purpose. It exercises movement, not selection or sizing, and deliberately does not route through the registry.

## Landed
- `8efa32e` doctrine registry + `decide_doctrine` in the decision crate (2026-06-26)
- `da0756d` bot force producers route through the registry (2026-06-26)
- `c574bc3` per-doctrine harness beds + gate (2026-06-26)
