# ADR 0026a — Candidate strategy modes (ideation catalog)

- **Status:** PROPOSED / UNVALIDATED (2026-06-26). Companion to [ADR 0026](0026-combat-strategy-selection.md). These are ideation outputs (6-lens proposal → dedup → exploitability/testability vet), NOT yet tournament-validated on the (bug-fixed, bit-deterministic) sim. The validation phase prunes this list and records measured results before any wire-in to the `CombatStrategy` registry.
- **Prereq:** the edge-exit two-creeps-on-a-tile engine fix must land first (it changes cross-room/base-attack dynamics); then re-tune the general profiles; then validate each situational mode on its target bed under the `exploitability` ship-gate.

`KernelParams = {approach, incumbency, discohesion, cohesion_k, spacing}`. Current shipped profiles: `default {2,3,10,3,1}`, `open_combat {1,6,20,2,1}`, `breach {1,4,10,3,1}`, `breach_drain {1,6,10,3,1}`.

## Catalog (ranked by the vetting agent)

| # | Mode | Vector `{a,i,d,K,s}` | Activator (class + NEW signal) | Beats | Exploit risk |
|---|------|----------------------|--------------------------------|-------|--------------|
| 1 | `ranged_duel_kite` | `{0,3,14,3,2}` | OpenCombat + `enemy_has_ranged` (hostiles ranged & we're ranged-led) | `open_combat` in a ranged mirror — its `i6` freezes us in a flat risk band at range 1-2; lower incumbency + spacing re-optimizes to the RMA-safe outer edge | **LOW** (strictly lower-commitment than open_combat) |
| 2 | `anti_aoe_spread` | `{1,5,6,4,5}` | Open/Breach + `aoe_pressure` (RMA-capable hostile near centroid OR ≥2 energized towers cover the tile) | `open_combat`/`breach` vs RMA or multi-tower — current tight cohesion packs the blob into one RMA / overlapping tower footprints; `spacing 5` de-stacks | MED-LOW (concedes focus density — gated to actual AoE) |
| 3 | `focus_ball` | `{2,4,28,1,0}` | OpenCombat + `single_target_threat` (no RMA/tower) + `outnumber_ratio ≥ 1.3` + melee-led | `open_combat` in a no-AoE brawl we outnumber — `spacing 0`/`K1` packs combined fire to delete ONE creep/tick before spilling (Lanchester convexity) | **MED** — `spacing 0` is a self-inflicted kill-box if the no-AoE gate is wrong; safety rests entirely on the gate |
| 4 | `anti_kite_chase` | `{5,1,6,4,1}` | OpenCombat + `enemy_kiting` (focus hostile faster, ranged, net-receding) | `open_combat` vs a faster shoot-and-scooter it never catches (stalemate-disengage, 0 kills); `approach 5` walks the gap down | **HIGH** — canonical lure-into-kill-box. Ship only the `in_our_territory`-gated variant; test as the exploit target first |
| 5 | `defensive_hold` | `{1,10,14,2,2}` | OpenCombat + `in_our_territory` (we own/reserve the room) | `default`/`open_combat` when an attacker pokes-and-retreats to bait us off our ramparts; `i10` plants the firing line on the choke | **LOW** (anti-over-extension by construction; pairs with rampart damage-redirect) |
| 6 | `drain_spread` | `{1,6,10,4,4}` | Breach + `Drain` + `multi_tower_crossfire` (≥2 energized towers overlap the staging tile) | `breach_drain` on a 2+ tower base — it co-clusters the soak tank + waiting squad in the 600-dmg band; `spacing 4`/`K4` keeps non-tank members OUT of tower range | LOW (structures, not creep-exploitable) — needs a tower-drain bed |
| 7 | `drain_breach_handoff` | `{3,4,10,3,1}` | Breach + `Drain` + `towers_drained` (cumulative soak exhausted tower energy) | `breach_drain` on the transition tick — it stays in SOAK posture after towers empty; `approach 3` rushes the now-undefended ring before refill (hot approach is safe *only* here — the towers that punished it are empty) | LOW-MED (signal-timing risk: early flip → dash into live towers) |
| 8 | `safe_mode_countdown` | `{1,8,14,2,1}` | Breach + `enemy_safe_mode` + `safe_mode_ticks_remaining ≤ 50` | `SafeModeHold` (which over-retreats to the kite standoff) — pre-stage a tight blob at the gap mouth so dismantle resumes at range 1 the instant safe mode lapses | LOW (base invulnerable while staging; survival-veto guards the post-lapse tile) |

## New signals the bot must compute (not yet wired)
`enemy_has_ranged`, `aoe_pressure`, `single_target_threat` + `outnumber_ratio`, `enemy_kiting`, `in_our_territory`, `multi_tower_crossfire`, `towers_drained` (force_sizing already estimates this), `safe_mode_ticks_remaining`. Most are cheap body-part / tower / ownership reads; `enemy_kiting` reuses `threat_step_ticks`.

## New beds/scenarios the validation phase needs
- ranged-vs-ranged mirror comp (modes 1) and RMA-heavy comp (modes 2-3) for self-play.
- `outnumber_ratio ≥ 1.3` melee-led, no-AoE comp (mode 3).
- speed-asymmetric receding-kiter opponent (mode 4).
- defender-owns-the-room scenario with a feinting attacker (mode 5).
- tower-energy-bounded Drain base with ≥2 towers + a `towers_drained` flip (modes 6-7) — the ADR already flags this bed as not-yet-landed.
- safe-mode base with a scripted mid-scenario expiry (mode 8).

## Validation gates (per mode, before wire-in)
1. Beats the relevant current profile on its **target bed** (the measured win it claims).
2. `exploitability ≤ 0` vs the self-play field (or ≤ the profile it replaces) — the ship-gate; especially `focus_ball` and `anti_kite_chase`, run as the exploit *target*.
3. No regression on the existing `per_objective` + determinism fences.
