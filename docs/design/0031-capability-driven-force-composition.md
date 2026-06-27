# ADR 0031 — Capability-driven force composition

- **Status:** Accepted
- **Date:** 2026-06-27
- **One line:** Consolidates ADR 0026 §9 / 0029 / 0030 and **supersedes their `template() + sized_for` sizing mechanism**, replacing it with a capability-vector emitter, a deterministic role-distribution assembler (bridging to an EV optimizer), and a pure-classifier doctrine driving one squad-generation path.

---

## 1. Context

Combat squads are produced by **one** pipeline: a `ForceDoctrine` classifier selects an objective, an oracle decides whether it is winnable, and a builder turns that into a fielded squad. Before this ADR that builder was three divergent sizing maths (`assess` for structures, `clear_force` for creeps, a bespoke SK calc) each scaling a **hardcoded template** through `SquadComposition::sized_for`, drawing bodies from **two static catalogs** (≈13 `SquadComposition` role-set constructors + ~20 static `BodyType` shapes). A correctly-classified siege under-fielded against a defended core.

**The measured failure.** In `screeps-combat-eval`, an oracle-sized siege comp (`siege_quad` = WORK+HEAL), formed under economy contention then MOVING with breach tactics vs a `Guard`-defended core, timed out at **0 damage**. Swapping to an anti-creep ranged comp **killed** the core (3 of 4 configs); the pre-placed path won ~99% on the same beds. The gap was entirely in the moving engagement, and an anti-creep weapon closed it. Three stacked defects compounded:

- **Layer A — the brain scores dismantle as zero offense.** `our_dps` summed melee + ranged power but excluded `dismantle_power` (`screeps-combat-decision/src/kernel.rs:360`, `lib.rs:1021`, `lib.rs:1101`), even though the data exists on `EvMember` (`kernel.rs:236`). A WORK+HEAL siege read `fighting_strength == 0` → Lanchester `balance = -1000` → retreat at t0 even when winnable.
- **Layer B — `sized_for` could not add a role a template lacked, and snapped the count.** `SquadComposition::sized_for` skipped any role absent from the template and floored per-role count at the template slot count (`.max(template_count)`). A `siege_quad` (Dismantler+Healer) could never gain a `RangedDPS` slot; a single-slot template snapped solo(1)↔quad(≥4) with nothing between.
- **Layer C — `RequiredForce` had no anti-CREEP kill dimension.** Its `ranged_parts` was anti-STRUCTURE DPS for an immune core, not "kill the blocking guard." Structure doctrines folded enemy creep DPS only into the HEAL requirement, never a KILL requirement.

Two structural smells fed the failure: the **`is_sized()` lie** (it meant "use the generic `assess` path," not "is dynamically sized," and the false-returners hid `.unwrap_or_else(static_template)` silent fallbacks), and the **two hardcoded catalogs** (mostly superseded by `BodyType::Sized(CombatBodySpec)`, which can already emit any mixed body). Templates, catalogs, and `is_sized` were the wrong primitives: they presume a fixed member count and role set, so any requirement they did not anticipate (a new role, an in-between count, a second weapon) was structurally unreachable.

---

## 2. Decision — the unified architecture

All composition logic lives in `screeps-combat-decision` (the decision crate, not the bot), pure over `screeps-game-api` value types and engine constants with no `game::*` access, so the bot, sim, and eval field and size the **same** bodies through one implementation (`composition.rs:6-9`, `bodies.rs:1-3`). The pipeline is: classify → emit a capability vector → assemble a composition → gate on winnability + ROI → field.

### 2(a) The capability vector — `RequiredForce`

`RequiredForce` (`force_sizing.rs:292-312`) is the capability→PARTS vector, the inverse of a composition's `capabilities()`. Fields: `heal_parts` (out-heal the position), `dismantle_parts` (WORK to breach a dismantle-able structure), `immune_struct_parts` (RANGED for the same structure-DPS vs a dismantle-immune target — core/SK), `anti_creep_parts` (RANGED/ATTACK to kill blocking defender creeps — **kept SEPARATE from `immune_struct_parts`, summed not max-ed**, so a siege can do both at once), `tough_parts` (EHP, v1=0). It is **NOT `Serialize`** — a transient per-tick vector, never persisted, so it costs no `WORLD_FORMAT_VERSION` bump. Methods: `from_assessment` (`:317`), `as_solo_spec` (`:336`), `scaled` (`:350`, ceil-scale all non-zero parts, clamped to a no-op below 1.0 — the importance over-invest knob).

