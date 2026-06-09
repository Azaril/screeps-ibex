# Ibex Review Report — <YYYY-MM-DD>

> Output of the review driven by [`ibex-review-prompt.md`](ibex-review-prompt.md). Fill every section. Keep findings in the uniform block format. Flag-and-track bugs into §7 (don't root-cause here).
>
> **Pre-seeded baseline:** §1, §5, and §7 are pre-populated from the review-kickoff survey and prompt-prep verification. Rows tagged **`[seed]`** are *starting points to confirm, locate precisely, and extend* — **not** final findings; treat survey-sourced items as **Hypothesis** until confirmed. Two items are already **verified** (§1). The review should validate, re-rank, and add to these — not assume the baseline is complete.

## Executive Summary
- **Operator Field Report verdicts (A–H)** — root-mechanism hypothesis each:
  - A — war/squad cohesion (quads scatter): …
  - B — operation/mission lifecycle hangs: …
  - C — CPU pathfinding death-spiral / load-shedding: …
  - D — serialization brittleness: …
  - E — ECS dangling entity refs: …
  - F — job FSM friction: …
  - G — single-creep routing (acceptable?): …
  - H — world renderer corrupts all rendering: …
- **Top 5 must-fix** (severity + one-liner): …
- **3 most fragile subsystems:** …
- **Single biggest rewrite recommendation:** …
- **Competitive verdict:** … *(Does the CPU death-spiral make the current bot non-viable as-is?)*

## 1. Prioritized Findings
<!-- One block per finding. Severity: Critical | High | Medium | Low -->
```
ID:                  IBEX-NN
[SEVERITY]           <one-line title>
Subsystem:           <name from prompt §4, or "cross-cutting">
Location:            <file:line(s)>
Type:                Correctness | Tick-safety | CPU | Persistence/Migration | Architecture | Strategy | Maintainability | Test-gap
Status:              Observed-fact | Hypothesis (confidence H/M/L)
Evidence:            <code reference / load-bearing line>
Impact:              <gameplay/operational consequence>
Recommendation:      <concrete fix, or the exact check to run if Hypothesis>
Breaking-change?:    None | Memory/format | Behavioral
Rewrite-implication: keep-as-is | refactor | replace  (+ alternative, → ../design ADR)
```

> **Two findings already verified during prompt prep** (format exemplars):

```
[INFO]               Spawn-queue priority ordering is CORRECT — do NOT "fix"
ID:                  (n/a — verified correct; guards against re-flagging. See IBEX-022 for the real, separate bug)
Subsystem:           Spawning, stats & helpers
Location:            spawnsystem.rs:85–94 (insert), :236–248 (consume)
Type:                Maintainability (false-positive guard)
Status:              Observed-fact (traced against code)
Evidence:            binary_search_by(|probe| new.priority.partial_cmp(&probe.priority)) keeps the vec DESCENDING (SPAWN_PRIORITY_CRITICAL=100.0 … NONE=0.0); the forward consume loop at :236 spawns highest-first. Insert 0,100,50 → [100,50,0].
Impact:              None — behavior is correct. The real risk is "fixing" the non-idiomatic comparator and INTRODUCING an inversion.
Recommendation:      Do not change ordering. Optional: clarifying comment + unit test (CRITICAL spawns before NONE). Pursue the SEPARATE body-sizing bug (IBEX-022) and NaN-coalescing instead.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

```
[HIGH]               staticmine container resolve().unwrap() — reachable panic
ID:                  IBEX-009
Subsystem:           Jobs layer
Location:            jobs/staticmine.rs:201
Type:                Tick-safety
Status:              Observed-fact (construct verified; reachable)
Evidence:            state_context.container_target.resolve().unwrap() inside `if !displaced` after the transition to Harvest; resolve() → None if the container is destroyed or decays mid-task.
Impact:              Panic in a hot path halts all remaining systems that tick; recurs whenever a miner's container is lost.
Recommendation:      if let Some(c) = …resolve() { … } else { Wait }. Add a test for container-removed-mid-mining.
Breaking-change?:    None
Rewrite-implication: refactor
```

## 2. Per-Subsystem Health (all 12)
| # | Subsystem | Assessment (2–4 sentences) | Biggest single risk |
|---|---|---|---|
| 1 | Tick orchestration & ECS core | | |
| 2 | Memory & serialization | | |
| 3 | Operations (campaigns) | | |
| 4 | Economy & infra missions | | |
| 5 | Combat & expansion missions | | |
| 6 | Jobs layer | | |
| 7 | Military core | | |
| 8 | Room data, visibility & planning | | |
| 9 | Pathing & movement | | |
| 10 | Transfer & market logistics | | |
| 11 | Spawning, stats & helpers | | |
| 12 | Visualization & support/API-fork | | |

## 3. Cross-Cutting Architectural Findings
<!-- layering/coupling, dispatch ordering, persistence/entity model, panic surface, CPU governance, test strategy -->

## 4. Quick-Wins vs. Deep-Refactors
**Quick-wins** (localized, low-risk, don't break the running bot):
- …

**Deep-refactors** (structural; feed the rewrite):
- …

## 5. Risk Register
<!-- Lead with survival/extinction risks: CPU death-spiral (C), deser failure/state loss, segment overflow, hot-path panics -->
| Risk | Subsystem | Likelihood | Impact | Trigger condition | Mitigation |
|---|---|---|---|---|---|
| CPU pathfinding death-spiral → colony collapse `[seed]` | Pathing / cross-cutting | Med–High | **Extinction** | Sustained pathfinding load / low bucket | CPU governor + load-shedding (ADR 0004), plan Increment 1 |
| Deserialization failure → full state loss `[seed]` | Memory & serialization | Med | Critical | Format/field change or corrupt segment | Version header + round-trip/old-snapshot tests; telemetry + intentional reset |
| Segment-55 ECS / cost-matrix collision → corruption `[seed]` | Memory & serialization | Low–Med | Critical | ECS payload grows into segment 55 | Reserve/assert the segment; split (IBEX-013) |
| Reachable hot-path panic → partial tick / cascade `[seed]` | cross-cutting | Med | High | Container lost / invalid TransferTarget | Replace reachable unwraps/panics with handled paths (IBEX-009/010/019) |
| Renderer corruption → no visual debugging `[seed]` | Visualization | High (when enabled) | Med (debugging) | Enable world renderer | Fix visual serialization (IBEX-008) |
| _(review adds more)_ | | | | | |

## 6. Maturity / Score Rubric (1–5)
| Subsystem | Correctness | Robustness (tick/reset) | Performance/CPU | Maintainability | Strategic fitness |
|---|---|---|---|---|---|
| | | | | | |

## 7. Bug & Issue Register (for later individual deep-dives)
<!-- Do NOT root-cause here. Field Reports A–H seed the top rows. -->
| ID | Title | Subsystem | Location | Symptom / observed impact | Status | Suggested validation (repro / test / log) |
|---|---|---|---|---|---|---|
| IBEX-001 | War/squad cohesion: "quads" scatter (Field Report A) | Combat missions / Military core | military/formation.rs, squad.rs, jobs/squad_combat.rs | Creeps don't hold formation; war system untrusted | Observed symptom | Replay engagement; log per-tick member ranges; trace formation→move intents |
| IBEX-002 | Operation/mission lifecycle hangs (Field Report B) | Operations / Combat missions | operations/war.rs, operations/attack.rs, missions/attack_mission.rs | Campaigns stall; no progress or teardown | Observed symptom | Enumerate states+exits; add watchdog/timeout; replay a stuck campaign |
| IBEX-003 | CPU pathfinding death-spiral (Field Report C) | Pathing / cross-cutting | pathing/*, screeps-rover | Bucket exhaustion → tick-restart loop → collapse | Observed symptom | Not reproducible — design load-shedding (ADR 0004); add bucket/restart telemetry |
| IBEX-004 | Serialization brittleness (Field Report D) | Memory & serialization | serialize.rs, memorysystem.rs | Repeated breakage; fragile entity mapping | Observed symptom | Round-trip/fuzz/old-snapshot tests; version header (ADR 0002) |
| IBEX-005 | ECS dangling entity refs (Field Report E) | Tick / ECS core | game_loop.rs (repair_entity_integrity) | Recurring dangling-ref bugs; per-tick repair needed | Observed symptom | Audit entity-ref components; evaluate handles/ID-store (ADR 0001) |
| IBEX-006 | Job FSM friction (Field Report F) | Jobs | jobs/*, screeps-machine | FSM inflexible/opaque; possible double-fire | Observed DX | Pilot BT/utility on one job; compare (ADR 0003) |
| IBEX-007 | Single-creep routing — verify acceptable (Field Report G) | Pathing | movementsystem.rs, screeps-rover | Seems OK but under-tested | Suspected-OK | Add routing tests; confirm it isn't a cohesion contributor |
| IBEX-008 | World renderer corrupts all rendering (Field Report H) | Visualization | visualization.rs, visualize.rs, screeps-visual | Enabling renderer breaks ALL later rendering incl room visuals | Observed symptom | Bisect offending draw call; diff payload vs screeps/engine; check >16KiB / NaN coord |
| IBEX-009 | staticmine resolve().unwrap() reachable panic | Jobs | jobs/staticmine.rs:201 | Panic if container destroyed mid-task | Confirmed (verified) | if-let → Wait; test container-removed path |
| IBEX-010 | Transfer panic! on invalid TransferTarget | Transfer & logistics | transfer/transfersystem.rs:208–289 | panic halts all creeps on a bad target variant | Confirmed exists `[seed]` | Return Result / type-split enums; test invalid variants |
| IBEX-011 | Partial-haul abandon mid-delivery | Jobs / economy | jobs/utility/haulbehavior.rs:513–567 | Haulers strand resources instead of finishing | Suspected `[seed]` | Confirm trigger; test deposit-list pruning; cf. Overmind logistics + "Hauling is NP-hard" |
| IBEX-012 | SquadMember entity refs not repaired pre-serialize | Combat missions | missions/attack_mission.rs, military/squad.rs | resolve() None after member death → heal/retreat breaks | Suspected `[seed]` | Extend repair to SquadContext.members; test post-death |
| IBEX-013 | Segment-55 ECS / cost-matrix collision | Memory & serialization | game_loop.rs COMPONENT_SEGMENTS, costmatrixsystem.rs | ECS growth may clobber cost matrix on segment 55 | Suspected `[seed]` | Add reservation/assert; test large ECS payload |
| IBEX-014 | Deser failure unrecoverable + silent >50KiB drop | Memory & serialization | game_loop.rs:445–449, 508/533 | No recovery/telemetry; silent partial state loss | Confirmed by-design `[seed]` | Add telemetry + segment-fullness watermark |
| IBEX-015 | No job-layer stuck recovery | Pathing / Jobs | movementsystem.rs, jobs/utility/movebehavior.rs | check_movement_failure only reports; no recovery | Confirmed (todo) `[seed]` | Add recovery; test corridor-block cascade |
| IBEX-016 | Unbounded find_route; no global CPU governor | Pathing | screeps-rover find_route; (no governor) | Pathfinding can blow budget (feeds C) | Confirmed gap `[seed]` | Add budget + governor (ADR 0004); test under pressure |
| IBEX-017 | Cost-matrix rebuilt every tick | Pathing | pathing/costmatrixsystem.rs:67 | Ephemeral costs cleared each tick → CPU sink | Confirmed TODO `[seed]` | Cache creep positions N ticks; measure CPU |
| IBEX-018 | Market manipulation guards weak | Transfer / market | transfer/ordersystem.rs:349–351 | Only count/volume/stddev; no trend/spike detection | Suspected `[seed]` | Add time-series guards; test spoofed-price scenario |
| IBEX-019 | Misc hot-path unwraps (some guarded) | cross-cutting | visibilitysystem.rs:362–363, visualization.rs:1016, attack.rs:615 (guarded), RepairQueue NaN | Latent panics / undefined order | Mixed `[seed]` | Harden with if-let; NaN filter; tests |
| IBEX-020 | attack_mission get_room .expect last-resort | Combat missions | missions/attack_mission.rs:1917 | Panic if no home + no owner + no squad entities | Suspected reachability `[seed]` | Determine reachability; fallback vs panic |
| IBEX-021 | War per-tick scans (cadences=1, greedy reassign O(A·H), O(n) is_attacking_room) | Operations | operations/war.rs:1097–1316, :1246 | Heavy per-tick CPU at scale (feeds C) | Suspected `[seed]` | Profile; HashSet lookup; tune cadences |
| IBEX-022 | Spawn body-sizing: min-cost body never enters queue | Spawning | spawnsystem.rs (body-calc), todo.md | Body below min cost never queued (NOT ordering — ordering is correct) | Suspected `[seed]` | Confirm body-calc path; test min-cost body |
| IBEX-023 | Zero automated tests | cross-cutting | whole crate + support crates | No offline validation before deploy | Confirmed `[seed]` | Stand up test harness (plan Increment 0) |
| IBEX-024 | Oversized files | cross-cutting | transfersystem.rs 2439, attack_mission.rs 2040, visualization.rs 1485, war.rs 1444, squad.rs 1021, … | Maintainability / complexity | Confirmed `[seed]` | Decompose during rewrite |
| IBEX-… | _(review adds more)_ | | | | | |

## 8. Rewrite Direction & Architectural Alternatives
<!-- Short ADR-style per pillar + incremental migration path. Promote each to ../design/NNNN-*.md -->
| Pillar | Current → Pain | Recommended direction | ADR |
|---|---|---|---|
| Entity model | specs/ECS → dangling refs (E) | | ../design/0001-entity-model.md |
| Serialization | bincode→segments → breakage (D) | | ../design/0002-serialization.md |
| Behavior modeling | screeps-machine FSM → friction (F) | | ../design/0003-behavior-modeling.md |
| CPU governance | none → death-spiral (C) | | ../design/0004-cpu-governance-and-load-shedding.md |
| Runtime / scheduling | specs dispatch | | ../design/0005-runtime-and-scheduling-model.md |

## 9. Observability & Self-Improvement Plan
<!-- console vs segment telemetry; CPU+intent accounting; death-spiral early-warning → load-shedding trigger; offline feedback loop; colony-health objective function -->
