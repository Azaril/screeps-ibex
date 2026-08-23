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

- [ ] **T-HEAL-3a — winnability-gate inputs** (0008a Tier 0 #1, unboosted half; the boost
      multipliers land with 0041): (a) `project_enemy` (`squad_manager.rs:~876`) derives enemy
      `hits` from the body instead of hard-coding `0`; (b) `estimated_heal` (`threatmap.rs:~315`)
      gated on REACHABLE healers (adjacent-12 / ranged-3 relative to the focus), mirroring
      `heal_reaching`, instead of summing all hostile heal. Every engage/abandon/sizing decision
      reads these.
- [ ] **damage.rs readiness tranche — wire it** (built + tested + uncalled): route the emergency
      defender spawn path through `defender_spawn_readiness` (spawn-now-vs-wait) and evaluate
      wiring `should_towers_fire`/`net_tower_damage` at the tower decision seam (review O3).
- [ ] **S5-CAP** — `MAX_CONCURRENT_SQUADS` (hardcoded 4, `squad_manager.rs:211`) becomes
      governor/empire-size-aware (review R7: 4 offense squads can starve base defense).
- [ ] **0035 FU2** — give-up for a COMMITTED squad that reaches the room but never engages (the
      zombie class the budgets only bound loosely).
- [ ] **0026 L8** — coordination DPS keyed on OBSERVED bodies, not `TargetSource`.
- [ ] **0034 rally-bias live-wire** — renewable-rally bias exists sim-side (`lifecycle.rs:1423`),
      never wired live.
- [ ] **0028 K3/K4 wiring** — `slots_to_spawn` into the spawn adapter; `claims_allowed` into claim
      pacing (the harness computes both; the bot ignores them). The CLOSEOUT run stays
      HARNESS-gated; the wiring is not.
- [ ] Batch-ship + live-watch (drain signatures, engage/retreat sanity on the next real fight)

## Design deltas

- (running)

## Verification

Kernel pins RED-verified per fix; suite + wasm + fence; ship per RULING-8; live watch. The private
offense-soak (B-1) validates the wave end-to-end when the operator is home.

## Log

- 2026-08-23 — created on the operator's military-first reorder; T-HEAL-3a first.
