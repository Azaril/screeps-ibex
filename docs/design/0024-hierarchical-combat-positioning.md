# ADR 0024 — Hierarchical Combat Positioning (strategic goal + threat-aware tactical step)

- **Status:** Proposed
- **Date:** 2026-06-24
- **Deciders:** operator + combat-AI
- **Related:** [ADR 0019](0019-combat-position-selection.md) (the unified position utility this refines), [ADR 0020](0020-ev-adaptive-blob-combat.md), [ADR 0023a](0023a-staged-combat-harness.md) (the harness that surfaced the failures)

## Context

ADR 0019 scores every reachable tile with one utility and the squad **moves onto the single
min-cost tile** (`plan_kite_anchor` → `decision.movement`; `plan_squad_layout` → `member_goals`).
A session of harness work (the staged self-play replays, ADR 0023a) exposed that this single-level
model conflates two different things — *where the squad should ultimately be* and *which tile to
step to this tick* — and that conflation is the root of every positioning defect we chased:

- **Oscillation (period-2 jitter).** The cohesion term measures distance from the **centroid**
  (`cohesion::centroid`, the mean of members' *current* positions — i.e. ~0 steps ahead, where the
  squad *is*). As members step, the centroid follows, so the optimum is never a fixed point → a
  2-tile limit cycle. Measured 18% of moves were ping-pong before any fix.
- **Wandering / arbitrary goals.** The engage preset's proximity term (`w_prox=1.5`) pulls toward
  the focus, but the bounded flood (≤`MAX_KITE_OPS=400`, ~radius 9) can't reach a distant objective,
  so it returns the **flood-edge tile nearest the focus** — a *many-steps-ahead, arbitrary* position
  that jumps as the flood re-centers. In the twin-room assault a creep that **started within range 8
  of the objective** (spawn at W1N1 `(10,25)`, creep at `(5,25)`) moved *away*, crossed into W2N1,
  and oscillated off the map (`roomIdx -1`).
- **Get picked off en route.** Because only the *endpoint* is scored (and the path is the rover's
  threat-blind cost matrix — `CombatCostSource` makes walls/creeps impassable + swamps costly but
  does **not** weight tower/threat exposure), the squad will beeline through a tower's range to reach
  a "safe" tile and die on the way.

Four cohesion variants were tried this session, each regressing something:

| Variant | Result |
|---|---|
| Centroid g-cohesion (ADR 0019 / "A3") | stable + all harness green, but the centroid lags (swamp-approach stalls; designed-2 timed out) |
| Coordinate-step projection (project centroid 1 step / to the mean) | overshoots near the objective → diagonal swing; designed-4 regressed |
| Pure adjacency-to-placed (drop centroid) | first-placed member has **no anchor** → leads freely → block wanders off-map; designed-4 regressed |
| Rigid fixed-offset formation | not adaptive (operator rejected) |

The operator's synthesis: **pick both a tactical "next-step" goal that is locally optimal AND an
N-step "optimal goal" it works toward** — and the centroid being a far/arbitrary anchor is exactly
what we don't want.

## Decision

Adopt a **two-level (hierarchical) positioning model**:

1. **Strategic goal (N-step, stable).** Per member, a *destination*: where it should ultimately
   stand to engage — its weapon-range ring of the focus / the breach tile / the §8 heal-coverage
   slot. Anchored on the **focus/objective** (the real target), not the centroid, and recomputed but
   **stable tick-to-tick** because its anchor barely moves. This is the "optimal goal."

2. **Threat-aware path to it (safest route).** The route from the member to its strategic goal is
   planned over a **threat-weighted cost** (the `ThreatField` folded into the traversal cost), so the
   squad routes *around* tower/enemy coverage to reach the safe location **without getting picked off
   on the way** — "the safest path to the eventual safest location."

3. **Tactical next-step (local, locally optimal).** Each tick a member scores only its **local
   reachable neighborhood** (the tiles it can actually step to — current ± 1), with
   `safety + adjacency-to-placed + heal-coverage(placed) − progress-toward-strategic − incumbency`,
   and moves to the best. It can only take an **incremental** step, always biased toward the stable
   strategic goal — so it can never teleport to an arbitrary flood-edge, and the step is locally safe
   + in formation.

The global flood stops being the goal-picker. It (or a dedicated toward-objective search) computes
the **strategic goal**; the **rover path** (threat-weighted) is the strategic mover; the **local
tactical scan** is the per-tick refinement.

This subsumes the prior terms cleanly:
- **Centroid** → gone; the focus/objective is the strategic anchor and the strategic goal is the
  cohesion reference.
- **Cohesion** → `adjacency-to-placed` (local, self-consistent) + everyone sharing the strategic
  goal keeps the block together.
- **§8 heal coverage** → the healer's strategic slot = max coverage of the *placed/strategic*
  teammate positions (next-tick coverage).
- **Spacing + incumbency** → retained as local tactical terms.

## Why this fixes all four

