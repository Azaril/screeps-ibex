# docs/execution/

Execution plans that drive implementation — the bridge between the design corpus (`../design/`, `../plans/`) and code. One document per phase; tasks carry stable IDs (`P0.A1`, …) for tracking in commits and reviews.

| File | Purpose | Status |
|---|---|---|
| [`phase-0.md`](phase-0.md) | Baseline tooling (`screeps-server-kit` + `screeps-ibex-eval` after the A14 split), host test lane + pin tests, supplanted-code cleanup, critical fixes (proposed-fixes Group A). Precedes all ADR increment work. | **Complete** (exit audit §6) |
| [`phase-1.md`](phase-1.md) | Increment-0 remainder + Increment 1 → M0+M1: seg-57 metrics, colony-health score + gates, CpuGovernor + budgeted pathfinding facade, tick containment, scheduler seam at parity, intent sink/differ, design-settled riders. | **Complete** (exit audit §2.7; operator sign-off 2026-06-12) |
| [`phase-2.md`](phase-2.md) | Combat-Effective (Inc 3–5 + combat-sim addition): combat micro-sim harness, SquadStore/SquadId, the anchor mover (footprint pathfind + orientation), CombatObjectiveQueue + SquadManager + tactics, war supervision + escort + thin posture hook, synchronized spawning → M4. Checkpoints CP-H/I/M/G/W/S are the resume points. **⚠ Now the cold-resume historical log** — forward-looking status moved to the master doc below. | In progress (G1–G4/O6, K0–K5, M1–M3, H1–H3 landed) |
| [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) | **★ Combat master status & plan — the single source of truth for combat/war status + remaining work.** Repurposed 2026-06-18 from the old stale overview. Per-workstream status (supersedes phase-2.md Status columns), the ordered remaining-work plan, legacy delete-tracking, WFV ledger, operator-settled constraints, checkpoints. Design rationale stays in ADR 0008 (+0008a). | Living |

**Flow:** design corpus → execution plan → implementation commits referencing task IDs → baseline/regression reports land back here (e.g. `baseline-0-report.md`).
