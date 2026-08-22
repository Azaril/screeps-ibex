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

---

## 1. NOW — the single active workstream

### WS-1 · Get WFV 28 verified and live → [`../implementation/ws-1-ship-wfv28.md`](../implementation/ws-1-ship-wfv28.md)

**State: BLOCKED on tooling (see §2). Nothing else should start.**

`master` (`2506b58`, WFV 28) carries ADR 0046 plus the expansion fixes and has **never run against
any world**. This is the only genuinely half-finished thing in the project — everything else is
either shipped or not started.

| Step | State |
|---|---|
| Private soak per [`adr-0046-private-soak-plan.md`](adr-0046-private-soak-plan.md) | **blocked** — Docker |
| Judge against ADR 0046 §5 criteria (C1–C5 in the plan) | pending soak |
| MMO deploy + observe | **deliberately held** — live is healthy at 7 rooms; nothing forces the reset |
| L2 poison-list self-heal — ships **last** of the expansion program | pending live evidence |

**Deploy-risk note (from the 2026-08-22 code sweep):** WFV 28 has never executed, yet these ship
default-ON: `military.offense`, `source_keeper.farming`, `derelict.declaim`,
`derelict.breach_sealed`, `claim.on` (rapid-spread, `max_concurrent_missions: 4`), `visualize.on`.
Review the set before the MMO step, not before the private step.

---

## 2. BLOCKED

- **B-1 · `com.docker.service` is Stopped/Manual; starting it needs Administrator.** Symptom is
  `docker ps` **hanging**, not erroring, while Docker Desktop, its pipes and the WSL distro all look
  healthy. Fix (elevated): `Start-Service com.docker.service` then
  `Set-Service com.docker.service -StartupType Automatic`. Detail: soak plan §0.
  **This one blocker gates far more than WS-1:** ADR 0046's soak, H5 parity oracle, P2.M2-LIVE,
  M4 exit criteria 1–11, ADR 0036 live-raze confirmation, ADR 0028's `run_defended_lifecycle`
  closeout, and the dismantle-seam soak. It is the highest-leverage unblock in the project.

---

## 3. NEXT — ordered queue, not started

Each is one workstream. Do not begin one while §1 is occupied.

1. **WS-2 · Combat review Tier −1 Wave B** → [`../implementation/ws-2-combat-wave-b.md`](../implementation/ws-2-combat-wave-b.md). D2, D3 (safe-mode: hair trigger at
   `CRITICAL_STRUCTURE_MIN_HITS = 5000`, and a fires-once-ever latch), D4, D5, D6 (roster churn),
   D9, D10 (live-adapter gaps), D28. No WFV bump — soaks as one wave, no reset. Closes the
   2026-07-09 review as a live document. **R19 does not gate this** (see RULING-6).
2. **WS-3 · ADR 0010 boost producer → ADR 0041 boost layer.** 0041 is the highest-value item on the
   board (gates review risk R1, enemy-boost blindness, and the whole boosted-assault frontier) but
   it is **blocked on 0010**: nothing in the bot ever calls `boostCreep`, and `BoostQueue` is a dead
   pipe. 0010 L0 first, then 0041.
3. **WS-4 · R19 chokepoint re-tune.** Its rover-eval sweep is already committed (`c4b3d17`).
   Resolve RULING-6 as part of it.
4. **WS-5 · ADR 0045 power creeps.** Pure greenfield, zero interaction with the above — genuinely
   parallelizable, which is exactly why it must wait until §1 and WS-2 are closed.

---

## 4. Deployment ledger

| Where | Artifact | WFV | Date |
|---|---|---|---|
| Live MMO (shardX) | `ab692bd` + decision `f6c084a` | 27 | 2026-07-28 |
| Docker private | same | 27 | 2026-07-28 |
| `master` | `2506b58` | **28 — never executed anywhere** | 2026-08-22 |

**Anything committed after `ab692bd` is undeployed.** Use this as the test when an ADR claims a
deploy — several claim deploy dates that predate the only real one (see CHORE-1).
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
**Partial** — `0003 0007 0008a 0009 0009a 0009b 0011 0012 0018 0020 0021 0022 0028 0030 0031a 0037 0039 0043`
**Design-only, zero code** — `0010 0013 0014 0015 0016 0041 0045`

Open work for these is in §6 and §7. An ADR absent from both is Closed.

---

## 6. Open work by owning ADR

One line per item. Header-drift is **not** listed here — that is CHORE-1.

