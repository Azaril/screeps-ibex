# Combat Overhaul — Master Status & Plan

- **Owner:** William Archbell
- **Last updated:** 2026-06-18
- **This is the source of truth for combat/war STATUS and REMAINING WORK.** Forward-looking status lives here; landed-task detail (commit SHAs, per-task validation cells) lives in [`../execution/phase-2.md`](../execution/phase-2.md); design rationale lives in ADR [0008](../design/0008-combat-and-squad-architecture.md) (+ tactics annex [0008a](../design/0008a-combat-tactics.md)).

## 0. How to use this doc

| Doc | Role |
|---|---|
| **THIS doc** | Current status by workstream + the single ordered remaining-work plan + gates. Supersedes the phase-2.md Status columns. |
| ADR [0008](../design/0008-combat-and-squad-architecture.md) (+ [0008a](../design/0008a-combat-tactics.md) annex) | Design end-state / why. 0008a = the ~55-tactic catalog + ~70-knob param table + EXP-* register. |
| [`phase-2.md`](../execution/phase-2.md) | Cold-resume per-task historical log: §2.0 newest-first status log w/ commit SHAs, §2.2–2.7a per-task validation cells, §2.8 sequencing graph, §2.9 checkpoints, §2.1 operator-settled constraints. |
| [`../execution/g4-offense-plan.md`](../execution/g4-offense-plan.md), [`../execution/g3-tail-plan.md`](../execution/g3-tail-plan.md) (ARCHIVED in place) | Archived sub-plan history (O-series + 8-step kite). Redirect here for what remains. |

One-line rule: **forward-looking status lives here; landed-task detail lives in phase-2.md; design rationale lives in ADR 0008.**

## 1. The problem (one paragraph)

Squads were ineffective in three named ways: (1) **null tactics** — "just stand and ranged-mass-attack" (no kiting in the ordered path, best-effort focus-fire, oscillating retreat, flat-HP heal math); (2) **orphan → idle** — combat creeps weren't mission children, so on objective completion the `SquadContext` was deleted but creeps kept a dangling `squad_entity` and idled until TTL; (3) **scatter** — cohesion was N independent solo pathfinds against a virtual anchor, members trickle-spawned rooms/ticks apart. The fix copies the **scout** subsystem (request→claim→complete→release→retire): a global `CombatObjectiveQueue` + a `SquadManager` lifecycle/tactics owner + an anchor mover + synchronized spawning.

> **Status note (2026-06-18):** the tactics / cohesion / lifecycle layers are **LIVE on master**. This paragraph is the original motivation, not the current state. (Supersedes the stale "design+ADR+plan only / just stands RMA" framing in MEMORY.md and the "no production code yet" line that previously headed this doc.)

## 2. Architecture at a glance

```
CombatObjectiveQueue  (global persistent priority/TTL queue; ephemeral claims)
        │  request() upsert-merge by kind; best_unclaimed_near; mark_unwinnable backoff
        ▼
SquadManager  (single perpetual system: reconcile → field rosters → compute orders → claim, ≤4 squads)
        │  Phase A reconcile/re-claim · B spawn-demand · B2 orders · C claim-new
        ▼
pure decide_squad_with_pathing / decide_combat / decide_movement   (screeps-combat-decision, 41 host tests)
        │  focus-fire · coupled engage/retreat hysteresis · orientation · heal assignment · scored kiting
        ▼
SquadCombatJob  (executes orders + Recall)
```

- **Movement is anchor-primary** (footprint "moving-maximum" pathfind, lockstep block advance) — **NOT** lead-follower. Corridors handled by *relaxing the same mover*; `Follow`/`pull` reserved for no-MOVE bodies only (M4, conditional).
- **Crates:** `screeps-combat-engine` (JS-free mechanism) · `screeps-combat-decision` (pure tactics + seam) · `screeps-combat-agent` (sim glue + scripted opponents) · `screeps-combat-eval` (scenarios/metrics/parity/replay).
- Design: ADR 0008 §3–5. Tactics catalog + EXP register: ADR 0008a.

## 3. Current status — by workstream

This table **supersedes** the phase-2.md Status columns (including the stale §2.5-line-177 "G4 unstarted" cell).

