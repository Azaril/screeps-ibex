# docs/design/

Architecture Decision Records (ADRs) for the rewrite's pillars, plus design notes.

- New ADRs: copy `adr-template.md`, number sequentially (`NNNN-title.md`).
- **Status lifecycle:** Proposed → Accepted → Superseded by NNNN.
- The five seed ADRs below are **Proposed stubs**, pre-filled with the current approach, the pain, and candidate alternatives drawn from the review prompt. Fill in **Decision / Consequences / Migration Path** after the review.

| ADR | Pillar | Drives | Status |
|---|---|---|---|
| [0001](0001-entity-model.md) | Entity model (ECS vs handles vs ID-store) | Field Report E | Proposed |
| [0002](0002-serialization.md) | Serialization & persistence | Field Report D | Proposed |
| [0003](0003-behavior-modeling.md) | Behavior modeling (FSM vs BT vs utility) | Field Report F, A | Proposed |
| [0004](0004-cpu-governance-and-load-shedding.md) | CPU governance & load-shedding | Field Report C | Proposed |
| [0005](0005-runtime-and-scheduling-model.md) | Runtime & scheduling model | — | Proposed |
| [0006](0006-eval-and-iteration-harness.md) | Local-server eval & iteration harness | review §11/§12, plan Increment 0 | Proposed |
| [0007](0007-hauling-logistics.md) | Hauling & logistics (algorithmic complexity) | IBEX-011/030/031/050, "Hauling is NP-hard" | Proposed |
| [0008](0008-combat-and-squad-architecture.md) | Combat & squad architecture (scout-style objective queue + generic Squad Manager + anchor mover) | Field Reports A/B, IBEX-001/002/026/027/028/043 | Proposed |
| [0008a](0008a-combat-tactics.md) | Combat tactics & behavior catalog + experiment register (the concrete behaviors 0008's tactics model executes; iterated on the 0006 sim) | operator: ADR/plan underspecified on actual tactics | Proposed |
| [0009](0009-room-planning-and-multiroom-layout.md) | Room planning & multi-room layout (foreman) | IBEX-032/036/037 | Proposed |
| [0009a](0009a-room-planner-performance.md) | Room-planner performance: diagnosis & redesign (candidate-explosion → defendable-by-construction footprint) | operator "very slow"; overturns 0009's "no rewrite" premise | Proposed |
| [0009b](0009b-room-planner-scoring-and-evaluation.md) | Room-planner scoring, evaluation & RCL revamp (tower coverage, source-proximate early extensions, storage→refill hauling; independent bench evaluator) | operator: tower coverage 0.302 over 5123 rooms; scoring discrimination collapse | Proposed |
| [0010](0010-boost-lab-factory-pipeline.md) | Boost, lab & factory resource pipeline | IBEX-027 (wire), gap-analysis G-7 | Proposed |
| [0011](0011-spawn-orchestration.md) | Spawn orchestration & group spawning | review §6.11(b), gap G4 wishlist | Proposed |
| [0012](0012-market-and-risk.md) | Market & trade risk management | IBEX-018 | Proposed |
| [0013](0013-power-economy-and-power-creeps.md) | Power economy & power creeps | gap-analysis G-2, Increment 8 anchor | Proposed |
| [0014](0014-empire-strategy-and-posture.md) | Empire strategy & posture (executive layer) | completeness-critic gap #1 | Proposed |
| [0015](0015-testing-and-validation-strategy.md) | Testing & validation strategy (taxonomy L0–L6, seam contracts, fast-iteration policy) | IBEX-023, review §9; per-component plans in [`../plans/component-test-plans.md`](../plans/component-test-plans.md) | Proposed |
| [0016](0016-visualization-and-hud.md) | Visualization & HUD ("Glance HUD": exception-first edge rails, L0–L3 disclosure, wire-string flush) | IBEX-008/024, Field Report H, operator readability/CPU complaint; mockups in [`assets/`](assets/) | Proposed |
| [0017](0017-threat-aware-expansion-lifecycle.md) | Threat-aware expansion lifecycle (commit gate, no-abort claimer loop, reactive defense, unwired un-claim → contested-claim drain fix) | derelict-rooms work; supersedes implicit `claim.rs`/`colony.rs`/`war.rs` expansion behavior | Proposed |
| [0018](0018-source-keeper-room-exploitation.md) | Source Keeper room exploitation (coordinate detection + SK frontier; activate the dead `AttackReason::SourceKeeper`/`duo_sk_farmer`; persistent suppression farm; suppression-gated SK source+mineral mining) | combat-overhaul SK audit 2026-06-18; activates 0008a T-NPC-2 | Proposed |
| [0019](0019-combat-position-selection.md) | Unified combat position-selection: one objective-aware integer per-tile utility (flee/stand/close emerge as argmax) over two shared maps — integer threat field + multi-source reachability flood-fill — + 5 hard guards. Subsumes U8 kiting + T-POS attack-positioning. No WFV bump. | combat overhaul; 11-agent deep-dive 2026-06-19; generalizes 0008 §5 / 0008a T-POS | Accepted — **Stages 0–3 LANDED** (host-validated, live as default); Stage 4 = ongoing tuning |
| [0020](0020-ev-adaptive-blob-combat.md) | EV-driven, adaptive, blob-generalized combat: squad-level argmax-EV engage/target/sizing above 0019's positioning utility; arbitrary-N heterogeneous blobs (role-weighted EV, soft cohesion, role auction); anti-counterability (archetype + seeded mixed strategy over an offline meta-Nash); self-play tournament + exploiter ship-gate + adversarial room-gen. Keystone first slice = occlusion-aware threat/heal estimator. | combat overhaul; 8-agent ultracode research + adversarial critique 2026-06-19; extends 0008/0008a/0019 | Proposed (awaiting operator; keystone slice recommended) |

ADRs 0001–0006 are the rewrite's foundational pillars; **0007–0009** are the second design-pass deep-dives (hauling, combat/squad, room planning); **0010–0014** are the third (world-class) pass: the boost/spawn/market systems, power economy, and the empire executive layer. Each specializes the foundational pillars and cross-references rather than re-decides. Companion analysis notes (not ADRs): [`competitive-analysis-overmind.md`](competitive-analysis-overmind.md) (Ibex vs Overmind) and [`world-class-gap-analysis.md`](world-class-gap-analysis.md) (first-principles dominance target + the verified 18-row gap map seeding Increments 8/9). Engine ground truth lives in [`../references/engine-mechanics.md`](../references/engine-mechanics.md) (cites the cloned `C:\code\screeps-engine` / `screeps-common` / `screeps-driver`) — **check it before guessing mechanics**. Small-bug fix proposals: [`../plans/proposed-fixes.md`](../plans/proposed-fixes.md).

Constraints every ADR must respect: single-threaded WASM (no parallelism), per-tick CPU budget **including intents**, VM-reset resilience, and **incremental** migration (a stable seam, verifiable per step). Prior art for inspiration (not copying): see `../references/external-references.md`.