- **Oscillation:** the tactical step is incremental + incumbency-damped, and the strategic goal is
  a stable fixed anchor (no centroid feedback).
- **Wandering:** a creep takes only local steps toward a *stable* strategic goal — no arbitrary
  flood-edge teleport.
- **Long advance / swamp:** the threat-aware path marches ~1 tile/tick to the strategic goal,
  routing around terrain/threats instead of stalling at a high-cost mouth.
- **Picked off en route:** the path cost is threat-weighted, so it trades a few tiles of detour for
  not crossing a tower's kill-zone.

## Consequences

- **Positive:** one model replaces kite/engage/cohesion/§8/spacing/coordinate-step special cases;
  the path becomes a first-class, threat-aware object; behavior is stable + interpretable (strategic
  goal + a short local scan are both inspectable in replays).
- **CPU:** ~one strategic search per squad (as today) + a tiny per-member local scan (~9 tiles vs
  400) — net **cheaper** than the global per-member flood. The threat-weighted cost adds a per-tile
  lookup to the matrix build (the `ThreatField` already exists as a layer).
- **Negative / risk:** the strategic goal can be *stale* against a fast-moving focus (mitigated by
  recomputing it each tick — it's cheap and the anchor moves slowly); tuning the threat-cost weight
  (too high → cowardly detours, too low → picked off); a member with no reachable progress tile must
  degrade gracefully (hold).
- **No serialized-shape change** in the bot (positioning is computed per tick); WFV unchanged.

## Components (the implementer's contract)

1. **Strategic goal** — `fn strategic_goal(member, focus/objective, role) -> Position`. Reuse the
   existing scored search (`plan_kite_anchor` / a toward-objective `search_scored`) anchored on the
   focus; role sets the desired ring (melee→1, ranged→`r*`, healer→coverage of placed). Stable +
   deterministic.
2. **Threat-weighted path cost** — extend `screeps_combat_agent::pathing::CombatCostSource` (and the
   live cost-matrix recipe) to add a per-tile additive cost from the `ThreatField` (tower falloff +
   enemy ranged/melee stamps), tunable `w_threat_path`. The rover `LocalPathfinder` then yields the
   safest next step for free.
3. **Tactical local scan** — in `plan_squad_layout`, restrict candidates to the member's local
   reachable neighborhood; score `safety + adjacency-to-placed + heal-coverage(placed) +
   progress-toward-strategic − incumbency`; deterministic `(cost, dist-to-strategic, x, y)` tiebreak.
   `progress` = reduction in (threat-weighted) path-distance to the strategic goal.
4. **First-member anchor** — the strategic goal itself (no centroid seed needed); a soloed member
   with no focus holds its tile.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| Keep ADR 0019 single-level (A3) | shipped, all-green, simple | centroid lag (swamp stall), 12–15% residual oscillation in towered fights |
| Coordinate-step centroid projection | self-consistent in unit tests | overshoot/swing near the objective; regressed cross-room |
| Pure adjacency, no anchor | clean, no centroid | unanchored lead member wanders off-map |
| Full N-ply lookahead / rollout | most optimal | far more CPU; over-engineered for a 1-step move decision |

## Incremental Migration Path

Stage behind the existing `member_goals` seam (live + sim already consume it):
1. **Threat-weighted cost** in `CombatCostSource` + a unit test (a path detours around a tower).
   Behaviorally inert until the strategic mover uses it.
2. **Strategic goal** computation (per member, focus-anchored) — assert stability (a fixed-point
   test: members on their strategic goals stay).
3. **Local tactical scan** replacing the global-flood goal pick in `plan_squad_layout`; drop the
   centroid cohesion reference.
4. **Validate** on the harness: `multi_room_assault_crosses_the_border` (designed-4) MUST pass;
   designed-2 (swamp) should resolve; the A-B-A oscillation metric ≤ A3 across designed-0..5 + perms;
   the existing `decide_squad_with_pathing` + layout unit tests updated.
5. Tune `w_threat_path` + the local-scan weights via the existing EXP-*/`SquadTacticParams` sweep.

Breaking changes: none serialized. Each stage is gated on the harness suite (green before/after) +
the oscillation metric.

## Open Questions

- **How far is "N"?** The strategic goal is a *destination*, not a fixed N — the path length is
  whatever the search returns. Do we ever cap the per-tick progress to >1 tile (fast creeps)? (No —
  the engine moves 1 tile/tick; fatigue handles speed.)
- **Threat-cost vs. progress balance** — a creep must sometimes accept some exposure to make
  progress (a fully threat-avoidant squad never closes). The engage gate (ADR 0020 winnability)
  decides *whether* to commit; this ADR decides *how* to route once committed.
- **Multi-room strategic paths** — the rover search is already multi-room; the threat field is
  per-room. Cross-room strategic goals (the twin-room case) need the path cost stitched across the
  seam (ties into the operator's "cross-room edge/flee awareness" follow-up).
