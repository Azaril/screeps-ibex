# ADR 0036 — Opportunistic Structure Targeting (threat × value)

Status: PROPOSED (2026-06-30) — **Phase S1 (D1+D2+D3+D4+D5) DONE** (decision `64aee70`, agent `7bdf1fd`, eval `b63c7fa`, super; no WFV bump). D1 `struct_target_value = strategic_value + threat_removed` (energized tower's real DPS via `tower_attack_damage_at_range`); D2 focus-selection consumes that one order (deleted `structure_rank`, which had ranked a live tower below a spawn); D3 `resolve_focus` keeps a Structure target as `(pos, None)` so the job focus-fires it; D4 `should_drop_anchor_for_structure_siege` (scoped to `focus.id.is_none()`) drops the anchor like DRAIN so the approach gradient closes to range. **D1/D2 + the raze are sim-proven (decision/eval); D3/D4 are bot-crate and STRUCTURALLY UNREACHABLE by the eval (it doesn't depend on the bot crate; ManagedSimSquad is anchorless) — verified by code-trace + the LIVE soak (the RC-11 sim-gap class).** **D3/D4 now RELIABLY OFFLINE-PROVEN** (3 deterministic bot-crate tests, each RED-proven by reverting the change: `resolve_focus` keeps `(pos,None)` for a Structure; the D4 predicate; a real `apply_squad_decision` Engaged-arm test asserting `squad_path=None` + `attack_target=Structure(pos)`, with a creep-focus control). **OPPORTUNISTIC-EVERYWHERE is ALREADY covered by S1** (diagnosis definitive): Phase B2 runs `compute_squad_orders` for *every* managed squad with the room's full hostile-structure list + a uniform `engage_objective=Destroy`, and `decide_squad → select_focus_target` picks a structure focus UNCONDITIONALLY (no doctrine gate) — so any engaged squad (defense/SK/offense) razes a hostile structure when there's no better creep target. **So S2 = just D6 (salvage routing), plus confirming the offense-Dismantle objective is emitted+fielded for these cores.** REACH note: the soak "no squad reached a core" was an objective-FIELDING mismatch, not a raze failure — an undeployed no-creep core has no hostile creep → `hostile_warrants_defender=false` → no Secure emitted there; the core routes to the offense `Dismantle` arm (war.rs:1386). **Remaining: S2 (D6 salvage routing + confirm offense-Dismantle fielding) + the live raze confirmation (blocked by private-server world mechanics — see [[war-lifecycle-debug]]).**
Cross-refs: ADR 0019 (combat position selection), ADR 0020 (EV target/kill calc),
ADR 0025 (EV position×action kernel), ADR 0026 §9 (the "enters but does nothing"
invader-core bug), ADR 0031 (capability-driven force composition / dismantle weapon
select), ADR 0034 (rally/travel convergence), ADR 0035 (scout-before-commit /
abandon-on-contact).

## Problem

A force-sized squad REACHES a winnable invader-core room (e.g. W5N3, offense-scored
102.3) but never RAZES the core: 0 hits damaged across all cores; a bare core in the
`Engaged` state takes 0 damage; the room stays 100000/100000.

Operator directive: "we should attack structures opportunistically but weight them by
threat/value to take out." Still fight creeps when present, but a structure-only room
must get razed; pick WHICH structure by threat (towers — what shoots us) + value
(the core/objective, spawns).

## Confirmed root cause (the squad deals 0 structure damage)

The kernel and the per-creep firing pipelines are BOTH correct in isolation — the break
is in the LIVE WIRING between them. Three real gaps, in order of decisiveness:

### G1 — the kernel's structure-attack intents are computed then DISCARDED (primary)

`plan_squad_ev` (`screeps-combat-decision/src/kernel.rs:339-430`) prices the core into
the shared `dmg` ledger (kernel.rs:376-392; a focus structure gets
`(700+600)×4 = 5200`) and emits `CombatIntent::RangedAttack`/`Attack` against it in
`decision.member_intents`.

