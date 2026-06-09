# ADR 0001 — Entity Model

- **Status:** Proposed
- **Date:** <YYYY-MM-DD>
- **Related:** Field Report E (ECS dangling-ref bug farm); review prompt §6.1, §6.2, §12.

## Context
Current: **specs 0.20 ECS** — one `specs::Entity` per room / creep / operation / mission / squad. Cross-references between components hold `Entity` handles that **dangle** when the referent is removed, a recurring bug source. A per-tick **`repair_entity_integrity`** 5-phase scan exists solely to fix dangling refs before serialization (and `ConvertSaveload` can panic without it). ECS does buy serialization support and Rust-lifetime decoupling between systems.

## Decision
<TBD after review.>

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep specs/ECS, harden `repair_entity_integrity` | least change; keeps decoupling | the dangling-ref *class* persists; per-tick repair cost; still index-based |
| Typed **generational handles** (validate-on-access) | dangling refs become detectable `None`, not silent corruption | manual plumbing; must thread generation checks |
| **Arena / store keyed by stable game IDs** (creep name, room name) | no dangling indices; **deletes the repair pass**; serialization-friendly | ID→object lookups; lifecycle mapping for non-game entities (ops/missions) |
| **Owned-tree** (operations own missions own jobs/squads) | clear ownership & teardown (helps Field Report B) | cross-tree references still need stable IDs |

## Consequences
<TBD.>

## Incremental Migration Path
<e.g. introduce a handle/store abstraction behind the current entity-access seam; migrate one component type at a time; validate via serialization round-trip + behavior replay. Dropping serialized state is acceptable at cutover.>
