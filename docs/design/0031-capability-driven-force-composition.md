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

### 2.2 T2 — the assembler (`assemble_force`)

One function replaces `template() + sized_for`:

```rust
// composition.rs
pub fn assemble_force(req: &RequiredForce, member_energy: u32) -> Option<SquadComposition>
```

It is a **marginal-fill auction over a fixed, Vec-ordered set of weapon roles** — the body is *sized per pick*,
there is no body catalog. Algorithm (bit-deterministic, integer/ceil over Vec-ordered inputs, no HashMap):

1. **Min-viable ROLE-SET floor** (a constraint, not a scalar count): if any kill demand > 0 → ≥1 fighter slot of
   each demanded weapon; if `heal_parts > 0` → ≥1 Healer. This is the role-set viability floor (never an
   under-sized "healing required but no healer," never "defenders present but no anti-creep"). It is **NOT a
   template-count floor** — `sized_for`'s `.max(template_count)` (composition.rs:770) is deleted; there is no
   template, so Layer B cannot recur. If a required role can't field even one member at this energy → `None`.
2. **Probe** the per-member single-role part cap via the **real** builder (the existing `cap_for` reverse-probe
   over `MAX_SINGLE_ROLE_PARTS` using `build_combat_body`, composition.rs:741–747, lifted out of `sized_for`) at
   `min(member_energy, PREFERRED_MEMBER_ENERGY)` — so the cap can never drift from what actually spawns.
3. **Marginal fill** — maintain a remaining-need ledger `r` (the capability vector, mutable). Repeatedly: pick
   the **scarcest** unmet dimension (largest unmet fraction `r_i / req_i`; ties → fixed dimension order
   `[heal, dismantle, immune_struct, anti_creep, tough]`), then among the roles that supply it add the member
   with the highest **capability-per-energy** (`cap_of(role, dim) / cost(role)`; ties → role enum order). Recompute
   `r` from what is placed each iteration. Stop when `all(r ≤ 0)`, or when `members > MAX_SIZED_MEMBERS`
   (composition.rs:26 = 8) → return **`None`** (an honest defer to the unimplemented multi-squad G4-HEAVY, §3 P5).
   *The "scarcest capability per energy first" rule IS the auction* — a tower@100k out-heal demand floods Healer
   picks until ~7/8 slots, then `None` (correct deferral); a Guard-defended core interleaves AntiStructure +
   AntiCreep + Healer until all three needs clear.
4. **Re-balance** each role's even share over its grown count (`per_member = total.div_ceil(count)`, ceil so the
   force never under-sizes), then build each slot as `BodyType::Sized(CombatBodySpec)` via the existing builder.
   Role from the dominant part (HEAL→Healer, WORK→Dismantler, RANGED→RangedDPS, ATTACK→MeleeDPS, TOUGH→Tank;
   closing the MeleeDPS gap at composition.rs:733). Formation from member count + roles (the existing
   `FormationShape` heuristics, not a per-template constant); `retreat_threshold` from objective class. Re-confirm
   every spec builds at `member_energy` (else `None` = defer).

