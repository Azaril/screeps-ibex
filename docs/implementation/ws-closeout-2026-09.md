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
- [ ] **0028 bed 3** — `MultiSquadFormingScenario` under K4 claim pacing (both admission arms:
      `claims_allowed` reproduces the forming-cap=1 lockup, `claim_admission` shows the fix); bed 1
      re-run at N>1. Beds 5/6 stay out (WvC-2 ruling names 1+3).
- [ ] **0041 §7 P3 boosted lifecycle bed** (critic finding — required before ws-5 can delete).
- [ ] Gate battery (6 named gates) + workspace suite + determinism fence + `clippy-wasm`; commit.

**Phase C — the ONE Docker refresh (no world wipe, D8):**
- [ ] Fix the guide's stale pass line (cap = `max_concurrent_squads(owned)` + surge) BEFORE grading.
- [ ] Deploy the batched build to `private-server`; loud reset expected (WFV 27→29 world).
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
| (filled in Phase C) | | | | |
