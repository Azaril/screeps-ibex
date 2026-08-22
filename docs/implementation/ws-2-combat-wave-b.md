# Combat Tier −1 Wave B — implementation

**Workstream:** WS-2 · **Advances:** ADR 0008a, ADR 0037, [combat review 2026-07-09](../reviews/combat-systems-review-2026-07-09.md) · **Status:** active

## Resume point

**6 of 8 defects fixed and pushed (2026-08-22 late). Next action: D9.**

D9 = wire the engaged stuck-threshold ladder into the live bot. The sim's heal-cluster fix
(`engaged_stuck_thresholds`, the traveller-vs-engaged ladder split in `screeps-rover` /
`squad.rs` per the ADR 0033 slice-7 work) was never wired into the live adapter — zero
`stuck_thresholds` hits in `screeps-ibex/src`. Start by finding the sim's wiring
(`screeps-combat-agent` / rover `MoverConfig`) and mirroring it in the live `MovementSystem`
construction. Then D10: `screeps-rover/src/movementsystem.rs:1678` returns `PathNotFound` for an
INCOMPLETE flee result instead of using the partial path — a retreating creep under a swarm
freezes; use the incomplete path (flee semantics: any distance gained beats standing still).
Then the 0037 T1/T2 orphan decision (wire or delete — see plan).

Method notes that held: every fix lands as a pure kernel + RED-verified pins; revert TEMP-RED
injections with Edit, never `git checkout --` (that discarded uncommitted work once this session).

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
- [x] **D28** (`b26eba4`) — an uncontested `Secure` over a hostile-free room can never reach `Resolved`: the terminal requires `engaged_once`, which latches only in-room *with* a focus, and an empty room offers no focus. Let `resolved` fire for an uncontested objective with in-room members, live visibility and zero hostiles.
- [ ] Decide the fate of the **T1/T2 neighbour kernels** orphaned by Wave A's D27 (`war_decision.rs:182,327` have no non-test callers; `war.rs:531` passes `tower_danger: 0.0`) — wire or delete. Owned by ADR 0037.

## Design deltas

- **D28 written into ADR 0027** (2026-08-22): the Resolved gate's design of record now carries the
  vacuous-clear evidence form, its live-visibility R10 guard, and the is_defend exclusion.
- **Found work (harness):** the lifecycle harness's empty-room scenarios model vision-gap arrivals
  (vacuous_clear stays false); a live-visible-empty D28 scenario (vacuous_clear=true asserting the
  Resolve) would close the loop offline. Small, optional, rides the next harness touch.

## Verification

- Host suite green; wasm build + clippy clean
- Private soak as one wave once WS-1 has the server up — watch for the D4/D5 churn signatures the
  review names: hold-tick spikes and Loose ratchets after casualties
- **R19 does NOT gate this** (tracker RULING-6). R19 gates kernel-*parameter* tuning; this wave is
  safe-mode constants, roster/formation logic and adapter wiring.

## Log

- 2026-08-22 (late) — D2/D3 (`8fa0c60`), D4/D5/D6 (`be5ce24`), D28 (`b26eba4` + decision/eval
  submodule commits, pushed). 13 RED-verified pins; ibex 327 / decision 345 / eval 114 green; wasm
  clean; determinism fence passes. ADR 0027 amended. Remaining: D9, D10, 0037 decision.
- 2026-08-22 — scoped from the combat review; all eight defects re-verified present at HEAD.
