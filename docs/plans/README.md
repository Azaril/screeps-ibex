# docs/plans/

Project / rewrite plans produced from the review.

| File | Purpose |
|---|---|
| `rewrite-plan.template.md` | Skeleton for the **incremental** rewrite plan. Copy to `rewrite-plan.md` and fill from the review report (`../reviews/`) + pillar ADRs (`../design/`). |

The rewrite is **incremental & confidence-driven** (strangler-fig): each increment sits behind a stable seam and is verified before the next. Back-compat is not required — serialized state may be dropped per step.
