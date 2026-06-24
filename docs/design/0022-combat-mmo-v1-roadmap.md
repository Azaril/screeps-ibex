# 0022 — Combat → MMO v1 roadmap (end-state, reconciled)

Status: **Accepted 2026-06-23** (operator-driven re-plan; supersedes the incremental P1–P5 / R-ladder framing in ADR 0020 §10–§12 as the *sequencing* authority — the sizing *design* in 0020 still stands).

## Why this exists

We have repeatedly deployed combat to the private server that did **zero attack damage**, because the work was sequenced as incremental stepping-stones that each shipped an incomplete design — every soak was missing "one more piece," so we could never tell if anything worked. Operator directive: **stop incremental deferrals; build each area to its end-state; reconcile all outstanding work; do not soak incomplete code; deliver real MMO value.** This ADR is the single reconciled plan. It was produced by a 7-agent reconciliation of the actual code + ADRs 0001/0006/0008/0019/0020/0021 against the operator's 8 work areas, then an operator interview on the open forks.

The spine: **movement, objectives, identity, and sizing are each proven correct in isolation BEFORE the first combat soak.**

## Decisions (D1–D15)

Settled by the operator directive or the interview. **Interview answers are marked ★; operator overrides of the analysis are marked ⚑.**

| # | Decision | Resolution |
|---|---|---|
| D1 ★ | SquadId implementation | **ECS Entity handle** (index+generation) + validate-on-access; `repair_entity_integrity` kept as a **permanent** dangling-ref backstop. This is ADR 0001's "A1"; A2/A3 (mint a real id to delete the scan) are declined. Overrides ADR 0001's minted-id end-state. |
| D2 | Composition selection | **Curated template library** selected by target type; `sized_for` force-sizes slots. Role-auction (S5) → P5. |
| D3 | Role exceeds 50-part cap | **Defer** (`sized_for` → None; never field undersized). Member-count scaling → P5. |
| D4 | Defense composition | **Keep static `DefenseEscalation`** (no oracle on defense for v1). |
| D5 ⚑ | Invader-core kill model | **Cumulative-siege in v1** (operator override of "one-lifetime only"). Cores never heal, so the live `structure.hits` *is* the cumulative residual — winnability becomes "net structure-DPS > repair" with the kill allowed to span squad generations; the bot sustains presence on the persistent objective. **R-attack still lands** (fast one-lifetime kills where possible; cumulative is the guarantee for the slow tail). No new serialized shape (live hits = state), so the single-reset invariant holds. |
| D6 | R-attack (sized RANGED) | **Land in v1 (P-CORE).** Soak-confirmed blocker. |
| D7 | Multi-room sim scope | **Lightweight two-room movement harness first** (P-MOVE); full-fidelity engine → P5 only if a scenario needs cross-room combat to diagnose. |
| D8 | Anchor pathfinding | **Keep straight-line signum advance** for v1; footprint-pathfind fix only if a corridor/tight-terrain scenario fails (it is an optimization gap, not the random-rooms cause). |
| D9 | Completion detection | **Manager-polled `SuccessPredicate`** (one pure fn per `ObjectiveKind`); producers own request cadence only. |
| D10 | Idle-creep elimination | **`Recall` 4th FSM state** in `SquadCombatJob` (self-transition when squad lookup → None) + the already-immediate ephemeral claim release. |
| D11 | Spawn alignment | **One-tick synchronized spawn** from a shared `GroupId = SquadId` token, deterministic `slot_index` order. |
| D12 | Gather enforcement | **Two-stage**: hard all-present-or-timeout **Rally** gate, then soft 75% quorum for formation **Move**. |
| D13 | INVULNERABILITY skip | **war.rs pre-filter** (skip a deploying core before building a `DefenseProfile`) **+ a re-assess timer** so a skipped core is re-checked after the deploy window, never permanently abandoned (mirrors ADR 0021 stale-threat re-scout). |
| D14 | Importance margin (R5) | **Fixed formula** (`1 + importance×0.5`) for v1; per-objective field / P(win) gate → P5 (a per-objective field would add a serialized shape = a 2nd reset). |
| D15 | SK keeper-slip recovery | **Passive** via R6 `HOLD_MARGIN` sized healer; active emergency-kite → P5. |
| **Retask-fitness** ★ | Squad finishes A, retasks to B | **Re-run the oracle against B before re-claiming.** Only retask a healthy squad to B if its *existing composition* wins B; else retire + re-spawn correctly-sized. Closes the load-bearing P-OBJ↔P-CORE seam (prevents the engage/retreat cycle recurring via retask). Uses the existing `force_sizing::assess` (extended in P-CORE). |
| **Core priority** ★ | Cores starved under the 4-squad cap | **Promote winnable invader cores to HIGH + light preemption** — pull a small war-supervisor slice into v1 that can trim a lower-value squad to free a slot for a winnable core. Directly answers "we're losing ground." |