| WS | Rollup | Detail |
|---|---|---|
| **H** harness/engine | **PARTIAL** | H1/H2/H3 DONE (combat-engine, tactical seam, seg-57 cohesion metrics + military score term). **H4/H5 UNSTARTED** (scenarios/opponents/self-play/replay; server-parity + nightly gate). |
| **I** identity | **UNSTARTED** | Manager interim-keys squads by `SquadContext` Entity. I1 SquadStore/SquadId + I2 re-key both UNSTARTED. |
| **M** movement | **PARTIAL** | M1 DONE (footprint primitive). M2 code-done + sim-validated + live-swapped (WFV 7→8); orientation gap closed via G4-O2; **behavioral live-combat validation pending (M2-LIVE)**. M3 DONE (sim). M4 conditional. |
| **G** goals/squads | **PARTIAL** | G1/G2/G3/G3-tail DONE. G4: defense-half DONE + offense O1/O2/O3/O4/O6 DONE. **O5 NEXT, O7 LAST**; heavy multi-squad assault DEFERRED; SK-deadcode trivial. |
| **W** war supervisor | **PARTIAL** | W1 DONE (defense → Defend, default ON). **W2/W3/W4 UNSTARTED** (supervisor trim/economy-abort; Escort producer; WarDecl posture). |
| **S** spawning | **UNSTARTED** | S1 sole-demand-producer (GroupId=SquadId) + S2 boost handoff (gated ADR 0010). |
| **K** source-keeper | **PARTIAL** | K0/K1/K2/K3/K5 DONE. K2c-2 (military_free gate + farm retirement) + K-RECONCILE UNSTARTED; **K4 mineral DEFERRED/BLOCKED** (needs remote extractor/container + market-glut predicate). |

## 4. Resume point + do-next ordering

**RESUME POINT (2026-06-18):** after P2.G4-O6. Combat slice on master at Docker **WFV 12** (clean reset, ticking, no thrash). Host tests green: bot 160, rover 27, decision 41; wasm + clippy clean. Current Docker soak is the validation gate for defense `Defend` squads / O1 cohesive travel / K5 stronghold standdown — **world is peaceful post-reset, so combat paths are dormant until a threat appears.**

**Do-next ordering** (re-numbered sequentially; the missing-item-3 gap in the old phase-2.md list is closed): **O5 → O7** (gated on soak + heavy multi-squad assault + power-bank haul), then **H4/H5, I1/I2, W2–W4, S1/S2** as independent tracks. Full dependency-ordered queue in §5.

## 5. Remaining work (the execution plan)

Dependency order. Each item: id · what · why · deps · gated_on · effort.

### (a) G4 finish

- **P2.G4-O5** — Power-bank as `Farm{PowerBank}` objective + bespoke dropped-power haul collector. *Why:* crack-alone regresses (dropped power lands on HIGHWAY ground, not a HaulMission-readable transfer queue); required before O7 can delete `AttackMission::Exploiting`. *Deps:* O6. *Gated_on:* bundled into the O6/O7 window (operator 2026-06-18); needs the dropped-power collector extracted out of `AttackMission::Exploiting` first. *Effort:* M.
- **P2.G4-SK-DEADCODE** — Remove dead `AttackReason::SourceKeeper` arm (`duo_sk_farmer`) in `operations/attack.rs`. *Why:* nothing sets it (SK is fully `SourceKeeperOperation→Farm{SourceKeeper}`); doc comment misleadingly implies SK runs through `AttackOperation`. *Deps:* none. *Gated_on:* fold into O7 or do standalone now; trivial. *Effort:* S.
- **P2.G4-HEAVY** — Heavy multi-squad player assault (towers≥4 → drain-duo + sequenced quad). *Why:* doesn't fit one-squad-per-objective; the planner (`plan_by_detected_threat`/`build_force_plan`) lives only in `attack.rs`; `ForceRequirement.squads` is a Vec but `SquadManager` reads only `.first()`. O7 deletion is gated on this. *Deps:* O6, I1, I2. *Gated_on:* needs a new multi-squad/sequenced-objective mechanism in the queue+manager (AfterSquad/DeployCondition); design not started. *Effort:* L.
- **P2.G4-O7** — Delete legacy `AttackMission`/`AttackOperation`/`AttackReason` (+ variants, dispatch arms, `war.rs` ActiveAttack/active_attacks/reassign_home_rooms/update_threat_intel, `TargetSource→AttackReason` From impl, dead `SourceKeeper`/`InvaderCreeps` arms) and bump WFV. *Why:* completes "no legacy combat-driving mission code remains"; `SquadCombatJob` shrinks to pure order-execution + Recall. *Deps:* O5, HEAVY. *Gated_on:* **private-server (Docker) soak parity** — bot must clear a real invader core AND lay siege with the squad cohesive across rooms, no CPU spiral, no orphan/idle. "Do not delete AttackMission on faith." *Effort:* M.

