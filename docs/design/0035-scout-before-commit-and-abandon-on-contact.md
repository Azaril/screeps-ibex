# ADR 0035 — Scout-before-commit + Abandon-on-unwinnable-contact (the VACUOUS-INTEL ENGAGE CASCADE)

- **Status:** Proposed (2026-06-30)
- **One line:** A squad commits to a TOWERED room on EMPTY/STALE intel, reaches it (now that RC-11 unfroze
  travel), discovers it cannot win, and oscillates reach↔retreat forever with no abandon. Fix: make
  target-selection, force-sizing, AND the `target_is_uncontested` rally classification require REAL
  (`LiveVisible` / non-empty) intel — NOT `is_reliable()` / `defense.is_some()`, which both admit
  empty-Cached/stale — and add an ABANDON-ON-CONTACT latch that de-commits the objective when the real in-room
  P(win) = lose, with a backoff so it neither oscillates nor re-advances.

Direct continuation of [ADR 0034](0034-rally-travel-convergence-robustness.md) RC-11/D9: the D9 fast-path
intel-gate let squads **reach**; reaching then exposed this **downstream** blocker (commit was already made on
vacuous intel, so a reached squad faces a fight it was never sized for and has no de-commit path). Builds on
[ADR 0027](0027-objective-squad-lifecycle.md) (objective lifecycle / `reconcile` kernel / `mark_unwinnable`
backoff), [ADR 0031](0031-capability-driven-force-composition.md) (force-sizing / winnability oracle / the
P(win) directive), and [ADR 0032](0032-ev-optimal-squad-assignment.md) (the EV auction).

---

## 0. Problem statement (live, 2026-06-30)

An offense squad commits to **W4N5 (a TOWERED room)** on EMPTY/unscouted intel, reaches at 3/3, the real
in-room **P(win) = LOSE**, and it oscillates reach↔retreat: **455 ticks Retreating vs 1 Engaged, 0 kills, no
abandon.** `uncontested` flickers (live vision toggling as a member enters/exits), and the rally is computed
**AS the target room** so members walk straight into tower range instead of staging one room short.

The bug has **two halves**, both turning on the same root: *the system treats "we have a `RoomData` entity
/ `defense.is_some()`" as "we have trustworthy intel for an assault decision," but an empty-Cached or stale
defense profile is VACUOUS — it means "no towers were visible the last time we looked," not "no towers."*

1. **Commit half (selection + sizing + rally classification):** the room is selected, sized for ZERO towers,
   and classified `uncontested` — so the rally stages AT the target centre and the squad deploys at a
   min-viable quorum, walking into a fight it was sized to lose.
2. **Abandon half (no de-commit):** once reached, the squad retreats from real towers, the `reconcile` kernel
   mis-reads the retreat as a clean **clear** (`resolved`) and **withdraws** the objective; war.rs re-upserts
   it ~10 ticks later; Phase C re-fields the same squad; same vacuous intel ⇒ same outcome. Spiral.

---

## 1. Consolidated ROOT-CAUSE MAP (each link → file:line)

The cascade, end to end. Verified against the tree on 2026-06-30.

### Commit half — selection / sizing / classification trust empty intel

