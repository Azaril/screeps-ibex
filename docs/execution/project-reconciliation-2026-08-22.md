# Project reconciliation — 2026-08-22

> **Purpose.** A cold-resume snapshot after a pause: what is **DONE** (and where it is deployed),
> what is **IN-FLIGHT** (uncommitted, unmerged, un-accepted, or un-deployed), what does **not
> reconcile**, and the recommended order to pick back up. Every claim below is evidence-backed
> (git SHA, file:line, ADR status line, or session-log record) — where evidence was absent or
> contradictory, it is called out rather than smoothed over.
>
> **Method.** Git ground truth (branches / worktrees / working tree / submodules) → docs narrative
> (`docs/execution`, `docs/plans`, all 57 ADRs in `docs/design`, `docs/reviews`) → the project
> memory ledger → the 13 prior Claude Code session transcripts at
> `C:\Users\willi\.claude\projects\C--code-screeps-ibex\*.jsonl` (all readable; none lost).

---

> ## ✅ CLOSEOUT — all in-flight work was tied off the same day (2026-08-22)
>
> This document was written as a diagnosis, and the operator then closed out everything it found.
> **The repo is now in the clean state §5 recommended.** What changed, in order:
>
> 1. **Working tree emptied.** The stale rover-eval scenario work was committed
>    (`c4b3d17` in the submodule, pointer bump `bd569b7`). No uncommitted files, no stashes.
> 2. **ADR 0046 merged.** Rebased onto `e857c76`, both conflicts resolved (see §3.2 for how),
>    gated, and fast-forwarded onto master. **Master is now WFV 28.**
> 3. **All gates run and green:** 320 lib tests (318 + 2 new pins), wasm check + wasm clippy clean,
>    and — for the first time on this branch — the **`sim_is_deterministic_over_rounds`
>    determinism fence passes**.
> 4. **The self-pin regression pin was reconstructed.** 0046 deleted `best_unclaimed_for`, which
>    took L1's regression test with it. The guarantee now lives in a named pure kernel,
>    `entry_needs_service`, pinned by two RED-verified tests (§3.2).
> 5. **Everything pushed to `origin`** — master plus 49 commits across 8 submodules, ending a
>    ~7-week local-only backlog.
> 6. **Doc hygiene done:** stale status headers on ADR 0038 and ADR 0042 corrected, the duplicate
>    ADR 0044 renumbered to **0044a**, all worktrees and merged branches removed.
>
> **The one thing deliberately NOT done: no deploy.** The operator scoped this tie-off to the repo.
> Nothing has shipped since `ab692bd` (2026-07-28), and master now carries a WFV 27→28 loud reset
> whenever you do deploy. The tag **`wfv27-deployable-e857c76`** preserves the last no-reset
> deployable point if you still want the cheap observe-one-discover-cycle path first.
>
> The body below is the analysis of record, kept as written. Where a finding is now resolved it is
> marked **[RESOLVED 2026-08-22]** rather than deleted, so the reasoning survives.

## 0. The 30-second answer

Three workstreams were open when this was written:

1. **Expansion stall (Aug 11–12).** Root-caused, fixed, committed on `master` — **but never
   deployed anywhere.** The last session ended by asking a direct question that never got an
   answer. *(Still undeployed by choice; the question was answered 2026-08-22 — merge 0046.)*
2. **ADR 0046 (scout redesign).** ~1500 lines, fully implemented, 318 tests green — sitting
   **unmerged in a git worktree branch**, un-soaked, un-fence-tested, carrying a WFV 27→28 loud
   reset. **[RESOLVED 2026-08-22 — merged, fence now passes.]**
3. **Combat review Tier −1.** Wave A shipped and is live-verified on MMO; **Wave B was never
   started** (7 of the 9 recommended Tier −1 defects remain, plus new D28). *(Still open — this is
   now the top code workstream.)*

Everything else — ADR 0040 (accepted, WFV-27 live on MMO), ADR 0042/0043/0044 economy work, the
G4 offense migration, the combat Wave A drain fix — is genuinely finished and deployed.

---

## 1. Deployment ledger — what is actually running

