# 0023 — N-room combat sim (P-ENGINE design)

Status: **In progress 2026-06-24.** Implements ADR 0022 **P-ENGINE** (operator chose the full N-room engine over a movement-only harness). Extends the single-room `screeps-combat-engine` to N rooms so integrated multi-room flee / attack / group-up and the auction's objective-bed composition tournament (ADR 0022 P-AUCTION) are simulatable end-to-end.

## Architecture finding (what the sim does — and doesn't)

Verified by reading the crate:

- **The engine is a deterministic tick over a `CombatWorld`.** `resolve.rs::resolve_tick` takes an `Intents` (per-creep combat actions **+ move `Direction`s**, tower actions) and resolves it (two-phase accumulate-then-apply combat + the `movement.rs` same-tile-contention port). It does **not** pathfind.
- **Move *directions* are produced by the agent/harness layer** (`screeps-combat-agent`) from the bot's decision output (`decide_combat`/`decide_squad`) — the engine just resolves the given `Direction`s.
- **Therefore the sim validates the bot's DECISIONS over a world; it does NOT run the bot's rover/`MovementSystem`/`AnchorPath`.** The anchor multi-room fix (rover `fd977f2`) is validated by rover unit tests + live, **not** here. P-ENGINE's job is multi-room **decision** behavior (when/where to flee, group-up, engage across a border) + the objective-bed combat the auction needs.
- **Single-room assumptions to lift:** `CombatWorld.terrain` is one `CombatTerrain`; `movement.rs::step` returns `None` at room edges ("a later slice"); `rampart_at(x,y)`/contention key by `(x,y)` not `(room,x,y)`; combat range uses `(x,y)`. Positions are **already** `screeps::Position` (room-qualified) — the data model is half-ready.

## N-room design

- **World:** `CombatWorld` keeps `terrain: CombatTerrain` as the default/common room terrain (so single-room builders stay unchanged — zero churn) **plus** `rooms: HashMap<RoomName, CombatTerrain>` per-room overrides, read via `terrain_for(room) -> &CombatTerrain` (override if present, else the default). No explicit room graph needed — adjacency is `Position` world-coord arithmetic (`Position::checked_add((dx,dy))` crosses rooms), and `Position::get_range_to` is already room-aware (Chebyshev over global coords). *(Caveat of the hybrid: the default terrain applies to every override-less room, so a true multi-room scenario with distinct walls per room gives each room its own override via `terrain_mut`.)*
- **Movement:** `step()` crosses edges via `Position::checked_add` (lands at the mirror tile of the adjacent room); `resolve_moves` + fatigue read `terrain_for(dest.room())`. **S1 keeps contention keyed by `(x,y)`** — room-aware keying `(room,x,y)` is deferred to S2 (two creeps at the same `(x,y)` in *different* rooms is rare during a border crossing). Fatigue resets on entering an edge tile (already have `is_edge`).
- **Combat (per-room):** tower fire only reaches creeps in the **tower's room** (range via world-coords is naturally huge across rooms → out of range); `rampart_at`/redirect become room-aware. Most range checks already use `get_range_to` (room-aware) so this is mostly fixing the `(x,y)` helpers.
- **Agent (`screeps-combat-agent`):** N-room `ScenarioBuilder` (multiple rooms, per-room terrain/structures, exits); **cross-room direction production** — the harness must pathfind a creep toward a target in another room and emit the per-tick `Direction` (the engine doesn't pathfind). **Objective beds:** core + towers + ramparts + defender creeps with active rampart-repair + tower-heal-of-defenders (the D5 fight model), with a win condition.
- **Eval (`screeps-combat-eval`):** metrics/cohesion room-aware; extend the tournament to attacker-vs-objective + composition space (ADR 0022 P-AUCTION).

## Build slices (each independently testable)

- **S1 — multi-room terrain + edge-crossing movement** (foundational substrate): **DONE 2026-06-24** — `terrain` default + `rooms` override map + `terrain_for`/`terrain_mut`; `step()` crosses edges via `checked_add`; `resolve_moves` + the fatigue loop read the *destination* room's terrain. 3 cross-room tests (step crosses + round-trips; `resolve_moves` carries a creep across an open border; the wall check reads the dest room, not the start room). All combat-crate tests green (engine 43 / agent 35 / eval 72 / decision 18), clippy-clean, dependents unaffected (`..Default::default()`). *Deferred to S2: room-aware contention keying.*
- **S2 — per-room combat:** **DONE 2026-06-24** — rampart shield/redirect keyed by full `Position` (`rampart_tiles`/`rampart_id_at`/`on_rampart`/`redirect` in resolve.rs); movement contention keyed by `Position` (`Mover.current_pos`/`dest_pos` + all `want_count`/`matrix`/`want_idx`/`creep_at`/swap/chain-block maps); the dead `(x,y)`-keyed `CombatWorld::rampart_at` removed. A 5-agent adversarial audit confirmed every *other* combat site is already room-safe via `get_range_to` (tower fire is naturally out-of-range across rooms — no explicit room-scoping needed; damage/heal pools are id-keyed; Phase-D edge/fatigue is room-local). 2 regression tests: a rampart only shields within its own room; two creeps at the same `(x,y)` in different rooms don't contend. Engine 45 tests green, clippy-clean.
- **S3 — N-room ScenarioBuilder + cross-room direction production** (agent): build multi-room scenarios; pathfind directions toward cross-room targets.
- **S4 — objective beds with active repair** (the D5 fight model): core+towers+ramparts+repairing defenders + win condition.
- **S5 — scenarios + integration gate:** `CROSS-ROOM-TRAVEL`, `FLEE-ACROSS-ROOMS`, `GROUP-UP-THEN-ENGAGE-ACROSS-BORDER`, `STUCK-MEMBER-TIMEOUT`, `ATTACKER-VS-OBJECTIVE`; the offline whole-stack integration gate (ADR 0022 PROVE-1).

## Cross-references
ADR 0022 (P-ENGINE / P-AUCTION / PROVE-1), 0006 (eval harness), 0008 (combat arch). The anchor movement fix is rover `fd977f2` (validated separately, not in this sim).
