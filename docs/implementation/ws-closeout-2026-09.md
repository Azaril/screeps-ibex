# WS-CLOSE — the in-flight tie-off (RULING-10) — implementation

**Workstream:** WS-CLOSE · **Advances:** ADR 0006 (H5 oracle), 0004 (governor calibration),
0023/0023a (border scenarios, MultiRoom), 0028 (lifecycle closeout beds), 0036 (live raze),
0041 (§7 P3 boosted lifecycle bed), 0008a/0037 (parity M17/M18), 0025/0027/0034/0035 (write-backs)
· **Status:** active (opened 2026-09-07)

## Resume point

Scoping done (10-agent fan-out + critic, 2026-09-07). Private stack is UP (warm world: ibex owns
W9N8 + W9N6 at RCL 8 with 10 labs each, storages hold base minerals but NO T3 compounds (seed
them for the boost lane), W9N8 `safeModeAvailable: 7`, 9 ramparts ≥10k, cores in W6N6 (L4) /
W6N7 / W7N5 (L0) and all target rooms `active: true`, tick 100 ms, bot code = July `ab692bd`,
live tree: offense/attack_invaders/debug_log ON, boost_military OFF, auth mod 2.8.3 + signin
route present). **H5 Q1 answered empirically 2026-09-07: a creep inserted via
`storage.db['rooms.objects'].insert` (type/user/room/x/y/name/body/hits/hitsMax/fatigue/
spawning/ageTime/actionLog) persists, appears in the bot's `Game.creeps`, and the engine ticks
it — two `move(TOP)` from the runtime moved it (25,25)→(25,23).** **Phase A+B COMMITTED
`7dde003` (six submodules pushed) on a fully green battery** (eval 154/154, six gates, ladder
unchanged L1 T3 open 231 / choke 355, fence spread 0, workspace 1621/1621, clippy-wasm + host
clippy clean). RUNNING NOW: the RULING-11 roots A+B fix lane (`wf_0b4c6f70-f06`: rover ops refund
+ shoveable-on-budget-miss + rotation; miners CRITICAL + need-scaled hauler bid + starvation
sizing) and the Phase D ADR write-back lane (`wf_dfd300a9-8b2`, doc-only). Six T3 compounds
(3000 each) seeded into W9N8's private storage; `ibex-2` exists for the H5 two-owner beds.
**The private world is the roots-fix BEFORE/AFTER bed: on the July build it sits in the SAME
collapse** (tick 19054699: 30 creeps over 9 rooms, GCL 13, every storage at 0 energy, 417 of 458
extensions empty, spawns idle at 30–300e, cpu 11/100) — deploy the batch there first and grade
recovery (ops_used ≪ pool, lanes leave 300, creeps climb) before the MMO hot swap. MMO after the
flips: creeps 46→78 in ~500 ticks, but the 50k pool RE-SATURATES at 78 creeps (48k/50k used,
repaths 8, move_failures 36) — the reservation-not-refund root is structural; the flip only bought
headroom. **Roots A+B LANDED + VERIFIED (PASS-WITH-NOTES both; must-fixes applied: controller-side
links excluded from lane-reachable energy, test-comment arithmetic)** — root A: rover pool
reserve-then-REFUND (`PathfindingResult.ops`), `PathBudgetExhausted` vs `PathNotFound` split with
a displaceable `budget_missed_occupant`, per-layer round-robin first-path cursor, seg-57
`move_failed_budget`/`move_failed_nopath`; rover-eval H 0.9625 unchanged, six gates + fence green.
Root B: `SPAWN_BID_MINER = CRITICAL`, bootstrap floors CRITICAL+1000/+2000 (available-sized, never
bank), need-scaled `hauler_bid` (w = min(body throughput, unmet − roster throughput)), per-tick
`replacement_body_energy` starvation sizing over `spawn_lane_reachable_energy`. **Phase D
write-backs LANDED + VERIFIED** (0025 A–G, 0008a/0035/0026/0026a/0034/0027/0031/0031b/0024/0019/
0032/0037, 0041/0010/0020/0023a, eval README, parity audit; must-fixes applied). Battery #2 running
(`battery2.txt`). Next: commit (rover submodule + super) → Phase C Docker refresh (private world =
the before/after bed) → provocation pass → deletions + tracker collapse → MMO hot swap.