But `apply_squad_decision` (`screeps-ibex/src/military/squad_manager.rs:2734-2759`,
the `Engaged` arm) consumes only `decision.member_goals` (movement, line 2744-2750)
and the focus/heal assignments. **It NEVER reads `decision.member_intents`.** The
kernel's structure-attack intents are dropped on the floor. The live managed creep's
firing therefore does NOT come from the kernel at all — it comes entirely from the JOB
running `decide_combat` per creep.

### G2 — the job drops the STRUCTURE focus, so firing is undirected/opportunistic-only

In the job, `execute_combat_via_seam` builds the per-creep `CombatView.focus` via
`o.attack_target.and_then(|t| t.resolve_creep())`
(`screeps-ibex/src/jobs/squad_combat.rs:528` and `:627`). `resolve_creep()` is `None`
for a `AttackTarget::Structure(pos)` (the comment at :526-527 says so). So for a core
objective the live creep enters `decide_combat` with `focus = None`.

`decide_combat` still WILL hit the core via the in-range structure fallback
(`attack_with_orders` lib.rs:581-583/602-604, `fallback_attack` lib.rs:612-621), but
ONLY opportunistically — there is no DIRECTED focus pulling fire onto the chosen
structure, and no kernel-driven approach. Firing therefore depends entirely on the
formation happening to place the creep within range 3 of the core.

### G3 — reach in the FORMATION phase (the ADR 0026 §9 failure mode)

In the formation/anchor phase the creep takes `TickMovement::Formation`
(`squad_combat.rs:972` `execute_formation_movement`), moving to its slot tile derived
from the anchor `virtual_pos`. The anchor is advanced to `standoff_one_tile(core, center)`
(`squad_manager.rs:2502-2506`) precisely because advancing onto the impassable core
tile pathfinds to range 0, finds no path, reports `Blocked`, and parks the squad SHORT
of weapon range — the documented "enters but does nothing" bug (ADR 0026 §9, cited at
squad_manager.rs:2497-2500). The one-tile standoff is a band-aid; whether the slotted
members actually land within range 3 of the core depends on the layout offsets, and the
kernel's own approach gradient (which WOULD close to range) is bypassed in the
formation phase (`member_intents`/`member_goals` are inert with an anchor set — see the
DRAIN follow-up note at squad_manager.rs:2515-2529, which only drops the anchor for the
DRAIN directive, not for a normal structure siege).

### Threat/value gap (the operator directive)

There is no single score combining tower-threat + structure-value to choose WHICH
hostile structure to raze first. Two parallel, hardcoded orderings exist and never meet:
- `kernel::struct_kind_value` (kernel.rs:254-261): InvaderCore 700 / Tower 600 /
  Spawn 500 / else 0 — a fixed kind value; a tower's ACTUAL dps is not folded in.
- `lib::structure_rank` (lib.rs:253-260): InvaderCore 0 > Spawn 1 > Tower 2 > else 10 —
  a fixed rank used by `select_focus_target`'s fallback and the per-creep
  `best_hostile_structure_within`.

So a tower (which is literally what shoots us) is valued BELOW a spawn by `structure_rank`
and below the core by `struct_kind_value`, and neither reflects how much incoming damage
razing it removes. The threat field tracks `hostile_tower_positions`/`tower_energy`
(threatmap) and `force_sizing::tower_dps_at_assault`, but that channel feeds force-sizing
only — it never reaches the target-priority decision.

## Design — opportunistic structure targeting weighted by threat × value

Principles:
1. **One target set, one currency.** Creeps and structures already share the kernel's
   `dmg` ledger in the `g_them·value` currency (kernel.rs:361-392). Keep that. The fix
   is to make structures enter the set with a value that reflects threat (incoming
   damage removed) + strategic value (the objective), and to make the LIVE bot actually
   consume the kernel's structure intents and approach.
2. **Creeps usually first, but a structure-only room still gets razed.** When killable
   hostile creeps exist they out-value structures (a live threat × ttk dominates), so
   the ordering naturally fights creeps first; once they're gone (or there never were
   any), the structure entries are the highest-value targets and the squad razes them.
