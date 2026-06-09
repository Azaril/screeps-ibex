# Ibex Review Report — <YYYY-MM-DD>

> Output of the review driven by [`ibex-review-prompt.md`](ibex-review-prompt.md). Fill every section. Keep findings in the uniform block format. Flag-and-track bugs into §7 (don't root-cause here).

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
| | | | | | |

## 6. Maturity / Score Rubric (1–5)
| Subsystem | Correctness | Robustness (tick/reset) | Performance/CPU | Maintainability | Strategic fitness |
|---|---|---|---|---|---|
| | | | | | |

## 7. Bug & Issue Register (for later individual deep-dives)
<!-- Do NOT root-cause here. Field Reports A–H seed the top rows. -->
| ID | Title | Subsystem | Location | Symptom / observed impact | Status (suspected/confirmed) | Suggested validation (repro / test / log) |
|---|---|---|---|---|---|---|
| IBEX-?? | | | | | | |

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