## Target

A clean stopping point: every open workstream closed on evidence, all six impl docs deleted with
their durable facts written back to ADRs (README rule 5), §5/§6 collapsed, the batch live on MMO.
0048 stays a parked Draft; multi-squad work is NOT started.

## Plan

**Phase A — LANDED 2026-09-07 (workflow `wf_86ab5f71-3f7`, five lanes, every lane
PASS-WITH-NOTES under adversarial verification; uncommitted until Phase B's integrated battery).**
Found work from Phase A (recorded, not all fixed here): (F1) **engine seam-fidelity gap** — the sim
engine let towers/creeps target ACROSS room seams (real engine cannot) → Phase B lane (ii) fixes at
source; (F2) **kernel duplicate-goal park** — two members assigned the same goal tile → dance
damper Immovable holds → squad parks Engaged forever without acting (GROUP-UP bed geometry,
(46-48,21-23) vs a tower at (46,23)) → queued under ADR 0025 in tracker §6; (F3) **bloc-gate
straggler release** — ASSEMBLED (every traveller within 4 of centroid) releases while a straggler
is ~6 tiles back, so one member crosses ~11 ticks late → watch in the Phase C crossing grade
(0034/agent gather constants); (F4) tower bounded-probe state machine never resolves when a
confirmed drainer's visible heal out-heals all towers (`tracker.engaging` stays true; energy
behavior correct) → 0008a note; (F5) M20 stale-bit edge at the two Phase-B early `continue`s →
FIXED by the parent same day; (F6) three sim-placeholder-only H5 vectors + the structure/tower
frame gap → Phase B lane (iii).
- [x] **(a) M17/M18 unify** — `threat_value` += WORK + CLAIM (D1); live no-squad tower path routed
      through `decide_towers` (M17); `total_tower_damage`/`is_likely_tower_drain` deleted (D2);
      RED-verified pins (declaimer/dismantler outrank a grunt; no-squad kernel fallback).
- [x] **(b1) M20–M23 both sides** — M23 K-latch → shared `EconomicGiveUp` (econ-decision) + harness
      economic term; M21 `live_visible_clear` churn knob + vacuous Resolve pins; M22
      `RetreatClock`/`EnemyStallTracker` → decision `lifecycle.rs` + in-room stall flow pins;
      M20 = Option A (D3).
- [x] **(b2) 0023/0023a sim side** — GROUP-UP-THEN-ENGAGE-ACROSS-BORDER + STUCK-MEMBER-TIMEOUT beds,
      `integration_gate_marker` completed, stale headers fixed; squad cross-border rout bed (B1);
      `MultiRoom` Generator (C1/C2) enumerating twin_room_siege / BorderGauntlet / multi-room
      strongholds. B2 (seam-stitched field) stays OUT — a 0024/0025 design item (D4).
- [x] **(b3) H5 oracle, Docker-free half** — golden-vector schema + replay + diff (engine crate,
      `tests/conformance/` + `tests/conformance.rs`, ZERO tolerance); bot-side scripted-intent
      driver (`EvalFeatures.parity_script`, `PV1` per-tick emitter); server-kit
      `cmd_insert_creeps`/`cmd_room_creeps`; ibex-eval `Fault::ParityScript` + `parity capture` /
      `parity report` + `--keep-world` (D5, D6). 0039 P2–P4 disposed (D7).
- [x] **(b4) 0004 prep** — harder-burn scenario JSONs (critical hover / release / reset-under-
      critical) + seg-57 series extractor.

**Phase B — after A lands:**
- [x] **0028 bed 3** — `MultiSquadFormingScenario` under K4 claim pacing (both admission arms:
      `claims_allowed` reproduces the forming-cap=1 lockup, `claim_admission` shows the fix); bed 1
      re-run at N>1. Beds 5/6 stay out (WvC-2 ruling names 1+3).
- [x] **0041 §7 P3 boosted lifecycle bed** (critic finding — required before ws-5 can delete).
- [x] Gate battery (6 named gates) + workspace suite + determinism fence + `clippy-wasm`; commit (`7dde003`, then `3d0e998` with roots A+B + Phase D).

