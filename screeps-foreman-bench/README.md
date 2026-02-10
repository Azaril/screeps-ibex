# screeps-foreman-bench

An offline benchmarking and visualization tool for [screeps-foreman](../screeps-foreman/), the Screeps room planning library. screeps-foreman-bench loads map data from JSON files, runs the room planning pipeline on native targets (no Screeps runtime required), and outputs PNG images, plan JSON files, and score reports.

Use this tool to evaluate room plan quality, compare candidate rooms, profile the planner, and iterate on planning algorithms without deploying to the game.

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| *(none)* | yes | Core benchmarking tool. Plans rooms, renders images, outputs plan JSON and scores. Uses rayon for parallel room processing. |
| `profile` | | Enables profiling via `screeps-timing`. Outputs Chrome-compatible trace JSON files. Disables rayon parallelism so traces are single-threaded. |

## Prerequisites

- **Rust toolchain** — stable Rust with Cargo.
- **Map data** — A JSON file containing room terrain and object data. Pre-built map files for MMO shards are included in `resources/`:
  - `resources/map-mmo-shard0.json`
  - `resources/map-mmo-shard1.json`
  - `resources/map-mmo-shard2.json`
  - `resources/map-mmo-shard3.json`

## Usage

### Build

```bash
# Debug build
cargo build -p screeps-foreman-bench

# Release build (recommended for benchmarking)
cargo build -p screeps-foreman-bench --release
```

### Commands

The tool provides three subcommands: `plan`, `compare`, and `list-rooms`.

#### `plan` — Plan a Single Room

Runs the planner on a single room and writes output files to the output directory.

```bash
cargo run -p screeps-foreman-bench --release -- plan \
  --map resources/map-mmo-shard3.json \
  --room W3S52 \
  --shard shard3 \
  --output output
```

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `--map`, `-m` | yes | | Path to the map data JSON file. |
| `--room`, `-r` | yes | | Room name to plan (e.g. `W3S52`). |
| `--shard`, `-s` | no | `shard1` | Shard name, included in output metadata. |
| `--output`, `-o` | no | `output` | Output directory for generated files. |

**Output files:**
- `<room>.png` — 500×500 pixel image with terrain and planned structures rendered.
- `<room>_plan.json` — Serialized plan in a format compatible with Screeps room planner tools.

#### `compare` — Plan and Rank Multiple Rooms

Plans multiple rooms (optionally in parallel) and prints a score comparison table.

```bash
# Compare specific rooms
cargo run -p screeps-foreman-bench --release -- compare \
  --map resources/map-mmo-shard3.json \
  --rooms W3S52,E11N11,E12N8

# Auto-select rooms with 2 sources and a controller (default: top 20)
cargo run -p screeps-foreman-bench --release -- compare \
  --map resources/map-mmo-shard3.json \
  --limit 50
```

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `--map`, `-m` | yes | | Path to the map data JSON file. |
| `--rooms`, `-r` | no | *(auto)* | Comma-separated room names. If omitted, auto-selects rooms with 2 sources and a controller. |
| `--shard`, `-s` | no | `shard1` | Shard name, included in output metadata. |
| `--limit`, `-n` | no | `20` | Maximum rooms to plan when `--rooms` is omitted. |
| `--output`, `-o` | no | `output` | Output directory for generated files. |

**Score table columns:**

| Column | Metric |
|--------|--------|
| Total | Weighted composite score |
| SrcDst | Source distance (closer is better) |
| CtrlD | Controller distance (closer is better) |
| Hub | Hub quality (centrality and accessibility) |
| Tower | Tower coverage (defensive reach) |
| ExtEff | Extension efficiency (fill path length) |
| Upkeep | Road/rampart upkeep cost |

#### `list-rooms` — List Rooms in a Map File

Lists rooms in a map data file, optionally filtered by source count or controller presence.

