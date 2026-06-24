# Move force-sizing + composition into `screeps-combat-decision` (+ oracle-calibration tournament)

Status: **IN PROGRESS** (started 2026-06-24). Operator-directed: *"We can move pure (no game
dependency, or shim/dependency-injected) code into the combat decision crate if needed. The win is
being able to dynamically generate scenarios, then dynamically generate our force using force sizing
and prove that size + tactics wins against a variety of random/good/bad compositions."*

Operator decisions (AskUserQuestion, 2026-06-24): **(1)** move scope = **sizing core + SquadComposition**
(the sim fields the *real* composition, no replica); **(2)** harness home = **combat-eval**; **(3)** gate =
**oracle calibration** (winnable⇒wins, defer⇒wouldn't), reporting false-positive / false-negative rates.

Vetted by mapping workflow `wf_ad62f826-c67` (4 agents: closure/purity, call-sites, shims, tournament).

## Why / end-state
`assess` + `RequiredForce` + `SquadComposition::sized_for` + `build_combat_body` are pure but bot-internal,
so the sim (`screeps-combat-agent`) and eval (`screeps-combat-eval`) — siblings of the bot, both depending
on `screeps-combat-decision` — cannot field the *real* force. Move the pure sizing/composition core into
`screeps-combat-decision` (already "pure logic over screeps-game-api value types, depended on by the sim
not the whole bot"). Then combat-eval runs the REAL `assess`→`sized_for` over randomized scenarios and
proves the oracle is calibrated against the engine ground truth.

## Serde / WFV
**No WFV bump.** bincode is positional/structural, NOT crate-path-keyed. The persisted types
(`SquadRole`, `BodyType`, `CombatBodySpec`, `SquadSlot`, `FormationShape`, `FormationMode`,
`SquadComposition`) move with derives + field/variant order byte-identical → serialized shape unchanged.
Keep `BodyType::Sized` LAST. WFV stays **17**. (`DefenseProfile`/`force_sizing` types are NOT serde — ephemeral.)

## Constants — import, don't re-declare
The moved code currently re-declares constants that already exist in `screeps_combat_engine::constants`
(`ATTACK_POWER`/`RANGED_ATTACK_POWER`/`HEAL_POWER`/`DISMANTLE_POWER`/`TOWER_ENERGY_COST`/`CREEP_LIFE_TIME`)
and `screeps::constants` (`CREEP_SPAWN_TIME`, `MAX_CREEP_SIZE`). Delete the local redefs
(`force_sizing.rs:22,203`; `composition.rs:10-11`; `bodies.rs:36`) and import. Keep the bot's **f32**
`tower_attack_damage_at_range` (move it, don't reuse the engine's u32 — avoids a rounding delta in `assess`).

## The two shims
- **Shim A — PathfinderService:** make `SquadComposition::estimated_combat_time`/`is_viable_from` take a
  precomputed `travel_ticks: u32` (drop `pathfinder`/`home`/`target`); **delete** `estimated_travel_time`
  (pure pass-through to a host-only API). Only real caller: `best_force_budget` (war.rs ~1247) which already
  owns the pathfinder + loops over homes — hoist one `pathfinder.travel_ticks(h,t,game::time())` call there.
  `is_viable_from` has zero live callers. Return type `Option<u32> → u32`.
- **Shim B — spawning:** move `SpawnBodyDefinition`, `spawning::create_body`, the private `clamp`, and the
  5 create_body test pins into a new `screeps-combat-decision::spawning` module. Confirmed pure (deps:
  `Part::cost`, `screeps::MAX_CREEP_SIZE` u32, local `clamp`, std). Drop the `#[cfg_attr(feature="profile"…)]`
  attr (decision has no profile feature). Keep the bot's `spawning::build` (specs-coupled) in creep.rs.

## Re-export surface (bot keeps old paths compiling — NO call-site edits except Shim A's war.rs hoist)
1. `src/military/force_sizing.rs` → `pub use screeps_combat_decision::{assess, DefenseProfile, ForceBudget, ForceAssessment, RequiredForce, TowerThreat, AssaultMode, win_probability, importance_margin, HOLD_MARGIN};` (HOLD_MARGIN must become `pub`).
2. `src/military/bodies.rs` → `pub use screeps_combat_decision::{CombatBodySpec, MoveProfile, build_combat_body};` (templates stay in the bot, but they can also move — see Move 3).
3. `src/military/squad.rs` → `pub use screeps_combat_decision::SquadRole;` (rest of squad.rs stays).
4. `src/military/composition.rs` → `pub use screeps_combat_decision::{BodyType, SquadSlot, SquadCapabilities, FormationShape, FormationMode, SquadComposition};`
5. `src/creep.rs` → `pub use screeps_combat_decision::spawning::SpawnBodyDefinition;` + inside `pub mod spawning { pub use screeps_combat_decision::spawning::create_body; /* keep build */ }`. The `creep::*` glob users (localbuild/remotebuild/salvage) need `SpawnBodyDefinition` at the `creep` module root.

Consumer files (must keep compiling via the above): game_loop (comments only — no-op), objective_queue, squad, squad_manager, sourcekeeperfarm, war, + 15 files using SpawnBodyDefinition/create_body (claim, haul, scout, reserve, upgrade, localsupply/{source_mining,mineral_mining,body_helpers}, localbuild, remotebuild, salvage).

## Sub-commit order (each GREEN: combat-decision builds+tests, bot clippy-wasm+tests, then super pointer bump)
- **Move 1 — spawning** (Shim B): `SpawnBodyDefinition`+`create_body`+`clamp`+5 tests → `combat-decision::spawning`; creep.rs re-exports. Foundational (templates need it). Independent of Moves 2/3.
- **Move 2 — sizing core**: the pure `damage` slice (`tower_attack_damage_at_range` f32 + `defender_heal_parts_for_dps` + `HEAL_PER_PART_ADJACENT`/`KILL_WINDOW_TICKS`/`MAX_OFFENSE_PARTS`/`attack_parts_to_kill`/`drain_heal_parts_for_dps`) + `CombatBodySpec`/`MoveProfile`/`build_combat_body`/`assemble_combat_body` + all of `force_sizing` → `combat-decision::{tower,bodies,force_sizing}` (or one `force` module). bot re-exports. Independent of Move 1.
- **Move 3 — composition**: `SquadRole` + `BodyType` + the template body fns + `SquadComposition`/`SquadSlot`/`FormationShape`/`FormationMode`/`SquadCapabilities`/`sized_for`/`capabilities` + Shim A → `combat-decision::composition`. Depends on Moves 1+2. bot re-exports + the war.rs Shim A hoist.
- **Move B — tournament** (the WIN): `combat-eval/src/oracle_calibration.rs`.

## Move B — oracle-calibration tournament (combat-eval)
- New `pub mod oracle_calibration;` next to `tournament`. Reuse `room()`/`pos()`, `ScenarioBuilder`,
  `run_siege`/`defense_intents`/`SiegeResult`, `ManagedSimSquad`, `TournamentBudget`, `report` idiom.
- **Seeding:** inline SplitMix64 `Rng::seeded(index)` (no `rand` dep; per-index reproducible). No
  `Math.random`/`Date` (deterministic).
- **Generator `generate(index) -> Scenario`:** builds BOTH a `CombatWorld` bed AND the matching
  `DefenseProfile` from the SAME draws (so the oracle judges what the engine resolves). Ranges STRADDLE the
  boundary: core_hits 20k–100k; ramparts 0–80k (70% present); 0–6 towers at known tiles (range→assault
  computed from the tile), ~25% drained (<10 energy); 40% have 1–3 ATTACK defenders; repair 0–800 only when
  ramparts+tower present (models `defense_intents` single-tower maintenance); 5% safe_mode. Mirrors
  war.rs:804-820. member_energy ∈ RCL4..8; onsite_budget_ticks 600–1400 (synthetic — no PathfinderService).
- **Size our force (REAL path):** `pick_composition` (siege_quad if breach_hits>0 else quad_ranged) →
  `comp.capabilities(energy)` → `ForceBudget` → `assess(profile,budget)` → `RequiredForce::from_assessment`
  → `comp.sized_for(required, energy)`. `place_attackers` realizes each slot via `BodyType::build_body`.
- **Gate (`#[cfg(test)]`):** over N seeds (200), for `winnable&fielded` rows run the siege and a
  `won==CoreBreached` MISS is a **false positive** (gate HARD: fp_rate ≤ 1%). For `unwinnable`/`unaffordable`
  rows, falsify by fielding the **max** affordable single squad (grow every role to MAX_SIZED_MEMBERS/50-cap);
  if it breaches in-budget it's a **false negative** (gate SOFT: fn_rate ≤ 20%). Asymmetric thresholds match
  the oracle's "a yes is safe, a no defers, never the reverse" contract.
- **CRITICAL caveat (encode in doc comments):** the managed squad's brain is kite/engage-oriented; for a
  pure siege it must close + dismantle-through-ramparts. If `decide_squad_with_pathing` doesn't yet drive
  that in-sim, **grade on a ScriptedSiege closure** (the objective_bed dismantler+healer intents — sizing-pure)
  with `Managed` reported as a SECONDARY diagnostic, so a *pathing* gap can't masquerade as a *sizing*
  false-positive. The calibration of interest is SIZING (did we bring enough heal/dps), not squad pathing.

## Cross-refs
ADR 0022 (P-FORCE), ADR 0006 (the decision-crate seam / "one implementation, no fork"), ADR 0023 (sim beds).
Map artifact: workflow `wf_ad62f826-c67` output.