### 2(b) The requirement emitter — `emit_requirement` (T1)

`emit_requirement(objective, defense, enemy_force, budget, coordination, importance) -> (ForceAssessment, RequiredForce)` (`doctrine.rs:137-206`) is the ONE place that derives the capability vector + verdict, folding the three formerly-divergent maths. Match arms:

- **KillImmuneStructure | DismantleStructure** (`:146-165`): runs `assess(defense, budget)`; if not winnable, returns default. Else `RequiredForce::from_assessment(&a).scaled(importance_margin(importance))`. **Weapon selection (D14):** `from_assessment` sets both `dismantle_parts` (WORK) and `immune_struct_parts` (RANGED); `DismantleStructure` zeroes `immune_struct_parts` (WORK razes), `KillImmuneStructure` zeroes `dismantle_parts` (RANGED for the immune core). Then `overlay_anti_creep`.
- **ClearCreeps | RaidCreeps** (`:170-175`): `clear_force(...)` square-law sizing; `ClearCreeps` passes `hits=0` (the DPS race binds), `RaidCreeps` threads `enemy.hits` (kill-in-window for HP-rich rooms). No importance scaling (out-powering binds, not importance).
- **Suppress** (`:179-195`): NOT `clear_force` (kited duo, no square-law). Sizes heal from `keeper.dps × HOLD_MARGIN` and anti-creep from `keeper.hits` over `SK_KEEPER_KILL_TICKS=34`; synthesizes a winnable assessment.
- **Harass** (`:200-204`): D11 dynamic anti-creep, same `clear_force` sizing as a creep-clear; the deny-don't-hold distinction is purely tactical.

The **anti-creep overlay** (`overlay_anti_creep`, `:215-230`, Layer C): when defenders are observed (`dps > 0`), runs `clear_force` over `enemy_force` and, if winnable, sets `anti_creep_parts` and raises `heal_parts` — so a guarded structure gets both a structure weapon AND an anti-creep weapon. **INERT with no defenders** (creep-free calibration beds stay unperturbed). The emitter is a pure fold over Vec-ordered inputs (determinism fence `emit_requirement_is_deterministic_over_objectives`, `:678`).

### 2(c) The composition model — all BUILT

**BUILT — `assemble_force` (the deterministic role-distribution builder; the per-candidate body builder under the optimizer).** `assemble_force(&RequiredForce, member_energy) -> Option<SquadComposition>` (`composition.rs:359-412`) turns the vector directly into a fielded composition, no template, no catalog. It is no longer a top-level producer on its own — `optimize_composition` (below) is the composition decision and calls `assemble_force` to build each EV candidate from the per-rung-scaled requirement:
1. `probe_energy = member_energy.min(PREFERRED_MEMBER_ENERGY)` (`:362`) — split a force into more, smaller, bankable members rather than one un-spawnable ~5000e blob.
2. Frozen Vec-ordered demand list `[(Healer, heal), (Dismantler, dismantle), (RangedDPS, immune_struct + anti_creep), (Tank, tough)]` (`:366-371`) — RANGED carries both immune-structure DPS and anti-creep kill (same part, additive demand).
3. Per demanded role: `cap = single_role_cap(role, probe_energy)`; if `cap == 0` → **None** (terminal defer). `count = total.div_ceil(cap).max(1)`; `per_member = total.div_ceil(count)` (ceil so Σ ≥ total, never under-sizes).
4. **Role-set floor:** ≥1 member per demanded role, NEVER a template count — so the Layer-B "can't add a missing role" gap and the solo↔quad snap are structurally impossible; all of `1..=MAX_SIZED_MEMBERS` are reachable and monotonic (`:338-353`).
5. Returns **None** (terminal defer, D10) on empty requirement or `slots.len() > MAX_SIZED_MEMBERS`.
6. The frozen demand order is both the slot order and the determinism tie-break — a bit-deterministic integer/ceil fold, no HashMap.