**Phase C — the ONE Docker refresh (no world wipe, D8):**
- [x] Fix the guide's stale pass line (cap = `max_concurrent_squads(owned)` + surge) BEFORE grading.
- [x] Deploy the batched build to `private-server` (wasm `cbcd88671d5e`, clean loud reset) — and the MMO hot swap (`53a3c66f9633`) per RULING-11.
- [ ] ONE seeded offense-soak graded for four lanes: 0036 bare-core raze (hits strictly ↓ → 0),
      0028 pacing canary, 0023 bloc crossing, Wave-B churn signatures. Then owned-room defense
      provocation (T-DEF-1/T-DEF-5/S5-CAP), attack-flag stall (FU2 clock, T-POS-5), seeded T3 lab
      (BoostQueue→labs→AwaitBoost), H5 vector capture, 0004 ladder (`--keep-world`).
- [ ] Evidence table below filled (behavior · provocation · proof · verdict · run dir).

**Phase D — close-out:**
- [ ] Design-delta write-backs: 0025 (RULING-9 + items 1/2/5/8a), 0041 (own-side boost pricing,
      P4 result), 0031 (capped optimizer family), 0035 (FU2 in-contact gate), 0034 (rout-to-rally),
      0027 (Retreating decay), 0008a (T-DEF-1/5, T-POS-5 as built; T-DEF-4 → `threat_value`),
      0023a (corpus lanes landed), 0006 (crate split), 0004 (constants).
- [ ] Delete ws-2 / wvc-1 / wvc-2 / ws-4 / ws-5 / ws-val + this doc; tracker §5/§6 collapse (LAST,
      after every §6 HARNESS line is struck); parity report M17/M18/M20–M23 → FIXED; §10 lines.
- [ ] MMO hot swap of the batch (RULING-8 + operator 2026-09-07 authorization), tail-verified;
      memory updated.

## Design deltas (decisions taken 2026-09-07 — each is written back to its ADR in Phase D)

