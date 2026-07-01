# Handoff brief — generalized force composition & sizing

**Audience:** the session authoring the broader ADR on squad composition + force sizing.
**Status:** input brief (findings + measured evidence + open decisions). Not an ADR itself — fold into your ADR.
**Provenance:** produced from a defended-breach investigation + a multi-agent design pass (ADR 0029 §10 #1).
Where a fact was code-verified this session it carries a `file:line`; refs marked *(verify)* came from the
design-pass agents and should be re-confirmed before you rely on them.

---

## 0. TL;DR

Combat squads are sized through a **per-doctrine hand-wired** mix of three different sizing maths and two
catalogs of hardcoded creep variants. The abstraction that's supposed to unify them (`ForceDoctrine` +
`is_sized()` + `SquadComposition::sized_for`) is leaky: `is_sized()` does **not** mean "dynamically sized,"
`sized_for` can only scale roles a hardcoded template **already** contains, and the capability model
(`RequiredForce`) is missing a whole dimension (anti-creep kill power). The result is silent under-fielding —
proven by a squad that can dismantle a structure but is scored as having **zero combat strength** the moment a
defender creep is present, so it retreats and never engages.

**The ask (operator):** generalize this across **all** doctrines/jobs/strategies; replace hardcoded creep
variants with **composition strategies and/or an auction**; retire old sub-optimal systems; land **one cohesive,
simple, testable pattern** (provable in the sim harness *and* the tournament). Don't fix only the one doctrine
we tripped over — fix the model so the others don't need fixing later.

---

## 1. The triggering problem + measured evidence

A formed, **moving** squad sized by the `SiegeBreach` doctrine (a `siege_quad` = WORK dismantlers + HEAL
healers) cannot kill a creep-defended structure core. Measured this session in `screeps-combat-eval`
(oracle-sized siege comp, FORMED under economy contention, then MOVING with breach tactics, member_energy 12_900):

| Defended fixture | Result |
|---|---|
| undefended core | **Killed** (~t65) |
| tower + rampart, **no defender creeps** | **Killed** (~t55) |
| same + **any `ForceSpec::Guard(n≥1)`** (melee + healer) | **Timeout, 0 damage** — never breaches |
| guarded core, but comp swapped to `quad_ranged` (anti-creep) | **Killed** (3 of 4 configs) |

The **pre-placed** path (eval `breaches()`/`SizingWins` → `siege_intents`) wins ~99% on the *same* defended beds
— it dismantles the structure and ignores creeps. So the gap is entirely in the **moving** engagement, and an
anti-creep comp closes it. The siege comp has no anti-creep weapon, so it parks.

---

## 2. Verified root causes (three stacked layers)

**Layer A — engagement assessment scores a dismantle-only squad as zero strength.**
`our_dps = Σ(member.melee_power + member.ranged_power)` — it **excludes `dismantle_power`**
(`screeps-combat-decision/src/kernel.rs:360`, and again `lib.rs:1021`, `lib.rs:1101`). So a WORK+HEAL siege reads
`our_dps == 0`. In `assess_engage` (`lib.rs:1013`), with a guard present you get `killable_dps==0 &&
unkillable_dps>0`, which **misses** the "can't-hurt-but-can't-be-hurt → hold" special case (that requires *both*
zero, ~`lib.rs:1055` *(verify)*) → `fighting_strength(0, ehp, n) == 0` (`lib.rs:987`) → Lanchester `balance =
-1000` → **retreat at t0**, even though the oracle says the engagement is winnable. `select_focus_target` /
`ev_target_order` (`lib.rs:264`, `lib.rs:339`) also net every creep to 0 when `our_dps==0`, so the squad never
even fixates the structure.

**Layer B — `sized_for` can only scale roles the template already has.**
`SquadComposition::sized_for` (`screeps-combat-decision/src/composition.rs:723`) iterates a fixed roles array
(`composition.rs:753-758`) and **skips any role whose `template_count(role)==0`** (`composition.rs:761`:
`if total == 0 || template_count(role) == 0 { continue }`). `siege_quad` has only Dismantler + Healer slots, so
even if the required force carried `ranged_parts > 0`, `sized_for` drops them — the template can never *gain* a
role. The hardcoded variant is a hard ceiling on capabilities, not just magnitudes.

