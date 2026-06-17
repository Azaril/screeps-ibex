# ADR 0006 — Eval & Iteration Harness (server-backed substrate + a deterministic combat micro-sim)

- **Status:** Proposed
- **Date:** 2026-06-09 · **Revised:** 2026-06-16 (the server harness is BUILT; added the deterministic combat micro-sim as the combat-iteration substrate; unblocked the military score term)
- **Related:** **IBEX-023** (was "zero automated tests" — now stale, see §Reality update); review §11 (observability/self-improvement), §12 (testing); report §9 (metrics segment + colony-health score); Field Report C (fast repeatable runs for load-shedding) and **Field Report A** (combat scenarios vs. an opponent — the squad-cohesion-rate term); **ADR 0008** (combat overhaul — this ADR is its harness-first prerequisite and hosts the combat sim); ADR [0015](0015-testing-and-validation-strategy.md) (test taxonomy/policy — **division of ownership: 0015 owns the taxonomy and the L5 assertion/flake/gating policy; this ADR owns the harness machinery, the colony-health score, the combat sim, and the pre-deploy gates**); operator brief 2026-06-16 ("a faster way to iterate ... introspect ... a simulation ... without fully replicating the server ... combat is more isolatable").

## Reality update (2026-06-16) — the server harness EXISTS; this revision adds the combat half

The 2026-06-09 ADR described a manual flow to be automated. **That automation is built.** Verified against code:

