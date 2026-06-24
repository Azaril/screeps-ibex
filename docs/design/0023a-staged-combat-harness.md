# 0023a — Staged combat harness: Generation / Evaluation / Validation (annex to ADR 0023)

Status: **Proposed** (2026-06-24). Operator-directed: *"ensure we have a plan to generate a large
variety of single and multi-room layout permutations … generate random or designed squad or
multi-squad opponent forces, including single and multi-room objectives. Split into generation,
evaluation and validation stages so generation and validation can be swapped. The evaluation just
needs a run-until predicate or condition."*

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
- **`MultiRoom`** — composes per-room sub-beds via `ScenarioBuilder::in_room` (the engine is already
  N-room — ADR 0023 S3 / task P-ENGINE), with objectives carrying their room.

**Opponent forces** — a `ForceSpec` (archetype + count + placement) realized into defender `SimCreep`s:
`Turtle` (HEAL walls), `Rush` (melee), `Drain`, `SiegeDefenders` (ATTACK/RANGED behind ramparts),
`MultiSquad` (several coordinated groups, one per room or layered). Random *or* designed.

### 2. Evaluation — *step until a predicate fires; know nothing about objectives or oracles*
The evaluator optionally **records** every tick (the engine's `record_tick` → `CombatRecording`) so the
same run feeds both validation (the outcome) and visualization (the frames). The recording model is
already rich (`CreepFrame` owner/hits/attack/ranged-power, `StructureFrame` kind/owner/hits,
`TowerFrame` energy/hits, intents + "why" reasons, deaths, destroyed-kinds) — **one gap for multi-room:
frames store only in-room `x,y`, no room** (see §4 / the engine extension).

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
- **`SizingWins`** — the simple "size our real force, field it, did we win?" pass/fail.
- **`Metrics`** — cohesion / positioning / EV (the EXP-register instruments) over the outcome.

**Runner**: `run_suite(&mut dyn Generator, &mut dyn Validator) -> SuiteReport` crosses every scenario the
generator offers with the validator and aggregates. Generation ⊥ validation ⊥ run-until — any triple
composes.

### 4. Visualization — *render a recording (+ metadata) as an interactive, multi-room HTML replay*
The operator-facing **visual validation** layer: turn a `CombatRecording` + scenario metadata into a
self-contained **interactive HTML** replay the operator opens and scrubs, to eyeball both tournament
*outcomes* and the *variety* of generated permutations. **Operator decisions (2026-06-24):**
*interactive HTML player ONLY — no SVG rendering (the agent's `replay.rs` SVG filmstrip is REMOVED)*;
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
  panel. Requires the engine recording to carry the **room** per entity (the one gap — see below).
- **Terrain + buildings**: per-room backdrop (plain/swamp/wall tiles) + **typed** buildings: Spawn
  (filled square), Tower (square + an energy bar that tracks the drain), Rampart (translucent shield,
  opacity ∝ hits), constructed Wall (solid, distinct from terrain wall). Creeps = owner-coloured discs,
  radius ∝ HP, role hinted by part mix, with a per-tick attack/heal flash from the frame intents.
- The runner writes one HTML file per scenario (+ a contact-sheet index linking them) under a run dir;
  the operator opens them to validate outcomes + permutation variety.
- **Reuse `screeps-visual`** (operator 2026-06-24): the backend-agnostic primitives crate
  (`VisualBackend` circle/rect/poly/line + `render_structure`/`structure_primitives` per
  `StructureType`, dep = just `screeps-game-api`). Implement a `VisualBackend` that captures each
  structure type's primitive template once; the player's JS instances the template at every building's
  room position so typed buildings match the bot's own rendering. `combat-eval` adds `screeps-visual` as
  a (host-only) dep.

**Engine extension (multi-room recording).** `CreepFrame`/`StructureFrame`/`TowerFrame` carry only
`x,y`; add the **room** (store the entity's `Position` or a `RoomName`/compact room id) so the
visualizer can place entities across rooms. Additive to `record.rs`; bumps the engine submodule. The
data is already on hand (`SimCreep.pos` is a `Position`); the frame just drops it today.

## The pathing-vs-sizing-purity tension (and how the split resolves it)
The oracle-calibration deliberately grades a **scripted, in-range** siege so a *squad-pathing* gap can't
masquerade as a *sizing* false positive (ADR 0023 caveat). Multi-room objectives inherently need
**traversal** (pathing). These don't fight in the staged model: they're **different validators over the
same generator**. `OracleCalibration` stays sizing-pure (places the sized force in-range per objective,
even in a multi-room world — one engagement per objective). A separate **`ManagedSquadIntegration`**
validator drives the real `decide_squad_with_pathing` across rooms and grades end-to-end (the movement
workstream's gate). Same scenarios, two lenses — exactly what the swap buys.

## Phased build plan
- **Phase A — foundation (extract the seams):** introduce `Scenario`/`Objective`/`Generator`,
  `evaluate`/`RunUntil`/`StopReason`, `Validator`/`Verdict` in `combat-eval`; re-land the *current*
  single-room calibration as `RandomDefendedBase` (generator) + `OracleCalibration` (validator) on the
  new seams — **behavior-identical** (gate still 0 FP / ≤20% FN). De-risks the seams before any new
  variety. (Modules: `harness/{scenario,generate,evaluate,validate,visualize}.rs`.)
- **Phase V — visualization (early, so variety is eyeballable as it grows):** the engine recording
  multi-room extension (room per frame entity) + `replay_to_html`/`replay_to_svg` in `combat-eval`;
  wire the runner to emit a replay per scenario (+ a contact-sheet index). Land it right after the
  foundation so every later phase is visually inspectable.
- **Phase B — layout variety:** rich single-room permutations (wall/rampart/tower/cwall configs, multiple
  breach corridors) + `Designed` fixtures + the `Permutations` enumerator. Multi-room layouts via
  `MultiRoom`. (Now visually validated via Phase V.)
- **Phase C — opponent forces + multi-room objectives:** `ForceSpec` archetypes (random + designed,
  single & multi-squad) → defender creeps; wire `enemy_dps` into the derived profile; multi-room
  objective lists; the `ManagedSquadIntegration` validator for the traversal lens.
- **Phase D — more validators + scale:** `SizingWins`, `Metrics`; widen the seed count / enumerate the
  permutation grid; a report dashboard linking the per-scenario replays.

## Constraints (carried)
Deterministic (SplitMix64 by index — no `Date`/`Math.random`); host-only in `combat-eval`; the engine is
ground truth (the validators judge against `resolve_tick`); generation stays oracle-agnostic so a
generator can feed any validator. Break serialization freely (no persisted state here). No bot→engine
dep (the harness is eval-side, which already depends on engine+decision+agent).

## Cross-refs
ADR 0023 (sim beds), ADR 0022 P-FORCE (the oracle), `combat-eval/src/oracle_calibration.rs` (the Move B
monolith to refactor onto these seams), `combat-agent/src/{objective_bed,scenario}.rs` (`run_siege` /
`ScenarioBuilder` — the evaluate/generate primitives to generalize).
