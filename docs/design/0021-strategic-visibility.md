# 0021 — Strategic-room visibility & re-scouting

Status: **Partial — Step 1 landed 2026-06-23; deeper follow-up open.**
Origin: the combat MMO soak (ADR 0020 P3). An injected invader core in a
previously-scouted neutral room (W5N3) was never re-evaluated because the offense
scan reads intel but never asks for a refresh. This note records how visibility
actually works, the one fix that landed, and the deeper work it needs.

## The existing architecture (already good)

Visibility is a **register → broker → fulfill** pipeline. Consumers do not scout;
they declare what they need and how fresh, and the system fulfills it as cheaply as
possible.

- **Broker — `VisibilityQueue`** ([room/visibilitysystem.rs](../../screeps-ibex/src/room/visibilitysystem.rs)). `request(VisibilityRequest::new(room, priority, flags))` upserts/coalesces per room: max priority, OR's the `OBSERVE`/`SCOUT` flags, extends a `DEFAULT_VISIBILITY_TTL = 100`-tick entry. Idempotent — re-requesting the same room is free. Persisted across VM resets. Holds an exponential **unreachable backoff** (`UNREACHABLE_BACKOFF_BASE = 2000` → cap `20000`) that suppresses *scouts* for rooms a scout couldn't reach — but **not observers**.
- **Observer fulfillment — `ObserverSystem`** (same file). Each tick, assigns RCL8 home observers (`OBSERVER_RANGE = 10`, Chebyshev) to `OBSERVE`-flagged entries via `observe_room()`. The planner places one observer per RCL8 room ([foreman utility.rs](../../screeps-foreman/src/layers/utility.rs)). Cheap: no creep, no travel, no spawn throughput.
- **Scout fulfillment — `ScoutOperation`** ([operations/scout.rs](../../screeps-ibex/src/operations/scout.rs)). Spawns scout-creep missions for `SCOUT`-flagged, non-opportunistic, non-unreachable, unclaimed entries — capped at `MAX_SCOUT_MISSIONS = 3`, home within ≤5 (Manhattan). `new_opportunistic()` requests never spawn a mission (idle-scout exploration only).
- **Consumers register** via the `request_intel` pattern. Reference: [salvage.rs](../../screeps-ibex/src/operations/salvage.rs) `request_intel` (derelict confirm/recheck gates, `VISIBILITY_PRIORITY_MEDIUM`). Also `claim.rs` (`refresh_visibility_requests`, CRITICAL for unknowns / HIGH for stale candidates) and `sourcekeeperfarm.rs` (`STRONGHOLD_RESCOUT_INTERVAL = 1500`, LOW). **`war.rs` was the lone consumer that did not register** — fixed in Step 1.

### Mechanism nuance (don't over-claim observer-preference)

`ObserverSystem` runs **late** in the tick (after `UpdateRoomDataSystem`,
`ThreatAssessmentSystem`, the scout-spawn gate, and scout target-pick). So it is
**not** an intra-tick "observer wins before a scout spawns" gate, and the
scout-spawn gate (`has_unclaimed_scout_eligible`) does **not** consult observer
coverage. The real reasons a scout isn't wasted on an observer-covered room:

1. **Intel-freshness loop** — the observer refreshes the room's intel (lands the
   *next* tick), the consumer's staleness check then passes, it stops re-requesting,
   and the entry TTL-expires → no scout.
2. **`OBSERVE`-only flag** — a request without the `SCOUT` flag can *never* spawn a
   scout. This is the only way to *guarantee* no scout for an observer-covered room.

Implication: registering with `ALL` (`OBSERVE|SCOUT`) on a room a scout can't reach
(walled/defended) will still attempt one scout before backoff kicks in. For those
rooms, `OBSERVE`-only is the correct registration — see follow-up.

## Step 1 — landed 2026-06-23

[war.rs](../../screeps-ibex/src/operations/war.rs) offense scan: the stale-threat
branch (`last_seen > 200`) no longer silently `continue`s. It now (a) does the
distance check first, so only **in-range** stale rooms are touched, and (b)
**registers** `VisibilityRequest::new(room, MEDIUM, ALL)` (register, never dispatches
a scout itself), then skips this scan. A core that deployed since our last visit gets
re-evaluated once fresh intel lands; an in-range observer covers it for free.

This is the minimal, correct fix. It is intentionally NOT the full design below.

## Deeper follow-up (open — not done)

1. **A strategic re-scout *scheduler* (the WHAT/WHEN/HOW split).** Today every
   consumer inlines its own cadence; there is no component that owns "refresh these
   strategic rooms every N ticks." Add one (e.g. an `inject_strategic_rescout`
   pass in `ScoutOperation` next to `inject_flag_scout_requests`, or a small
   `StrategicVisibilitySystem`) that walks the hot-tier rooms and re-pushes requests
   on an explicit cadence. Until then, Step 1's `MEDIUM`/`ALL` re-request rides the
   offense-scan cadence + the 100-tick TTL — adequate, not principled.
2. **`OBSERVE`-only for observer-covered rooms (prerequisite, not fast-follow).**
   Add a helper to register/downgrade an entry to `OBSERVE`-only once a room is
   confirmed in range of a live observer, so defended strongholds / walled SK rooms
   never queue a scout at all. Without this, the hot-tier scheduler would burn one
   scout per cycle on unreachable rooms before backoff.
3. **Frequency tiers**, by volatility: **hot** (active war/offense targets, SK rooms
   with a live stronghold, derelict-in-confirm-window) ≈ every 30–50t (under the
   100t TTL); **candidate** (claim, mining-outpost) thousands of ticks — already
   correct; **lazy** (frontier) idle-scout opportunistic only — already correct.
4. **Priority tuning.** Step 1 uses `MEDIUM` (mirrors salvage, doesn't starve
   economy expansion which uses HIGH/CRITICAL). Escalate to HIGH for rooms with an
   *active* offense objective if offense should preempt expansion scouts.
5. **SK "stronghold cleared" probe.** `sourcekeeperfarm.rs` only re-probes while a
   stronghold is *present*; add the symmetric probe so the farm notices when it
   clears (register, don't self-scout).
6. **Observer capability/round-trip caveats.** No feedback loop validates an RCL8
   observer was actually built before marking a room `OBSERVE`-eligible; and an
   observe→intel→threat-update round-trip is ≥2 ticks by game-loop ordering. Keep
   `ALL` flags (not `OBSERVE`-only) until an observer is confirmed in range.

## Cheap-visibility direction

Observers are the cheap path and are already wired; the strategic goal is to lean on
them (RCL8) so creep-scouts become the exception. Power-creep `OPERATE_OBSERVER`
(unlimited-range observe) is **not** integrated (ADR 0013 §6.3) — a future lever for
cross-shard / out-of-range strategic intel without a structure observer.
