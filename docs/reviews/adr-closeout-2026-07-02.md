# ADR Corpus Closeout — 2026-07-02

**Purpose.** A full pass over all ~50 ADRs to (1) verify status vs code, (2) close out every open design question via operator interview, (3) make each ADR resumable, and (4) produce the master kickoff prompt for a future ultracode session to drive **all** remaining work to completion with **no deferrals**.

**Method.** 8-cluster read-only review (verify-code-over-docs) → synthesis → operator interview (this doc's §2 decisions) → per-ADR resumability patches → [ultracode-completion-kickoff.md](ultracode-completion-kickoff.md).

**Headline finding: documentation drift, not open defects.** The combat + foundation work is largely landed and **deployed to MMO at WFV 26** (1227 workspace tests green), but many foundational ADR headers still read "Proposed," and several prescribed mechanisms were **superseded by different shipped solutions**. See the reconciliation ledger [reconciliation-2026-07-01.md](reconciliation-2026-07-01.md) (§5 ADR status table, §6 backlog).

---

## 1. Operator directives governing the completion pass

- **WFV/serialized-shape bumps are FINE** (deploy resets anyway) — never defer or half-build to dodge a bump ([[wfv-fine-clean-design-no-debt]]).
- **Clean design, NO tech debt** — right crate, root causes, clear MINOR findings before commit, no drift.
- **Verify code over docs** — ADR headers drift; code + the reconciliation artifact are ground truth.
- **NO deferrals in the final push** — implement, or explicitly stop-and-ask for design/implementation guidance; never silently skip.

## 2. Operator decisions (interview 2026-07-02) — all 13 open questions CLOSED

| # | ADR(s) | Question | **DECISION** |
|---|---|---|---|
| Q1 | 0001 / 0005 | Minted-SquadId+SquadStore vs the shipped marker `EntityOption<Entity>` fix (REC-009b) | **Ratify the marker fix; supersede 0001's mechanism.** repair_entity_integrity stays as the backstop. 0005's specs-dispatch-replacement dies with it. |
| Q2 | 0002 | Serialization Stage-2 format swap | **Leave positional bincode + WFV-loud-reset as the end-state.** Build Stage-2 only if a genuine no-reset migration ever appears. IBEX-049 (serde-skip rover path) stays operator-DECLINED. |
| Q3 | 0003 / 0007 | Data-driven FSM rewrite (replace screeps-machine) | **Abandon the FSM.** Deliver the one live win — `MissionResult::Wait/Idle` transient-tolerance for **economy** missions — as a scoped standalone change. |
| Q4 | 0003 §B | Footprint-aware anchor-mover cohesion | **Superseded** by the shipped SquadManager + combat-decision kernel + rover formation. |
| Q5 | 0007 | TransferSnapshot two-phase hauler matcher | **BUILD it** — the pure, replay-diffable matcher (two-phase snapshot + governor-gated re-match). |
| Q6 | 0009 / 0038 | Route-distance sizing (IBEX-032) ownership | **ADR 0038 owns it.** Drop 0009 D3.5; re-scope 0009 D3 to graph-model + inter-room roads; 0007 item-4 reuses 0038's route machinery (one policy). |
| Q7 | 0009 | Uncapped fingerprint-mismatch plan restart | **Accept the shipped Failed{attempts}+escalation design + a cheap warn-once + seg-57 counter** (completed plans persist → CPU-waste only). |
| Q8 | 0009a / 0009b | Unprovable no-plan-loss gate | **Add the adaptive beam-widen/cap-lift fallback** (hang on 0009c ESCALATION_BEAMS) to make no-plan-loss provable. |
| Q9 | 0009b / 0038 | Sequencing 0009b's claim.rs recalibration vs 0038 | **Verify claim.rs is still the sole `plan.score.total` consumer post-0038, then re-tune `plan_score_weight`/`max_score_delta`; bump from LIVE WFV 26** (not the stale 7→8). Cross-room composite for expansion desirability is a clean follow-up. |
| Q10 | 0011 | SpawnOrchestrator scope | **Re-scope to the economic half + Step-0 quick-wins.** Do the None-breaking executor fixes now (renew-before-CRITICAL inversion, renew energy over-charge); build the economic orchestrator (throughput budget, cross-room assist/G3 incubation, starvation cure); **drop** the obviated combat-cohesion pieces (handled by the auction lifecycle). |
| Q11 | 0015 | screeps-testkit vs the combat eval stack | **BUILD the full generic testkit + Seam Contract Registry, and migrate the combat-eval/IntentRecorder stack onto it** (uniform contract pattern across all seams). |
| Q12 | 0010/0012/0013/0014/0017/0018 | Empire/economic tier scope | **INCLUDE the full empire executive layer in this pass** — 0010 boost-lab-factory pipeline (also 0041's supply producer) → 0012 market/risk → 0013 power economy/power-creeps → 0014 empire strategy/posture (arbitration capstone) → 0017 threat-aware expansion lifecycle → 0018 SK exploitation. |
| Q13 | 0016 | Glance HUD redesign + renderer-corruption bug | **BUILD the full HUD redesign.** Operator hint: **the render corruption appears to come from world (RoomVisual) visuals** — focus the Field Report H fix there. |
| — | 0041 | (Resolved earlier, 2026-07-02) | Boost layer Accepted: dark-first, full T0→T3 uniform-per-body ladder, market-priced-with-validated-approximation-fallback cost, offense-first. See ADR 0041 §8. |

## 3. Resumability status

**Already fully resumable:** 0004, 0005 (dies with Q1), 0006, 0009c, 0033, 0037, 0038 (bar the deploy note), the combat corpus 0019–0037 (reconciled in reconciliation §5), 0040, 0041.

**Patched this closeout** (status line flipped off stale "Proposed" + a decision-log block + enumerated remaining-work + SHAs/anchors + a resume-point): **0001, 0002, 0003, 0007, 0009, 0009a, 0009b, 0011, 0015**, plus the two aggregators (combat-overhaul-plan.md, design/README.md — DOC-1/DOC-3, already applied in the reconciliation). Each patched ADR now carries its Q-decision from §2.

## 4. Dependency-ordered remaining-work rollup

Ordered free→gated. Full detail + phasing lives in [ultracode-completion-kickoff.md](ultracode-completion-kickoff.md).

**Tier 0 — free, high value-per-effort (do first):**
1. **Doc-truth**: apply the resumability patches (§3) — status/decision-log/resume-point on the 9 stale ADRs. (this doc)
2. **0011 Step-0 spawn-executor quick-wins** (S, None-breaking): engine-true renew energy decrement, move renew behind the priority check (fixes P4 renew-before-CRITICAL inversion), `debug_assert!(priority.is_finite())`, comparator unit test (do NOT reverse the comparator — verified-correct).
3. **0009 D1 residual** (S): warn-once + seg-57 counter on fingerprint-mismatch restart (Q7).
4. **0002** (S): verify the segment-fullness fail-loud-on-overflow half is complete.

**Tier 1 — planner + economy-mission robustness (mostly host, no live risk):**
5. **0009a** provability fallback (S, Q8) + doc hygiene.
6. **0003** `MissionResult::Wait/Idle` for economy missions (M, Q3 standalone win).
7. **0009b** planner: §7 ground-truth bench (M) → §4.3/§4.6 cost terms (M) → §3/§5/§6 scoring re-weight (L) → §8 step-6 WFV bump from live-26 + claim.rs recalibration (M, Q9, cross-check 0038) → §8 step-7 calibration sweep.
8. **0009a/0009b** scorer-quality remainder (L) + placement-driven reachability hub-approach-tile reservation (cross-check 0009c first).

**Tier 2 — build-outs the operator selected:**
9. **0007** TransferSnapshot two-phase hauler matcher (M, Q5 — BUILD).
10. **0009 D3** RoomGraph → exit-affinity → InterRoomRoadLayer (XL; D3.5 dropped per Q6).
11. **0011** economic orchestrator: D2 throughput budget → D5 cross-room assist/G3 incubation → D7 starvation cure (XL, Q10).
12. **0015** full generic screeps-testkit + Seam Contract Registry + migrate combat-eval/IntentRecorder onto it (XL, Q11).
13. **0016** full Glance HUD redesign + fix the RoomVisual render-corruption bug (L, Q13).

**Tier 3 — the empire executive layer (Q12, full):**
14. **0010** boost-lab-factory pipeline (L0/L1 supply is also ADR 0041's producer dependency) → **0012** market/risk → **0013** power economy/power-creeps → **0014** empire strategy/posture (arbitration capstone) → **0017** threat-aware expansion lifecycle → **0018** SK exploitation. (XL initiative.)

**Tier 4 — flagship combat + tuning (interleave as unblocked):**
15. **0041** combat boost layer: P0 dark kernel → P1 populate `available_boosts` + supply clamp → P2 persisted `CombatBodySpec.boost` (WFV 26→27) → P3 `AwaitBoost`/`boostCreep` lifecycle (gated on 0010 L0/L1 stocking compounds) → P4 tournament re-sweep.
16. **[TUNE]** eval sweeps (M each, needs-compute): 0031a Tier-2 weapon-archetype in EV search (highest-priority composition follow-up) + tunables + P6 re-sweep; 0026 §9.10 L6c DoctrineParams; 0019 St.4 weight-discriminating bed. (0031b re-tune already RAN — seeds hold.)
17. **[ADR-then-build]** 0020 §11 S5 blob auction (needs the R7 cross-goal EV currency) + S6 archetype classifier/meta-Nash + S7 adversarial room-gen; 0030 §9 EngagementTempo phases 2–6; 0031 Tier-2 archetype design; 0025a §2 object-anomaly root-cause; the budget-free `emit_requirement` assess redesign (optimizer_ceiling_budget is the winnability seam — a calibration-changing redesign, not a cleanup).
18. **[OP/soak]** 0035 FU1/FU2, 0029 §10 W9N8 re-soak, 0033 Docker soak — watch live behavior; land the give-up/scout-first pipeline once soak evidence exists.

## 5. Superseded / closed (no work — recorded to prevent duplication)

- **0001** minted-SquadId+SquadStore → marker `EntityOption<Entity>` (REC-009b, WFV 24→25). **0005** runtime-model-off-specs dies with it.
- **0003 §B** anchor-mover cohesion → SquadManager + combat-decision + rover formation (deployed).
- **0011** combat-cohesion (D3/D4-combat/D8-combat) → objective-queue/auction lifecycle (ADR 0027/0032, deployed).
- **0009 D3.5** route-gating → ADR 0038 (IBEX-032 owner, committed cf5e8be).
- **0002** Stage-2 → obviated-for-now by reset-anytime (Q2). **IBEX-049** serde-skip rover path → operator-DECLINED.
- **0003** FSM rewrite → abandoned (Q3); guarded intent sink already removed the hazard.
