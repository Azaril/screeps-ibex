# screeps-combat-engine

A **deterministic, JS-free port of the Screeps combat tick** — the *mechanism* layer of the combat
micro-simulator (ADR [0006](../docs/design/0006-eval-and-iteration-harness.md) Part B). It models a
single 50×50 room of creeps + structures and resolves combat **exactly as the real Screeps engine
does**, so the bot's own combat decision code can be exercised against it in milliseconds —
deterministically, with full introspection, no Docker, no server, no JavaScript.

> **Why this exists.** Combat in Screeps is unmeasurable on a live server (concurrent ticks,
> non-reproducible, minutes per run). This crate lets us iterate on squad tactics
> (ADR [0008](../docs/design/0008-combat-and-squad-architecture.md) /
> [0008a](../docs/design/0008a-combat-tactics.md)) with `cargo test`-speed turnaround and exact,
> seed-reproducible outcomes — the harness-first foundation of the combat overhaul (phase
> [P2.H1](../docs/execution/phase-2.md)). The Dockerized private server stays as the *fidelity
> oracle*; this is the *per-change loop*.

## How it was built (provenance)

This is a **hand-port from the cloned engine source at `C:\code\screeps-engine`** — **not** from
documentation, and **not** machine-generated. Every formula cites the engine file + lines it ports
(in its doc comment), and is pinned by host conformance tests against hand-computed engine values.

It was ported against:

| Source | Pinned version |
|---|---|
| `screeps-engine` | `8097782` — package **v4.3.2** (2026-06-01) |
| `screeps-common` (constants) | `2fb779b` (2026-04-19) |
| `screeps-game-api` (value types) | `0a8dd78` / crate `0.23.1` |

**When the engine updates, follow the reconciliation procedure in [`AGENTS.md`](AGENTS.md)** — it
carries the full engine→code source map and the step-by-step re-verify checklist. That file is the
first thing to read before changing any formula here.

## Status (P2.H1, in progress)

- **Done:** the combat-math kernel (`constants`, `body`, `damage`) + value types (`state`) + the
  **two-phase stationary-combat resolver** (`resolve`): creep actions + tower fire accumulate into
  per-target damage/heal pools, then net **damage-then-heal** with the death check. 16 host
  conformance tests; host + wasm32 compile; clippy-clean.
- **Next:** same-tile **movement-conflict resolution** (`rate1..rate4` / pull — where kiting and
  cohesion bugs live), structures as damage targets (ramparts/walls/spawn) + dismantle + tower
  heal/repair, `CombatRecording` (per-tick replay artifact), and the **server-captured golden
  vectors** that mark P2.H1 *done* (byte-exact vs the live engine).

## Quick start

```rust
use screeps_combat_engine::{CombatWorld, SimCreep, Intents, CombatAction, resolve_tick};
use screeps_combat_engine::body::SimBody;
use screeps::{Part, Position, RoomCoordinate, RoomName};

let room: RoomName = "W1N1".parse().unwrap();
let at = |x, y| Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room);

let mut world = CombatWorld {
    creeps: vec![
        SimCreep { id: 1, owner: 0, pos: at(25, 25), body: SimBody::unboosted(&[Part::Attack; 15]), fatigue: 0 },
        SimCreep { id: 2, owner: 1, pos: at(25, 26), body: SimBody::unboosted(&[Part::Heal; 14]),   fatigue: 0 },
    ],
    ..Default::default()
};

let mut intents = Intents::new();
intents.set(1, vec![CombatAction::Attack(2)]); // creep 1 melee-attacks creep 2
intents.set(2, vec![CombatAction::Heal(2)]);   // creep 2 self-heals

let report = resolve_tick(&mut world, &intents);
// report.outcomes[&2] carries raw/effective damage + heal; report.deaths lists ids that died.
```

Run the conformance tests:

```
cargo test -p screeps-combat-engine        # host (the `test-host` lane)
cargo check -p screeps-combat-engine --target wasm32-unknown-unknown   # dual-target rule
```

## Modules

| Module | What it is |
|---|---|
| [`constants`](src/constants.rs) | Combat constants (powers, ranges, RMA + tower falloff, fatigue) transcribed from the engine. |
| [`body`](src/body.rs) | The body model: per-part 100-hit pools, back-to-front degradation (`_recalc-body`), boost-aware power (`calcBodyEffectiveness`), and the TOUGH/boost damage reduction (`_applyDamage`). |
| [`damage`](src/damage.rs) | Range-dependent formulas: rangedMassAttack distance falloff + tower output falloff (kept identical to the bot kernel `military/damage.rs`). |
| [`state`](src/state.rs) | `CombatWorld` / `SimCreep` / `SimTower` value types (JS-free, over `screeps::Position`). |
| [`resolve`](src/resolve.rs) | The two-phase tick: intent priority/exclusion → per-target pooling → damage-then-heal netting → deaths → fatigue regen. |

## Where it fits

```
screeps-combat-engine  (this crate — MECHANISM: the exact combat tick)
        ▲ drives
screeps-combat-agent   (P2.H2, planned — the CombatView/CombatIntent trait seam; adapts the bot's
                        REAL decision code so the sim runs it with no tactics fork → self-play)
        ▲ used by
screeps-combat-eval     (P2.H4, planned — POLICY: scenarios, cohesion metrics, scoring, sim-vs-server
                        parity, replay)
```

- The bot kernel `military/damage.rs` is a *sizing heuristic*; this crate is *exact tick
  resolution*. They are kept identical on the tower falloff so sim and live never disagree.
- Fidelity is bounded against the Dockerized private server (the oracle) via conformance golden
  vectors (per-change) and a nightly parity report (ADR 0006 Part B §4).

## Determinism

No RNG, no wall-clock, no network. Outcomes never depend on `HashMap` iteration order — creeps are
processed in `CombatWorld::creeps` order and per-target pools are keyed by creep id. Identical
`(CombatWorld, Intents)` ⇒ identical result, every time. This is what makes the N-seed combat gates
(ADR 0015) buildable.

## A workspace member, not excluded

Pure logic over `screeps-game-api` value types (the `screeps-rover` / `screeps-foreman` profile),
no host-only deps — so it is a workspace **member** that builds both targets and inherits the
workspace `screeps-game-api` patch. (ADR 0006 §B.1 originally said "excluded"; corrected there.)
