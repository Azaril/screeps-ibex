# Combat Tier −1 Wave B — implementation

**Workstream:** WS-2 · **Advances:** ADR 0008a, ADR 0037, [combat review 2026-07-09](../reviews/combat-systems-review-2026-07-09.md) · **Status:** code-complete — soak pending (B-1)

## Resume point

**CODE-COMPLETE (2026-08-23). All 8 defects fixed + the T1/T2 ruling made. Remaining: the
private-server soak wave once B-1 clears (elevated `Start-Service com.docker.service`), watching
the D4/D5 churn signatures (hold-tick spikes, Loose ratchets after casualties). After a clean
soak these ride the next MMO deploy and this doc is DELETED per the impl-doc lifecycle.**

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
- [x] **Live-adapter gaps** (`1a85a57`) — the sim believes these work; the bot never got them
  - [x] **D9** (`1a85a57`) — the engaged stuck-threshold ladder was never wired into the live bot (zero `stuck_thresholds` hits in `screeps-ibex/src`)
  - [x] **D10** (rover `850a06b`) — rover discards incomplete flee results (`screeps-rover/src/movementsystem.rs:1678`), so retreating creeps freeze under a swarm
- [x] **D28** (`b26eba4`) — an uncontested `Secure` over a hostile-free room can never reach `Resolved`: the terminal requires `engaged_once`, which latches only in-room *with* a focus, and an empty room offers no focus. Let `resolved` fire for an uncontested objective with in-room members, live visibility and zero hostiles.
- [x] **T1/T2 decision: RETAIN, no code change** — the Wave A D27 commit itself documents it (war.rs:550): the kernels stay in `screeps-combat-decision` as sim/test-covered decision code for a future offense-side candidate feed; `tower_danger: 0.0` on the owned path is BY DESIGN (the neighbour-only signal — an owned room's towers are ours). The reconciliation's "orphaned" framing was too strong.

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

- 2026-08-23 — D9 (`1a85a57`: rover `StuckThresholds::engaged()` shared constructor, agent delegation, live wiring at the two mirror sites) + D10 (rover `850a06b`: flee uses partial paths). 2 more RED-verified pins. T1/T2 ruled retained-by-design (war.rs:550 already documented it). CODE-COMPLETE; soak pending B-1.
- 2026-08-22 (late) — D2/D3 (`8fa0c60`), D4/D5/D6 (`be5ce24`), D28 (`b26eba4` + decision/eval
  submodule commits, pushed). 13 RED-verified pins; ibex 327 / decision 345 / eval 114 green; wasm
  clean; determinism fence passes. ADR 0027 amended. Remaining: D9, D10, 0037 decision.
- 2026-08-22 — scoped from the combat review; all eight defects re-verified present at HEAD.
