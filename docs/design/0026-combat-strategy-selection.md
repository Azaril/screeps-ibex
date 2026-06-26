# ADR 0026 — Objective/Information-Dependent Combat Strategy-Selection Layer

- **Status:** Proposed (2026-06-26)
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

| Objective class | Mode | Profile | `KernelParams` | Per-objective score | Cross-objective robustness | Adopted |
|---|---|---|---|---|---|---|
| OpenCombat | — | `default()` / candidate | a2/i3 (default) → candidate **a1-i6-tight** (exploit 0) or balanced **a2-i4-tight** | thorough: default middling (#21/48, exploit 313); `a1-i6-tight` #0 exploit 0 | (is the open-combat baseline) | ⏳ candidate, pending Step 4 |
| StructureBreach | Breach | `breach_hot()` | ~~a4 (quick-run seed — SUPERSEDED)~~ → sweep **incumbency/cohesion with approach LOW (1–2)**; balanced **a2-i4-tight** | thorough: a4 middling-poor; winners are approach 1–2 + tight cohesion | _TBD Step 4_ | ⏳ pending Step 4 |
| StructureBreach | Drain | `breach_drain()` | = default (seed) | _TBD (needs drain scenario)_ | _TBD_ | ⏳ deferred |

> **Seed correction (thorough re-tune, ADR 0025 §12):** the original approach=4 `breach_hot` seed (from a 6-config quick run) **did NOT replicate** at scale (48 configs × 56 beds × 52 Raze+Breach bases). With a winnable-sized force, base-attack is weakly discriminating and approach stays LOW (1–2); the real levers are **incumbency + cohesion (tight)**. The open-combat optimum is also low-approach/high-incumbency/tight (`a1-i6-tight`, unexploitable), and `a2-i4-tight` is the best balanced all-rounder. Step 4's per-objective tournament fills the adopted constants; the breach profile should sweep incumbency/cohesion, not approach.