**Continuous count + role-mix; the granularity gap is subsumed.** One mandatory fighter + one mandatory healer =
a duo; a heavier need adds members one at a time. 1,2,3,…,8 are all reachable. The floor is a **role-set** (≥1 of
each required role), never a count — so the solo↔quad snap (composition.rs:770) and Layer-B (can't-add-a-role)
are **structurally** impossible.

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
(defer-on-unwinnable) is the only per-doctrine policy bit, and even the one truly-fixed doctrine (`HarassRemote`)
flows through `assemble_force` via a tiny fixed capability vector — so there is one path, not a fixed/dynamic
fork.

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
- kernel.rs:360, lib.rs:1021, lib.rs:1101: `our_dps += dismantle_power`. Gate the focus reorder (T3b).
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

### Phase 2 — `emit_requirement` (T1): collapse the three maths (no WFV)
- Implement T1 as a pure function composing `assess` + `clear_force` + the SK kite terms (the producers become
  thin callers, differing only in their kill-rate model). Route the eval's `siege_doctrine_plan` and the
  SK/defense/raid `plan()`s through it — still emitting into the OLD `sized_for` (the assembler is not yet in the
  loop, isolating the emitter).
- **Deletes:** the per-doctrine sizing forks' *math* moves into the emitter (the `plan()` bodies still call it).
- **Tests:** a **run-twice-equal determinism test over `emit_requirement`** (the standing fence only covers the
  hardcoded `quad_ranged`); a golden-output byte-stability test over `realistic_bases()` (generate.rs:414) — the
  emitter must reproduce the old per-doctrine outputs on the existing beds.

### Phase 3 — `assemble_force` (T2): replace `sized_for` (no WFV)
- Implement T2 alongside `sized_for` (parity bridge). Route `run_defended_lifecycle` (lifecycle.rs:257) through
  emit → assemble. **Un-ignore `oracle_sized_force_forms_and_kills_a_defended_core`** (lifecycle.rs:470) — it
  must now pass on its own (the P3 re-soak the brief names).
- **Deletes:** nothing yet (`sized_for` retained as the bridge).
- **Tests:**
  - **Determinism:** run-twice-equal over `assemble_force` (its own fence — required by the brief).
  - **Graded regime sweep** (build it; the removed `diag_defended_sweep` was scratch): tower-only / rampart-only
    / melee `Guard` / ranged `Skirmishers` / combined / shielded-guard — each FORMED + MOVING via
    `run_defended_lifecycle` (using `assemble_single_room`/`ForceSpec`, generate.rs:256/211), asserting
    `Killed`-when-winnable and clean-defer-when-not.
  - **Pre-placed-vs-moving discriminator** for each winnable regime.
  - **OFFENSE continuous-sizing test:** sweep `RequiredForce` trivial→large; assert member count is **monotonic
    non-decreasing**, **starts at the role-mix viability floor**, and **never snaps 1→4** (the Layer-B regression
    pin; contrast composition.rs:770).
  - `OracleCalibration`/`SizingWins`/`CreepClearWins` green + discriminating; FP/FN gates held.

### Phase 4 — route all doctrines through emit+assemble; retire `is_sized` + silent fallbacks (no WFV)
- Replace each doctrine's `template()`/`is_sized()`/custom `plan()` with the shared driver (T3a). Delete the
  `.unwrap_or_else(static_template)` fallbacks (PlayerRaid:253, GarrisonDefense:363, SkSuppression:420,
  NpcCore/SiegeBreach via `sized_plan:150`). **Deletes:** `is_sized()` (doctrine.rs:168), `sized_plan`
  (142), `ForcePlan::fixed` (122), the three silent fallbacks, the per-doctrine `plan()` bodies.
- Bot wiring (tracked follow-up): war.rs:1099–1157 collapses to the driver keyed on `honor_verdict`; add the
  `DismantleStructure` producer (war.rs:1030). Bot/eval parity (ADR 0026 §9) preserved — same decision-crate code.
- **Tests:** the no-silent-static invariant (ADR 0030 D19) — an unwinnable/unaffordable objective returns
  `composition: None`, never a static template; all gates green; `realistic_kernel_tournament` (tournament.rs:764)
  + `assault_score` (validate.rs:466, route `choose_fielded_comp` through the assembler).

### Phase 5 — DELETE the catalogs + dead `BodyType` (the WFV reset)
- **Deletes:** the `SquadComposition` constructors (composition.rs:326–590) and the static `BodyType` variants
  (composition.rs:66–87) + their `body_definition` arms (98–117) + the `bodies::*_body` functions they call —
  leaving `BodyType::Sized` (+ Hauler/non-combat for power-bank haul) as the only variants. Update
  `siege_ceiling` (validate.rs:841) + `defended_forming` (lifecycle.rs:447) + any test naming a constructor to
  build via `assemble_force`. Delete `sized_for` (composition.rs:723).
- **WFV bump checklist:** (1) `WORLD_FORMAT_VERSION` 18 → 19 (game_loop.rs:652); (2) re-prove the bump is required
  via `git diff` of the serialized-shape types (`SquadComposition`, `SquadSlot`, `BodyType`); (3) confirm
  `serialized_data` framing (game_loop.rs:490) + the decode fingerprint (game_loop.rs:717) round-trip a fresh
  world; (4) operator go-ahead before any MMO deploy.
- **Tests:** full suite green; all three determinism fences (assembler + emitter +
  `sim_is_deterministic_over_rounds`) green; tournament + base-attack lens unchanged in ranking.

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

---

## 8. Consequences

- **Positive:** one sizing pipeline; the Layer A/B/C defects are structurally eliminated; doctrines become
  trivial classifiers; the catalogs (≈13 constructors + ≈20 `BodyType` shapes) disappear; the size-granularity
  snap is gone; `is_sized()` + every silent fallback is deleted; new objectives are one classifier + (optionally)
  one shaping input.
- **Negative / risk:** the assembler is a new deterministic path that must reproduce the old calibration on the
  existing beds (P2/P3 golden-output + gate tests guard this); one WFV reset (operator-accepted); the bot wiring
  lags the eval (tracked). The marginal-fill tie-break must be frozen and tested (determinism).
- **Deferred:** multi-squad G4-HEAVY (the >8-member `None` target); boosted-body sizing; the cross-goal body/blob
  EV auction (0020 R7-R8 / 0029 §8) that reads tempo; full cross-blob role re-allocation (R8 / 0020-S5).
