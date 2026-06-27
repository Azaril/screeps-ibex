# ADR 0031 — Capability-driven force composition (the assembler)

- **Status:** Proposed
- **Date:** 2026-06-27
- **Extends:** [0026 §9](0026-combat-strategy-selection.md) (the doctrine registry + force-sizing model), [0029](0029-generalized-force-composition.md) (generalize composition onto the one oracle — the *first* increment), [0030](0030-squad-composition-size-tuning.md) (the lifetime/wave tempo axis + the winnability-validated deploy gate + the `is_sized`/silent-fallback cleanup — *more* increments), [0020 §12](0020-ev-adaptive-blob-combat.md) (the force-sizing oracle + the deferred part-auction), [0028](0028-lifecycle-harness.md) (the lifecycle/rally harness that proves it)
- **Supersedes:** the `template() + SquadComposition::sized_for` sizing mechanism (ADR 0026 §9 / 0029) and the variant catalogs; recasts ADR 0030 D12/D14/D15/D19's cleanup as **structural** rather than incremental (see §9).

> **This is the disruptive end-state ADRs 0029 and 0030 were incrementing toward.** 0029 generalized composition onto a single oracle; 0030 added the lifetime/wave tempo, the winnability gate, and named the `is_sized()` lie + the silent-fallback "murky middle" — but both *kept* the template + `sized_for` mechanism and the two hardcoded catalogs. This ADR retires that mechanism entirely: a **capability vector → a marginal-fill assembler** replaces template+`sized_for` across **all** doctrines, deletes the variant catalogs, and **subsumes**: the Layer-B granularity gap (the solo↔quad snap), the silent-fallback murky-middle (0030 §10), `is_sized()` (0030 D14), and the winnability gate (0030 §6) — which is recast as "validate the **assembled** force." Prefer deletion over abstraction throughout.

**Provenance:** the Track-4 migration brief (`docs/design/force-composition-generalization-handoff.md`) + a code-confirmed root-cause pass. Every `file:line` here was re-confirmed against the working tree (master @ `4741990`) this session.

---

## 1. Context — three verified root causes + the `is_sized` lie + the two catalogs

Combat squads are selected by a `ForceDoctrine` classifier and sized by one of **three divergent maths**
(`assess` for structures, `clear_force` for creeps, a bespoke SK calc), each then scaling a **hardcoded
template** via `SquadComposition::sized_for`. A correctly-classified siege under-fields against a defended core.

**Measured (this session, `screeps-combat-eval`):** an oracle-sized siege comp (`siege_quad` = WORK + HEAL),
FORMED under economy contention then MOVING with breach tactics vs a `Guard`-defended core →
**Timeout at 0 damage, never breaches.** Swap the comp to `quad_ranged` (anti-creep) → **Killed** (3 of 4
configs). The **pre-placed** path (`breaches()`/`SizingWins` → `siege_intents`) wins ~99% on the *same* defended
beds. So the gap is entirely in the **moving** engagement, and an anti-creep weapon closes it.

### 1.1 The three stacked defects (A / B / C)

- **Layer A — the brain scores dismantle as zero offense.**
  `our_dps = Σ(member.melee_power + member.ranged_power)` excludes `dismantle_power`:
  `screeps-combat-decision/src/kernel.rs:360`, `lib.rs:1021`, `lib.rs:1101`. `EvMember` (kernel.rs:222) and the
  member view (lib.rs:863–874) **already carry `dismantle_power`** — it is simply never summed. A WORK+HEAL
  siege reads `our_dps == 0`. In `assess_engage` (lib.rs:1013) the `killable_dps == 0 && unkillable_dps == 0`
  hold-case (lib.rs:1055) requires **both** zero, so a guard present (`unkillable_dps > 0`) misses it →
  `fighting_strength(0, ehp, 2) == 0` (lib.rs:987) → Lanchester `balance = -1000` → **retreat at t0** even when
  winnable. With `our_dps == 0`, `ev_target_order`/`select_focus_target` (lib.rs:339/264) also net every creep to
  0, so the squad never even fixates the structure.

- **Layer B — `sized_for` can't add a role a template lacks, and snaps the count.**
  `SquadComposition::sized_for` (`composition.rs:723`) iterates a fixed 4-role array (lines 753–758) and
  **skips any role the template lacks**: `if total == 0 || template_count(role) == 0 { continue }`
  (`composition.rs:761`). It also **floors the per-role count at the template slot count**:
  `count = total.div_ceil(cap).max(template_count(role))` (`composition.rs:770`). A `siege_quad` (Dismantler +
  Healer only) can therefore never gain a `RangedDPS` slot even if `force.ranged_parts > 0`, and a single-slot
  template snaps solo(1) ↔ quad(≥4) with nothing in between. (A second silent gap: `spec_for` maps only
  Healer/Dismantler/Tank/RangedDPS — MeleeDPS + Hauler fall to an empty `CombatBodySpec::default()`,
  composition.rs:733.)

- **Layer C — `RequiredForce` has no anti-CREEP kill dimension.**
  `RequiredForce { heal_parts, dismantle_parts, ranged_parts, tough_parts }` (`force_sizing.rs:285`). Its
  `ranged_parts` is **anti-STRUCTURE** DPS for a dismantle-immune core (force_sizing.rs:290–295, set from
  `required_dismantle_dps` in `from_assessment` at 313), **not** "kill the blocking guard." The structure
  doctrines fold enemy creep DPS only into the **heal** requirement (`assess`: `incoming = tower_dps +
  enemy_dps`, force_sizing.rs:153), never a **kill** requirement.

These compound: the structure doctrine never asks for anti-creep (C); couldn't field it into a `siege_quad` if
it did (B); the brain scores the weaponless result as zero strength and retreats (A).

### 1.2 The `is_sized()` lie

`is_sized()` (`doctrine.rs:168`) means "use the generic `assess` structure path," **not** "is dynamically
sized." Of the four `is_sized()==false` doctrines, three size internally via a custom `plan()`:

| Doctrine | file:line | `is_sized()` | Actually sized? | Sizing math → template |
|---|---|---|---|---|
| `NpcCore` | doctrine.rs:186 | `true` | dynamic | `assess` (structure) → `quad_ranged` |
| `SiegeBreach` | doctrine.rs:205 | `true` | dynamic | `assess` (structure) → `siege_quad` **(no anti-creep role)** |
| `PlayerRaid` | doctrine.rs:229 | **`false`** | **dynamic** | `clear_force` (custom plan, 243–255) → `quad_ranged` |
| `GatedPlayerRaid` | doctrine.rs:266 | `true` | dynamic | `clear_force` (custom plan, 280–303) |
| `HarassRemote` | doctrine.rs:307 | **`false`** | **truly fixed** | none → `solo_harasser` |
| `GarrisonDefense` | doctrine.rs:330 | **`false`** | **dynamic (continuous)** | `clear_force` (custom plan, 344–365) |
| `SkSuppression` | doctrine.rs:398 | **`false`** | dynamic | bespoke SK kite (custom plan, 412–433) |

