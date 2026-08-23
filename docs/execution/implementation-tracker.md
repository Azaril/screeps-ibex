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

### ~~WS-1 · Ship WFV 28~~ **CLOSED 2026-08-23 — C1–C5 ALL PASS; the empire claimed its 8th room.**

The full arc: deployed to MMO 2026-08-22 (loud reset, clean), features reconciled to compiled
defaults, observed through 6+ checks (0 panics, 0 drain signatures throughout), and the claim
pipeline **committed W7N47** — a dist-4, above-ring, score-0.835 candidate, the exact far-sprawl
target class the 2026-08-11 diagnosis → ADR 0046 program was built to reach. A
`RemoteBuildMission` is constructing it now (8 rooms). The unreachable list holds only bounded
retries (attempts 1–2 + `retry_after` + fresh-sighting clears) — **L2 (poison-list self-heal) is
OBVIATED BY CONSTRUCTION**: the 0046 machinery *is* the self-heal; L2 targeted the old permanent
103-room list, which no longer exists as a class. WS-1 doc deleted per the impl-doc lifecycle.

### NOW: WvC-1 · Military completion wave → [`../implementation/ws-wvc1-military-completion.md`](../implementation/ws-wvc1-military-completion.md)

**Operator reorder (2026-08-23): military peak effectiveness first — finish partial machinery
before the boost build.** The boost pipeline (0010+0041, both zero-code) is the largest NEW
feature on the board; the military column holds the densest partial work incl. built-but-unwired
machinery, and the 2026-07-09 review remainder is the standing bug farm. Boost moves behind the
military waves + P4 re-tune (it completes peak-vs-BOOSTED-opponents last).

**2026-08-23: ALL 7 WvC-1 items CODE-COMPLETE** (T-HEAL-3a, readiness wiring, S5-CAP, FU2, L8,
D6c rally bias, K3/K4 shared-kernel resolution — details + SHAs in the ws doc). Remaining:
batch-ship (hot swap, no WFV) + live-watch, then the doc deletes and WvC-2 starts.

WS-6 shipped 2026-08-23 (msgpack WFV 29 live — additive shape changes now reset-free). WATCH:
segment chars as plans rebuild; wasm 48.5% of code limit; Wave B behaviors on the next real fight.

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

**Phase 2 — the triage pass — DONE 2026-08-23** (verdicts in git: `f3c822b`, `29072c3`, the sweep). Original brief: **decide, don't build** (one session; create its impl doc — the
verdicts must land as ADR amendments via `Design deltas`). Every §6/§8 item gets one of three
verdicts: **schedule** (into Phases 3–6), **amend out** (shrink the ADR's end state — candidates:
0030 `EngagementTempo` → Withdraw/fold into 0031; 0020 S5/S6/S7 keep-or-cut; 0026a's six deferred
modes; 0039 P2–P4; 0025a residual → close as documented-mitigated), or **do now** (the §8
wire-or-delete one-liners, then **UNOWNED-3: remove `#![allow(dead_code)]`** so the compiler
enforces the register). *Exit criterion: every remaining §6 line is inside a scheduled phase.
Plausibly closes 6–10 ADRs by amendment.*

**Phase 2.5 — WS-6 · ADR 0047 — SHIPPED 2026-08-23** (msgpack world stream + Plan shrink, WFV 29 live; the LAST format-transition reset). WATCH: segment chars once plans rebuild (projected 12–14% of budget); wasm binary 48.5% of code limit.

**Phase 3 — the military completion waves (WvC-1/WvC-2, operator-prioritized 2026-08-23).** WvC-1 (correctness + wiring): T-HEAL-3a winnability inputs, the damage.rs readiness-tranche wiring, S5-CAP governor-dynamic squad cap, 0035 FU2 never-engages give-up, 0026 L8 observed-bodies coordination, 0034 rally-bias live-wire, 0028 K3/K4 wiring. WvC-2 (defensive features): T-DEF-1 rampart-anchored defenders, T-DEF-5 predictive safe-mode arm, T-POS-5 exit-tile cost, 0037 T3 candidate emission. Batched + shipped like Wave B (no WFV — and under 0047, even shape changes are cheap now).

