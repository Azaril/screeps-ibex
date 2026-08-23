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
5. **Closing anything appends one line to §9** and deletes the entry. §9 is the only place that
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

### WS-1 · Get WFV 28 verified and live → [`../implementation/ws-1-ship-wfv28.md`](../implementation/ws-1-ship-wfv28.md)

**State: ACTIVE — deployed straight to MMO 2026-08-22 (operator decision), observing.**

B-1 blocked the private lane, so the operator inverted the order: MMO directly (pre-authorized
2026-08-11), with the soak plan's C1–C5 criteria judged against live. Full record: the WS-1 doc.

| Step | State |
|---|---|
| MMO deploy (loud reset WFV 27→28) | **done 2026-08-22** — see WS-1 doc for verification |
| One-shot `reset.features` fired — `Memory._features` reconciled to compiled defaults (`77dc9cc`) | done — note: this turns `military.offense` back ON (compiled default; had been manually off since the July drain era — Wave A's fixes are in this artifact) |
| Observe one discover cycle, judge C1–C5 live | in progress — **healthy through 3 checks** (CPU 14–36/140, bucket pinned 10000, 0 panics, 0 drain signatures). The claim pipeline is live and holding its L3 Select window; C2's failure signature (stale-intel skip) is ABSENT; below-ring candidates correctly deferred on ring patience while the post-reset frontier re-scouts. C3/C5 verdicts pending frontier coverage. |
| L2 poison-list self-heal — ships **last** of the expansion program | pending live evidence |
| Private soak (when B-1 clears) — now for the harness lane, not this deploy | deferred |

> WS-2 runs concurrently: WS-1's remaining work is passive wall-clock observation (a 30-min review
> cadence), so the one-workstream policy treats the pair as one active lane.

---

## 2. BLOCKED

- **B-1 · `com.docker.service` Stopped/Manual (needs elevation) — DEMOTED by RULING-8** (operator
  2026-08-23): the private lane no longer gates ANY deploy; it gates only the harness work (H5
  parity oracle, P2.M2-LIVE, M4 exit criteria, 0036 live-raze, 0028 closeout), all deferred until
  the operator is home. Fix when convenient: elevated `Start-Service com.docker.service` +
  `Set-Service … -StartupType Automatic`; symptom is `docker ps` HANGING. Detail: soak plan §0.

---

## 3. NEXT — the completion roadmap (decided 2026-08-22)

Goal: **finish what is started before starting anything new.** Ordered so each phase closes the
maximum number of open ADR tails; Phase 2 is a *decision* pass, not a build pass. One workstream
active at a time (§1).

**Phase 0 — close WS-1** (above). Observe live, judge C1–C5, L2 last. Ride-alongs while in the
deploy loop: ~~UNOWNED-4~~ **retuned 1→2 + live-reconciled 2026-08-23** (UNOWNED-5/6 also closed
— features.rs doc fixed, `allow_replan` deleted). *Closes: 0046→Live, 0038, 0017's deploy residue, 0021 re-head, the
expansion program; collects the 0018 "has an SK farm ever run" evidence for free.*

**Phase 1 — WS-2 · Combat review Tier −1 Wave B** →
[`../implementation/ws-2-combat-wave-b.md`](../implementation/ws-2-combat-wave-b.md).
**CODE-COMPLETE 2026-08-23**: D2/D3 `8fa0c60`, D4/D5/D6 `be5ce24`, D28 `b26eba4`, D9/D10
`1a85a57` + rover `850a06b`, T1/T2 ruled retained-by-design. 15 RED-verified pins, no WFV;
ADR 0027 amended. **SHIPPED to live 2026-08-23** (hot swap `0d9524f2668f`, world persisted — missions carried through, 0 deser errors). Live-watch, then the WS-2 doc deletes.
*Closes: 0037's decision item and the 2026-07-09 review's Tier −1 as a live work list.*

**Phase 2 — the triage pass: decide, don't build** (one session; create its impl doc — the
verdicts must land as ADR amendments via `Design deltas`). Every §6/§8 item gets one of three
verdicts: **schedule** (into Phases 3–6), **amend out** (shrink the ADR's end state — candidates:
0030 `EngagementTempo` → Withdraw/fold into 0031; 0020 S5/S6/S7 keep-or-cut; 0026a's six deferred
modes; 0039 P2–P4; 0025a residual → close as documented-mitigated), or **do now** (the §8
wire-or-delete one-liners, then **UNOWNED-3: remove `#![allow(dead_code)]`** so the compiler
enforces the register). *Exit criterion: every remaining §6 line is inside a scheduled phase.
Plausibly closes 6–10 ADRs by amendment.*