| # | Mechanism | file:line |
|---|---|---|
| C1 | `ThreatAssessmentSystem::run` updates `RoomThreatData` ONLY for **currently-visible** rooms; a room visible at T0 with no towers gets `hostile_tower_positions=[]`, `tower_energy=[]`, `last_seen=T0`. Towers that build/energize **after** T0 are invisible until the room is seen again. | `military/threatmap.rs:308-311` (visible-only gate), `:335-344` (towers gathered from current structures), `:390-408` (persist with empty towers + `last_seen=current_tick`) |
| C2 | Stale data is retained up to `THREAT_DATA_MAX_AGE = 500` ticks, but war.rs's re-scout gate only fires past **200** ticks. A room seen 60 ticks ago whose towers energized at T0+60 is "fresh" (<200), so the gate does NOT re-scout — it commits on the empty snapshot. **Freshness of `last_seen` ≠ reliability of the tower content.** | `military/threatmap.rs:266` (`THREAT_DATA_MAX_AGE=500`), `operations/war.rs:970` (200-tick re-scout gate) |
| C3 | `DefenseProfile.towers` is built directly from `threat_data.hostile_tower_positions` — `[]` when C1/C2 hold. `candidate.defense = Some(defense)` is set **even with empty towers** — there is NO flag distinguishing *scouted-with-towers* / *scouted-empty* / *never-scouted-but-cached*. | `operations/war.rs:1127-1145` (DefenseProfile from `hostile_tower_positions`), `:1184` (`defense: Some(defense)`) |
| C4 | The winnability GATE checks only `candidate.defense.is_none()` (and `defense_intel_reliable = candidate.defense.is_some()`). An empty-tower `Some(defense)` PASSES the gate. | `operations/war.rs:1398` (`defense_intel_reliable = candidate.defense.is_some()`), `:1414-1416` (gate on `defense.is_none()`) |
| C5 | `economic_rank_score` reads `defense.towers` as empty ⇒ `tower_dps_at_assault = 0` ⇒ `p_win_proxy = 300/(300+0) = 1.0` ⇒ the room ranks as **maximally winnable + cheap**, floating to the top of the candidate list. | `operations/war.rs:1733-1748` |
| C6 | `plan_engagement` / the force-sizing oracle sizes against `DefenseProfile { towers: [], .. }` ⇒ sizes for **zero tower DPS** ⇒ the committed squad is under-built for the real towers. | `operations/war.rs:1430` (`plan_engagement`), `screeps-combat-decision/src/force_sizing.rs` (sizes to the supplied profile; no scouted/reliable flag) |
| C7 | **The rally classification still trusts empty-Cached** (the half RC-11/D9 did NOT cover). `intel_reliable = intel_source.is_reliable()` = `Cached \|\| LiveVisible`. An empty-**Cached** towered room → `is_reliable()=true` → `target_is_uncontested(true, no_hostiles, no_hostile_towers, ..) = true`. | `military/squad_manager.rs:1799` (`is_reliable` = Cached\|LiveVisible), `:2131` (`intel_reliable = is_reliable()`), `:2135-2140` (uncontested gate), `screeps-combat-decision/src/rally.rs:61` (`rally_intel_reliable`), `:80-82` (`target_is_uncontested`) |
| C8 | `uncontested=true` then drives BOTH: (a) `shared_rally_point` returns the **target-room centre** (walk straight in, no one-room-short staging out of tower range), and (b) the gather quorum trickles in at `gathered>=1` + the depart gate releases at min-viable. Live vision toggling as a member crosses the boundary FLAPS empty-Cached↔LiveVisible (and back to empty-Cached when it leaves), oscillating the rally room. | `screeps-combat-decision/src/rally.rs:280-302` (`shared_rally_point` uncontested→target centre, contested→one-room-short), `military/squad_manager.rs:2220` (rally call), `:2324` (`gather_quorum_met` with `uncontested`), `:2171` (`ready_to_depart_gate`) |

> **The C7/C8 inconsistency is the crux.** RC-11/D9 correctly gated the *win-or-stall fast-path* on
> `have_target_intel = … \|\| intel_source == LiveVisible` (`squad_manager.rs:2167-2169`), **deliberately NOT
> `is_reliable()`** ("an empty-*Cached* room is itself the vacuous case"). But the **`uncontested`
> classification on the very same path** (`:2131`) still uses `is_reliable()`. So D9 stopped the *fast-path*
> from firing on empty-Cached, yet `uncontested` still flips true on empty-Cached and steers the rally AT the
> target. The two intel predicates on one code path disagree about what "reliable" means.

### Abandon half — a retreat is mis-read as a clear, then re-fielded

