# 0009c — Room Planner: Guaranteed Road Connectivity

**Status:** Accepted + Implemented 2026-06-22 (operator sign-off; landed on master, build + clippy + 30 tests green)
**Addendum to:** [0009](0009-room-planning-and-multiroom-layout.md), [0009a](0009a-room-planner-performance.md), [0009b](0009b-room-planner-scoring-and-evaluation.md)
**Relates to:** road-connectivity review (this session); future low-MOVE base-ops bodies (out of scope here)
**Driver:** William Archbell
**Date:** 2026-06-22

> **Design posture (operator, 2026-06-22):** target the *correct end-state* even where it breaks serialization. The `Plan` struct gains fields (explicit approach tiles, real road score fields); migration is a **loud full reset** (`WORLD_FORMAT_VERSION` 13 → 14), superseding the earlier "let it ride." Acceptable pre-MMO under the reset-anytime policy (EP-5.1).

> **As-built (2026-06-22).** Implemented as specified, with two simplifications found during implementation and one consolidation:
> - **D5 (extension stamp connector → road-BFS) was not needed** and was skipped. D3's endpoint resolution routes the all-buildings pass to *every* must-connect structure (including extensions) and traces islanded fragments back onto the trunk over plains/swamp, so extensions are reconnected by construction regardless of the stamp connector. The extension layer is unchanged.
> - **D7 (prune hardening) was not needed** and was skipped. `RoadPruneLayer`'s redundant-removal already uses road-BFS and protects every structure that has a baseline adjacent road (which, post-D3, is all must-connect structures); dead-end pruning never touches multi-tile connector interiors; and the D6 gate runs after prune as the backstop. Verified, no change required.
> - **`road_upkeep` was consolidated into the existing `upkeep_cost`** rather than a separate `PlanScore` field: `UpkeepScoreLayer` now counts all roads (plain ×1, swamp ×3). `PlanScore` thus gains `road_transport` + `road_connectivity` (and drops `traffic_congestion`); `RoadScoreLayer` pushes `road_transport`.
> - **D6 score weights (as-built):** `road_connectivity` is pushed with weight **0** at the tight tiers (reporting only — the hard reject enforces connectivity) and weight **2000** (dominant; `> must_connect_count × Σ other weights`) at the unbounded tier so it is lexicographically primary there. In fully-connectable rooms the assumed-1.0 connectivity term cancels between the optimistic search bound and the connected best, so admissible pruning is unaffected.
> - **D9 (no road/wall overlap / tunnel avoidance)** was added during this work after an engine check confirmed road+wall = 150× upkeep tunnel — see the D9 decision below.
> - **Files:** `plan.rs`, `layer.rs`, `pipeline/{finalize,analysis}.rs`, `planner.rs` (boxed `PlanResult::Complete`), `layers/{road_network,road_connectivity,road_score,upkeep_score,utility,spawn,defense,mod}.rs`, `screeps-ibex/src/game_loop.rs` (WFV 13→14), `screeps-prospector/src/score.rs` (deref boxed Complete). Tests: 4 gate tests + 1 end-to-end reconnection test + 1 road/wall-overlap regression test.

---

## Context

The room planner's only enforced connectivity invariant is **walkable**-reachability, never **road**-reachability. Every connectivity check floods over plains + swamp + roads alike:

