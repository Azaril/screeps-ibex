# WS-VAL — combat validation corpus (strongholds, boosted self-play, live↔sim parity)

**Workstream:** WS-VAL · **Advances:** ADR 0025 (eval harness), ADR 0041 (boost validation), ADR 0044 (terrain regimes) ·
**Status:** corpus landed; tactical follow-ups queued

## Operator directive (2026-08-23, verbatim intent)

"A test corpus in simulation that matches real invader strongholds and boosted creeps self play …
multi room and challenging room layouts … I remain worried about our ability to move in and out of
rooms effectively without being picked off … fully open layouts are not interesting …
increasingly challenging scenarios to stress test … make sure the live code uses all the same
behavior as simulation."

## What landed

### 1. Stronghold corpus (`screeps-combat-eval/src/harness/stronghold.rs`)

Transcribed from the canonical engine sources, not invented (ground-truth citations in the module
doc): bunker1–5 templates with exact offsets + full rampart blanket
(`screeps-common/lib/strongholds.js`), rampart hits 100K→2M, core 100K (dismantle-immune in the
engine model), exact defender bodies WITH boosts (`invader-core/stronghold/creeps.js` — T2
UH2O/KHO2 defenders at L4, T3 XUH2O/XKHO2/XZHO2 at L5), per-level populations with seeded deck
draws (`stronghold.js`), tower AI focusClosest (L1–3) / focusMax (L4–5), core-refilled towers
(drain not viable — modeled as a 100K pool).

Scenario axes: level 1–5 × terrain {Open, **Chokepoint** (procedural caves, connectivity-verified)}
× {single-room, **multi-room** (staging room east, squad must cross a border whose open columns are
verified passable on both edges)} × attacker boost supply {T0, T3}. Assaults are ORACLE-sized
(`optimize_composition`, `KillImmuneStructure`, breach-hits derived from the built world) and run
the real managed-squad brain — the corpus grades the whole pipeline, sizing through tactics.

Border gauntlet (`BorderGauntlet`): grades 1–4 — a bare core guarded by camper packs bracketing the
entry border columns (2 unboosted → 4 → T2 rangers+melee → 6×T3 rangers) — the distilled
"picked off moving in and out of rooms" fear.

Checked-in (fast) pins: template/population ground-truth match; chokepoint connectivity; the
capability floor `stronghold_floor_t0_defers_t3_fields`. The full ladder is the `#[ignore]`d
`stronghold_gauntlet` dashboard.

### 2. Boosted self-play lane (`screeps-combat-eval/src/tournament.rs`)

`boost_body` / `boosted_comp_basket` (same seeded comps as `comp_basket`, uniformly boosted) /
`build_bed_bodies`+`play_bed_bodies` (per-side pre-built bodies ⇒ tier-asymmetric matches) /
`payoff_over_boosted_comps`. Checked-in pin `t3_twin_decisively_beats_unboosted_twin` — RED if
boost multipliers stop flowing anywhere (sim-core body → engine resolution → DTO adapter →
kernels). Dashboard: `boosted_selfplay_dashboard` (`--ignored --nocapture`).

### 3. The boost-blind seam fix (live + sim — the parity keystone)

Root-caused from the corpus itself: the L1@T3 assault froze because `CombatBodyPart` carried no
boost, so an EV-sized T3 squad read its own heal at ¼ real value → `assess_engage` unwinnable →
perma-retreat. Fix (both sides of the seam, one shared primitive):

- decision: `CombatBodyPart.boost_mult` (+`new`/`boosted` ctors), `CombatCreepDto::effective_output`
  = Σ working parts × per_part × mult; consumers swapped: `heal_reaching`, `threat_value`,
  `best_heal_target`, `assess_engage` dps, `kite_threats`. `bodies::boosts::output_multiplier_for`
  = the exact 18-compound → ×2/×3/×4 map.
- live adapters: `squad_combat.rs::creep_to_dto` stamps the mult from `BodyPart::boost()`;
  `military/squad.rs` heal_power **un-latched** (was computed once and stale forever — parity H4)
  and boosted; `squad_manager.rs` member-view part sums boosted.
- sim adapters: `screeps-combat-agent` `creep_dto` stamps the SimBody tier; managed member views
  boosted; `place_at_entry` fields the comp at its stamped tier.
