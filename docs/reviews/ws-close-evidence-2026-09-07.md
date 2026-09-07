# WS-CLOSE evidence — the clean stopping point (2026-09-07)

> **What this is.** The durable record of the WS-CLOSE workstream (RULING-10, operator 2026-09-07) and
> the live-collapse fix that rode with it (RULING-11): the program, the eight decisions taken while
> building, the found-work register, and the Phase C evidence table — verbatim from the implementation
> doc that was deleted on close per [`../implementation/README.md`](../implementation/README.md) rule 3.
> Reviews are permanent; implementation docs are not. Design content lives in the ADRs' dated
> `Design deltas (2026-09-07 …)` sections; status lives in
> [`../execution/implementation-tracker.md`](../execution/implementation-tracker.md).
>
> **Commits:** `7dde003` (Phase A+B + the collapse diagnosis) → `3d0e998` (RULING-11 roots A+B + the
> Phase D write-backs into 20 ADRs) → `8c3cece` (Phase C: H5 byte-exact on the real server, F10, F12,
> lint sweep). **Deploys:** Docker private wasm `cbcd88671d5e` (= `3d0e998`, loud reset WFV 27→29 over
> the July world) and the MMO shardX hot swap wasm `53a3c66f9633` (= `3d0e998`, no reset). Sibling
> report: [`mmo-collapse-diagnosis-2026-09-07.md`](mmo-collapse-diagnosis-2026-09-07.md).

## 1. The program

**RULING-10 — tie off everything in flight before any new ADR build.** The operator's brief was a CLEAN
STOPPING POINT: every open workstream closed on evidence, every implementation doc deleted with its
durable facts written back to ADRs, the tracker's §5/§6 collapsed — and ADR 0048 left as a parked Draft
with no multi-squad work started. Four sub-rulings shaped the lanes: **(i)** the live-watch closure basis
is *provoke it on Docker* — refresh the private server to the batched build and run the offense-soak
recipe plus a seeded boost lab so the shipped-but-unexercised behaviors (ADR 0035 FU2 give-up clock,
S5-CAP surge, T-DEF-1 rampart anchoring, T-DEF-5 predictive safe mode, T-POS-5 exit discipline, the
`a2-i6-tight` profile, BoostQueue → labs → AwaitBoost) execute for real, and the five workstream docs close
on that evidence rather than on sim pins alone; **(ii)** parity M17/M18 = *unify* — WORK and CLAIM go into
the one shared `threat_value` so tower targeting and squad focus agree; **(iii)** segment capacity is
*watch-only* this pass; **(iv)** the harness lane is *in scope* — the H5 sim-vs-server parity oracle (ADR
0006), 0036 live raze, 0028 lifecycle closeout beds, 0004 governor calibration, 0023/0023a border
scenarios, M20–M23. The lanes landed in order: **Phase A** (M17/M18 unify; M20–M23 both sides; the
0023/0023a sim side + `MultiRoom` generator; the Docker-free half of the H5 oracle; 0004 scenario prep;
ten agents + a critic, every lane PASS-WITH-NOTES under adversarial verification) → **Phase B** (0028 bed
3 + bed 1 at N>1, the ADR 0041 §7 P3 boosted lifecycle bed, the engine seam-fidelity fix F1, the H5
must-fixes; the six named gates + workspace suite + determinism fence + `clippy-wasm`; committed
`7dde003`) → **Phase D** write-backs (every impl-doc fact into its owning ADR's dated delta section,
verified against code; `3d0e998`) → **Phase C** (ONE Docker refresh with no world wipe; the provocation
pass; H5 golden-vector capture; the 0004 calibration run; `8c3cece`) → this close-out (docs deleted,
tracker collapsed). Process rules held throughout: no AI commit attribution; MMO/Docker reads + deploys
batched for validation.

