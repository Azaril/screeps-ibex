# Move force-sizing + composition into `screeps-combat-decision` (+ oracle-calibration tournament)

Status: **IN PROGRESS** (started 2026-06-24). Operator-directed: *"We can move pure (no game
dependency, or shim/dependency-injected) code into the combat decision crate if needed. The win is
being able to dynamically generate scenarios, then dynamically generate our force using force sizing
and prove that size + tactics wins against a variety of random/good/bad compositions."*

Operator decisions (AskUserQuestion, 2026-06-24): **(1)** move scope = **sizing core + SquadComposition**
(the sim fields the *real* composition, no replica); **(2)** harness home = **combat-eval**; **(3)** gate =
**oracle calibration** (winnable⇒wins, defer⇒wouldn't), reporting false-positive / false-negative rates.

Vetted by mapping workflow `wf_ad62f826-c67` (4 agents: closure/purity, call-sites, shims, tournament).

## CURRENT STATE (2026-06-24) — Moves 1–3 + tower curve DONE; only Move B (the WIN) remains
`decision` now owns the ENTIRE sizing core: modules `spawning`, `damage`, `bodies`, `force_sizing`,
`composition` (109 host tests). The bot uses them (no duplicate defs); deleted bot files:
`military/{force_sizing,bodies,composition}.rs`. All green, full `clippy-wasm` clean, working tree clean. Commits:
- Move 1 (spawning): decision `1a99fb9` / super `21db86e`.
- Move 2 (sizing core: bodies primitives + force_sizing): decision `c7e092f` / super `521a232` (added `serde` dep to decision).
- Tower-curve consolidation: decision `f1a1db6` / super `b86a032` (bot's duplicate f32 curve deleted; `decision::damage` re-exports the engine's canonical curve; bot reaches it via decision — no bot→engine dep).
- **Move 3 (the composition HUB): decision `0d8b3ec` / super `a6c95cb`.** Moved `SquadRole` + `BodyType`
  (+ methods) + `SquadComposition`/`SquadSlot`/`FormationShape`/`FormationMode`/`SquadCapabilities`/`sized_for`/
  `capabilities` → `decision::composition`; the static template bodies + `assemble_combat_body`/
  `sized_defender_body`/`sized_healer_body`/`boosts` + the part-sizing cluster (`attack_parts_to_kill`/
  `KILL_WINDOW_TICKS`/`MAX_OFFENSE_PARTS`/`drain_heal_parts_for_dps`) → `decision::bodies`. **Shim A applied**
  (`estimated_combat_time`/`is_viable_from` take `travel_ticks: u32`, return `u32`; `estimated_travel_time`
  deleted; the one `best_force_budget` caller hoists `pathfinder.travel_ticks`). Bot `military/{bodies,composition}.rs`
  DELETED; `SquadRole` dropped from `squad.rs`; the part-sizing cluster dropped from `damage.rs` (it keeps its
  tower-over-`Position` math + spawn-readiness). **Verified behavior-neutral by adversarial workflow
  `wf_2caaf704-c55`** (4 lenses — transcription-fidelity / Shim-A-semantics / layering-boundary /
  completeness — ALL findings `none`: templates byte-faithful, Shim A equivalent incl. the None→continue
  path, decision stays JS-free with no bot→engine dep, zero stale paths). Green: decision 109, bot lib 149,
  agent 48, eval 18; `cargo test --all` 566; `clippy-wasm` clean.
- Plan/doc commits: `ea0161e` (plan), `98bc372` (Move 1 done), `b8b0c35` (Moves 1–2 + tower doc).

**Remaining: Move B (the tournament — the WIN) only.** WFV unchanged (17); serialization may break freely
per operator (one reset at the very end). Layering verdict (applied + verified): mechanics → `engine`;
combat policy (sizing/composition/bodies/tactics) → `decision`; the bot reaches engine mechanics *through*
decision. The composition core now lives in `decision`, so `combat-eval` can field the REAL
`assess`→`sized_for`→`BodyType::build_body` over randomized scenarios (the gap the `objective_bed` test's
"sim can't depend on the bot's `sized_for`" comment named is now closed).

## Why / end-state
`assess` + `RequiredForce` + `SquadComposition::sized_for` + `build_combat_body` are pure but bot-internal,
so the sim (`screeps-combat-agent`) and eval (`screeps-combat-eval`) — siblings of the bot, both depending
on `screeps-combat-decision` — cannot field the *real* force. Move the pure sizing/composition core into
`screeps-combat-decision` (already "pure logic over screeps-game-api value types, depended on by the sim
not the whole bot"). Then combat-eval runs the REAL `assess`→`sized_for` over randomized scenarios and
proves the oracle is calibrated against the engine ground truth.

## Serde / WFV — break it freely (operator 2026-06-24)
**Operator directive:** *"We can break serialization at any point in the current work — we'll get to
fully working/complete and then do the reset. I'd rather get to final/clean code than carry debt."* So:
do NOT preserve serde shape or add forward-compat shims; if a clean restructure changes a persisted
type, just bump `WORLD_FORMAT_VERSION` (folds into the one end-of-work reset). A straight relocation
happens to be shape-neutral anyway, but don't contort for it.

## CLEAN end-state — no re-export husks (operator 2026-06-24)
Same directive ("final/clean over debt") overrides the map's re-export-shim approach. The fully-moved
bot modules (`military/force_sizing.rs`, `military/bodies.rs`, `military/composition.rs`) are **DELETED**,
not left as `pub use` husks; call sites import from `screeps_combat_decision::...` directly. Modules that
retain bot-specific code keep it and import the moved pieces: `military/damage.rs` keeps its
game-coupled fns (`tower_dps_at_room_edge`/`net_tower_damage`/… over `Position`) and imports the pure
slice; `military/squad.rs` keeps everything except `SquadRole`; `creep.rs` keeps `spawning::build`
(Move 1 already re-exports `create_body`/`SpawnBodyDefinition` there — acceptable, creep.rs retains real
code). The re-export surface in the section below becomes a **call-site-edit** surface instead.

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
- **Move 1 — spawning** (Shim B): ✅ **DONE** (decision `1a99fb9` / super `21db86e`). `SpawnBodyDefinition`+`create_body`+`clamp`+5 tests → `combat-decision::spawning`; creep.rs re-exports (kept `spawning::build`). clippy-wasm clean.
- **Move 2 — sizing core**: ✅ **DONE** (decision `c7e092f` + the bot switch). Relocated to `decision::{bodies, force_sizing}`: `CombatBodySpec`/`MoveProfile`/`build_combat_body` + `defender_heal_parts_for_dps` (using the engine's `HEAL_POWER`, not a dup) + all of `force_sizing` (using `screeps_combat_engine::{constants, damage}` — the canonical tower curve, NOT a duplicated f32 copy; added `serde` derive dep). Bot `force_sizing.rs` DELETED; `bodies.rs` re-exports the 3 primitives + the heal helper (transitional — `bodies.rs` fully moves in Move 3); `damage.rs` lost `defender_heal_parts_for_dps` (kept its tower-over-Position fns); call sites (war/sourcekeeperfarm/composition) import from decision. 39 bot + 95 decision host tests; clippy-wasm clean. **Constraint honored:** force-sizing = pure POLICY in decision, on engine MECHANICS (no curve dup). The part-sizing helpers `attack_parts_to_kill`/`KILL_WINDOW_TICKS`/`MAX_OFFENSE_PARTS`/`drain_heal_parts_for_dps` stay in bot `damage.rs` for Move 3 (used by the template `sized_defender_body`/`drain_body` that move with composition).
- **Tower-curve consolidation**: ✅ **DONE** (decision `f1a1db6` / super `b86a032`). `decision::damage` re-exports the engine's canonical tower attack/heal/repair curve; bot `military/damage.rs` deleted its duplicate f32 trio (attack used only internally; heal/repair were dead) and reaches the curve through decision (no bot→engine dep). Behavior-neutral (value-identical curves). force_sizing uses `crate::damage` (one internal path).
- **Move 3 — composition (✅ DONE — decision `0d8b3ec` / super `a6c95cb`; the hub, done as ONE coherent green push)**: moved into `decision::composition`: `SquadRole` + `BodyType` (+ all its methods) + `SquadComposition`/`SquadSlot`/`FormationShape`/`FormationMode`/`SquadCapabilities`/`sized_for`/`capabilities` + **Shim A** (drop `estimated_travel_time`; `estimated_combat_time`/`is_viable_from` take `travel_ticks: u32`, return `u32`). Also move the remaining `bodies` templates + the part-sizing cluster (`attack_parts_to_kill`/`KILL_WINDOW_TICKS`/`MAX_OFFENSE_PARTS`/`drain_heal_parts_for_dps` + `assemble_combat_body`/`sized_defender_body`/`sized_healer_body`/`boosts`) into `decision::bodies` (`BodyType::body_definition` references the templates, so they must be co-located). **`composition.rs` is the HUB** (`BodyType::body_definition` pulls in every template) — so it does NOT split into safe sub-commits without churning the hub or leaving transient dup. **Execute as ONE push:** relocate the files wholesale (prefer `git`-level move/copy + import fixups over re-transcription — keep it exact), fix imports (`super::bodies`→`crate::bodies`, `super::squad::SquadRole`→local, `crate::pathing`→Shim A, `crate::military::damage::*`→engine/local), then the bot switch: DELETE bot `military/{bodies,composition}.rs`, drop `SquadRole` from `squad.rs` (+ re-point its internal uses), drop the part-sizing cluster from `damage.rs`, the **one** `best_force_budget` (war.rs ~1247) `travel_ticks` hoist, and update all call sites to `screeps_combat_decision::{composition,bodies}::…`. WFV: serialization may break freely (one reset at end) — don't fuss over `BodyType`/`SquadComposition` shape. Exit: 0 bot `military::{bodies,composition,force_sizing}` modules remain (force_sizing already gone); decision owns it all; `clippy-wasm` clean; bot + decision host tests green.
- **Move B — tournament (the WIN — NEXT, the only remaining step)**: `combat-eval/src/oracle_calibration.rs` — see the design below.

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
