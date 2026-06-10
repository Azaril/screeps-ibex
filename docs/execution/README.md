# docs/execution/

Execution plans that drive implementation — the bridge between the design corpus (`../design/`, `../plans/`) and code. One document per phase; tasks carry stable IDs (`P0.A1`, …) for tracking in commits and reviews.

| File | Purpose | Status |
|---|---|---|
| [`phase-0.md`](phase-0.md) | Baseline tooling (`screeps-server-kit` + `screeps-ibex-eval` after the A14 split), host test lane + pin tests, supplanted-code cleanup, critical fixes (proposed-fixes Group A). Precedes all ADR increment work. | Active |

**Flow:** design corpus → execution plan → implementation commits referencing task IDs → baseline/regression reports land back here (e.g. `baseline-0-report.md`).
