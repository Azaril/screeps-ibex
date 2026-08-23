# WvC-1 — military completion wave (correctness + wiring)

**Workstream:** WvC-1 (Phase 3) · **Advances:** ADR 0008a Tier 0, ADR 0026, ADR 0028, ADR 0034,
ADR 0035, review O3/R7 · **Status:** active

## Resume point

Working the ranked list below, Wave-B style: pure kernel + RED-verified pins per fix, batch-ship
per RULING-8 (no WFV needed; under 0047 even shape changes are cheap). Start: **T-HEAL-3a**.

## Target

The military machinery that already SHIPPED makes correct decisions: winnability gates see real
inputs, built-but-unwired decision code gets wired, and the known zombie/starvation classes close.
This wave + WvC-2 + the P4 re-tune is the "peak effectiveness" program (operator, 2026-08-23);
boost (Phase 5) then feeds correct machinery.

## Plan (ranked by effectiveness-per-risk)

- [x] **T-HEAL-3a — winnability-gate inputs** (SHIPPED-ready; first 0047-tolerant field — no WFV) (0008a Tier 0 #1, unboosted half; the boost
      multipliers land with 0041): (a) `project_enemy` (`squad_manager.rs:~876`) derives enemy
      `hits` from the body instead of hard-coding `0`; (b) `estimated_heal` (`threatmap.rs:~315`)
      gated on REACHABLE healers (adjacent-12 / ranged-3 relative to the focus), mirroring
      `heal_reaching`, instead of summing all hostile heal. Every engage/abandon/sizing decision
      reads these.
- [x] **damage.rs readiness tranche — wired** (`81ee72f`): `defender_spawn_readiness` live at the
      slot spawner (Phase B `DefenseUrgency` → `queue_slot_spawn`); only the URGENT verdict changes
      behavior (downsize to available energy when nothing holds the line); tower half
      (`should_towers_fire`/`net_tower_damage`) DELETED as superseded by U-TOWER `decide_towers`
      (review O3 answered: already heal-aware, and better).
- [x] **S5-CAP** (`7a87df5`): cap = `max_concurrent_squads(homes)` (floor 2 / +1 per 2 rooms /
      ceiling 8; parity with old 4 at 4 rooms) + `DEFENSE_SURGE_SQUADS=2` so a full offense board
      can never block a defense claim at the ACTIVE cap (closes R7's starvation; REC-008 covered
      only the forming pace). `claim_admission` pure kernel, 2 RED-verified pins.
- [x] **0035 FU2**: the real hole = the no-headway probe bounce (Retreating↔Engaged) resetting the
      REC-003 clock every Engaged tick → immortal in-room squad. First cut (kernel re-engage veto,
      decision `4d044be`) REVERTED (`4d186d8`) — it deadlocked two eval beds (positional
      "harmless" reads wrong at range; the probe bounce is load-bearing for borderline fights).
      Final: the give-up clock runs while Retreating OR stall-latched (`retreat_clock_holds`),
      clears only on genuine headway; stall streaks grow engaged-only / reset-on-decrease-any-state
      (bot + sim driver mirrored, agent `0c57c45`). Predicate CLOSED as terminator composition
      (ADR 0035 §2.1); pins RED-verified; eval regressions resolved (114 green).
- [x] **0026 L8** (`0455298`): `classify_coordination` reads scouted hostile OWNERS first (all-NPC ⇒
      Individual, any player body ⇒ Coordinated); Q1 source prior only as unobserved fallback.
      RED-verified pin. (ADR 0026 L8 note should drop its "until it is" caveat — done below.)
- [x] **0034 rally-bias live-wire** (decision `e6aa3ce`): D6c/D3(d) renewable-rally bias —
      `shared_rally_point_for_members_biased` prefers a friendly-spawn room among the valid+safe
      staging candidates; live caller feeds `homes`. The renew half was already free: Phase B-renew
      matches any member in a home room, and the job side walks them to the spawn. RED-verified pin.
- [x] **0028 K3/K4 wiring** — resolved the inverse way: the LIVE loop had evolved past both
      kernels. K4: `claim_admission` + `max_concurrent_squads` + surge/forming consts MOVED into
      `screeps_combat_decision::claim_pacing` as the shared kernel (bot imports it; pins moved
      with it); scalar `claims_allowed` stays harness-only — wiring it live would regress
      REC-008/S5-CAP defense exemptions. K3: the shared content is `build_body` +
      `PREFERRED_MEMBER_ENERGY` (already decision-crate); `slots_to_spawn` is the harness adapter,
      `queue_slot_spawn` the live one (latches + readiness downsize) — separate by design.
      ADR 0028 K3/K4 rows rewritten to as-built.
- [ ] Batch-ship + live-watch (drain signatures, engage/retreat sanity on the next real fight)

## Design deltas

- Review O3's "wire `should_towers_fire` at the tower seam" is MOOT: U-TOWER's `decide_towers`
  already makes the heal-aware fire decision (per-target `heal_reaching`, out-healed dogpile
  refusal) — the tranche's single-target tower helpers were deleted, not wired.

## Verification

Kernel pins RED-verified per fix; suite + wasm + fence; ship per RULING-8; live watch. The private
offense-soak (B-1) validates the wave end-to-end when the operator is home.

## Log

- 2026-08-23 — created on the operator's military-first reorder; T-HEAL-3a first.
- 2026-08-23 — readiness tranche wired (`81ee72f`); tower half deleted as superseded. Next: S5-CAP.
- 2026-08-23 (cont.) — S5-CAP (`7a87df5`), L8 (`0455298`), FU2 (veto→revert→clock, decision `4d186d8` + agent `0c57c45`), D6c rally bias (decision `e6aa3ce`) all landed. Remaining: 0028 K3/K4 wiring, batch-ship + live-watch.