**RULING-11 — the live collapse rode the same batch.** The same morning the operator reported the MMO
empire starving (7 owned rooms, 46 creeps, storage energy 0 in every room, spawns idle, ramparts decayed,
720k oxygen hoarded). A six-lane, adversarially-verified diagnosis found two independent roots — **(A)**
the rover ops-pool *movement wedge* (each search RESERVES `rooms×2000` ops, ×2 when stuck, never refunded,
so a handful of stuck creeps drained the 20k pool every tick, every other search returned `PathNotFound`,
and a `PathNotFound` creep was posted as an unshoveable occupant — haulers froze ADJACENT to empty
extensions until TTL death) and **(B)** the *spawn-queue head-of-line deadlock* (`spawnsystem.rs` breaks
on the first unaffordable body while the hauler bid is 99,999 for ANY body and capacity-sized, so 550e
miners never spawn behind it and no starvation arm exists) — plus amplifiers C–F (bootstrap bodies,
defense fan-out with own-room give-up marking, 1e6 tower survival bids over never-refilled peace towers,
standing reserver requests for minerless outposts). The ruling: apply two live flips immediately
(`pathing.pathfinding_cpu_budget` 20→50 and `remote_mine.reserve=false`, tick 5486228; HOLD
`military.defense`), fix roots A+B NOW inside the WS-CLOSE batch — the collapsed July-build private world
became the natural before/after bed — and take amplifiers D/E/F as a SECOND batch after the stop. Result:
on the private world creeps 30→253 sustained over 4,300 ticks with the ops pool saturating only
transiently; on MMO after the hot swap `ops_used` 6,589 of 50,000, zero move failures, the W13N52 lane
301→4,079. The batch then surfaced **F12** (a second wedge arm on the normal-mode pathfinding headroom,
fixed in `8c3cece`) and **F11** (a tick killed mid-serialize resets the world — an ADR 0047 design item
for the second batch). Both flips are wiped by the next `reset.features` one-shot and must be re-applied.

## 2. Where the implementation docs went

| Deleted doc (2026-09-07) | Its durable facts now live in |
|---|---|
| `ws-2-combat-wave-b.md` | ADR 0008a "Safe mode as built" (D2/D3); ADR 0027 delta (D28 confirmed in code); ADR 0037 delta (T1/T2 retained by design); D9/D10 in rover + ADR 0008a; the [2026-07-09 review](combat-systems-review-2026-07-09.md) carries the closed Tier −1 list. |
| `ws-wvc1-military-completion.md` | ADR 0008a "T-HEAL-3a" + readiness tranche; ADR 0035 delta (FU2 = `RetreatClock`); ADR 0026 delta (L8); ADR 0034 delta (D6c renewable-rally bias); ADR 0028 delta (K3/K4 → `claim_pacing`, S5-CAP); ADR 0020 (S5-CAP). |
| `ws-wvc2-defensive-features.md` | ADR 0008a (T-POS-5, T-DEF-1, T-DEF-5 as built) + ADR 0019 delta; ADR 0037 delta (T3 emission closed by ruling); ADR 0028 (lane contention = bed 3). |
| `ws-4-r19-retune.md` | ADR 0026 delta + ADR 0026a adoption table (the `open_combat` profile history); ADR 0031 delta + 0031b §5 (w_energy=1.0 re-sweep); ADR 0019/0024 deltas (`s4_weights_retune`); ADR 0033 (the corpus-wide determinism fence). |
| `ws-5-boost-pipeline.md` | ADR 0041 "Design deltas (2026-09-07)" + "WS-CLOSE write-back — the layer as built"; ADR 0010 delta (L0 as shipped); ADR 0020 rows corrected. |
| `ws-val-combat-validation-corpus.md` | ADR 0023a "WS-VAL corpus write-back" + [`screeps-combat-eval/README.md`](../../screeps-combat-eval/README.md) "Stronghold / border / boosted lanes"; ADR 0025 deltas A–G (RULING-9, items 1/2/5/8a, D1, open items); ADR 0041 (the boost-blind seam); ADR 0034/0027/0035/0031/0024 deltas; the [parity audit](live-sim-parity-audit-2026-08-23.md). Tail/console tooling (`tail.rs --server`, `console.rs`) is documented in the crates. |
| `ws-closeout-2026-09.md` | This document (§3–§5) + ADR deltas 0006 / 0008a / 0023 / 0028 / 0034 / 0039 / 0004 / 0033 / 0040 / 0043 / 0047. |

