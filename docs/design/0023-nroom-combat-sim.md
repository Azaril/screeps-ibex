# 0023 — N-room combat sim (P-ENGINE design)

Status: **In progress 2026-06-24.** Implements ADR 0022 **P-ENGINE** (operator chose the full N-room engine over a movement-only harness). Extends the single-room `screeps-combat-engine` to N rooms so integrated multi-room flee / attack / group-up and the auction's objective-bed composition tournament (ADR 0022 P-AUCTION) are simulatable end-to-end.

## Architecture finding (what the sim does — and doesn't)

Verified by reading the crate:

- **The engine is a deterministic tick over a `CombatWorld`.** `resolve.rs::resolve_tick` takes an `Intents` (per-creep combat actions **+ move `Direction`s**, tower actions) and resolves it (two-phase accumulate-then-apply combat + the `movement.rs` same-tile-contention port). It does **not** pathfind.
- **Move *directions* are produced by the agent/harness layer** (`screeps-combat-agent`) from the bot's decision output (`decide_combat`/`decide_squad`) — the engine just resolves the given `Direction`s.
- **Therefore the sim validates the bot's DECISIONS over a world; it does NOT run the bot's rover/`MovementSystem`/`AnchorPath`.** The anchor multi-room fix (rover `fd977f2`) is validated by rover unit tests + live, **not** here. P-ENGINE's job is multi-room **decision** behavior (when/where to flee, group-up, engage across a border) + the objective-bed combat the auction needs.
- **Single-room assumptions to lift:** `CombatWorld.terrain` is one `CombatTerrain`; `movement.rs::step` returns `None` at room edges ("a later slice"); `rampart_at(x,y)`/contention key by `(x,y)` not `(room,x,y)`; combat range uses `(x,y)`. Positions are **already** `screeps::Position` (room-qualified) — the data model is half-ready.

## N-room design

- **World:** `CombatWorld.terrain: HashMap<RoomName, CombatTerrain>` + `terrain_for(room) -> &CombatTerrain` (default all-plain). No explicit room graph needed — adjacency is `Position` world-coord arithmetic (`world_coords()`/`from_world_coords`), and `Position::get_range_to` is already room-aware (Chebyshev over global coords).
- **Movement:** `step()` crosses edges via world-coords (`from_world_coords(wx+dx, wy+dy)`); `resolve_moves` keys contention by `(room,x,y)` and checks `terrain_for(dest.room())`. Fatigue resets on entering an edge tile (already have `is_edge`).
- **Combat (per-room):** tower fire only reaches creeps in the **tower's room** (range via world-coords is naturally huge across rooms → out of range); `rampart_at`/redirect become room-aware. Most range checks already use `get_range_to` (room-aware) so this is mostly fixing the `(x,y)` helpers.
- **Agent (`screeps-combat-agent`):** N-room `ScenarioBuilder` (multiple rooms, per-room terrain/structures, exits); **cross-room direction production** — the harness must pathfind a creep toward a target in another room and emit the per-tick `Direction` (the engine doesn't pathfind). **Objective beds:** core + towers + ramparts + defender creeps with active rampart-repair + tower-heal-of-defenders (the D5 fight model), with a win condition.
- **Eval (`screeps-combat-eval`):** metrics/cohesion room-aware; extend the tournament to attacker-vs-objective + composition space (ADR 0022 P-AUCTION).

## Build slices (each independently testable)

- **S1 — multi-room terrain + edge-crossing movement** (foundational substrate): `terrain` → per-room map; `step()` crosses edges; `resolve_moves` room-aware. Test: a creep at a room edge steps into the adjacent room; a cross-room column stays cohesive. *(This note's first implementation.)*
- **S2 — per-room combat:** tower fire / rampart-redirect / contention keyed by room; a tower does not hit a creep in another room.
- **S3 — N-room ScenarioBuilder + cross-room direction production** (agent): build multi-room scenarios; pathfind directions toward cross-room targets.
- **S4 — objective beds with active repair** (the D5 fight model): core+towers+ramparts+repairing defenders + win condition.
- **S5 — scenarios + integration gate:** `CROSS-ROOM-TRAVEL`, `FLEE-ACROSS-ROOMS`, `GROUP-UP-THEN-ENGAGE-ACROSS-BORDER`, `STUCK-MEMBER-TIMEOUT`, `ATTACKER-VS-OBJECTIVE`; the offline whole-stack integration gate (ADR 0022 PROVE-1).

## Cross-references
ADR 0022 (P-ENGINE / P-AUCTION / PROVE-1), 0006 (eval harness), 0008 (combat arch). The anchor movement fix is rover `fd977f2` (validated separately, not in this sim).
