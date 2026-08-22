# Ship WFV 28 — implementation

**Workstream:** WS-1 · **Advances:** ADR 0046, ADR 0038, ADR 0017 · **Status:** blocked

## Resume point

**Blocked on one elevated command, waiting on the operator being at the machine (expected
2026-08-23).**

`master` (WFV 28) has never executed against any world. The private soak is the next
step and cannot start because `com.docker.service` is Stopped/Manual and starting it needs
Administrator.

**Literal next action, from an elevated PowerShell:**

```powershell
Start-Service com.docker.service
Set-Service com.docker.service -StartupType Automatic
```

Then confirm the engine actually attached — `docker ps` must **return**, not hang — and run the
soak from [`../execution/adr-0046-private-soak-plan.md`](../execution/adr-0046-private-soak-plan.md),
which is the full runbook (commands, criteria C1–C5, failure signatures, rollback). Do not
duplicate its content here.

> Diagnostic, so it is not re-derived: `docker ps` **hanging** rather than erroring, while Docker
> Desktop, all 25 named pipes and the `docker-desktop` WSL distro all look healthy, means the
> service is down. Restarting Docker Desktop, `wsl --shutdown`, and booting the distro by hand were
> all tried and all fail — none can start a privileged service without UAC.

## Target

WFV 28 soaked on the private server against ADR 0046 §5's success criteria, then a deliberate
decision on the MMO reset. The scout redesign, the expansion-stall fixes (Wave 1 + L1 + L3) and the
zombie-war-mission fix all reach a world for the first time.

## Plan

- [x] Merge ADR 0046 to master, gated (320 tests, wasm clean, determinism fence)
- [x] Re-establish the self-pin regression pin lost to the merge (`entry_needs_service`, 2 RED-verified tests)
- [x] Write the soak runbook
- [ ] **Unblock Docker** (see Resume point)
- [ ] Private deploy → first-tick health gate (no panics, no deser errors, bucket recovering)
- [ ] Judge C1–C5; record verdicts with supporting console/decoder lines
- [ ] Decide the MMO reset — **held by default**; live is healthy at 7 rooms, nothing forces it
- [ ] L2 poison-list self-heal — ships **last**; it is load-bearing for `scouting_coverage_complete` until the list is healthy

## Design deltas

- **ADR 0046 supersedes the fulfillment half of ADR 0021.** 0021's follow-ups #1 (re-scout
  scheduler) and #2 (observer preference) are absorbed. Re-head 0021 once this deploys.
- **The self-pin guarantee moved.** 0046 deletes `best_unclaimed_for`; the invariant now lives in
  `entry_needs_service(intel_age, want_fresh_within)` in `room/scoutassignment.rs`. If that
  predicate is ever loosened to `>=`, the 103-room poison list can return. Recorded in the ADR.
- No other design change. The merge took 0046's `want_fresh_within` mechanism over L1a's
  producer-side guard by design, not by accident — 0046's tour planner needs the entry present in
  the demand set to reason about it.

## Verification

- Private soak clean against ADR 0046 §5 (C1 no re-poisoning · C2 stale-intel skip gone from Select
  · C3 scouts tour and fleet tracks demand · C4 no self-pin · C5 a claim actually fires)
- Before the MMO step only: review the default-ON flag set, which has never run at WFV 28 —
  `military.offense`, `source_keeper.farming`, `derelict.declaim`, `derelict.breach_sealed`,
  `claim.on` (rapid-spread, cap 4), `visualize.on`
- Rollback: `wfv27-deployable-e857c76` is the last no-reset point

## Log

- 2026-08-22 — 0046 merged and gated; soak plan written; Docker blocker diagnosed to
  `com.docker.service`; MMO deploy deliberately held after the live check showed 7 healthy rooms
  (the stall had partially self-resolved on unfixed code, removing the urgency argument).