3. **Functionally pure in the kernel.** The threat/value weighting and the
   target-priority math live in the pure crate (kernel + lib), unit-tested without the
   engine. The bot adapter only supplies inputs (tower dps, structure hits) and consumes
   intents.

### D1 — a unified `struct_target_value(threat, value)` in the kernel (REPLACES `struct_kind_value`)

Compute a structure's ledger value as **strategic value + threat removed**, both in the
same fighting-strength-removed units as a creep's `threat_value`:

```
value(s) = strategic_value(s.kind)         // core/spawn/objective worth
         + threat_removed(s)               // dps this structure stops doing TO US once dead
```
- `strategic_value`: InvaderCore / objective = high (it is the win condition); Spawn =
  medium; others = 0 unless the breach focus.
- `threat_removed`: for an energized Tower, its `tower_attack_damage_at_range(range to
  our centroid)` (REUSE `screeps_combat_engine::damage::tower_attack_damage_at_range`,
  the same function `assess_engage` and `force_sizing::tower_dps_at_assault` already
  call); 0 for a drained tower or a non-shooter. This makes a live tower out-prioritize
  a spawn (it is what shoots us) without a hand-tuned rank, and folds the EXISTING
  threat signal (tower energy + range) into the value.

This is a pure function on `(CombatStructureDto, centroid)`; the energy/range inputs are
already on the DTO (`energy`) and derivable from `pos`. The FOCUS_STRUCT bonus stays as
the breach-commit multiplier.

### D2 — `select_focus_target` / `structure_rank` consume the same threat×value order

Replace `structure_rank` (the fixed core>spawn>tower order) with a `struct_target_value`
descending sort so `select_focus_target`'s structure fallback (lib.rs:291-296) and
`best_hostile_structure_within` (lib.rs:558-563) pick the highest threat×value structure
— towers-then-core in a towered room, the core directly in a bare room. Creeps still come
first via `ev_target_order` (unchanged); the structure order is the tiebreak/sole-set
when no creeps are killable.

### D3 — LIVE: consume `member_intents` (fixes G1) and direct the structure focus (fixes G2)

- In `apply_squad_decision` (`squad_manager.rs` Engaged arm), stamp each member's
  `decision.member_intents[i]` onto its `TickOrders` and have the job re-emit them
  through `translate_intents` (squad_combat.rs:555-603, which ALREADY handles
  `Attack`/`RangedAttack` against `id:None` structures by position — only `Dismantle`
  is a no-op there). This makes the kernel's coordinated, value-sorted structure fire
  the live behavior instead of the job's undirected fallback. (Alternative, lower-blast-
  radius: thread the structure focus into the job's `CombatView.focus` by resolving an
  `AttackTarget::Structure(pos)` to `FocusTarget { pos, id: None }` instead of dropping
  it via `resolve_creep()` — so `attack_with_orders` focus-fires the chosen structure.)

### D4 — REACH: let the EV approach gradient close on a structure focus (fixes G3)

For a structure-siege Engaged squad (no kiting threat), drop the formation anchor the
same way the DRAIN path does (squad_manager.rs:2527-2529) so the job routes through the
anchorless path and each member moves to its kernel `member_goal`. The kernel's
`dist_to_target` flood is already built from the structure focus (lib.rs:2058-2067) and
the approach term pulls members downhill to weapon range (kernel.rs `best_tile`), so a
squad parked at range >3 closes to range 3 and fires — exactly the bare-spawn test
(`a_melee_creep_breaches_the_focus_structure`, kernel.rs:893) but with the live wiring
honored. Scope to structure focus (`focus.id.is_none()`) so creep formations are
untouched.

### D5 — composition: ensure a weapon that can actually raze