Only `HarassRemote` is genuinely unsized. The custom `plan()`s each `.unwrap_or_else(static_template)` when
sizing fails (PlayerRaid:253, GarrisonDefense:363, SkSuppression:420) — the **silent static fallback** ADR 0030
D12/D19 forbids. Heal is *already* unified (every path routes through `bodies::defender_heal_parts_for_dps`,
force_sizing.rs:309 / SK doctrine.rs:415) — the one parity this ADR preserves.

### 1.3 The two hardcoded catalogs

Both `Serialize`/`Deserialize`, persisted in mission state:

- **Role-set templates** — `SquadComposition` constructors: `solo_ranged` (composition.rs:326),
  `duo_attack_heal` (340), `duo_tank_heal` (360), `quad_ranged` (380), `quad_siege` (408), `duo_drain` (436),
  `solo_harasser` (456), `duo_melee_heal` (470), `duo_sk_farmer` (492), `power_bank_duo` (513),
  `power_bank_haulers` (534), `siege_quad` (551), `solo_core_attacker` (579).
- **Body templates** — the `BodyType` enum (`composition.rs:65`) — ~20 static shapes dispatched in
  `body_definition` (96), mostly superseded by `BodyType::Sized(CombatBodySpec)` (91), which builds via
  `build_combat_body` (`bodies.rs:72`). `CombatBodySpec` (`bodies.rs:23`) already carries all six combat part
  fields (tough/attack/ranged_attack/work/carry/heal) — **an assembler can already emit any mixed body today.**

### 1.4 Working-tree scaffolding (do not soften)

- `run_defended_lifecycle` (`screeps-combat-eval/src/harness/lifecycle.rs:257`) — oracle-sizes a breach force for
  a rampart+tower+`Guard` core, FORMS it under economy contention, drives it MOVING.
- `oracle_sized_force_forms_and_kills_a_defended_core` (lifecycle.rs:470) — `#[ignore]`d KNOWN-FAILING; asserts
  `Killed`. **Un-ignore when the model lands; do not soften.** `defended_lifecycle_is_deterministic` (483) passes.
- `pub(crate)` helpers in validate.rs: `derive_profile` (153), `siege_ceiling` (841), `siege_doctrine_plan`
  (195) — which **hardcodes `enemy_force: None` (validate.rs:200)**: the eval has never fed defenders to a
  doctrine. `enemy_force_of` (validate.rs:653) exists.
- Bot already populates `enemy_force` for every offense candidate (`war.rs:1077`); structure arms ignore it.
  `war.rs:1030` maps only `KillImmuneStructure`/`ClearCreeps`/`RaidCreeps` — **no `DismantleStructure` producer**
  (the bot-wiring follow-up, §3 P4).
- `WORLD_FORMAT_VERSION` is **18** (`screeps-ibex/src/game_loop.rs:652`) — memory/older docs saying 13/14 are
  stale.

### 1.5 Squad-generation audit (2026-06-27, operator-requested — "validate no other squad-gen exists")

A whole-workspace sweep for composition building (`SquadComposition::*`, `ForceRequirement::single`, body
catalogs). The unification target: **every fielded combat squad is generated by the ONE driver
(`emit_requirement → assemble_force`); no squad-gen lives outside it (D15).** Findings:

- **Already routed through the doctrine registry** (→ become driver calls in P4): offense (`war.rs:1175`,
  `default_doctrines`), owned-room defense (`war.rs:389`, `GarrisonDefense`), remote-invader defense
  (`war.rs:569`, `GarrisonDefense`), SK farm (`sourcekeeperfarm.rs:409`, `SkSuppression`).
- **GAP — squad-gen OUTSIDE the registry (the smell to unify in P4):**
  - `war.rs:467` — operator `defend`-flags field a **hardcoded `SquadComposition::duo_attack_heal()`** — NOT
    sized, NOT via a doctrine. P4 routes it through the defense doctrine sized to the room's observed threat.
  - **Silent-static fallbacks** `.unwrap_or_else(SquadComposition::solo_ranged)` (`war.rs:375`, `war.rs:556`)
    and `.unwrap_or_else(SquadComposition::duo_sk_farmer)` (`sourcekeeperfarm.rs:403`) — the no-silent-static
    violation (ADR 0030 D12/D19). P4 deletes them: `None` is an honest defer (for owned-room defense, a
    minimal *assembled* floor, never a hardcoded comp).
- **Dead catalog (zero live callers — delete trivially in P4):** `duo_tank_heal`, `duo_drain`,
  `duo_melee_heal`, `quad_siege`, `power_bank_duo`, `power_bank_haulers`, `solo_core_attacker`. **Power-bank
  harvesting has no live squad-gen** (those two constructors are dead; `PowerBank` is only an `ObjectiveKind`
  / target source). So power-bank is out of scope — no parallel squad path to unify.
