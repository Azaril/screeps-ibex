# ADR 0001 — Entity Model

- **Status:** Decided
- **Date:** 2026-06-09
- **Related:** Field Report E (ECS dangling-ref bug farm); IBEX-002b (raw-u32 squad-link aliasing), IBEX-005 (`repair_entity_integrity` hand-maintained, no-op default), IBEX-012 (REFUTED — `SquadContext.members`/`heal_priority` ARE repaired), IBEX-047 (economy missions rely on reactive `remove_creep`); review §1, §3, §5, §8 (Entity-model pillar); review prompt §6.1, §6.2, §12. Sibling ADRs: 0002 (serialization), 0003 (behavior), 0005 (runtime/scheduling).

## Context

The substrate is **specs 0.20 ECS** — one `specs::Entity` per room / creep / operation / mission / squad. Cross-references between components hold `Entity` handles that **dangle** when the referent is removed, a recurring bug source. A per-tick **`repair_entity_integrity`** scan (`game_loop.rs:228`, run at `game_loop.rs:1188`; seven borrow-scoped blocks — the prompt's "5-phase" is loose) exists to fix dangling refs before serialization, because `ConvertSaveload` can panic without it. ECS does buy serialization support and Rust-lifetime decoupling between systems.

The review confirmed the mechanism: the durable cross-subsystem key is a **recyclable `specs::Entity` index**, and that single choice is the root of two distinct failure modes (review §1, §8 Entity-model detail):

- **Field Report E / IBEX-005 (confirmed, H).** `repair_entity_integrity` exists *only* so `ConvertSaveload` does not panic on a dangling `Entity`. `Mission`/`Operation::repair_entity_refs` defaults to a **no-op** (missionsystem.rs:140), so a newly-added `Entity`-bearing field is silently uncovered. That default is a maintenance trap: coverage is hand-maintained rather than structural.
- **IBEX-002b (confirmed mechanism, H; bounded blast radius, M).** The creep→squad link was persisted as a **bare `Entity::id()` u32** (`squad_entity: Option<u32>`), resolved via `entities.entity(id)`, which re-attaches the *current* live generation at that slot. Because `JobData` was plain serde, the marker remapper never touched it and the repair pass *could not* cover it. After a (frequent) VM reset the squad entity gets a fresh index; a recycled index can host a different `SquadContext`, silently aliasing another squad's orders. The dominant outcome is graceful degradation to solo fallback (squads scatter — Field Report A).

The pattern that deletes the entire class was already proven in-tree: key durable refs by **stable game IDs** and rebuild id→`Entity` each tick. `EntityMappingData` (`HashMap<RoomName, Entity>`, entitymappingsystem.rs:7–8) is rebuilt every tick from live rooms (entitymappingsystem.rs:34) and is never serialized; `CreepOwner` stores an `ObjectId<Creep>` (creep.rs:10–11), the engine's own stable key. Neither needs the repair pass.

Two seed concerns are **refuted** and must not be reintroduced as problems: IBEX-012 — `SquadContext.members`/`heal_priority` ARE repaired pre-serialize (game_loop.rs:264–302, plus per-tick prune in squad.rs:959–971); the only residual squad hazard was the IBEX-002b raw-u32 link. IBEX-047 (economy missions' `EntityVec` creep lists relying solely on reactive `remove_creep`) is a *suspected* (M) round-trip hazard, not a confirmed live bug — stable identity closes it as a side effect rather than motivating it.

## Decision

**No durable, cross-subsystem reference is a raw ECS index.** Every such reference is keyed either by a stable game ID — `RoomName`, `ObjectId<_>` — or, for ECS-only entities that have no game object, by a **saveload-marker-converted entity reference** that the specs marker machinery remaps across a VM reload. The `id → Entity` mapping is rebuilt each tick (the `EntityMappingData` + `CreepOwner` pattern, generalized). A lookup miss becomes a **handled `None`**, not a silent alias and not a serialize-time panic.

Concretely, for the one class with no engine-provided key — squads — the creep→squad link is **`squad_entity: EntityOption<Entity>`** (`jobs/squad_combat.rs:29`), which round-trips natively through the `ConvertSaveload`-derived `JobData` marker remapper (serialize.rs). It survives a VM reload *as ECS identity*, with no minted counter, no per-tick `id → Entity` rebuild, and no separate store (`jobs/squad_combat.rs:1518`; `game_loop.rs:713–714` "reload-stable squad identity, native fix").

**`repair_entity_integrity` is retained as the serialize-time backstop.** The marker conversion makes a stale ref a handled `None` on the read side; the retained pass guarantees that no dangling `Entity` ever reaches `ConvertSaveload`, scrubbing a dead/unmarked squad ref to `None` before serialize (`jobs/squad_combat.rs:26`, `1524`, `1633`; `military/squad_manager.rs:1247`). Defence in depth on both sides of the seam is the design; the pass is not scheduled for removal.

This is an **identity** decision only. Whether `specs` remains the **dispatch** substrate is independent (ADR 0005); nothing here forces a runtime-model change. Because identity is solved *natively on specs*, identity supplies no motivation to move dispatch off specs.

### Squad identity: saveload markers vs. a minted `SquadId`

A `specs::Entity` is `{ index, generation }`, **both assigned by the ECS allocator at world-build time** — it is a *runtime handle*, valid only within one VM lifetime, not a durable key. Screeps resets the VM frequently; on each rebuild the allocator hands out fresh indices/generations, so an `Entity` captured before a reset does not denote the same logical object after it. specs' answer is the **saveload marker machinery** (`SimpleMarker`/`SimpleMarkerAllocator`, `serialize.rs:10-14`; `Entity`-bearing components round-trip via `ConvertSaveload<M>`), and the marker `u64` — not the raw `Entity` — is the reset-stable name.

The considered alternative was a **minted `SquadId`** plus a `SquadStore`: a monotonic, never-recycled counter persisted alongside a squad-state store, with the creep→squad link persisting the id and an `id → state` map rebuilt per tick. Its original motivation was that `JobData` was plain serde and therefore could not use the marker remapper at all.

| | saveload markers (chosen) | minted `SquadId` + `SquadStore` |
|---|---|---|
| Survives reset | yes (marker re-maps on load) | yes (plain id, no remap) |
| Stale/recycled ref | handled **`None`** (marker absent) | handled **`None`** at a validate-on-access seam |
| Per-tick cost | none (remap happens at load) | one `HashMap` rebuild every tick |
| Usable in `JobData` | yes — `JobData` derives `ConvertSaveload` | yes, natively (a plain serde value) |
| New persisted machinery | none — reuses what specs already provides | a new id type, a monotonic counter, a store lifecycle |
| Type safety | an untyped `u64` marker under the hood | a distinct type — cannot be confused with a creep/mission/room key |

Converting `JobData` to `ConvertSaveload` dissolves the minted id's whole motivation, and the marker route then closes the same aliasing class with strictly less machinery: no new persisted id type, no monotonic-counter lifecycle, no per-tick map rebuild. The minted id's one genuinely new cost — an id that must never be recycled, because a recycled id reintroduces exactly the aliasing it removes — is a lifecycle the marker allocator already owns. `SquadStore`/`SquadId` is therefore **not part of the design**; residual mentions in `military/objective_queue.rs:30` and `military/squad_manager.rs:24` are aspirational comments with no corresponding types in the tree.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| Keep specs/ECS, harden `repair_entity_integrity` alone | least change; keeps decoupling | the dangling-ref *class* persists; still index-based on the read side |
| Typed **generational handles** (validate-on-access) | dangling refs become detectable `None`, not silent corruption | manual plumbing; must thread generation checks; does not survive a VM reload |
| **Arena / store keyed by a minted stable ID** | no dangling indices; serialization-friendly | a new persisted id type and never-recycle lifecycle to maintain; per-tick ID→object rebuild |
| **Saveload-marker-converted refs** (chosen) | reuses machinery specs already provides; reload-stable; no new persisted id type | requires every ref-bearing component to derive `ConvertSaveload`; needs a serialize-time backstop against dangling refs |
| **Owned-tree** (operations own missions own jobs/squads) | clear ownership & teardown (helps Field Report B) | cross-tree references still need stable IDs |

## Consequences

**Positive**
- **Closes IBEX-002b aliasing.** A recycled or stale slot resolves to `None` instead of another squad's `SquadContext`. The dominant cohesion-breaker on the persistence side (Field Report A's serialization root) is eliminated.
- **Tick-safety.** A lookup miss is a handled `None`, not a `ConvertSaveload` panic at serialize time. Combined with the tick-level panic containment in ADR 0005, this removes one of the named reachable-panic sources that would otherwise abort the tick and skip `serialize_world`.
- **Testability.** Stable keys and marker-converted refs are trivially constructible in host-target fixtures, where recycled `specs::Entity` slots are not. This enables the round-trip tests the review prioritizes (review §9): kill a squad member / reload a snapshot with a stale creep ref and assert the creep resolves the *same logical* squad or `None` (covers the IBEX-012 round-trip and the IBEX-047 stale-`EntityVec` hazard as a side effect).
- **Interaction with ADR 0002.** Durable refs that are stable ids or markers keep the serialized payload free of raw index+generation values. This is the precondition ADR 0002 §8 calls out for the tagged/schema-evolving format swap: a payload of plain ids and marker `u64`s is far simpler to migrate to a self-describing format.

**Negative / costs**
- **Per-tick rebuild + lookup cost** where a stable game ID is the key: the `id → Entity` maps are rebuilt each tick (as `EntityMappingData` already is) and resolution is a `HashMap` lookup rather than a direct index. This is bounded — rooms, squads, ops and missions number in the tens to low hundreds. It must still draw nothing from the pathfinding budget; the rebuild sits inside the cheap pre-pass, not a sheddable tier (ADR 0004).
- **`ConvertSaveload` discipline.** Any component that holds a durable entity ref must derive `ConvertSaveload` and express the ref through the marker-aware wrappers (`EntityOption`/`EntityVec`); a plain-serde component holding an `Entity` is the bug, and it is not caught by the type system.
- **The backstop remains a hand-maintained surface.** `repair_entity_refs`' no-op default (missionsystem.rs:140) means a new `Entity`-bearing field on a mission/operation is uncovered by the pass until someone writes its repair. The marker conversion is what makes that survivable rather than fatal.

**New risks**
- **Serialized-shape changes.** Converting a ref class to markers changes the serialized shape; per AGENTS.md §6, under positional bincode (ADR 0002) that does not round-trip old snapshots. Confine each such change to a labelled, intentional one-time reset on a low-stakes tick; never break the running bot mid-change.
- **False confidence from the backstop.** Because the pass scrubs dangling refs before serialize, a read-side identity bug can hide behind it. Identity correctness must be asserted by round-trip tests, not inferred from the absence of serialize panics.

## Incremental Migration Path

Hide the change behind the existing entity-access seam and migrate one ref class at a time, validating with the eval harness (ADR 0006) before each next step. Dropping serialized state is acceptable at a labelled cutover (AGENTS.md §6 — confine to a low-stakes tick).

1. **Squad link (Memory/format).** Replace the bare `Entity::id()` u32 with a marker-converted `EntityOption<Entity>` and derive `ConvertSaveload` on `JobData`. Validate: a round-trip that kills a squad member and recycles its slot asserts the creep resolves the *same* logical squad or `None` — never a different squad (closes IBEX-002b; satisfies the IBEX-012 round-trip recommendation).
2. **Mission/operation ownership (Behavioral).** Convert mission/operation owner/room/children refs to stable ids or marker-converted refs, so no durable ref is a raw index. Validate: a full snapshot round-trip with deliberately stale refs deserializes to handled `None`s with no panic.
3. **Backstop coverage.** Extend `repair_entity_integrity` alongside each converted class rather than retiring it, so the serialize-time guarantee holds for ref classes that have not yet been converted and for any future ref-bearing field.

**Breaking-change summary:** the squad-link conversion is Memory/format (one-time reset); the mission/operation conversion is Behavioral, co-stageable with ADR 0002's format swap so entity wrappers leave the payload at the same labelled cutover.