Invader cores are dismantle-IMMUNE (war.rs:1383-1391 maps them to
`DoctrineObjective::KillImmuneStructure`, which zeroes `dismantle_parts` and fields
RANGED — doctrine.rs:223-227). That is CORRECT for cores (ranged damages them). Keep it.
The Dismantle objective carries the target structure's hits so the oracle sizes ranged
kill-parts against the real HP (ADR 0031 R-attack already does this for cores). No
composition change is needed for cores beyond confirming the ranged ceiling sizes to the
core HP; the bug was wiring/reach, not weapon selection. (Dismantle-able rings keep WORK
via `DismantleStructure`.)

### D6 — salvage-vs-offense routing

A derelict-marked invader-owned room must not be admitted by Salvage while a live core
remains (salvage spawns WORK labor + a Declaim objective and never targets the core).
Either exclude such rooms from Salvage eligibility until the core is razed, or have
Salvage emit a Dismantle (ranged) objective for the core tile mirroring war.rs's
producer. This keeps the core on the offense (War) path that fields the ranged razing
force.

## Sim-first plan (RED → GREEN)

Pure-kernel tests (combat-decision, no engine):
1. `struct_target_value` orders an energized tower ABOVE a spawn and below/above the core
   per the chosen weights; a drained tower drops to 0 (RED first against current
   `struct_kind_value`).
2. `select_focus_target(hostiles=[], structures=[core])` returns the core (already passes
   at lib.rs:2307 — keep as a guard); add `structures=[tower, core, spawn]` → focus =
   highest threat×value (tower in range, else core).
3. `plan_squad_ev` bare-core: a ranged member at range 5 from a focus core closes (goal
   range decreases) and, once at range ≤3, emits `RangedAttack{target: core}`
   (extend the existing range-5 approach test + the bare-spawn breach test).
4. Towered room: members prioritize the in-range energized tower's tile, then the core
   (ordering assertion on emitted intents across two ticks).

Engine/harness coverage (combat-eval):
5. A scenario: our force-sized ranged quad vs a BARE level-0 core (no creeps). Assert the
   core's hits strictly DECREASE over N ticks and reach 0 (RED on master — 0 damage;
   GREEN after D3/D4). Add a towered variant: tower hits → 0 first, then core.
6. Reuse the deterministic sim fence (per the memory sim-determinism note) so the raze
   test is bit-stable.

## Interactions

- **ADR 0025 (EV kernel):** the structure ledger entry and `struct_target_value` are the
  natural extension of kernel.rs:376-392; the FOCUS bonus and the position×action
  currency are unchanged. The kernel was already correct (bare-spawn test passes); this
  ADR makes the LIVE bot honor its output (D3) and approach (D4).
- **ADR 0020 (EV target/kill):** `ev_target_order` (creeps) is untouched; structures join
  the SAME `g_them` currency, so creeps-first emerges from values, not a hardcoded phase.
- **ADR 0031 (dismantle composition):** weapon selection for cores (ranged, immune) stays
  as-built; D5 only asserts the ranged ceiling sizes to core HP (R-attack already does).
- **ADR 0026 §9 (enters-but-does-nothing):** D4 is the principled fix for the standoff
  band-aid — the approach gradient closes to range instead of relying on a layout offset.
- **ADR 0034/0035 (rally / abandon-on-contact):** unchanged; this fires only AFTER the
  squad is Engaged and in-room. The `lost_in_room` verdict (squad_manager.rs:2548) now
  correctly sees damage progress on the core, so a winnable bare core is no longer
  mis-read as a stalemate.
- **Salvage routing:** D6 keeps cores on the offense path; the declaim path neutralizes
  the controller only.
- **WFV risk:** D1/D2 are pure-crate (no serialized shape). D3 stamps an existing
  ephemeral `TickOrders` field (no serialized member shape change if `member_intents` is
  threaded through the same non-serialized order channel `member_goals` uses). D4 drops
  the anchor at RUNTIME (the same pattern as the DRAIN/rally/skirmish anchor-drop,
  squad_manager.rs:2526 explicitly notes "no WFV bump"). So the likely WFV impact is
  NONE; confirm by git-diffing the serialized structs before deploy.