**Phase 2.5 — WS-6 · ADR 0047 experiments** (operator pulled forward 2026-08-23): the offline encoding bench matrix (real payload, native + wasm, sizes post-deflate) per 0047 §Experiments; promote the ADR to Decided with numbers. Every future WFV change gets cheaper the sooner this lands.

**Phase 3 — WS-3 · Boost pipeline (ADR 0010 L0 → 0041).** The biggest completion unlock: 0041
gates review risk R1 (enemy-boost blindness, the top MMO risk) and the whole boosted-assault
frontier, but is **blocked on 0010** — nothing calls `boostCreep`; `BoostQueue` is a dead pipe.
*Closes: 0010, 0041; unblocks 0019 boosted-TOUGH, 0020-TOUGH, 0008a Tier 3, 0008 S2.*

**Phase 4 — WS-4 · R19 chokepoint re-tune.** One tournament pass on realistic terrain (entry point
already committed: rover-eval `c4b3d17`). *Closes six tuning tails as a batch — 0019 S4-TUNE,
0024 FU#4, 0026 L6c, 0031a/0031b's invalidated sweeps, 0032 `value_e`, 0033 kite retune — which is
why RULING-6 held them.*

**Phase 5 — economy completion.** The 0043 band→EV conversions (A2/A4/A7/A9/A10, A11, A12,
C1–C7), 0042 `opportunity_floor` + R1–R4, 0044/0044a P3 all-sinks activation, 0007 item 4,
0040 §D8 reserve retirement. Mechanical batch work against a shipped market.
*Closes: 0007, 0040, 0042, 0043, 0044, 0044a.*

**Phase 6 — remaining designs, only what survives Phase 2.** WS-5 (0045 power creeps), 0020 S5–S7 (after Phase 4, ratified), plus
whichever of 0011/0012/0013/0014/0015/0016 the triage keeps (0013's spending half is already
delegated to 0045; 0014 may reduce to the W4 `WarDecl` hook owned by 0008). New builds, so last
by policy.

*Convergence: Phases 0–2 ≈ a week of sessions, taking the corpus from 2 Closed to ~15–20 Closed;
Phases 3–5 are the three real build programs; Phase 6 is a choice, not a debt.*

---

## 4. Deployment ledger

| Where | Artifact | WFV | Date |
|---|---|---|---|
| Live MMO (shardX) | `77dc9cc` (wasm `d9b748497e4a`) | **28** | 2026-08-22 |
| Docker private | `ab692bd` (stale — refresh when B-1 clears) | 27 | 2026-07-28 |
| `master` | HEAD (WFV-anchored; do not pin a SHA here — it drifts every commit) | 28 — **live on MMO** | since 2026-08-22 |

**The deployed-artifact test point is now `77dc9cc`** (2026-08-22); anything after it is undeployed. Use this as the test when an ADR claims a
deploy — pre-split ADRs claimed deploy dates predating the only real one (fixed by the doc split).
`wfv27-deployable-e857c76` is the last no-reset point. Live MMO baseline 2026-08-22: 7 rooms,
GCL 12, CPU 18.5/140, bucket 10000 flat.

---

## 5. ADR state index

56 ADRs. States: **Live** (in `ab692bd`) · **Host-only** (offline tooling, never in the wasm
bundle) · **On master** (merged, undeployed) · **Partial** · **Design-only** · **Closed**.

**Closed — no open work. Detail in the ADR; do not re-track.** `0001`, `0009c`

