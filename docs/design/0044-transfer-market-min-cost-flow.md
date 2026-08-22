# ADR 0044 — The transfer market is a min-cost flow: the reduced-cost bid (V·U − source_floor − haul)

- **Status:** Decided
- **Date:** 2026-07-08
- **Deciders:** William Archbell
- **Related:** ADR 0040 (the e/t currency + transfer market + `market_pass`/`matching`), ADR 0038 (`room_net_roi` haul coefficient), ADR 0043 (the band-normalization ledger — this is the principled foundation A1 was reaching for); memory [[sim-determinism-fence]], [[no-one-off-pathfinding-algorithms]], [[prefer-per-tick-optimal-over-hysteresis]]. Supersedes the interim `.min(refill_roi_cap)` hard cap (ADR 0043 A1 stopgap).
- **One line:** The transfer (hauling) market is the decentralized solution to a **min-cost flow**: each sink prices energy at its own marginal downstream value (a shadow price `V·U`), storage-at-par is the numeraire/outside option, and each delivery is chosen by its **reduced cost** `bid − source_floor − haul(d)`. Haul is BOTH an **accept/reject subtraction** (is this arc profitable at all?) AND, separately, the `service_ticks` **divisor** (which scarce carrier-tick to spend). This is exactly the operator's `V·U − haul`; "maximize units-in-flight" is refuted (a diagnostic, not a maximand) and "minimize latency" is refined (weighted by urgency U).

## Context — what the market optimizes today, and the gap

`matching.rs` scores each carrier→sink arc by **value-density** `v = bid·amount / service_ticks` and greedily takes the highest-density feasible arc. That already IS `V·U/haul`, folded: V (downstream value) = the sink bid (refill inherits body-ROI, build/repair/upgrade inherit their completion value), U (urgency) is baked into the bid (deficit-scaled refill floor, repair imminence), and haul is the `service_ticks` **divisor**.