| Environment | Last shipped artifact | Date | Evidence |
|---|---|---|---|
| **Live MMO (shardX)** | `ab692bd` + decision submodule `f6c084a` — WFV 27 + combat Wave A | **2026-07-28** | `b269682` §7.2a; session `36c4583d` final message ("CPU 16/130 with a full, recovering bucket") |
| **Docker private server** | same Wave A build (private-soaked ~7k ticks before the MMO push) | 2026-07-28 | `b269682` §7.2a |
| **`master` HEAD** | `f81c415` (was `e857c76` when written) | 2026-08-22 | 6 commits ahead of the deployed artifact, now including ADR 0046 |
| **`origin/master`** | in sync with local master | 2026-08-22 | **[RESOLVED]** was `ce7069e`, 119 commits behind |

**`WORLD_FORMAT_VERSION` is now 28** (ADR 0046 merge). It was 27 when this was written, and
`09c36db`/`e857c76` did not bump it — so if you want a no-reset deploy of just the expansion fix,
deploy the tag **`wfv27-deployable-e857c76`**, not master.

**Submodules:** all unpushed work is now published — 49 commits across combat-decision (15),
combat-eval (12), sim-core (7), combat-agent (5), rover (4), rover-eval (4), combat-engine (1) and
rest-api (1). foreman was already clean. Every superproject pointer is current.

---

## 2. DONE

### 2.1 Deployed to live MMO **and** private

- **ADR 0040 — the unified energy market (e/t), economy decision seam, offline economy sim.**
  Accepted `536e922` (2026-07-06, EP-10.7); **WFV-27 deployed to MMO** `f772ecf` (same day,
  wasm sha `bbe1bcdff09c`, health-check clean). Numeric-bid tickets, bid-native selection,
  opportunity-floor repair admission, spawn bids.
- **ADR 0042 — R_net forming pricing.** P0 implemented + private-validated 2026-07-08
  (`d3367bb`, `7c1401a`, `91a8503`, `7fd12d0`). Rode to MMO inside the Wave A build.
  *(Its status header said "MMO deploy pending operator final review" — corrected 2026-08-22.)*
- **ADR 0043 — band-normalization ledger.** A3/A5/A6 + the A1 refill lane wired live
  (`a73ee0b`, `2ab94ad`, `2062a62`, `8df0974`).
- **ADR 0044 (transfer market = min-cost flow).** Accepted `2a43764`; P0 two-stage reduced-cost
  admission, P1 live-spawn-queue refill pricing, P2 true-routed-distance haul (live + sim
  unified), the Family-R multi-room econ sim, the realistic-terrain generator shared into
  sim-core, and the A3 Architecture-1 live EV consumer pricing + Use-lane admission (`26f8312`,
  merged with the hauler move+deposit concurrency win `a3d673c`). All of this is in the deployed
  build.
- **Combat review Wave A — the live-MMO "creeps sitting near spawn" drain.** Root cause
  `33742e8`, fix `ab692bd` (+ decision `f6c084a`), verification `b269682` §7.2a. Closes
  **D24** (frozen-anchor deploy deadlock), **D25** (zombie claimed objectives), **D26**
  (threat-blind rally staging), **D27** (intercept over-breadth), **D11** (`HostileBehavior::Deny`
  routing stall), **R22** (never-departed circuit breaker) and structurally **D1**.
  Live-verified: CPU 87→16 of 130, bucket full and recovering, zombie objectives expired instead
  of re-fielding, rally staging moved out of the W11N54 fortress, `[SpawnQueue]` churn stopped.

### 2.2 Committed on `master`, **not** in the deployed artifact

- **`e480ed9` — kill zombie war missions on lost rooms** (WallRepair/SafeMode/NukeDefense gated on
  `is_valid_home_room`, ADR 0017 ownership-subordinate). Committed 21:57 on 2026-07-28, i.e.
  **after** the `ab692bd` artifact that was deployed at 20:31. Its memory ledger records
  deploy + tail-verify as pending/permission-blocked. No serialized-shape change — the existing
  MMO zombies would self-abort on the first tick after a deploy. **Treat as undeployed.**