**Layer C — `RequiredForce` has no anti-creep dimension.**
`RequiredForce` (`screeps-combat-decision/src/force_sizing.rs:285`) = `{ heal_parts, dismantle_parts,
ranged_parts, tough_parts }`. Its `ranged_parts` is anti-**structure** DPS for dismantle-immune cores
(`force_sizing.rs:290`), *not* "kill the blocking guard." The structure doctrines fold enemy creep DPS only into
the **heal** requirement (out-heal), never a **kill** requirement. So the capability vector itself can't express
"clear the defenders."

These three compound: the structure doctrine never asks for anti-creep (C), couldn't field it into a
`siege_quad` if it did (B), and the brain scores the resulting weaponless comp as zero-strength and retreats (A).

---

## 3. The structural confusion (this is what's "confusing us")

**`is_sized()` is a leaky flag — it means "uses the generic structure-`assess` path," NOT "is dynamically sized."**
Three of the four `is_sized()==false` doctrines are in fact dynamically sized via a custom `plan()`:

| Doctrine | file:line | `is_sized()` | Actually sized? | Sizing math | Hardcoded template (role-set is fixed) |
|---|---|---|---|---|---|
| `NpcCore` | `doctrine.rs:186` | `true` | dynamic magnitude | `assess` (structure) | `quad_ranged` |
| `SiegeBreach` | `doctrine.rs:205` | `true` | dynamic magnitude | `assess` (structure) | `siege_quad` ← **no anti-creep role** |
| `GatedPlayerRaid` | `doctrine.rs:266` | `true` | dynamic | `clear_force` (creep) | `quad_ranged` |
| `PlayerRaid` | `doctrine.rs:229` | **`false`** | **dynamic** | `clear_force` | `quad_ranged` |
| `GarrisonDefense` | `doctrine.rs:330` | **`false`** | **dynamic** (continuous) | `clear_force` | `quad_ranged` → `duo` floor |
| `SkSuppression` | `doctrine.rs:398` | **`false`** | **dynamic** | bespoke SK math | `duo_sk_farmer` |
| `HarassRemote` | `doctrine.rs:307` | **`false`** | **truly FIXED** | none | `solo_harasser` |

Only `HarassRemote` is genuinely unsized. Everything else is sized — but through **three divergent maths**
(`assess` for structures, `clear_force` for creeps with a square-law over-power margin, and a bespoke SK
out-heal+kill-window), each doctrine hand-picking one in its own `plan()`. `ForceDoctrine::plan` default vs
override is at `doctrine.rs:171`; the shared structure path `sized_plan` is `doctrine.rs:142`.

**Two catalogs of hardcoded creep variants:**
- **Role-set templates** — `SquadComposition` constructors: `solo_ranged` (`composition.rs:326`),
  `duo_attack_heal` (340), `duo_tank_heal` (360), `quad_ranged` (380), `quad_siege` (408), `duo_drain` (436),
  `solo_harasser` (456), `duo_melee_heal` (470), `duo_sk_farmer` (492), `siege_quad` (551), `solo_core_attacker`
  (579). (`siege_assault_quad` does **not** exist yet — the narrow fix would add it.)
- **Body templates** — the `BodyType` enum (`composition.rs:65`) has ~20 static shapes (`QuadMember`,
  `SiegeDismantler`, `Harasser`, `Drain`, `SkRangedAttacker`, boosted variants, …) dispatched in
  `body_definition` (`composition.rs:96`). Most are superseded by `BodyType::Sized(CombatBodySpec)` (the dynamic
  builder, `composition.rs:91`/`build_body:125`) but still exist as the template-fallback path.

The role vocabulary is `SquadRole` (`composition.rs:47`: Tank, Healer, RangedDPS, MeleeDPS, Dismantler, Hauler).

---

## 4. Generalization goals (operator's steer — the target end-state)

1. **One pipeline for all combat squad generation** (offense doctrines, defense, SK, jobs, strategies) — doctrine
   *selection* picks the objective; a *unified requirement* + a *composition assembler* produce the force. No
   per-doctrine bespoke sizing+template wiring.
2. **Retire hardcoded creep variants** — replace "pick a `SquadComposition` constructor, then `sized_for` scales
   its existing roles" with **composition strategies and/or an auction** that *assembles* the slot-set + `Sized`
   bodies from the requirement, within budget + the member cap. Delete or deprecate the static `BodyType`
   templates and the constructor catalog where possible.
