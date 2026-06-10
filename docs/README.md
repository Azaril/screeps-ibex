# docs/

Human-readable, generated, and curated project artifacts — kept out of the workspace root and separate from build output (`dist/`, `pkg/`, `output/`, `target/`).

## Convention for generated/task artifacts

Outputs produced by reviews, analyses, planning, and agent/workflow tasks live here under a type-named subfolder, **not** at the repo root:

| Folder | Contents |
|---|---|
| `docs/reviews/` | Review-kickoff prompts and review reports (e.g. `ibex-review-prompt.md`). |
| `docs/plans/` | Project / rewrite plans produced from reviews. |
| `docs/design/` | ADRs and design notes (entity model, serialization, behavior modeling, CPU governance). |
| `docs/references/` | External references & prior art (Overmind, engine source, docs, telemetry) and engine-mechanics ground truth. |
| `docs/execution/` | Execution plans driving implementation (Phase 0 baseline → increments), with stable task IDs and baseline reports. |

Add new subfolders by artifact type as needed. Hand-maintained top-level docs (`AGENTS.md`, `todo.md`) stay where they are.
