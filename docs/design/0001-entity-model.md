# ADR 0001 — Entity Model

- **Status:** Proposed
- **Date:** 2026-06-09
- **Related:** Field Report E (ECS dangling-ref bug farm); IBEX-002b (raw-u32 squad-link aliasing), IBEX-005 (`repair_entity_integrity` hand-maintained, no-op default), IBEX-012 (REFUTED — `SquadContext.members`/`heal_priority` ARE repaired), IBEX-047 (economy missions rely on reactive `remove_creep`); review §1, §3, §5, §8 (Entity-model pillar); review prompt §6.1, §6.2, §12. Sibling ADRs: 0002 (serialization), 0003 (behavior), 0005 (runtime/scheduling).

## Context
Current: **specs 0.20 ECS** — one `specs::Entity` per room / creep / operation / mission / squad. Cross-references between components hold `Entity` handles that **dangle** when the referent is removed, a recurring bug source. A per-tick **`repair_entity_integrity`** 5-phase scan exists solely to fix dangling refs before serialization (and `ConvertSaveload` can panic without it). ECS does buy serialization support and Rust-lifetime decoupling between systems.

The review confirmed the mechanism: the durable cross-subsystem key is a **recyclable `specs::Entity` index**, and that single choice is the root of two distinct failure modes (review §1, §8 Entity-model detail):