- **D1 · `threat_value` currency (ADR 0025/0008a T-DEF-4).** WORK term = `effective_output(Work,
  DISMANTLE_POWER)` (50/part, boost-aware, UNCONDITIONAL — ranking may use the danger proxy; only
  SIZING must not, per the dismantle ruling). CLAIM term = `effective_output(Claim,
  CONTROLLER_ATTACK_PER_PART)` (300/part, the engine's own controller-damage unit): one CLAIM part
  = ten ATTACK parts, so a declaimer outranks any realistically-sized breacher — T-DEF-4's
  ordering falls out of the one additive currency instead of a lexicographic tower-only rule.
- **D2 · Legacy tower heuristics deleted.** `is_likely_tower_drain` is subsumed by the kernel's
  `full_tower_damage <= heal` hold-fire; `total_tower_damage` was its duplicate. The near-edge
  nuance is not kept.
- **D3 · M20 = measure live (Option A).** `queue_slot_spawn` returns whether it queued; an
  EPHEMERAL per-objective `any_queued_last_tick` (never serialized — no WFV) feeds
  `forming_in_flight = forming && (queued || members in flight)`, so a roster that can never be
  built stops holding a claim slot. Harness fixture: all slots unbuildable at the energy cap →
  lease lapses at +400.
- **D4 · 0023 "cross-room Flee" closes on the single-creep bed + the squad rout bed.** The
  seam-stitched threat/approach field (0024 §follow-up, 0025 open item, ≤0.97 oscillation pin)
  remains a kernel design item — not harness closeout.
- **D5 · H5 crate split (ADR 0006 §B.1 delta).** Pure schema/replay/diff live in
  `screeps-combat-engine` (so `tests/conformance.rs` has no dependency cycle); the Docker-facing
  seeder/capture/report lives in `screeps-ibex-eval` over `screeps-server-kit` command builders.
- **D6 · "Nightly gate" = a one-command Rust runner + an `#[ignore]` lane.** There is no CI;
  layer 2 starts REPORT-ONLY with `parity-budget.json` seeded from the first reports (ADR 0015's
  report-only → gating rule; the promotion count is an operator number, not invented here).
- **D7 · 0039 P2–P4** — disposed with H5 (re-parked or closed by the H5 lane; recorded in §6).
- **D8 · No world wipes.** The warm private world (two RCL-8 rooms, labs, strongholds) is the
  seeded world for every Docker lane; `bootstrap --reset` consumers get a `--keep-world` path.
  Every `strongholds.spawn` is batched before the single neutral-room `active:true` restart.

## Verification

Phase A/B: the six gates (`stronghold_floor_t0_defers_t3_kills_every_l1_rung`,
`multi_member_drain_soak_kills_with_tank_forward_coordination`,
`assembler_kills_across_defended_regimes`, `positioning_oscillation_stays_low_across_designed`,
`exp_register_passes_all_gates`, `t3_twin_decisively_beats_unboosted_twin`), every new pin
RED-verified, workspace suite, `sim_is_deterministic_over_rounds`, `cargo clippy-wasm`
warning-free. Phase C: the evidence table, one row per behavior, verdict from a quoted proof.

## Evidence table (Phase C)

| Behavior | Provocation | Proof | Verdict | Run |
|---|---|---|---|---|
| Root A rover wedge (RULING-11) | Private world = the collapsed July build (30 creeps / 9 rooms, every storage 0, 417/458 extensions empty, spawns idle, `ops_used 20000 == ops_pool` pinned); deploy `cbcd88671d5e` | seg-57 series ticks 19114351→19118682 (10 samples): `ops_used` 8 / 535 / 19244 / 20000 ×4 / 3087 / 11931 / 2861 of 20000 with `move_failed_budget` 0/0/3/64/89/10/7/0/0/0 — saturation is now TRANSIENT and self-relieving (round-robin + refund), never a wedge; `move_failed_nopath` ≤2; creeps 113→281; lanes W9N8 299→2619, W6N1 145→1356, W9N6 125→1316; 3 new rooms claimed (W3N8, W2N8, W8N9) | PASS | `scratchpad/pmon.txt` |
| Root B spawn deadlock (RULING-11) | same world (spawns idle at 30–300e behind 1800e haulers) | every home spawning within the first sample (`*` = busy), creeps 30→253 sustained over 4,300 ticks at `pathfinding_cpu_budget` 20 (the default — no flip needed on the fixed build) | PASS (queue-head proof by outcome; `[SpawnQueue]` has no log line) | `pmon.txt` |
| Loud reset WFV 27→29 | deploy over the July world | 120 s tail: zero panic / deser / INTEGRITY lines; cpu 44–76/100, bucket 10000 | PASS | tail `b1p312cdo` |
| 0035 FU2 give-up clock | organic: W7N5 L0 core objective | `[War] Skip W7N5 -- in give-up backoff (ADR 0035 D5); requested re-scout` every scan (the clock FIRED and the objective is backed off) | PASS (backoff state observed; the GIVEUP line predates the tail) | tail `bdq4plmkp` |
| 0036 live raze (Dismantle end-to-end, D3/D4) | organic: `[War] Offense objective Dismantle W5N7` and later `Dismantle W7N7` (npc-core sized to 11 ranged, P(hold)~100%), three squad generations acquire `FOCUS … structs=2 via=mapping` in W5N7 | 14 DB samples over 19 min: W7N5 core 100000→64140→GONE (razed between samples 2 and 3); **W7N7 core 29600→28240→13800→4000 under our Dismantle objective — hits strictly DECREASE (D3/D4 live-confirmed)** but stalled at 4000; W5N7 core stayed 100000 across all three generations | PARTIAL — the mechanics fire (hits fall, one core razed), the "reaches 0" leg is blocked by **F10** (below) | `scratchpad/raze.txt` |
| **F10 · rally-room flap (NEW live defect, Phase C)** | same objectives | `[Lifecycle] TRAVEL` for one squad within 45 s alternates `rally=(W5N7)` `uncontested=true` ↔ `rally=(W4N7)` `uncontested=false` (and W7N5↔W7N6 for another): the `uncontested` bit is an instantaneous visibility read, so the rally room flips as members enter/leave, `gathered` never holds, the squad leaves after acquiring focus (`STATE … phase=engaged in_room=false dist=1 focus=false lease_left=8`) — the "objective creeps idling" class | FIXED same day (rally `TargetIntel` evidence classifier, 250-tick freshness, fed from the persisted visibility record; 10 RED pins incl. the harness `run_rally_flap_flow`) — re-observe the raze after the next private deploy | tail 45 s |
| H5 parity oracle layer 1 (ADR 0006) — melee-1v1 | `parity capture --scenario melee-1v1` on the warm world: 2 creeps DB-inserted into neutral W9N7 (owners `private-server` + `ibex-2`), both owners armed with the chunked script (the console's ~1000-char limit — the arming is streamed through `Memory._pv`), PV1 frames captured | **ZERO divergence over 7 frames** replaying the server-captured vector through the sim engine; golden vector written to `screeps-combat-engine/tests/conformance/melee-1v1.json` (provenance: `3d0e998`, tick 19124089) | PASS | `runs/parity-capture-melee-1v1-3d0e998-20260907-183954` |
| H5 layer 1 — the other four beds | same capture path, one bed at a time in W9N7 | **tough-ladder ZERO divergence over 9 frames** (T1–T3 boosted TOUGH reduction matches the server); kite-r3 23 deltas / heal-race 13 / tower-rampart 29, all first-divergent at tick 1 → diagnose-fix-verify workflow `wf_e50bf8d4-55a`: NOT engine gaps — the second owner (`ibex-2`) had no bot code and the server had switched its runtime off (`users.active = 0` for a user with no objects), so every owner-1 actor stood still; the seeded tower answered `ERR_RCL_NOT_ENOUGH` (neutral room controller); then two more bed defects surfaced by the new gates: the owning bot's `TowerMission` replaced scripted shots with remote-road repairs (engine keeps one tower intent: heal > repair > attack) and a leftover tower-rampart tower at (20,25) blocked the kiter. Fixed in the harness/driver (owner code + runtime gate, per-owner console merge, scripted-execution gate with `!<ErrorCode>` markers, controller claim/restore for tower beds, `ParityReserved` towers, end-of-bed cleanup) — **all three re-captured ZERO divergence (kite-r3 11 frames, heal-race 13, tower-rampart 13)**; conformance lane: 5 vectors, 5 server-captured, 0 placeholders; sim engine untouched | 5 PASS | `runs/parity-capture-{kite-r3-3d0e998-20260907-194048,heal-race-3d0e998-20260907-194057,tower-rampart-3d0e998-20260907-194404}` |
| H5 tower-bed side effect (verifier finding) | the tower bed's ~60-tick controller claim of W9N7 | the acting bot treated W9N7 as a new colony (room planning ×24, `Construction W9N7 (RCL 3): 10 sites created`, a spawn site + 6 extensions + 2 containers, one extension completed, six builders sinking energy) and the bed's cleanup did not remove them | FOUND + CLEANED by hand (all ibex objects in W9N7 removed; controller neutral, reservation intact) → harness must-fix lane: snapshot-and-remove every object not present before the bed, harness-marker-gated stale-claim release, exact controller restore incl. safeMode fields | `defense3.txt` / verifier verdict |
| Defense provocation, attempt 1 | `system.runCronjob('genInvaders')` | returned OK but NO invader creep appeared in any room over 6 samples / 6 min (the cron gates on accumulated harvest stats); observed instead: W9N8/W9N6 towers hold 0–9 energy — the amplifier-E "never-refilled peace towers" defect reproduces on the private world too | NOT PROVOKED (method) — attempt 2 = DB-inserted raiders | `defense.txt` |
| Defense provocation, attempt 2 (S5-CAP / defense claim / T-DEF-1 / T-DEF-5) | 3 DB-inserted `ibex-2` raiders (5 ATTACK + 5 WORK + 5 MOVE, 1500 hp each) at (30–32,28) inside W9N8 + a 5,000-hit rampart inserted on spawn (31,32) | Within the first 30 s sample a **Secure objective for W9N8 had fielded a 6-member defense squad (Entity 853, `present=6/6 in_room=true`)** — the defense claim + surge + spawn path fires on the fixed build; the raiders were dead before the first DB sample (no tombstones left, towers at 0–7 energy so the kill was creep-side); the inserted rampart was also gone (engine rejected or the raiders' WORK razed it — undetermined); safe mode never armed (`safeModeAvailable 7, safeMode null`) | S5-CAP/defense claim PASS; **T-DEF-1 anchoring and T-DEF-5 predictive arm NOT PROVOKED** (fight too short; needs a sustained attacker — the second-bot AttackFlag route) | `defense3.txt` |
| Garrison stand-down (amplifier D / F8 evidence) | same | the W9N8 defense squad (obj 56) stayed in-room 13+ min (~8,000 ticks) after the hostiles died: `phase=in_room state=Moving present=5–6/6 engaged_once=false focus=false lease_left=400 forming_budget_left→0` — the Secure objective outlives its threat; own-room defense squads do not stand down | FINDING (second batch, amplifier D bounds) | `defense3.txt` |
| 0004 governor calibration (rung 1, warm world) | `scenario --file pressure-critical-hover.json --keep-world` (220/96 ms burn on a ~75-cpu warm world → cpu 250 → bucket 9500→10 in ~55 ticks) | tiers fired as designed (normal→conserve 6945/−10 → critical 3128/−54; pools 20000→10000→5000, movement /4→2000→0; recovery critical→conserve 1764→normal 4195); creeps 372→412 (progress continued); 3 VM kills at bucket≈0 + ONE corrupt-gzip world reset (**F11**, 0047); shipped constants CONFIRMED | PASS on evidence (rungs 2/3 covered by this run's recovery + organic resets); F11 recorded | `runs/pressure-critical-hover-3d0e998-20260907-200026` (+ `report --run`) |
| Boost chain (BoostQueue→labs→AwaitBoost) + attack-flag stall (FU2 on a committed fight, T-POS-5) | boost_military ON + six T3 compounds seeded in W9N8; attempted an `attack_w6n6` flag on the L5 stronghold W6N6 | every organic fight this world offers is won at T0 (P(hold)~100% → the sizer correctly picks T0, no BoostQueue request) or is an L5 stronghold the bot correctly declines (`[War] Evaluating W6N6 … dps=32880, towers=6, core_lvl=5`); `RoomPosition.createFlag` without vision does not create the flag on this server (`Game.flags` stayed empty), so the AttackFlag route could not be armed remotely | NOT PROVOKED live; the apply loop is validated where ADR 0041 §7 says it must be (the offline 0028 lifecycle bed: `boosted_roster_routes_to_the_labs_and_departs_boosted`, `stock_loss_mid_await_boost_falls_through_at_the_deadline`, RED-verified) — first boosted live engagement stays a WATCH | `attackflag.txt` |
| H5 tower bed after the must-fixes | `parity capture --scenario tower-rampart` re-run | `bed cleanup: controller restored` → `seeded objects removed: 1` → WARN `4 object(s) the bed did not seed appeared (creep×2, ruin×1, tombstone×1) — removing them` → `room put back to the pre-bed snapshot: removed 4`; **ZERO divergence over 13 frames**; ibex non-creep objects left in W9N7: 0 | PASS | `runs/parity-capture-tower-rampart-3d0e998-20260907-202214` |
| **F12 · normal-mode pathing headroom = cap (NEW live finding, MMO)** | organic: ~1.5 h after the hot swap the MMO bucket dipped to 9487 (<`bucket_burst_threshold` 9500) | seg-57: `ops_used 0 / 50000`, `move_failures 29`, **`move_failed_budget 29`**, `move_failed_nopath 0` — every search refused on the headroom arm (`pathing/movementsystem.rs` normal mode sets `pathfinding_headroom = movement_cap`, i.e. "never start pathfinding"), W13N52 lane 4427→1039 and storage 1275→0, creeps 117→98: a SECOND wedge that fires whenever the bucket is below the burst threshold (the roots-A verifier had flagged it latent) | FAIL → mitigation flip applied tick 5492700 (`bucket_burst_threshold` 9500→2000); code fix = finite normal-mode headroom, in the batch | `mmo_post3.txt` |
| MMO hot swap of the batch (RULING-8/11) | wasm `53a3c66f9633` at tick ~5490100, no reset | 150 s tail: zero panic / deser / INTEGRITY; seg-57 `aborted_ticks` flat at 2687; `ops_used` 6589 of 50000, `move_failures` 0, `move_failed_budget` 0, `move_failed_nopath` 0 (pre-fix: 20000/20000 and 23 failures); W13N52 lane 301→4079, storage 0→685; segment stream 163 KB (watch) | PASS | `mmo_postswap.txt` |