| # | Mechanism | file:line |
|---|---|---|
| A1 | `engaged_once` latches when `Engaged && in_room` — and **stays latched through a subsequent Retreating** (the latch is monotonic; nothing clears it on retreat). | `military/squad_manager.rs:2630-2631` (latch on `Engaged && in_room_any`); `squad.rs:426` (the field, never reset except on retire) |
| A2 | A squad that retreats from in-room contact presents to `reconcile` as `engaged_once=true, in_target_room=true, has_focus=false, has_members=true` — **identical** to a squad that CLEARED the room. The `resolved` gate cannot tell "cleared" from "retreated." | `screeps-combat-decision/src/lifecycle.rs:166` (`resolved = engaged_once && in_target_room && !has_focus && has_members && !declaiming`) |
| A3 | `resolved ⇒ withdraw=true, mark_unwinnable=false` — the objective is REMOVED from the queue as a clean win (no backoff). | `screeps-combat-decision/src/lifecycle.rs:205-206`, `military/squad_manager.rs:1276-1277` (`withdraw(obj_id)`) |
| A4 | war.rs re-upserts the offense objective every scan (TTL refresh, `OFFENSE_OBJECTIVE_TTL`), so the withdrawn objective re-appears almost immediately; Phase C re-fields a new squad on the same room. **`is_unwinnable_now` is never consulted by the offense producer** — even if A3 had set `mark_unwinnable`, the producer would re-upsert anyway. | `operations/war.rs:1490-1494` (re-upsert), `military/objective_queue.rs:407` (`is_unwinnable_now` exists, **unused in war.rs**) |

**Consolidated mechanism:** C1/C2 (visible-only, freshness≠content) → C3/C4 (empty `Some(defense)` passes the
gate) → C5 (ranks top) → C6 (sized for zero towers) → C7/C8 (classified uncontested → rally AT target,
trickle in) ⇒ the squad **reaches** a fight it was sized to lose; then A1/A2 (retreat looks like a clear) →
A3 (withdraw, no backoff) → A4 (re-upsert, no `is_unwinnable_now` consult, re-field) ⇒ **oscillation with
zero de-commit machinery.** Each half is independently a bug; together they are the observed spiral.

---

## 2. FIX DESIGN

Two principles, mirroring the operator's two directives and the RC-11 precedent.

### Principle I — SCOUT-BEFORE-COMMIT: a commit decision requires REAL intel, not merely "we have an entity"

*Empty hostiles/towers may only mean "clear" when we have LIVE or non-empty intel for the room.* This is the
exact RC-11/D9 semantic (`== LiveVisible || non-empty-DTO`), applied to the THREE remaining commit gates that
RC-11 did not touch: target-selection, force-sizing, and the `uncontested` rally classification.

**Key changes (where they live):**