The gap (remote rooms): **haul as only a divisor can never make an arc net-negative** — so an EV-negative remote haul is still assigned if nothing better exists, and the ADR 0043 A1 `.min(refill_roi_cap)` cap **flattens** every starved lane (home and remote) to one indistinguishable number. The min-cost-flow theory (LP duality), the field (Overmind's `dq/dt` + net-of-haul remote curves, storage-as-hub consensus), and Screeps mechanics (remote energy is net-of-distance with an empirical break-even ~+8.7 e/t at the door, ~0 past ~250 tiles) all agree: a beyond-break-even remote delivery must be **rejectable**, not merely deprioritized. That requires a haul **subtraction**.

## Decision — the corrected two-stage reduced-cost model

All bids milli-e/t, par = `STORAGE_BID` = `BID_SCALE` = 1000. **Sink prices stay pure `V·U` shadow prices** (one price serves near AND far carriers); the per-arc haul term is applied at **selection**, not baked into the kernel. Survival vetoes (downgrade clock, tower-under-attack, container<50%) stay **outside** the market.

### Stage 1 — admission
Keep an arc `source → sink` (distance `d`) iff:
```
delivered = bid_sink − source_floor − haul_milli(d)  > 0
```
else **decline** (leave the energy at the source). This is the reduced-cost / par-after-haul reject floor the divisor form lacks.

### Stage 2 — allocation
Among admitted arcs, maximize the density `Σ bid_sink·flow / service_ticks` (distance in the divisor only). The subtraction gates accept/reject + remote ordering; the divisor allocates scarce carrier-ticks. **Never write these as one divided expression** — that double-charges distance.

### The pieces (with the adversarial fixes baked in)

- **`haul_milli(d)` — an INTEGER kernel** in `sink_economics.rs` (NOT a reuse of the f64 `room_net_roi` function — that would put a float in the exact-rational ordering and lives in the wrong crate; **determinism blocker**). Transcribe only the coefficient: `haul_milli(d) = (2·d·(CARRY_COST+MOVE_COST)·BID_SCALE) / (CARRY_CAPACITY·CREEP_LIFE_TIME)` in saturating u32 ≈ `round(d·2.667)` for a plain 1:1 body, times an integer **road factor** `road_q/1000` (500 for roaded lanes). `d` = the **structural source→sink leg** (`pickup → sink`), NOT the carrier-approach leg the divisor uses and NOT the composite `service_ticks`. Gate at **pickup-commit** (empty-hauler edges) where `d` is well-defined and declining genuinely leaves energy banked; do **not** re-gate already-loaded cargo (declining strands it). No new pathfinding beyond the distance oracle below ([[no-one-off-pathfinding-algorithms]]).

- **`source_floor` — the outside option, source-dependent** (the "decline is free" premise fails for a saturating source): **par (1000)** when the pickup source is lossless (storage/terminal); **~0** when it is a saturating buffer (source container above a fill threshold, or dropped energy — declining there means overflow/decay, not lossless hold). This is the newsvendor/base-stock treatment applied source-side and it dissolves the remote-chronic-starvation degenerate case.

- **NO `room_net_roi` multiplicative tilt.** It is redundant — a structurally-poor remote already prices low via `haul(d)` AND via its own lower sink bids (a low-throughput remote spawns cheaper bodies → lower refill ROI). Adding the tilt double-counts distance and reintroduces the float-in-ordering blocker. If a non-distance room-quality signal is later shown necessary, add it as an **integer per-mille** kernel, never the f64 function.

- **Remote sinks:** the SAME sink-price formulas, and the per-arc `haul_milli(d)` subtraction is the sole remote discriminator — a high-V remote refill correctly outbids a mid-V near sink *when genuinely worth the haul*, and a beyond-break-even one is DECLINED, not flattened to the cap.

### Refill pricing — the exact inputs and the deficit bound

Refill is the largest and most explosive sink price, so it is specified precisely (this is ADR 0043 A1 done right — the flat cap stops being the discriminator):

- **Kernel deficit-bound (`refill_bid`).** The `instant_spawnability_premium` floor uses `min(lane_deficit, next_body_cost)` as its deficit input, capping the floor at `[par, 2·par]`: filling the *next* body is worth up to 2×par, and energy beyond that is buffer, priced by `buffer_deposit_bid`. This is what bounds the deep-lane price explosion (an empty RCL8 lane otherwise bids ~40× par and starves everything). The naive framing — "thread the exact `next_body_cost` into the denominator" — is **wrong**: the floor is a *lower* clamp bound, so on a deep lane it explodes regardless of how exactly `next_body_cost` is measured. The **deficit itself** has to be bounded.
- **Head-of-line ROI as the driver.** `SpawnRequest.priority` already IS the body ROI in the shared `BID_SCALE` currency (ADR 0040 M5b), so `top_blocked_roi` = the head-of-line queued bid — no `w` reconstruction. The bid is `clamp(top_blocked_roi, floor, cap)`, i.e. bounded by V: the value of the creep the energy *becomes*.
- **`refill_roi_cap` survives only as a degenerate-body guard** (a 1-part body bidding a near-∞ ROI), paired with a `body_cost` floor / `w` clamp inside `body_roi_milli` so the degenerate infinity is bounded at the source. It is not the discriminator.
- **Freshness by system ordering, not by lag.** The refill generator flushes inside `build_econ_snapshot` at the top of `RunJobSystem`, which runs *after* `RunMissionSystem` + `SquadManagerSystem` have registered every spawn. A dedicated **`SpawnRefillPricingSystem`** sits between the last spawn producer and `RunJobSystem`; it **reads** the now-complete `SpawnQueue` and **writes** a *separate* `RefillPricingCache` resource (`RefillPricingContext { next_body_cost_e = cheapest queued body, top_blocked_roi_milli = head-of-line bid }`) — so the queue stays a pure producer/consumer (never mutated for pricing) and the published data can never be stale queue state. The refill generator captures the room's cell from that cache at registration; refill is *not* separable into a demand-registering system because `execute_demands` prices it as one target-type inside the room's full lazy haul-demand scan. Same tick, fully fresh; no serialized state (the cache and its cells are transient).

### The distance model — true routed distance, one narrow oracle

The haul `d` must be **true path distance**, not Chebyshev `get_range_to`. `Position::get_range_to` returns a straight-line *global-coordinate* Chebyshev distance across rooms, so cross-room it silently prices a remote at (e.g.) `d≈40` when the true routed path is `90+`; `haul_milli(d)` then underprices the far haul and the admission serves remotes that are actually beyond break-even — exactly the failure this ADR exists to prevent. On a straight synthetic corridor routed ≈ Chebyshev (`d=40→42`, `d=150→154`), so the difference only bites where **routed ≫ Chebyshev**: realistic (cave) terrain and structure-obstructed home rooms — which is the normal case.

- **The kernel takes a `dist` oracle, it does not compute geometry.** `market_pass` receives a distance closure/trait for the pickup→sink structural leg; the carrier→pickup *approach* leg keeps the cheap Chebyshev estimate (it only affects the divisor). Sim and live back the same seam with the same distance MODEL (shortest walkable tile path via a `screeps-rover` `PathfindingProvider`), so tuning transfers between them.
- **Full-tile, cached, exact — not a room-level hybrid.** A room-level `travel_ticks = hops×50` is a FLAT per-room cost with zero intra-room terrain (and is JS-only, so the sim cannot call it); on a line of remotes it degenerates to ~Chebyshev and misses the intra-room detour that is the whole signal. Live therefore computes the pickup→sink distance with rover's engine-backed pathfinder over the STRUCTURE overlay from the shared `CostMatrixCache` (a static obstacle set — no creeps or sites — hence cacheable), wrapped in a per-static-pair memo (`(pickup, sink)` → routed ticks, TTL'd). The decision-critical pairs are between STATIC structures (source containers/storage → spawn/extension sinks; remote source containers → home), so the cache hits after first compute and steady-state pathfinds tend to zero. Pathfinds/tick + hit-rate + CPU-pool usage are the CPU ship gate.
- **Range 1, not range 0.** A refill SINK tile is itself impassable (a spawn/extension is a blocker), so the distance query must be "path to ADJACENT" (range 1). Querying range 0 is unreachable-onto-a-wall and silently falls back to a straight line, defeating the whole migration on realistic terrain. Both sim and live use range 1.
- **NO PATH ⇒ NOT A TRANSFER OPTION.** The oracle returns `Option<u32>`; `None` means no path, and the kernel SKIPS that `(pickup, sink)` arc rather than fabricating a cheap straight-line price for an unservable haul. Caching the `None` stops re-searching a doomed pair every tick (an ops-exhausted search is `incomplete` ⇒ `None` ⇒ declined, which is correct — such a haul is beyond break-even anyway).
- **Loaded-carrier deliver reachability.** The loaded-carrier delivery edge likewise gates on `dist(carrier, sink).is_some()`: a loaded carrier is not offered a sink it cannot path to, mirroring the empty-carrier pickup gate. The oracle is consulted for the reachability BOOLEAN only — the service PRICE keeps the cheap Chebyshev approach leg — and a loaded carrier decides its delivery ONCE (it is not re-decided while executing), so this is a per-decision, not per-tick, cost. This is strictly more correct than a static per-sink graph check because it tests the actual carrier→sink reachability.
- **Coupling constraint.** The transfer/market layer depends on a NARROW distance abstraction (a `HaulDistance`-style trait: `haul_distance(from, to) -> Option<u32>`), never on rover / `PathfinderService` / cost-matrix internals. The rover-backed cache is the concrete impl living in the pathing layer, injected through that seam. No rover types leak into `transfer/`.
- **Secondary approximations that are deliberately left alone** (they mis-*rank* cross-room but do not drive the core pricing): nearest-source / nearest-delivery selection by linear range; `spawn_policy`'s `room_manhattan_distance`/`max_distance` (coarse sizing heuristics, correctly room-count); repair/controller `<= 3` range gates (correct as Chebyshev adjacency).