## 3. Decisions D1–D8 (taken 2026-09-07; each written back to its ADR)

- **D1 · `threat_value` currency (ADR 0025 delta F / 0008a T-DEF-4).** WORK term = `effective_output(Work,
  DISMANTLE_POWER)` (50/part, boost-aware, UNCONDITIONAL — ranking may use the danger proxy; only SIZING
  must not, per the dismantle ruling). CLAIM term = `effective_output(Claim, CONTROLLER_ATTACK_PER_PART)`
  (300/part, the engine's own controller-damage unit): one CLAIM part = ten ATTACK parts, so a declaimer
  outranks any realistically-sized breacher — T-DEF-4's ordering falls out of the one additive currency
  instead of a lexicographic tower-only rule.
- **D2 · Legacy tower heuristics deleted (ADR 0008a §C).** `is_likely_tower_drain` is subsumed by the
  kernel's `full_tower_damage <= heal` hold-fire; `total_tower_damage` was its duplicate. The near-edge
  nuance is not kept.
- **D3 · M20 = measure live, Option A (ADR 0028).** `queue_slot_spawn` returns whether it queued; an
  EPHEMERAL per-objective `any_queued_last_tick` (never serialized — no WFV) feeds `forming_in_flight =
  forming && (queued || members in flight)`, so a roster that can never be built stops holding a claim
  slot. Harness fixture: all slots unbuildable at the energy cap → lease lapses at +400.
- **D4 · 0023 "cross-room Flee" closes on the single-creep bed + the squad rout bed (ADR 0023/0024).** The
  seam-stitched threat/approach field (ADR 0024 §follow-up, ADR 0025 §11 #10, ≤0.97 oscillation pin)
  remains a kernel design item — not harness closeout.
- **D5 · H5 crate split (ADR 0006 §B.1 delta).** Pure schema/replay/diff live in `screeps-combat-engine`
  (so `tests/conformance.rs` has no dependency cycle); the Docker-facing seeder/capture/report lives in
  `screeps-ibex-eval` over `screeps-server-kit` command builders.
- **D6 · "Nightly gate" = a one-command Rust runner + an `#[ignore]` lane (ADR 0006).** There is no CI;
  layer 2 starts REPORT-ONLY with `parity-budget.json` seeded from the first reports (ADR 0015's
  report-only → gating rule; the promotion count is an operator number, not invented here).
- **D7 · 0039 P2–P4 (ADR 0039 "Disposition").** Re-parked as design items, not closed by H5: the oracle
  supplies the fidelity bound §0 assumed; P2 is a decision-crate extraction, P3/P4 sim-driver work; no
  harness dependency remains.
- **D8 · No world wipes.** The warm private world (two RCL-8 rooms, labs, strongholds) is the seeded world
  for every Docker lane; `bootstrap --reset` consumers get a `--keep-world` path. Every `strongholds.spawn`
  is batched before the single neutral-room `active:true` restart.

## 4. Found-work register F1–F12

| # | What | Where recorded / fixed | Owner · state |
|---|---|---|---|
| F1 | The sim engine let towers/creeps target ACROSS room seams (the real engine cannot) — the multi-room beds' early wipes were a fidelity leak, not a tactic | FIXED at source (Phase B lane (ii): `resolve.rs` `in_range` same-room gate on every creep and tower action; 7 RED-verified `*_seam` engine tests; `same_room_defense` shim deleted) — ADR 0023 "engine seam fidelity" delta | 0023 · closed |
| F2 | Kernel duplicate-goal park — two members assigned the SAME goal tile → dance damper converts both to Immovable holds → the squad parks Engaged forever without acting (GROUP-UP bed geometry, members at (46–48,21–23) vs a tower at (46,23)) | ADR 0025 delta G (bar: goal assignment excludes tiles already claimed this tick, or the damper never freezes two members on one tile; repro = the 0023 S5 GROUP-UP bed with the staging moved onto the tower's flank); ADR 0023 S5 notes; tracker §6 0025 | 0025 · OPEN |
| F3 | The bloc gate's ASSEMBLED test (every traveller within 4 of the centroid) releases while a straggler is ~6 tiles back, so one member crosses ~11 ticks late (GROUP-UP bed, measured) | ADR 0034 delta (F3: grade on a live crossing before touching `GATHER_RADIUS`/`GATHER_EDGE_SLACK` — a tighter radius re-opens the measured permanent-gather freeze) | 0034 · WATCH |
| F4 | The tower bounded-probe state machine never resolves when a confirmed drainer's visible heal out-heals all towers (`tracker.engaging` stays true, no strike accrued; energy behavior correct) | ADR 0008a "T-DEF-3 note … F4"; tracker §6 0008a | 0008a · OPEN (small state-machine fix, on evidence) |
| F5 | M20 stale-bit edge at the two Phase-B early `continue`s | FIXED same day (Phase B); ADR 0028 delta D3 | 0028 · closed |
| F6 | Three H5 vectors were sim-generated placeholders, and the frame schema had no structure/tower rows | FIXED (Phase B lane (iii): structure/tower frames captured; Phase C: 5 of 5 vectors SERVER-CAPTURED, 0 placeholders) — ADR 0006 deltas | 0006 · closed |
| F7 | The rout-to-rally leg ENDS AT THE SEAM — withdrawn crossers exit alive, `Retreating` decays to `Forming`, the bloc gate re-releases nobody, survivors idle un-rallied to the timeout | Honest-baseline pin `border_rout_withdraws_across_the_seam_but_never_reaches_the_rally_yet` (fails loudly when the rally leg lands); ADR 0034 delta F7 + ADR 0027 Retreating-decay delta + ADR 0023 D4 — the M23 economic give-up is the designed re-entry/give-up terminal | 0034 / 0027 · OPEN |
| F8 | A claimed roster that never STARTS a member lapses its +400 commitment lease; offense is then backed off (`mark_unwinnable`) while a Defend objective re-claims every 400t FOREVER (6 generations on a lane-starved defender board) | ADR 0028 delta "What bed 3 measured (ii)"; tracker §6 0027 — decide whether a merely lane-starved offense roster should be marked unwinnable at all, and whether Defend re-claim needs a bound | 0027 · OPEN (second batch, amplifier D bounds) |
| F9 | Rout-to-rally is a SIM-DRIVER discipline only — the live `SquadManager` Retreating arm consumes the kernel's kite/withdraw goal and never steers toward the rally; the Phase C "routs back to rally" provocation therefore grades the withdraw leg only | ADR 0034 delta ("a SIM-DRIVER discipline today, not a live one"); tracker §6 0034 | 0034 · OPEN (live half) |
| F10 | The travelling/engaged squad's RALLY ROOM flipped tick to tick — `uncontested` was a per-tick VISION read one layer down (the room's creep/structure caches refill only from live vision, so no eye ⇒ empty DTOs ⇒ "contested"); `gathered` never held, squads acquired focus and left — the "objective creeps idling" class (W5N7 L0 core untouched through three generations; W7N7 raze stalled at 4,000) | FIXED `8c3cece`: `rally::TargetIntel` + `target_is_uncontested_by_evidence` (clear observation ≤250 ticks old ⇒ uncontested; any hostile observation ⇒ contested; unknown ⇒ contested) fed from the persisted `RoomDynamicVisibilityData`; 10 RED-verified pins across rally.rs / squad_manager.rs / the lifecycle harness (`run_rally_flap_flow`); ADR 0034 "F10 rally flap" delta | 0034 · FIXED — re-observe the W5N7/W7N7 razes (0036) |
| F11 | A tick KILLED mid-serialize (bucket≈0) leaves a truncated world chunk; the next boot fails `corrupt gzip stream does not have a matching checksum` and the bot RESETS THE WORLD (missions/plans lost) — seen once in the 0004 calibration run alongside three VM kills at bucket≈0 | ADR 0047 delta (design item: write-then-swap — write the new chunk set to alternate slots / under a version tag, checksum, only then flip the active pointer; a failed checksum falls back to the last good set); tracker §6 0047, with the segment-capacity watch | 0047 · OPEN (second batch) |
| F12 | Normal-mode pathfinding headroom = movement cap ("never start a search") — a SECOND wedge arm that fires whenever the bucket sits below `bucket_burst_threshold` (MMO ~1.5 h post-swap: bucket 9,487 → `move_failed_budget` 29 with the ops pool untouched, W13N52 lane 4,427→1,039, storage 1,275→0, creeps 117→98) | Mitigation flip `bucket_burst_threshold` 9500→2000 at tick 5492700, later REVERTED to 9500 after it drained the bucket; code FIXED `8c3cece` (`pathfinding_headroom_for`: a finite 20-CPU normal-mode headroom strictly below the cap, pinned) — ADR 0004 delta | 0004 · FIXED |
| — | Garrison stand-down: an own-room Secure objective outlives its threat — the W9N8 defense squad stayed in-room 13+ min (~8,000 ticks) after the hostiles died (`engaged_once=false`, `lease_left=400` renewing) | Evidence row below; tracker §1 (a) / §6 0027 | 0027 · OPEN (second batch, amplifier D) |
| — | H5 tower-bed side effect: the ~60-tick controller claim of W9N7 made the acting bot colonize it (planning ×24, 10 sites, an extension completed, six builders) and the bed's cleanup left them | FIXED (must-fix lane): snapshot-and-remove every object not present before the bed, `parityClaim`-marker-gated stale-claim release, exact controller restore incl. safe-mode fields — ADR 0006 "H5 must-fixes" delta; re-run PASS | 0006 · closed |
| — | Amplifier E reproduces on the private world: peace towers hold 0–9 energy in every storage room | Evidence row below; the [collapse diagnosis](mmo-collapse-diagnosis-2026-09-07.md) §3b #5 (0044 sink pricing) | 0044 · OPEN (second batch) |

## 5. Evidence table (Phase C, verbatim from the implementation doc)

Legend — **Behavior**: the shipped-but-unexercised behavior RULING-10 (i) asked to provoke (or the fix under
validation); **Provocation**: what was done on the private server, or observed organically on MMO;
**Proof**: the quoted measurement; **Verdict**: PASS / PARTIAL / FAIL / NOT PROVOKED / FINDING / FOUND;
**Run**: the run directory (`runs/…`, kept by the eval harness) or the session-local capture file
(`scratchpad/…`, `*.txt`, tail ids — transient, not in the repo). Private-world ticks are ~19.1M; MMO
ticks ~5.49M.

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

**Post-table notes (2026-09-07, later the same day).** The F12 mitigation flip (`bucket_burst_threshold`
2000) was reverted to 9500 after it drained the bucket; the code fix in `8c3cece` is the answer. The
`pathfinding_cpu_budget` 50 and `remote_mine.reserve=false` flips stand and are wiped by any
`reset.features` one-shot. The 0036 "reaches 0" re-observation after F10 is the one verdict still owed
(tracker §6 0036).