- `ReachabilityLayer` accepts a structure if **one** 8-neighbour is walkable within `chebyshev*2 + 8` ([reachability.rs:94](../../screeps-foreman/src/layers/reachability.rs#L94), [:162](../../screeps-foreman/src/layers/reachability.rs#L162)); a bare plain/swamp tile passes, and the detour BFS is terrain-agnostic.
- `RoadPruneLayer.walkable_bfs` and `ExtensionLayer.walkable_bfs` are walk-only ([road_prune.rs:374](../../screeps-foreman/src/layers/road_prune.rs#L374), [extension.rs:448](../../screeps-foreman/src/layers/extension.rs#L448)).
- The `all_buildings` road pass routes to each building's **own tile** ([road_network.rs:310](../../screeps-foreman/src/layers/road_network.rs#L310)), but `hub_road_tree` treats any solid-structure tile as impassable (`if has_other { continue; }`, [road_network.rs:397](../../screeps-foreman/src/layers/road_network.rs#L397)), so the destination is silently skipped ([road_network.rs:170](../../screeps-foreman/src/layers/road_network.rs#L170)). The only walkable destination it actually serves is the mineral container. The documented purpose — "roads to all remaining interactable buildings… so towers get refuel roads" ([mod.rs:71-75](../../screeps-foreman/src/layers/mod.rs#L71)) — is **not achieved**.

Consequently the planner can ship layouts whose roads form **disconnected islands** (an extension stamp separated from the hub trunk by a plains gap; a tower with no routed refuel road). Per [0009](0009-room-planning-and-multiroom-layout.md)/[0009a](0009a-room-planner-performance.md) this was a *deliberate, documented* tradeoff: "roads are an optimization over walkable terrain, not a connectivity guarantee."

**Why this must change now.** Today every energy-logistics body is 1 CARRY : 1 MOVE ([haul.rs:255](../../screeps-ibex/src/missions/haul.rs#L255)), break-even on plains, so a road gap over plain costs **zero** extra ticks — the defects are a swamp-only tax. We intend to move base-operations creeps to **low-MOVE / road-optimized** bodies (more CARRY/WORK per part budget for higher EV). That EV *presupposes road travel* (EP-7.1 engine mechanics: move cost road 1 / plain 2 / swamp 10 per loaded non-MOVE part; each MOVE removes 2 fatigue/tick). For a canonical road-optimized 2:1 hauler:

| Body | Capacity | Road | Plain | Swamp |
|---|---|---|---|---|
| 1:1 (24C/24M, today) | 1200 | 1 t/tile | **1 t/tile (free)** | 5 t/tile |
| 2:1 road-optimized (32C/16M) | 1600 (1.33×) | 1 t/tile | **2 t/tile (2×)** | 10 t/tile |

A 2:1 hauler beats a 1:1 only when its route's off-road fraction `f < ~1/3`; any swamp counts ~10× and flips it sooner. Worse, hauler **sizing** is road-assuming and terrain-blind (`estimate_travel_ticks` hardcodes `MOVE_COST_ROAD = 1`, [body_helpers.rs:109](../../screeps-ibex/src/missions/localsupply/body_helpers.rs#L109); count uses Manhattan distance, [haul.rs:264](../../screeps-ibex/src/missions/haul.rs#L264)), so an off-road route runs slower **and** gets ~`(1+f)`× too few haulers — a silent, compounding shortfall.

**Decision:** make a single hub-connected road network a *planner invariant* — solved entirely inside the planner, with each interactable structure's road **approach tile modelled explicitly on the `Plan`**, validated by a hard gate that degrades gracefully — so low-MOVE base ops can later be deployed on the guarantee that every base logistics route is fully roaded, and consume the stored approach tiles directly.

### Prior art / constraints absorbed

- **"Route to building-adjacent tiles was inert" ([0009a:163](0009a-room-planner-performance.md)).** This endpoint change was tried in Phase 2 and reverted — because the rejection gate is walkable-BFS, so moving road endpoints changed nothing it measured, and ~89% of reachability rejections were *placement* bugs (Factory/PowerSpawn boxed in by `UtilityLayer` with no access check), not routing. **Lesson: the road-BFS gate (D6) is load-bearing; the routing fix (D3) is inert without it; and the placement fix (D4) is required or the gate just relocates the failure.**
- **No-plan-loss is a hard invariant (EP-6.8).** Escalation only widens the anchor beam (`ESCALATION_BEAMS = [16, 64, usize::MAX]`, [planner.rs:158](../../screeps-foreman/src/planner.rs#L158)); there is no constraint-relaxation tier. A naive hard-reject on road-disconnection would make a genuinely un-road-connectable room **permanently Failed + backoff** (2000–32000 ticks). The gate must therefore relax at the final tier.
- **Determinism.** Any new BFS/Dijkstra must tie-break on `Location::packed_repr()` (the search compares plan fingerprints, [search.rs:352](../../screeps-foreman/src/search.rs#L352)).
- **Engine mechanics (EP-7.1).** Roads coexist with `Container`/`Rampart` (road sits on the tile) but not with solid buildings; the `hub_road_tree` cost model already encodes exactly this eligibility ([road_network.rs:381-418](../../screeps-foreman/src/layers/road_network.rs#L381)).

---

## Decision

Eight parts, all inside `screeps-foreman`. D6 (the gate) is load-bearing; D2 (data model) is the structural cleanup that makes everything else exact; D3–D5 make the gate satisfiable; D7 keeps it satisfied through pruning; D8 is the scoring that shapes *which* connected network wins.

### D1 — Invariant and structure tiers

A plan is **road-connected** iff every *must-connect* structure has a stored **approach tile** (D2) that is a `Road` (or road-bearing `Container`/`Rampart`) in the single connected road component containing the **hub** landmark.

Three tiers (mirroring `ReachabilityLayer`'s existing mineral-area skip):

- **Must-connect (critical, hard at all tiers):** source containers, controller container/approach. Already routed from the hub by construction ([road_network.rs:258-265](../../screeps-foreman/src/layers/road_network.rs#L258)) — the hard check here is a near-free safety assertion.
- **Must-connect (graceful):** hub cluster (storage/link/terminal/spawns/hub-extension — served by the required hub-centre road), all extensions, all towers, extra spawns, labs, and `UtilityLayer` buildings (observer/factory/power-spawn/nuker). The real exposure.
- **Optional (never gates):** mineral extractor + container (extraction is optional); controller upgrade-parking slots (road-averse, +200 — only the upgrade `approach` is must-connect); source/controller **links** (solid, validated *via* their adjacent container's road — no creep stands at a link); static-miner harvest-stand tiles (travel-once-then-park; best-effort).

### D2 — Explicit approach-tile model on the `Plan` *(Plan-shape change, C3 — the prize)*

Generalize the existing `spawn_approaches: Vec<Location>` ([plan.rs:290](../../screeps-foreman/src/plan.rs#L290), itself added in a prior WFV bump 6→7) into a first-class **per-structure approach map**: for every must-connect interactable structure, store the on-network road tile a creep stands on to interact with it.

- Add `approach_tiles: Vec<(Location, Location)>` (structure-tile → approach-road-tile) to `Plan` ([plan.rs:260](../../screeps-foreman/src/plan.rs#L260)). Keep `spawn_approaches` or fold it in.
- Today this approach tile is *computed* during routing (D3) and *discarded*; storing it makes it the single source of truth.
- **Consumers:** the gate (D6) validates stored approach tiles exactly (no adjacency re-derivation); haulers/fillers — especially future low-MOVE bodies — path to a known on-network stand tile (deterministic, never off-road).

This is the structural reason the rest is clean: "approach tile" stops being an implicit, thrice-recomputed concept and becomes Plan output.

### D3 — Connected-network construction: route to (and store) road-eligible approach tiles

Change destination **selection** only (the search algorithm is unchanged — feature modules supply pricing policy, algorithms live in the pathfinder):

- Change `hub_road_tree` to return `(parent, dist)` (the internal `dist` already settles a finite value on every reachable road-eligible tile, [road_network.rs:420-426](../../screeps-foreman/src/layers/road_network.rs#L420); only `parent` is currently returned, [:430](../../screeps-foreman/src/layers/road_network.rs#L430)).
- For each must-connect **solid** building, instead of its own (impassable) tile, scan its 8 neighbours, keep road-eligible ones with finite `dist`, pick `argmin(dist)` (tie-break `packed_repr`), **store it as the approach tile (D2)**, and trace it back to the hub, placing roads along the trace (existing placement loop at [road_network.rs:189-241](../../screeps-foreman/src/layers/road_network.rs#L189) already handles empty / container / rampart tiles).
- All traces share one hub-rooted shortest-path tree and existing-road tiles cost 0 → the union of placed roads is a **single connected component including the hub** (connectivity by construction); islanded stamp fragments merge onto the trunk automatically.

Closes the towers/utility gap. **Inert on its own** (prior art) — meaningful only paired with D6. **Perf:** neutral-to-faster (the `all_buildings` flood already runs to completion today since its impassable own-tile targets never finalize, [road_network.rs:362-374](../../screeps-foreman/src/layers/road_network.rs#L362); switching to road-eligible targets lets early-termination fire). Keep `infrastructure()`'s few-target early-term intact.

### D4 — Upstream placement fix for solid buildings (the 89%)

`UtilityLayer` (observer/factory/power-spawn/nuker) and `SpawnLayer` (extra spawns) must, when choosing a tile, require ≥1 road-eligible 8-neighbour reachable in the structure-aware sense (mirror `controller_infra`'s `approach` reservation, [controller_infra.rs:166-174](../../screeps-foreman/src/layers/controller_infra.rs#L166)) and prefer tiles adjacent to the trunk. This is the root cause of ~89% of historical reachability rejections ([0009a:163](0009a-room-planner-performance.md)); fixing it is what makes the gate cheap.

### D5 — Extension per-stamp connector: road-BFS aware

Switch the per-stamp connectivity test and `has_adjacent_reachable_road` fillability test from `walkable_bfs` to **road-BFS** ([extension.rs:359-380](../../screeps-foreman/src/layers/extension.rs#L359), [:448](../../screeps-foreman/src/layers/extension.rs#L448)), so a stamp that is walk-reachable over plains but not *road*-connected gets a connector laid via the existing `pathfind_road_connection` ([extension.rs:524](../../screeps-foreman/src/layers/extension.rs#L524)). Store each kept extension's approach tile (D2).

### D6 — The gate: a dedicated `RoadConnectivityLayer` *(C1 — own layer, single responsibility)*

Add a new validation layer (not bolted onto `ReachabilityLayer`), inserted after `RoadPruneLayer` and `ReachabilityLayer` (it sees the final pruned network). It is **not** a fingerprint contortion — it carries its own name `"road_connectivity"`; renaming/adding layers only invalidates in-flight `PlanningState`, never committed plans.

It BFS-floods from the hub over `Road`/`Container`/`Rampart` tiles (deterministic tie-break) and computes `connected_fraction` = (must-connect structures whose stored **approach tile** is in the hub road component) / (total must-connect). Behaviour parameterized by a `require_connectivity: bool` field set from the beam:

- **Critical tier (always):** any critical structure (source/controller container) not road-connected → `Some(Err(()))`. Near-free; should never fire.
- **`require_connectivity == true` (beams 16, 64):** `connected_fraction < 1.0` → `Some(Err(()))` (hard reject → search backtracks → driver escalates the beam).
- **`require_connectivity == false` (final `usize::MAX` beam):** never reject; `push_score("road_connectivity", connected_fraction, W_CONN)` with a **dominant** weight (≫ Σ other weights ≈ 13, default 25). Most-connected achievable plan wins; a plan is never lost.

Wire the flag in `default_layers_with_beam(anchor_beam)` ([mod.rs:92](../../screeps-foreman/src/layers/mod.rs#L92)): `require_connectivity = anchor_beam != usize::MAX`. Same name across beams → fingerprint stable across the escalation ladder. This gives a **true hard guarantee whenever the room is road-connectable at beam ≤ 64** (essentially always, given D3–D5) and graceful best-effort only in pathological rooms reaching the unbounded tier — satisfying "hard-reject and escalation" while preserving no-plan-loss (EP-6.8).

### D7 — Pruning safety

`RoadPruneLayer` runs before the gate, so the gate is a backstop, but prune should preserve connectivity by construction:

- Its redundant-prune already uses road-BFS over all interactable structures ([road_prune.rs:156-238](../../screeps-foreman/src/layers/road_prune.rs#L156)). After D3 every must-connect structure has a baseline adjacent road → protected.
- **Harden** `best_adjacent_road_dist`: treat "had a road at baseline, lost all road access after removal" as a hard violation (reject the removal) rather than relying on `unwrap_or(u32::MAX) <= baseline` ([road_prune.rs:235-238](../../screeps-foreman/src/layers/road_prune.rs#L235)).
- Dead-end prune is safe for connectors (interior tiles have 2 road neighbours; endpoints are structure-adjacent and protected).

### D8 — Scoring with real `PlanScore` fields *(Plan-shape change, C2)*

Add proper named fields to `PlanScore` ([plan.rs:118-132](../../screeps-foreman/src/plan.rs#L118)) and match arms in `to_plan_score` ([layer.rs:228-241](../../screeps-foreman/src/layer.rs#L228)) — visible in plan inspection/HUD/debug instead of folded into `.total` — and **delete the dead `traffic_congestion` field** (declared, never populated):

1. **`road_transport`** (new layer after roads, weight ~1.0 — primary-objective tier): reward short **road-path** distances from hub to logistics endpoints (source containers, controller approach, mineral container, extension-field worst/centroid) via structure-aware `hub_pathing_distances` ([layer.rs:197](../../screeps-foreman/src/layer.rs#L197)); normalize `1 - dist/max` as `anchor_score` does. The low-MOVE EV metric (ticks-per-trip). Optionally mirror into `composite_anchor_score` ([anchor_score.rs:36-164](../../screeps-foreman/src/layers/anchor_score.rs#L36)) so it influences anchor pre-ranking too (the file warns the two must stay in sync).
2. **`road_upkeep`** (extend `UpkeepScoreLayer`, weight 0.5): today counts only **swamp** roads ([upkeep_score.rs:60-66](../../screeps-foreman/src/layers/upkeep_score.rs#L60)); count **all** roads (plain ×1, swamp ×3). `road_transport` (wants short) and `road_upkeep` (wants few) balance to compact spanning networks.
3. **`road_connectivity`** — the field the D6 score populates; now reported, not just summed.

All pushed values clamped to `[0,1]` (the admissible-pruning bound at [search.rs:266-290](../../screeps-foreman/src/search.rs#L266) requires it).

### D9 — No road/wall overlap (tunnel avoidance)

A road that shares a tile with a wall is a **tunnel**: the engine taxes it at `CONSTRUCTION_COST_ROAD_WALL_RATIO = 150` for **both** build cost and decay/repair — triggered by natural wall terrain **or a `wall` object** on the tile ([roads/tick.js:16-18](../../c/code/screeps-engine/src/processor/intents/roads/tick.js), [build.js:180](../../c/code/screeps-engine/src/processor/intents/creeps/build.js)). `ROAD_HITS=5000`, `ROAD_DECAY_AMOUNT=100`/`ROAD_DECAY_TIME=1000` → a tunnel is 45,000 energy to build and 15,000 hits/1000t to repair (150× upkeep, same lifetime).

Audit result: roads are never placed on natural wall terrain (every road placer skips `terrain.is_wall`). The one unguarded overlap was `DefenseLayer.classify_wall_rampart` ([defense.rs](../../screeps-foreman/src/layers/defense.rs)) assigning walls by checkerboard parity without checking for a road already on the tile — an infrastructure road (placed at layer 14, before defense at 17) on a min-cut tile could get a `Wall` on top, severing the crossing and creating a tunnel.

**Decision:** a cut tile carrying a road is always a **rampart**, never a wall (ramparts seal against enemies but stay passable and decay at ×1). Implemented in `classify_wall_rampart` (takes the road-tile set, forces those cut tiles to ramparts) with a regression test. Zero defensive downside — the safety pass already guarantees walls have adjacent ramparts.

### Out of scope (explicitly deferred)

- **Extension RCL / source-distance placement** ([0009b](0009b-room-planner-scoring-and-evaluation.md) §5/§6) — separate work.
- **Low-MOVE / road-optimized bodies + terrain-aware hauler sizing** (consumer side) — the follow-up this ADR unblocks; will consume D2's approach tiles.
- **Inter-room / remote roads** ([0009 D3](0009-room-planning-and-multiroom-layout.md), still Proposed) — `RoomGraph` etc.

---

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| **A. Hard-reject only (no relaxation tier)** | Simplest; true guarantee | Violates no-plan-loss (EP-6.8): an un-road-connectable room is permanently Failed + backoff. Rejected. |
| **B. Soft penalty only** | Zero plan-loss risk | Only a preference — a pathological score mix could let a disconnected plan win when a connected one exists. Weaker than required. |
| **C. Hard at tight beams + graceful penalty at unbounded (D6, chosen)** | True guarantee whenever achievable; graceful otherwise; fingerprint-stable across the ladder; matches "hard-reject and escalation" | `W_CONN` must dominate so connected always out-scores partial. |
| **D. New explicit constraint-relaxation escalation tier** | Most explicit "try hard, then relax" | New plumbing through `default_layers_with_beam` + driver; C achieves the same on the existing beam axis. |
| **E. Approach-tile routing, no gate** | Tiny change | Already tried and reverted as inert ([0009a:163](0009a-room-planner-performance.md)). Rejected. |
| **F. Plan-shape-neutral (scores in `.total`, no approach model, gate inside ReachabilityLayer)** | "Let it ride" — no reset, no churn | Approach tile stays implicit/recomputed; scores not reportable; gate conflated with walkable reachability. Rejected in favour of the correct end-state. |

---

## Consequences

**Positive**
- A single hub-connected road network becomes a planner invariant — the prerequisite for low-MOVE base-ops EV — and eliminates the road-island bug class for all creeps.
- Approach tiles are first-class Plan output: exact validation, deterministic consumer pathing, low-MOVE-ready.
- Towers/utility get real refuel roads (R1/R2); islanded stamps get connectors (R12/R13); the validation gap (R6/R11/R14) is closed by a hard gate with a regression-proof relaxation.
- `road_transport` + `road_upkeep` steer the search to compact minimal networks (lower upkeep, shorter low-MOVE trips); dead `traffic_congestion` removed; road scores reportable.

**Negative / risks**
- More road tiles in some rooms → more construction energy + 1 energy/tick decay + repair CPU. Mitigated by `road_upkeep` and trunk reuse (cost-0 merge).
- Stricter gate → more rejections at beams 16/64 → more escalation (CPU). Mitigated: the BFS is ~O(tiles); D4 removes the dominant rejection cause; validate against constrained/swampy rooms (EP-6.11).
- **Full reset on deploy** (migration). Accepted pre-MMO.

**Validation (EP-6.11, EP-7.1)**
- `screeps-foreman` test: all placed `Road` tiles form a single hub-connected component under road-BFS, on synthetic + worst-case terrains (swamp-banded, pinched, multi-source). The `road_bfs` primitive exists ([road_prune.rs:267](../../screeps-foreman/src/layers/road_prune.rs#L267)).
- Test: every must-connect structure's stored approach tile is in the hub road component.
- Test: a boxed-in / plains-gapped layout is rejected at beam 16, accepted best-effort (`connected_fraction < 1`) at the unbounded tier — proving graceful degradation.
- Bench the corpus for escalation-rate / CPU vs the [0009a](0009a-room-planner-performance.md) budget.

---

## Incremental migration path

**Breaking-change label: Memory-format (EP-5.2).** `Plan` gains `approach_tiles` and `PlanScore` gains `road_*` fields / drops `traffic_congestion` → the bincode shape changes (positional; trailing `#[serde(default)]` does **not** make old payloads decode). Therefore:

- **Bump `WORLD_FORMAT_VERSION` 13 → 14** ([game_loop.rs:600](../../screeps-ibex/src/game_loop.rs#L600)) → one loud full reset on deploy (chosen migration; supersedes the earlier "let it ride," which only existed to avoid this shape change). Acceptable pre-MMO (EP-5.1 reset-anytime); revisit with a self-versioned plan store before live MMO.
- Layer changes (new `RoadConnectivityLayer`, D3 routing) also shift the seg-60 fingerprint → in-flight `PlanningState` rebuilds; moot after a full reset.

**Recommended sequence (each independently shippable, behind the existing kill-switch posture where applicable):**

1. D2 `Plan` data model + D8 `PlanScore` fields (the schema changes; ship together with the WFV bump so the reset happens once).
2. D4 placement fix + the connectivity **test** (cheap regression guard; no behaviour gate yet).
3. D3 approach-tile routing/storage + D5 road-BFS stamp connector (connectivity becomes *achievable* and approach tiles get populated).
4. D6 `RoadConnectivityLayer` (hard at tight beams, graceful at unbounded) — the guarantee turns on.
5. D7 prune hardening.
6. D8 scoring values (`road_transport`, `road_upkeep`).

**EP rules respected:** EP-6.8 (plan-loss accounting — satisfied by the graceful tier), EP-7.1 (engine-cited road/terrain mechanics), EP-6.11 (validate constrained/worst-case rooms), EP-5.2/5.3 (Memory-format label + WFV bump for the positional-bincode shape change), EP-4.1/4.5 (shared CPU budget + bounded backoff already in the driver), EP-10.7 (stays Proposed until operator accepts).