## Phased roadmap

Ordering rationale: P0 freezes every fork (you cannot build end-state designs with open forks). Then the three correctness pillars run in parallel off P0 — **P-MOVE** (cross-room movement, the operator's loudest doubt), **P-OBJ** (no idle/orphan creeps, observable completion + scouting), **P-ID** (serialize-safe identity, no recycled-slot aliasing). **P-SPAWN** follows (needs identity + the rally gate). **P-CORE** (sizing: R-attack + cumulative-siege + INVULNERABILITY skip) follows P-OBJ (it sizes whatever the objective demands; its oracle also backs retask-fitness). Only when **all five** are proven does **P-SOAK** run the first real combat soak (the literal embodiment of "don't soak incomplete code"). **P-DEPLOY** ships MMO v1 with exactly one loud reset. **P5** batches every deferred exotic/scale item, each its own complete end-state, none blocking MMO value.

### P0 — Lock decisions + freeze contracts
- **Deliverables:** this ADR merged (D1–D15 + retask-fitness + core-priority resolved); frozen end-state type contracts as compiling stubs: (a) `SquadId = Entity` handle signature, (b) `SuccessPredicate` trait + per-`ObjectiveKind` list, (c) two-stage `Forming→Rallying→Moving` `SquadState` contract (note: `Rallying`/`Complete` enum variants already exist but are inert — activating them is shape-neutral), (d) `RequiredForce` + `ranged_parts`, (e) final `DefenseProfile`/`ForceAssessment` field set; the **WFV ledger** (only P-ID bumps shape: 16→17 = one reset).
- **Exit:** ADR merged; contract stubs `cargo check`-green with `#[allow(dead_code)]`.

### P-MOVE — Cross-room movement provably correct (points 2,4,5)
- **Deliverables:** lightweight **two-room movement sim** in `screeps-combat-agent` (room-qualified positions, minimal room graph + exit selection, multi-room `ScenarioBuilder`; movement-only, room-scoped damage). Two-stage cohesion wired (hard `Rallying` gate → soft formation advance). Validate the 24f5494 `cross_room_formation_target` (already has 3 unit tests) **and** add the missing **boundary-hold quorum** coverage. Scenarios as passing tests: `SQUAD-CROSSROOM-1`, `BOUNDARY-HOLD-MAJORITY`, `CORRIDOR-CROSS`, **`STUCK-MEMBER-TIMEOUT`** (timeout fires → squad advances at quorum, deterministic — required, not happy-path-only). Trace-capture + offline-replay parity tool (bot emits a boundary-cross trace; harness replays → position parity).
- **Exit:** all four scenarios pass deterministically; a Docker movement soak (squads traversing 2+ rooms, light/no combat) replays through the harness with position parity. If parity diverges, the harness pinpoints the broken gate (this is the operator's confidence answer). Straight-line anchor advance kept (D8); footprint-pathfind is a follow-on only if `CORRIDOR-CROSS` fails.

### P-OBJ — Objective lifecycle: zero orphans + scouting (points 3,4,8)
- **Deliverables:** centralized **`SuccessPredicate`** (one pure fn per `ObjectiveKind`; manager polls each tick; `sourcekeeperfarm`'s inlined withdraw folds into the `Farm` predicate). **Retask-on-complete with oracle fitness** (Phase A: release claim + re-pull next-best + **re-run the oracle against the candidate** + re-claim a healthy squad only if its composition wins it, else retire — no idle window, no losing retask). **`Recall`** FSM terminal state (squad lookup → None → return home + volunteer-defend). **Strategic re-scout scheduler** (`ScoutOperation::inject_strategic_rescout` generalizes the ADR 0021 war.rs pattern: hot/candidate/lazy tiers + OBSERVE-only downgrade). **`Escort` producer** in `claim.rs` (the only `ObjectiveKind` with no producer; note its predicate is *hand-off-shaped* — completes when safe-to-start, not kill-shaped). **Core priority + light preemption** (winnable cores → HIGH; manager can trim a lower-value squad for a winnable core). seg-57 canary: per-kind completed/withdrawn counters + an **orphan-at-tick** counter.
- **Exit:** unit test per `SuccessPredicate`; sim: complete-then-retask has no idle tick AND a squad never retasks to an objective its comp loses; kill a squad mid-fight → creeps `Recall`, none idle-to-TTL; Docker soak: orphan-at-tick stays 0.

### P-ID — Serialize-safe squad identity (points 4,5; D1)
- **Deliverables:** `squad_entity` migrated `u32` → generation-carrying **`Entity` handle** in plain-serde `JobData`, with validate-on-access (stale handle → None, no recycled-slot aliasing). All squad-entity deletions routed through `EntityCleanupQueue` so `repair_entity_integrity` scrubs dangling refs pre-serialize (no `ConvertSaveload` halt; see [[ecs-dangling-ref-serialize]]). `repair_entity_integrity` **kept** (the Entity path needs it — permanent per D1).
- **Exit:** stale-handle resolves to None at the access seam; serialize/deserialize across a simulated VM reset preserves creep→squad binding (no scatter); integrity scan reports zero dangling refs on a soak. **WFV 16→17** applied here — the single loud reset, folded into P-DEPLOY.

### P-SPAWN — Synchronized spawn + gather (points 4,5; D11,D12)
- **Deliverables:** `SquadManager` is the sole combat spawn-demand producer; one shared `GroupId = SquadId` token, all slots fulfilled in deterministic `slot_index` order (kills HashSet-iteration nondeterminism), align-finish window W=1 (equal TTL). `Forming→Rallying` hard gate enforced at spawn (rally at one point or timeout before `Moving`). Remove the independent per-slot broadcast-token `queue_slot_spawn` path.
- **Exit:** sim: a 4-member squad spawns within W at equal TTL, gathers at rally, does not advance until the gate passes; member→home assignment stable across runs. **Includes a force-SIZED (`BodyType::Sized`) squad spawn test** — not just a template — so sized-body spawn is proven before P-SOAK, not first-contact-tested live. No WFV bump.

### P-CORE — Force-sizing complete (points 1,6,7; D5,D6,D13)
- **Deliverables:** **R-attack** (`RequiredForce::ranged_parts` from kill-time; `sized_for` sizes `RangedDPS`; gate becomes "do required ranged parts fit at home energy?"). **Cumulative-siege** (oracle reads the core's live `hits` as the residual; winnability = net structure-DPS > repair for non-repairing targets, kill may span generations; one-lifetime gate removed for repair=0 targets; an EV/sanity cap bounds absurdly-slow far sieges). **INVULNERABILITY skip** (war.rs pre-filter + re-assess timer, D13). The full `target→DefenseProfile→winnability→RequiredForce(heal/dismantle/ranged/tough/move)→composition-selection→per-member-sized-bodies` pipeline confirmed coherent and target-driven (cores→`quad_ranged` ranged-sized; SK→`duo_sk_farmer` healer-sized; defense→static escalation). The oracle also serves P-OBJ retask-fitness.
- **Exit:** host tests: R-attack sizes ranged to a 100k core within onsite budget at RCL6; a previously-"breach too slow" core returns winnable + fields a squad (regression-locked); cumulative-siege returns winnable on net-positive DPS for a partially-damaged core; INVULNERABILITY-skip never assesses a deploying core then re-assesses after; `force_sized_squad_keeps_holding_while_damaged` + `sk_setup_*` stay green; oracle within the CPU bench budget. **Goal scope is "correctly sized / deferred"** — the actual *clear* is proven in P-SOAK.

### P-SOAK — First real combat soak (points 1,2,4,5,6,7)
- **Deliverables:** private-server force-soak (offense=true; all neutral rooms active+restarted): a winnable invader-core clear end-to-end (assess→size→spawn→gather→cross-room move→engage→kill→retask); an SK duo holding **0 deaths** while mining children run; defense escalation vs an injected threat. Live validation of the anchor mover, two-stage cohesion, retask-on-complete, and `Recall` under fire via seg-57 canary + boundary-cross trace replay (movement parity).
- **Exit:** SK 0-deaths; cores cleared (no engage/retreat cycling — P2b sizing-to-energy bug closed by P-CORE + retask-fitness); orphan-at-tick 0; cohesion canary shows blocks not scatter; every defect fixed and re-soaked, not deferred. **Note:** combat-tick *position* parity is NOT a gate here — the P-MOVE harness is movement-only (D7); combat is validated by live soak + canary, not harness replay.

### P-DEPLOY — MMO v1 (points 1,6,7,8)
- **Deliverables:** WFV re-proof (git diff confirms the only serialized-shape change is P-ID Entity-handle → bump once 16→17 = one loud reset); `attack_players` OFF; `MAX_CONCURRENT_SQUADS` static 4. Deploy via screeps-pack **only after explicit operator go-ahead**. Post-deploy watch: seg-57 canary + per-kind completion counters; rollback trigger = scatter regression or orphan-count > 0.
- **Exit:** clean single reset; no serialize halt/recover loop; canary shows cohesive squads clearing cores / holding SK / defending; operator confirms real MMO value.

### P5 — Exotic / scale fast-follows (each a complete end-state, none blocking v1)
Role-auction S5 + cross-goal EV currency R7 (behind a tournament + exploitability gate); multi-squad **G4-HEAVY** (stronghold pre-clear + assault; escort pre-clear → claimer); member-count scaling (D3); boost handoff S2 (needs ADR 0010 lab); war supervisor W2–W4 (full trim/economy-abort/posture — the v1 light-preemption slice is the seed); full multi-room combat-fidelity harness (U11/U12, L1 cross-room flee); player offense (`attack_players` + adaptivity S6, only after the exploiter gate); SK mineral K4; empirical tuning (HOLD_MARGIN, importance_margin, Lanchester n, P(win)-as-gate) on live + tournament telemetry.

## Reconciliation of outstanding work (keep / drop / redo / fold-in)

**KEEP (proven, no rework):** force-sizing ladder R1–R6 + hold-margin; P2a intel (WFV 15); defense static escalation; comp template library; Lanchester gate + safeMode veto + rampart-redirect fidelity; CombatObjectiveQueue + SquadManager; scouting architecture (VisibilityQueue/ObserverSystem/ScoutOperation); G3 heal/orientation; SK K0–K3/K5; `repair_entity_integrity` (now permanent per D1); `MAX_CONCURRENT_SQUADS=4`; rover Follow/pull (dormant, no speculative build); WFV=16.

**REDO (build to end-state):** R-attack (P-CORE); INVULNERABILITY skip (P-CORE); centralized `SuccessPredicate` (P-OBJ); retask-on-complete **with oracle fitness** (P-OBJ); `Recall` FSM state (P-OBJ); strategic re-scout scheduler (P-OBJ); `squad_entity` u32→Entity (P-ID); boundary-hold + cross_room validation/tests (P-MOVE).

**FOLD-IN (realize an existing design into a phase):** synchronized spawn / align-finish (ADR 0011 D3/D4/D9 → P-SPAWN); Escort producer (P-OBJ); **cumulative-siege → now P-CORE/v1** (operator override D5, via live-hits — no separate storage); tournament/exploitability gate + EV currency R7 + S5 + player-offense + S2 → P5.

**DROP:** ADR 0001 minted SquadId + SquadStore (D1 override — Entity instead); multi-room XL-rewrite deferral framing (replaced by the lightweight harness, D7); W2–W4 *full* supervisor as a v1 item (only the light-preemption slice comes to v1 via core-priority; the rest → P5).

## WFV ledger (single reset)
WFV is **16** today (`game_loop.rs`). The **only** serialized-shape change in this plan is **P-ID** (`u32`→Entity handle) → **16→17**, the one loud reset folded into P-DEPLOY. `SuccessPredicate`, retask, `Recall`, R-attack, cumulative-siege (live-hits, no storage), spawn-align, and the fixed importance formula (D14) all add **no** serialized shape. **P0's WFV ledger is a gate** — any phase that sneaks in a shape change (e.g. a per-objective importance field, or residual-hit storage) breaks the single-reset guarantee and must be re-litigated.

## Open risks
1. "Random rooms" might be straight-line anchor advance (D8 keeps it), not formation logic — if MVP scenarios pass but live traces still diverge, the footprint-pathfind follow-on becomes load-bearing.
2. P-SOAK is the first true combat exposure of the *whole* stack at once; compounded first-contact bugs may force several re-soak cycles (the "don't soak incomplete code" rule forbids shortcuts).
3. Two-stage rally timeout mis-tuning → hang or scatter; mitigated by the mandatory `STUCK-MEMBER-TIMEOUT` test.
4. Entity-handle SquadId keeps `repair_entity_integrity` as permanent overhead (deliberate, D1).
5. Cumulative-siege ties up a squad slot for slow far cores; mitigated by core-priority + light preemption + an EV/sanity cap on est_ticks.
6. Hand-chosen HOLD_MARGIN / importance_margin / Lanchester n are unvalidated live — conservative-by-default means over-investment (wasted energy), not casualties; tuned in P5.

## Cross-references
ADR 0001 (SquadId — D1 overrides to Entity/A1-permanent), 0008 (combat architecture / objective queue / tactics / anchor movement), 0019 (position selection), 0020 (EV/force-sizing design — still the sizing authority; this ADR supersedes its §10–§12 *sequencing*), 0021 (strategic visibility — scheduler realized in P-OBJ), 0006 (eval harness — extended to two-room in P-MOVE). Living tracker: `docs/execution/phase-2.md`.