### The haul road factor

`haul_road_factor_break_even_calibration` pins the coefficient: the physically-derived plains factor (`road_q = 1000`, slope `8d/3` from a 1:1 hauler's body-cost/capacity/life) puts the PAR break-even at **374 tiles** and the upgrade break-even at **749**. The empirical field figure (~250) sits BELOW plains 375 because real lanes are plains-or-worse, and the sim shows no over-hauling at `road_q = 1000` (instrument D `haul_cost_permille ≈ 0`) — so the **derived constant is retained**: tuning a first-principles coefficient to a fuzzy field number with unknown road/body assumptions would be worse. Because the distance oracle already routes the path, a principled refinement is available: scale `road_q` with the path's road fraction (roaded segments cheaper, `road_q → 500`).

### Keep unchanged (all confirmed theory-correct against the code)
`instant_spawnability_premium` (base-stock underage U), `buffer_deposit_bid = base·(free/cap)²` (the (s,S) holding discount; overage ≈ 0 in-structure → correctly biases buffer-ahead for spawn bursts), `imminence_q`/`repair_bid` (V·U for repair), `upgrade_bid` + `V_UPGRADE` with its `CONTROLLER_MAX_UPGRADE_PER_TICK` saturation, `opportunity_floor`/`admit_use_withdraw` (the VCG/complementary-slackness outside option gating Use-lane withdraws), all survival VETOES outside the market, and the **exact-rational `u128` greedy** (no float reaches an ordering).

## Verdicts on the operator's candidate framings (from the research)
- **`V·U − haul`: CONFIRMED** — it is precisely the per-arc reduced cost of the flow objective (LP duality), and mirrors Overmind's proven split of sink `multiplier` from the `dq/dt` matcher.
- **"Maximize units-in-flight": REFUTED** — Little's Law `L = λW` is an identity, not an objective; at fixed conversion throughput, more in-flight energy means more latency + holding cost. It is a **diagnostic** (rising haul backlog ⇒ W rising ⇒ sinks starved), never a maximand.
- **"Minimize mined→used latency": REFINED** — latency matters only *weighted by value-at-risk*, which is exactly what U (stockout-imminence) already encodes. A global minimize-W over-hauls cheap far deliveries and burns CPU.

## The six adversarial fixes (naive synthesis → shipped model)
1. **BLOCKER** — `room_net_roi` float tilt in the ordering → **drop the tilt** (haul subtraction already prices far/poor remotes; integer per-mille kernel only if ever needed).
2. **MAJOR** — `d` must be the source→sink structural leg, gated at pickup-commit (not the carrier-approach leg the divisor uses; don't gate loaded cargo).
3. **MAJOR** — `source_floor` = par for lossless storage, ~0 for a saturating buffer ("decline is free" fails for a filling source container → overflow/decay).
4. **MAJOR** — one shared `arc_admitted()` in BOTH greedy and oracle, or `match_optimality_gap` is an artifact.
5. **MINOR** — the refill call-site `.min(cap)` and the exact-input threading are load-bearing together: dropping the cap without the bounded deficit lets a big home lane bid ~40× par and starve everything.
6. **MINOR** — state the objective as two stages, not one divided expression (else distance is double-charged).

## Phasing

The design has four parts; they are separable but only the whole is the end state.

- **P0 — kernel + core:** the `haul_milli(d)` integer kernel, `source_floor`, the two-stage `arc_admitted()` in `matching.rs`, and the `body_cost`/`w` clamp in `body_roi_milli`. No serialized-shape change.
- **P1 — refill exact inputs:** the kernel deficit-bound plus `SpawnRefillPricingSystem` / `RefillPricingCache` (above). Retires the `.min(cap)` call-site clamp and the `REFILL_NEXT_BODY_COST_FALLBACK_E = 300` stopgap.
- **P2 — true distance + the multi-room sim** (below): the `dist` oracle in the kernel, the rover-backed live and sim impls, and the multi-room corpus that makes remote pricing measurable at all.
- **P3 — all-sinks activation (ADR 0043 A1/A8):** route `buffer_deposit_bid` / `upgrade_bid` / `build_bid` / `repair_bid` to live registration so the whole economy is EV-priced.

**No feature flags.** The reduced-cost admission (`source_floor` + haul subtraction) and true routed distance are ALWAYS on: the market *is* the end state, and there are no `admission` / `true_distance` toggles to configure. Sweeps tune the CONSTANTS around this end-state market, they do not select between markets.

## The sim-experiment plan

A single-room corpus (`layout.room` one `RoomName`, the remote arm inert at `max_distance == 0`) cannot measure remote flattening at all, so `screeps-econ-eval` carries a multi-room corpus. What the design is validated on:

- **Family R** — one home + N remotes at real path-distances `d ∈ {10, 40, 90, 150, 210, 260}` (straddling the ~200–250 break-even), each with its own source(s), so `service_ticks` and `haul_milli(d)` actually vary. A **saturated** variant (home storage full / long-lived fleet ⇒ no refill hunger) is required to exercise DECLINE, and a **realistic-terrain** variant makes `routed ≫ Chebyshev` normal.
- **Instruments** (diagnostics, reported off `RunOutcome.remote`): (B) mined→used dwell via `buffer_tick_integral` (Σ source-container energy — mined-but-unconsumed); (C) units-in-flight (`in_flight_sum`/`_max`) + carrier utilization (`carrier_ticks`); (D) `realized_haul_cost` vs `delivered_value` → `haul_cost_permille()`, the over-hauling detector (Σ `haul_milli(routed_d)·amount` vs Σ `bid·amount`, accumulated from the greedy's chosen edges); (E) wasted/idle/re-hauled (dropped-energy integral); plus `admission_declines` (Σ generated edges the reduced-cost gate rejected — each a viable arc, since edges only generate when flow is available).
- **One shared `arc_admitted()` filter** applied in BOTH `matching.rs` `greedy_assign` AND `econ-eval`'s `oracle_best_fp` (drop `delivered ≤ 0` edges), then rank survivors by the same density — otherwise `match_optimality_gap` measures the greedy failing to optimize an objective it isn't running (an artifact).
- **Measure** — economy: `T_recover` (η), `T_RCL(N)`, `H`, `extension_deficit_integral`, deficit-episode p05–p95, `spawn_idle_frac`, `repair_leak_e`, income e/t. Transfer optimality: `match_optimality_gap` (permille, vs the reduced-cost oracle), mined→used latency, in-flight/utilization, realized-haul net-of-value, wasted energy, flap.
- **Success gate:** on Family R, remote high-ROI refills SERVED when profitable and DECLINED past break-even (zero realized `delivered < 0`); remote deficit-episode p95 drops; **no** single-room `T_recover`/`T_RCL`/`H`/`repair_leak_e` regression (bootstrap CI); gap within the greedy budget under the shared-objective oracle; determinism fence green throughout. If the model over-hauls at scale (D shows realized haul-e rising faster than delivered value) → add a CPU/fleet-size cap.

### The multi-room mover

The econ sim's single-room `AnalyticMover` cannot express Family R, and pathfinding belongs in rover ([[no-one-off-pathfinding-algorithms]]), so the sim uses the SAME rover-backed multi-room pathfinding the combat sim uses rather than a bespoke econ mover or an analytic distance abstraction. `EconWorld` (in `screeps-econ-engine`) is already position-based and multi-room-capable, the K1–K4 kernels and the engine resolver already handle cross-room `Position`s, and `EconWorld.movement` is already the shared multi-room `MovementState` — the only structural blocker is the mover's single-room assumption.

- **`RoverMover`** routes with `screeps_rover::LocalPathfinder::search` (genuine multi-room A* over room-qualified `Position`s, offline / no JS, returns the full `Vec<Position>`) and then walks a **multi-room-generalized `walk_trace`** (a `rooms` map with edges PASSABLE, so the engine's edge-exit relocation fires) for the fatigue trace. `travel_ticks` = trace length — the true distance the market consumes. Memoized per `(from, to, range, body class)`.
- **Terrain is baked into the cost matrix, not read from a `Terrain` object.** Headless has no `Terrain`, so the caller supplies a per-room `LocalCostMatrix`: 255 = impassable (walls + blocker structures), a swamp cost for swamp tiles, 1 for roads, 0 = plain default. `search` is used DIRECTLY rather than through the combat driver / `CostMatrixDataSource`, because econ's analytic tier ignores contention by design — it needs rover's multi-room *pathfinding*, not the driver's traffic management. Off-world rooms return `None` from the callback (impassable).
- **`AnalyticMover` is retained for the single-room families**; the mover is selected as a `Box<dyn Mover>` so single-room corpora stay byte-identical (a multi-room A* may legitimately pick a different single-room path than the analytic oracle). The `Mover` trait and the runner call sites are otherwise unchanged; `invalidate_from` takes the multi-room `MovementState`.
- **The sim is structure-aware.** `realize()` folds every blocking structure (spawns/extensions/storage/towers/labs/links) from the real captured foreman layout into `terrain.walls`, RCL-staged by `included_at`, and inserts the home terrain into the multi-room `rooms` map; roads cost 1, containers stay walkable — aligned with live's cost matrix. (Ramparts are not folded, a known second-order gap.) This is precisely why the sink query must be range 1.
- **Remote mining must actually run** for the flow the admission prices to exist: harvesters are assigned round-robin across remote sources, and `max_distance > 0` so the remote-hauler sizing and the ≥Low repair arm activate.

### What the experiments established

These are the calibration facts the design rests on; they are findings, and they are why the constants are what they are.

- **The DECLINE needs the right scenario.** `haul_milli(d) = d·8/3` milli, so `d = 260 → ~693` (0.69 e/e). A hungry REFILL sink bids ~8000 (8×), so `delivered ≫ 0` and every remote is SERVED — which is **correct**: remote energy is a bargain against an 8× refill (refill break-even lands near `d ≈ 3000` tiles, i.e. never). The reject floor bites for **par-value sinks** (a storage rebalance bidding 1000 → break-even `d ≈ 375`) or where **routed ≫ Chebyshev**. So the success gate must be measured on a saturated / par-sink Family R or under realistic terrain, not on a hungry-refill corridor.
- **End-to-end decline is real.** On a saturated home over realistic cave rooms (farthest routed 464 > the par break-even ~375), a 1200-tick run rejects tens of thousands of otherwise-viable arcs while `haul_cost_permille ≈ 0` — the admission fires and there is no over-hauling. The per-remote *retention* gradient is confounded (a stocked storage's `base·(free/cap)²` buffer discount decays as it fills, and haul capacity binds), so the clean signals are the direct decline COUNT and instrument D, never retention.
- **The corrected model needs no haul-constant retune.** A coordinate-descent sweep over the end-state market held every distance-related axis at its default (`build_bid_road = 250`, `refill_roi_cap = 10000`, `build_bid_extension = 8000`, `v_upgrade = 2000`). The only axis the descent moved was `imminence_horizon_ticks` 1500 → 750 — a repair-TIMING constant unrelated to haul distance. Full-corpus adjudication showed 750 does lift H (C 0.357→0.394, D 0.343→0.393) but **fails the Family S steady-state guard rail**, so **`imminence_horizon = 1500` stands**. The sweep's real value is the confirmation that true-distance + structure-awareness + unreachable-exclusion did not destabilize the existing tuning.

### Validation corpus — foreman layouts × RCL

Every captured foreman layout, realized at each RCL stage it supports (13 layouts × RCL 1..full-build ≈ 104 runs), is run to the guard-rail horizon under the end-state market. The health invariants are: not deadlocked; roads not collapsed; at RCL ≥ 4 the market GENERATES edges (refill/haul sinks reachable — this is the structure-reachability proof) and there is no permanent refill deficit. The permanent-deficit check is gated to **RCL ≥ 4** deliberately: the pre-storage container economy (RCL 1–3) is inherently haul-tight across the whole corpus, so a transitional deficit there is a threshold artifact, not a defect; RCL ≥ 4 is where a stocked storage should always keep the spawn servable. A fast gated smoke variant guards against rot.

### Realistic terrain generation

The corpora need terrain where `routed ≫ Chebyshev`, so terrain generation lives in `screeps_sim_core::terrain_gen` (cellular-automata cave walls + swamp + natural-cave connectivity, deterministic in seed) and is shared by ALL sims rather than being an econ-only fixture.

- **Seam alignment is correct-by-construction.** The kernel/engine relocates a creep across an exit tile WITHOUT a wall check, so seams MUST match or a creep lands on a wall. `generate_terrain_for_room(room, world_seed, connect, params)` derives each seam's exit range from a deterministic ORDER-INDEPENDENT function of the two rooms (`seam_range(id_a, id_b)`), so adjacent rooms' opposing exits match by construction — no fix-up carving (this mirrors the engine's own globally-continuous generation). `seam_range_between(a, b)` aligns a captured room to a generated neighbour.
- **Cross-sim consequences.** Pathing: rover stays optimal on cave terrain (worst `R_fatigue = R_ticks = 1.0000` over 30 seeds, zero ops-cap hits), so the route-optimality and ops gates are safe. Combat: realistic terrain exposes real deviations that trivial rooms hid — mirror-symmetric self-play nets swing ±~500 (a position/order bias), and the tuned `open_combat` edge over `default` ranges −750..+890 across cave seeds, i.e. **combat tuning done on open fields does not generalize to chokepoints**. Economy: `haul_milli` is a LINEAR distance model and is optimistic once `routed ≫ Chebyshev`, which is exactly why the true-distance oracle is mandatory rather than an optimization.

## Open design questions

- **The `RefillPricingCache` shared-cell bridge.** Publishing the pricing context through an `Rc<RefCell<_>>` cell captured by the lazy generator makes lifetime and read/write ownership non-obvious — the same objection that applies to the `supply_structure_cache` precedent it mirrors. It exists only because the lazy generator closure can read `room_data` but not a resource. Cleaner alternatives to evaluate when the transfer-generator architecture is next touched: (a) extend `TransferRequestSystemData` with an explicit `refill_context(room)` accessor backed by a plain (non-`Rc`) resource reference held in `TransferQueueGeneratorData` — explicit dataflow, no interior mutability, but threads the resource through ~18 construction sites; (b) lift refill-demand registration out of the lazy generator into a system with direct `SpawnQueue` + `TransferQueue` access — which needs `execute_demands`' room-wide haul-demand scan split or shared. Not a correctness issue; a clarity/maintainability one.
- **In-flight demand-netting** (Overmind's discount-by-inbound) — adds a DTO field; justified only if per-tick greedy re-pricing proves insufficient.
- **A CPU / fleet-size cap** — justified only if instrument D shows over-hauling.
- **Per-lane road awareness** — scale `road_q` with the routed path's road fraction, since the oracle already has the path.
- **Loaded-carrier delivery leg** still uses the dynamic Chebyshev carrier→sink approach distance for its price (only reachability is oracle-gated), so it is not reachability-gated against a sink that becomes unreachable mid-flight; the movement layer handles that at runtime. Revisit if it proves a problem.

## Where the mechanisms live (file:line)
- Shared movement primitives: `screeps-sim-core/src/rover_driver.rs:248` `resolve_moves_via_system_with(movement_state, &[SimMoveRequest], cache, cost_source, config)`; `SimMoveRequest` at `rover_driver.rs:86`; `resolve_movement` edge-exit relocation in `screeps-sim-core/src/tick.rs:25`. `EconWorld.movement` is the shared multi-room `MovementState`.
- Rover traits: `screeps-rover/src/traits.rs:76` `CostMatrixDataSource {get_structure_costs, get_construction_site_costs, get_creep_costs}`; `PathfindingProvider` at `traits.rs:20`; `LocalPathfinder::search` at `local_pathfinder.rs:438` (signature `local_pathfinder.rs:12,176,609`).
- Combat reference for multi-room cost sourcing: `CombatWorldCostSource` at `screeps-combat-agent/src/pathing.rs:351`; driver wrapper `resolve_moves_via_system` at `pathing.rs:523`.
- Econ mover seam: `Mover` trait `screeps-econ-eval/src/movement.rs:53` (`trace`/`invalidate_from`/`travel_ticks`); instantiated `runner.rs:256`, invalidated `runner.rs:330`, used in `step_worker`. Structure realization: `screeps-econ-eval/src/layout.rs:174` `realize()`; multi-room insert `scenario.rs:530`.
- Live distance seam: `HaulDistance` trait in `transfer/market_adapter.rs`; impl `pathing/hauldistance.rs` (`HaulDistanceService` resource + `RoverDistanceOracle`), threaded `run_room_market` → `select_market_pickup_and_delivery` → `get_new_market_pickup_and_delivery_state` → `haul.rs` Idle; telemetry `PathingMetrics.haul_dist_{computes,hits,cached_pairs}`.
- Room-level route cache (coarser, still used by missions): `PathfinderService` `screeps-ibex/src/pathing/pathfinderservice.rs` — `travel_ticks(from_room,to_room,tick):206`, `route_distance:201`, `nearest_by_path:171`, `snapshot:161`, CPU pool `pool_for_tier:44`.
- Market kernel: `screeps-econ-decision/src/market.rs` — `market_pass:159`, `service :209`/`:241`, haul `d` `:262`, `nearest_refill:182`. Sim refill via `refill_bid_from_plans` `screeps-econ-eval/src/market.rs:502` (shared-kernel parity with live).
- Kernel pins for the admission's boundary behavior: `market::admission_declines_far_par_sink`, `market::unreachable_pickup_is_not_a_transfer_option`, `market::loaded_carrier_unreachable_sink_is_declined`, `scenario::structure_sink_reachable_at_range_one_not_zero`.

## Consequences

Turns the transfer market from "value-density with haul-as-divisor-only + a flattening cap" into the correct decentralized min-cost flow: remotes priced by real value − haul (served when worth it, declined past break-even), the cap inert, the whole economy EV-priced. All integer/deterministic (the one float — the `room_net_roi` tilt — was dropped). Remote correctness is inseparable from the sim gaining a multi-room corpus and from true routed distance: with Chebyshev `d` the admission is right only where routed ≈ Chebyshev, which realistic terrain is not. Every kept kernel is confirmed theory-correct.

## Landed
- `7bb947f` P0 `haul_milli`/`source_floor` reduced-cost kernel (2026-07-08)
- `2f5445c` two-stage admission in the market pass (2026-07-08)
- `57999c5` refill priced from the live spawn queue via `RefillPricingCache` (2026-07-08)
- `ab4230f` + `c73e4e5` true routed haul distance, sim then live (2026-07-09)