- RED-verified pins: `effective_output_prices_boosts_and_skips_dead_parts`,
  `heal_reaching_reads_boosted_healers_at_real_strength`, `output_multiplier_map_matches_engine_tiers`
  (decision); `creep_dto_stamps_boost_multipliers` (agent); the T3-twin end-to-end pin (eval).

### 4. Live↔sim parity audit (ultracode, 7 seams, adversarially verified)

Report: `docs/reviews/live-sim-parity-audit-2026-08-23.md` — 43 confirmed findings, 10 HIGH.
Fixed in this batch: H4/M16 (heal latch), H7/M15/M19 (boost-blind seam + harness now fields
boosts). Backlog ranked in the report; top items: **H0** (live silently drops Attack/Dismantle vs
NEUTRAL constructed walls — live squads stall against unowned wall rings the kernel chose to
breach), the **threat/traversal unification cluster** (H1/H2/H3: live executes movement largely
WITHOUT the threat layer the sim validates against), **H5** (roster scope), **H6** (mover config),
**H8** (no-squad tower path fork), **H9** (lifecycle inputs), **M4/M6** (wrong-room matrix).

## Honest results (2026-08-23, post-seam-fix)

### Stronghold gauntlet (oracle-sized, real populations)

| Rung | T0 | T3 |
|---|---|---|
| L1 open / choke | Deferred | **Timeout** (7/8 picked off; lone healer stalemate) |
| L1 choke-multi | Deferred | **AttackerWiped t131** (killed crossing the border) |
| L2–L5 (all layouts) | Deferred | Deferred |

### Border gauntlet

| Grade | T0 | T3 |
|---|---|---|
| g1 (2 unboosted campers) | Timeout | Timeout |
| g2 (4 campers) | Deferred | Timeout |
| g3 (T2 pack) / g4 (6×T3) | Deferred | Deferred |

Readings (each a finding, not a bug in the corpus):

1. **T0 capability ceiling is real and quantified**: with `PREFERRED_MEMBER_ENERGY=3000` clamping
   healer size, the T0 heal ceiling cannot out-sustain even ONE stronghold tower — which is exactly
   why live has only ever killed towerless level-0 cores. Boosts are the unlock (L1@T3 fields).
2. **Cohesion under focused fire is the binding tactical defect**: the L1@T3 sizing is honest
   (3 T3 healers = 1296 heal/tick > 600 tower dps) but only at heal range 1; the squad approaches
   strung out, heals land as ranged-heal at ⅓ power, and focusClosest kills full-hp members serially
   (~5 ticks each). The operator's "picked off" fear reproduced IN-room, not just at borders.
3. **Lone-survivor freeze**: the last member kites to where tower falloff < self-heal and holds
   there forever — no retreat/disband/re-form decision. (Timeout, not a crash.)
4. **Border camping stalls even trivial fights**: g1 — TWO unboosted campers vs an oracle-sized
   squad — times out. Crossing under fire is unsolved (multi-room L1@T3 wipes at t131).
5. **Boosted self-play reshuffles the meta**: default tactics ≈ best at T0, but lose to
   `focus_ball` (+2797) at T2 and to `ranged_duel_kite`/`anti_aoe_spread` (+2254/+2967) at T3 —
   the kernel-params tuning was done unboosted and does NOT generalize. A boosted re-tune pass
   (over `boosted_comp_basket`) is a queued follow-up, alongside ADR 0041 P4.

## Follow-up queue (ordered — mirrored as tracker §3 **Phase 4.5**, the durable copy)

1. **Tactical: cohesion under focused fire** — approach formation must keep healers adjacent to the
   focused member (or gate the advance on heal-delivery geometry, not just totals). Grade with the
   stronghold gauntlet (L1@T3 → Killed is the acceptance bar).
2. **Tactical: border crossing under fire** — the border gauntlet g1→g4 ladder is the instrument
   (bar: g1–g2 Killed, then g3+).
3. **Lone-survivor policy** — wipe-or-retreat, never eternal stalemate.
4. **Parity backlog** — H0 first (neutral-wall intents), then the threat/traversal unification,
   then the ranked rest (H5/H6/H8/H9/M4/M6 — see the parity report).
5. **Boosted kernel re-tune** over `boosted_comp_basket` (merges with ADR 0041 P4 sweep).
6. **Stronghold capability: L2+ defer even at T3** — one squad under the 3000-energy member clamp
   cannot out-sustain 2+ towers at any tier. Needs multi-squad assault doctrine or a deliberate
   siege member-clamp lift — a DESIGN fork (stop-and-ask), not a tuning knob.