- **`09c36db` — expansion-stall Wave 1** (route-pricing parity incl. the REC-024 mover-parity
  mirror, rolling commit during Scouting, coverage-gate simplification, plan prefetch,
  claimer-death TTL guard, RCL2 discovery-freeze fix, home-consumption cap, `search_radius` knob).
  ibex lib 315 green, wasm builds.
- **`e857c76` — the actual expansion-stall root cause (L1) + the below-ring commit window (L3).**
  Scouts self-pinned in the room they were standing in and never issued a move intent (three
  interacting sites: `operations/claim.rs` unconditional CRITICAL re-assert, `best_unclaimed_for`
  with no current-room exclusion, `tick_move_to_room_with_bid` reporting "arrived" on room
  membership). That fixed point minted the 103-room poison list — and being pure control flow it
  **survives a world reset**, which is why a WFV reset alone would have re-manufactured it.
  ibex lib 316 green, wasm builds, no WFV bump.

### 2.3 Design / analysis artifacts completed

- **Combat-systems review** `b85b489` (2026-07-09) — five deep-read passes + adversarial
  verification; 28 defects (D1–D28), 22 risks, 8 opportunities, a WFV inventory, a verified-clean
  do-not-reflag list, and the **Tier −1** sequencing amendment to ADR 0008a. Extended twice with
  the live-MMO root-cause addendum (§7, §7.2a).
- **ADR 0008a readiness review** `a500309` (2026-06-29) — per-tactic readiness matrix + Tier 0–3
  build order + the boost layer identified as the foundational enabler.
- **ADR 0045 — autonomous power-creep operators** `7bca5b1` (2026-07-28). **Design only, Proposed,
  zero runtime code** (confirmed: the only power file in the tree is `missions/powerspawn.rs`,
  which predates it; `power_policy.rs` is planned, not written).
- **Expansion-stall diagnosis** `docs/reviews/expansion-stall-2026-08-11.md` — M1–M5 all
  adversarially verified and live-confirmed against a captured Select cycle, plus two new tools:
  the offline world decoder (`operations::claim::live_world_decode`) and the generalized console
  tail (`screeps-rest-api/examples/tail.rs`).
- **ADR 0033 rover pathing sim** — complete through M5 slice 7 + the combat friendly-layer;
  H = 0.963.
- **ADR 0046 doc** — written (`master` copy Proposed), then implemented and re-written in a
  worktree (see §3.2).

---

## 3. IN-FLIGHT

### 3.1 Uncommitted working-tree work — **[RESOLVED 2026-08-22]**

| What | State |
|---|---|
| `screeps-rover-eval/src/scenario.rs` (+63 lines) — `generate_realistic(seed)` + the `realistic_rooms_stay_near_optimal` 30-seed sweep test | Was **uncommitted in the submodule** (pointer showed `67a3e777…-dirty`) — the rover-eval half of the ADR 0044 cross-sim realistic-terrain workstream (the sim-core side shipped as `db32580` / `eab74c6`). Verified compiling, then **committed as `c4b3d17`** with the superproject pointer bumped in `bd569b7`. It is the natural entry point for the R19 chokepoint re-tune. |

Nothing else was uncommitted; there were no untracked files and no stashes.

### 3.2 Unmerged branches / worktrees — **[RESOLVED 2026-08-22]**

