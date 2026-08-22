# Combat Tier −1 Wave B — implementation

**Workstream:** WS-2 · **Advances:** ADR 0008a, ADR 0037, [combat review 2026-07-09](../reviews/combat-systems-review-2026-07-09.md) · **Status:** active

## Resume point

**Not started.** Parked behind WS-1 by the one-workstream policy. Nothing is blocking it
technically — no WFV bump, no reset, no Docker dependency for the code work itself (only the soak
would want a server).

Start with D2 + D3: two small, self-contained fixes to `screeps-ibex/src/missions/safe_mode.rs`
that protect a scarce irreversible resource. All eight defects below were re-verified present at
HEAD on 2026-08-22.

## Target

Close the 2026-07-09 combat review's Tier −1 as a live document. Wave A (`ab692bd`, deployed
2026-07-28) closed D1/D11/D24/D25/D26/D27/R22; this is the remainder of the recommended first wave
plus the residual D28 that Wave A's own verification surfaced.

## Plan

Grouped so each group can soak as one increment. **No WFV bump on any of these.**

- [x] **Safe-mode pair** (`8fa0c60`) — protects an irreversible resource, two-line class fixes
  - [x] **D2** — `CRITICAL_STRUCTURE_MIN_HITS = 5000` (`safe_mode.rs:19`) equals spawn max hits, so any spawn scratch arms the trigger
  - [x] **D3** — the `activated` latch (`safe_mode.rs:41,217`) is permanent and has no reset path, so a room auto-safe-modes at most once ever
- [x] **Roster-churn cluster** (`be5ce24`) — breaks the casualty cycle every fight exercises
  - [x] **D4** — mid-fight slot refill re-anchors an engaged skirmish squad, disabling kiting for the whole replacement window
  - [x] **D5** — formation layout only ever shrinks (`squad.rs:831`, `living_count < slot_count`), so a refilled seat resolves to offset `(0,0)` — a phantom seat stacked on the anchor
  - [x] **D6** — Phase C counts engaged, battle-damaged squads as "forming", silently turning `MAX_FORMING_SQUADS` into an offense-concurrency reducer
- [ ] **Live-adapter gaps** — the sim believes these work; the bot never got them
  - [ ] **D9** — the engaged stuck-threshold ladder was never wired into the live bot (zero `stuck_thresholds` hits in `screeps-ibex/src`)
  - [ ] **D10** — rover discards incomplete flee results (`screeps-rover/src/movementsystem.rs:1678`), so retreating creeps freeze under a swarm
- [ ] **D28** — an uncontested `Secure` over a hostile-free room can never reach `Resolved`: the terminal requires `engaged_once`, which latches only in-room *with* a focus, and an empty room offers no focus. Let `resolved` fire for an uncontested objective with in-room members, live visibility and zero hostiles.
- [ ] Decide the fate of the **T1/T2 neighbour kernels** orphaned by Wave A's D27 (`war_decision.rs:182,327` have no non-test callers; `war.rs:531` passes `tower_danger: 0.0`) — wire or delete. Owned by ADR 0037.

## Design deltas

None yet. D28's fix is a completion-rule change and should be written into ADR 0027 (objective
lifecycle) when it lands, not left only here.

## Verification

- Host suite green; wasm build + clippy clean
- Private soak as one wave once WS-1 has the server up — watch for the D4/D5 churn signatures the
  review names: hold-tick spikes and Loose ratchets after casualties
- **R19 does NOT gate this** (tracker RULING-6). R19 gates kernel-*parameter* tuning; this wave is
  safe-mode constants, roster/formation logic and adapter wiring.

## Log

- 2026-08-22 — scoped from the combat review; all eight defects re-verified present at HEAD.