This is the marginal-capability-per-energy fill specialized to the current 1:1 role↔dimension map; a future overlapping dimension would generalize to the full scarcest-dimension auction (the frozen order being its tie-break).

**BUILT — `optimize_composition` (the EV-maximizing optimizer, D16/D17).** The top-level composition decision (`composition.rs:513-594`). `force_ceiling`'s presumed 3+5 budget is **DELETED** as the producer — the EV/commit decision is **per-candidate**, scored from each candidate's OWN `capabilities()`, with no reference squad. The optimizer treats composition as a multi-dimensional optimization that **maximizes expected value**:

> `EV(C) = P(win | C) · target_value − cost(C)`, with `cost = w_energy·energy + w_creep·creeps` and `P(win) = win_probability(heal, incoming) · win_probability(deliverable_struct_dps·window, required_kill)`. The `dynamic_margin` inflates the observed hostile force (incoming DPS + enemy hits) so a growing threat still loses.

`optimize_composition(objective, defense, enemy, target_value, onsite_window, coordination, importance, honor_verdict, params) -> Option<SquadComposition>` runs ONE parameterized, bit-deterministic search over the **over-power ladder** (`OVER_POWER_LADDER = [1.0, 1.25, 1.5, 2.0]`) × the **TOUGH ladder** (`TOUGH_LADDER = [0.0, 0.1, 0.2]`), building each candidate by scaling `emit_requirement`'s (per-objective weapon-mixed) requirement by the rung and adding `ceil(t·fighter_parts)` TOUGH, then calling `assemble_force` — so the creep-split (n_fighters × n_healers, `1..=MAX_SIZED_MEMBERS`) is *derived* by `assemble_force`'s role-set fill, not separately enumerated. It commits the max-EV candidate iff (`honor_verdict` ⇒ `EV > commit_ev_threshold`), else `None` (the honest unwinnable defer, D10); an always-field doctrine commits the max-EV candidate regardless (deterministic tie-break: max EV, then lowest k, then lowest tough, then fewest members). `CompositionParams` (NOT `Serialize`) carries the tournament-tunable knobs. `emit_requirement` (T1) survives as the optimizer's per-rung requirement source; `win_probability` (`force_sizing.rs:366`) survives as its probability model. The seed constants `HOLD_MARGIN` (`:27`), `COORDINATED_DPS_MARGIN` (`:34`), `PREFERRED_MEMBER_ENERGY` (`composition.rs:43`) are threaded through `CompositionParams` (`hold_margin` / `over_power_margin` / `member_energy`) and are now swept knobs (`CompositionParams::default` reproduces them so the swap is behavior-preserving for the calibration gates). Small-many-vs-few-big, over-power, and armor all **emerge** from the search. **Tuning result (ADR 0031b):** the Tier-1 count × margin sweep across all four bed regimes confirmed the seeds are Pareto-optimal across regimes and KEPT (no `default()` change). Weapon `archetype` is NOT yet an EV-search dimension — it is still selected upstream by the objective (`optimizer_ceiling_budget`'s fighter weapon); promoting it into the search is the Tier-2 follow-up (D17 / 0031a §2B).

**RESIDUAL — `optimizer_ceiling_budget` (the renamed 3+5, used only for the requirement-assess).** `force_ceiling`'s deleted presumed budget survives in ONE narrow role: `optimizer_ceiling_budget(objective, member_energy, onsite_window)` (`composition.rs:602-624`) reproduces the IDENTICAL 3-fighter + 5-healer ceiling math purely as the BUDGET `emit_requirement` assesses *winnability* against (so the structure/clear arms' `winnable` verdict stays conservative). It does NOT shape any fielded candidate — the per-candidate EV/commit decision is budget-free (each candidate is scored from its own `capabilities()`, §2c above). The verdict it produces is not even the gate (the EV commit is, D16); it only keeps `from_assessment` conservative. **A budget-free `emit_requirement` is the documented follow-up to fully retire it** (§5 Deferred) — once the structure/clear arms size from the per-candidate parts directly, the last presumed-shape constant in the pipeline is gone.

