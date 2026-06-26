# ADR 0025a — Terrain coordinate convention (RESOLVED) + residual object anomaly (open)

**Status:** The primary "coordinate mismatch" is **RESOLVED** (2026-06-26): it was a terrain-decode
**transpose** (row-major vs column-major), now fixed. A smaller **residual anomaly** remains open (~15–20%
of objects read as wall under the corrected decode, with no global transform fixing them) — under
investigation; mitigated by `snap_to_open`.

## 1. Resolution: the terrain string is COLUMN-MAJOR

The screeps `/api/game/room-terrain?encoded=1` string is **column-major** (`index = x*50 + y`), matching
the `screeps-game-api` `LocalCostMatrix` `xy_to_linear_index` — **not** row-major (`y*50+x`) as the
rest-api comment and python-screeps docs claim. My `decode_terrain`/`decode_fast`/`encode_terrain` had it
row-major, so every imported room was silently **transposed**, and the `snap_to_open` step was
compensating by moving the (now-misplaced) objects to adjacent open tiles in the transposed frame.

**How it was settled (definitive cross-check):** using the prospector CLI (`fetch`, auto-loads the root
`.screeps.yaml` token) against the **official** `screeps.com/shard3`:
- The screeps+ dump's terrain string for E11N1 is **byte-identical** to the official API, and the object
  coords match exactly — so **the dump is NOT corrupt** (this reverses the earlier "dump objects are the
  bug" conclusion).
- Under **column-major**, E11N1 renders as a coherent room with **all four objects on open tiles**; under
  row-major, **100% of objects (across 3000 rooms) sit on walls**. The 100%-on-wall "anomaly" was just the
  transpose viewed through the wrong index.

Fix (eval `<pending>`): `decode_terrain`/`decode_fast`/`encode_terrain` use `x*50+y`. `decode_fast`
transposes into the `FastRoomTerrain` row-major buffer (`buffer[y*50+x] = string[x*50+y]`) so the foreman
planner and the sim agree. Foreman bases were re-captured on the corrected terrain.

## 2. Residual anomaly (OPEN — the "other anomaly")

Even under the correct column-major decode, **~15–20% of objects still read as wall**, and crucially
**no single global transform fixes them** (tested: 8 dihedral symmetries × ±2 translations, on authoritative
data). It is not even per-room consistent:

- **E11N1:** all 4 objects open under column-major. ✓
- **E5N8:** controller (33,31) + mineral (7,3) align column-major (open), but **both sources (19,13),
  (41,13) sit deep inside wall masses** under *every* transform — yet a source must have an adjacent
  harvest tile, so those tiles cannot truly be walls.

So within one room, some objects align under column-major and others don't — which rules out a pure
convention/transform. Candidate explanations to chase (§ next):
1. **Source-keeper / special rooms** encode or place objects differently (W5N5 was an SK room straggler).
2. **Stale object snapshots** in the dump/API for some rooms (terrain immutable, objects drift).
3. A **screeps quirk for specific object types** (the E5N8 *sources* misalign while controller/mineral don't).
4. The operator's lead: **engine-placed border walls vs player walls** — engine border walls are natural
   terrain; player walls are `StructureWall` objects, never in the terrain string. (Not yet shown to
   explain *interior* straggler sources like E5N8's, which are nowhere near the border.)

**Mitigation in place:** `terrain_import.rs::snap_to_open` snaps each object to its nearest open tile at
load. Under column-major this is a **no-op for the majority** and a small nudge for the residual — so the
foreman planner always gets valid, non-wall seed positions. The residual does not corrupt scenarios:
terrain is correct and foreman-planned structures are derived from the terrain.

## 3. Data flow (where coords come from / where used)

```
screeps+ dump (screepspl.us) == official screeps.com API  (terrain byte-identical, objects match)
   per room: terrain (2500-char COLUMN-MAJOR string)  +  objects[{type,x,y}]
        │  offline node extraction → 13 varied rooms (raw coords)
        ▼
screeps-combat-eval/resources/real-terrain.json
        │  terrain_import.rs
        ├─ decode_terrain / decode_fast  → x*50+y  [CORRECT — column-major]
        └─ fixtures(): snap_to_open(objects)  ← safety net for the §2 residual (no-op for most)
        │
        ├─► Stage 2 ImportedRoom: base anchored from the (now correctly-oriented) terrain
        └─► Stage 3 foreman_capture → plan_room(seed objects) → captured-bases.json → ForemanGenerator
```

## 4. Cross-check tool (reproducible)

```
cargo run --release -p screeps-prospector -- --config .screeps.yaml --server-name mmo --shard shard3 \
  --cache-file target/xcheck-shard3.json fetch --rooms <ROOM[,ROOM...]>
```
Writes `{rooms:[{room, terrain, objects}]}` (same shape as the dump). Diff terrain/objects + test the
decode convention. (Auth token auto-loads from the root `.screeps.yaml`; read-only.)