**Phase 4 — WS-4 · R19 chokepoint re-tune** (unchanged, directly military: the combat kernels' parameters proven on realistic terrain; closes 0019 S4-TUNE, 0024 FU#4, 0026 L6c, 0031a/b re-sweeps, 0032 value_e, 0033 kite retune).

**Phase 5 — Boost pipeline (ADR 0010 L0 → 0041)** — the military CAPSTONE, after the machinery it feeds is correct: 0041
gates review risk R1 (enemy-boost blindness, the top MMO risk) and the whole boosted-assault
frontier, but is **blocked on 0010** — nothing calls `boostCreep`; `BoostQueue` is a dead pipe.
*Closes: 0010, 0041; unblocks 0019 boosted-TOUGH, 0020-TOUGH, 0008a Tier 3, 0008 S2.*

**Phase 6 — economy completion.** The 0043 band→EV conversions (A2/A4/A7/A9/A10, A11, A12,
C1–C7), 0042 `opportunity_floor` + R1–R4, 0044/0044a P3 all-sinks activation, 0007 item 4,
0040 §D8 reserve retirement. Mechanical batch work against a shipped market.
*Closes: 0007, 0040, 0042, 0043, 0044, 0044a.*

**Phase 7 — remaining designs.** WS-5 (0045 power creeps), 0020 S5–S7 (after Phase 4, ratified), plus
whichever of 0011/0012/0013/0014/0015/0016 the triage keeps (0013's spending half is already
delegated to 0045; 0014 may reduce to the W4 `WarDecl` hook owned by 0008). New builds, so last
by policy.

*Convergence: Phases 0–2 ≈ a week of sessions, taking the corpus from 2 Closed to ~15–20 Closed;
Phases 3–5 are the military program (waves → re-tune → boost capstone); Phase 6 economy; Phase 7 is a choice, not a debt.*

---

## 4. Deployment ledger

| Where | Artifact | WFV | Date |
|---|---|---|---|
| Live MMO (shardX) | wasm `bd6eebcc0f56` (Wave B + retune hot swaps) | **28** | 2026-08-23 |
| Docker private | `ab692bd` (stale — refresh when B-1 clears) | 27 | 2026-07-28 |
| `master` | HEAD (WFV-anchored; do not pin a SHA here — it drifts every commit) | 28 — **live on MMO** | since 2026-08-22 |

**The deployed-artifact test point is now `77dc9cc`** (2026-08-22); anything after it is undeployed. Use this as the test when an ADR claims a
deploy — pre-split ADRs claimed deploy dates predating the only real one (fixed by the doc split).
`wfv27-deployable-e857c76` is the historical WFV-27 point. Live MMO baseline 2026-08-23: **8 rooms**,
GCL 12, bucket 10000, W7N47 under remote-build.

---

## 5. ADR state index

56 ADRs. States: **Live** (in `ab692bd`) · **Host-only** (offline tooling, never in the wasm
bundle) · **On master** (merged, undeployed) · **Partial** · **Design-only** · **Closed**.

**Closed — no open work. Detail in the ADR; do not re-track.** `0001`, `0005` (containment ratified as-shipped 2026-08-23), `0009c`, `0038`

**Live** — `0002 0004 0008 0047 0017 0019 0024 0025 0027 0029 0031 0031b 0032 0034 0035 0036 0040 0042 0044 0044a 0046`
**Host-only** — `0006 0023 0023a 0025a 0026 0026a 0033`

**Partial** — `0003 0007 0008a 0009 0009a 0009b 0011 0012 0018 0020 0021 0028 0031a 0037 0039 0043`
**Withdrawn** — `0030` (2026-08-23; tempo axis preserved in 0031 — no open work)
**Superseded** — `0022` (by 0027; its P-AUCTION residue is owned by 0020/0031, and its P-OBJ asks were superseded by 0027's observed-success model — no open work of its own)
**Design-only, zero code** — `0010 0013 0014 0015 0016 0041 0045`

Open work for these is in §6 and §7. An ADR absent from both is Closed.

---

## 6. Open work by owning ADR

One line per item. **Phase tags** (sweep 2026-08-23): P3 boost · P4 R19-tuning · P5 economy · P6 remaining designs · **WvC** = the post-P4 combat wave (small live combat fixes batched like Wave B) · HARNESS = B-1 lane · WATCH = live observation.

**Combat**
- `0008` — S2 boost handoff **→P3**; S1 pre-spawn, W2 trim + W4 `WarDecl` **→P6**; O5 power-bank + heavy assault = deferred capabilities (activate by decision, not schedule).
- `0008a` — T-HEAL-3 (widens into R1) **→P3** (T-HEAL-3a SHIPPED, WvC-1); T-DEF-1/T-DEF-5/T-POS-5 **→WvC-2** (readiness tranche wired/pruned, WvC-1); Tier 3 **→P6** (after P3).
- `0019` — S4-TUNE **→P4**; boosted-TOUGH **→P3** (blocked on 0041).
- `0020` — S5/S6/S7 (blob auction + R7 currency, adaptivity, adversarial room-gen): **operator-ratified scheduled end-state, sequenced AFTER Phase 4** (they want the R19-retuned kernels). S5-CAP SHIPPED (WvC-1): empire-scaled cap + defense surge, shared `claim_pacing` kernel.
- `0026` — L6c **→P4** (L8 SHIPPED, WvC-1: observed-owner classifier).
- `0026a` — modes activate as their signals land **→P6** (catalog; no standalone schedule).
- `0027` — Farm producers (PowerBank needs its own ADR) + salvage-teardown migration **→P6**.
- `0028` — `run_defended_lifecycle` closeout **→HARNESS** (K3/K4 RESOLVED, WvC-1: claim_admission is the shared kernel, `claims_allowed` harness-only; K3 adapters separate by design); multi-squad lane contention **→WvC-2**.

- `0031` — Tier-2 archetype search + Tier-3 axes **→P4** (the 0031a sweep plan).
- `0031a` / `0031b` — sweeps invalid (`w_energy` now 1.0, not the 0.001 the results assume); re-run + amend conclusions **→P4**.
- `0034` — convergence gates **→P4** (D6c renewable-rally bias SHIPPED, WvC-1).
- `0035` — FU1 **→P6** (poll-until-fresh sufficiency undecided; FU2 CLOSED, WvC-1: terminator composition + stall-aware give-up clock).
- `0036` — live raze confirmation **→HARNESS** (private-server world mechanics, B-1).
- `0037` — ~~T1/T2 orphan decision~~ **RULED 2026-08-23: retained by design** (war.rs:550 documents it; owned-path `tower_danger: 0.0` is the neighbour-only-signal design). Remaining: T3 seam candidate emission **→WvC**.
- `0039` — P2–P4 **folded into the harness lane** (2026-08-23): re-activate with H5 **→HARNESS**.

**Economy**
- `0007` — item 4 (route-distance hauler sizing + shared predicted capacity) **→P5**.
- `0010` — L0 populate `available_boosts`, per-tick `BoostQueue::clear`, chain math; L1–L4 planner/labs/factory. **Nothing in the bot calls `boostCreep`.** Blocks 0041 **→P3**.
- `0012` — M2/M3 **→P6**.
- `0040` — §D8 #2: the 20% military reserve (`economy.rs:87`) was never retired post-soak. Owns review R15 **→P5**.
- `0042` — `opportunity_floor` still hardcoded `0` (`squad_manager.rs:1868`, gated on 0043 A2); R1–R4 refinements **→P5**.
- `0043` — A2/A4/A7/A9/A10 band lerps still live in `spawn_policy.rs`; A11 importance margin; A12 exponential backoff; C1–C7 vetoes **→P5**.
- `0044` / `0044a` — P3 all-sinks only partially activated (build/repair bids are admission gates, not EV-priced haul registrations); per-lane road awareness; Phase-3 verification never recorded **→P5**.

**Rooms, expansion, infrastructure**
- `0003` — `MissionResult::Wait/Idle` park-don't-teardown **→P6**.
- `0009` / `0009a` / `0009b` — planner revamp (bench evaluator gates it) **→P6**.
- `0011` — D5 assist, G3 incubation, empire spawn-budget orchestrator **→P6**.
- `0017` — M5b escort (owned by 0008 **→P6**); abort-threshold tune **→WATCH** (needs live attacker evidence).
- `0018` — K4 mineral + K-RECONCILE (incl. `HoldModel::Suppress` unification) **→P6**; SK-farm live evidence **→WATCH**.
- `0021` — follow-ups #5/#6 **→P6** (#1/#2 absorbed by 0046, live).
- `0046` — staleness-bucket quantization tune rides live observation (low priority; C1–C5 all passed) **→WATCH**.

**Platform / tooling**
- `0004` — governor pressure-scenario calibration **→HARNESS**.
- `0006` — server-harness combat scenarios absent (`Fault` enum is only CpuBurn/GlobalReset/PanicOnce); **H5 sim-vs-server parity oracle** (golden vectors + nightly gate — reassigned here from 0008/0028, see UNOWNED-2; blocked on B-1) **→HARNESS**.
- `0013` / `0014` / `0015` / `0016` / `0045` — design-only. 0015 (testkit + seam registry) and 0016 (HUD) were marked "in scope" by the ultracode completion kickoff, a program that has driven nothing since 2026-07-02 (RULING-5) **→P6**.
- `0023` / `0023a` — S5 border scenarios, cross-room `Flee`, MultiRoom generator **→HARNESS**.
- `0025` — `action_oscillation_rate` metric **→P4** (rides the sweep instrumentation). (0025a residual closed 2026-08-23.)
- `0033` — kite retune **→P4**; corpus-wide fence promotion **→P4**.
- `0041` — entire P0–P3 **→P3** (blocked on 0010).

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

- **2026-08-23 (late)** — **WvC-1 code-complete, all 7 items**: T-HEAL-3a winnability inputs (`c5a06c8`), defender spawn-readiness wired + tower half deleted as U-TOWER-superseded (`81ee72f`), S5-CAP empire-scaled cap + defense surge (`7a87df5` → shared kernel `13112e6`), 0035 FU2 closed (veto attempt `4d044be` reverted `4d186d8` after 2 eval-bed regressions — the probe bounce is load-bearing; final = stall-aware give-up clock + engaged-gated stall streaks, agent `0c57c45`), 0026 L8 observed-owner coordination (`0455298`), 0034 D6c renewable-rally bias (`e6aa3ce`), 0028 K3/K4 resolved as-built (ADR rewritten). Ship + live-watch pending.
- **2026-08-23** — **Operator reorder: military first.** Boost pipeline (largest NEW build) demoted behind the military completion waves (WvC-1 correctness+wiring, WvC-2 defensive features) and the P4 re-tune — finish partial machinery + kill the bug farm before feeding it boosts. WvC promoted out of the old Phase-6 into Phase 3; NOW = WvC-1.

- **2026-08-23** — **WS-6 SHIPPED + CLOSED: ADR 0047 live at WFV 29** (msgpack struct-map stream + foreman Plan shrink `5c89f30` — road_network deleted, build_order on-demand; plans ~70% smaller). The LAST format-transition reset paid; additive changes are now reset-free. Live: 2.8% of segment budget mid-rebuild (proj. 12–14% full), named decode FASTER than old bincode. Costs recorded: wasm +71% (48.5% of code limit). Operator constraint recorded: plans are durable state, never recompute-after-reset. ws-6 doc deleted.

- **2026-08-23** — **ADR 0047 → DECIDED**: whole-stream msgpack struct-map, ONE encoding (operator simplicity steer, confirmed by round-2 data: 30.4% of the real 400KB segment budget; RoomPlanData=86% of bytes and shape-stable; real-world named round-trip works; sectioning rejected as unnecessary). WS-6 remaining: the game_loop swap (one WFV bump, batched).

- **2026-08-23** — **Phase 2 (triage) CLOSED**: final sweep phase-tagged every §6 line (P3/P4/P5/P6/WvC/HARNESS/WATCH); 0005 containment ratified as-shipped → Closed; ws-triage doc deleted. WS-6 (0047 benches) is NOW.

- **2026-08-23** — **WS-1 CLOSED: C1–C5 ALL PASS.** The pipeline claimed **W7N47** (dist 4, above-ring, score 0.835) — 8 rooms; RemoteBuildMission constructing. L2 ruled OBVIATED by 0046's bounded-retry machinery. 0046→Live, 0038→Closed. WS-1 doc deleted per lifecycle.

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