- **D1 — Three-state defense reliability on the candidate (selection + sizing).** Replace the boolean
  `candidate.defense: Option<DefenseProfile>` *implicitly* carrying reliability with an explicit tri-state.
  Add to `RoomThreatData` a `tower_intel: TowerIntel` discriminant — `{ Seen, ScoutedEmpty, NeverSeen }` —
  set in `threatmap.rs:run`: `Seen` when towers were found, `ScoutedEmpty` when the room was visible this
  update with zero hostile towers, persisted across non-visible ticks. Thread it into `DefenseProfile` as
  `tower_intel` (war.rs:1136). Then:
  - **Selection gate (war.rs:1414):** a `honor_verdict()` doctrine DEFERS not just when `defense.is_none()`
    but also when `defense.tower_intel == ScoutedEmpty && stale` (see D2) — request a HIGH re-scout and skip.
    `Seen` (real towers) and `NeverSeen` (already handled by `is_none`) are unchanged.
  - **Sizing (force_sizing):** sizing already sizes to the supplied profile; with D2's re-scout gate, an
    empty-but-stale profile never reaches sizing. No oracle change needed — the gate is upstream.
  - **Rank (war.rs:1733):** `economic_rank_score` clamps `p_win_proxy` to a **penalty** (e.g. ≤0.5) when
    `tower_intel == ScoutedEmpty` so a vacuous-empty room does NOT outrank a genuinely-`Seen`-clear one.
    (Defense-in-depth: even if D2's gate is bypassed, the room stops floating to the top.)
- **D2 — Content-staleness gate distinct from `last_seen`-staleness.** The 200-tick `last_seen` gate
  (war.rs:970) is about *recency of any vision*. Add a SEPARATE, tighter `SCOUT_RECONFIRM_TICKS` (e.g. 30–50)
  gate **specifically for an empty-tower defense on an invader-core/attack candidate**: if `tower_intel ==
  ScoutedEmpty` AND `now - last_seen > SCOUT_RECONFIRM_TICKS`, request a HIGH re-scout and skip the commit
  this scan. This catches "towers energized after the snapshot" without waiting for the 200-tick window. It
  is scoped to candidates we're about to COMMIT a squad to (cheap — the loop already has the room in hand).
- **D3 — `uncontested` requires `LiveVisible || non-empty`, NOT `is_reliable()` (the C7 fix — RC-11 parity).**
  In `squad_manager.rs:2131`, compute the uncontested-intel predicate the SAME way D9 computes
  `have_target_intel`: `uncontested_intel = !hostiles.is_empty() || !structures.is_empty() || intel_source ==
  LiveVisible`. Pass THAT (not `is_reliable()`) as the first arg to `target_is_uncontested`. An empty-Cached
  towered room then classifies **contested** ⇒ `shared_rally_point` stages **one room short** (out of tower
  range) ⇒ the squad masses at the staging room and only advances under the gather quorum. The instant a
  member gains live vision and sees the towers, `no_hostile_towers=false` keeps it contested for real. **This
  makes the two intel predicates on the path agree (one source of truth for "real intel").** Document the
  rename of `target_is_uncontested`'s first param semantics in `rally.rs` and adjust `rally_intel_reliable`'s
  doc (it stays for the *legacy oscillation* concern but is no longer the gate the *uncontested classifier*
  reads — they were conflated).

### Principle II — ABANDON-ON-UNWINNABLE-CONTACT: de-commit a reached fight we cannot win, with a backoff

*When a squad has reached the target, engaged, and the real in-room P(win) = LOSE, that is the only point at
which we have ground-truth that the commit was wrong. De-commit: keep the objective off (backoff), do NOT
withdraw-as-clean (which invites instant re-field), and do NOT re-advance (which oscillates).*

**Key changes (where they live):**

- **D4 — Distinguish CLEARED from RETREATED in `reconcile` (the A2/A3 fix).** Add to `ReconcileSnapshot` a
  `retreated_from_contact: bool` — true when the squad's combat state is `Retreating` (or `Engaged`-but-
  losing per the present-force P(win)) while in-room, NOT merely focus-less. Then split the `resolved` gate:
  - `resolved` (clean clear) requires `engaged_once && in_target_room && !has_focus && has_members &&
    !declaiming && !retreated_from_contact` — a TRUE clear has no living hostile, so the squad is not
    retreating.
  - NEW `unwinnable_contact = engaged_once && in_target_room && retreated_from_contact && has_members` ⇒
    `Retire { reason: GaveUp, withdraw: false, mark_unwinnable: true }` (NOT a `Defend` objective — kernel
    already exempts `is_defend` from `mark_unwinnable`). So the objective is BACKED OFF, not withdrawn.

  The snapshot field is computed in the manager from the same P(win) frame already on the path:
  `retreated_from_contact = in_target_room && engaged_once && !present_force_wins_or_stalls(&view, center)`
  (the real in-room DTOs now drive `present_force_wins_or_stalls`; in-room ⇒ `LiveVisible`, so the assessment
  is over REAL towers — no vacuous win). This reuses the EXACT inverse of the retreat condition (lib.rs), so
  "abandon" and "retreat" can never disagree about losing.