- **Field Report E / IBEX-005 (confirmed, H).** `repair_entity_integrity` (game_loop.rs:168–369, seven borrow-scoped blocks — the prompt's "5-phase" is loose) exists *only* so `ConvertSaveload` does not panic on a dangling `Entity`. `Mission`/`Operation::repair_entity_refs` defaults to a **no-op** (missionsystem.rs:140), so a newly-added `Entity`-bearing field is silently uncovered. This is a per-tick CPU + maintenance tax that grows with every entity-ref component.
- **IBEX-002b (confirmed mechanism, H; bounded blast radius, M).** The creep→squad link is persisted as a **bare `Entity::id()` u32** (`squad_entity: Option<u32>`, squad_combat.rs:18), resolved via `entities.entity(id)` which re-attaches the *current* live generation at that slot. `JobData` is plain serde, so the marker remapper never touches it and the repair pass *cannot* cover it. After a (frequent) VM reset the squad entity gets a fresh index; a recycled index can host a different `SquadContext`, silently aliasing another squad's orders. Dominant outcome is graceful degradation to solo fallback (squads scatter — Field Report A).

The fix that deletes the entire class is already proven in-tree: key durable refs by **stable game IDs** and rebuild id→`Entity` each tick. `EntityMappingData` (`HashMap<RoomName, Entity>`, entitymappingsystem.rs:7–8) is rebuilt every tick from live rooms (entitymappingsystem.rs:34) and is never serialized; `CreepOwner` stores an `ObjectId<Creep>` (creep.rs:10–11), the engine's own stable key. Neither needs the repair pass.

Two seed concerns are **refuted** and must not be reintroduced as problems: IBEX-012 — `SquadContext.members`/`heal_priority` ARE repaired pre-serialize (game_loop.rs:264–302, plus per-tick prune in squad.rs:959–971); the only residual squad hazard is the IBEX-002b raw-u32 link. IBEX-047 (economy missions' `EntityVec` creep lists relying solely on reactive `remove_creep`) is a *suspected* (M) round-trip hazard, not a confirmed live bug — the stable-ID store closes it as a side effect rather than motivating it.

## Decision
**Key all durable, cross-subsystem references by stable game IDs** — `RoomName`, `ObjectId<_>`, and a minted `SquadId` for ECS-only entities that have no game object — and rebuild the `id → Entity` mapping each tick (the `EntityMappingData` + `CreepOwner` pattern, generalized). A lookup miss becomes a **handled `None`**, not a serialize-time panic. This **deletes `repair_entity_integrity`** outright, closing Field Report E and the IBEX-002b raw-u32 aliasing in one move.

This is an **identity** decision only. Whether `specs` remains the **dispatch** substrate is independent and is deferred to **ADR 0005** (runtime/scheduling); nothing here forces a runtime-model change, and a runtime-model change off `specs` would itself ride on this decision.

### Why a minted `SquadId` and not a `specs::Entity` / saveload marker (the recurring question)

A `specs::Entity` is `{ index, generation }`, **both assigned by the ECS allocator at world-build time** — it is a *runtime handle*, valid only within one VM lifetime, not a durable key. Screeps resets the VM frequently; on each rebuild the allocator hands out fresh indices/generations, so an `Entity` captured before a reset does not denote the same logical object after it. specs' answer is the **saveload marker machinery** (`SimpleMarker`/`SimpleMarkerAllocator`, `serialize.rs:10-14`; `Entity`-bearing components round-trip via `ConvertSaveload<M>`). That machinery works, but it is exactly what forces `repair_entity_integrity` (a `ConvertSaveload` panic on a dangling `Entity` would otherwise skip `serialize_world`) — the per-tick scan + no-op-default trap this ADR exists to delete (Field Report E). And the creep→squad link lives in `JobData`, which is **plain serde, not `ConvertSaveload`** — so it cannot use the marker remapper at all, which is precisely why today's code stores the bare `Entity::id()` u32 and silently aliases a recycled slot (IBEX-002b).

**You do not lose ECS access by storing an id.** The pattern already in-tree is: persist a stable key, resolve the `Entity` each tick. Rooms are referenced by `RoomName` and the `Entity` is rebuilt every tick into `EntityMappingData` (`entitymappingsystem.rs:30-35`); creeps store `ObjectId<Creep>` (`CreepOwner`). Neither persists an `Entity`; neither needs the repair pass; a stale key resolves to a handled `None`, never a foreign object. `SquadId` extends this to the **one entity class with no engine-provided stable key** — a room has a `RoomName`, a creep has an `ObjectId<Creep>`, but a squad is ECS-only with no game object, so its stable key must be *minted*.

Full-`Entity`-plus-markers vs. minted `SquadId`, honestly compared:

| | full `Entity` + saveload markers | minted `SquadId` (chosen) |
|---|---|---|
| Survives reset | yes (marker re-maps on load) | yes (plain id, no remap) |
| Stale/recycled ref | **panics `ConvertSaveload`** → requires `repair_entity_integrity` | handled **`None`** at a validate-on-access seam |
| Per-tick cost | the seven-block repair scan | one `HashMap` rebuild (net-negative — replaces the scan) |
| Usable in `JobData` (plain serde) | **no** — would require converting `JobData` to `ConvertSaveload` | yes, natively (a plain serde value) |
| Type safety | an untyped u64 marker | a distinct type — cannot be confused with a creep/mission/room key |
| Lets the repair pass be **deleted** | no (keeps the whole machinery) | **yes** (A3) |

So `SquadId` is not "instead of the ECS" — it is the **serialization-stable name** for a squad, resolved to an `Entity` (or to `&SquadState` in the store, A2) every tick exactly as rooms already resolve from `RoomName`. Its one genuinely new cost is lifecycle: it must be minted from a monotonic counter persisted with the store and **never recycled** (a recycled id would reintroduce the very aliasing it removes — see Costs).

Migrate squads first (smallest surface, most broken) in three confidence-driven steps:

- **A1 — generation-carrying handle.** Replace the bare-u32 link with a `{ index, generation }` handle resolved through *one* validate-on-access helper, so a stale/recycled slot resolves to `None` instead of silently aliasing. **Breaking change: Behavioral** (interim — closes the aliasing without a full store).
- **A2 — `SquadStore` keyed by a minted `SquadId`.** Squad state lives in a store keyed by a minted, stable `SquadId`; the creep→squad link persists the `SquadId`, not an index/generation. The `id → Entity` (or `id → &SquadState`) map is rebuilt per tick.
- **A3 — mission/operation ownership by id.** Convert mission/operation cross-refs (owner/room/children) to stable ids, **then delete `repair_entity_integrity`** (the pass becomes unreachable once no durable ref is an index).

Cross-ADR ordering (review §8 Sequencing): this work is **Increment 3** (gate: a dangling-ref/restart counter is emitting — see ADR 0004/0006), after the CPU governor + panic containment (Increment 1) and serialization Stage 1 (Increment 2) have landed. A3's pass-deletion completes in **Increment 5**, co-staged with serialization Stage 2 (ADR 0002) so the two intentional one-time resets are confined to low-stakes ticks.

## Alternatives Considered
| Option | Pros | Cons |
|---|---|---|
| Keep specs/ECS, harden `repair_entity_integrity` | least change; keeps decoupling | the dangling-ref *class* persists; per-tick repair cost; still index-based |
| Typed **generational handles** (validate-on-access) | dangling refs become detectable `None`, not silent corruption | manual plumbing; must thread generation checks |
| **Arena / store keyed by stable game IDs** (creep name, room name) | no dangling indices; **deletes the repair pass**; serialization-friendly | ID→object lookups; lifecycle mapping for non-game entities (ops/missions) |
| **Owned-tree** (operations own missions own jobs/squads) | clear ownership & teardown (helps Field Report B) | cross-tree references still need stable IDs |

## Consequences

**Positive**
- **Deletes `repair_entity_integrity` (IBEX-005, Field Report E).** Removes the seven-block per-tick scan (game_loop.rs:168–369) and the no-op `repair_entity_refs` default trap (missionsystem.rs:140). A whole category of "new entity-ref field silently dangles" bugs becomes unrepresentable: there are no durable indices left to dangle.
- **Closes IBEX-002b aliasing.** A1 makes a recycled/stale slot resolve to `None` instead of another squad's `SquadContext`; A2 removes the index from persistence entirely. The dominant cohesion-breaker on the persistence side (Field Report A's serialization root) is eliminated.
- **Tick-safety.** A lookup miss is a handled `None` at a validate-on-access seam, not a `ConvertSaveload` panic at serialize time. Combined with the tick-level panic containment in ADR 0005, this removes one of the named reachable-panic sources that would otherwise abort the tick and skip `serialize_world`.
- **Testability.** Stable string/object ids are trivially constructible in host-target fixtures, where recycled `specs::Entity` slots are not. This enables the round-trip tests the review prioritizes (review §9): kill a squad member / reload a snapshot with a stale creep ref and assert the creep resolves the *same logical* squad (covers the IBEX-012 round-trip and the IBEX-047 stale-`EntityVec` hazard as a side effect).
- **Interaction with ADR 0002.** Once durable refs are stable ids, the serialized payload no longer carries `Entity` wrappers (or the `ConvertSaveload` marker machinery for cross-refs). This is the precondition ADR 0002 §8 calls out for **Stage 2** (the tagged/schema-evolving format swap): a payload of plain ids/strings is far simpler to migrate to a self-describing format than one threaded with index+generation markers. The two are co-sequenced (Increment 5).

**Negative / costs**
- **Per-tick rebuild + lookup cost.** The `id → Entity`/`id → &state` maps are rebuilt each tick (as `EntityMappingData` already is) and resolution is a `HashMap` lookup rather than a direct index. This is bounded (entity counts are small — rooms, squads, ops, missions number in the tens/low hundreds) and is **net-negative CPU** because it replaces the unconditional seven-block repair scan that ran every tick regardless. It must still draw nothing from the pathfinding budget; coordinate the rebuild placement with the CPU governor (ADR 0004) so it sits inside the cheap pre-pass, not a sheddable tier.
- **Minted-id lifecycle.** `SquadId` (and any other ECS-only id) must be minted, persisted, and reclaimed; unlike `RoomName`/`ObjectId` it has no engine-provided source of truth. This is the one genuinely new mechanism; keep it a monotonic counter persisted alongside the store, never a recycled index.
- **Manual plumbing during A1.** The generation-carrying handle requires threading a validate-on-access helper through every squad-link read; this is interim scaffolding superseded by A2's `SquadId` and should not accrete callers.

**New risks**
- **Migration-window correctness.** A1 is Behavioral and A2 introduces a new serialized field (`SquadId`); per AGENTS.md §6 these are the kind of serialized-shape changes that, under positional bincode (ADR 0002), do not round-trip old snapshots. Confine each step's serialized-shape change to a labelled, intentional one-time reset on a low-stakes tick (Increment 3 / Increment 5 per §8 Sequencing); never break the running bot mid-increment.
- **Gating discipline.** A3's pass-deletion is only safe once *no* durable ref is still an index. Gate it on the dangling-ref counter reading zero across a sustained window (the Increment-3 gate emits this signal) before removing `repair_entity_integrity`.

## Incremental Migration Path
Hide the change behind the existing entity-access seam and migrate one ref class at a time, validating with the eval harness (ADR 0006) before each next step. Dropping serialized state is acceptable at a labelled cutover (AGENTS.md §6 — confine to a low-stakes tick).

1. **A1 — generation-carrying squad handle (Behavioral, Increment 3).** Replace `squad_entity: Option<u32>` (squad_combat.rs:18) with a `{ index, generation }` handle behind one validate-on-access helper. Validate: a round-trip that kills a squad member and recycles its slot asserts the creep resolves the *same* logical squad or `None` — never a different squad (closes IBEX-002b aliasing; satisfies the IBEX-012 round-trip recommendation).
2. **A2 — `SquadStore` keyed by minted `SquadId` (Memory/format, Increment 3).** Introduce the store and persist `SquadId` on the creep→squad link; rebuild `id → state` each tick like `EntityMappingData`. Validate via serialization round-trip + behavior replay; cut over on a low-stakes tick.
3. **A3 — mission/operation ownership by id, then delete the pass (Behavioral + pass-deletion, Increment 5).** Convert mission/operation owner/room/children refs to stable ids; once the dangling-ref counter is zero across a sustained window, **delete `repair_entity_integrity`** (game_loop.rs:168–369) and the `repair_entity_refs` trait method (missionsystem.rs:140). Co-stage with ADR 0002 Stage 2 so `Entity` wrappers leave the payload at the same labelled cutover. Validate: full snapshot round-trip with deliberately stale refs deserializes to handled `None`s with no panic and no repair pass.

**Breaking-change summary:** A1 = Behavioral; A2 = Memory/format (one-time reset); A3 = Behavioral + removes the repair pass (co-staged Memory/format reset with ADR 0002 Stage 2).