3. **Doctrine selection must be optimal per objective** and pick the right composition — including the right
   *capabilities*, not just magnitudes.
4. **Clean up sub-optimal old systems** — the `is_sized()` flag, the three divergent maths, the bucket-era
   leftovers — collapse into one cohesive, simple pattern.
5. **Testable as one pattern** — provable in the eval sim (`SizingWins`/`OracleCalibration`/`CreepClearWins`) *and*
   the tournament/base-attack lens, end-to-end (formed → moved → resolved).

---

## 5. The narrow fix (use as increment 1, not the destination)

The design pass's top-scored proposal (42/50), "SiegeBreach capability-vector fusion," is a good *first step* and a
sanity-check of the model:
- When defenders are observed, size anti-creep `ranged_parts` via `clear_force` over `ctx.enemy_force`, and field a
  template (`siege_assault_quad`, new) that **has a `RangedDPS` slot** so `sized_for` keeps them → `our_dps>0` →
  the guard is killable → Lanchester balance favorable → existing `quad_ranged` moving success path executes
  (clear guard → `ev_target_order` empties → structure fallback → `breach_redirect` razes, `lib.rs:1408`).
- **Precedent:** `GatedPlayerRaid` (`doctrine.rs:266`) already does custom-`plan` + `clear_force` + reads
  `ctx.enemy_force`. The bot already populates `enemy_force` for the structural arm (`war.rs:1077`); the eval just
  stops hardcoding `enemy_force: None` (eval `validate.rs` ~`:200` *(verify)*). No `WORLD_FORMAT_VERSION` bump
  (reuses the 4-field vector).
- **Open risks it flagged (resolve in your ADR):**
  - The `select_focus_target` reorder is a **shared-kernel** change (every doctrine) → must be regression-proven
    against `CreepClearWins`; gate it (only when `ev_target_order` empty AND a hostile structure exists).
  - Making a mixed WORK+RANGED+HEAL template the **default** perturbs the *undefended* budget from pure-WORK
    (`capabilities()`-derived ceiling, `war.rs` ~`:1380` *(verify)*) → small calibration risk on the creep-free
    `SizingWins`/`OracleCalibration` beds.
  - **8-member cap:** a tower@100k out-heal demand (~800 hp/tick) can consume ~7 of 8 slots, so a heavily defended
    fixture may correctly **defer** (→ multi-squad G4-HEAVY) rather than reach `Killed`. The "defer to G4-HEAVY"
    hand-off is currently only a log string, not implemented.

**Why it's not the destination:** it's still per-doctrine hand-wiring (fuses two maths inside `SiegeBreach.plan`),
keeps the variant catalogs, and leaves `is_sized()` in place. Your ADR should make it fall out of the general model.

---

## 6. Design space for the generalization (for your ADR to decide)

A coherent target shape (synthesize / improve):

- **Unified requirement (capability vector).** Collapse `assess` + `clear_force` + SK into one
  requirement-emitter that outputs a capability vector keyed by *what the objective needs*, e.g.
  `{ structure_dismantle_dps, immune_structure_dps, anti_creep_kill_dps, out_heal_per_tick, tank_ehp,
  kill_in_window }`. Extend `RequiredForce` (Layer C) to carry an explicit anti-creep kill term distinct from the
  immune-structure ranged term. `EngagementContext` already carries `enemy_force` + `coordination` + `defense`
  (`doctrine.rs:82`) — the inputs exist.
- **Composition assembler OR auction (replaces template + `sized_for`).**
  - *Assembler:* map the capability vector → a set of role-slots + `Sized` bodies directly, filling binding
    constraints first, bounded by budget + the member cap. No starting template.
  - *Auction:* roles bid for member slots by marginal capability-per-energy; the auction fills the scarcest
    capability first until the requirement is met or the cap is hit (then defer). This is the operator's
    "composition strategies and/or auction" — pick one or make the strategy pluggable.
  - Either way, `sized_for`'s "can't add a role" limitation (Layer B) disappears because the role-set is *derived*,
    not pre-fixed.
- **Doctrine = pure classifier + objective intent.** Doctrines stop carrying a `template()` + `is_sized()`; they
  declare the objective + any objective-specific shaping, and the shared requirement+assembler does the rest.
  Retire `is_sized()`.
