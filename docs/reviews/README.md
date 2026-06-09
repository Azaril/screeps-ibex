# docs/reviews/

Review-kickoff prompts and the reports they produce.

| File | Purpose |
|---|---|
| `ibex-review-prompt.md` | The Ultracode review-kickoff prompt (**input**). Launch the multi-agent review by pointing at it with `ultracode`. |
| `ibex-review-report.template.md` | Skeleton for the review's **output**, matching the prompt's §9 deliverables. Copy to `ibex-review-report.md` (or `ibex-review-report-YYYY-MM-DD.md`) and fill in. |

**Flow:** prompt → run review → report → feeds `../plans/` (incremental rewrite plan) and `../design/` (pillar ADRs).