### (b) Harness

- **P2.H4** — Scenarios + scripted opponents (rush/kite/turtle/drain) + self-play runner + SVG/ASCII replay scrubber; carries the **LIVE-combat-moves-score check for CP-H/M2**. *Why:* required to run the 0008a EXP-* register and close CP-H (military-score-moves on live combat is only kernel-tested). *Deps:* H1, H2, H3. *Gated_on:* nothing blocking; unstarted. *Effort:* L.
- **P2.H5** — Dockerized-server parity oracle + nightly N=9 seeded combat gate + byte-exact golden-vector capture. *Why:* acceptance gate / fidelity oracle; tightens the sim parity budget; the live seg-57 cohesion canary is the final arbiter. *Deps:* H4. *Gated_on:* depends on H4. *Effort:* L.

### (c) Identity

- **P2.I1** — SquadStore + minted, generation-carrying `SquadId`; `resolve(id)→same-squad-or-None`. *Why:* hard gate for CP-I; eliminates dangling-ref risk (IBEX-049 family); G2 currently keys on Entity (interim). *Deps:* G2. *Gated_on:* nothing blocking; ADR 0001 A1→A2. *Effort:* M.
- **P2.I2** — Re-key `SquadContext`/`SquadCombatJob` by `SquadId`; dangling-ref counter to seg-57; `#[serde(skip)]` on `CreepPathData.path` (IBEX-049); bump WFV. *Why:* completes the identity workstream, closes CP-I, removes the Entity-keying interim. *Deps:* I1. *Gated_on:* depends on I1. *Effort:* M.

### (d) War supervisor

- **P2.W2** — `WarOperation` as supervisor: withdraw/trim low-value objectives when `max_concurrent_attacks` shrinks (IBEX-028); feed real per-squad spend so economy-abort fires (IBEX-026); UnwinnableTarget backoff on the supervisor side. *Why:* only the defense half of migration step 5 is done; the supervisor still doesn't throttle/withdraw offense by real spend or cap shrink. *Deps:* O6. *Gated_on:* benefits from per-squad spend instrumentation. *Effort:* M.
- **P2.W3** — `Escort{room}` pre-clear producer in `claim.rs` for marginal claim targets (`DefenseEscalation::from_threat` sizing). *Why:* the `Escort` kind is defined + handled by `squad_manager` but has NO producer — inert; ADR 0017 expansion pre-clear. Operator decided "build it." *Deps:* O6. *Gated_on:* nothing blocking; unstarted. *Effort:* M.
- **P2.W4** — Thin `WarDecl` posture hook (player-offense only under WarDecl); feature-flag proactive de-reservation OFF (T-CTRL-3); register S11. *Why:* posture/ADR-0014 governance; reactive reserve denial always-on, proactive flagged OFF. *Deps:* W2. *Gated_on:* nothing blocking; unstarted. *Effort:* M.

### (e) Spawn

- **P2.S1** — `SquadManager` as sole combat spawn-demand producer (GroupId=SquadId); align-finish group admission + pre-spawn replacement. *Why:* synchronized spawning (ADR 0011); closes CP-S; pre-spawn deferred from G2. *Deps:* I1, I2. *Gated_on:* depends on SquadId. *Effort:* M.
- **P2.S2** — boost-on-spawn handoff (IBEX-027) behind kill-switch. *Why:* boost-commit policy (conservative floor, downgrade to unboosted when short). *Deps:* S1. *Gated_on:* gated on the ADR 0010 lab pipeline. *Effort:* M.

### (f) Source-keeper