**Combat**
- `0008` — S1/S2 synchronized spawning unbuilt; W2 supervisor trim + W4 `WarDecl` posture outstanding; O5 power-bank + heavy multi-squad assault deferred.
- `0008a` — Tier 0 T-HEAL-3 unbuilt (`project_enemy` hard-codes `hits: 0`); T-DEF-1 cover term, T-DEF-5 predictive safe-mode, T-POS-5 exit-tile cost all unbuilt; Tier 3 untouched.
- `0019` — S4-TUNE weight sweep flat on melee beds; boosted-TOUGH conversion blocked on 0041.
- `0020` — S5 blob role auction + R7 currency, S6 adaptivity, S7 adversarial room-gen: zero code. **S5-CAP: `MAX_CONCURRENT_SQUADS` still hardcoded 4** (`squad_manager.rs:211`).
- `0022` — P-AUCTION and P-OBJ largely unbuilt; sequencing overtaken by 0026/0027/0031/0032/0034–0037. See RULING-4.
- `0026` — L6c doctrine weights untuned; L8 coordination keyed on `TargetSource` not observed bodies.
- `0026a` — six deferred modes unbuilt; the nine activator signals still uncomputed.
- `0027` — `Farm{Core}` and `Farm{PowerBank}` inert, no producer; salvage teardown still mission-owned.
- `0028` — wire `slots_to_spawn` (K3) and `claims_allowed` (K4) into the bot; record a `run_defended_lifecycle` closeout; multi-squad lane contention in `run_forming` (owns 0029's duplicate).
- `0030` — `EngagementTempo` has zero code; §9 steps 2–5 unbuilt.
- `0031` — Tier-2 weapon archetype still doctrine-chosen, not searched; Tier-3 param axes absent.
- `0031a` / `0031b` — sweeps invalid until re-run: **`w_energy` default is now `1.0`, not the `0.001` the results assume.** Re-run, then amend the conclusion.
- `0034` — no convergence gates in `param_sweep.rs`; renewable-rally bias sim-only, never live-wired.
- `0035` — FU1 scout-first request/await pipeline; FU2 give-up for a committed squad that never engages.
- `0036` — live raze confirmation blocked on private-server world mechanics (B-1).
- `0037` — **decide the fate of T1/T2 kernels orphaned by D27** (`war_decision.rs:182,327` have no non-test callers; `war.rs:531` passes `tower_danger: 0.0`). See RULING-3.
- `0039` — P2 cohesion-kernel extraction, P3 unified self-play loop, P4 render corpus.

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
- `0006` — server-harness combat scenarios absent (`Fault` enum is only CpuBurn/GlobalReset/PanicOnce).
- `0013` / `0014` / `0015` / `0016` / `0045` — design-only. 0015 (testkit + seam registry) and 0016 (HUD) were marked "in scope" by the ultracode completion kickoff, a program that has driven nothing since 2026-07-02 (see RULING-5).
- `0023` / `0023a` — S5 border scenarios deferred; cross-room `Flee` still single-room; no MultiRoom generator.
- `0025` / `0025a` — `action_oscillation_rate` metric never implemented; residual 15–20% objects reading as wall unexplained.
- `0033` — kite-weight retune never done; determinism fence not promoted corpus-wide.
- `0041` — entire P0–P3. Blocked on 0010.

---

## 7. Cross-cutting work with no ADR owner

- **UNOWNED-1 · Ship WFV 28.** No ADR owns "soak and deploy". Owned here as **WS-1**.
- **UNOWNED-2 · H5 sim-vs-server parity oracle.** ADR 0008 says 0028 tracks it; 0028 does not. No
  `parity.rs`, no golden vectors, no nightly gate. Blocked on B-1. **Assign to ADR 0006.**
- **UNOWNED-3 · `#![allow(dead_code)]` at `lib.rs:2`** silences the compiler for the whole bot and
  is why §8 accumulated invisibly. Remove it and fix or annotate the fallout.
- **UNOWNED-4 · `remote_mine.search_radius` still defaults to `1`** (`features.rs:209`) — the
  expansion Wave-1 fix shipped the knob at the value that was the bug. "Wave 1 done" reads as if the
  remote ring widened; it did not.
- **UNOWNED-5 · `features.rs` self-contradiction:** the `SourceKeeperFeatures` doc comment says
  "Default OFF until… a private-server soak validate it" (`:637`) while `farming: true` sits at
  `:657`.
- **UNOWNED-6 · `construction.allow_replan`** (`features.rs:97`) is read by no code — an operator
  flipping it gets nothing.
- **UNOWNED-7 · Stale `Memory._features` overrides** can both shadow retunes and silently revert
  them — a live-operations hazard for every default-ON flag. Needs a versioned posture or a
  refresh procedure.
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
| T1/T2 neighbour kernels | `war_decision.rs:182,327` | Orphaned by D27's seam removal. See RULING-3. |
| `HoldModel::Suppress` | `room_economics.rs:88,191` | Unreachable — SK farming runs a duplicate ROI kernel at `sourcekeeper.rs:99`. |
| `StructureIdentifier` | `structureidentifier.rs:7,32` | Superseded half of a live module. |

---

## 9. Rulings — decided 2026-08-22, do not relitigate

Recorded because the corpus contradicted itself and a future reader would otherwise reopen these.

- **RULING-1 · Minted `SquadId`/`SquadStore` (I1/I2) will NOT be built.** `EntityOption<Entity>` +
  `repair_entity_integrity` is the end state (ADR 0001, REC-009b). Three sources disagreed
  (0008 listed it open, 0020 said "dropped per 0022 D1", plan §3 and phase-2 CP-I list it blocking).
  ⇒ **CP-I is retired, not pending.** Amend 0008 and plan §3 as part of CHORE-1.
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

- **2026-08-22** — **Design/implementation split.** All 56 ADRs rewritten as pure end-state designs; status moved here and to `../implementation/`. Status vocabulary reduced to Decided/Draft/Superseded/Withdrawn (+ note types). Closes CHORE-1 structurally. Adversarial verify caught 4 design-loss regressions and 19 lesser ones, all remediated and re-verified. Rollback tag: `pre-doc-split`.
- **2026-08-22** — Full ADR-corpus reconciliation (56 verified, 29 drifted); this tracker created; rulings 1–7 recorded.
- **2026-08-22** — Repo tie-off: ADR 0046 merged (WFV 28), working tree emptied, all branches/worktrees removed, master + 49 submodule commits pushed, ADR 0044a renumbered, 0038/0042 headers fixed.
- **2026-07-28** — Combat Wave A shipped to MMO (`ab692bd`): D1/D11/D24/D25/D26/D27/R22. CPU 87→16.
- **2026-07-06** — ADR 0040 accepted; WFV 27 to MMO.
