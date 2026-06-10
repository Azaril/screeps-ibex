# docs/execution/

Execution plans that drive implementation — the bridge between the design corpus (`../design/`, `../plans/`) and code. One document per phase; tasks carry stable IDs (`P0.A1`, …) for tracking in commits and reviews.

| File | Purpose | Status |
|---|---|---|
| [`phase-0.md`](phase-0.md) | Baseline tooling (`screeps-server-kit` + `screeps-ibex-eval` after the A14 split), host test lane + pin tests, supplanted-code cleanup, critical fixes (proposed-fixes Group A). Precedes all ADR increment work. | **Complete** (exit audit §6) |
| [`phase-1.md`](phase-1.md) | Increment-0 remainder + Increment 1 → M0+M1: seg-57 metrics, colony-health score + gates, CpuGovernor + budgeted pathfinding facade, tick containment, scheduler seam at parity, intent sink/differ, design-settled riders. | Not started |

**Flow:** design corpus → execution plan → implementation commits referencing task IDs → baseline/regression reports land back here (e.g. `baseline-0-report.md`).
