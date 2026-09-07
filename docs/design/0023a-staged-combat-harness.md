# 0023a — Staged combat harness: Generation / Evaluation / Validation (annex to ADR 0023)

- **Status:** Decided

Operator-directed: *"ensure we have a plan to generate a large variety of single and multi-room layout
permutations … generate random or designed squad or multi-squad opponent forces, including single and
multi-room objectives. Split into generation, evaluation and validation stages so generation and
validation can be swapped. The evaluation just needs a run-until predicate or condition."*

> **Type-name note:** the `CombatWorld` / `resolve_tick` / `Intents` types in the API sketches below are
> renamed + split by [ADR 0033](0033-rover-pathing-sim-and-benchmark.md): `CombatWorld`→`sim_core::SimWorld`,
> `resolve_tick`→`resolve_movement_tick` (sim-core) + `resolve_combat_tick` (combat-engine),
> `Intents`→`MoveIntents` + `CombatIntents`. Read them as their successors; the staged
> Generation / Evaluation / Validation design is unchanged.

## Why
The P-FORCE oracle-calibration tournament (`combat-eval/src/oracle_calibration.rs`, the WIN) proved the
seam works — but it **welds the three concerns together**: `generate()` builds a single-room bed +
the oracle `DefenseProfile` in one shot; `breaches()`/`run_siege` is a fixed siege loop; `calibrate()`
is the FP/FN judge. To grow coverage (multi-room, designed beds, opponent squads, other gates) without
rewriting the runner each time, split into three **swappable stages** with the evaluator as a dumb,
shared engine in the middle.

## The three stages

### 1. Generation — *produce scenarios; know nothing about how they're judged*
```rust
/// A generated scenario: a world + the objective(s) + the opponent force already placed in the world.
/// Single- OR multi-room. Carries NO oracle/validator state (a validator that needs an oracle profile
/// derives it from `world`), so generation and validation are independent.
pub struct Scenario {
    pub world: CombatWorld,          // terrain + structures + towers + DEFENDER creeps (the opponent)
    pub objectives: Vec<Objective>,  // what the attacker must achieve (one per room, or several)
    pub attacker_owner: PlayerId,
    pub defender_owner: PlayerId,
    pub label: String,
    pub seed: u64,                   // provenance — fully reproducible
}

pub enum Objective {
    /// Destroy a structure (a core/spawn) at a known room+pos. (Future: ClearRoom, HoldFor(ticks), …)
    Destroy { id: StructureId, room: RoomName, pos: Position },
}

pub trait Generator {
    fn label(&self) -> &str;
    fn count(&self) -> u32;                       // how many distinct scenarios it offers
    fn generate(&mut self, index: u32) -> Scenario; // seeded by index → reproducible
}
```
Generators (all `impl Generator`, freely swapped into the runner):
- **`RandomDefendedBase`** — the seeded SplitMix64 draws from Move B, extended: per-room tower/rampart/
  wall configs + a random **opponent force** (`ForceSpec` → defender creeps).
- **`Permutations`** — a systematic cross-product over a feature grid (room count × {open / walled-gap /
  rampart-bunker / tower-nest / corridor} × opponent archetype × objective kind) so coverage is
  *enumerable*, not just sampled.
- **`Designed`** — named hand-authored fixtures (the `objective_bed` beds, the SK farm, a multi-room
  outpost). Regression anchors with known-correct verdicts.
- **`MultiRoom`** — composes per-room sub-beds via `ScenarioBuilder::in_room` (the engine is N-room —
  ADR 0023 S3 / task P-ENGINE), with objectives carrying their room.
