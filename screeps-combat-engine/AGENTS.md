# AGENTS.md — `screeps-combat-engine`

Guidance for AI (and humans) working on this crate. **Read this before changing any formula or
constant here.** This crate is a faithful port of the Screeps engine's combat tick; correctness is
defined by agreement with the engine, not by what looks reasonable.

## Prime directive

**The cloned engine source at `C:\code\screeps-engine` is ground truth — over docs, over
`docs/references/engine-mechanics.md`, over folklore, over your prior.** Every formula and constant
in this crate was hand-ported from a specific engine file, and its doc comment cites that source
(file + lines). If you change a number, you must be able to point at the engine line that justifies
it. If the engine and a doc disagree, the engine wins and the doc is the bug.

This crate is **not** machine-generated and **not** derived from documentation. It is a hand-port,
pinned by conformance tests. Keep it that way.

## Provenance (what it was ported against)

| Source | Pinned commit | Notes |
|---|---|---|
| `screeps-engine` | **`8097782`** — package **v4.3.2** (2026-06-01) | the processor + intents (combat resolution) |
| `screeps-common` | **`2fb779b`** (2026-04-19) | `lib/constants.js` (powers, ranges, BOOSTS, tower) |
| `screeps-game-api` | **`0a8dd78`** / crate **0.23.1** | the JS-free value types (`Part`, `Position`, `RoomName`) |

When you reconcile (below), **update this table** to the new commit you ported against, and stamp
the date in the changelog at the bottom.

## Engine → code source map

Every load-bearing piece, the engine source it ports, and the conformance test that pins it. This
table IS the reconciliation checklist: when the engine updates, diff exactly these engine files.

| This crate | Ports (engine behavior) | Engine source (`C:\code\screeps-engine` unless noted) | Pinned by test |
|---|---|---|---|
| `constants::{ATTACK,RANGED_ATTACK,HEAL,RANGED_HEAL,DISMANTLE}_POWER`, `BODYPART_HITS`, ranges, `CREEP_*_LIFE_TIME` | the numeric constants | `screeps-common/lib/constants.js` | `body::tests::action_power_*` |
| `constants::RANGED_MASS_ATTACK_FALLOFF` | `RANGED_ATTACK_DISTANCE_RATE {0:1,1:1,2:0.4,3:0.1}` | `constants.js`; applied in `src/processor/intents/creeps/rangedMassAttack.js` | `damage::tests::rma_falloff_matches_engine` |
| `constants::TOWER_*` | tower power/optimal/falloff/energy | `constants.js`; `src/processor/intents/towers/attack.js` | `damage::tests::tower_damage_falloff_matches_engine` |
| `body::SimBody::part_hits` | per-part hits fill **back-to-front** (last part fills first → `body[0]` dies first) | `src/processor/intents/creeps/_recalc-body.js` | `body::tests::part_hits_fill_back_to_front` |
| `body::SimBody::effective_power` | `calcBodyEffectiveness(body, type, method, base)` — sum over **alive** parts of `base × boostMult` | `src/utils.js:623` | `body::tests::action_power_*`, `power_degrades_as_front_parts_die` |
| `body::SimBody::damage_after_tough` | `_applyDamage` — front-to-back boost loop, `damageReduce` accumulated, **single `Math.round`**, then `hits -= damage` | `src/processor/intents/creeps/tick.js:7-29` | `body::tests::{unboosted_takes_full_damage, tough_reduces_within_capacity, tough_capacity_exceeded_spills_unreduced, dead_tough_gives_no_mitigation}` |
| `body::BoostTier::{action_mult, tough_damage_ratio, move_mult}` | `BOOSTS[type][mineral].{attack/rangedAttack/heal/dismantle/damage/fatigue}` (×2/3/4, TOUGH ×0.7/0.5/0.3) | `constants.js` (`BOOSTS`) | `body::tests::action_power_*`, TOUGH tests |
| `damage::tower_amount_at_range` | tower range falloff: full ≤5, linear to 25% ≥20, **floored** | `src/processor/intents/towers/attack.js:32-46` (heal.js/repair.js identical shape) | `damage::tests::tower_damage_falloff_matches_engine` |
| `damage::ranged_mass_attack_damage` | per-target RMA = `round(power × rate[range])` | `src/processor/intents/creeps/rangedMassAttack.js` | `damage::tests::rma_falloff_matches_engine` |
| `resolve::resolve_tick` (two-phase) | accumulate **all** damage/heal in the intent phase, then apply at each object's tick → order-independent | `src/processor.js:227-483`; `src/processor/intents/creeps/tick.js:118-136` | `resolve::tests::*` (all) |
| `resolve` damage-then-heal netting | `hits -= damage` then `hits += heal`, signed, **before** the death check (same-tick heal can rescue) | `src/processor/intents/creeps/tick.js:118-136` | `resolve::tests::kill_inequality_*` |
| `resolve::filtered_actions` | intent priority/exclusion: rangedAttack dropped if RMA queued; melee attack dropped if heal/rangedHeal present | `src/processor/intents/creeps/intents.js:3-31` | (covered indirectly; add a direct test when extended) |
| `resolve` Attack/RangedAttack/Heal/RangedHeal ranges + power | range gates 1/3/1/3; power from `effective_power` | `src/processor/intents/creeps/{attack,rangedAttack,heal,rangedHeal}.js` | `resolve::tests::*` |
| `resolve` melee attack-back | target's ATTACK parts hit a melee attacker (rampart-exempt — ramparts not yet modelled) | `src/processor/intents/_damage.js:14-19,86-91` | `resolve::tests::melee_attack_back_hits_the_attacker` |
| `resolve` safe-mode gate | a hostile's combat vs the safe-mode owner's objects is zeroed | per-intent guard in `creeps/*.js`; `src/processor/intents/controllers/tick.js` (activation) | `resolve::tests::safe_mode_zeroes_hostile_combat` |
| `resolve` tower fire + energy | tower fires once for `TOWER_ENERGY_COST` energy, range falloff | `src/processor/intents/towers/attack.js` | `resolve::tests::tower_drain_self_heal_survives_and_burns_energy` |