**Live** — `0002 0004 0005 0008 0017 0019 0024 0025 0027 0029 0031 0031b 0032 0034 0035 0036 0038 0040 0042 0044 0044a`
**Host-only** — `0006 0023 0023a 0025a 0026 0026a 0033`
**On master, undeployed** — `0046`
**Partial** — `0003 0007 0008a 0009 0009a 0009b 0011 0012 0018 0020 0021 0028 0031a 0037 0039 0043`
**Withdrawn** — `0030` (2026-08-23; tempo axis preserved in 0031 — no open work)
**Superseded** — `0022` (by 0027; its P-AUCTION residue is owned by 0020/0031, and its P-OBJ asks were superseded by 0027's observed-success model — no open work of its own)
**Design-only, zero code** — `0010 0013 0014 0015 0016 0041 0045`

Open work for these is in §6 and §7. An ADR absent from both is Closed.

---

## 6. Open work by owning ADR

One line per item.

**Combat**
- `0008` — S1/S2 synchronized spawning unbuilt; W2 supervisor trim + W4 `WarDecl` posture outstanding; O5 power-bank + heavy multi-squad assault deferred.
- `0008a` — Tier 0 T-HEAL-3 unbuilt (`project_enemy` hard-codes `hits: 0`); T-DEF-1 cover term, T-DEF-5 predictive safe-mode, T-POS-5 exit-tile cost all unbuilt; Tier 3 untouched.
- `0019` — S4-TUNE weight sweep flat on melee beds; boosted-TOUGH conversion blocked on 0041.
- `0020` — S5/S6/S7 (blob auction + R7 currency, adaptivity, adversarial room-gen): **operator-ratified scheduled end-state, sequenced AFTER Phase 4** (they want the R19-retuned kernels). **S5-CAP: `MAX_CONCURRENT_SQUADS` still hardcoded 4** (`squad_manager.rs:211`).
- `0026` — L6c doctrine weights untuned; L8 coordination keyed on `TargetSource` not observed bodies.
- `0026a` — six deferred modes unbuilt; the nine activator signals still uncomputed.
- `0027` — `Farm{Core}` and `Farm{PowerBank}` inert, no producer; salvage teardown still mission-owned.
- `0028` — wire `slots_to_spawn` (K3) and `claims_allowed` (K4) into the bot; record a `run_defended_lifecycle` closeout; multi-squad lane contention in `run_forming` (owns 0029's duplicate).

- `0031` — Tier-2 weapon archetype still doctrine-chosen, not searched; Tier-3 param axes absent.
- `0031a` / `0031b` — sweeps invalid until re-run: **`w_energy` default is now `1.0`, not the `0.001` the results assume.** Re-run, then amend the conclusion.
- `0034` — no convergence gates in `param_sweep.rs`; renewable-rally bias sim-only, never live-wired.
- `0035` — FU1 scout-first request/await pipeline; FU2 give-up for a committed squad that never engages.
- `0036` — live raze confirmation blocked on private-server world mechanics (B-1).
- `0037` — ~~T1/T2 orphan decision~~ **RULED 2026-08-23: retained by design** (war.rs:550 documents it; owned-path `tower_danger: 0.0` is the neighbour-only-signal design). Remaining: T3 seam adds no candidate and only logs under `war_debug`.
- `0039` — P2–P4 **folded into the harness lane** (2026-08-23): re-activate with H5 (shared Docker dependency + sim-fidelity goal).

**Economy**
- `0007` — item 4: size haulers from ADR 0038 route distance, not Manhattan; shared predicted storage capacity.
- `0010` — L0 populate `available_boosts`, per-tick `BoostQueue::clear`, chain math; L1–L4 planner/labs/factory. **Nothing in the bot calls `boostCreep`.** Blocks 0041.
- `0012` — M2 `MarketSnapshot`/`TradePlanner` absent; M3 governor/graylist/kill-switch absent; `market.buy_minerals` off.
- `0040` — §D8 #2: the 20% military reserve (`economy.rs:87`) was never retired post-soak. Owns review R15.
- `0042` — `opportunity_floor` still hardcoded `0` (`squad_manager.rs:1868`, gated on 0043 A2); R1–R4 refinements.
- `0043` — A2/A4/A7/A9/A10 band lerps still live in `spawn_policy.rs`; A11 importance margin; A12 exponential backoff; C1–C7 vetoes.
- `0044` / `0044a` — P3 all-sinks only partially activated (build/repair bids are admission gates, not EV-priced haul registrations); per-lane road awareness; Phase-3 verification never recorded.

**Rooms, expansion, infrastructure**
- `0003` — `MissionResult::Wait/Idle` + park-don't-teardown for economy missions.
- `0009` / `0009a` / `0009b` — planner: D3 RoomGraph + inter-room road layer unbuilt; Q8 cap-lift unwired; **0009b §7 ground-truth bench evaluator gates the whole scoring/RCL revamp**.
- `0011` — D5 cross-room spawn assist and G3 incubation: zero code; no empire spawn-budget orchestrator.
- `0017` — M5b securing escort never built; abort thresholds untuned against live attackers.
- `0018` — K4 SK mineral mining unbuilt; **no live evidence an SK farm has ever actually run**, yet `farming` is default-ON.
- `0021` — follow-ups #1/#2 absorbed by 0046 (undeployed); #5/#6 unimplemented. Re-head once 0046 ships.
- `0038` — post-`ab692bd` claim.rs work (`09c36db`, `e857c76`, `527e9e8`) undeployed → WS-1.
- `0046` — see WS-1. Staleness bucket quantization still to tune in soak.

**Platform / tooling**
- `0004` — governor thresholds still flagged INITIAL, pending pressure-scenario calibration.
- `0005` — an aborted tick loses its serialize and rolls back one tick (divergence from the ADR's goal; loudly accounted, not fixed).
- `0006` — server-harness combat scenarios absent (`Fault` enum is only CpuBurn/GlobalReset/PanicOnce); **H5 sim-vs-server parity oracle** (golden vectors + nightly gate — reassigned here from 0008/0028, see UNOWNED-2; blocked on B-1).
- `0013` / `0014` / `0015` / `0016` / `0045` — design-only. 0015 (testkit + seam registry) and 0016 (HUD) were marked "in scope" by the ultracode completion kickoff, a program that has driven nothing since 2026-07-02 (see RULING-5).
- `0023` / `0023a` — S5 border scenarios deferred; cross-room `Flee` still single-room; no MultiRoom generator.
- `0025` — `action_oscillation_rate` metric never implemented. (0025a residual: **closed as documented-mitigated**, 2026-08-23.)
- `0033` — kite-weight retune never done; determinism fence not promoted corpus-wide.
- `0041` — entire P0–P3. Blocked on 0010.

---

## 7. Cross-cutting work with no ADR owner

- **UNOWNED-1 · Ship WFV 28.** No ADR owns "soak and deploy". Owned here as **WS-1**.
- **UNOWNED-2 · H5 sim-vs-server parity oracle — assigned to ADR 0006** (2026-08-22; 0008 had
  mis-routed it to 0028). No `parity.rs`, no golden vectors, no nightly gate. Blocked on B-1.
  Listed under 0006 in §6.
- ~~**UNOWNED-3**~~ **CLOSED 2026-08-23** (ws-triage): the crate-wide allow is GONE; 115 warnings triaged to zero. The compiler now IS the dead-code register. New annotations carry an owner tag (KEEP/TEST-PINNED/FOLLOW-UP).
- **UNOWNED-4 · `remote_mine.search_radius` still defaults to `1`** (`features.rs:209`) — the
  expansion Wave-1 fix shipped the knob at the value that was the bug. "Wave 1 done" reads as if the
  remote ring widened; it did not.
- ~~**UNOWNED-5**~~ **CLOSED 2026-08-23**: the `SourceKeeperFeatures` container doc contradicted
  its own field default for two months; it now records the operator's real 2026-06-18 default-ON
  decision.
- ~~**UNOWNED-6**~~ **CLOSED 2026-08-23**: `construction.allow_replan` **deleted** (declared but
  read by no code — an operator flipping it silently got nothing). Re-add a replan flag together
  with its consumer when discretionary replan lands (0009).
- ~~**UNOWNED-7 · Stale `Memory._features` overrides**~~ **CLOSED 2026-08-22** by the
  `reset.features` one-shot (`77dc9cc`): setting `Memory._features.reset.features = true` rebuilds
  the persisted tree from compiled defaults next tick (self-clearing, like the other reset flags).
  Fired and live-verified on MMO the same day — `military.offense` reconciled `false→true`, new
  keys appeared, flag self-cleared. **Deliberate retunes now go through this pattern**, never a
  hand-edit that then shadows future defaults.
- ~~**CHORE-1 · 29 ADR headers are stale.**~~ **CLOSED 2026-08-22 by the design/implementation
  split.** All 56 ADRs were rewritten as pure end-state designs; status moved here and to
  [`../implementation/`](../implementation/). The drift class is now structurally impossible — an
  ADR header can no longer make a claim about code. Rollback tag: `pre-doc-split`.

---

## 8. Dead / unwired code register

Found 2026-08-22 by removing `#![allow(dead_code)]` and reading the compiler. Each is a decision —
wire it or delete it — not necessarily work.

| Item | Location | Note |
|---|---|---|
| `gameview.rs` | 104 lines, zero refs | The ADR 0006 seam Inc-6 record/replay and 0015's fakes both assume. Never migrated a single consumer. |
| `ui.rs` | 36 lines, `UISystem` never constructed | Doc comment claims consumers that do not exist. |
| `BoostQueue` | `military/boostqueue.rs` | Plumbed into every mission, **no mutator ever called**; `clear()` never called, so it would grow unbounded if fed. |
| Readiness tranche | `military/damage.rs:33,39,64,78,99,115` | `defender_spawn_readiness`, `net_tower_damage`, `should_towers_fire`, `estimated_ticks_to_kill` — built, tested, uncalled. One coherent unfinished tranche. |
| `issue_virtual_anchor_flee` | `military/formation.rs:398` | The **only** squad-level flee construct; nothing replaced it ⇒ squads have no coordinated retreat. Adjacent to review D10. |
| `Job::describe` layer | `jobs/jobsystem.rs:99,105` + ~15 jobs | Every job implements it; nothing dispatches it. A whole overlay with no renderer. |
| T1/T2 neighbour kernels | `war_decision.rs:182,327` | **Decided: retained by design** — sim/test-covered, awaiting the offense-side candidate feed (war.rs:550). Not dead code. |
| `HoldModel::Suppress` | `room_economics.rs:88,191` | Unreachable — SK farming runs a duplicate ROI kernel at `sourcekeeper.rs:99`. |
| `StructureIdentifier` | `structureidentifier.rs:7,32` | Superseded half of a live module. |

---

## 9. Rulings — decided 2026-08-22, do not relitigate

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

- **2026-08-23** — Triage decisions ratified (operator): 0030 Withdrawn (tempo→0031), 0025a residual documented-mitigated, 0039 P2–P4→harness lane, 0020 S5–S7 kept-scheduled (after Phase 4); **0047 pulled forward as Phase 2.5 (WS-6)**. `search_radius` 1→2 shipped + live-reconciled (wasm `bd6eebcc0f56`, hot swap, pattern proven twice). UNOWNED-4 closed.

- **2026-08-23** — **Wave B SHIPPED to live MMO** (hot swap `0d9524f2668f` per RULING-8; vm_starts 2749, missions persisted, 0 deser). RULING-8 recorded (deploy-to-live batched; B-1 demoted); ADR 0047 drafted (reset-tolerant serialization).

- **2026-08-23** — **Wave B CODE-COMPLETE**: D9/D10 landed (`1a85a57` + rover `850a06b` — shared engaged ladder now ONE implementation in rover, wired live; flee uses partial paths); T1/T2 ruled retained-by-design. 15 pins total. The 2026-07-09 review Tier −1 work list is closed; soak pending B-1. World checks 1–3 healthy.

- **2026-08-22 (late)** — Wave B 6/8: D2/D3 safe-mode (`8fa0c60`), D4/D5/D6 roster churn (`be5ce24`), D28 vacuous clear (`b26eba4` + decision/eval submodules). 13 RED-verified pins; fence green; ADR 0027 amended. D9/D10 + 0037 decision remain. WS-1 observation healthy through 3 checks (claim pipeline live, C2 signature absent).

- **2026-08-22** — **WFV 28 DEPLOYED TO LIVE MMO** (`77dc9cc`, wasm `d9b748497e4a`; operator inverted soak order, MMO-first). Loud reset clean: 0 panics, CPU 52→37/140, bucket 10000. `reset.features` one-shot built + fired + verified — live config at compiled-default parity (offense back ON; Wave A fixes in-artifact). Closes UNOWNED-7. Observation window open (C1–C5).
- **2026-08-22** — Completion roadmap (Phases 0–6) recorded in §3; §1 updated to MMO-first.

- **2026-08-22** — **Design/implementation split.** All 56 ADRs rewritten as pure end-state designs; status moved here and to `../implementation/`. Status vocabulary reduced to Decided/Draft/Superseded/Withdrawn (+ note types). Closes CHORE-1 structurally. Adversarial verify caught 4 design-loss regressions and 19 lesser ones, all remediated and re-verified. Rollback tag: `pre-doc-split`.
- **2026-08-22** — Full ADR-corpus reconciliation (56 verified, 29 drifted); this tracker created; rulings 1–7 recorded.
- **2026-08-22** — Repo tie-off: ADR 0046 merged (WFV 28), working tree emptied, all branches/worktrees removed, master + 49 submodule commits pushed, ADR 0044a renumbered, 0038/0042 headers fixed.
- **2026-07-28** — Combat Wave A shipped to MMO (`ab692bd`): D1/D11/D24/D25/D26/D27/R22. CPU 87→16.
- **2026-07-06** — ADR 0040 accepted; WFV 27 to MMO.