- **`ForemanGenerator`** — **realistic rooms from the bot's own planner.** Run
  `screeps-foreman`'s room layout / base generation over seeded terrain to get a REAL base plan
  (spawn/towers/ramparts/walls in believable positions), then realize the plan's structures + terrain
  into a `CombatWorld` + an `Objective` (the spawn/core, breach corridor from the plan's rampart ring).
  Replaces hand-authored terrain with layouts the bot would actually build/face — the realism the
  operator wants to validate against. (Foreman is a host crate; `combat-eval` can dep it.)
- **`ImportedRoom`** — **a live world/shard import tool.** Fetch a real room's
  terrain + structures from a shard (via the `screeps-rest-api` crate / the game API) and realize them
  into a `CombatWorld`; then SEED attacker/defender forces (a `ForceSpec`) to make a sim scenario from a
  real target. Lets us replay-validate the bot's sizing/tactics against actual MMO rooms. The fetch is a
  separate offline tool (writes a captured-room JSON the generator loads) so the harness itself stays
  deterministic + offline; needs operator go-ahead + credentials for live fetches (never auto-run).

**Opponent forces** — a `ForceSpec` (archetype + count + placement) realized into defender `SimCreep`s:
`Turtle` (HEAL walls), `Rush` (melee), `Drain`, `SiegeDefenders` (ATTACK/RANGED behind ramparts),
`MultiSquad` (several coordinated groups, one per room or layered). Random *or* designed.

### 2. Evaluation — *step until a predicate fires; know nothing about objectives or oracles*
The evaluator optionally **records** every tick (the engine's `record_tick` → `CombatRecording`) so the
same run feeds both validation (the outcome) and visualization (the frames). The recording model is
rich (`CreepFrame` owner/hits/attack/ranged-power, `StructureFrame` kind/owner/hits,
`TowerFrame` energy/hits, intents + "why" reasons, deaths, destroyed-kinds); for multi-room it must also
carry the **room** per entity (see §4 / the engine extension).

```rust
pub enum StopReason { ObjectivesComplete, AttackersWiped, Timeout, Custom(&'static str) }

pub struct EvalOutcome { pub world: CombatWorld, pub ticks: u32, pub stop: StopReason }

/// `Some(reason)` ⇒ stop now. Composable: `All`, `Any`, `ObjectivesDestroyed(&[Objective])`,
/// `SideWiped(owner)`, `Timeout(max)`.
pub trait RunUntil {
    fn check(&self, world: &CombatWorld, tick: u32) -> Option<StopReason>;
}

/// The generic engine loop — generalizes `objective_bed::run_siege`. Drives attacker + defender intents
/// through `resolve_tick` until `run_until` fires. Multi-room (resolve_tick already is).
pub fn evaluate(
    mut world: CombatWorld,
    attacker: &mut dyn FnMut(&CombatWorld) -> Intents,
    defender: &mut dyn FnMut(&CombatWorld) -> Intents,
    run_until: &dyn RunUntil,
) -> EvalOutcome;
```
The attacker/defender intent producers are pluggable closures or `TacticalAgent`s — a **scripted siege**
(sizing-pure, Move B), the **managed squad** (`decide_squad_with_pathing`, full pathing), or any agent.

### 3. Validation — *judge a scenario, driving evaluation as it sees fit; swappable*
```rust
pub struct Verdict { pub pass: bool, pub label: String, pub metrics: Vec<(String, f64)> }

pub trait Validator {
    fn label(&self) -> &str;
    fn validate(&mut self, scenario: &Scenario) -> Verdict;
}
```
Validators (independent of the generator):
- **`OracleCalibration`** — derives the `DefenseProfile` from `scenario.world` + the objective (the
  derivation Move B has, now living here, oracle-agnostic generation above it), assesses → sizes →
  fields the attacker force → `evaluate(run_until = objectives-or-wiped-or-timeout)` → FP/FN. The Move B
  gate, re-expressed on the seams.
- **`SizingWins`** — the at-a-glance "size our real force, field it, did we win?" win-rate lens over the
  same generators.
- **`Metrics`** — cohesion / positioning / EV (the EXP-register instruments) over the outcome.
- **`SelfPlay`** (operator-requested realism) — BOTH sides run the real `ManagedSimSquad` brain
  (`decide_squad_with_pathing`) + the defender's towers fire; the opposing side MOVES + fights (not a
  static `defense_intents` line). The realistic engagement; pairs with the agent's `ManagedSimSquad`
  **cross-room travel mode** (a squad whose members are in another room paths to the objective room via
  the rover before engaging — fixes the room-scoped-view "no cross-room movement"). Stage a cross-room
  assault near the border (the rover's per-call search is range-bounded).
- **`ManagedSquadIntegration`** — the traversal lens: field a ranged+heal quad at the entry and drive the
  real `decide_squad_with_pathing` to engage, grading end-to-end (movement-rich replays). Kept *off*
  `RandomDefendedBase` so the calibration's zero-FP grading stays sizing-pure.

**Runner**: `run_suite(&mut dyn Generator, &mut dyn Validator) -> SuiteReport` crosses every scenario the
generator offers with the validator and aggregates. Generation ⊥ validation ⊥ run-until — any triple
composes.

### 4. Visualization — *render a recording (+ metadata) as an interactive, multi-room HTML replay*
The operator-facing **visual validation** layer: turn a `CombatRecording` + scenario metadata into a
self-contained **interactive HTML** replay the operator opens and scrubs, to eyeball both tournament
*outcomes* and the *variety* of generated permutations. **Operator decisions:**
*interactive HTML player ONLY — no SVG rendering (no SVG filmstrip in the agent)*;
the renderer *takes a replay output + metadata*; it lives in a *host-only crate/module, never in live
bot code*.
```rust
/// Render a recording + scenario metadata to a self-contained interactive HTML replay player.
pub fn replay_to_html(rec: &CombatRecording, meta: &ReplayMeta) -> String;
```
- **Home**: a host-only module in `combat-eval` (the harness crate — it's `--workspace --exclude`'d from
  the wasm build, so it can NEVER reach live bot code). The engine `record.rs` note already says "a
  richer renderer is policy and lives in screeps-combat-eval". (Extractable to its own crate later, like
  the other Azaril crates, if desired.)
- **`ReplayMeta`**: the scenario label/seed, the room layouts (per-room terrain: plain/swamp/wall) and
  static buildings, owner→side legend, the objectives, and the validator verdict — everything the player
  needs beyond the per-tick frames.
- **Interactive HTML player** (single self-contained file, frames embedded as JSON, vanilla JS — no
  external deps): a tick **scrubber + play/pause/step**, a **per-frame data panel** (tick, per-creep
  HP / role / intent + "why" reason, tower energy, deaths, destroyed structures), and the verdict.
- **Multi-room**: rooms tiled into a labeled grid (room name per panel); each entity drawn in its room's
  panel. Requires the engine recording to carry the **room** per entity (see the engine extension below).
- **Terrain + buildings**: per-room backdrop (plain/swamp/wall tiles) + **typed** buildings: Spawn
  (filled square), Tower (square + an energy bar that tracks the drain), Rampart (translucent shield,
  opacity ∝ hits), constructed Wall (solid, distinct from terrain wall). Creeps = owner-coloured discs,
  radius ∝ HP, role hinted by part mix, with a per-tick attack/heal flash from the frame intents.
- The runner writes one HTML file per scenario (+ a contact-sheet index linking them) under a run dir;
  the operator opens them to validate outcomes + permutation variety.
- **Reuse `screeps-visual`**: the backend-agnostic primitives crate
  (`VisualBackend` circle/rect/poly/line + `render_structure`/`structure_primitives` per
  `StructureType`, dep = just `screeps-game-api`). Implement a `VisualBackend` that captures each
  structure type's primitive template once; the player's JS instances the template at every building's
  room position so typed buildings match the bot's own rendering. `combat-eval` adds `screeps-visual` as
  a (host-only) dep.

**Engine extension (multi-room recording).** `CreepFrame`/`StructureFrame`/`TowerFrame` carry the
entity's **room** (its `Position`, or a `RoomName`/compact room id) alongside `x,y` so the visualizer can
place entities across rooms. Additive to `record.rs`; bumps the engine submodule. The data is already on
hand (`SimCreep.pos` is a `Position`).

## The pathing-vs-sizing-purity tension (and how the split resolves it)
The oracle-calibration deliberately grades a **scripted, in-range** siege so a *squad-pathing* gap can't
masquerade as a *sizing* false positive (ADR 0023 caveat). Multi-room objectives inherently need
**traversal** (pathing). These don't fight in the staged model: they're **different validators over the
same generator**. `OracleCalibration` stays sizing-pure (places the sized force in-range per objective,
even in a multi-room world — one engagement per objective). A separate **`ManagedSquadIntegration`**
validator drives the real `decide_squad_with_pathing` across rooms and grades end-to-end (the movement
workstream's gate). Same scenarios, two lenses — exactly what the swap buys.

## Phased build plan
- **Phase A — foundation (extract the seams):** `Scenario`/`Objective`/`Generator`,
  `evaluate`/`RunUntil`/`StopReason`, `Validator`/`Verdict` under `combat-eval/src/harness/`; the
  single-room calibration re-expressed as `RandomDefendedBase` + `OracleCalibration` on the seams,
  **behavior-identical** to the `oracle_calibration.rs` monolith it replaces (identical fielded / FP /
  deferred / FN counts is the acceptance condition), and the monolith deleted.
- **Phase V — visualization:** engine recording carries room per frame entity;
  `harness/visualize.rs::replay_to_html` is the self-contained interactive player (scrubber/play/step +
  per-frame data panel, multi-room grid, terrain + `screeps-visual` typed buildings, owner-coloured HP
  discs); `evaluate_recorded` + `render_calibration_replay` + `calibration_replay(index)` wire the full
  chain. **Interactive HTML only**, host-only in `combat-eval`.
- **Phase B — layout variety:** rich single-room permutations (wall/rampart/tower/cwall configs,
  multiple breach corridors) + `Designed` fixtures + the `Permutations` enumerator. Multi-room layouts
  via `MultiRoom`. (Visually validated via Phase V.)
- **Phase C — opponent forces + traversal lens:** `ForceSpec` archetypes (None/Skirmishers/Guard;
  random *and* designed, single & multi-squad) → defender creeps, with `enemy_dps` wired into the derived
  profile and combat wired into the assault; multi-room objective lists; and the
  `ManagedSquadIntegration` validator for the traversal lens. Kept off `RandomDefendedBase` to preserve
  the sizing-pure calibration.
- **Phase D — more validators + scale:** `SizingWins`, `Metrics`; widen the seed count /
  enumerate the permutation grid; a report dashboard (a contact-sheet index linking the per-scenario
  replays — `run_suite` already returns per-scenario verdicts).
- **Phase F — realistic rooms (`ForemanGenerator`):** generate beds from `screeps-foreman` base plans
  (operator-requested realism). Lands after D so the dashboard + managed lens render real layouts.
- **Phase G — live import (`ImportedRoom`):** the offline shard-capture tool + the generator that
  replays the bot's force against real MMO rooms (operator go-ahead + credentials required).

## Constraints (carried)
Deterministic (SplitMix64 by index — no `Date`/`Math.random`); host-only in `combat-eval`; the engine is
ground truth (the validators judge against `resolve_tick`); generation stays oracle-agnostic so a
generator can feed any validator. Break serialization freely (no persisted state here). No bot→engine
dep (the harness is eval-side, which already depends on engine+decision+agent).

## Cross-refs
ADR 0023 (sim beds), ADR 0022 P-FORCE (the oracle), `combat-agent/src/{objective_bed,scenario}.rs`
(`run_siege` / `ScenarioBuilder` — the evaluate/generate primitives this generalizes).

## Design deltas (2026-09-07 — WS-CLOSE lane (b2))

- **`MultiRoom` folds the standing multi-room builders in as enumerated indices** rather than
  re-deriving them: index 0 aliases the twin-room siege (`Designed#4`), 1–4 the border-gauntlet
  grades (`BorderGauntlet::build(grade, 3)`), 5–14 the strongholds staged in the neighbour room
  (`StrongholdScenario::build(level, {Open, Chokepoint}, multi_room = true, 1)`), byte-identical to
  the builders (pinned) so a `run_suite` over `MultiRoom` grades the very rungs the stronghold floor
  pins. The composed family (48 indices: staging layout × target layout × `ForceSpec`) is the
  per-room composition this ADR specified: `ScenarioBuilder::empty(staging).in_room(target)`, the
  objective carrying the target room, the entry the staging room; the seed (the index) draws the
  target's rampart hits + tower count so the family spans undefended → towered.
- **The staging-room layout is MIRRORED** (x → 49 − x around the rally) so the corridor gap /
  swamp band / bunker gap lies between the rally and the exit the squad must reach; the single-room
  layouts assume a west approach, and the target keeps that convention by putting the staging room
  to the target's WEST (the crossing is the target's west edge). Pinned.
- **`MultiRoom::decode(index) -> MultiRoomCase`** is the public, pure index → case map (the
  "layout pair, ForceSpec, seed" decode), so a sweep can label or filter by case without
  generating the world.

## Landed
- `12b19c0` (eval) / `8189383` (super) — Phase A: the Generation/Evaluation/Validation seams, calibration re-landed behavior-identically.
- `0d67830` (engine) / `c56ad6e` (agent) / `5427dff` (eval) / `dafcd73` (super) — Phase V: per-frame room + the interactive HTML replay player.
- `9139b8f` (eval) — Phase B, single-room half: terrain-rich `Layout`s (`Corridor`/`SwampApproach`/`Bunker`), the `Designed` fixtures (incl. the twin-room siege) and the `Permutations` enumerator.
- `978f92d` (eval) — Phase C: `ForceSpec` opponent forces + the `ManagedSquadIntegration` traversal lens.
- `2fbf5a5` (eval) — Phase D: the `SizingWins` validator + the replay dashboard (`report::write_dashboard`, the contact-sheet index).
- `6dd6bfb` (eval) — Phase F: `ForemanGenerator` (foreman-planned realistic bases over real terrain; ADR 0025 §12 Stage 3).
- `287a689` (eval) — Phase G: `ImportedRoom` (captured-room fixtures × objective kinds × comps, single + multi-room; ADR 0025 §12 Stage 2).
- WS-CLOSE 2026-09-07 batch (eval; SHA on the parent's commit) — Phase B, multi-room half: the `MultiRoom` generator (`generate.rs`), its pins (determinism, seam staging, builder aliasing, calibration assessability, mirrored staging layout, twin-index crossing) + the `multi_room_traversal_sweep` dashboard; the border ROUT bed (`stronghold::run_border_rout`, ADR 0023 cross-room Flee squad side).

### WS-VAL corpus write-back (2026-09-07 — the stronghold / border / boosted lanes, as built)

Written back from the WS-VAL implementation doc (docs/implementation/README.md rule 5) and verified against
`screeps-combat-eval`. These lanes are `Designed`-class generators + `SelfPlay`-class validators in
this ADR's terms, specialised to the operator's 2026-08-23 directive ("a test corpus that matches real
invader strongholds and boosted creeps self play … multi room and challenging room layouts … make
sure the live code uses all the same behavior as simulation").

- **Stronghold corpus (`harness/stronghold.rs`).** Transcribed from the canonical engine sources,
  not invented (module doc carries the citations): bunker1–5 templates with exact structure offsets
  and the full rampart blanket (`screeps-common/lib/strongholds.js`), rampart hits 100K/200K/500K/1M/2M
  (`RAMPART_HITS`), core 100K (`CORE_HITS`; dismantle-immune in the engine model), the exact defender
  bodies WITH boosts (`invader-core/stronghold/creeps.js` — T2 `UH2O`/`KHO2` defenders/rangers at L4,
  T3 `XUH2O`/`XKHO2`/`XZHO2` melee/rangers at L5, the XLH2O fortifier), per-level populations as
  seeded draws from the engine's deck (`stronghold.js`), tower AI `focusClosest` (L1–3) / `focusMax`
  (L4–5) in `stronghold_tower_intents`, and core-refilled towers modeled as a 100K pool per tower
  (`TOWER_ENERGY`) so drain is honestly non-viable. `StrongholdScenario::build(level, terrain,
  multi_room, seed)` spans level 1–5 × `StrongholdTerrain::{Open, Chokepoint}` (procedural caves,
  connectivity-verified) × single-room / multi-room (the attacker stages in the east neighbour and must
  cross a border whose open columns are verified passable on both edges). `run_stronghold_assault`
  is ORACLE-SIZED: `optimize_composition(DoctrineObjective::KillImmuneStructure, …)` with the
  attacker's `boost_max_tier` as the supply clamp (T0 / T3), breach hits derived from the built world,
  the comp placed by `validate::place_at_entry` at its stamped tier and driven by the real managed
  brain (`ManagedSimSquad`, `Destroy`), defenders = the population under `Hold` — so a rung grades the
  whole pipeline, sizing through tactics. `RungOutcome` = `Killed{ticks}` / `Deferred` (the oracle
  refused — honest for what one squad cannot take) / `Unfieldable` / `AttackerWiped` / `Timeout{reached}`.
- **The honest-verdict rule.** `Killed` means the OBJECTIVE was razed (`ObjectivesDestroyed`): the
  runner has no `SideWiped(defender)` stop, because killing the camper creeps is not taking the
  stronghold (towers + core still stand). The earlier defender-wipe stop mis-scored L2 rungs as
  `Killed{~20}` when the lone camper died 13 tiles from the core; removing it exposed two real
  tactical defects that were then fixed (the stall clocks accruing through the march; the
  out-of-contact rigid-body park — ADR 0025/0035 write-backs).
- **Border gauntlet (`BorderGauntlet::build(grade, seed)`).** The distilled "picked off moving in and
  out of rooms" fear: a bare core in a chokepoint room, a camper pack parked 2–4 tiles inside the
  arrival edge bracketing the open border columns — grade 1 = 2 unboosted rangers, 2 = 4, 3 = 4 T2
  boosted rangers + 2 T2 boosted melee, 4 = 6 T3 `fullBoostedRanger`s. Same runner, same verdicts.
- **Boosted self-play lane (`tournament.rs`).** `boost_body(body, tier)` (uniform per-part boost via
  `screeps_sim_core::BodyPartDef::boosted`; `BoostTier::None` ⇒ byte-identical unboosted build);
  `boosted_comp_basket(n, energy, tier)` = `comp_basket`'s exact seeded comps and beds, boosted — so a
  tier sweep isolates what boosts change, not a comp reshuffle; `build_bed_bodies`/`play_bed_bodies`
  take per-side pre-built bodies (tier-asymmetric matches); `payoff_over_boosted_comps` is the
  antisymmetric payoff over such a basket. NB `comp_basket`/`boosted_comp_basket` are SYNTHETIC beds
  only (open field / corridor / tower crossfire); terrain regimes come from `chokepoint_comp_basket`.
- **Checked-in pins (fast, in `cargo test -p screeps-combat-eval`):** template/population ground-truth
  match, chokepoint connectivity, `stronghold_floor_t0_defers_t3_kills_every_l1_rung` (L1 open /
  chokepoint / chokepoint-multi: T0 `Deferred` — the quantified pre-boost capability truth under the
  3000-energy member clamp — and T3 `Killed`; border g1@T0, g1@T3, g2@T3 `Killed` — the bloc crossing
  beats the campers), `border_rout_withdraws_across_the_seam_but_never_reaches_the_rally_yet` (the
  ADR 0023 cross-room Flee squad side, honest baseline), `t3_twin_decisively_beats_unboosted_twin`
  (RED if boost multipliers stop flowing anywhere sim-body → engine → DTO → kernels; pinned against a
  HOLDING T0 twin because an equal-speed fleer is honestly uncatchable). **`#[ignore]` dashboards
  (read, assert nothing):** `stronghold_gauntlet` (every level × terrain × rooms × {T0,T3} + the
  border grades), `write_stronghold_replays` (the operator viewer → `target/replays/stronghold/index.html`),
  `probe_rung` (one rung tick-traced from its recording — the instrument that root-caused the
  cohesion-under-fire and border-crossing defect chains), `boosted_selfplay_dashboard` (default vs the
  0026a catalog per tier), `boosted_tier_retune` / `joint_boosted_terrain_retune` (the ADR 0041 P4
  instruments), `multi_room_traversal_sweep`. Run commands are in `screeps-combat-eval/README.md`.
- **Documented corpus approximations (revisit on evidence, not bugs):** the fortifier's rampart
  REPAIR is unresolved (the sim has no creep-repair intent; the body still fields as the eHP + T3-WORK
  blob it is); defender micro is `ManagedSimSquad` under `Hold`, not the engine's spot-walk
  `coordinated` behaviour; L5's anti-nuke fortify is out of scope; roads and containers are omitted
  (no combat effect — roads only touch fatigue, and combat squads size MOVE for plains).
- **What the corpus established (design-bearing):** the T0 heal ceiling under the member-energy clamp
  cannot out-sustain even one stronghold tower (why live only ever razed towerless cores; boosts are
  the unlock — L1 fields at T3); L2+ defer even at T3 for a single 8-squad — the multi-squad
  operation is the L2+ path (ADR [0048](0048-multi-squad-assault-doctrine.md), Draft); default
  tactics tuned unboosted do not generalize to boosted play (the 0041 P4 re-tune). The
  live↔sim parity audit the corpus triggered is `docs/reviews/live-sim-parity-audit-2026-08-23.md`.
