# ADR 0003 — Behavior Modeling (jobs & squads)

- **Status:** Proposed
- **Date:** <YYYY-MM-DD>
- **Related:** Field Report F (FSM friction), Field Report A (squad cohesion); review prompt §6.5, §6.6, §6.7, §12.

## Context
Current: jobs are state machines via **`screeps-machine`** (`MAX_STATE_TRANSITIONS=20`/tick; multi-transition-per-tick). Useful but **inflexible / hard to understand**, and the multi-transition model may underlie double-fire hazards (register-pickup/deposit). Squad/combat behavior coordinates poorly (creeps scatter instead of forming quads; cohesion requires staying **in range** to act on teammates). Prior art: Overmind's `CombatOverlords` / swarm cohesion (`../references/external-references.md`).

## Decision
<TBD after review.>

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep `screeps-machine`, simplify transition semantics | least change | friction & opacity largely remain |
| **Behavior trees** | composable, debuggable, reactive | new framework; per-tick eval cost |
| **Utility AI** (score actions) | flexible priorities; good for economy/target choice | tuning; less explicit control flow |
| **Data-driven / declarative FSM** | clarity; testable transition tables | expressiveness limits |

**Squad cohesion** (Field Report A): explicit **lead-follower with hard in-range wait-gates**, or **single "fat-position" group movement**, with cohesion as an invariant.

## Consequences
<TBD.>

## Incremental Migration Path
<e.g. pick one job (or the squad layer) as a pilot behind the Job/Squad trait seam; A/B against the FSM version via replay before broader rollout.>
