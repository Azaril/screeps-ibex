# docs/execution/

Execution plans that drive implementation — the bridge between the design corpus (`../design/`, `../plans/`) and code. One document per phase; tasks carry stable IDs (`P0.A1`, …) for tracking in commits and reviews.

| File | Purpose | Status |
|---|---|---|
| [`phase-0.md`](phase-0.md) | Baseline tooling (`screeps-server-kit` + `screeps-ibex-eval` after the A14 split), host test lane + pin tests, supplanted-code cleanup, critical fixes (proposed-fixes Group A). Precedes all ADR increment work. | **Complete** (exit audit §6) |
| [`phase-1.md`](phase-1.md) | Increment-0 remainder + Increment 1 → M0+M1: seg-57 metrics, colony-health score + gates, CpuGovernor + budgeted pathfinding facade, tick containment, scheduler seam at parity, intent sink/differ, design-settled riders. | **Complete** (exit audit §2.7; operator sign-off 2026-06-12) |
| [`phase-2.md`](phase-2.md) | Combat-Effective (Inc 3–5 + combat-sim addition): combat micro-sim harness, SquadStore/SquadId, the anchor mover (footprint pathfind + orientation), CombatObjectiveQueue + SquadManager + tactics, war supervision + escort + thin posture hook, synchronized spawning → M4. Checkpoints CP-H/I/M/G/W/S are the resume points. **⚠ Now the cold-resume historical log** — forward-looking status moved to the master doc below. | In progress (G1–G4/O6, K0–K5, M1–M3, H1–H3 landed) |
| [`implementation-tracker.md`](implementation-tracker.md) | **★ THE living master status doc — start here.** Active workstream, blockers, ordered next queue, deployment ledger, per-ADR open work, unowned cross-cutting items, the dead-code register, and the standing rulings. Deliberately minimal: it tracks *status*, never design — detail stays in the ADRs. Carries its own trim rules. | **Living** |
| [`project-reconciliation-2026-08-22.md`](project-reconciliation-2026-08-22.md) | Dated snapshot + method behind the tracker: how the 2026-08-22 reconciliation was done, how the ADR 0046 merge resolved, and what was closed out. History, not status. | Complete |
| [`adr-0046-private-soak-plan.md`](adr-0046-private-soak-plan.md) | The runbook for the one workstream currently in flight — preconditions (incl. the Docker blocker), deploy commands, success criteria, failure signatures, rollback. | Active |
| [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) | **⚠ HISTORICAL — no longer the source of truth** (self-demoted 2026-07-01; forward status moved to the tracker above 2026-08-22). Frozen at the 2026-06-19 / WFV-14 era. Useful for: the §0a verified-still-true residue and landed-task history. Note several of its "open" items are verified CLOSED — see the tracker §9. | Historical |

**Flow:** design corpus → execution plan → implementation commits referencing task IDs → baseline/regression reports land back here (e.g. `baseline-0-report.md`).