- **Keep the brain honest.** Fix Layer A so a comp's *actual* offensive capability (including dismantle vs an
  out-healed structure) feeds `our_dps`/`fighting_strength`, so a correctly-assembled force is never scored as
  zero-strength. (Even with anti-creep, you want dismantle-as-offense represented.)

---

## 7. Constraints & invariants (do not break)

- **Determinism is sacred.** The offline sim must stay bit-deterministic (memory: `sim-determinism-fence`,
  `sim_is_deterministic_over_rounds`). No result-affecting HashMap iteration; integer/ceil math over Vec-ordered
  inputs. Any new path needs its own run-twice-assert-equal test (the standing fence runs over the *hardcoded*
  `quad_ranged`, so it won't cover a new assembler path).
- **Serialization / `WORLD_FORMAT_VERSION`.** `SquadComposition`, `SquadSlot`, `BodyType` are
  `Serialize`/`Deserialize` (`composition.rs`) and persist in mission state. Retiring `BodyType` variants or
  changing the comp shape is a serialized-shape change → bump `WORLD_FORMAT_VERSION` (game_loop.rs) = one loud
  reset on deploy. `BodyType::Sized` was appended last specifically to keep discriminants stable — preserve that
  discipline or accept the reset.
- **Calibration gates must stay live + discriminating.** `OracleCalibration` (FP ≤ 0.010, FN ≤ 0.200) and
  `SizingWins` run on creep-free beds today; `CreepClearWins` on creep beds. Don't silently change what they grade.
- **8-member squad cap** (`MAX_SIZED_MEMBERS`, `composition.rs`) — a bigger force is the multi-squad G4-HEAVY path
  (not implemented). "Smallest favorable-Lanchester force" + per-tick-optimal (no hysteresis) are standing
  principles (memory: `prefer-per-tick-optimal-over-hysteresis`).
- **Bot/eval parity** — the bot (`war.rs`) and eval must select+size through the same decision-crate code (ADR
  0026 §9). Keep the parity seam.

---

## 8. Test / proof strategy

- **Sim seam-closer already scaffolded this session:** `run_defended_lifecycle`
  (`screeps-combat-eval/src/harness/lifecycle.rs:257`) forms an oracle-sized force under economy contention, then
  drives it MOVING vs a rampart+tower+`Guard` core. Test `oracle_sized_force_forms_and_kills_a_defended_core`
  (~`lifecycle.rs:466`) asserts `Killed` and is currently **`#[ignore]`d as KNOWN-FAILING** (with a comment telling
  agents not to soften it) — un-ignore it when the model lands. `defended_lifecycle_is_deterministic` passes.
  Helpers `derive_profile`/`siege_ceiling`/`siege_doctrine_plan` were made `pub(crate)` for this.
- **Add a graded regime sweep** (build it — the `diag_defended_sweep` referenced earlier was scratch and removed):
  tower-only / rampart-only / melee Guard / ranged Skirmishers / combined / shielded-guard, each FORMED+MOVING,
  asserting `Killed`-when-winnable and clean-defer-when-not.
- **Discriminator** (ADR 0029 §10 #1): for each winnable regime assert the SAME assembled comp also breaches
  **pre-placed** via `breaches()`/`siege_intents`. Pre-placed kills but moving misses → form/travel/brain bug;
  both miss → the assembler under-sized. That bisection is the whole point of the lifecycle test.
- **Tournament:** grade the assembler through the existing base-attack lens (`assault_score`,
  `realistic_bases()`) and the self-play tournament so the generalized force is proven across the regimes a real
  attacker meets, not just unit fixtures.

---

## 9. Concrete file:line map

**Decision crate — sizing/doctrine (`screeps-combat-decision/src/`):**
- `doctrine.rs` — `ForceDoctrine` trait (158), `is_sized` (168), default `plan` (171), `sized_plan` (142),
  `ForcePlan` (98) + `::fixed` (122), `EngagementContext` (82), `EnemyForce` (68), `DoctrineObjective` (46),
  doctrines (186/205/229/266/307/330/398), `default_doctrines` (377), `sk_doctrines` (438).
- `force_sizing.rs` — `RequiredForce` (285) + `from_assessment` (304), `assess`/`clear_force` (grep),
  `AssaultMode` (74), `ForceAssessment` (83), `DefenseProfile`.
- `composition.rs` — `SquadComposition` (303), `SquadRole` (47), `BodyType` (65) + `body_definition` (96) +
  `build_body` (125), `force_budget` (701), `sized_for` (723) + the role-skip (**761**), `capabilities`,
  variant constructors (326–579), `PREFERRED_MEMBER_ENERGY` (43), `MAX_SIZED_MEMBERS`.
- `kernel.rs` — `our_dps` excl. dismantle (**360**), `FOCUS_STRUCT_BONUS` (265).
- `lib.rs` — `select_focus_target` (264), `ev_target_order` (339), `fighting_strength` (987),
  `assess_engage` (1013), `our_dps` (1021/1101), `breach_redirect` (1408).

**Bot (`screeps-ibex/src/`):**
- `operations/war.rs` — `enemy_force` populated (367, 550, **1077**); the structural-arm budget ~1380 *(verify)*.
- `military/squad_manager.rs` — `queue_slot_spawn` (524) (the live spawn path that builds slot bodies).
- `missions/sourcekeeperfarm.rs` — SK `enemy_force` (396).

**Eval (`screeps-combat-eval/src/harness/`):**
- `validate.rs` — `siege_intents` (grep), `breaches` (284), `SizingWins` (626), `OracleCalibration` (94),
  `derive_profile` (153), `siege_doctrine_plan` (195) [hardcodes `enemy_force: None` ~200 *(verify)*],
  `siege_ceiling` (841), `run_managed_assault_with` (416), `choose_fielded_comp` (307), `place_squad` (213),
  `run_until_for` (396), `clear_outcome_at` (674), `CreepClearWins` (698), `enemy_force_of` (653).
- `lifecycle.rs` — `run_lifecycle` (201), `run_defended_lifecycle` (257), the `#[ignore]`d kill test (~466).
- `generate.rs` — `assemble_single_room` (256), `ForceSpec` (210), `place_force` (222), `breach_geometry` (36).

**ADRs:** `docs/design/0026-combat-strategy-selection.md` (§9 doctrine/force-sizing model, the L-rungs),
`0029-generalized-force-composition.md` (§10 #1 = this task), `0028-lifecycle-harness.md`,
`0020-ev-adaptive-blob-combat.md` (§12 force-sizing), `0019-combat-position-selection.md` (§8 scored search).

---

## 10. Open decisions for the ADR

- **Strategy vs auction** — does composition come from named pluggable strategies, an auction, or both (strategy
  selects, auction fills)? The operator said "and/or."
- **Delete vs deprecate the variant catalogs** — retire `SquadComposition` constructors + static `BodyType`
  templates outright (accept the `WORLD_FORMAT_VERSION` reset) or keep as seed/fallback during migration?
- **Requirement vector shape** — exact capability dimensions; do you extend `RequiredForce` or introduce a new
  capability type that the assembler/auction consumes?
- **`enemy_force` as the single state channel** — read defenders via the existing `ctx.enemy_force` everywhere
  (recommended; bot already populates it) rather than new `DefenseProfile` fields.
- **Scope of first landing** — eval-only/offline capability first (the bot lacks a `DismantleStructure` producer +
  a real G4-HEAVY defer target today), with bot wiring as a tracked follow-up?
- **Layer A (brain) fix** — fold dismantle-as-offense into `our_dps`/`fighting_strength`, or rely on always
  assembling an anti-creep weapon so the question never arises?

---

## 11. What was changed in code this session (already on the working tree, uncommitted)

- `screeps-combat-eval/src/harness/lifecycle.rs`: added `run_defended_lifecycle` + `defended_forming` test helper
  + the `#[ignore]`d KNOWN-FAILING `oracle_sized_force_forms_and_kills_a_defended_core` + a passing determinism
  test. (Scratch `diag_defended_sweep` was removed.)
- `screeps-combat-eval/src/harness/validate.rs`: `derive_profile`, `siege_ceiling`, `siege_doctrine_plan` made
  `pub(crate)` so the lifecycle harness can size via the same oracle path `SizingWins` uses.
- Nothing committed. `cargo test -p screeps-combat-eval` is green (the known-failing test is ignored, not deleted).

The full multi-agent design-pass output (root-cause trace, 4 design proposals, judge scores, 3 adversarial
refutations, synthesis with an 11-step migration + risk table) is in the run's task-output file under the session
`tasks/` dir — re-run or ask the originating session if you want the raw proposals.
