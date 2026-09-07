# Implementation Tracker — the living master status doc

> **★ This is the single forward-looking source of truth.** If you are resuming cold, read §1–§4
> and stop; the rest is reference. Supersedes the "single source of truth" claim in
> [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) and the forward-looking half
> of [`phase-2.md`](phase-2.md) (a historical log, frozen 2026-06-23).
>
> **Last reconciled against code:** 2026-08-22 (56 ADRs verified by an 11-agent code-grounded pass;
> 29 carried stale headers). History and method: [`project-reconciliation-2026-08-22.md`](project-reconciliation-2026-08-22.md).

## The document model

Three tiers, each answering exactly one question. Introduced 2026-08-22 — before that, ADR
`Status:` conflated the design's maturity with the code's progress, which is why 29 of 56 headers
had drifted.

| Tier | Answers | Lifetime |
|---|---|---|
| [`../design/`](../design/) — ADR | *What are we building, and why?* | Permanent. **Contains no status.** |
| [`../implementation/`](../implementation/) — impl doc | *Where am I, what is the next action?* | Ephemeral — created when work starts, **deleted when it closes**. |
| **this file** | *What is in flight across the project?* | Permanent index, one line per item. |

**An ADR's `Status:` is now only ever `Decided` / `Draft` / `Superseded by NNNN` / `Withdrawn`** —
a statement about the *design*, never the code. There is deliberately no "Implemented". Whether
something is built is answered here, in §5–§6. Conventions and the impl-doc template:
[`../implementation/README.md`](../implementation/README.md).

## How to use this document — the trim rules

This doc tracks **status and open work only**. It must stay small enough to read in full.

1. **Detail lives in the ADR, never here.** A line here is a pointer plus what remains. If you find
   yourself explaining a design, put it in the ADR and link it.
2. **Done collapses.** An ADR with zero open items drops out of §6/§7 and becomes an ID in §5's
   Closed list. Do not leave a paragraph behind describing what it used to be.
3. **One line per open item.** If an item needs a paragraph, it is a workstream — promote it to §1
   or §3, or give it its own doc and link it.
4. **§1 holds exactly one workstream.** That is a policy, not an observation. Finishing beats
   starting; if §1 is full, §3 waits.
5. **Closing anything appends one line to §10** and deletes the entry. §10 is the only place that
   grows without bound, and it is one line per close.
6. **Bulk documentation drift is a chore, not work.** See CHORE-1 — do not create one tracking entry
   per stale header.
7. **Target ≤ 300 lines.** If it is longer, something that should have collapsed did not.
8. **Every session closes with an update** (standing convention, operator 2026-08-22): before a
   session ends, refresh §1's state, tick the active impl doc's checkboxes + log, and record any
   FOUND WORK — new defects, discovered gaps, process lessons — either under its owning ADR in §6,
   in the active impl doc, or (if unowned) in §7. Progress that lives only in a chat transcript is
   lost progress.

---

## 1. NOW — the single active workstream

**NONE — the clean stopping point RULING-10 asked for is reached (2026-09-07).** WS-CLOSE closed on Docker + MMO evidence (`7dde003` → `3d0e998` → `8c3cece`; the durable record is [`../reviews/ws-close-evidence-2026-09-07.md`](../reviews/ws-close-evidence-2026-09-07.md)); every implementation doc is deleted with its facts in the ADRs' dated `Design deltas (2026-09-07 …)` sections, and `docs/implementation/` holds only its README. What comes next is a **choice, not a commitment** — pick one and open its impl doc:

- **(a) The second batch** — RULING-11's deferred half plus the found work that rides with it: amplifiers **D/E/F** of the [collapse diagnosis](../reviews/mmo-collapse-diagnosis-2026-09-07.md) — defense fan-out + forming give-up bounds (0027/0035, `war.rs`; includes F8 and the garrison stand-down finding: an own-room Secure objective outlives its threat, the defense squad never stands down), tower peace refill + SURVIVAL-bid dedupe (0044 sink pricing), reserver gating on live miners (0018, `reserve.rs`) — plus **F11** (0047 write-then-swap: a tick killed mid-serialize resets the world).
- **(b) ADR 0048 review** — the multi-squad assault doctrine Draft; three operator questions in its §5; its sizing engine (the item-8a capped optimizer) is already in the tree, wired off.
- **(c) The queued §6 items** — F2 kernel duplicate-goal park (0025); the rally leg + live rout half F7/F9 (0034/0027); the 0036 reaches-0 re-observation; the segment-capacity fix lane (below); Phase 5's 0041/0010 remainder; Phase 6 economy.