- **P2.K2c-2** — Replace hardcoded `military_free: true` (`sourcekeeper.rs` L324) with a real yield-to-defense/war predicate; add Withhold/Veto retirement of an EXISTING farm when ROI drops or it falls out of haul range. *Why:* SK farm never stands down for active defense/war and never proactively retires an over-the-hill farm. *Deps:* K2. *Gated_on:* needs the war/defense posture signal (overlaps W4). *Effort:* M.
- **P2.K-RECONCILE** — Extract shared `ensure_source_mining(gate)` helper (SK duplicates `LocalSupplyMission`'s source loop); convert the outpost `DefendMission` child into a `Defend` objective. *Why:* de-dupes per-source vs room-level mining child logic; folds the last defender path onto the queue. *Deps:* K3, G4-defense. *Gated_on:* cleanup; nothing blocking. *Effort:* M.
- **P2.K4** — SK mineral mining. *Why:* completes the SK economic surface. *Deps:* K3. *Gated_on:* **DEFERRED/BLOCKED** — needs remote extractor/container construction + a market-glut predicate (ADR 0012 is trading-risk, not mining-glut). *Effort:* L.

### (g) Cross-cutting / future

- **P2.M2-LIVE** — M2 anchor-mover live behavioral validation (private server). *Why:* CP-M/M3 is code-done but not behaviorally live-validated. *Deps:* O6. *Gated_on:* needs a live Docker soak with an actual threat. *Effort:* S.
- **T-POS** — Attack-positioning experiment (`plan_engage_anchor`): reuse `search_scored` with combat attack-pricing (maximize EV — damage to focus incl RMA stacking, heal coverage, optimal weapon range; minimize damage taken). *Why:* generalizes scored search from kiting to offensive positioning. *Deps:* G3-tail, H4. *Gated_on:* operator — explore strictly AFTER kiting (now done); benefits from H4 self-play. *Effort:* M.
- **L1** — Cross-room flee/kite evacuation: when `LocalPathfinder::search_scored` finds no safe in-room tile, fall back to a server PathFinder multi-room flee (`search_many(flee)`); keep single-room scored search primary. *Why:* a creep cornered at a room edge with the threat between it and the interior can't escape; concrete K5 trigger (stronghold appears in farmed SK room, last duo/miners take tower fire exiting). *Deps:* G3-tail, K5. *Gated_on:* document + watch on Docker soak first; live-only, not self-play-validatable. *Effort:* M.
- **L2** — Trait-based combat view (avoid per-tick DTO copy on live path): `CombatCreep` live-impl over `screeps::Creep`, sim-impl over `SimCreep`, decisions generic. *Why:* drops per-tick copy/alloc (CPU+GC) on the live combat path. *Deps:* H2. *Gated_on:* **MEASURE FIRST** — DTO build reads cached RoomData (cheap-ish); gate on a measured live CPU win. *Effort:* M.
- **EXP-REGISTER** — Run the ADR 0008a ordered EXP-* register (items 1–17): FOUND → KITE → FOCUS → TOWER → BREACH → COMP → DEF → ENGAGE → NPC → CTRL → PARITY, each hypothesis/scenario/metric/gate; sim-per-change + server-at-acceptance. *Why:* turns the ~55-tactic catalog + ~70-knob table from hypotheses into shipped defaults; G3 tactics are live but untuned against the register. *Deps:* H4, H5. *Gated_on:* depends on the harness. *Effort:* L.
- **P2.M4** — pull for under-MOVE compositions (Follow/pull for no-MOVE/under-MOVE bodies). *Why:* only sanctioned use of lead-follower. *Deps:* M2-LIVE. *Gated_on:* conditional — only if under-MOVE compositions get fielded; otherwise skip. *Effort:* S.
- **CP-CHECKPOINTS** — Reach CP-I/CP-G/CP-W/CP-S/CP-K + close CP-H/CP-M live + audit the 11 M4 exit criteria. *Why:* checkpoints are the milestone gates for the whole Phase 2 overhaul; all 11 exit criteria pending. *Deps:* O7, I2, W4, S2, K2c-2, M2-LIVE. *Gated_on:* aggregates all workstreams + live validation. *Effort:* M.

## 6. Legacy still present (delete-tracking — input to the O7 checklist)

Compiled-but-scheduled-for-deletion combat code and why it can't go yet:

- `operations/attack.rs`: **`AttackOperation`** (full Recon/Prepare/Execute/Exploit/Complete state machine), `AttackReason`, `AttackPhase` — still launched by `war.rs:run_offense_evaluation` for **PowerBank** (O5 deferred). ThreatResponse/Expansion/ProactiveDefense have no producer (dormant but mappable).
- `operations/attack.rs:build_force_plan`/`plan_by_detected_threat` — the **heavy multi-squad assault planner** lives only here (G4-HEAVY-gated).
- `AttackReason::SourceKeeper` arm — **DEAD** (SK-DEADCODE; nothing sets it).
- `AttackReason::InvaderCreeps` arm — dead via `war.rs` reconcile into the defense remote-invader path.
- `missions/attack_mission.rs`: **`AttackMission`** — **sole owner of the power-bank crack→haul (`Exploiting`/`power_bank_haulers`)** and the multi-room siege layered-dismantle/wave-retry; referenced by `jobs/squad_combat.rs` anchor-path branch.
- `war.rs`: `ActiveAttack`/`active_attacks`/`reassign_home_rooms`/`update_threat_intel` + `TargetSource→AttackReason` From impl + legacy launch path — all exist solely to drive legacy `AttackOperation`s.
- `ObjectiveKind::Escort` — defined + handled, **no producer** (inert; W3).
- `sourcekeeper.rs`: `military_free: true` hardcoded (TODO K2c-2/W); no Withhold/Veto retirement of an existing farm (K2c-2).
- `war.rs:run_heavy_recompute` ProactiveDefense — only `debug!`-logs; produces no objective.

## 7. WORLD_FORMAT_VERSION ledger

| Change | WFV |
|---|---|
| M2 anchor mover | 7→8 |
| G1 CombatObjectiveQueue | 8→9 |
| G2 SquadManager | 9→10 |
| K3 per-source mining | 10→11 |
| G4 defense-half | 11→12 (**current Docker**) |
| I2 (SquadId field) | pending |
| O7 (removed enum variants) | pending |

**Standing rule:** `WORLD_FORMAT_VERSION` in `game_loop.rs` MUST bump on any serialized-shape change = one loud reset (reset-anytime policy).

## 8. Operator-settled constraints + decisions

Standing rules (single home):
- **Harness** = hybrid combat micro-sim running the bot's OWN code (focused deterministic sim + complementary Docker server).
- **Movement** anchor-primary, NOT lead-follower; corridors via relaxing the mover; `Follow`/`pull` for no-MOVE bodies only.
- **Squad on objective-complete** = retask-if-viable-else-recycle (Recall).
- **Boosts** wired behind a kill-switch (conservative floor; downgrade to unboosted when short).
- **Missions provide context / jobs own creep intent** including movement. The `AttackMission` §5 inversion is being unwound — new work must NOT extend it.
- **Anti-overfitting:** no opponent-specific constants; threat read at runtime; seed + opponent diversity; the live **seg-57 cohesion canary is the final arbiter**.

Old Q1–Q7 decisions, now SETTLED: Q1 retask-on-complete (realized); Q3 wire boosts behind kill-switch; Q4 orphan recovery realized via Recall + retask; Q5/Q2 subsumed by O4 wave-retry + unwinnable backoff + `MAX_CONCURRENT_SQUADS` cap; Q6/Q7 as recorded. (Stale crate name `screeps-ibex-metrics` → the landed crate is `screeps-combat-decision`.)

## 9. Checkpoints + exit criteria

(Milestone view; entry/exit gates in phase-2.md §2.9.)

- **CP-H** — code-done; **PARTIAL** (military-score-moves on combat is kernel-tested only; the LIVE combat-move carries to H4). M2 milestone not fully closed.
- **CP-M** — code-done; behavioral live-combat validation PENDING (M2-LIVE).
- **CP-I / CP-G / CP-W / CP-S / CP-K** — NOT reached.
- All **11 M4 exit criteria** PENDING.

Unblock map: CP-H/CP-M ← live soak + H4; CP-I ← I1/I2; CP-G ← O7; CP-W ← W2–W4; CP-S ← S1/S2; CP-K ← K2c-2 (+ K4 deferred).

## 10. Doc map + history pointers

- **Design end-state:** ADR [0008](../design/0008-combat-and-squad-architecture.md) (+ [0008a](../design/0008a-combat-tactics.md) tactics annex + EXP register).
- **Landed-task history with SHAs:** [`phase-2.md`](../execution/phase-2.md) §2.0 (newest-first); §2.9 checkpoints.
- **Archived sub-plans (ARCHIVED in place):** [`../execution/g3-tail-plan.md`](../execution/g3-tail-plan.md) (8-step kite history; T-POS/L1/L2 future now tracked in §5 here) and [`../execution/g4-offense-plan.md`](../execution/g4-offense-plan.md) (O-series landed history + the `AttackReason→ObjectiveKind` mapping table + O7 deletion checklist).
- **This doc is the source of truth for status and supersedes the phase-2.md Status columns.**