- **Three crates exist:** `screeps-server-kit` (bot-agnostic mechanism: Docker lifecycle via `bollard`, world bootstrap, deploy via `screeps-pack`, tick control, capture loop, operator CLI), `screeps-ibex-eval` (ibex policy: scenarios, colony-health score, smoke/scenario loop, `compare`), and `screeps-ibex-metrics` (the shared seg-57 **schema contract** crate, a workspace member compiled into both the bot writer and the eval reader so it can't drift). `server-kit`/`ibex-eval` are workspace-EXCLUDED host crates.
- **One command** does `server up → bootstrap --reset → deploy → capture N ticks → gates + colony-health → runs/<scenario>-<sha>-<stamp>/`. The bot writes an always-on versioned seg-57 block every tick (CPU/bucket, GCL/GPL, per-room RCL/energy, fault counters, governor tier, pathing budget, **and combat intent counts+digest**).
- **~520 host tests** across ~69 files exist (`cargo test-host`). IBEX-023's "zero tests" is **closed**.

**What's missing is the entire combat half.** The harness is an economy/survival/CPU instrument only:
- The only scenarios are `smoke`/`pressure`/`reset-under-pressure`/`panic-containment`; the `Fault` enum is `{CpuBurn, GlobalReset, PanicOnce}` — **no opponent, no engagement, no contested terrain.**
- `colony_health()` hard-codes **`let military: Option<f64> = None;`** (`score.rs:100`) — a change that fixes or breaks squads scores **identically**.
- seg-57 carries `IntentMetrics` (attack/heal *counts* + a parity digest) but **no cohesion, in-range, orphan, members-per-squad, or squad-lifecycle field** — squad behavior is uninspectable from any captured artifact.
- The server samples seg-57 every 2 s (~20 ticks at tickMs=100), so even the data it does capture **aliases** engagements, which resolve in a handful of ticks.

So iterating on squad tactics today means: edit Rust → full wasm build → Docker deploy → multi-hundred-tick concurrent-tick run → grep raw console → guess. That is the slow, blind loop the operator wants gone. This revision adds the fast, deterministic, introspectable combat substrate.

## Goals / Non-goals
- **Goals:** repeatable fast runs; automated metric extraction (console + seg-57); a colony-health score (incl. a real **military** term); run-to-run regression detection; **for combat: a deterministic, introspectable, self-play-capable iteration loop** with sim-to-real overfit bounded.
- **Non-goals:** replacing MMO validation; reproducing MMO CPU exactly; a GUI; reimplementing the *whole* engine (only the bounded combat tick is ported — see §combat-sim).

## Part A — the server-backed harness (BUILT; reframed as the fidelity oracle)

The existing `screeps-server-kit` + `screeps-ibex-eval` stack stays as-is and remains the source of truth for **whole-bot, long-running, survival/economy/CPU** behavior and as the **combat fidelity oracle + acceptance gate**. Its machinery is correct and fully reused: mechanism/policy crate split; shared seg-57 schema crate; fault-injection-as-console-flags; hard-zero gates (zero ticks / panic / deser) separate from informational metrics; `runs/<scenario>-<sha>-<stamp>/` artifacts; `compare` regression diff.

**Drift corrected from the 2026-06-09 text** (do not re-propose the superseded plans): deploy is **Rust-native via `screeps-pack`** (not `js_tools/deploy.js` — deprecated); auth/console-ws/code-upload/segment-reads are a purpose-built **`screeps-rest-api`** crate (not the daboross `screeps-api` crate); the layout landed as **`screeps-server-kit` + `screeps-ibex-eval`** (not `screeps-eval`/`tools/eval`). seg-57 is live and versioned (the report's "add seg 57" is done).

### seg-57 schema — combat additions (the schema change that unblocks introspection + scoring)

Add a `CohesionMetrics` block to `screeps-ibex-metrics` (additive, version-bumped per the existing rule; one schema, writer and reader share the crate — registry seam S12):
```
cohesion = { ver,
  squads_active, members_total,
  cohesion_rate,            // fraction of combat ticks with all live members in formation-range (ADR 0006 §military)
  member_spread_p50/p95,    // tiles from squad centroid
  ticks_noncohesive,        // ticks a squad spent below the cohesion invariant
  orphaned_creeps,          // living combat creeps with a dead/None squad (the idle-after-objective signal)
  idle_combat_creeps,       // combat creeps emitting Idle this tick
  time_to_form_up,          // ticks from spawn-complete to first cohesive engage
  targets_killed, waves_spent, own_losses,
  force_abort_latency }      // ticks from non-cohesive/impossible to retirement
```
These are computed both in the sim (Part B) and on the live bot (the same code path), so the **MMO canary** can vouch for the same numbers the sim optimizes.

### Colony-health score — unblock the military term

Replace `military: Option<f64> = None` with a real term from the cohesion/engagement metrics: a blend of **win-rate** (objective achieved), **cohesion-rate**, **targets-killed-per-energy-spent**, and **own-losses**. The blend already renormalizes over present terms, so this slots in with zero structural change. A combat change now moves the score and is caught by `compare`. (The four weighted, survival-dominating terms — SURVIVAL gate, CPU HEADROOM, ECONOMIC GROWTH, MILITARY — and the pre-deploy gates are otherwise unchanged.)

## Part B — the deterministic combat micro-sim (Option C / hybrid) — the combat iteration substrate

**Decision: a fast deterministic Rust combat micro-simulator that drives the bot's OWN combat decision code, fenced behind a `GameView`-style tactical seam, validated against the private server to bound the sim-to-real gap.** The Docker server (Part A) is the *fidelity oracle and acceptance gate*, not the per-change loop. This resolves the operator's fork ("without fully replicating the server", "combat is more isolatable") and his anti-overfit fear.

**Why this is feasible (verified against the engine clone `C:\code\screeps-engine`):** the combat tick is faithfully and cheaply portable to deterministic Rust — every combat action funnels through pure integer formulas (`processor/intents/_damage.js`, `creeps/tick.js`, `towers/attack.js`, `utils.js:623 calcBodyEffectiveness`) with **no RNG**; the only subtlety is the resolution *order* (two-phase accumulate-then-apply; the intent priority/exclusion table `creeps/intents.js:3-31`), which is mechanical to mirror. Combat is *isolatable*: it needs none of economy/market/power/spawn-logistics. And `military/damage.rs` is **already a pure host-tested kernel**, while `screeps-rover` already demonstrates the exact seam pattern (`#[cfg(feature="screeps")] screeps_impl.rs` fences JS; traits take JS-free value types).

> **Why not server-mockup:** community status (2026) — stale (no major release in 12 months) and its documented core risk is exactly the operator's worry: `screepsserver.js` replicates most of `launcher`+`engine` → constant drift. Our sim is *narrower* (only the combat tick, re-captured on engine bumps), *more faithful* for combat (real engine formulas + our own decision code), and *deterministic*. Keep server-mockup rejected.

### B.1 Crate layout (three new crates)

> **Correction (2026-06-17, from P2.H1 implementation):** `screeps-combat-engine` is a workspace **MEMBER**, not EXCLUDED. It is pure logic over `screeps-game-api` value types (the `screeps-rover`/`screeps-foreman` profile) with no host-only deps (tokio/bollard) and no wasm-default to fight, so the exclusion rationale doesn't apply and it inherits the workspace `screeps-game-api` patch + builds both targets. `screeps-combat-eval` (and the server-parity path) stay EXCLUDED where they pull host-only deps; `screeps-combat-agent` will be a member (it adapts the bot's decision code).

```
screeps-combat-engine/   MECHANISM: faithful combat-tick port — CombatWorld (JS-free value types),
                         two-phase resolve, per-part 100-hit pools, damage/heal/tower formulas,
                         fatigue + same-tile movement conflict, ramparts/safe-mode, single 50x50 room;
                         CombatRecording (per-tick snapshot + event log); tests/conformance/ (golden
                         vectors captured from the LIVE server — the fidelity oracle).
screeps-combat-agent/    SEAM ADAPTER: TacticalAgent trait; CombatView (read seam) / CombatIntent (write
                         seam); ibex_agent.rs adapts the bot's REAL combat decision code (screeps-ibex
                         with the "tactical" feature); scripted.rs (rush/kite/turtle/drain opponents).
screeps-combat-eval/     POLICY: CombatScenario; run_engagement(scenario, agent_a, agent_b); cohesion.rs
                         (the metrics, also fed to seg-57); score.rs (the military term); parity.rs
                         (sim-vs-server drift report); replay.rs (SVG/ASCII engagement scrubber).
```

### B.2 The tactical seam — the linchpin that makes self-play real without duplicating logic

Today the per-tick combat decision (`SquadCombatState::tick`) holds a live `Creep`, calls `creep.pos()`/`game::time()`, and writes into `screeps_rover::MovementData<Entity>` — it cannot run in a sim or drive a second side. Refactor it into a pure function over a DTO, following the rover template exactly:
```rust
pub struct CombatView<'a> {           // read seam — a CombatWorld slice for one player, JS-free
    pub tick: u32, pub me: &'a CreepDto, pub squad: &'a SquadStateDto,
    pub friends: &'a [CreepDto], pub hostiles: &'a [CreepDto],
    pub structures: &'a [StructureDto], pub terrain: &'a TerrainDto,
}
pub enum CombatIntent {               // write seam — mirrors the guarded intents
    Attack(ObjectId), RangedAttack(ObjectId), RangedMassAttack,
    Heal(ObjectId), RangedHeal(ObjectId), Dismantle(ObjectId),
    MoveTo { target: Position, range: u8, priority: MovementPriority },
    Flee { from: Vec<Position>, range: u8 }, Idle,
}
pub trait TacticalAgent { fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent>; /* + squad advance */ }
```
**Seam mechanism — trait-first, no cargo feature (operator preference, 2026-06-16).** The combat decision logic is made **generic over the `CombatView` trait** (emitting `CombatIntent`s) — JS-free value types only, no `game::*` call below the seam. Two implementors: a **live adapter** reading `game::*` (the thin per-tick shim, isolated in a leaf module — the rover `screeps_impl.rs` template), and a **sim adapter** backed by `CombatWorld`. There is then **exactly one implementation** of target selection / formation / kite / focus-fire — the sim calls `decide(&sim_view)` directly; the live wasm path calls `decide(&live_view)`. This needs **no cargo feature**: the decision function is pure and always compiles host-side (ADR 0015 verified `screeps-ibex` host-links — only a *runtime* `game::*` call would fault, and `decide` makes none), so the sim crate depends on `screeps-ibex` at the host target and invokes the generic `decide`. A `tactical` cargo feature is held in reserve **only** as a fallback if host-compiling the full bot crate proves too heavy or the JS-leaf isolation too entangled to do cleanly — traits are the default, the feature is the escape hatch. Self-play is `IbexAgent` vs `IbexAgent` (or `ScriptedAgent`). No tactics fork to overfit or drift. Pathfinding is **not** reimplemented: the sim reuses `screeps-rover`'s `PathfindingProvider` (the agent supplies move targets; the sim resolves the move), so sim and live share pathing too.

The extraction is **parity-first**: the first thing the seam must pass is an intent byte-diff (the existing `IntentRecorder` digest) between the live shim and the extracted function on a recorded tick — behavior-preserving by construction before any sim result is trusted.

### B.3 What the sim models vs omits
**Models** (everything tactically load-bearing, from the engine clone, not docs): the **two-phase accumulate-then-apply** resolution (damage+tower pooled, netted damage-then-heal per object at its tick — so simultaneous heal can save a creep); the **intent priority/exclusion table verbatim**; **per-part 100-hit pools + front-to-back destruction + `calcBodyEffectiveness` with boosts** (DPS/heal degrade as a creep is whittled); all damage formulas (melee + attack-back with rampart exemption + safe-mode zeroing, ranged, mass-attack falloff `{0:1,1:1,2:0.4,3:0.1}`, dismantle, heal/ranged-heal, **tower falloff**); **TOUGH/boost reduction** with the single aggregate `Math.round`; **fatigue + same-tile movement conflict resolution** (`rate1..rate4`, pull/swap — *required*, because cohesion/"creep sat idle" bugs live precisely here); ramparts/walls, safe mode, single 50×50 room.
**Omits** (don't affect single-engagement tactics): economy, market, power processing, spawn logistics, terrain generation, history/stats, inter-room global pass, NPC pretick AI, power creeps/effects (add only when the bot must face them).

### B.4 Fidelity validation — the anti-overfit core (three layers, increasing cost)
1. **Conformance golden vectors (per-change, deterministic).** ~12 hand-built micro-scenarios (1v1 melee, ranged-vs-healer, tower-vs-drain, quad-vs-quad, two creeps contesting a tile / mutual swap / pull) run **once on the live private server**, capturing exact per-tick hits/positions/deaths, checked into `screeps-combat-engine/tests/conformance/`. The sim must reproduce them **byte-exact** under `cargo test-host`. This is the tripwire that catches any formula/ordering drift the moment it appears (engine = ground truth, not the doc).
2. **Parity report (nightly).** `parity.rs` runs the *same scenario* through the sim and the Docker server and reports per-tick divergence (positions/hits/deaths/intent stream). A divergence **budget** is tracked and reviewed; the sim score is trusted only within it. This number bounds the sim-to-real gap.
3. **MMO canary (continuous).** The cohesion/orphan metrics the sim optimizes are *also* emitted to seg-57 on the live bot. If the sim says "fixed" and MMO says "still scattering," the parity budget tightens and the missing mechanic is identified — overfitting is caught with live evidence.

### B.5 Introspection / replay (answering "WHY did the squad do that")
Every tick the sim records a `CombatRecording`: full `CombatWorld` snapshot (positions, per-part hits, boosts, fatigue); each agent's emitted `CombatIntent`s **with the reason tag the decision code already branches on** (e.g. `MoveTo{reason:"formation-hold: slot2 out of range"}`, `Idle{reason:"boundary-hold quorum not met"}` — cheap, the code already has these branches); the resolved outcome (who moved, who was blocked by a movement conflict, damage, deaths). On top: **deterministic replay** (re-run old-vs-new tactics on the same scenario+seed and diff recordings — the L3 replay-parity ADR 0015 wanted, realized for combat *now*, years ahead of GameView-real); an **SVG/ASCII engagement scrubber** (formation, focus-fire lines, idle creeps highlighted, click a creep on the misbehaving tick to see its reason tag); the cohesion metrics computed from the recording.

### B.6 Scenarios & opponents
`CombatScenario` is versioned data (terrain/map-slice, seed, `force_a`/`force_b` as explicit bodies+positions OR "ask `IbexAgent` to compose for this threat", structures, safe_mode, victory `{kill_all|hold_room(N)|breach_rampart|survive(N)}`, abort `{cohesion_below(T,N)|wipe}`). Opponents, cheapest first: **scripted built-ins** (rush/kite/turtle/drain — deterministic regression fodder); **self-play** (`IbexAgent` vs `IbexAgent` with asymmetric forces — the cleanest way to surface formation/focus bugs because both sides use identical scrutinized logic); **recorded opponents** (capture a real hostile force from an MMO engagement via the same `CombatView` ingest and replay it — closest to real enemy behavior without overfitting to one sparring partner). This supersedes "deploy a second account as opponent" *for tactics iteration*; the second-account path stays for the nightly server acceptance gate.

### B.7 Scoring & gates
Sim conformance vectors gate per-change as **hard exact** (deterministic, fully owned). Engagement *outcome* gates are **N-seeded paired-seed diffs vs the stored (scenario, seed, SHA) baseline** — and because sim seeds (terrain perturbation, body jitter, start-offset) are **perfectly reproducible** (unlike a concurrent server), the N=9 distributional gate ADR 0015 specified is *buildable for combat for the first time*. The server acceptance gate stays the nightly N-seed confirmation.

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| **Hybrid: server harness (built) + deterministic combat micro-sim driving the bot's own code (chosen)** | fast/deterministic/introspectable per-change combat loop; self-play free (no tactics fork); fidelity bounded by conformance+parity+canary; reuses all existing machinery | the tactical-seam extraction is real refactoring; an engine-port to maintain (re-captured on bumps) |
| Server-only (the 2026-09 plan, combat scenarios on Docker) | maximum fidelity | the slow blind loop; concurrent-tick → non-reproducible seeds → the N-seed gates ADR 0015 mandates aren't even buildable; can't answer "why did this creep idle" |
| Pure sim, reimplement tactics in-sim | simplest to start | creates a second tactics copy to overfit/drift; violates "no duplication"; no fidelity bound |
| `screeps-server-mockup` (in-process JS) | no Docker | stale, version-divergent, replicates launcher+engine (the operator's exact drift fear) |

## Consequences
**Positive:** all four operator requirements met — fast iteration (`cargo test-host`, no Docker/wasm/deploy), introspection (per-tick state + reason tags + scrubber), self-play/opponents (one seam, three opponent tiers), anti-overfit (real code + parity budget + seed diversity + opponent roster + MMO canary). Combat changes finally move the colony-health score and are diffable. The deterministic combat recorder/replay arrives cheaply, ahead of the GameView-real schedule.
**Negative / risks:** the seam extraction touches entangled code (`squad_combat.rs`/`formation.rs` hold `Creep`/`Entity`) — mitigated by strictly parity-first, incremental decision-by-decision, using the proven rover template; engine-port drift over time — caught immediately by the per-change conformance gate and re-captured on deliberate engine bumps; "sim says fixed, MMO disagrees" — that *is* the canary working (tighten the budget, find the missing mechanic; operator owns acceptance).
**CPU / tick-safety:** the sim is host-only (no MMO cost). The bot-side change is the `game::* → CombatView` shim (one build per tick) feeding the same `decide()` — no extra intents; the live combat path is unchanged in cost.

## Incremental Build Path
- **Inc A — engine port + conformance (trust foundation).** `screeps-combat-engine` (state, body pools, two-phase resolve, formulas aligned with `military/damage.rs`); capture ~12 golden vectors from the live server; sim reproduces byte-exact. *Gate:* conformance exact.
- **Inc B — tactical seam + parity-first extraction.** Make the first decision (target selection + formation advance) generic over the JS-free `CombatView` **trait** (no cargo feature — §B.2); live adapter over `game::*` in an isolated leaf, sim adapter over `CombatWorld`; prove live-vs-extracted intent byte-diff parity; `IbexAgent` wraps it. *Gate:* intent byte-diff parity.
- **Inc C — cohesion metrics + military score.** `cohesion.rs`; extend seg-57 additively; replace `score.rs:100`'s `None` with the real military term. *Value:* combat changes visible to the score and seg-57 (sim AND live MMO). *Gate:* metrics round-trip + military-term unit tests.
- **Inc D — scenarios, opponents, self-play, replay viz.** `CombatScenario`, scripted opponents, self-play runner, SVG/ASCII replay with reason tags. *Value:* the full fast introspectable loop. *Gate:* report-only scenario scores, earning gate status per the flake policy.
- **Inc E — server parity harness + nightly acceptance.** `parity.rs`; nightly sim-vs-server divergence; wire named combat scenarios into the server acceptance gate (N-seed). *Value:* the sim-to-real gap is measured and bounded. *Gate:* parity within budget; nightly N-seed server gate.

By **Inc C** the operator has fast iteration + introspection + a moving combat score; by **Inc D**, self-play + visual "why"; by **Inc E**, the anti-overfit loop is closed. **This Inc A–E is the harness-first prerequisite for ADR 0008's behavior steps** (operator sequencing, 2026-06-16).

## Sequencing & cross-ADR ordering
This remains the verification substrate every later increment validates against. The server harness (Part A) is built. **Part B (the combat sim) is the new harness-first work that must precede ADR 0008's combat behavior overhaul** — so cohesion/orphan regressions are caught deterministically per-change. The cohesion metrics this ADR defines are the source for ADR 0008's force-abort/cohesion validation and ADR 0014's posture audit. Test-layer ownership (assertion forms, flake policy, seam registration of `TacticalAgent`/`CombatView`) stays with ADR [0015](0015-testing-and-validation-strategy.md).