- **D5 — Consult the abandon backoff in the producer (the A4 fix).** In war.rs's offense candidate loop,
  SKIP a candidate whose room `is_unwinnable_now(room, now)` (the exponential backoff `mark_unwinnable`
  already records — `objective_queue.rs:391/407`). This is the missing producer-side consumer: today the
  backoff is written but never read by war.rs, so re-upsert is instant. With D5, an abandoned room is
  suppressed for the backoff window and **also keeps a re-scout registered** (the existing stale path) so it
  is re-assessed with FRESH intel when the backoff expires — never permanently abandoned, never oscillating.
- **D6 — Per-objective abandon latch (anti-flicker).** `mark_unwinnable`'s exponential backoff IS the latch
  (it already escalates `retry_after` per attempt and is capped + clearable). The manager must call it
  ONCE per de-commit (the `Retire{mark_unwinnable}` path already calls `mark_unwinnable(room, now)` at
  `squad_manager.rs:1281`). Add `clear_unwinnable(room)` on a genuine `Resolved` clear so a later real win
  resets the backoff. No new serialized field beyond the already-serialized `unwinnable` vec.

---

## 3. SIM-FIRST plan (reproduce the cascade RED, then GREEN)

Operator's standing requirement: prove the root cause and the fix offline before any deploy. The cascade has
TWO provable surfaces — the **pure kernels** and the **lifecycle harness integration**.

### 3.1 Pure-kernel tests (where the harness cannot see it) — `screeps-combat-decision`

- **K1 (rally.rs) — `target_is_uncontested` over an intel TRANSITION.** Call twice:
  `(no_real_intel=false, no_hostiles=true, no_towers=true, …)` ⇒ expect `false` (an empty-Cached towered room
  is NOT uncontested); then `(live_visible=true, no_towers=false, …)` ⇒ expect `false` (real towers seen).
  Pin that the ONLY `true` path is `(real_intel=true, no_hostiles=true, no_towers=true, …)`. **RED today:**
  the param is `is_reliable()` (Cached ⇒ true ⇒ uncontested true). **GREEN after D3.**
- **K2 (rally.rs) — `shared_rally_point` stages one-room-short for a contested(empty-Cached) target.** With
  `uncontested=false` (the D3 output), assert the rally room is the neighbour toward the approach, NOT the
  target centre. Pins C8a.
- **K3 (lib.rs) — `winnable_fast_path_allowed` chain over the transition** (extend the existing
  `winnable_fast_path_gated_on_real_target_intel`): vacuous-win + empty-Cached ⇒ blocked; same after
  `LiveVisible` ⇒ allowed. Already GREEN (D9) — keep as the regression fence and add the empty-Cached case
  explicitly so C7's "Cached is not LiveVisible" is pinned.