**WATCH (live; attribute observations here first):**
- **Segment capacity (RULING-10 (iii): watch-only).** Measured 2026-09-07: the msgpack world stream is at **4 of 4 chunks, 169 KB of a 205 KB ceiling (~82%)**; 163 KB at the post-swap tail. The 0047 projection (12–14%) was wrong by ~6×; it grows with every claimed room and plan rebuild, and past the ceiling the world stops persisting each tick (and F11 turns a truncated write into a world reset). Re-measure on every tail. Fix lane (queued, not scheduled): profile the dominant component (plans suspected), shrink it AND widen the chunk budget inside the 10-segment ledger — designed together with F11 (0047 delta).
- **Live flips the next `reset.features` one-shot WIPES — re-apply after any one-shot:** `pathing.pathfinding_cpu_budget` 50 (default 20) and `remote_mine.reserve = false` (RULING-11, tick 5486228). `bucket_burst_threshold` was flipped 9500→2000 as the F12 mitigation (tick 5492700) and **reverted to 9500** after it drained the bucket; F12's code fix (finite normal-mode headroom) ships in `8c3cece` (deploy state: §4).
- **F13 · room visuals cost ~75 CPU/tick (max 126) on the 140 limit — `apply_visuals` was the single largest system** (per-system timing sample 2026-09-07 evening, 28 ticks): `visualize.on` + `room.visualize.on` flipped OFF live at tick 5493181 (cpu 140→68, bucket 1489→5606 in ~90 ticks); the compiled default of `VisualizeFeatures` is now `on: false` (this commit), so a future `reset.features` no longer re-enables it. Turn visuals on deliberately, never as the resting state. **F12 verified live** after `d38133ffad78`: normal mode (bucket < 9500) now paths (`move_failed_budget` 0, `ops_used` 38k/50k, creeps 96→112).
- **First boosted live engagement.** `boost_military` ON since 2026-08-24; no fight on either server has yet needed >T0 (the Docker world's fights are won at T0 or are L5 strongholds the bot correctly declines), so BoostQueue → labs → AwaitBoost is validated only in the offline 0028 boosted bed. Attribute lab/spawn/sizing changes here first.
- **F3 straggler**: the bloc gate releases with one traveller ~6 tiles back (crosses ~11 ticks late) — grade on a live crossing before touching the gather constants (0034 delta).
- **Shipped-but-unexercised defensive behaviors**: T-DEF-1 cover-anchoring, T-DEF-5 predictive safe-mode arm, T-POS-5 exit discipline, FU2 give-up on a COMMITTED fight — RED-pinned + sim-validated; the Docker provocation was too short to exercise them (raiders dead within 30 s; the AttackFlag route could not be armed remotely). The next sustained owned-room attack is the live grade.
- Others: wasm ~49% of the code limit; foreman `InvalidTarget` transients; post-hot-swap one-tick `INTEGRITY` squad-ref scrubs (benign backstop — attribute only if it recurs outside a deploy); boosted hostiles are priced at real strength (×2–×4), so defense sizing may grow vs boosted invaders and offense may defer fights it previously under-priced. ~~Movement CPU~~ CLOSED 2026-09-07 (zero over-budget lines two weeks post-swap; the post-swap 80–143 band was cache warm-up).

---

## 2. BLOCKED

- Nothing. ~~B-1 · `com.docker.service` Stopped/Manual~~ RESOLVED 2026-08-24; the private-server/harness lane ran to completion under WS-CLOSE.

---

## 3. NEXT — the completion roadmap (decided 2026-08-22)

Goal: **finish what is started before starting anything new.** Phases 0–4.5 and WS-CLOSE are done; the prose that used to live here is in the ADR delta sections and two review docs. One workstream active at a time (§1 — currently none).

- **Phase 0 — WS-1 (WFV 28 live, C1–C5)** — CLOSED 2026-08-23 (§10).
- **Phase 1 — WS-2 · Combat review Tier −1 Wave B** — shipped 2026-08-23 (`0d9524f2668f`), closed on Docker evidence 2026-09-07. Detail: ADR 0008a "Safe mode as built" (D2/D3), ADR 0027 delta (D28), ADR 0037 delta (T1/T2 retained), rover + 0008a (D9/D10); the [2026-07-09 review](../reviews/combat-systems-review-2026-07-09.md) carries the closed Tier −1 list.
- **Phase 2 — the triage pass** — DONE 2026-08-23 (verdicts `f3c822b`, `29072c3`).
- **Phase 2.5 — WS-6 · ADR 0047** — SHIPPED 2026-08-23 (WFV 29, the LAST format-transition reset). Capacity watch + F11 in §1/§6.
- **Phase 3 — WvC-1 / WvC-2 military completion waves** — shipped 2026-08-23 (`e08162810921`, `1fb233b30416`), closed 2026-09-07. Detail: ADR 0008a (T-HEAL-3a, readiness tranche, T-POS-5 / T-DEF-1 / T-DEF-5), 0035 delta (FU2 = `RetreatClock`), 0026 delta (L8), 0034 delta (D6c), 0028 delta (K3/K4 → `claim_pacing`; S5-CAP), 0037 delta (T3 emission closed by ruling).
- **Phase 4 — WS-4 · R19 chokepoint re-tune** — DONE 2026-08-23 (`9913ef980109`), closed 2026-09-07; its winner was superseded by Phase 4.5 item 6. Detail: ADR 0026 delta + 0026a adoption table (the `open_combat` profile history), 0031 delta + 0031b §5 (w_energy=1.0 re-sweep), 0019/0024 deltas (`s4_weights_retune`), 0033 (corpus-wide fence).
- **Phase 4.5 — WS-VAL defect program** — COMPLETE 2026-08-24 (items 1–7 + 8a + RULING-9 + the watched boost flip), closed 2026-09-07. Detail: ADR 0025 `Design deltas (2026-09-07)` A–G (RULING-9 currency, cohesion, border crossing, drain rework, capped optimizer, `threat_value` WORK/CLAIM, open items); 0034 / 0027 / 0035 / 0031 / 0024 / 0041 deltas; the corpus in ADR 0023a "WS-VAL corpus write-back" + [`screeps-combat-eval/README.md`](../../screeps-combat-eval/README.md) (gates, dashboards, debug env gates); the [parity audit](../reviews/live-sim-parity-audit-2026-08-23.md) (43 findings; the ranked backlog is closed except M14 → 0048 D3). **Item 8b = ADR 0048 Draft, awaiting operator review** (§1 (b)).
- **WS-CLOSE (RULING-10) + RULING-11 roots** — CLOSED 2026-09-07 → [`ws-close-evidence-2026-09-07.md`](../reviews/ws-close-evidence-2026-09-07.md) (program, D1–D8, F1–F12, evidence table) and [`mmo-collapse-diagnosis-2026-09-07.md`](../reviews/mmo-collapse-diagnosis-2026-09-07.md) (roots, amplifiers, ranked fixes).
- **Phase 5 — Boost pipeline (ADR 0010 L0 → 0041)** — consumer side P0–P3 shipped dark 2026-08-23, activated 2026-08-24 (ADR 0041 "WS-CLOSE write-back"; ADR 0010 delta). Remaining in §6 (0041: O4 resolver, D2 reservation, the 0031b sizing re-sweep, boost-tile routing; 0010: L1–L4). *Closes: 0010, 0041; unblocks 0019 boosted-TOUGH, 0020-TOUGH, 0008a Tier 3, 0008 S2.*
- **Phase 6 — economy completion.** The 0043 band→EV conversions (A2/A4/A7/A9/A10, A11, A12, C1–C7), 0042 `opportunity_floor` + R1–R4, 0044/0044a P3 all-sinks activation, 0007 item 4, 0040 §D8 reserve retirement. Mechanical batch work against a shipped market — the root-B income ladder (0040/0043 deltas) is the new baseline arm. *Closes: 0007, 0040, 0042, 0043, 0044, 0044a.*
- **Phase 7 — remaining designs.** WS-5 (0045 power creeps), 0020 S5–S7 (ratified; eligible now that Phase 4 is done), plus whichever of 0011/0012/0013/0014/0015/0016 the triage keeps (0013's spending half is already delegated to 0045; 0014 may reduce to the W4 `WarDecl` hook owned by 0008). New builds, so last by policy.

*Convergence: Phases 0–4.5 + WS-CLOSE took the corpus from 2 Closed to 9 Closed (+0030 Withdrawn, 0022 Superseded) in 17 days; the second batch, Phase 5's remainder, Phase 6 and Phase 7 are choices, not debts.*

---

## 4. Deployment ledger

| Where | Artifact | WFV | Date |
|---|---|---|---|
| Live MMO (shardX) | wasm `d38133ffad78` = `8c3cece` (F10 rally flap + F12 headroom + H5; hot swap 2026-09-07 20:39Z; earlier the same day `53a3c66f9633` = `3d0e998` and `e1595e7745f5` 2026-08-24). The visuals-off default (F13) is in the FOLLOWING commit and not yet deployed — the live flip covers it | **29** (0047 msgpack stream) | 2026-09-07 |
| Docker private | wasm `5b9aac9ee58e` = `8c3cece` (F10 re-observed: two W7N7 cores razed to 0; earlier `cbcd88671d5e` = `3d0e998` was the collapse before/after bed) | 29 | 2026-09-07 |
| `master` | HEAD (WFV-anchored; do not pin a SHA here — it drifts every commit) | 29 — **live on MMO** | since 2026-08-24 |

**The deployed-artifact test point is now `77dc9cc`** (2026-08-22); anything after it is undeployed. Use this as the test when an ADR claims a
deploy — pre-split ADRs claimed deploy dates predating the only real one (fixed by the doc split).
`wfv27-deployable-e857c76` is the historical WFV-27 point. Live MMO baseline 2026-08-23: **8 rooms**,
GCL 12, bucket 10000, W7N47 under remote-build.

---

## 5. ADR state index

57 ADRs. States: **Live** (in the deployed wasm) · **Host-only** (offline tooling, never in the wasm bundle) · **On master** (merged, undeployed) · **Partial** · **Design-only** · **Closed**.

**Closed — no open work. Detail in the ADR; do not re-track.** `0001` · `0004` (governor calibration run on evidence 2026-09-07, constants confirmed; its F11 finding is 0047's) · `0005` (containment ratified as-shipped 2026-08-23) · `0009c` · `0023` / `0023a` (S5 + rout bed + `MultiRoom` landed 2026-09-07; cross-room Flee closed per D4 — the seam-stitched field is 0024/0025's) · `0028` (beds 1+3 + the 0041 P3 boosted bed landed, live canary observed 2026-09-07; the three still-mirrored harness constants are noted in its delta, untracked) · `0036` (live raze confirmed on Docker 2026-09-07: under a Dismantle objective the W7N7 core went 29,600→4,000→0 and its successor 18,270→0 within two minutes of the F10-fixed build — hits strictly decrease AND reach 0; D3/D4 + D1 ordering as designed) · `0037` (T1–T3 landed; emission closed by ruling 2026-08-23) · `0038`

**Live** — `0002 0008 0010 0017 0019 0024 0025 0027 0029 0031 0031b 0032 0034 0035 0040 0041 0042 0044 0044a 0046 0047`
**Host-only** — `0006 0025a 0026 0026a 0033 0039`
**Partial** — `0003 0007 0008a 0009 0009a 0009b 0011 0012 0018 0020 0021 0031a 0043`
**Withdrawn** — `0030` (2026-08-23; tempo axis preserved in 0031 — no open work)
**Superseded** — `0022` (by 0027; its P-AUCTION residue is owned by 0020/0031, and its P-OBJ asks were superseded by 0027's observed-success model — no open work of its own)
**Design-only, zero code** — `0013 0014 0015 0016 0045`
**Draft (operator review pending)** — `0048` (multi-squad assault doctrine — Phase 4.5 item 8b deliverable, 2026-08-24; sizing engine = the wired-off item-8a machinery)

Open work for these is in §6 and §7. An ADR absent from both is Closed.

---

## 6. Open work by owning ADR

One line per item. **Phase tags**: P3 boost · P4 R19-tuning · P5 economy · P6 remaining designs · **BATCH-2** = the second batch (§1 (a)) · WATCH = live observation. (The HARNESS and WvC tags are retired — those lanes closed 2026-09-07.)

**Combat**
- `0008` — S2 boost handoff **→P3**; S1 pre-spawn, W2 trim + W4 `WarDecl` **→P6**; O5 power-bank + heavy assault = deferred capabilities (activate by decision, not schedule).
- `0008a` — T-HEAL-3 (widens into R1) **→P3**; Tier 3 **→P6** (after P3); **F4** the tower bounded-probe never resolves vs a visibly out-healed drainer ("T-DEF-3 note … F4") = small state-machine fix, on evidence; T-DEF-1 / T-DEF-5 / T-POS-5 live grade **→WATCH** (§1). (T-DEF-4 = the `threat_value` CLAIM term, D1 — done.)
- `0019` — boosted-TOUGH receiving-side reduction **→P3** (own-side boost pricing exists via 0041; the TOUGH damage-reduction curve does not).
- `0020` — S5/S6/S7 (blob auction + R7 currency, adaptivity, adversarial room-gen): operator-ratified end-state, Phase 7 (eligible now that Phase 4 is done). S5-CAP shipped (WvC-1; shared `claim_pacing`).
- `0025` — **F2** kernel duplicate-goal park (delta G: bar + repro) OPEN; the seam-stitched threat/approach field (§11 #10, ≤0.97 pin, D4) = the remaining cross-room design item (with 0024); goal-convergence churn + generated-bed fairness sign (delta G) on evidence; `action_oscillation_rate` metric **→P4**; `kite.rs` module doc still says "unboosted" (stale comment — a code-lane one-liner).
- `0026` — L6c **→P6** (no `DoctrineParams` consumer — its own rule). `0026a` — modes activate as their signals land **→P6**.
- `0027` — **F8** a lane-starved (never-spawned) roster → `mark_unwinnable` while a Defend objective re-claims every 400t forever, and the garrison stand-down finding (an own-room Secure outlives its threat) = amplifier D bounds **→BATCH-2**; Farm producers (PowerBank needs its own ADR) + salvage-teardown migration **→P6**.
- `0031` (+ `0031a`) — Tier-2 archetype search + Tier-3 axes **→P6**; `SIEGE_MAX_SIZED_MEMBERS` re-wire waits on 0048. (0031a/b re-sweep DONE, defaults confirmed — 0031b §5.)
- `0034` — convergence gates **→P4**; **F7/F9** the rally leg (sim: the rout ends at the seam — honest-baseline pin; live: the Retreating arm never steers to the rally) OPEN — the M23 economic give-up is the designed re-entry/give-up terminal; **F3 →WATCH**. (D6c shipped WvC-1; F10 rally flap FIXED `8c3cece`.)
- `0035` — FU1 **→P6** (poll-until-fresh sufficiency undecided); own-room forming give-up bounds **→BATCH-2** (with 0027). (FU2 CLOSED, WvC-1.)
- `0039` — P1 landed; P2–P4 **re-parked** (2026-09-07, D7): H5 supplies the fidelity bound §0 assumed; P2 = decision-crate extraction item, P3/P4 unscheduled sim-driver work; no harness dependency remains.
- `0048` — Draft awaiting operator review (§1 (b)); owns parity M14 (D3) and the border g3/g4 rungs (defer at sizing).

**Economy**
- `0007` — item 4 (route-distance hauler sizing + shared predicted capacity) **→P5**.
- `0010` — L0 live (delta 2026-09-07); reaction-selection kernel extraction + L1–L4 demand-driven planner/labs/market/factory **→P3**.
- `0012` — M2/M3 **→P6**.
- `0040` — §D8 #2: the 20% military reserve (`economy.rs:87`) never retired post-soak; owns review R15 **→P5**. (Root-B income ladder landed `3d0e998` — delta.)
- `0042` — `opportunity_floor` still hardcoded `0` (`squad_manager.rs`, gated on 0043 A2); R1–R4 refinements **→P5**.
- `0043` — A2's EV half, A4/A7/A9/A10's band step, A11 importance margin, A12 exponential backoff, C1–C7 vetoes **→P5** (the spawn-EV batch; its baseline = the root-B ladder, which the sim arms must adopt first — delta).
- `0044` / `0044a` — P3 all-sinks only partially activated (build/repair bids are admission gates, not EV-priced haul registrations); per-lane road awareness; Phase-3 verification never recorded **→P5**; tower peace refill + SURVIVAL-bid dedupe (amplifier E) **→BATCH-2**.

**Rooms, expansion, infrastructure**
- `0003` — `MissionResult::Wait/Idle` park-don't-teardown **→P6**.
- `0009` / `0009a` / `0009b` — planner revamp (bench evaluator gates it) **→P6**.
- `0011` — D5 assist, G3 incubation, empire spawn-budget orchestrator **→P6**.
- `0017` — M5b escort (owned by 0008 **→P6**); abort-threshold tune **→WATCH** (needs live attacker evidence).
- `0018` — reserver gating on live miners (amplifier F) **→BATCH-2**; K4 mineral + K-RECONCILE (incl. `HoldModel::Suppress` unification) **→P6**; SK-farm live evidence **→WATCH**.
- `0021` — follow-ups #5/#6 **→P6** (#1/#2 absorbed by 0046, live).
- `0046` — staleness-bucket quantization tune rides live observation (low priority; C1–C5 all passed) **→WATCH**.

**Platform / tooling**
- `0047` — **F11** write-then-swap (delta 2026-09-07: a tick killed mid-serialize resets the world) **→BATCH-2**, designed together with the segment-capacity fix lane **→WATCH** (§1).
- `0006` — H5 oracle LIVE-VALIDATED 2026-09-07 (five server-captured byte-exact vectors; `parity capture|report|nightly`; layer 2 report-only with `parity-budget.json`); remaining: the layer-2 budget promotion (report-only → gating) is an operator number under ADR 0015's earned-promotion rule — nothing else open.
- `0013` / `0014` / `0015` / `0016` / `0045` — design-only. 0015 (testkit + seam registry) and 0016 (HUD) were marked "in scope" by the ultracode completion kickoff, a program that has driven nothing since 2026-07-02 (RULING-5) **→P6**.
- `0041` — deltas written back 2026-09-07; open design items: O4 `mineral_value_e` resolver, D2 supply reservation, the 0031b sizing re-sweep half of P4, boost-tile routing **→P3**; first boosted live engagement **→WATCH** (§1).

---

## 7. Cross-cutting work with no ADR owner

- **UNOWNED-8 · live MMO economic collapse (operator-reported 2026-09-07)** — DIAGNOSED → [`../reviews/mmo-collapse-diagnosis-2026-09-07.md`](../reviews/mmo-collapse-diagnosis-2026-09-07.md); roots **A** (rover ops-pool movement wedge — ADR 0033 delta) + **B** (spawn-queue head-of-line deadlock — ADR 0040/0043 deltas) FIXED in `3d0e998`, Docker-validated on the collapsed July world (creeps 30→253) and LIVE (`53a3c66f9633`: `ops_used` 6589/50000, 0 move failures, W13N52 lane 301→4079); **F12** second wedge arm FIXED `8c3cece` (ADR 0004 delta); amplifiers D/E/F **→BATCH-2** (§1 (a)); live flips (`pathfinding_cpu_budget` 50, `remote_mine.reserve=false`) persisted but wiped by `reset.features` (§1 WATCH).
- ~~**UNOWNED-1 · Ship WFV 28**~~ CLOSED 2026-08-23 as WS-1.
- ~~**UNOWNED-2 · H5 sim-vs-server parity oracle**~~ CLOSED 2026-09-07 — the oracle exists (ADR 0006, `screeps-combat-engine::parity` + `screeps-ibex-eval parity`).
- ~~**UNOWNED-3**~~ CLOSED 2026-08-23 (ws-triage): the crate-wide `#![allow(dead_code)]` is gone; the compiler IS the dead-code register (new annotations carry a KEEP / TEST-PINNED / FOLLOW-UP owner tag).
- ~~**UNOWNED-4 · `remote_mine.search_radius` defaulted to `1`**~~ CLOSED 2026-08-23: default `2`, live-reconciled via `reset.features`.
- ~~**UNOWNED-5**~~ CLOSED 2026-08-23: the `SourceKeeperFeatures` container doc now records the operator's real 2026-06-18 default-ON decision.
- ~~**UNOWNED-6**~~ CLOSED 2026-08-23: `construction.allow_replan` deleted (declared, read by no code); re-add a replan flag together with its consumer when discretionary replan lands (0009).
- ~~**UNOWNED-7 · Stale `Memory._features` overrides**~~ CLOSED 2026-08-22 by the `reset.features` one-shot (`77dc9cc`): rebuilds the persisted tree from compiled defaults, self-clearing. **Deliberate retunes go through this pattern**, never a hand-edit that shadows future defaults — NB it also wipes the RULING-11 flips (§1).
- ~~**CHORE-1 · 29 ADR headers are stale.**~~ CLOSED 2026-08-22 by the design/implementation split (status moved here and to `../implementation/`; the drift class is structurally impossible now). Rollback tag: `pre-doc-split`.

---

## 8. Dead / unwired code register

Found 2026-08-22 by removing `#![allow(dead_code)]` and reading the compiler. Each is a decision —
wire it or delete it — not necessarily work.

| Item | Location | Note |
|---|---|---|
| `gameview.rs` | 104 lines, zero refs | The ADR 0006 seam Inc-6 record/replay and 0015's fakes both assume. Never migrated a single consumer. |
| `ui.rs` | 36 lines, `UISystem` never constructed | Doc comment claims consumers that do not exist. |
| ~~`BoostQueue`~~ | `military/boostqueue.rs` | **WIRED 2026-08-23** (ws-5 P3): manager files compounds, LabsMission fulfills, owner-staged clears. No longer dead — row kept for the register's history. |
| `issue_virtual_anchor_flee` | `military/formation.rs:398` | The **only** squad-level flee construct; nothing replaced it ⇒ squads have no coordinated retreat. Adjacent to review D10. |
| `Job::describe` layer | `jobs/jobsystem.rs:99,105` + ~15 jobs | Every job implements it; nothing dispatches it. A whole overlay with no renderer. |
| T1/T2 neighbour kernels | `war_decision.rs:182,327` | **Decided: retained by design** — sim/harness kernels (`run_v1_flow` proofs). WvC-2 ruling: NO offense-side feed — emission would contradict ADR 0037 T3 ("structurally incapable of opening a new attack path") + D27. Not dead code. |
| `HoldModel::Suppress` | `room_economics.rs:88,191` | Unreachable — SK farming runs a duplicate ROI kernel at `sourcekeeper.rs:99`. |
| `StructureIdentifier` | `structureidentifier.rs:7,32` | Superseded half of a live module. |

---

## 9. Rulings — decided 2026-08-22, do not relitigate

- **RULING-11 (operator 2026-09-07) — the live collapse (UNOWNED-8).** Live flips: APPLY
  `pathing.pathfinding_cpu_budget` 20→50 and `remote_mine.reserve=false` NOW (done tick 5486228,
  persisted); HOLD `military.defense=false` (defense stays on; the fan-out fix handles the
  harasser). Code: **roots A (rover wedge) + B (spawn-queue deadlock) NOW, riding the WS-CLOSE
  batch** (Docker refresh validates them, MMO hot swap together); amplifiers D/E/F (defense
  fan-out + give-up bounds, tower peace refill + survival dedupe, reserver gating) = a SECOND
  batch after the stop. NB both flips are wiped by the next `reset.features` one-shot — re-apply.
- **RULING-10 (operator 2026-09-07) — the clean stopping point.** Tie off EVERYTHING in flight
  before any new ADR build (0048 stays a parked Draft; multi-squad work is NOT started). Four
  sub-rulings: **(i) live-watch closure basis = PROVOKE IT ON DOCKER** — refresh the private server
  to the current build and run the offense-soak recipe + a seeded boost lab so the shipped-but-
  unexercised behaviors (0035 FU2 give-up clock, S5-CAP surge, T-DEF-1 rampart anchoring, T-DEF-5
  predictive safe-mode, T-POS-5 exit discipline, R19/`a2-i6-tight` profile, BoostQueue→labs→
  AwaitBoost) execute for real; the five ws docs close on THAT evidence, not on sim pins alone.
  **(ii) parity M17/M18 = UNIFY**: WORK (structure-threat channel) + CLAIM terms go into the shared
  `threat_value` so tower targeting and squad focus agree (kernel-wide; gates re-run). **(iii)
  segment capacity = WATCH ONLY** (no change this pass; numbers in §1). **(iv) the harness lane IS
  in scope** for the stop: H5 parity oracle (0006), 0036 live-raze, 0028 closeout, 0004 governor
  calibration, 0023/0023a border scenarios, M20–M23. Process: NO AI commit attribution (the
  project rule stands over the harness default); MMO/Docker reads + deploys authorized for
  validation, batched (slow realtime ticks).
- **RULING-9 (operator 2026-08-24)** — Phase 4.5 tail order: **heal-EV principled redesign NOW**
  (reprice heal in the same progress-diluted currency as attack — an ADR 0025 semantics change,
  done BEFORE the re-tune so the tune grades the final currency) → **item 6 boosted re-tune** →
  **boost activation flip, watched** (gated on the re-tune) → item 7 parity remainder → **item 8:
  BOTH capability directions, clamp first** (siege member-clamp lift lands first for L2–L3 reach;
  multi-squad assault doctrine is the follow-on design for L4–L5). Phase 4.5 completes before the
  private-harness lane.

Recorded because the corpus contradicted itself and a future reader would otherwise reopen these.

- **RULING-1 · Minted `SquadId`/`SquadStore` (I1/I2) will NOT be built.** `EntityOption<Entity>` +
  `repair_entity_integrity` is the end state (ADR 0001, REC-009b). Three sources disagreed
  (0008 listed it open, 0020 said "dropped per 0022 D1", plan §3 and phase-2 CP-I list it blocking).
  ⇒ **CP-I is retired, not pending.** 0008 was retargeted to the marker-converted `squad_entity` in the 2026-08-22 doc split; plan §3 is historical.
- **RULING-2 · "Live" means "in the deployed wasm artifact."** Offline harnesses are **Host-only**,
  a separate state. Previously both were called Live, making "is it live?" unanswerable.
- **RULING-3 · D27 is closed AND created dead code.** Both facts stand; ADR 0037 owns the cleanup.
- **RULING-4 · ADR 0022's "no MMO deploy until all roadmap objectives are complete" is VOID.**
  `ab692bd` shipped with P-AUCTION and parts of P-OBJ unbuilt. Left unamended it reads as a standing
  block on every future deploy.
- **RULING-5 · The ultracode completion kickoff is dormant, not live.** It has driven nothing since
  2026-07-02. Do not treat its "in scope" list as commitments.
- **RULING-6 · R19 does not gate Wave B.** R19 gates *kernel-parameter tuning*; Wave B is safe-mode
  constants, roster/formation logic and adapter wiring. It **does** gate 0024 FU#4, 0031a Tier-2/3,
  0031b's re-sweep, 0032's `value_e` tuning and 0026 L6c — all of which currently list tuning as
  their next action without acknowledging it. Resolve in WS-4.
- **RULING-8 · Testing posture (operator 2026-08-23): deploy-to-live, batched.** The Docker soak
  lane is NOT a deploy gate until the operator is home. Validation = the offline sims/pins/fence
  where possible; otherwise test on live with batches large enough that a reset is acceptable.
  Rationale: empire impact is acceptable — the real cost is RECOVERY latency (MMO ~1 tick/sec +
  re-scout/re-plan convergence), which batching amortizes and no-WFV hot swaps avoid entirely.
  ADR 0047 (Draft) is the structural fix: reset-tolerant serialization so shape changes stop
  costing a recovery at all.
- **RULING-7 · Three distinct quantities are called `opportunity_floor`** — `market_adapter.rs:105`
  (computed, discarded), `transfersystem.rs:1669` (the one consumers see), and ADR 0042's forming
  give-up floor (hardcoded `0`). Name them separately; they are not one thing.

**Single owner for previously-duplicated items:** 20% military reserve → `0040` · `MAX_CONCURRENT_SQUADS`
→ `0020` · multi-squad lane contention → `0028` · boosted-TOUGH → `0041` · weapon archetype → `0031`
· BoostQueue → `0010` · `available_boosts` → `0010` · SK mineral K4 → `0018` · W2/W4 + S1/S2 →
`0008` · activator signals → `0026a`.

**Verified closed — do not reopen** (plan/phase-2 still list some as open): W3 escort producer
(`claim.rs:1269`, `81ed7f2`) · K2c-2 yield-to-defense predicate (`sourcekeeper.rs:337`) · U-TOWER
(`tower_fire.rs` → `missions/tower.rs:353`) · G workstream in full (legacy attack path deleted) ·
review D1/D11/D24/D25/D26/D27/R22 (Wave A).

---

## 10. Changelog

Append one line per closed item. Newest first.

- **2026-09-07 (close-out)** — **WS-CLOSE CLOSED — the RULING-10 clean stopping point reached.** `7dde003` (Phase A+B: M17/M18 unify, M20–M23 both sides, 0023/0023a S5 + rout bed + `MultiRoom`, the H5 Docker-free half, 0004 prep, 0028 beds 1+3, the 0041 P3 boosted bed, engine seam fidelity F1; + the collapse diagnosis) → `3d0e998` (RULING-11 roots A+B + Phase D write-backs into 20 ADRs) → `8c3cece` (Phase C: H5 byte-exact ×5 on the real server, F10, F12, lint sweep). Docker `cbcd88671d5e` + MMO hot swap `53a3c66f9633` (= `3d0e998`). Durable record → `reviews/ws-close-evidence-2026-09-07.md` (program, D1–D8, F1–F12, evidence table). Seven impl docs deleted (ws-2, wvc-1, wvc-2, ws-4, ws-5, ws-val, ws-closeout); §1 empty by design; §5/§6 collapsed; F11 → 0047 delta, F12 → 0004 delta.
- **2026-09-07** — **ws-2 Wave B CLOSED** (shipped `0d9524f2668f` 2026-08-23): D2/D3 → ADR 0008a "Safe mode as built", D28 → 0027 delta, T1/T2 → 0037 delta, D9/D10 rover-side; closed on pins + write-back — the Docker pass exercised the offense lifecycle (three squad generations, FU2 backoff, two Dismantle objectives, one core razed) but recorded no dedicated churn-signature row; D4/D5 stay pinned offline.
- **2026-09-07** — **wvc-1 CLOSED** (shipped `e08162810921`): T-HEAL-3a, readiness tranche, S5-CAP, FU2 `RetreatClock`, L8, D6c, K3/K4 `claim_pacing` in ADR deltas (0008a/0035/0026/0034/0028/0020); live canary on Docker: a Secure objective fielded a 6/6 in-room defense squad within 30 s (S5-CAP / defense claim PASS); FU2 backoff observed on W7N5.
- **2026-09-07** — **wvc-2 CLOSED** (shipped `1fb233b30416`): T-POS-5 / T-DEF-1 / T-DEF-5 as built in 0008a (+0019 delta), 0037 T3 closed by ruling (0037 delta), lane contention → 0028 bed 3; T-DEF-1 anchoring / T-DEF-5 arm NOT provoked live (fight too short) → §1 WATCH.
- **2026-09-07** — **ws-4 CLOSED** (shipped `9913ef980109`): R19 winner + the 0026a reversal → 0026/0026a; 0031a/b w_energy=1.0 → 0031b §5 + 0031 delta; S4-TUNE / FU#4 / kite → 0019/0024 deltas; the winner was superseded by `a2-i6-tight` (item 6) under the RULING-9 currency.
- **2026-09-07** — **ws-5 CLOSED**: P0–P3 as built + P4-as-merged + activation provenance → ADR 0041 "WS-CLOSE write-back"; L0 as shipped → ADR 0010 delta; 0020 rows corrected; the apply loop validated offline (`boosted_roster_routes_to_the_labs_and_departs_boosted`, `stock_loss_mid_await_boost_falls_through_at_the_deadline`); first boosted live engagement → WATCH.
- **2026-09-07** — **ws-val CLOSED**: corpus → ADR 0023a "WS-VAL corpus write-back" + `screeps-combat-eval/README.md`; kernel work → ADR 0025 deltas A–G; the boost-blind seam → 0041; parity audit updated (M4/M6 struck, M14 → 0048 D3, M17/M18/M20–M23 FIXED).
- **2026-09-07** — **ADR 0004 CLOSED**: governor calibration run on evidence (`runs/pressure-critical-hover-3d0e998-20260907-200026`, warm 10-room world): tiers fire as designed (normal→conserve 6945/−10 → critical 3128/−54; recovery 1764 → 4195), constants CONFIRMED; F11 found → 0047; F12 delta recorded.
- **2026-09-07** — **ADR 0006 H5 oracle LANDED + LIVE-VALIDATED** (UNOWNED-2 closed): `screeps-combat-engine::parity` + `screeps-ibex-eval parity capture|report|nightly`; five SERVER-CAPTURED byte-exact vectors (melee-1v1 7 frames, tough-ladder 9, kite-r3 11, heal-race 13, tower-rampart 13; 0 placeholders); layer 2 report-only with `parity-budget.json`; Q1 answered (DB-inserted creeps tick). Remaining = the budget promotion (operator number).
- **2026-09-07** — **ADR 0028 CLOSED**: bed 3 (`run_multi_forming`, both K4 arms) + bed 1 at N=2 + the 0041 P3 boosted bed (`run_boosted_forming`) + M20–M23 shared clocks; live canary: defense claim/surge fielded 6/6 in-room on Docker.
- **2026-09-07** — **ADR 0023 / 0023a CLOSED**: S5 GROUP-UP + STUCK-MEMBER-TIMEOUT beds, the squad rout bed, the `MultiRoom` generator (48-index family + builder aliases), engine seam fidelity fixed at source (F1); cross-room Flee closed per D4 — the seam-stitched field stays a 0024/0025 design item.
- **2026-09-07** — **ADR 0039 P2–P4 re-parked** (D7): H5 supplies the fidelity bound §0 assumed; no harness dependency remains.
- **2026-09-07** — **UNOWNED-8 roots A+B FIXED + LIVE** (`3d0e998` = wasm `53a3c66f9633`): rover ops refund + `PathBudgetExhausted` / displaceable occupant + first-path round-robin (ADR 0033 delta); miners CRITICAL + bootstrap floors + need-scaled hauler bid + starvation sizing (ADR 0040/0043 deltas). Docker before/after: creeps 30→253, 3 new rooms; MMO: `ops_used` 6589/50000, 0 failures, W13N52 lane 301→4079. **F12** second wedge arm FIXED `8c3cece` (ADR 0004 delta); amplifiers D/E/F → the second batch.
- **2026-09-07 (close)** — **ADR 0036 CLOSED** (live raze reaches 0 on Docker under the F10-fixed build: W7N7 4,000→0 and 18,270→0). **F13** visuals-off flip + default. Live MMO after the day: creeps 46→112, storage energy back in 4 of 7 rooms, cpu 68/140, bucket climbing +44/tick, zero budget failures.
- **2026-09-07** — **F10 rally-room flap FIXED** (`8c3cece`, ADR 0034 delta): evidence classifier over the persisted visibility record; 10 RED pins. **0036 D3/D4 live-confirmed** (W7N7 core 29600→4000 under Dismantle; W7N5 razed); the reaches-0 re-observation is pending.
- **2026-09-07** — **Parity M17/M18 (tower/kernel unification, D1/D2) + M20–M23 FIXED** — audit updated.
- **2026-09-07** — **RULING-10 recorded; WS-CLOSE opened** (the tie-off program, §1). Movement-CPU watch CLOSED on evidence (zero over-budget lines two weeks post-swap); segment capacity MEASURED 4/4 chunks, 169 KB/205 KB (watch-only by ruling); §4 ledger refreshed to `e1595e7745f5`/WFV 29; UNOWNED-4 struck (closed 2026-08-23, never marked).
- **2026-08-24 (latest)** — **Phase 4.5 item 8b: ADR 0048 multi-squad assault doctrine DRAFTED** (N coordinated 8-squads, 4 coordination points, joint sizing = the item-8a machinery; closes parity M14 as a by-product). Awaiting operator review — the LAST Phase 4.5 item. Item-8a batch deployed to MMO (wasm `e1595e7745f5`, tail clean).
- **2026-08-24 (latest)** — **Phase 4.5 item 8a: siege clamp lift MACHINERY LANDED, WIRED OFF (measured red)** + shipped by-products: honest gauntlet verdicts (Killed = core RAZED), in-contact stall gate BOTH sides (fixed the border-g1 period-2 room-edge flap), kernel out-of-contact handoff (fixed the mid-room rigid-body park) — border g1/g2 now kill END-TO-END. L2+ path = item 8b multi-squad doctrine. decision `d693229`, agent `380ceb3`, eval `6c01505`.
- **2026-08-24 (latest)** — **Phase 4.5 item 7 (parity remainder) ACHIEVED**: H6/H8/H9 + M10/M11/M12 landed both sides (sim shove depth 10, flee-knob parity, live engaged-ladder retreat arm, live tower heal_reaching + chip fallback deleted, forming_state departed gate); LIVE friendly_creep_distance 15->5 (pre-tournament hand-tune, made crossings arrive strung out). Remainder: M14+M17/M18 -> item 8/design fork; M20-M23 -> harness lane.
- **2026-08-24 (latest)** — **boost_military ACTIVATED on live MMO (WATCHED)**: Memory._features.military.boost_military = true on shardX via the new rest-api console example (`eeabd42` — POST /api/user/console, shard auto-injected); verified by console echo of the military tree; 0 panics post-flip. WATCH: first boosted engagement (labs fill BoostQueue, AwaitBoost job, offense sized per tier); attribute lab/spawn/sizing changes here first.
- **2026-08-24 (latest)** — **Phase 4.5 item 6 (boosted re-tune, merges 0041 P4) ACHIEVED**: `open_combat` -> `a2-i6-tight` (decision `173cd5f`) via the new joint tier x terrain 9-cell maximin (eval `8997234`); the R19 profile regressed to worst -852 under the RULING-9 currency. Battery + fence green. Unblocks the watched boost_military flip.
- **2026-08-24 (latest)** — **RULING-9 one-currency EV redesign ACHIEVED + shipped** (decision `f8b97a6`, agent `34152cd`, eval `04c7414`): progress-diluted heal value_per_hp (triage form) + MARGINAL self-risk price (flat NET_RISK_MULT deleted) + kite dead-zone fall-through + our_dps=0 focus gate + heal-incumbency dead-band. All 6 gates + 1537 workspace + fence green. Full detail in ADR 0025 Design deltas A.
- **2026-08-23 (late)** — **ADR 0041 P0–P3 consumer side complete, dark, shipped** (`00cf29b552ff`): EV axis + per-tier winnability (decision `04cc020`), supply table/clamp (`d217a3d`), persisted tier + required_boosts (`78e70b8`), apply wire (`2fd8253`+`82f2e83` — queue/labs/AwaitBoost/renew-skip). 7 RED-verified pins; fence ×2; byte-identical live at T0. `boost_military` = the activation switch.
- **2026-08-23 (late)** — **Live validation PASS** (CPU 34/140, bucket full, 0 panics, seg ~18%, INTEGRITY scrubs = benign REC-009b backstop) + **Phase 5 P0a landed dark** (decision `04cc020`): BoostTier EV axis, per-tier ceiling assessment, T-TOWER-3 proof green, 3 RED-verified pins; not separately deployed (byte-identical at T0). ADR 0041-P2 WFV bump obviated by 0047 (design delta).
- **2026-08-23 (late)** — **Phase 4 / WS-4 R19 re-tune DONE + shipped** (`9913ef980109`): `chokepoint_comp_basket` + maximin tournament (eval `940f739`); `open_combat` → a0-i3-d14-K3-s2 (decision `a7acb0b` — only cross-regime-positive config; old profile NEGATIVE vs untuned default, 0026a rejection reversed-with-reconciliation); 0031a/b re-swept at w_energy=1.0 → defaults confirmed, margin knobs inert (0031b §5); S4-TUNE/FU#4-presets/0033-kite-retune closed defaults-confirmed (`s4_weights_retune`, eval `bbc1184`); 0033 corpus-wide fence promoted (rover-eval `ab3e818`, spread==0/21, H 0.9625); L6c + value_e re-tagged →P6 (consumer/bed-gated). Process: run fences in RELEASE (18s vs 385s).
- **2026-08-23 (late)** — **WvC-2 code-complete + shipped** (hot swap `1fb233b30416`): T-POS-5 exit-tile surcharge (decision `3d451ac`), T-DEF-1 rampart cover via `ThreatField::build_covered` (`47a163a` — TAKEN/EV-risk/survival-veto/traversal all inherit the redirect from one point), T-DEF-5 predictive safe-mode arm (`8502af9`); 0037-T3 emission closed by ruling (contradicts the ADR's no-new-aggression seam + D27), 0028 lane-contention re-routed to the harness closeout. Fence green ×2 this session.
- **2026-08-23 (late)** — **WvC-1 code-complete, all 7 items**: T-HEAL-3a winnability inputs (`c5a06c8`), defender spawn-readiness wired + tower half deleted as U-TOWER-superseded (`81ee72f`), S5-CAP empire-scaled cap + defense surge (`7a87df5` → shared kernel `13112e6`), 0035 FU2 closed (veto attempt `4d044be` reverted `4d186d8` after 2 eval-bed regressions — the probe bounce is load-bearing; final = stall-aware give-up clock + engaged-gated stall streaks, agent `0c57c45`), 0026 L8 observed-owner coordination (`0455298`), 0034 D6c renewable-rally bias (`e6aa3ce`), 0028 K3/K4 resolved as-built (ADR rewritten). Ship + live-watch pending.
- **2026-08-23** — **Operator reorder: military first.** Boost pipeline (largest NEW build) demoted behind the military completion waves (WvC-1 correctness+wiring, WvC-2 defensive features) and the P4 re-tune — finish partial machinery + kill the bug farm before feeding it boosts. WvC promoted out of the old Phase-6 into Phase 3; NOW = WvC-1.
- **2026-08-23** — **WS-6 SHIPPED + CLOSED: ADR 0047 live at WFV 29** (msgpack struct-map stream + foreman Plan shrink `5c89f30` — road_network deleted, build_order on-demand; plans ~70% smaller). The LAST format-transition reset paid; additive changes are now reset-free. Live: 2.8% of segment budget mid-rebuild (proj. 12–14% full), named decode FASTER than old bincode. Costs recorded: wasm +71% (48.5% of code limit). Operator constraint recorded: plans are durable state, never recompute-after-reset. ws-6 doc deleted.
- **2026-08-23** — **ADR 0047 → DECIDED**: whole-stream msgpack struct-map, ONE encoding (operator simplicity steer, confirmed by round-2 data: 30.4% of the real 400KB segment budget; RoomPlanData=86% of bytes and shape-stable; real-world named round-trip works; sectioning rejected as unnecessary). WS-6 remaining: the game_loop swap (one WFV bump, batched).
- **2026-08-23** — **Phase 2 (triage) CLOSED**: final sweep phase-tagged every §6 line (P3/P4/P5/P6/WvC/HARNESS/WATCH); 0005 containment ratified as-shipped → Closed; ws-triage doc deleted. WS-6 (0047 benches) is NOW.
- **2026-08-23** — **WS-1 CLOSED: C1–C5 ALL PASS.** The pipeline claimed **W7N47** (dist 4, above-ring, score 0.835) — 8 rooms; RemoteBuildMission constructing. L2 ruled OBVIATED by 0046's bounded-retry machinery. 0046→Live, 0038→Closed. WS-1 doc deleted per lifecycle.
- **2026-08-23** — Triage decisions ratified (operator): 0030 Withdrawn (tempo→0031), 0025a residual documented-mitigated, 0039 P2–P4→harness lane, 0020 S5–S7 kept-scheduled (after Phase 4); **0047 pulled forward as Phase 2.5 (WS-6)**. `search_radius` 1→2 shipped + live-reconciled (wasm `bd6eebcc0f56`, hot swap, pattern proven twice). UNOWNED-4 closed.
- **2026-08-23** — **Wave B SHIPPED to live MMO** (hot swap `0d9524f2668f` per RULING-8; vm_starts 2749, missions persisted, 0 deser). RULING-8 recorded (deploy-to-live batched; B-1 demoted); ADR 0047 drafted (reset-tolerant serialization).
- **2026-08-24 (latest)** — **Phase 4.5 item 4 (parity H0): FIXED** — `is_combat_targetable` in squad_combat.rs: the execution-side structure list now includes neutral constructed walls with hits (both the cached arm and the live find fallback), so kernel-chosen wall breaches resolve instead of silently dropping. H5/M0 also closed (item-2 batch). Parity report triage updated; no native pin (JS types) — live-verify on the next neutral-wall breach. Master replay index (all six lenses) generated + delivered.
- **2026-08-24 (item 5 partial)** — drain delivery honesty + heal-premium rebalance shipped (decision `acf3600`, agent `bed5f0e`); the sim-field delegation + M2 + live mover threat layer DEFERRED into a drain-rework sub-batch with four precisely-traced findings (form-into-the-nest tank bleed is pre-existing and invariant; the drain canary passes on remnant luck). SQ_DEBUG trace instrumentation landed. 1537 + fence + wasm green.
- **2026-08-24 (item 5 COMPLETE)** — the threat/traversal unification cluster is CLOSED both sides: sim field = shared build_room_threat_field + M2 (agent `4e68de4`), plateau tie-break (decision `71c6e0a`), drain-aware placement (eval `1736ea6`), live mover threat overlay (H3/M13: RoomThreatCosts + ThreatOverlayCostSource, rover `fae493b` merge_from) and room-correct decide callback (M4/M6) — super `1abcc8f`. 1537 + fence + wasm green.
- **2026-08-24 (later)** — **Phase 4.5 item 2: border crossing — ACHIEVED** (decision `b0b7ea0`, agent `1fbff1b`, eval `8dab84d`; root-caused live from the operator's replay observation "one creep enters and everything outside the room stalls"): bloc crossing gate + full-roster views (parity H5) + fight-room kernel anchoring (the centroid-room V-1 aliasing) + room-gated mover anchor + exit-edge pricing + room-local tower assessment + rout-to-rally + state decay + a latent twin_room_siege fixture bug. **Every fielding gauntlet rung now KILLS** (L1 open/choke/choke-multi + border g1×2/g2 — pinned). Replay viewer regenerated + sent; 1537 workspace + fence + wasm green. Item 1's choke trickle-in tail closed by the same batch.
- **2026-08-24** — **Phase 4.5 item 1: cohesion under focused fire — open-layout bar ACHIEVED** (decision `be725c9` + agent `e3660d8` + eval `17d2d74`): four composing EV-kernel fixes (deliverable heal, siege risk-currency floor + ×4 uncovered steepener [later deleted by RULING-9 — only the `g_us.max(unit)` floor survives], lockstep healer advertising, evidence-gated urgent heal triage). L1-open@T3 → Killed/151 ticks/zero losses (was timeout-freeze → 611-tick kill with 5 deaths). Floor pin upgraded; permanent `probe_rung` trace instrument; healers-first-sort attempt reverted on the oscillation gate (documented in kernel.rs). 1537 workspace + fence + wasm green. Remaining item-1 tail: choke trickle-in.
- **2026-08-23 (later)** — **WS-VAL phase closed out**: MMO hot swap tail-verified clean (new `tail.rs --server` reads `.screeps.yaml` directly — no more env-token dance); found work swept into §3 **Phase 4.5** (the defect program, queued next per operator), §6 refreshed (0010/0041/0025/0023 were stale vs shipped code), §8 BoostQueue row closed.
- **2026-08-23 (late)** — **WS-VAL corpus landed**: engine-exact stronghold gauntlet + border gauntlet + boosted self-play lane; boost-blind seam fixed live+sim (shared `effective_output`, heal_power un-latched); ultracode parity audit (43 findings; H4/H7/M15/M16/M19 fixed, rest triaged in `docs/reviews/live-sim-parity-audit-2026-08-23.md`); pre-existing `screeps-prospector` breakage fixed (`Plan.build_order` → `compute_build_order`). 1536 workspace tests + fence + wasm green. Honest baseline tables in the ws doc (now ADR 0023a); tactical follow-ups queued (cohesion under fire is the binding defect).
- **2026-08-23** — **Wave B CODE-COMPLETE**: D9/D10 landed (`1a85a57` + rover `850a06b` — shared engaged ladder now ONE implementation in rover, wired live; flee uses partial paths); T1/T2 ruled retained-by-design. 15 pins total. The 2026-07-09 review Tier −1 work list is closed; soak pending B-1. World checks 1–3 healthy.
- **2026-08-22 (late)** — Wave B 6/8: D2/D3 safe-mode (`8fa0c60`), D4/D5/D6 roster churn (`be5ce24`), D28 vacuous clear (`b26eba4` + decision/eval submodules). 13 RED-verified pins; fence green; ADR 0027 amended. D9/D10 + 0037 decision remain. WS-1 observation healthy through 3 checks (claim pipeline live, C2 signature absent).
- **2026-08-22** — **WFV 28 DEPLOYED TO LIVE MMO** (`77dc9cc`, wasm `d9b748497e4a`; operator inverted soak order, MMO-first). Loud reset clean: 0 panics, CPU 52→37/140, bucket 10000. `reset.features` one-shot built + fired + verified — live config at compiled-default parity (offense back ON; Wave A fixes in-artifact). Closes UNOWNED-7. Observation window open (C1–C5).
- **2026-08-22** — Completion roadmap (Phases 0–6) recorded in §3; §1 updated to MMO-first.
- **2026-08-22** — **Design/implementation split.** All 56 ADRs rewritten as pure end-state designs; status moved here and to `../implementation/`. Status vocabulary reduced to Decided/Draft/Superseded/Withdrawn (+ note types). Closes CHORE-1 structurally. Adversarial verify caught 4 design-loss regressions and 19 lesser ones, all remediated and re-verified. Rollback tag: `pre-doc-split`.
- **2026-08-22** — Full ADR-corpus reconciliation (56 verified, 29 drifted); this tracker created; rulings 1–7 recorded.
- **2026-08-22** — Repo tie-off: ADR 0046 merged (WFV 28), working tree emptied, all branches/worktrees removed, master + 49 submodule commits pushed, ADR 0044a renumbered, 0038/0042 headers fixed.
- **2026-07-28** — Combat Wave A shipped to MMO (`ab692bd`): D1/D11/D24/D25/D26/D27/R22. CPU 87→16.
- **2026-07-06** — ADR 0040 accepted; WFV 27 to MMO.
