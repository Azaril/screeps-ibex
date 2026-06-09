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

Constraints every ADR must respect: single-threaded WASM (no parallelism), per-tick CPU budget **including intents**, VM-reset resilience, and **incremental** migration (a stable seam, verifiable per step). Prior art for inspiration (not copying): see `../references/external-references.md`.
