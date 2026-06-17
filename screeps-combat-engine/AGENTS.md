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
| `movement::resolve_moves` (phase C) | eligibility (`canMove`), same-tile contention (`rate1` swap / `rate4` moves÷weight), obstacle + recursive chain-block (`removeFromMatrix`) | `src/processor/intents/movement.js:11-187` | `movement::tests::*` (contention, swap, chain-block, wall, fatigue) |
| `movement::step` / `dir_delta` / `is_edge` | direction deltas (y down), edge-tile detection | `src/processor/intents/movement.js` (`add`, `isAtEdge`) | `movement::tests::simple_move_to_empty_tile` |
| `resolve` phase D movement+fatigue | apply move, add move fatigue `weight × terrain_rate` (0 on edge), regen `-2 × MOVE` | `src/processor/intents/creeps/tick.js:50,105-108`; `movement.js:235-252` | `resolve::tests::kiting_at_move_parity_takes_zero_melee` |
| can't-dodge-by-moving | attacks resolve on tick-START positions (phase B), before movement | `src/processor.js` (attack intents precede `movement.check`) | `resolve::tests::kiting_at_move_parity_takes_zero_melee` |
| `resolve` structure targets | Dismantle (WORK×50), AttackStructure (melee), RangedAttackStructure; structures take `hits -= damage`, destroyed at 0; **no** redirect of creep damage to a co-located rampart | `src/processor/intents/_damage.js:25-58`; `creeps/dismantle.js` | `resolve::tests::{dismantle_breaches_a_wall, melee_destroys_a_spawn}` |
| `resolve` rampart RMA-shield | RMA skips a non-rampart target on a rampart tile (can hit ramparts directly); single-target attacks still hit a creep on a rampart | `src/processor/intents/creeps/rangedMassAttack.js:38` + `_damage.js:16-21` | `resolve::tests::rampart_shields_creep_from_rma_but_not_single_target` |
| `resolve` attack-back rampart-exempt | a melee attacker standing on a rampart deals no attack-back | `src/processor/intents/_damage.js:17` | `resolve::tests::rampart_suppresses_attack_back` |
| `resolve` tower heal/repair | tower heals a creep / repairs a structure (same range falloff), costs `TOWER_ENERGY_COST` | `src/processor/intents/towers/heal.js`, `repair.js` | `resolve::tests::{tower_heal_keeps_a_defender_alive, tower_repair_outpaces_dismantle}` |
| `record::record_tick` / `CombatRecording` | per-tick replay artifact (pre-tick state + intents + reason tags + outcomes); deterministic id-sorted text dump | *not an engine port* — an introspection harness over `resolve_tick` | `record::tests::*` |

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
- **Pull** (`movement.js` rate2/rate3 — for no-MOVE/under-MOVE comps) and **room-edge crossing**
  (a step off the room is currently blocked, not a transition) and **roads** (fatigue stays
  plain/swamp). Same-tile conflict resolution + fatigue accumulation/regen *are* modelled.
- **Towers as damage targets** (a tower can fire + be attacked in the engine; here `SimTower`
  fires but isn't yet a dismantle/attack target — ramparts/walls/spawns are). FORTIFY/INVULNERABILITY
  rampart effects, power-bank hit-back.
- **NPC AI** (Source Keepers, invaders, invader cores), power creeps/effects, multi-room. (The
  `CombatRecording` replay artifact *is* modelled — see `record.rs`.)
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
- **2026-06-17** — Movement slice: same-tile conflict resolution (`movement.rs`: eligibility/
  fatigue, swap + moves/weight tiebreak, obstacle + chain-block) wired into `resolve_tick` as
  phase C + phase-D move/fatigue application; terrain (walls/swamp) on `CombatWorld`. 24 host tests
  (added kiting + 7 movement-resolution). Same pinned versions.
- **2026-06-17** — Structures slice: `SimStructure` (Spawn/Rampart/Wall) on `CombatWorld`;
  Dismantle/AttackStructure/RangedAttackStructure actions; RMA extended to structures + rampart
  RMA-shield + attack-back rampart-exemption; tower Heal/Repair actions. 30 host tests (added
  dismantle-breach, spawn-kill, rampart-shield, attack-back-suppression, tower-heal, tower-repair-
  vs-dismantle). Same pinned versions.
- **2026-06-17** — Recording slice: `record.rs` (`CombatRecording` / `record_tick`) — per-tick
  replay capture (pre-tick state + intents + optional reason tags + outcomes) with a deterministic
  text dump; `reasons` field added to `Intents` (resolver-ignored introspection metadata). 32 host
  tests. Same pinned versions.
