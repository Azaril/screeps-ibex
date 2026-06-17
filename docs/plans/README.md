# docs/plans/

Project / rewrite plans produced from the review.

| File | Purpose |
|---|---|
| `rewrite-plan.template.md` | Skeleton for the **incremental** rewrite plan. Copy to `rewrite-plan.md` and fill from the review report (`../reviews/`) + pillar ADRs (`../design/`). |
| `rewrite-plan.md` | The filled incremental rewrite plan (Increments 0–9). |
| `proposed-fixes.md` | Small-bug fix proposals backlog. |
| `component-test-plans.md` | Per-component test plans (the *what*, against ADR 0015's *how*). |
| `combat-overhaul-plan.md` | **Combat squad overhaul** — integrated harness-first → behavior backlog (ADR 0006 + 0008; cross-cuts 0003/0011/0015/0014). |

The rewrite is **incremental & confidence-driven** (strangler-fig): each increment sits behind a stable seam and is verified before the next. Back-compat is not required — serialized state may be dropped per step.