## Log

- 2026-08-23 — corpus + boosted lane + boost-blind seam fix + parity audit landed (this batch).
  Gauntlet + dashboards produced the baseline tables above.
- 2026-08-24 — **Phase 4.5 item 1 (cohesion under fire): open-layout bar ACHIEVED** (decision
  `be725c9`). The corpus's trace probe root-caused a four-defect chain in the EV kernel: squad-total
  heal optimism in the risk term; the risk currency collapsing in structure sieges (g_us ≈ 0 with
  no killable creeps); no lockstep model (a member priced coverage against its healer's stale
  tile); and heal triage misdelivery (the field stamps the enemy's full output on everyone →
  full-HP healers self-pre-healed while the actually-focused member died in a part-loss spiral,
  −78→−318/tick, watched tick-by-tick). All four fixed per-tick-optimally. **L1-open@T3 → Killed,
  151 ticks, zero losses** (floor pin upgraded). New precise findings: (a) chokepoint TRICKLE-IN —
  the corridor stretches the wedge, members reach the wall piecemeal, tower eats the trickle
  (choke rung → Timeout with 5/8 surviving a failed wall camp); (b) rout direction is still the
  local anti-threat gradient — survivors corner themselves NW instead of exiting east (items 2/3);
  (c) a healers-first commit-sort drove designed#0 oscillation 3%→44% and was reverted — ordering
  interacts with contention tuning; the oscillation gate must ride every kernel-order change.
- 2026-08-23 (late) — MMO hot swap (wasm `039c587dc1c6`) **tail-verified live**: 0 panics/deser/
  INTEGRITY over the watch window; war threat pricing visibly flowing through `effective_output`
  (`heal=12` per unboosted heal part). Movement CPU in the known post-swap transient band (~85),
  expected to settle ~34. Tail tooling upgraded for this: `screeps-rest-api/examples/tail.rs` now
  takes `--server <name>` and resolves the token straight from `.screeps.yaml` into a
  `SecretString` (no shell-env export needed). Found work swept into tracker §3 Phase 4.5 +
  §6 (0010/0041/0025/0023 refreshed, §8 BoostQueue row closed).
- 2026-08-24 (later) — **Phase 4.5 item 2 (border crossing): ACHIEVED** — decomposed live from the
  operator's replay observation ("one creep enters and then everything outside the room stalls")
  via the SQ_DEBUG driver trace + probe. Seven-defect chain, all fixed: (1) the sim driver's
  travel arm trickled members across individually → bloc crossing gate (gather at the exit band,
  release when clustered); (2) in-room-only member views (parity H5 — the REC-053 comment claimed
  live parity; the audit refuted it) let the first entrant decide the squad's fate alone →
  full-roster views with execution still room-scoped; (3) `plan_squad_ev` derived its tile room
  from the CENTROID → a mid-crossing squad's goals were built in the staging room (V-1 aliasing) →
  explicit fight-room param; (4) the mover's anti-scatter anchor pinned fight-room members to the
  cross-room centroid → room-gated; (5) entrants camped the arrival tiles ("waiting for coverage")
  and jammed the doorway their healers needed → exit-edge tiles priced as transitional (real-engine
  fidelity: you cannot hold an exit tile); (6) `assess_engage` counted the target room's towers at
  the 150 damage floor against a staging-room centroid (phantom threat — towers cannot fire
  cross-room) → room-local; (7) Retreating latched across a full withdrawal → state decay, plus
  rout-to-rally (withdraw the way you came; the drain runner deliberately keeps local-kite retreat).
  Bonus: the full-roster view exposed a LATENT fixture bug (twin_room_siege's "target-room" core
  was physically in the staging room). **Gauntlet: every rung that fields now KILLS** — L1 open
  172 / choke 273 / choke-multi 590; border g1@T0 74, g1@T3 49, g2@T3 50 (bloc crossing, campers
  wiped, zero losses). Pinned: `stronghold_floor_t0_defers_t3_kills_every_l1_rung`. g3/g4 defer at
  sizing (capability item 8). Replay viewer (`write_stronghold_replays` → target/replays/stronghold/
  index.html) regenerated + delivered.
