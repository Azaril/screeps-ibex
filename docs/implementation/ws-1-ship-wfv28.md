# Ship WFV 28 — implementation

**Workstream:** WS-1 · **Advances:** ADR 0046, ADR 0038, ADR 0017 · **Status:** active — observing live

## Resume point

**WFV 28 is LIVE ON MMO** (deployed 2026-08-22 ~22:44 UTC, wasm sha `d9b748497e4a`, from code
identical to the gated state plus the `reset.features` flag `77dc9cc`). The loud reset fired
cleanly: new VM (`vm_starts` 2747→2748), zero panics/deser errors across two console-tail windows,
room re-planning ran, CPU settled 52→37/140 as the rebuild completed, bucket pinned at 10000.

**The `reset.features` one-shot fired and verified**: `Memory._features` was rebuilt from compiled
defaults — `military.offense` flipped back to `true` (had been manually `false` since the July
drain era; Wave A's drain fixes are in this artifact), `remote_mine.search_radius` now present,
flag self-cleared. **Live config == compiled defaults everywhere.**

**Next action: observe one discover cycle (~840–5000 ticks) and judge C1–C5 live.**

```bash
# console tail (needs SCREEPS_TOKEN in env — read it from .screeps.yaml mmo.token)
cargo run --release --manifest-path screeps-rest-api/Cargo.toml --example tail -- --shard shardX --grep ClaimOp
# offline world decoder for C1 (unreachable list): fetch segs 50–52 same-tick, then
IBEX_WORLD_PAYLOAD=<file> IBEX_NOW=<tick> cargo test -p screeps-ibex decode_live_world -- --ignored --nocapture
```

> Plan change (operator, 2026-08-22 evening): private-soak-first was inverted to **MMO-first**
> because B-1 (Docker service, needs elevation) blocked the private lane until the operator is at
> the machine. The MMO deploy was pre-authorized (2026-08-11 ledger). The private server is still
> wanted once B-1 clears — for the harness lane (H5 etc.), not for this deploy.

## Target

WFV 28 verified live against ADR 0046 §5's success criteria; the scout redesign, expansion-stall
fixes (Wave 1 + L1 + L3) and zombie-war-mission fix all running on MMO. Then L2 closes the
expansion program.

## Plan

- [x] Merge ADR 0046 to master, gated (320 tests, wasm clean, determinism fence)
- [x] Re-establish the self-pin regression pin (`entry_needs_service`, 2 RED-verified tests)
- [x] Write the soak runbook (criteria reused for live observation)
- [x] `reset.features` one-shot flag (`77dc9cc`) — the UNOWNED-7 fix, pattern for future retunes
- [x] **MMO deploy** (loud reset WFV 27→28) + first-tick health gate — clean
- [x] Reconcile `Memory._features` to compiled defaults — verified on live
- [ ] Observe one discover cycle; judge C1–C5 — **interim @ tick 4,878,785 (world decoder): C1 PASS (unreachable=1, attempts=1, vs 103 pre-redesign) · C2 PASS (skip absent from the captured Select) · C3 PASS (visibility queue all age-0, unknowns 23 and falling, opportunistic explore active) · C4 PASS (all-age-0 queue = scouts moving, not welded) · C5 pending (0 claim missions; 15 scored candidates, 4 plan-VALID — prefetch working; Select imminent)**
- [ ] Watch-items while offense is freshly re-enabled: `[Lifecycle] RETIRE reason=GaveUp` clusters,
      spawn-queue combat churn, CPU/bucket trend (Wave A's fixes are in this artifact — the July
      drain signature should NOT reappear; if it does, `Memory._features.military.offense = false`
      is the live off-switch, no redeploy)
- [ ] `search_radius` retune (UNOWNED-4) once scouting is demonstrably healthy — a deliberate
      change, made **through** the new reconcile pattern, not a Memory hand-edit
- [ ] L2 poison-list self-heal — ships **last** (load-bearing for `scouting_coverage_complete`)
- [ ] When B-1 clears: private server back up for the harness lane (H5 etc.)

## Design deltas

- **ADR 0046 supersedes the fulfillment half of ADR 0021** (recorded in both; re-head 0021 when
  this closes).
- **The self-pin guarantee lives in `entry_needs_service`** (`room/scoutassignment.rs`); loosening
  it to `>=` re-opens the 103-room poison-list class. Recorded in the ADR.
- **`reset.features` added to the one-shot reset family** (`77dc9cc`): `features::load()` writes
  the full resolved struct back every tick, so a stale persisted copy shadows any retuned compiled
  default forever — the reconcile deletes the tree and lets the existing default+write-back
  machinery rebuild it. This is now the standing answer to UNOWNED-7. Live-proven 2026-08-22.

## Verification record

| Check | Result |
|---|---|
| Deploy | `d9b748497e4a`, 1.72 MiB / 5 MiB code limit (34%) |
| Loud reset | clean — new VM 2748, re-planning observed, no deser spam |
| Panics | 0 across two tail windows |
| CPU / bucket | 52→37/140 settling; bucket 10000 flat |
| Features parity | `offense: true`, `search_radius` present, flag self-cleared |
| Baseline to beat | 7 rooms, GCL 12 (pre-deploy 2026-08-22); old stall signature: 11 candidates, 0 missions |

## Log

- 2026-08-23 (cron #6, window close) — 3-hour watch COMPLETE, all 6 checks clean: 0 panics, 0 deser errors, 0 combat-drain signatures across the whole window (offense re-enabled throughout). Final decoder read: phase=Scouting live, 18 candidates (leader W7N47 dist=4 score 0.835 — the far-sprawl target class), plans prefetched, unknowns 23→18, unreachable steady at 1 bounded entry. C5 still pending but imminent. CPU spike to 123 in check 6 = the plan prefetcher grinding W16N52, budget-governed, bucket absorbed (9529→recovering). FOUND WORK (watch item): 3× `create_construction_site … Extension … InvalidTarget` from foreman during the RCL rebuild — transient-looking; recheck next session, escalate to a 0009 item if persistent.

- 2026-08-23 (cron #5) — world-decoder read @ 4,878,785: C1/C2/C3/C4 interim PASS, C5 pending (candidates scored + plans prefetched, no commit yet). Checks 1–5 all clean: 0 panics, 0 drain signatures, bucket pinned 10000.

- 2026-08-22 (late) — **Deployed WFV 28 to MMO** (operator inverted soak order; Docker blocked).
  Health clean. `reset.features` built, shipped, fired, verified — live config at compiled-default
  parity. Observation window open.
- 2026-08-22 — 0046 merged and gated; soak plan written; Docker blocker diagnosed
  (`com.docker.service`); MMO initially held after the 7-room live check.
