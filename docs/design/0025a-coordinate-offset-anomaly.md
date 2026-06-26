# ADR 0025a — Terrain/object coordinate-offset anomaly (SUSPECT — revisit)

**Status:** Open anomaly, mitigated by a workaround. Flagged 2026-06-26 during ADR 0025 §12 Stage 3
(realistic foreman bases). The fix in place (`snap_to_open`) is sound for the harness, but the *root
cause of the offset is not understood* and the dump data is suspect — **this doc exists so we come back
to it.**

## 1. The anomaly

The committed map dump `screeps-foreman-bench/resources/map-mmo-shard3.json` (14,884 rooms) has, per room,
a `terrain` string (2500 chars) and an `objects` array (controllers / sources / minerals / … with `x,y`).
Under the **correct** terrain decode, **every object sits on a wall tile, exactly one tile from open**:

- Across 3000 sampled rooms, **7883 / 7883** controller+source+mineral objects are on a wall tile under
  the row-major `y*50+x` decode — **100%**, deterministically (a 15–40% base wall rate makes this ~0
  probability by chance).
- Each object is **exactly Chebyshev-distance 1** from the nearest open tile (histogram: `{1: 7883}` —
  not a spread). So objects are uniformly nudged *one tile into wall-edges*, in a per-object direction.
- **No rigid transform recovers alignment.** Identity (`y*50+x`) → 100% on-wall; all 8 dihedral
  symmetries (flips/rotations/transpose) → ~28–37% (≈ the random base rate, i.e. *uncorrelated*); all
  25 translations in `[-2,2]²` → ≥42% (worse than chance). So it is **not** a flip, rotation, transpose,
  or uniform shift.

## 2. Why the TERRAIN decode is correct (not the suspect side)

Verified from first principles against the screeps Rust bindings — the offset is in the **objects**, not
the terrain:

- `screeps-game-api-0.23.1` `LocalRoomTerrain` is **row-major** (`bits[y*50+x]`), proven by its own test
  `addresses_data_in_row_major_order` (sets index 1 → reads tile `(1,0)`). Only `LocalCostMatrix` is
  column-major (`xy_to_linear_index = x*50+y`) — the trap, but not what terrain uses.
- `screeps-rest-api` `TerrainEntry` documents the API string as row-major `y*50+x`, "the exact encoding
  the foreman-bench map JSON stores."
- The decoded terrain shows authentic natural-room structure: border walls **with exit gaps** (e.g.
  `111…1110000000000011` on a top edge, fully-walled opposite edges), coherent wall masses + swamp
  patches. A wrong (transposed/flipped) decode would not produce a sensible exit pattern.
- `screeps-foreman`'s `FastRoomTerrain` indexes `Location::to_index = y*50+x` — same convention; the
  foreman planner consumes the dump on this basis.

Conclusion: terrain is right; the dump's **object coordinates** are systematically off by one tile in a
content-dependent direction. Most likely a **dump-generation-tool bug** (provenance of
`map-mmo-shard3.json` unknown — see §5).

## 3. Where the data comes from / where it is used (data flow)

```
screeps-foreman-bench/resources/map-mmo-shard3.json   ← SOURCE (provenance unknown — §5)
  per room: terrain (2500-char y*50+x string)  +  objects[{type,x,y}]   ← x,y are the SUSPECT coords
        │ (offline extraction: node, picks 13 varied rooms, RAW coords, no transform)
        ▼
screeps-combat-eval/resources/real-terrain.json       ← committed fixtures (raw dump coords)
        │
        ▼  terrain_import.rs
   decode_terrain(terrain)  → CombatTerrain        [y*50+x — VERIFIED CORRECT]
   fixtures(): snap_to_open(controller/sources/mineral)  ← THE WORKAROUND (snaps each suspect coord
                                                            to its nearest open tile; dist-1 ⇒ recovers
                                                            the true adjacent tile; keeps them distinct)
        │
        ├─► Stage 2  ImportedRoom (generate.rs): base anchored from the TERRAIN (open-component BFS),
        │            NOT from object coords — so the suspect coords don't even reach the Stage-2 sim.
        │
        └─► Stage 3  foreman_capture.rs::capture(): feeds the SNAPPED controller/sources to
                     screeps_foreman::planner::plan_room → CapturedBase{terrain, structures}.
                     The planner computes structure positions FROM the terrain it is given, so the
                     captured base's structures are terrain-aligned BY CONSTRUCTION.
                       → resources/captured-bases.json (committed cache)
                       → generate.rs ForemanGenerator/realize_base places terrain-aligned structures.
```

**Blast radius is narrow.** The only place the suspect coords enter is the *seed* controller/source/
mineral positions handed to the foreman planner (and they're snapped to valid open tiles first, within 1
tile of the dump position). The **terrain is correct**, and the **planned base structures are correct**
(the planner derives them from the terrain). So combat scenarios are not corrupted by the offset; at
worst a base is planned around a controller/source seeded one tile from its true shard location — which is
irrelevant for combat tuning (we want *realistic* bases on *real* terrain, not a byte-exact replica of a
specific shard base).

## 4. Current workaround (in place)

`terrain_import.rs::snap_to_open(terrain, x, y, taken)` — 8-connected BFS to the nearest non-wall tile not
already taken. Applied to controller/sources/mineral in `fixtures()`. Tested: objects land on clear,
distinct tiles, and the snap moves each ≤ 1 tile (`snap_recovers_objects_within_one_tile`). Sound for the
harness; does **not** explain or fix the underlying dump offset.

## 5. To revisit — hypotheses + what would crack it

Untested leads (deliberately not chased now — flagged for a focused revisit):

1. **Dump provenance — KNOWN (2026-06-26):** the dump was **downloaded from the screeps+ website**
   (screepspl.us, a third-party), NOT produced by our tooling. So the off-by-one is in **screeps+'s map
   export format** (their object-position or terrain-serialization step), which we don't control. That
   makes the live-API cross-check (below) the right way to root-cause: diff screeps+ vs the *authoritative*
   official server.
2. **Live-API cross-check (definitive — PLANNED, operator providing token).** Fetch ONE room (e.g.
   `E11N1`) from the **official** server (`https://screeps.com`, `shard3`) via `screeps-rest-api`
   `room_terrain_encoded` + `room_objects`, decode terrain `y*50+x`, and diff each object's tile against
   the screeps+ dump. This shows exactly which side is shifted and by how much. Auth = a screeps.com
   long-lived token (`AuthMode::Token`, `X-Token` header); pass it via env (`SCREEPS_TOKEN`) so it never
   enters code/logs/history. The dump is mmo:shard3, so the official server is the ground truth.
3. **String-index shift (cheap fallback test).** A shift of the terrain string by *k* chars *with
   row-wraparound* differs from a coordinate translation only at row edges (translations were tested and
   fail). Worth testing `±1`, `±50`, `±51` char shifts of the raw string if the cross-check is delayed.
4. **Half-tile / inclusive-bound artifact** in whatever coordinate conversion the dump tool used
   (e.g. a `RoomPosition` → local-xy step that floors toward an adjacent tile).

When resolved: replace `snap_to_open` with the correct decode (or regenerate the fixtures from an
authoritative source), and delete this anomaly note.