- **K4 (lifecycle.rs) — `reconcile` distinguishes RETREATED from CLEARED.** Add:
  `reconcile(engaged_once=true, in_target_room=true, has_focus=false, retreated_from_contact=true)` ⇒
  `Retire { GaveUp, withdraw=false, mark_unwinnable=true }`, NOT `Retire { Resolved, withdraw=true }`. And the
  mirror: `retreated_from_contact=false` (a true clear) ⇒ `Resolved/withdraw`. **RED today** (the field/gate
  don't exist). **GREEN after D4.**

### 3.2 Lifecycle-harness integration (`screeps-combat-eval/src/harness/lifecycle.rs`)

The harness already MODELS the delay (`ChurnTarget.empty_dtos_on_arrival_ticks`) but the offense path
short-circuits to `DeployedAndEngaged` the moment DTOs are readable (`:813-817`) — it never models
*commit-intel ≠ arrival-intel* nor *cannot-win-on-arrival*.

- **H1 — Extend `ChurnTarget`:** add `commit_intel_empty: bool` (commit-time DTOs empty ⇒ vacuous
  uncontested) and `arrival_has_towers: bool` (arrival DTOs reveal real towers ⇒ in-room P(win)=lose). Thread
  `uncontested` so that during Forming/Travel it reflects the (empty) commit view and on Arrival it flips to
  the real view.
- **H2 — Wire the gates the live bot wires.** Thread `have_target_intel` into the harness's
  `ready_to_depart` / `gather_quorum_met` calls (today they take a hardcoded `uncontested`), so the harness
  exercises D9 + D3, not a hand-set boolean. (Closes harness gap G3-for-intel.)
- **H3 — Arrival branch models cannot-win.** When `tick == arrives_at && arrival_has_towers`, the squad is
  in-room, `engaged_once` latches, but `present_force_wins_or_stalls=false` ⇒ set
  `retreated_from_contact=true` and feed `reconcile`.
- **New `ChurnOutcome` variants:** `LapsedOnVacuousCommit` (RED: committed uncontested, reached, retreated,
  reconcile mis-resolved → withdraw → re-field, generations climb) and `AbandonedOnContact` (GREEN: reconcile
  returns `GaveUp/mark_unwinnable`, the producer suppresses via `is_unwinnable_now`, NO re-field within the
  backoff — `generations` stable). 
- **RED scenario:** `ChurnTarget { commit_intel_empty: true, arrival_has_towers: true, latch_engaged_in_room_only: true, /* D3/D4 disabled */ }` ⇒ expect `LapsedOnVacuousCommit` with `generations > 1` (the oscillation: false-resolve → re-upsert → re-field).
- **GREEN scenario A (scout-before-commit):** D3 wired ⇒ `uncontested=false` on the empty commit view ⇒ the
  squad MASSES one-room-short and only advances on real intel. With `arrival_has_towers` it then either
  abandons cleanly (B) or, if winnable, engages.
- **GREEN scenario B (abandon-on-contact):** D4+D5 wired ⇒ `AbandonedOnContact`, `generations` stable (no
  oscillation), the room in backoff.
- **H4 — Param-sweep gate:** add the cascade family to `ParamScore` (`param_sweep.rs`) so the RED→GREEN is
  permanently regression-fenced, mirroring ADR 0034 Phase 4.

**Order:** K1/K4 RED first (pure, fast) → D3/D4 to GREEN → H1–H3 RED on the production driver → D5/D6 +
harness-wiring to GREEN → H4 fence. Production code changes land only after the matching RED test exists.

---

## 4. INTERACTIONS

- **RC-11 / D9 (ADR 0034) — the parent.** D9 gated the *fast-path* on `== LiveVisible`; D3 here applies the
  IDENTICAL semantic to the *`uncontested` classifier* that D9 left on `is_reliable()`. After D3 the two
  intel predicates on the squad-manager path are ONE notion of "real intel" — the inconsistency that let
  empty-Cached steer the rally is closed. D9 unfroze travel (squads now reach); 0035 is what makes *reaching*
  safe (don't commit to the unwinnable; abandon if you do). **Cross-ref:** this ADR's C7 is the explicit
  "D9 covered the fast-path, not the classifier" note.
- **The P(win) directive (combat-ev-economic-and-pwin-gating).** D4's abandon predicate is
  `!present_force_wins_or_stalls` over the **real in-room** DTOs — i.e. abandon is a genuine Lanchester
  P(win)=lose verdict, not a composition or HP heuristic. This is the directive applied at the de-commit
  point: "rally/forming completion is P(win)-driven (win-or-stall)" extended to "*de*-commitment is
  P(win)-driven (lose ⇒ abandon)." A vacuous no-intel win never triggers abandon (it's not in-room yet);
  abandon fires only against REAL towers (`LiveVisible`).
- **Force-sizing P2b (ADR 0031).** C6 shows the oracle currently sizes to an empty profile. 0035 fixes this
  UPSTREAM (D1/D2: an empty-stale profile never reaches the oracle — re-scout first), so the oracle keeps
  sizing to whatever profile it's given, now guaranteed `Seen`/non-vacuous. No change to the sizing math; the
  oracle's `RequiredForce`/`win_probability` are now fed reliable inputs. The abandon path (D4) is the
  runtime backstop when the oracle was nonetheless fed stale intel that changed after commit.
- **The auction (ADR 0032).** C5's inflated `p_win_proxy=1.0` for a vacuous-empty room also inflates its EV
  rank in the assignment auction (a death-trap looks like a free win). D1's `economic_rank_score` penalty for
  `ScoutedEmpty` de-ranks it so the auction does not preferentially assign squads to vacuous targets. The
  abandon backoff (D5/`is_unwinnable_now`) must also be consulted by the auction's candidate set (not just
  the offense producer) so a just-abandoned room is not immediately re-won by reassignment — the
  `reassign_available` snapshot input should exclude `is_unwinnable_now` rooms.
- **Objective lifecycle (ADR 0027).** D4 extends the `reconcile` kernel's terminal taxonomy: a retreat-from-
  contact is now its OWN terminal (`GaveUp + mark_unwinnable`, NOT `Resolved`), distinct from a clean clear.
  This uses 0027's existing `mark_unwinnable` exponential backoff (the latch) and `withdraw` semantics
  unchanged — only the *gate* that selects them is refined. The reassign path (0027 v1) must treat an
  abandon as a LOSS terminal (no in-place reassign — don't chain a squad that just lost into a sibling), which
  the kernel already does (`Wiped`/`GaveUp` retire, only `Resolved`/`ObjectiveGone` reassign).

- **WORLD_FORMAT_VERSION risk — assessment: NO bump.** 
  - D3 (`uncontested` predicate): pure per-tick recompute, no serialized state — like D9.
  - D4 (`reconcile` retreated-from-contact): `ReconcileSnapshot` is `Copy`, rebuilt each tick from ephemeral
    context; the new field is not serialized.
  - D5/D6 (abandon latch): reuses the **already-serialized** `objective_queue.unwinnable` vec — no NEW
    serialized shape. (Confirm the `unwinnable` vec is part of the serialized objective-queue snapshot; if it
    is NOT currently serialized, the latch is session-only, which is acceptable — a VM reset re-scouts and
    re-assesses anyway — and still no WFV bump.)
  - D1 (`TowerIntel` on `RoomThreatData`): **the one to watch.** `RoomThreatData` is a `Component`; if it is
    serialized (ConvertSaveload), adding a field changes the serialized shape ⇒ a WFV bump (one loud reset).
    Verify at implementation time. If `RoomThreatData` is NOT serialized (it is re-derived from live/cached
    structures each tick — likely, since `threatmap.rs:run` rebuilds it from `get_structures`), then no bump.
    **Mitigation if it is serialized:** carry `TowerIntel` as a derived value in war.rs (computed from
    `hostile_tower_positions.is_empty()` + `last_seen` recency) rather than a new persisted field — keeping
    D1 entirely WFV-neutral. This is the PREFERRED form: D1 needs no new persisted state at all if the
    `ScoutedEmpty` discriminant is computed from the existing `hostile_tower_positions` emptiness + the
    existing `last_seen` at the war.rs consumption point.

---

## 5. Cross-references
- [ADR 0034](0034-rally-travel-convergence-robustness.md) — RC-11/D9 (the parent: vacuous-win fast-path gate;
  this ADR extends the same semantic to the classifier + adds the abandon path).
- [ADR 0027](0027-objective-squad-lifecycle.md) — `reconcile` kernel, `mark_unwinnable` backoff, withdraw.
- [ADR 0031](0031-capability-driven-force-composition.md) — force-sizing oracle / P(win) directive.
- [ADR 0032](0032-ev-optimal-squad-assignment.md) — EV auction / reassign.
- Code (verified 2026-06-30): `military/threatmap.rs:266,308-311,335-344,390-408`;
  `operations/war.rs:970,1127-1145,1184,1398,1414-1416,1490-1494,1733-1748`;
  `military/squad_manager.rs:1799,2131,2135-2140,2167-2169,2220,2324,2630-2631`;
  `screeps-combat-decision/src/rally.rs:61,80-82,280-302`; `…/src/lib.rs:1430,1447`;
  `…/src/lifecycle.rs:166,205-206`; `military/objective_queue.rs:391,407`;
  `screeps-combat-eval/src/harness/lifecycle.rs:226-278,813-817,645-680`.