```bash
# List all rooms
cargo run -p screeps-foreman-bench --release -- list-rooms \
  --map resources/map-mmo-shard3.json

# List rooms with exactly 2 sources and a controller
cargo run -p screeps-foreman-bench --release -- list-rooms \
  --map resources/map-mmo-shard3.json \
  --sources 2 --has-controller
```

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `--map`, `-m` | yes | | Path to the map data JSON file. |
| `--sources` | no | *(any)* | Only list rooms with this many sources. |
| `--has-controller` | no | `false` | Only list rooms that have a controller. |

### Profiling

Build and run with the `profile` feature to generate Chrome-compatible trace files:

```bash
cargo run -p screeps-foreman-bench --release --features profile -- plan \
  --map resources/map-mmo-shard3.json \
  --room W3S52
```

This produces an additional `<room>_trace.json` file in the output directory. Open it in `chrome://tracing` or [Perfetto](https://ui.perfetto.dev/) to inspect the planning pipeline timing.

Note: profiling disables rayon parallelism so that the trace captures a single-threaded execution.

## Map Data Format

Map data files are JSON with the following structure:

```json
{
  "rooms": [
    {
      "room": "W3S52",
      "terrain": "<2500-char hex string>",
      "objects": [
        { "type": "source", "x": 12, "y": 34 },
        { "type": "controller", "x": 25, "y": 10 },
        { "type": "mineral", "x": 40, "y": 45 }
      ]
    }
  ]
}
```

| Field | Description |
|-------|-------------|
| `room` | Room name (e.g. `W3S52`). |
| `terrain` | 2500-character hex string. Each character is a terrain mask for one tile (0 = plain, 1 = wall, 2 = swamp, 3 = wall+swamp). Tiles are in row-major order (x changes fastest). |
| `objects` | Array of game objects with `type`, `x`, and `y` fields. Recognized types: `source`, `controller`, `mineral`. |

## Output

All output files are written to the output directory (default: `output/`, gitignored).

| File | Description |
|------|-------------|
| `<room>.png` | 500×500 pixel rendering. Black = wall, white = swamp, gray = plain. Structures are color-coded (yellow = spawn, orange = extension, red = tower, cyan = storage, green = rampart, etc.). |
| `<room>_plan.json` | JSON plan compatible with Screeps room planner tools. Contains structure positions grouped by type, with room name, shard, and RCL metadata. |
| `<room>_trace.json` | *(profile feature only)* Chrome tracing format. Open in `chrome://tracing` or Perfetto. |

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                     Map Data (JSON)                        │
│  resources/map-mmo-shard3.json                            │
│  { rooms: [{ room, terrain, objects }] }                  │
└────────────────────────┬─────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────┐
│               BenchRoomData (deserialized)                 │
│  • name, terrain (Vec<u8>), objects (Vec<Value>)          │
│  • get_terrain() → FastRoomTerrain                        │
│  • get_sources/controllers/minerals() → Vec<PlanLocation> │
└────────────────────────┬─────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────┐
│            RoomDataPlannerDataSource                       │
│  impl PlannerRoomDataSource                               │
│  (bridges bench data → screeps-foreman planner interface) │
└────────────────────────┬─────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────┐
│              screeps-foreman::plan_room()                  │
│  (runs the full planning pipeline)                        │
└────────────────────────┬─────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────┐
│                    Output Generation                       │
│  • PNG image (terrain + plan visualization)               │
│  • Plan JSON (room planner compatible format)             │
│  • Score report (printed to stdout)                       │
│  • Trace JSON (profile feature only)                      │
└──────────────────────────────────────────────────────────┘
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| [screeps-foreman](../screeps-foreman/) | Room planning library (the system under test) |
| [clap](https://crates.io/crates/clap) | Command-line argument parsing with derive macros |
| [serde](https://serde.rs/) / [serde_json](https://crates.io/crates/serde_json) | Map data deserialization and plan serialization |
| [image](https://crates.io/crates/image) | PNG image generation for plan visualization |
| [rayon](https://crates.io/crates/rayon) | Parallel room processing in compare mode |
| [log](https://docs.rs/log/) / [env_logger](https://crates.io/crates/env_logger) | Logging (set `RUST_LOG=info` for verbose output) |
| screeps-timing *(optional, `profile` feature)* | Profiling and trace generation |

## License

See repository root for license information.