- **NOT generation, but quad/duo NAMING smells (D14):** `FormationShape::Box2x2` (composition.rs) +
  `military/formation.rs` (`is_valid_quad_position`, `apply_quad_cost_overlay` — a hardcoded **2×2** that is
  WRONG for the assembler's 1..=8 member range), and `from_threat`-era comments. The formation must be
  **footprint-driven from the member count**, not a named "quad" shape. Tests / `screeps-combat-agent`
  opponent models name "quad" cosmetically (opponent modeling + the sim's runtime squad — not OUR gen).

---

## 2. Decision

Replace **template + three sizing maths + `sized_for`** with **one requirement-emitter** (T1) that outputs a
single capability vector per objective, and **one composition assembler** (T2) that fills that vector into
role-slots + `Sized` bodies by a deterministic **marginal-capability-per-energy** rule. A doctrine becomes a
**pure classifier** (T3a); the brain's Layer-A scoring is fixed so an assembled force is never scored
zero-strength (T3b); `is_sized()`, the silent fallbacks, and the two catalogs are **deleted** in a phased,
offline-first migration ending in one `WORLD_FORMAT_VERSION` reset (T4). The full Decisions list is §7.

### 2.1 T1 — the unified requirement (capability vector)

**EXTEND `RequiredForce`** (force_sizing.rs:285) — do *not* add a parallel type (the assembler already keys off
it; `defender_heal_parts_for_dps` parity lives there; it threads `ForcePlan.required`, the bot log, the eval).
Add the missing Layer-C anti-creep term and disambiguate `ranged_parts`:

```rust
// force_sizing.rs — the capability vector (Layer C fix)
pub struct RequiredForce {
    pub heal_parts: u32,          // Σ HEAL — out-heal incoming (towers + defenders) × HOLD_MARGIN. UNCHANGED.
    pub dismantle_parts: u32,     // Σ WORK — breach + raze a DISMANTLE-able ring. UNCHANGED.
    pub immune_struct_parts: u32, // RENAME of `ranged_parts`: RANGED to kill a dismantle-IMMUNE structure
                                  //   (invader core / keeper-as-structure). The SAME structure DPS, in RANGED.
    pub anti_creep_parts: u32,    // NEW (Layer C): RANGED/ATTACK to KILL blocking DEFENDER creeps.
    pub tough_parts: u32,         // UNCHANGED (v1 = 0).
}
```

**The anti-creep kill term is the kill term Layer C lacked.** It is sized from `ctx.enemy_force` via the
existing `clear_force` out-power math whenever defenders are present, **independently** of `immune_struct_parts`
(sized from `required_dismantle_dps`). Keeping the two separate — both RANGED at the part level — lets the
assembler field **enough to do BOTH** (raze the immune core AND clear the guard) rather than `max()`-ing them: a
siege facing a guard needs WORK(dismantle) **and** RANGED(anti-creep) **simultaneously**, the exact gap the
measured `siege_quad` timeout exposed.

The conceptual dimensions the operator listed map onto the fields as: `out_heal_per_tick` → `heal_parts`;
`structure_dismantle_dps` → `dismantle_parts`; `immune_structure_dps` → `immune_struct_parts`;
`anti_creep_kill_dps` → `anti_creep_parts`; `tank_ehp` → `tough_parts`. `kill_in_window` is an **input** to the
emitter (the on-site-budget term), not a stored field. **Anti-creep-creep kill (the new term) is the only
behavioral addition; everything else is a rename or a re-route.** `RequiredForce` is *not* `Serialize` (it lives
on `ForcePlan`, which is computed each tick) — so the rename and the new field cost **no** WFV bump.

### 2.2 T2 — the composition OPTIMIZER (`optimize_composition`) — EV-maximizing, tournament-tunable (operator 2026-06-27, D16)

> **Supersedes the earlier "marginal-fill `assemble_force`" + the `force_ceiling` budget.** Operator critique:
> both PRESUMED a composition — `force_ceiling`'s 3-fighter+5-healer shape (inherited from the eval's
> `siege_ceiling`) is just another hardcoded template judging winnability, and the fixed `ceil(demand/cap)`
> marginal fill presumes a 1:1 role↔dimension shape. Neither OPTIMIZES; both assume the answer. The real
> problem is **multi-dimensional** — body parts, creep count, energy/spawn availability — and the best squad is
> **the highest expected value, most efficient one, with a margin for a hostile force that changes/grows.** The
> parameters of that trade-off are **tournament-tuned**.

One function replaces `template() + sized_for` AND `force_ceiling` (there is NO presumed reference squad):

```rust
// composition.rs
pub fn optimize_composition(
    req: &RequiredForce,        // the capabilities to win WITH MARGIN (from emit_requirement; T1 survives)
    defense: &DefenseProfile, enemy: Option<EnemyForce>,   // for P(win) — the incoming damage + threat
    target_value: f32,          // V — the objective's worth, in energy-equivalent units (the over-invest lever)
    member_energy: u32,
    params: &CompositionParams, // the tournament-tuned knobs
) -> Option<SquadComposition>   // the EV-max squad, or None (EV below the commit threshold → defer)
```

**The objective: maximize EV** (operator: "highest expected value and most efficient squad is the best").
`EV(C) = P(win | C) · target_value − cost(C)`, where `cost(C) = w_energy · spawn_energy(C) + w_creep ·
creep_count(C)` (the multi-dimensional efficiency term: energy AND the per-creep forming/CPU/management
overhead). For a fixed target, `target_value` is constant, so maximizing EV trades P(win) (more/over-powered
force → higher win-prob but higher cost) against cost — and `target_value` sets HOW MUCH to over-invest (the
principled EV form of R5/importance). The commit decision is `max EV > params.commit_ev_threshold` (else defer).

**P(win | C)** combines the survival + kill axes (both must hold): `P = win_probability(C.heal, incoming) ·
kill_feasibility(C, defense, window)` — each a logistic on its surplus (heal-surplus, kill-time-surplus). The
**dynamic margin** inflates the OBSERVED hostile force (`enemy × params.dynamic_margin`) before P(win), so the
fielded force over-powers enough that a force which grows/changes still loses (operator's "margin for hostile
forces to change/be dynamic").

**The search (one parameterized search — D16; emergent strategies stay implicit for now, see Consequences):**
bit-deterministic, integer/ceil over Vec-ordered candidates, no HashMap. Enumerate creep splits
`(n_fighters, n_healers)` with `1 ≤ n_fighters + n_healers ≤ MAX_SIZED_MEMBERS (8)` and an over-power factor
`k ∈ {tuned set}`: distribute `k · req` across the members (fighters carry `immune_struct + anti_creep` RANGED
+ `dismantle` WORK; healers carry HEAL), each member sized at `min(member_energy, params.member_energy)` (the
many-small ↔ few-big knob); skip if a needed role gets 0 members or a member exceeds the 50-part cap; build each
member as `BodyType::Sized`; score `EV(C)`. Return the **max-EV** candidate above the commit threshold, else
`None`. Small-many vs few-big EMERGES from the tuned `w_creep`/`member_energy`; the over-power level emerges from
`target_value` vs `cost`. Formation is footprint-derived from `creep_count` (D14); `retreat_threshold` from the
doctrine. `None` is the TERMINAL defer (D10).

```rust
// composition.rs — the tournament-tuned knobs (NOT Serialize — recomputed each tick, no WFV)
pub struct CompositionParams {
    pub w_energy: f32,            // energy → EV cost weight
    pub w_creep: f32,            // per-creep EV penalty (forming/CPU/management overhead)
    pub hold_margin: f32,        // out-heal the incoming × this (was HOLD_MARGIN)
    pub over_power_margin: f32,  // square-law over-power vs coordinated defenders (was COORDINATED_DPS_MARGIN)
    pub dynamic_margin: f32,     // inflate the OBSERVED hostile force (margin for a changing/growing threat)
    pub member_energy: u32,      // per-member energy cap for the search (many-small vs few-big; was PREFERRED)
    pub commit_ev_threshold: f32,// min EV to FIELD vs defer
}
```

All four operator-chosen knobs — **cost weights, safety margins, member-energy split, commit threshold** — are
fields here, swept by the tournament (P6) so the fielded squads are winning-but-EFFICIENT (D13, now the
optimizer's literal objective). `hold_margin`/`over_power_margin` thread into `emit_requirement` (replacing the
`HOLD_MARGIN`/`COORDINATED_DPS_MARGIN` constants); `Default::default()` reproduces today's constants so the
calibration gates hold until P6 re-sweeps. `target_value` comes from the caller (the bot's candidate score; the
eval's scenario), defaulting from `importance`.

**Layer B + the granularity snap remain structurally impossible** — there is no template, the creep count is a
free search variable (1..8 all reachable), and the role mix is derived from `req`. The "winnability budget" /
`force_ceiling` presumption is GONE: winnability = "the search found a candidate with `EV > threshold`."

### 2.3 T3a — doctrine = pure classifier; retire `template()`/`is_sized()`

`ForceDoctrine` loses `template()`, `is_sized()`, and the custom `plan()`s. It keeps `name` + `applies` (the
classifier) + the objective-shaping inputs (objective, coordination, importance, retreat policy). The shared
driver is:

```rust
decide_doctrine(ctx) -> objective
  → emit_requirement(objective, defense, ctx.enemy_force, budget, coordination, importance)   // T1
  → if !assessment.winnable { ForcePlan::skip }                                                 // explicit defer
  → assemble_force(required, ctx.member_energy)                                                  // T2
  → validate_assembled(comp, assessment)                                                         // D6 (§7)
```

No `template()` ⇒ no `.unwrap_or_else(static_template)` anywhere (subsumes ADR 0030 D12/D19). A defer is an
explicit `ForcePlan::skip` (doctrine.rs:116), never a silent template. The 3 custom-plan doctrines
(PlayerRaid/GarrisonDefense/SkSuppression) collapse to objective + shaping; their `clear_force`/SK math moves
into the emitter (T1). `is_sized()` (doctrine.rs:168) is **deleted** — the `enum Sizing { Fixed, Dynamic }`
0030 D14 proposed is unnecessary because *every* doctrine is now dynamic through the one driver; `honor_verdict`
(defer-on-unwinnable) is the only per-doctrine policy bit. **There is no fixed doctrine at all (D11):** even
`HarassRemote` emits a *dynamic* anti-creep vector sized to the room's observed force + a safety margin (its
"deny, don't hold" nature is tactical — `MultiLifetimeWave` + retreat-happy — not a sizing distinction). One
path, no fixed/dynamic fork.

### 2.4 T3b — the brain Layer-A fix (keep the engage gate honest)

The assembler removes the *cause* of the zero-strength score (it always fields an anti-creep weapon when
defenders are present), but the brain must *also* represent dismantle as real offense so any non-assembler caller
and the undefended-raze case are scored correctly. We do **both** (operator-resolved):

- **`our_dps` += `dismantle_power`** at kernel.rs:360, lib.rs:1021, lib.rs:1101 (the data already exists on the
  member view). A WORK+HEAL siege now reads `our_dps > 0` → `fighting_strength > 0` → favorable balance, no
  retreat at t0.
- **Gate the `select_focus_target`/`ev_target_order` reorder** (lib.rs:264/339) — a **shared-kernel** change.
  Only reorder to a structure focus when `ev_target_order` is **empty AND a hostile structure exists** (the
  existing structure-fallback condition). Regression-prove against `CreepClearWins` (validate.rs:698) — it must
  stay at its current win rate. (The existing fallback already fires when `ev_target_order` is empty; the
  Layer-A sum just lets the siege reach it. No unconditional creep-target reorder is introduced.)
- **`assess_engage`'s both-zero hold case** (lib.rs:1055): with the Layer-A sum + always-anti-creep assembly the
  weaponless case no longer arises in practice; the sum also makes a pure dismantle-vs-out-healed-structure case
  reach the hold/engage path honestly rather than being papered over.

---

## 3. Migration (T4) — phased, offline-first; one WFV reset

Every phase lands in `screeps-combat-decision` + `screeps-combat-eval` **first** (offline-provable), with bot
wiring (`war.rs`) as a tracked follow-up — the bot lacks a `DismantleStructure` producer (war.rs:1030) and a
real G4-HEAVY defer target. Each phase says what it **deletes**.

### Phase 0 — Layer A brain fix (no new types, no WFV)
- **DONE (P0a, commit `5db5e08`):** `assess_engage`'s `our_strength` (lib.rs) adds the squad's `dismantle_power`
  WHEN a hostile structure is present (a dismantle target) -> fighting STRENGTH > 0 -> no retreat at t0.
  CORRECTION: the literal "`our_dps += dismantle_power` at kernel.rs:360/lib.rs:1021/1101" was IMPRECISE —
  those `our_dps` feed `ev_target_order`'s creep-killability (lib.rs:346, `net = our_dps - heal`), and dismantle
  can't kill creeps; adding it there mis-scores dismantle as anti-creep. The strength fix is `assess_engage`'s
  `our_strength` ONLY (creep-targeting stays melee+ranged -> CreepClearWins-safe).
- **DEFERRED — the focus half (P0b, obviated by P3).** A pure-dismantle squad (`our_dps==0`) still fixates an
  unkillable guard via `select_focus_target` fallback-1. The assembler (P3) fields anti-creep -> `our_dps>0` ->
  the existing structure-fallback works, so a properly-assembled siege never hits this. A `select_focus_target`
  gate on `our_dps==0` OVER-REACHES (a healer remnant is `our_dps==0` too -> it disengaged to the core -> a
  self_play standoff); a precise gate needs the dismantle signal threaded into `select_focus_target`. Hold for P3.
- **Deletes:** nothing.
- **Tests:** `CreepClearWins` (validate.rs:698) stays at its current win rate; `OracleCalibration`
  (validate.rs:94) FP ≤ 0.010 / FN ≤ 0.200 unchanged; `sim_is_deterministic_over_rounds` (tournament.rs:804)
  green; a new Layer-A pin — a WORK+HEAL `SquadView` vs a guarded structure scores `our_strength > 0` /
  balance > -1000.

### Phase 1 — capability-vector model + narrow SiegeBreach fusion (sanity-check, no WFV)
- Extend `RequiredForce` with `anti_creep_parts`; rename `ranged_parts` → `immune_struct_parts`
  (force_sizing.rs:285). Update `from_assessment` (304), `scaled` (334), `as_solo_spec` (320), `sized_for`
  (757), `capabilities` (688), and the doctrine/SK/eval readers (SkSuppression.plan:416, war.rs log:1140,
  `clear_outcome_at:689`, the calibration tests).
- In the eval, **stop hardcoding `enemy_force: None`** (validate.rs:200) — feed `enemy_force_of`
  (validate.rs:653) into `siege_doctrine_plan` so the bed sees the `Guard`; also populate `derive_profile`'s
  defender kill inputs. Add the anti-creep term to the existing `SiegeBreach` path as a within-doctrine fusion
  (the brief's increment 1) so `siege_quad` gains a RangedDPS slot — the model sanity-check.
- **Deletes:** nothing yet (rename only).
- **Tests:** the **discriminator** (ADR 0029 §10 #1) — for the defended fixture the SAME comp must breach
  PRE-PLACED via `breaches()`/`siege_intents` (validate.rs:284/246). Pre-placed kills + moving misses ⇒
  form/travel/brain; both miss ⇒ under-sized. Keep `oracle_sized_force_forms_and_kills_a_defended_core`
  `#[ignore]`d until Phase 3 if fusion alone doesn't close it — **do not soften.**

### Phase 2 — `emit_requirement` (T1): collapse the three maths (no WFV) — **DONE (decision `778e93d`, eval `ac61b0b`, super `6bd8e1b`)**
- **DONE:** T1 is a pure function `emit_requirement(objective, defense, enemy_force, budget: Option<&ForceBudget>,
  coordination, importance) -> (ForceAssessment, RequiredForce)` (doctrine.rs) composing `assess` + `clear_force`
  + the SK kite terms. ALL six doctrine `plan()`s are now thin callers: `sized_plan` (NpcCore), SiegeBreach,
  PlayerRaid, GatedPlayerRaid, GarrisonDefense, SkSuppression. The anti-creep overlay (was duplicated in BOTH
  `sized_plan` AND `SiegeBreach::plan`) is now one `overlay_anti_creep` helper. `budget` is `None` only for SK
  (it sizes from the keeper). HarassRemote stays genuinely fixed (its `Harass` emitter arm is the seam P4 routes
  the dynamic harass through). Still emits into the OLD `sized_for` (the assembler was isolated until P3).
- **Deletes:** the per-doctrine sizing forks' *math* moved into the emitter (the `plan()` bodies still call it).
- **Tests DONE:** `emit_requirement_is_deterministic_over_objectives` (the run-twice fence) +
  `emit_requirement_reproduces_per_objective_semantics` (decision) +
  `emit_requirement_golden_output_is_stable_over_realistic_bases` (eval — bed-level byte-stability, defenders
  fed in). Exact-behavior-preserving (all prior doctrine unit tests unchanged + green). decision 179, eval 61.

### Phase 3 — `assemble_force` (T2): replace `sized_for` (no WFV) — **DONE (decision `5079bf8`, eval `38fd534`, super `da79345`)**
- **DONE:** T2 implemented alongside `sized_for` (parity bridge). `assemble_force(&RequiredForce, member_energy)
  -> Option<SquadComposition>` (composition.rs) fields the capability vector directly (no template, no catalog):
  continuous per-role count (`ceil(demand/cap)`), role-SET floor (≥1 per demanded role — Layer B gone),
  RANGED = immune_struct + anti_creep, `None` terminal past `MAX_SIZED_MEMBERS` (D10). `run_defended_lifecycle`
  routes emit → assemble (parameterized into `run_defended_lifecycle_with`). **`oracle_sized_force_forms_and_kills_a_defended_core`
  passes THROUGH the assembler (un-ignored).**
- **Deletes:** nothing yet (`sized_for` retained as the bridge until P4).
- **Tests DONE:**
  - **Determinism:** `assemble_force_is_deterministic`.
  - **Graded regime sweep:** `assembler_kills_across_defended_regimes` — rampart-only / tower-only /
    tower+rampart / corridor-choke melee-guard beds, each FORMED + MOVING via `run_defended_lifecycle_with`,
    asserting `Killed` + deterministic.
  - **OFFENSE continuous-sizing:** `assemble_force_sizes_continuously_no_snap` (count monotonic, minimal force
    is a duo not a quad, 3 reachable — the Layer-B pin) + `assemble_force_fields_the_full_role_set` +
    `assemble_force_defers_terminally`.
  - **Deferred:** the pre-placed-vs-moving discriminator for DEFENDED regimes (the eval's `place_squad` only
    handles Dismantler/Healer tiles, not RangedDPS — extending it would perturb `SizingWins`'s pass/fail on
    defended beds, a calibration gate; the MOVING regime sweep is the primary proof). Shielded-guard /
    ranged-Skirmisher kill assertions also deferred (kiters evade under breach tactics — not a sizing failure).
  - `OracleCalibration`/`SizingWins`/`CreepClearWins` green + discriminating; FP/FN gates held.

### Phase 4 — route all doctrines through emit+assemble; retire `is_sized` + silent fallbacks; DELETE catalogs + WFV (D12 folds P5 in)

**P4a — functional unification — DONE (decision `4691c00`, eval `54da38c`, super `0fb1370`).** `ForceDoctrine`
is a pure classifier (`name`/`applies`/`fighter_role`/`honor_verdict`/`retreat_threshold`); the shared free fn
`plan_engagement(doctrine, ctx, budget)` (emit_requirement → honor_verdict gate → assemble_force) is the SOLE
path; `sized_plan`/`ForcePlan::fixed`/the custom `plan()`s are deleted; `force_ceiling` (P4.1) is the
template-free budget (the bot's `best_force_budget` takes a `SquadRole`). Bot offense/defense/SK + the eval all
route through it; the `unwrap_or_else(solo_ranged/duo_sk_farmer)` + the hardcoded defend-flag `duo_attack_heal`
are gone (D15); always-field doctrines floor at a minimal vector + scale with threat (D11); `emit_requirement`
selects the structure weapon (KillImmuneStructure zeroes `dismantle_parts`, DismantleStructure zeroes
`immune_struct_parts`). `capabilities()` reports a Sized body's actual ranged parts (the full-energy ceiling
stays only for template RangedDPS). Green: decision 185, eval 62, clippy-wasm. The catalog constructors +
static `BodyType` variants + `sized_for` are now DEAD (only tests / budget-helpers reference them) — deleted in P4b.

**P4b — DELETE the catalog + the WFV reset — NEXT.** (a) Delete the ≈13 `SquadComposition` constructors +
`sized_for`, migrating their remaining test / eval-helper uses to `force_ceiling` / `assemble_force`; (b) delete
the static `BodyType` variants + `body_definition` arms + `bodies::*_body` (leaving `Sized` + `Hauler`); (c) D14
formation cleanup (`FormationShape::Box2x2` → footprint-from-`slots.len()`) — MAY be a tracked follow-up since
it touches the serialized formation enum + the squad runtime; (d) `WORLD_FORMAT_VERSION` 18→19. **D15 acceptance:
a grep for `SquadComposition::<constructor>` outside the driver + tests returns EMPTY.**

> **⚠ P4 sub-design (RESOLVED in P4a):** the template-free budget is `force_ceiling`, probed at the assembler's
> per-member cap (PREFERRED) so the budget is assembler-fieldable; the eval calibration stays on `siege_ceiling`
> (explicit budget), so the gates were UNPERTURBED (decision 185 / eval 62 green, no re-tune needed yet — P6
> re-tunes after P4b). The original "⚠ resolve first" note below is superseded.

> **⚠ P4 sub-design to resolve FIRST — the winnability BUDGET ceiling.** `emit_requirement`'s `assess`/`clear_force`
> still need a `ForceBudget` = the CEILING capabilities of one squad at `member_energy`. Today every caller derives
> it from a TEMPLATE (`doctrine.template().force_budget(..)` in war.rs; `siege_ceiling(..)` in the eval). Deleting
> the catalogs (Phase 5, folded here) removes those templates, so P4 needs a **template-free ceiling**. The
> conservative ceiling the oracle has ALWAYS judged against is a balanced ~4-member quad (2 fighters + 2 healers)
> at `member_energy` — the assembler can field MORE (grown to 8), so a "winnable" verdict stays conservative. So
> P4 introduces a `squad_ceiling_budget(member_energy, onsite, fighter_role)` (a synthetic 2-fighter+2-healer
> budget reusing `single_role_cap` + the part powers) that **must reproduce the current `siege_quad`/`quad_ranged`
> budgets numerically** — else the `OracleCalibration` (FP≤0.010/FN≤0.200) + `SizingWins` gates shift. Verify the
> calibration gates byte-for-byte before/after. This is the gating risk of P4; everything else is mechanical.

**P4 = the trait gut + the §1.5 unification + the catalog deletion + the WFV bump, in one phase (D12 folds the
old "Phase 5" in). Ordered so the tree stays buildable between steps where possible:**

1. **Budget ceiling (the gating risk above):** add `squad_ceiling_budget(member_energy, onsite, fighter_role)`;
   prove it vs the existing `siege_quad`/`quad_ranged` budgets; accept the calibration delta as P6's to re-tune.
2. **Trait → pure classifier (T3a, D4):** `ForceDoctrine` keeps `name` + `applies` + objective-shaping; **DROP
   `template()`, `is_sized()`, and every custom `plan()`**. One shared driver: `decide_doctrine →
   emit_requirement → winnable? → assemble_force → validate`. **DELETE `sized_plan`, `ForcePlan::fixed`,** and
   the per-doctrine `plan()` bodies. `HarassRemote` becomes a dynamic anti-creep emitter (D11), not a fixed solo.
3. **Unify the §1.5 GAP sites (D15):** route `war.rs:467` (the hardcoded defend-flag `duo_attack_heal`) through
   the defense doctrine sized to the room threat; **DELETE the silent-static fallbacks** `.unwrap_or_else(solo_ranged)`
   (`war.rs:375/556`) + `.unwrap_or_else(duo_sk_farmer)` (`sourcekeeperfarm.rs:403`) — `None` is an honest defer
   (owned-room defense uses a minimal *assembled* floor). The bot gate (`war.rs:1099–1157`) collapses to the
   driver keyed on `honor_verdict`; add the `DismantleStructure` producer (`war.rs:1030`). Bot/eval parity held.
4. **DELETE the catalog + named shapes (D7, D14):** the ≈13 `SquadComposition` constructors (incl. the 7 already
   DEAD: `duo_tank_heal`/`duo_drain`/`duo_melee_heal`/`quad_siege`/`power_bank_duo`/`power_bank_haulers`/
   `solo_core_attacker`) + the static `BodyType` variants + their `body_definition` arms + the `bodies::*_body`
   functions — leaving `BodyType::Sized` (+ `Hauler` for the non-combat haul path). **DELETE `sized_for`.**
   Replace `FormationShape::Box2x2`/named shapes + `military/formation.rs`'s hardcoded 2×2 quad overlay with a
   **footprint-driven** formation from `slots.len()` (D14; valid for 1..=8 — the 2×2 is wrong for ≥5). Migrate
   `siege_ceiling`/`defended_forming`/`choose_fielded_comp`/every test naming a constructor to `assemble_force`.
5. **WFV bump:** `WORLD_FORMAT_VERSION` 18 → 19 (`game_loop.rs:652`); re-prove via `git diff` of the
   serialized-shape types (`SquadComposition`/`SquadSlot`/`BodyType`); confirm `serialized_data` framing +
   the decode fingerprint round-trip a fresh world; operator go-ahead before any MMO deploy.
- **Tests / acceptance:**
  - **The unification check (D15):** a grep for `SquadComposition::<constructor>` outside the driver + its tests
    returns EMPTY; no `quad`/`duo`/`solo` shape name survives in fielded-squad code (D14).
  - The no-silent-static invariant (ADR 0030 D19): an unwinnable/unaffordable objective returns `None`, never a
    static template.
  - All gates green (`OracleCalibration`/`SizingWins`/`CreepClearWins` — re-tuned in P6 if the budget delta moved
    them), all three determinism fences green, `realistic_kernel_tournament` + `assault_score` (route
    `choose_fielded_comp` through the assembler), and the acceptance test + regime sweep still `Killed`.

---

## 4. Proof plan

- **Un-ignore the scaffolded test** (`oracle_sized_force_forms_and_kills_a_defended_core`, lifecycle.rs:470) —
  flips to `Killed` in Phase 3; do not soften. `defended_lifecycle_is_deterministic` (483) stays green.
- **Graded regime sweep** — tower-only / rampart-only / melee-Guard / ranged-Skirmisher / combined / shielded,
  each FORMED + MOVING, `Killed`-when-winnable + clean-defer-when-not.
- **Pre-placed-vs-moving discriminator** — the same assembled comp must also win PRE-PLACED via
  `breaches()`/`siege_intents`; bisects form/travel vs under-size.
- **OFFENSE continuous-sizing test** — count monotonic, starts at the role floor, never snaps 1→4 (the Layer-B pin).
- **Determinism fence over the assembler** (and the emitter) — its own run-twice-equal test (the standing fence
  covers only the hardcoded `quad_ranged`).
- **Gates stay green + discriminating** — `OracleCalibration` (FP ≤ 0.010, FN ≤ 0.200), `SizingWins`,
  `CreepClearWins` graded every phase; their input beds unchanged.
- **Tournament lens** — `realistic_kernel_tournament` (tournament.rs:764) + `assault_score` (validate.rs:466)
  grade the assembler across the regimes a real attacker meets, not just unit fixtures.

---

## 5. Invariants honored (per phase)

- **Bit-determinism:** no result-affecting HashMap iteration; integer/ceil math over Vec-ordered inputs; the
  emitter (P2) and assembler (P3) each get a dedicated run-twice-equal test. `sim_is_deterministic_over_rounds`
  (tournament.rs:804) green every phase.
- **8-member cap:** `MAX_SIZED_MEMBERS = 8` (composition.rs:26) is the assembler's hard defer boundary → `None`
  (multi-squad G4-HEAVY, unimplemented).
- **Smallest-favorable-Lanchester + per-tick-optimal, no hysteresis:** the marginal fill stops at the first comp
  that meets the requirement; the min-viable-role-set floor is correctness, not stickiness.
- **Calibration gates stay live + discriminating:** `OracleCalibration`/`SizingWins`/`CreepClearWins` graded
  every phase; creep-free beds are unperturbed (the anti-creep term fires only when `enemy_force.dps > 0`).
- **Heal parity:** every producer keeps `defender_heal_parts_for_dps` (force_sizing.rs:309 / SK doctrine.rs:415);
  the emitter calls it once.
- **Bot/eval parity:** both select+size through the same decision-crate driver (ADR 0026 §9).
- **No-silent-static fallback:** a defer is explicit `None` (`ForcePlan::skip`), never a hidden constructor
  (subsumes ADR 0030 D12/D19).

---

## 6. Relationship to / supersession of ADR 0029 and 0030

- **0029 (generalize onto the one oracle — first increment):** kept template + `sized_for`. This ADR **replaces**
  that mechanism with the assembler; 0029's "one oracle" *is* the emitter (T1). The discriminator (0029 §10 #1)
  becomes the assembler's acceptance bisection (§4).
- **0030 D12 (silent-static fallback → principled defer):** **subsumed** — there is no template, so there is no
  `.unwrap_or(template)`; `None` is the only defer.
- **0030 D14 (`is_sized() → enum Sizing { Fixed, Dynamic }`):** **superseded** — `is_sized()` is *deleted*, not
  re-typed; every doctrine is dynamic through the one driver (even `HarassRemote`, via a fixed capability vector).
- **0030 D15 (count floor → explicit `min_count`):** **subsumed** — the floor is a min-viable ROLE-SET, derived
  from the vector, not a count input; `composition.rs:770`'s `.max(template_count)` is deleted.
- **0030 D19 (no-silent-static test lock-in + `SizingWins` counts the winnable-but-None case as failure):**
  **carried forward** as the Phase-4 invariant test + the gate discipline.
- **0030 §6 (winnability-validated deploy gate):** **recast** — the gate now validates the **assembled** force's
  capabilities against the requirement (D6, §7), one combat-math home (no second Lanchester model).
- **0030 D16 (`BodyType` variant deletions ride a WFV bump):** **executed** here as Phase 5.
- **0030 §4 (`EngagementTempo` lifetime/wave axis) and D10 (auction reads tempo):** **orthogonal and preserved** —
  tempo decides *what* capability is requested (it parameterizes the emitter's deadline/kill-in-time terms); the
  assembler decides *how that vector is fielded*. The "auction" in this ADR is the assembler's marginal fill (a
  composition mechanism); the deferred body/blob EV auction (0020 R7-R8 / 0029 §8) is a separate cross-goal
  valuation that reads tempo — both can coexist (the assembler is the blob special-case fill).

---

## 7. Decisions

- **D1 — Capability vector (T1, resolves brief §10 "requirement vector shape").** EXTEND `RequiredForce`
  (force_sizing.rs:285), do **not** add a parallel type. Rename `ranged_parts` → `immune_struct_parts`
  (anti-immune-structure RANGED) and ADD `anti_creep_parts` (NEW, anti-creep kill). A siege vs a guarded base now
  carries BOTH `dismantle_parts > 0` AND `anti_creep_parts > 0`. Not `Serialize` ⇒ no WFV from this.
- **D2 — One requirement-emitter (T1, resolves brief §10 "enemy_force single channel").** `emit_requirement`
  composes `assess` (structure terms + heal) + `clear_force` over `ctx.enemy_force` (anti-creep + folded heal,
  margin = `COORDINATED_DPS_MARGIN` if Coordinated else 1.0) + the SK kite terms (the current `SkSuppression`
  math becomes inputs, not a fork) + `RequiredForce::scaled(importance_margin)`. Defenders are read **only** via
  the existing single channel `ctx.enemy_force` (the bot already populates it, war.rs:1077) — no new
  `DefenseProfile` fields beyond plumbing the defender kill inputs `derive_profile` already computes.
- **D3 — The assembler (T2, resolves brief §10 "strategy vs auction").** ONE function `assemble_force(&RequiredForce,
  member_energy) -> Option<SquadComposition>` with a marginal-capability-per-energy fill — **the "auction" IS that
  marginal fill**, not a separate strategy+auction layer. Min-viable ROLE-SET floor (not a template count;
  delete `composition.rs:770`'s `.max(template_count)`); per-member caps probed via the real builder; stop at met
  OR `> MAX_SIZED_MEMBERS` → `None`; frozen tie-break (capability order then role enum, Vec-ordered). Layer B and
  the solo↔quad snap vanish structurally.
- **D4 — Doctrine = pure classifier (T3a, resolves brief §10 by retiring `is_sized`).** `ForceDoctrine` drops
  `template()`/`is_sized()`/custom `plan()`; keeps `name`/`applies`/shaping. Shared driver:
  `decide_doctrine → emit_requirement → winnable? → assemble_force → validate_assembled`. No template ⇒ no
  `.unwrap_or_else(static)` anywhere.
- **D5 — Layer A fix = do BOTH (T3b, resolves brief §10 "Layer A").** `our_dps += dismantle_power` at
  kernel.rs:360, lib.rs:1021/1101 (gate the focus reorder behind "ev_target_order empty AND a hostile structure
  exists"; regression-prove `CreepClearWins`) AND always assemble an anti-creep weapon when defenders are present
  (the assembler's role floor). The brain fix removes the latent foot-gun; the assembler removes the cause.
- **D6 — Winnability gate validates the ASSEMBLED force.** The bot gate (war.rs:1099–1157) and the eval
  (`siege_doctrine_plan`) stop asking "did the template's `assess` say winnable" and instead emit → assemble →
  confirm the assembled comp's `capabilities(member_energy)` meets the requirement (replacing the `sized_for →
  None` defer path). The ROI affordability gate (war.rs:1124) is unchanged. (Recasts ADR 0030 §6.)
- **D7 — Delete the catalogs in a WFV reset (resolves brief §10 "delete vs deprecate" = DELETE).** Phase 5
  deletes the `SquadComposition` constructors (composition.rs:326–590) and static `BodyType` variants
  (composition.rs:66–87), leaving `BodyType::Sized` (+ Hauler). `WORLD_FORMAT_VERSION` 18 → 19 (game_loop.rs:652)
  = one loud reset (operator-accepted: full serialization reset is fine).
- **D8 — Eval-first scope (resolves brief §10 "scope of first landing").** First landing is the eval/offline path
  (P0–P3); the bot wiring (a `DismantleStructure` producer at war.rs:1030 + a real G4-HEAVY defer target) is the
  tracked follow-up (P4+). The assembler's `None` is the clean hand-off point to the unimplemented G4-HEAVY.
- **D9 — Subsume the ADR 0030 cleanup (resolves §6).** `is_sized()` is deleted (not re-typed; supersedes 0030
  D14); the count floor becomes the role-set floor (subsumes 0030 D15); the silent fallbacks are deleted (subsumes
  0030 D12); the no-silent-static test discipline is carried forward (0030 D19). `EngagementTempo` (0030 §4)
  remains orthogonal — it parameterizes the emitter; the assembler fields whatever vector results.
- **D10 — `None` is TERMINAL; remove the G4-HEAVY failover (operator 2026-06-27).** When the assembler can't field a winnable single squad (requirement > `MAX_SIZED_MEMBERS`, or no in-range home affords it), it returns `None` = an honest **"don't attack this objective"** — NOT a hand-off to a multi-squad "G4-HEAVY" path, which never existed beyond log strings + unwinnable-reason text (`force_sizing.rs:206`, `war.rs:1115`, comments in `composition.rs`/`force_sizing.rs`). Carrying that pretense as a failover is the broken outlier the operator flagged. The implementation **deletes the G4-HEAVY framing** (those reasons/logs become "not winnable for one squad"); **this decision supersedes every "defer to G4-HEAVY" mention elsewhere in this ADR.** The higher-power response — **scale up the blob, field multiple coordinated squads, or boost** — is a separate **strategy-layer** decision (a future ADR, tracked follow-up), invoked deliberately for a high-value objective, NOT a composition-layer failover. The composition layer's job ends at "the best single-squad force, or `None`."
- **D11 — No fixed doctrine; HarassRemote scales too (operator 2026-06-27).** `HarassRemote` does NOT field a
  fixed solo — it emits a DYNAMIC anti-creep capability vector sized to the room's observed force
  (`ctx.enemy_force`) + a safety margin, **updating as defense is identified**, then assembles like any
  creep-clear doctrine. Its distinction is purely TACTICAL (deny, don't hold — `MultiLifetimeWave`,
  retreat-happy), not a sizing/template one. So EVERY doctrine is dynamic through the one driver — no
  fixed/dynamic fork (strengthens D4). The `solo_harasser` constructor is deleted with the rest of the catalog.
- **D12 — The WFV reset is not a phase blocker (operator 2026-06-27).** A `WORLD_FORMAT_VERSION` bump only gates
  an MMO deploy; the private server recovers in minutes. So the catalog deletion + bump need NOT be quarantined
  to a final isolated phase — delete the constructors/`BodyType` variants as soon as the assembler is the only
  producer (**fold P5 into P4**), bump WFV 18→19 then, and keep soaking on the private server. Operator
  go-ahead is still required before any MMO deploy.
- **D13 — Re-tune + re-eval after the assembler lands (operator 2026-06-27).** The assembler changes WHICH
  forces are fielded, so the position-utility weights (ADR 0019) and the tournament tuning must be
  RE-EVALUATED (a final phase, P6): re-run the tournament/exploitability tuning, re-sweep the weights, and
  confirm the assembler yields **winning-but-EFFICIENT** squads (smallest-favorable-Lanchester — the marginal
  fill stops at "requirement met", no over-spend). Sufficient test coverage at every phase, not just the end.
- **D14 — NO quad/duo/solo naming; size/shape are DERIVED, never named (operator 2026-06-27).** A "quad" /
  "duo" / "solo" anywhere in fielded-squad code is a **design smell** — it presumes a fixed member count, the
  exact thing the assembler dissolves. The implementation removes the named SHAPE vocabulary, not just the
  catalog constructors: (a) the ≈13 `SquadComposition` constructors go (D7); (b) `FormationShape::Box2x2`/`Line`
  and `military/formation.rs`'s hardcoded **2×2** quad overlay (`is_valid_quad_position`, `apply_quad_cost_overlay`)
  are replaced by a **footprint-driven** formation computed from the member count (a W×H box / line derived from
  `slots.len()`, valid for 1..=`MAX_SIZED_MEMBERS`) — the current 2×2 is silently wrong for a 5–8-member
  assembled force; (c) labels/logs describe the *capability mix* (`assembled_label`: "2×Healer 1×Dismantler"),
  never "quad". Sim/opponent-model test names (`screeps-combat-agent`) are cosmetic and out of scope. The
  formation-geometry generalization MAY be a tracked follow-up if it is larger than P4's core, but the named
  shapes must not survive into the assembler's output contract.
- **D16 — T2 is an EV-MAXIMIZING optimizer, tournament-tuned; NO presumed reference squad (operator 2026-06-27).**
  Supersedes the marginal-fill `assemble_force` + `force_ceiling`: both presumed a composition (the 3+5 ceiling /
  the 1:1 role↔dimension fill) — the same smell as the templates. Composition is a **multi-dimensional
  optimization** over body parts + creep count + energy/spawn availability; the best squad **maximizes EV**
  (`P(win)·target_value − cost`, `cost = w_energy·energy + w_creep·creeps`) with a **dynamic margin** so a
  changing/growing hostile force still loses. `optimize_composition` runs ONE parameterized search over creep
  splits × over-power factor (no reference squad — winnability = "a candidate clears the commit-EV threshold").
  The **four tunable knobs the operator chose** — cost weights, safety margins, member-energy split, commit
  threshold — live on `CompositionParams` and are **swept in the tournament** (P6 — D13 made literal). `force_ceiling`
  is DELETED; `emit_requirement` (T1, the requirement + margins) survives as the optimizer's win-target. **One
  search for now** (small-many vs few-big EMERGE from the tuned weights); **documented follow-up:** codify
  emergent spawning/composition strategies as explicit selectable strategies later if the single search proves
  too narrow or a better structure emerges (operator). The `assemble_force`/`force_ceiling` built in P3/P4.1 are
  the bridge this replaces.
- **D15 — ONE squad-generation path; no gen outside the driver (operator 2026-06-27).** After P4 there is
  exactly ONE place a fielded combat squad is born: the shared driver `emit_requirement → assemble_force`.
  The §1.5 audit's out-of-registry sites are unified: `war.rs:467` (the hardcoded defend-flag `duo_attack_heal`)
  routes through the defense doctrine sized to the room's threat; the `.unwrap_or_else(static)` fallbacks
  (`war.rs:375/556`, `sourcekeeperfarm.rs:403`) are deleted (D9/no-silent-static). A grep for `SquadComposition::`
  constructors outside the driver + its tests must come back EMPTY — that emptiness is the P4 acceptance check
  for unification. (Power-bank has no live squad-gen; its dead constructors are deleted, not migrated.)

---

## 8. Consequences

- **Positive:** one sizing pipeline; the Layer A/B/C defects are structurally eliminated; doctrines become
  trivial classifiers; the catalogs (≈13 constructors + ≈20 `BodyType` shapes) disappear; the size-granularity
  snap is gone; `is_sized()` + every silent fallback is deleted; new objectives are one classifier + (optionally)
  one shaping input.
- **Negative / risk:** the assembler is a new deterministic path that must reproduce the old calibration on the
  existing beds (P2/P3 golden-output + gate tests guard this); one WFV reset (operator-accepted); the bot wiring
  lags the eval (tracked). The marginal-fill tie-break must be frozen and tested (determinism).
- **Deferred (a STRATEGY-LAYER future ADR, not a composition failover — D10):** the higher-power response to an
  assembler `None` — **scale up the blob / field multiple coordinated squads / boost** — invoked deliberately for
  a high-value objective the single-squad assembler can't win; boosted-body sizing; the cross-goal body/blob EV
  auction (0020 R7-R8 / 0029 §8) that reads tempo; full cross-blob role re-allocation (R8 / 0020-S5). The
  composition layer terminates at "best single-squad force, or `None`"; escalation is the strategy layer's call.