## Reconciliation procedure (when the engine updates)

Run this whenever `C:\code\screeps-engine` is updated, or whenever a live conformance vector starts
failing (the canary that an engine change drifted us).

1. **Capture the new version.**
   `git -C C:\code\screeps-engine rev-parse --short HEAD` and the `package.json` version; same for
   `screeps-common`. Record them.
2. **Diff exactly the files in the source map** between the pinned commit and the new one:
   ```
   git -C C:\code\screeps-engine diff 8097782..HEAD -- ^
     src/processor.js ^
     src/utils.js ^
     src/processor/intents/_damage.js ^
     src/processor/intents/creeps ^
     src/processor/intents/towers
   git -C C:\code\screeps-common diff 2fb779b..HEAD -- lib/constants.js
   ```
   (Replace `8097782`/`2fb779b` with the currently-pinned commits in the provenance table.)
3. **For each changed formula/constant**, update the corresponding Rust **and its doc-comment line
   reference** (the cited `file:lines`). If a new combat mechanic appears (a new intent, a changed
   exclusion rule, a new boost), port it with a new test.
4. **Re-run the conformance tests:** `cargo test -p screeps-combat-engine`. These are the cheap
   tripwire; a failure localizes to the formula whose test broke.
5. **Re-capture the server golden vectors** (once they exist — `tests/conformance/`) and confirm
   the sim is byte-exact against the live engine. This is the *strongest* check and catches drift
   the hand-written unit tests don't anticipate.
6. **Update the provenance table + the changelog** at the bottom of this file with the new commits
   and date, and note what changed.

## Porting conventions

- **Cite the source.** Every public formula's doc comment names the engine file (+ lines) it ports.
  No exceptions — that citation is what makes step 2 above mechanical.
- **Match arithmetic exactly,** including `Math.round` vs `Math.floor` and *where* it happens.
  `_applyDamage` rounds the accumulated reduction **once** at the end (not per part); tower amounts
  are **floored**; RMA per-target damage is **rounded**. Off-by-one drift here compounds over a
  fight and fails parity.
- **Integer where the engine is integer.** Hits, damage, energy are integers; intermediate boost /
  falloff math is `f64`, converted at the documented rounding point.
- **Two-phase is sacred.** Never apply damage at the moment an attack intent is processed — that is
  the single most common way a naive combat sim diverges. Accumulate into pools, then net at apply
  time. The apply order across objects must not affect outcomes (it doesn't, because pools are
  complete first — keep it that way).
- **Boosts as `BoostTier`,** not mineral `ResourceType`s. The engine's three tiers per part type
  map exactly onto T1/T2/T3 multipliers; the live `CombatView` adapter (P2.H2) maps
  `ResourceType → BoostTier` at ingest. If you add a boost effect, add it to `BoostTier`.

## Determinism (non-negotiable)

No RNG, no wall-clock, no network, no global mutable state. **No `HashMap` iteration order may reach
an outcome** — iterate `CombatWorld::creeps` (stable `Vec` order); the per-target pools are keyed by
creep id and only *looked up*, never iterated for decisions. Same `(CombatWorld, Intents)` ⇒ same
result. This is what makes seed-reproducible N-seed combat gates (ADR 0015) possible; breaking it
breaks the whole point of the sim.

## What is NOT modelled yet (do not assume it works)

Add these in the documented next slices; until then they are absent, not broken:
- **Movement** of any kind — same-tile conflict resolution (`movement.js` `rate1..rate4`), pull,
  fatigue *accumulation*, room-edge crossing. (Creeps hold position; `fatigue` regen runs but
  nothing adds fatigue.) This is the next slice and where kiting/cohesion fidelity comes from.
- **Structures as damage targets** — ramparts, walls, spawns; **dismantle**; tower **heal/repair**;
  the rampart exemption for melee attack-back.
- **NPC AI** (Source Keepers, invaders, invader cores), power creeps/effects, multi-room.
When you add one, extend the source map table, add conformance tests, and update the README status.

## How to extend

- **A new creep action:** add a `CombatAction` variant, handle it in `resolve_tick`'s phase B (with
  the correct range gate + power source + safe-mode gate), add it to `filtered_actions` if it
  participates in the priority/exclusion table, cite the engine intent file, add a test.
- **A new structure target / dismantle:** add a `SimStructure`/target type to `state`, a damage pool
  keyed appropriately, and the `_damage.js` shield/rampart rules (FORTIFY/INVULNERABILITY skip).
- **A new boost effect:** extend `BoostTier` and the relevant `body` multiplier; verify against
  `constants.js` `BOOSTS`.

## Relationship to the bot

The bot kernel `screeps-ibex/src/military/damage.rs` is a *sizing heuristic* (defender/attacker part
counts); this crate is *exact tick resolution*. They overlap only on the tower falloff, which is
**kept identical on purpose** — if you change one, change both (or, better, the eventual shared
extraction). The bot's combat *decision* code is driven against this engine via the P2.H2 trait
seam; do not duplicate tactics here (this crate has no tactics — it only resolves the engine tick).

## Changelog

- **2026-06-17** — Initial port (P2.H1): constants + body + damage + state + the two-phase
  stationary-combat resolver. Pinned engine `8097782` (v4.3.2), common `2fb779b`,
  screeps-game-api `0.23.1`.