**The research-grounded knob set (ADR 0031a) + the tuning result (ADR 0031b).** A four-bot survey (Overmind, The-International, TooAngel, community meta — `docs/design/0031a-force-composition-tunable-params.md`) confirmed the count-axis knobs (our `hold_margin 1.3`/`over_power 1.5` seeds sit dead-center of the field's 1.2–1.5 bracket) and surfaced the structural gaps our count-only search lacks — the **body/archetype axis**:
- **`archetype` (weapon select) — the biggest gap + the original failure.** Weapon (RangedBlob / MeleeAttack / WorkDismantle / derived Drainer) becomes a **tuned EV-search dimension**, no longer fixed upstream by `doctrine.fighter_role`. The DOCTRINE constrains the *feasible* archetypes for the objective (an immune core ⇒ only RANGED; a dismantle-able ring ⇒ WORK, or RANGED when creep-defended; creep-clear ⇒ RANGED/MELEE) and the EV search picks the best within that set — so `EV(C)` itself rejects a weapon mismatch (the measured WORK-siege-vs-guard = 0 damage). `fighter_role` becomes a feasible-set constraint, not a fixed pick.
- **`tough_parts` / `tough_fraction` (EHP armor)** — currently hardwired 0; ~10–12% of a body unboosted (Overmind 1 TOUGH:3 dmg:4 MOVE), required to survive a towered engage, coupled to heal (broken-TOUGH/tick must be refillable).
- **`commit_mode` (Siege vs Drain)** — leverage the EXISTING `AssaultMode::Drain` (`force_sizing.rs`): unboosted v1 cannot out-heal towers point-blank (~50 HEAL parts for ONE tower), so drain (tank + heal at tower-falloff range, out-spend the tower refill) is the only viable path vs multi-tower rooms.
- **Deferred to Tier-3 (0031a §4):** `engage_range`/`kite_range` (stand-off → tower-falloff DPS), within-member `attack_to_heal_mix` (~0.75), `reengage_threshold` hysteresis, and the v2 `boost_tier` axis. The full tiered sweep plan + grounded ranges live in 0031a.

### 2(d) The doctrine — pure classifier + `plan_engagement` (D15)

`ForceDoctrine: Sync` (`doctrine.rs:283-299`) declares ONLY classifier knobs — `name`, `applies` (the activator), `fighter_role` (Dismantler vs RangedDPS, selecting the ceiling weapon), `honor_verdict` (GATE/defer vs ALWAYS-FIELD), `retreat_threshold`. `template()`, `is_sized()`, per-doctrine `plan()`, and the catalogs are GONE.

`plan_engagement(doctrine, ctx, budget) -> ForcePlan` (`doctrine.rs:252-277`) is **the ONE path every fielded squad is born through**:
1. Resolve budget — for everything but Suppress, derive `force_ceiling(member_energy, fighter_role()).force_budget(..)` if the caller passed none (`:255-258`).
2. `emit_requirement(...)` → (assessment, required).
3. GATE: `honor_verdict() && !winnable` → `ForcePlan::skip` (gated defer, D10).
4. ALWAYS-FIELD floor + scale: a `!honor_verdict()` doctrine raises `required` to at least `default_floor_force()` (`RequiredForce { heal_parts: 4, anti_creep_parts: 4 }`, `:241`) — a max, so it scales UP with threat, never below floor, never a hardcoded template.
5. `assemble_force(&required, member_energy)`, stamp `retreat_threshold`.

The **seven doctrines** across three priority-ordered registries: OFFENSE `default_doctrines()` = [NpcCore, SiegeBreach, PlayerRaid, GatedPlayerRaid (ADR 0029 §7/D7), HarassRemote]; `sk_doctrines()` = [SkSuppression]; `defense_doctrines()` = [GarrisonDefense] (separate so defender selection is distinct from offense ClearCreeps). `decide_doctrine` returns the first activator that fires. Gated (`honor_verdict=true`): NpcCore, SiegeBreach, GatedPlayerRaid. Always-field (`false`): PlayerRaid, HarassRemote, GarrisonDefense, SkSuppression. GarrisonDefense fields a continuous blob from `clear_force` — member count emerges, so the historic W9N8 1↔2 defense flap is structurally impossible (`doctrine.rs:413-431`).

Every force-producing site routes through `decide_doctrine(...).and_then(|d| plan_engagement(d, &ctx, None).composition)` with no template fallback (D15): owned-room defense (`war.rs:387`), operator defend-flags (`war.rs:486`), remote-invader defense (`war.rs:585`), SK suppression (`sourcekeeperfarm.rs:402`), and the offense path (`war.rs:1140`). The bot feeds budgets via `best_force_budget(fighter, ...)` (`war.rs:1383`), which picks the home yielding the most on-site time and supplies its energy so the affordability check and the actual spawn agree.

### 2(e) The brain Layer-A fix

Dismantle counts as strength: `assess_engage`'s `our_strength` adds `dismantle_power`, **gated on a hostile structure being present** (the P0a correction — adding dismantle to `our_dps` everywhere mis-scores it as anti-creep in creep-killability, so the fix is scoped to the structure-engagement strength only; CreepClearWins-safe). A WORK+HEAL siege now reads positive strength and engages instead of retreating at t0.

---

## 3. Invariants

- **Bit-determinism.** No result-affecting HashMap; integer/ceil folds over Vec-ordered inputs; dedicated run-twice tests for the emitter and (when built) the optimizer; `sim_is_deterministic_over_rounds` green every phase.
- **8-member cap.** `MAX_SIZED_MEMBERS = 8` (`composition.rs:25`) is the hard `defer→None` boundary; beyond it is a strategy-layer (multi-squad) decision, not a composition failover.
- **Winning-but-efficient.** Size to the smallest-favorable-Lanchester force, per-tick-optimal, no hysteresis/anti-flap.
- **Bot/eval parity.** Both size through the same decision-crate driver (`plan_engagement` → `emit_requirement` → `assemble_force`); the eval's `siege_doctrine_plan` (`screeps-combat-eval/src/harness/validate.rs:195`) runs the identical selection + sizing path, with no divergent inline `assess` + `sized_for`.
- **No silent static.** A defer is an explicit `None`; there is no hidden constructor fallback anywhere.
- **No quad/duo/solo naming (D14).** Size and shape are DERIVED from member count, never named; a "quad"/"duo"/"solo" in fielded-squad code is a design smell.
- Calibration discipline: anti-creep fires only when `enemy_force.dps > 0`, so creep-free beds stay unperturbed; every producer keeps `defender_heal_parts_for_dps` for heal parity.

---

## 4. Proof

- **Acceptance test:** `oracle_sized_force_forms_and_kills_a_defended_core` (`screeps-combat-eval/src/harness/lifecycle.rs:495`) — the force is sized via `emit_requirement → assemble_force` against a defended bed (rampart + tower + guard), formed under economy contention, then moved in to kill. Un-ignored 2026-06-27 after P0a + P1b.
- **Regime sweep:** `assembler_kills_across_defended_regimes` (`harness/lifecycle.rs:521`), each graded case paired with a determinism assertion.
- **Calibration gates** (live + discriminating every phase): `OracleCalibration` (FP ≤ 0.010 / FN ≤ 0.200, `harness/validate.rs:94`), `SizingWins` (`harness/validate.rs:638`), `CreepClearWins` (`harness/validate.rs:749`).
- **Determinism fences:** `emit_requirement_golden_output_is_stable_over_realistic_bases` (`harness/validate.rs:909`, the bed-level parity fence — identical verdict + RequiredForce + composition run-twice), `sim_maintains_one_creep_per_tile` (`harness/validate.rs:923`), `sim_is_deterministic_over_rounds`.
- **Tournament lens (P6, D13/D16) — Tier-1 swept, results in ADR 0031b:** the optimizer changes WHICH forces are fielded, so the `CompositionParams` knobs ARE the sweep. The Tier-1 count × margin sweep (`harness::param_sweep::tests::sweep_composition_params`, env-driven, per regime `all`/`structure`/`creep`/`defended`) ran 2026-06-27; it **confirmed the seeds are Pareto-optimal across regimes and KEPT** (no `default()` change) — `member_energy` is the dominant axis (the defended-Kill floor pins it at 3000), `over_power_margin = 1.5` is the cheapest co-best in `all`, `hold_margin` is flat in v1, the FP gate ≤ 0.010 is the over-commit ceiling, the FN gate is never binding. Full per-regime emergent strategy map + the recommended-default rationale: `docs/design/0031b-force-composition-tuning-results.md`. The ADR-0019 position-utility weights + the exploitability tuning re-sweep (P6) and the Tier-2/3 archetype/drain sweeps remain (0031b §4).

---

## 5. Consequences

**Positive.** One sizing pipeline; templates, catalogs, and `is_sized` deleted (`prefer deletion over abstraction`). New roles, in-between counts, and dual weapons are all structurally reachable. Bot and eval are provably at parity. A defer is honest (`None`), never a silent static. `RequiredForce` is not `Serialize`, so the model costs no `WORLD_FORMAT_VERSION` bump; only the catalog deletion does (18→19, one loud reset, operator-accepted).

**Deferred.**
- **Budget-free `emit_requirement`** — the last presumed-shape residual. `optimizer_ceiling_budget` (the renamed 3+5) survives ONLY as `emit_requirement`'s winnability-assess budget; the per-candidate EV/commit is already budget-free. Make the structure/clear arms size from the per-candidate parts directly and `optimizer_ceiling_budget` retires entirely (§2c RESIDUAL).
- **Tier-2/3 archetype + drain + EHP-grading** — promote weapon `archetype` into the EV search (D17; the biggest remaining gap = the original WORK-siege-vs-guard failure); add a tower-present acceptance bed that *requires* `tough > 0` so the EHP ladder is graded; add the drain/kite cost-side branch + `engage_range` for the unboosted-vs-multi-tower path. Plan + ranges: 0031a §4; results so far: 0031b §4.
- **Formation-enum footprint cleanup** — `FormationShape::Box2x2`/`Line` and `military/formation.rs`'s hardcoded 2×2 overlay (`is_valid_quad_position`, `apply_quad_cost_overlay`) are silently wrong for 5–8 members; generalize to footprint-driven formation from member count.
- **P6 position-weights re-sweep** — re-run the ADR-0019 position-utility / exploitability tuning + re-sweep weights now that the optimizer changes which forces are fielded (the `CompositionParams` Tier-1 sweep itself is DONE — 0031b).
- **Higher-power multi-squad strategies** — the response to a `None` defer (scale the blob / coordinate multiple squads / boost) is a separate strategy-layer ADR; the composition layer's job ends at "best single squad, or None."

---

## 6. Decisions (D1–D16)

- **D1 — Capability vector.** EXTEND `RequiredForce`, not a parallel type; rename `ranged_parts`→`immune_struct_parts`, ADD `anti_creep_parts` (separate, summed). Not Serialize ⇒ no WFV.
- **D2 — One requirement emitter.** `emit_requirement` composes assess + `clear_force` over `ctx.enemy_force` + SK-kite + `scaled`; defenders read only via the existing `ctx.enemy_force` channel.
- **D3 — The assembler.** ONE `assemble_force` with marginal-capability-per-energy fill, role-set floor (delete `.max(template_count)`), `>MAX_SIZED_MEMBERS`→None, frozen tie-break. *(Superseded by D16 as the long-term builder.)*
- **D4 — Doctrine = pure classifier.** Drop `template()`/`is_sized()`/custom `plan()`; shared driver. No template ⇒ no `.unwrap_or_else(static)`.
- **D5 — Layer A fix = do BOTH.** Add dismantle to the strength sum AND always assemble an anti-creep weapon when defenders present (P0a scoped the strength fix to `assess_engage` gated on a hostile structure).
- **D6 — Winnability gate validates the ASSEMBLED force.** Confirm `capabilities(member_energy)` meets the requirement, not "did the template's assess say winnable." *(Recasts ADR 0030 §6.)*
- **D7 — Delete the catalogs in a WFV reset.** Leave `BodyType::Sized`; `WORLD_FORMAT_VERSION` 18→19, one loud reset.
- **D8 — Eval-first scope.** First landing is the eval/offline path; bot wiring is the tracked follow-up.
- **D9 — Subsume the ADR 0030 cleanup.** `is_sized()` deleted, count-floor → role-set floor, silent fallbacks deleted, no-silent-static test discipline carried forward; EngagementTempo stays orthogonal.
- **D10 — `None` is TERMINAL; no G4-HEAVY failover.** Can't field a winnable single squad ⇒ `None` = honest "don't attack." The G4-HEAVY framing (which never existed beyond log strings) is DELETED; higher-power response is a strategy-layer call. *(Supersedes every "defer to G4-HEAVY" mention.)*
- **D11 — No fixed doctrine; HarassRemote scales too.** Every doctrine is dynamic through the one driver; HarassRemote's distinction is purely tactical (deny-don't-hold). *(Makes 0030's `enum Sizing{Fixed,Dynamic}` unnecessary.)*
- **D12 — The WFV reset is not a phase blocker.** A WFV bump only gates an MMO deploy; delete the catalogs as soon as the assembler is the sole producer.
- **D13 — Re-tune + re-eval after the assembler lands (P6).** *(Made literal by D16 — the knobs ARE swept.)*
- **D14 — NO quad/duo/solo naming.** Size/shape are DERIVED; remove the named SHAPE vocabulary (constructors, the 2×2 overlay, the labels). Formation-geometry generalization is a tracked follow-up.
- **D15 — ONE squad-generation path.** Exactly one place a squad is born: `emit_requirement → assemble_force`; out-of-registry sites unified, `.unwrap_or_else(static)` fallbacks deleted. Acceptance: a grep for `SquadComposition::<constructor>` outside the driver returns EMPTY.
- **D16 — T2 is an EV-MAXIMIZING optimizer, tournament-tuned; NO presumed reference squad.** SUPERSEDES D3's `assemble_force` and the `force_ceiling` budget (both presume a shape). `optimize_composition` maximizes `EV(C) = P(win)·target_value − cost`; `emit_requirement` and `win_probability` survive as primitives. ONE search now; codify emergent strategies later.
- **D17 — The tunable surface is research-grounded; archetype is an EV-search dimension (ADR 0031a).** A four-bot survey (`docs/design/0031a-force-composition-tunable-params.md`) confirmed the count-axis knobs (`hold_margin`/`over_power_margin`/`member_energy`/`commit_ev_threshold` — our seeds sit in the field's 1.2–1.5 bracket) and added the **body/archetype axis**: (a) **`archetype`** (RangedBlob/MeleeAttack/WorkDismantle/Drainer) is a TUNED search dimension — the doctrine `fighter_role` becomes a *feasible-set* constraint, the EV search picks within it, so a weapon mismatch (the original WORK-siege-vs-guard failure) is rejected by `EV(C)` itself; (b) **`tough_parts`** EHP armor (was hardwired 0); (c) **`commit_mode` Siege vs Drain** reusing `AssaultMode::Drain` (unboosted v1 cannot out-heal towers point-blank). Tier-3 deferrals: `engage_range`, within-member `attack_to_heal_mix`, `reengage_threshold`, the v2 `boost_tier` axis. 0031a §4 is the tiered tournament-sweep plan.

---

> **Evolution.** Built across P0–P4b (committed: P0a Layer-A brain fix `5db5e08`; P2 `emit_requirement` `778e93d`/`ac61b0b`/`6bd8e1b`; P3 `assemble_force` `5079bf8`/`38fd534`/`da79345`; P4a pure-classifier unification `4691c00`/`54da38c`/`0fb1370`). **P4b landed alongside this consolidation:** the ≈13 `SquadComposition` constructors + `sized_for` + the static `BodyType` shapes (now `Sized`-only) deleted, the orphaned catalog `bodies::*_body` removed, all call sites migrated, and `WORLD_FORMAT_VERSION` 18→19. **The EV optimizer landed (D14/D16/D17):** `force_ceiling`'s presumed budget is deleted as a producer — `optimize_composition` is the per-candidate EV decision (over-power × TOUGH ladders, `assemble_force` builds each candidate), and `force_ceiling`'s 3+5 math survives only as the renamed `optimizer_ceiling_budget` for `emit_requirement`'s winnability-assess. **The Tier-1 `CompositionParams` tournament sweep landed + the seeds are confirmed-optimal-across-regimes and KEPT** (no `default()` change; ADR 0031b, grounded in 0031a's four-bot survey). **Remaining:** the budget-free `emit_requirement` (retires `optimizer_ceiling_budget`), the Tier-2/3 archetype/drain/EHP-grading sweep (D17 / 0031a §4 / 0031b §4), the formation-enum footprint cleanup, and the P6 position-weights re-sweep. See `docs/design/0031a-force-composition-tunable-params.md` (params) + `docs/design/0031b-force-composition-tuning-results.md` (results).