> **How the ADR 0046 merge actually went.** `git merge-tree` predicted it exactly: commit 1
> (`888e3db`) rebased clean, and **only commit 2 conflicted, in exactly two files, two hunks total.**
>
> - **`visibilitysystem.rs` — took 0046 wholesale.** It deletes `best_unclaimed_for`,
>   `best_unclaimed_for_at` and `has_unclaimed_scout_eligible` outright; the greedy per-creep picker
>   is replaced by the global `build_tours` pass. After resolution the only surviving reference to
>   `best_unclaimed_for` anywhere in the tree is a history comment. No dangling callers.
> - **`claim.rs` — the one genuine semantic conflict.** Both sides fix the same bug by different
>   means: L1a guards at the **producer** (skip re-asserting CRITICAL for an already-resolved room),
>   0046 filters at the **assigner** (producers re-assert idempotently declaring
>   `want_fresh_within`; the assigner derives service state from intel age). Took 0046's mechanism —
>   its tour planner needs the entry *present* in the demand set to reason about it, so removing
>   entries at the producer would blind it — but kept L1's diagnostic reasoning in the comment,
>   because that explains *why* the freshness filter is load-bearing and would otherwise be lost.
> - **L3 auto-merged**, as predicted: 0046's claim.rs edits are confined to lines ~351–425 plus a
>   test module, and `run_select` sits at 655.
>
> **The one real loss, and its replacement.** Deleting `best_unclaimed_for` also deleted
> `best_unclaimed_never_returns_the_room_the_scout_is_standing_in` — the pin proving the self-pin
> fixed point cannot return. That guarantee was re-established as a named pure kernel,
> `entry_needs_service(intel_age, want_fresh_within)`, used by the demand filter: a scout standing
> in a room sees it, so `intel_age` is 0, and 0 is never `>` any `want_fresh_within` — including an
> imperative 0. Pinned by `occupied_room_never_needs_service` (sweeping the window across 0, 1, 100,
> 250, default TTL, `u32::MAX`) and `imperative_entry_still_excludes_the_occupied_room`. **Both
> RED-verified:** flipping the predicate to `>=` re-admits the occupied room and fails exactly those
> two tests and nothing else. Landed in `f81c415`.
>
> **Adversarial diff review findings** (on the merged result): WFV 28 bump present with the
> `EXPECTED_WORLD_FORMAT_VERSION` mirror-assert in `claim.rs` intact (review resolution #10 holds);
> `ScoutAssignments.tours` is fully replaced each pass and `entry_fail` is `retain`-pruned against
> the live demand set, so neither accumulates dead entity keys — the dangling-ref class does not
> apply, and nothing here is serialized. One minor finding fixed (`demand.sort_by` →
> `sort_by_key` on a determinism-critical ordering). One benign note: `last_observed` is never
> pruned, but it is keyed by room, bounded by observer range, and ephemeral across resets.

| Branch | Ahead of master | Contents |
|---|---|---|
| **`worktree-agent-abc0e57ea9f15e4d8`** | **3 commits** | **ADR 0046 implementation.** `888e3db` (Operation creep-roster + spawn-queue seams, D4), `3647824` (scout assignment post-pass + multi-room tours + EV fleet sizing, **WFV 27→28**), `0cadb44` (ADR doc: the 11 binding design-review resolutions). 17 files, +1738/−1356; new `room/scoutassignment.rs` (~1080 lines); **deletes** the per-creep claim layer, `MissionData::Scout`, and `ScoutState::Idle`. 318 lib tests green, wasm clean. **NOT merged, NOT determinism-fence-tested, NOT soaked, NOT deployed.** Branched from `09c36db`, so it is 1 behind master and **will conflict with `e857c76`**: it deletes `best_unclaimed_for` (superseding L1b) and rewrites the claim.rs producer declarations (superseding L1a via `want_fresh_within`); L3's `run_select` should merge cleanly. |
| `worktree-agent-a365489048f5b1403` | 0 (merged) | Hauler concurrency — already in master via `26f8312`. Worktree has stray dirty submodule pointers; safe to delete. |
| `worktree-agent-afb8dd11f694e32b3` | 0 (merged) | ADR 0044 A3 Arch 1 — already in master. Safe to delete. |
| `blissful-shaw-165bc1` (detached `e480ed9`) | 0 | Zombie-war-mission fix — already on master. Safe to delete. |
| `new-room-planning` | **0** (fully merged) | Local tip `21e1f2f` *is* the merge-base with master — it is an ancestor, nothing unique on it. Safe to delete. (The `git branch -vv` "306 ahead / 305 behind" is relative to `origin/new-room-planning`, not master — see §4.9.) |
| `docs/review-planning` | **0** (fully merged) | Ancestor of master, local and remote. Safe to delete. |

### 3.3 Proposed / accepted-but-unimplemented ADRs

**Recently active — genuinely awaiting a decision or a build:**

| ADR | Status | Gap |
|---|---|---|
| **0046** scout assignment + fleet EV | **[RESOLVED]** Accepted + implemented, merged to master `f81c415` | Was Proposed on master / Accepted in the worktree. Merged 2026-08-22; master is WFV 28. Deploy (and its loud reset) is still pending. |
| **0045** power-creep operators | Proposed | Design complete, no code. Awaiting acceptance. |
| **0041** combat boost layer | **Accepted**, implementation pending (P0 dark-first) | Zero code — `military/boostqueue.rs` predates it (Feb) and is unrelated. Blocks ADR 0008a's boosted-assault frontier (T-COMP-1/5, T-TOWER-3, T-NPC-7) and combat-review R1 (enemy-boost blindness, the "top MMO risk"). |
| **0039** real-stack self-play sim | Proposed | Paused (per memory: the border-oscillation motivation turned out to be bot formation expel; rover exonerated). |
| **0023a** staged combat harness | Proposed | Annex to 0023; not started. |
| **0008a** combat tactics catalog | Proposed | Tier 0–3 build order written, **nothing from Tier 0 built** (T-HEAL-3, T-BREACH-3). |
| **0044 A3** all-sinks-EV scoping | "Decision-ready (scoping)" | Architecture 1 landed (`9dfdecb`); the rest of the scope is undecided. |

**Standing design backlog (Proposed, no recent activity, not blocking):** 0004, 0005, 0006, 0010,
0011, 0012, 0013, 0014, 0015, 0016, 0017, 0018, 0030. Several are marked "IN SCOPE this completion
pass" by the ultracode completion kickoff (0010, 0015, 0016) — that program has not been run.

### 3.4 Deferred checklist items

- **G4 offense (`docs/plans/combat-overhaul-plan.md` §5 — the doc referred to elsewhere as the
  "g4-offense-plan"; no file by that name exists):** O1 / O2 / O3 / O4 / **O6.1** / **O6.2** / O7
  are all **DONE** — O6.1 `ffbc032` (InvaderCore→Dismantle) and O6.2 (`AttackFlag`→Secure,
  `ResourceDenial`→Harass, `InvaderCreeps` reconciled into remote-defense `Defend`, unified war
  cap) both landed and deployed to Docker on 2026-06-18, and **O7 deleted the entire legacy**
  (`AttackMission` / `AttackOperation` / `AttackReason`, WFV 12→13). **O5 (power-bank as
  `Farm{PowerBank}`) was dropped, not deferred** — its haul lived only in code O7 deleted.
  **G4-HEAVY** (the multi-squad drain + quad player assault) is a deferred future capability
  needing multi-squad objective sequencing plus operator demand for PvP.
  ⚠ The phase-2.md §2 RESUME POINT callout still shows the 2026-06-18 pre-O7 snapshot and lists
  "O5 next" — it is a dated snapshot, superseded.
- **Combat review Tier −1 — Wave B not started.** Wave A closed D1 / D11 / D24 / D25 / D26 / D27 /
  R22. **Still open from the recommended first wave: D4** (mid-fight slot refill re-anchors an
  engaged skirmish squad — kiting disabled for the whole replacement window), **D5** (formation
  layout only ever shrinks; a refilled seat resolves to offset `(0,0)` — phantom seat stacked on
  the anchor), **D6** (Phase C's "forming" count includes engaged squads → `MAX_FORMING_SQUADS`
  silently becomes an offense-concurrency reducer), **D9** (the engaged stuck-threshold ladder was
  never wired into the live bot), **D10** (rover discards incomplete flee results — retreating
  creeps freeze under a swarm), **D2** (`CRITICAL_STRUCTURE_MIN_HITS = 5000` equals spawn max hits
  — any spawn scratch arms the safe-mode trigger), **D3** (the SafeModeMission `activated` latch is
  permanent — a room auto-safe-modes at most once, ever). Plus the review's other sequencing
  recommendations: widen T-HEAL-3 into R1 (enemy-boost threading), the drain/tower group
  (D13 / D20 / R2 / R3), D8 (neutral constructed walls are unattackable by every pipeline), and
  R19 (re-tune kernel parameters on realistic chokepoint terrain **before** any further
  parameter work).
- **D28 (new, Wave B, from `b269682`):** an uncontested `Secure` over a hostile-free room can never
  reach `Resolved` — `engaged_once` latches only in-room *with* a focus, and an empty room offers
  no focus. Bounded by budgets + D25, but it produces border-oscillation holds. Fix direction:
  allow `resolved` to fire for an uncontested objective with in-room members, live visibility, and
  zero hostiles.
- **Expansion-stall program:** Wave 1 ✅, L1 ✅, L3 ✅. **L2 (poison-list self-heal) is deliberately
  deferred and must ship LAST** — the 103-room unreachable list is currently load-bearing for
  `scouting_coverage_complete`; healing it early flips `covered` false and re-defers every
  below-ring candidate.
- **Other tracked-open:** W2 / W3 / W4 (war supervisor trim, Escort producer, WarDecl posture) —
  unstarted. I1 / I2 minted `SquadId` — open as REC-009. H5 parity oracle + golden vectors —
  in progress. K2c-2 / K-RECONCILE unstarted; K4 SK mineral deferred. ADR 0042 residuals R1–R4 +
  `opportunity_floor>0`. Reconciliation REC-062 / REC-068 deferred. ADR 0031b needs a re-tune
  under `w_energy=1.0`.

### 3.5 Half-finished chats

Every session log was readable (13 files, 2026-06-18 → 2026-08-13). Two ended mid-decision:

| Session | Title | Span | Ended |
|---|---|---|---|
| `624f61bc` (17 MB, 6196 records) | "Accept ADR 0040, deploy WFV-27 to private then MMO" — became the long-running master session | 2026-07-06 → **2026-08-13** | **OPEN — awaiting your answer.** Final message: two corrections (the Wave-2 agent did *not* die; the `PathNotFound` hypothesis was wrong), then a fork — *"Want me to deploy `e857c76` to private now, or go straight for the ADR 0046 merge + reset?"* Its own recommendation: deploy `e857c76` first, read one discover cycle, then decide on 0046 with evidence. **No reply was ever given.** |
| `05c3d02c` (2.1 MB) | "Debug MMO bot sprawl and room expansion logic" | 2026-08-11 | **Paused near the context limit, mid-Wave-1.** Its state was handed off to the memory file `expansion-fix-implementation-state.md` and picked up by `624f61bc`. Superseded — not lost work. |

Completed sessions: `36c4583d` (combat review → Wave A → MMO deploy verified, 2026-07-29),
`8e4191a2` (ADR 0045 written + committed), `5f50e415` (0008a readiness review, multi-agent),
`ef82dc31` (Dismantle seam fix, 8 commits), `399de59d` (rover border/swamp contention),
`9e6ee124` (ADR 0033 rover sim), `f46dd214` (dense-crowd resolver — found already fixed),
`a376dc10` (link-transfer targeting), `a891d4c9` (damage.rs comment sweep), `c4fca422` (the July
expansion investigation that became ADR 0038), and `9cae9f85` (130 MB — the June combat-overhaul /
G4 migration marathon; ended cleanly on a dismantle-danger simplification).

---

## 4. ORPHANED / UNCLEAR — things that do not reconcile

1. **`docs/execution/g4-offense-plan.md` does not exist.** The O1–O7 checklist lives in
   `docs/plans/combat-overhaul-plan.md` §5 and is mirrored in `docs/execution/phase-2.md` §2.0.
   The name appears only as prose inside those two files. Nothing is missing — the pointer is.
2. **[RESOLVED 2026-08-22 — renumbered to `0044a-all-sinks-ev-scoping.md`.]** Two ADRs numbered 0044 — `0044-transfer-market-min-cost-flow.md` (Accepted) and
   `0044a-all-sinks-ev-scoping.md` (Decision-ready scoping). A numbering collision, not a
   content conflict; the A3 doc should probably become `0044a`.
3. **[RESOLVED 2026-08-22]** ~~**ADR 0046 status divergence.**~~ `master` said *Proposed*; the unmerged worktree copy said
   *Accepted + implemented*. Whichever way the decision goes, one of these is wrong today.
4. **[RESOLVED 2026-08-22 — header fixed.]** ADR 0042's status line said "MMO deploy pending operator final review" — but its code rode
   to MMO inside the Wave A artifact on 2026-07-28. Stale header.
5. **[RESOLVED 2026-08-22 — header fixed.]** ADR 0038's status line said "MMO deploy pending operator go-ahead" — superseded; its reset
   shipped with the 2026-07-01 WFV-24 deploy and it has been live for weeks.
6. **`docs/plans/combat-overhaul-plan.md` bills itself as the single forward-looking SSOT**, but
   the 2026-07-01 reconciliation (DOC-1) already flagged that claim as stale, and its §4 resume
   point is explicitly marked `[OVERTAKEN]`. Its §3 rollup table is still the best per-workstream
   summary; §4 / §4D are history. **For anything after 2026-07-01, trust the per-ADR ledgers, the
   review docs, and the memory files — not this plan.**
7. **`docs/execution/phase-2.md` has not been updated since 2026-06-18.** Its §2 RESUME POINT and
   §2.0 status log stop at G4-O6. Six ADR waves, three MMO deploys, and two reviews have happened
   since. Nothing in it is *wrong*, but it is a June artifact and should not be read as current.
8. **[RESOLVED 2026-08-22 — all pushed.]** Nothing had been pushed to `origin` in ~7 weeks — 119 superproject commits and 45 submodule
   commits exist only on this machine. A single point of failure, not a correctness issue.
9. **[PARTLY RESOLVED 2026-08-22 — all local branches and worktrees deleted; the two stale remote branches remain, deliberately.]** Stale branches live only on `origin`, not locally. Every local branch other than `master` is
   fully merged (0 unique commits vs master) and safe to delete: `new-room-planning`,
   `docs/review-planning`, and the detached `blissful-shaw-165bc1`. What *is* divergent is on the
   remote: **`origin/new-room-planning`** carries 305 unique commits whose tip is `fe0cf5b`
   (2026-02-10 — it predates the entire rewrite), and **`origin/bindgen`** carries 298 unique
   commits from **2021**. Neither is referenced by any doc or memory entry. Low-stakes cleanup,
   but nothing local depends on either.
10. **A dead workflow artifact** referenced by memory: the Wave-2 ADR 0046 design-review output at
    `…\05c3d02c…\tasks\w4dpnhtzo.output` is 0 bytes. The review was successfully re-run (28
    findings, four reviewers; full JSON under `…\624f61bc…\tasks\ws40tlrlm.output`) and **all 28
    were triaged as non-live-bugs** — 25 critique unwritten ADR-0046 code, 2 were already fixed by
    Wave 1, 1 is a confirmation note. No action needed; recorded so the 0-byte file is not
    re-chased.
11. **`docs/reviews/ultracode-completion-kickoff.md`** is described in the memory ledger as the
    "master kickoff prompt driving all remaining work." It has driven nothing since 2026-07-02 —
    everything since was operator-directed. Either revive it or mark it superseded.

---

## 5. Recommended pick-back-up plan

> **⚠ Steps 0–4 below are DONE as of 2026-08-22, with one deliberate deviation.** The operator
> answered the Step-0 fork with **"merge ADR 0046"** rather than "deploy first", and scoped the
> tie-off to the repo — so Steps 1–2 (deploy + observe) were **not** performed, and Step 3's merge
> was executed directly. Step 4 (doc statuses) is done.
>
> **What is actually next, in order:**
>
> 1. **Decide the deploy shape.** Master is WFV 28, so deploying it is a loud reset. Two paths:
>    (a) deploy `wfv27-deployable-e857c76` first for a no-reset read of the three expansion signals,
>    *then* reset onto master; or (b) go straight to master and take the reset now. ADR 0046 was
>    built to make the reset safe (it removes the poison-list minting mechanism structurally), so
>    (b) is defensible — but nothing on master has been soaked.
> 2. **Private soak, whichever path.** ADR 0046 has never run against a live world. Its §5 success
>    criteria are the checklist: the unreachable list stays empty of 1-hop entries, the stale-intel
>    skip disappears from Select captures, and scouts visibly tour instead of pinning at 3.
> 3. **Then MMO** (already operator-authorized for this work), and read the three signals.
> 4. **Then combat Tier −1 Wave B** — the top open code workstream (Step 5 below, unchanged).
>
> The original plan text is kept below for its detail on gates and observation method.

**Step 0 — answer the open question (5 minutes).** Session `624f61bc` has been blocked on exactly
one fork for nine days. The recommendation on file is sound and is repeated below.

**Step 1 — deploy `e857c76` to the private server, then MMO. No reset.**
`cargo run --manifest-path screeps-pack/Cargo.toml -- deploy --server private-server`, then
`--server mmo` (MMO deploy for this work is already operator-authorized per the memory ledger).
This ships Wave 1 + L1 + L3 *and* picks up `e480ed9` (the zombie war missions currently spamming
"Expected structures" on MMO) as a free rider. ~33 lines of behavior change, no WFV bump, no reset,
instantly reversible via the feature flags.

**Step 2 — observe one discover cycle (~840–5000 ticks) and read three signals.**
Tail with `screeps-rest-api/examples/tail.rs --shard shardX`, grep `ClaimOp`; re-run the offline
world decoder (`IBEX_WORLD_PAYLOAD=<segs 50–52> IBEX_NOW=<tick> cargo test -p screeps-ibex
decode_live_world -- --ignored --nocapture`). The three signals: (a) scouts issuing move intents /
changing rooms; (b) the 103-room unreachable list draining; (c) a claim mission actually firing.
Baseline to compare against: tick ~4.47M, 5 rooms, GCL 12, bucket 9487, CPU 16.8/500.

**Step 3 — decide ADR 0046 on that evidence.**
- *Stall breaks* → 0046 becomes an optimization rather than a rescue. Merge it deliberately:
  rebase the worktree branch onto `e857c76` (expect conflicts in `claim.rs` and
  `visibilitysystem.rs` — 0046 supersedes L1a/L1b by construction), run the full suite **plus the
  `sim_is_deterministic_over_rounds` determinism fence**, adversarial diff review, private soak,
  then the WFV 27→28 loud reset.
- *Stall persists* → ship 0046 as the rescue, through the same gates, knowing precisely what it
  must fix.

Either way: **L2 (poison-list self-heal) ships last.**

**Step 4 — flip the doc statuses** the outcome settles: ADR 0046 (one way or the other), ADR 0042,
ADR 0038. Cheap, and it stops the next cold resume from having to re-derive this document.

**Step 5 — combat Tier −1 Wave B.** Take the bounded, no-WFV batch first: **D2 + D3** (two-line
safe-mode class fixes protecting a scarce irreversible resource), then the roster-churn cluster
**D4 + D5 + D6**, then **D9 + D10** (live-adapter gaps), then **D28**. Private-soak as one wave;
none of them needs a reset.

**Step 6 — then pick one of the three larger programs**, in the order the evidence argues for:
- **R1 / ADR 0041 boost layer** — the review calls enemy-boost blindness the top MMO risk, and it
  is the single prerequisite gating ADR 0008a's entire boosted-assault frontier. Largest
  strategic unlock.
- **R19 re-tune on realistic chokepoint terrain** — the review says this should *gate* any further
  kernel-parameter work. The uncommitted rover-eval sweep (§3.1) is its first piece and is already
  written and compiling; committing it is the natural entry point.
- **ADR 0045 power-creep operators** — accepted-pending, pure greenfield, no interaction with the
  open combat/expansion work, so it parallelizes cleanly if you want a second track.

**Housekeeping, any time:** push `master` plus all 45 submodule commits to `origin`; delete the
three merged worktrees (`a365489048f5b1403`, `afb8dd11f694e32b3`, `blissful-shaw-165bc1`) and the
three fully-merged local branches; rename the duplicate ADR 0044; decide whether the two stale
remote branches (`origin/new-room-planning` @ 2026-02-10, `origin/bindgen` @ 2021) are worth
keeping.

---

*Compiled 2026-08-22 from `master` @ `e857c76`. Sources: git (branches, worktrees, submodules,
working tree), `docs/design/0001`–`0046`, `docs/plans/combat-overhaul-plan.md`,
`docs/execution/phase-2.md`, `docs/reviews/combat-systems-review-2026-07-09.md`,
`docs/reviews/expansion-stall-2026-08-11.md`, the project memory ledger, and 13 session
transcripts. No runtime code was changed in producing this document.*
