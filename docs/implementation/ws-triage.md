# Phase 2 triage — decide, don't build

**Workstream:** Phase 2 · **Advances:** tracker §6/§8 closure · **Status:** near-complete — schedule-verdict sweep remains

## Resume point

Working through the §6/§8 items; verdicts land here as they're made, ADR amendments ride with them.
DONE: §8 do-nows, UNOWNED-3, amend-outs (0030 Withdrawn, 0025a closed, 0039→harness, 0020 kept-scheduled by operator), search_radius retune. REMAINING: the final schedule-verdict sweep — confirm every §6 line names its phase, then close this doc.

## Target

Every tracker §6 line inside a scheduled phase; §8 register emptied (each row wired, deleted, or
deliberately-kept-with-annotation); `#![allow(dead_code)]` removed so the compiler enforces it.

## Verdicts (the record)

| Item | Verdict | Rationale |
|---|---|---|
| `ui.rs` | **DELETE** | Never constructed; its doc claims consumers that don't exist. |
| `gameview.rs` | **DELETE** | 104-line consumer-less skeleton; its own growth rule ("add a method when a consumer is MIGRATED") was never exercised. Recreate with its first consumer (0015 testkit / 0006 Inc-6 recorder) — an empty trait anchors nothing. |
| `StructureIdentifier` (dead half) | **DELETE** | Superseded by the live `RemoteStructureIdentifier` in the same file. |
| `issue_virtual_anchor_flee` / `_movement` | **DELETE** | Superseded leftovers: REC-016 deliberately moved retreat to the kernel's threat-priced kite + heal triage (per-member), and the mission-advances/job-moves split consumes `virtual_anchor_target` directly. An anchor-level flee would re-introduce the formation-retreat model REC-016 removed. |
| `Job::describe` layer (~15 jobs) | **KEEP, annotated** | Cohesive, inert, and exactly the per-creep overlay feed ADR 0016's HUD consumes. Deleting 15 files of handlers to re-write them in Phase 6 is churn. Scheduled under 0016. |
| `damage.rs` readiness tranche | **KEEP, annotated → schedule** | Built + tested spawn-now-vs-wait and tower-engagement math; wiring it CHANGES live defense behavior — that's a combat-wave decision (review O3/R6/R7 adjacent), not a cleanup. Scheduled with the next combat wave under 0008a. |
| `BoostQueue` | **KEEP, annotated** | Owned by 0010, scheduled Phase 3 (WS-3). The plumbing is the part that exists; producer/consumer land there. |
| `HoldModel::Suppress` + duplicate SK ROI | **SCHEDULE → 0018 K-RECONCILE** | The unification (one room-economics kernel) is exactly K-RECONCILE's shape; not a deletion. |
| T1/T2 neighbour kernels | **(already ruled)** retained by design | See tracker RULING and war.rs:550. |
| `#![allow(dead_code)]` (UNOWNED-3) | **REMOVE + annotate survivors** | The compiler becomes the register. |

## Design deltas

- (running list — ADR amendments land with the amend-out verdicts)

## Verification

Suite + wasm clean after each tranche; deletions must not touch behavior (pure dead code);
annotations carry a reason + owner.

## Log

- 2026-08-23 — Do-now tranche + UNOWNED-3 DONE: deleted ui.rs, gameview.rs (with its self-test), StructureIdentifier dead half, both issue_virtual_anchor leftovers, 4 unused constants, 5 unused helpers; crate-wide allow(dead_code) REMOVED — 115 warnings triaged to 0 (deletes + owner-tagged keeps + 11 file-level 0016-layer allows + TEST-PINNED restores for 3 items the wasm check missed). FOUND WORK: the SystemData unused-fetch class (7 systems fetch storages they never read — per-tick waste, tagged FOLLOW-UP). Next: the amend-out ADR edits (0030/0020/0026a/0039/0025a).
- 2026-08-23 (later) — Operator ratified the amend-outs (0030/0025a/0039) and chose KEEP-scheduled
  for 0020 S5–S7 (after Phase 4); 0047 pulled forward to Phase 2.5. search_radius 1→2 shipped +
  live-reconciled. ADR edits landed (0030 banner, 0031 tempo line, 0025a closure section).
