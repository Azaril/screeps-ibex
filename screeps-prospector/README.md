# screeps-prospector

Spawn-site selection for [Screeps](https://screeps.com): scan a server
for rooms open to spawning, fetch terrain and objects into a local
cache, score the rooms (cheap heuristics, then real room-layout
planning), and place a spawn on the recommended tile — on explicit
confirmation only.

It is bot-agnostic: it talks to the standard Screeps HTTP API (private
servers and screeps.com) and recommends a placement any bot can use.
The layout planner it scores with is
[screeps-foreman](../screeps-foreman); the recommended tile is the spawn
position the foreman layout itself wants, so the spawn you place on day
one is already part of an optimal base plan.

Library-first: every CLI capability is a library function
(`screeps_prospector::{ops, cache, score, place, api, config}`), so other
tools (e.g. the `screeps-eval` harness) can drive the same code paths.

---

## Usage

### Prerequisites

- Rust (stable). **Run from this directory** (`cd screeps-prospector`) —
  paths are fixed by convention: credentials default to
  `../.screeps.yaml`, the cache lives in `./cache/`, and the crate-local
  `.cargo/config.toml` pins a host (non-wasm) build target.
- Credentials in `../.screeps.yaml` under `servers:` — the same file the
  screeps deploy tooling uses. Private servers use username/password;
  official servers use an [auth token](https://docs.screeps.com/auth-tokens.html):

```yaml
servers:
  private-server:
    host: 127.0.0.1
    port: 21025
    secure: false
    username: my-bot
    password: my-password
  mmo:
    host: screeps.com
    secure: true
    token: 00000000-aaaa-bbbb-cccc-dddddddddddd
```

Secrets policy: passwords/tokens are parsed straight into
`secrecy::SecretString` — `Debug`/`Display` output redacts them by
construction (pinned by tests). The file is read at runtime only; tests
never touch it. Nothing secret is ever written to the cache.

Select the entry with `--server-name` (default: `private-server`).
Official servers also need `--shard` (e.g. `--shard shard3`).

### Quick start — private server

```console
$ cargo run -- scan --all          # which rooms are open for spawning?
$ cargo run -- fetch               # terrain + objects for the open rooms
$ cargo run -- recommend           # rank them; plan the finalists
$ cargo run -- place --room W2N2 --x 19 --y 22 --yes
```

Or the whole thing in one command (private servers only):

```console
$ cargo run -- auto --yes
```

`auto` runs scan → fetch → recommend → places the best room's
plan-derived spawn tile. It refuses to run without `--yes`, refuses if
the account already has a spawn (world-status `normal`), and is
**refused outright against official servers** (see
[MMO safety](#mmo-safety-model)).

Tip: against a local private server, add `--min-delay-ms 50` — the
default API pacing (600 ms) is sized for screeps.com.

### Quick start — fully offline

No server needed: seed the cache from a
[screeps-foreman-bench](../screeps-foreman-bench) map dump and score it.

```console
$ cargo run -- cache seed          # copies the bench private-server map
$ cargo run -- score               # stage-1 heuristic table
$ cargo run -- recommend --top 3   # + foreman planning for the top 3
```

`score` and `recommend` make **no API calls** — they operate entirely on
the cache.

### Commands

Global flags (all commands): `--server-name <entry>`, `--shard <name>`,
`--config <path>` (credentials file), `--cache-file <path>` (default
`cache/<shard-or-server>.json`), `--min-delay-ms <ms>`.

#### `scan --rooms W1N1,W2N1 | --all`

Batched `map-stats` over the named rooms (or the whole map, enumerated
via `world-size`). Records per-room spawnability in the cache:
`open` (exists, unowned, unreserved), plus `novice`/`respawn`
protection flags surfaced separately (whether you can use such rooms
depends on your account — you decide). On an MMO shard `--all` is
~15k rooms ≈ 230 API calls ≈ 2.5 minutes at the default pacing.

#### `fetch [--rooms W1N1,… | --all-open] [--status-ttl-secs 3600]`

Fetches terrain (`room-terrain?encoded=1`) and planner objects
(`room-objects`, filtered to sources/controller/mineral) into the cache.
The default (and `--all-open`) is every room the cache flags open — run
`scan` first. Terrain is immutable: once cached, never refetched (the
cheapest API call is the one not made — terrain is the rate-limited
endpoint on MMO at 360/hour). Statuses older than the TTL are refreshed
in one batched call.

#### `score [--rooms … | --all] [--w-* weights]`

Stage-1 heuristic table, offline. Room selection default: the cache's
open rooms; if the cache has no scan statuses at all (e.g. seeded from
a bench dump), every cached room. Rooms with missing data are
**fetch-listed** — the output ends with the exact `fetch --rooms …`
command to fill the gaps — never silently skipped.

Weights (`--w-sources 3 --w-controller 2 --w-mineral 0.5 --w-swamp 1
--w-walls 0.5 --w-exits 1` are the defaults): source count (2 strongly
preferred), controller presence, mineral presence/type, swamp fraction,
wall fraction, exit count/side distribution.

#### `recommend [--top 8] [--plan-timeout-secs 120] [--plan-profile full|reduced] [--rooms…|--all] [--w-*]`

The full two-stage pipeline, offline: stage-1 ranking, then a real
foreman plan for each of the top-N finalists. Output: ranked table
(room, heuristic score, plan score, **proposed spawn tile**, planning
seconds), a rejected list with reasons (no controller, planning failed,
budget exceeded), and the fetch list. The proposed tile is the plan's
RCL-1 spawn — where the optimal layout wants your first spawn.

`--plan-profile reduced` runs a hub-only layer stack — much faster,
coarser scores; for iteration, not for real placement. A finalist that
exceeds the planning budget is rejected with that reason (no automatic
backfill from rank N+1 — re-run with a higher `--plan-timeout-secs` or
build with `--release`, which is dramatically faster).

#### `place --room W2N2 --x 19 --y 22 [--name Spawn1] --yes [--i-understand-this-is-mmo]`

Places a spawn via `POST /api/game/place-spawn`. Always prints exactly
what it is about to do (server, room, tile, name) **before** doing
anything, and refuses without `--yes`. Against an official server entry
it additionally requires `--i-understand-this-is-mmo`.

#### `auto [--top 8] [--name Spawn1] --yes [--status-ttl-secs] [--plan-*] [--w-*]`

Private-server end-to-end: world-status check → scan whole map → fetch
open rooms → recommend → place the best room's spawn tile. Refused
outright against official servers — no flag combination unlocks it.

#### `cache stats` / `cache seed [--from <map.json>] [--force]`

`stats` (alias `info`): path, room/terrain/status counts. `seed` copies
a foreman-bench map JSON in as the cache (the source file is never
modified); default source is the bench's private-server map.

### MMO (screeps.com) — recommend-first flow

```console
$ cargo run -- --server-name mmo --shard shard3 scan --rooms W21N5,W22N5,W23N5
$ cargo run -- --server-name mmo --shard shard3 fetch
$ cargo run -- --server-name mmo --shard shard3 recommend
$ cargo run -- --server-name mmo --shard shard3 place \
    --room W22N5 --x 24 --y 21 --yes --i-understand-this-is-mmo
```

Prefer `scan --rooms` over `--all` on MMO (scan the area you actually
want to settle). Rate limits: tokens get 120 requests/minute globally
and per-endpoint caps (room-terrain: 360/hour); the client paces itself
(`--min-delay-ms`, default 600) and surfaces HTTP 429 as a clear error
(it does not auto-retry). The cache exists precisely so re-runs cost
zero API calls.

### Troubleshooting

- `no cache at …` — run `scan`+`fetch`, or `cache seed` for offline use.
- `planning failed: planning exceeded the …s budget` — a cramped room
  can blow the per-room budget in debug builds; use
  `cargo run --release -- recommend …` (much faster) or raise
  `--plan-timeout-secs`.
- `server 'x' not in .screeps.yaml` — the error lists the entries it
  found; check `--server-name` and `--config`.
- `world-status is 'normal'` from `auto` — the account already has a
  spawn; respawn first if you really intend to move.

---

## Design

### Module map

| Module      | Responsibility |
|-------------|----------------|
| `config`    | `.screeps.yaml` parsing, server selection, `SecretString` secrets policy, the official-server classification (`is_official`) |
| `api`       | REST client: signin/token auth + rotation, world-size/map-stats/room-terrain/room-objects/room-status/world-status/place-spawn/respawn, courtesy rate limit, typed error envelope |
| `cache`     | File-backed room cache in the foreman-bench map-JSON shape; upsert semantics; terrain decode bridge to `FastRoomTerrain` |
| `ops`       | `scan`/`fetch` flows over client + cache; pure status derivation |
| `score`     | Two-stage scoring: stage-1 heuristics, stage-2 offline foreman planning, first-spawn extraction |
| `place`     | Confirmation gates (the MMO safety model) + placement description |
| `main`      | Thin clap CLI over all of the above |

### Endpoint pinning

Endpoint shapes are pinned from public client implementations — never
guessed — and unit-tested against recorded/literal fixtures (no network
in tests). Sources, cited per-method in `src/api.rs`:
[Qionglu735/screeps_tool `screeps_api.py`](https://github.com/Qionglu735/screeps_tool/blob/master/screeps_api.py)
(the operator reference), screepers/python-screeps `screepsapi.py`,
screepers/node-screeps-api `Endpoints.md`, and
docs.screeps.com/auth-tokens.html for the official rate limits.
Notable pinned behaviors: username goes in the signin `email` field;
the session token is sent as both `X-Token` and `X-Username`; a
response `X-Token` header ≥ 40 chars rotates the stored token; the
`{"error": …}` envelope can arrive with HTTP 200 and is checked before
the typed parse.

### Cache schema & bench compatibility

The on-disk shape is exactly what `screeps-foreman-bench` loads
(`{description, rooms: [{room, terrain, objects, …}]}`), so a cache file
is a valid bench map and the bench's `resources/*.json` dumps are valid
cache seeds. Prospector extensions are optional keys the bench loader
ignores: `spawnStatus` (`{open, novice, respawn}` — named that because
the bench seed dumps already use `status` for a server-status string)
and `fetchedAt` (Unix seconds, drives the status TTL). Unknown per-room
keys in seed data (`bus`, `sourceKeepers`, …) are preserved verbatim
across load/upsert/save. The compatibility is pinned by a test that
deserializes a cache-extended file through a hand-maintained mirror of
the bench loader structs.

Upsert semantics: terrain is immutable (never overwritten, never
cleared, never refetched); `objects` replaced only by a non-empty
incoming array; status/`fetchedAt` replace when present; unknown keys
merge per-key. Saves are deterministic (rooms sorted, pretty JSON,
stable key order) for clean diffs.

### Two-stage scoring

**Stage 1 (cheap, every candidate):** weighted average of six
subscores in `[0,1]` — source count (2 → 1.0, the dominant default
weight), controller presence, mineral presence/type (X preferred, then
H/O), `1 - swamp fraction`, `1 - wall fraction`, and exit
count/distribution (fewer exit tiles on fewer sides = cheaper, more
defensible perimeter). Deterministic: exact arithmetic, room-name
tie-breaks. Rooms missing terrain/objects are fetch-listed; rooms
without a controller or sources are disqualified with a reason (foreman
planning requires both) and never reach stage 2.

**Stage 2 (expensive, top-N finalists):** a `PlannerRoomDataSource`
over the cached terrain+objects drives the full foreman planning
pipeline (the bench's offline pattern) under a wall-clock budget. The
plan's `PlanScore` ranks the finalists, and the plan's **first spawn**
becomes the recommended tile.

*Why foreman placement matters:* the first spawn anchors everything —
foreman layouts grow around the hub, and a spawn placed by eye
frequently ends up misplaced relative to the eventual optimal base
(blocking the hub stamp, far from sources). Planning before placing
means the tile you spawn on is the tile the RCL-8 layout wants.

*First-spawn extraction (pinned):* `Plan.build_order` is sorted by
priority desc → required RCL asc → hub distance
(`screeps-foreman/src/pipeline/finalize.rs`); spawns are
`BuildPriority::Critical` and exactly one spawn is planned at RCL 1
(the hub stamp's `sp(Spawn, -1, 0, 1)`, `stamps/hub.rs`). The minimum-
RCL spawn in the build order is therefore the layout's first spawn.

*Determinism:* the foreman search has no RNG and uses FNV maps
(reproducible iteration); a test plans the same bench room twice and
asserts identical spawn tile and score.

### MMO safety model

A server entry is classified **official** when it authenticates by
token *or* targets screeps.com (`ProspectorConfig::is_official` — token
entries get MMO-grade caution even against private hosts). The gates
are pure functions (`place::gate_place`/`gate_auto`) with the full
refusal matrix unit-tested:

| Command     | Private server  | Official server |
|-------------|-----------------|-----------------|
| `recommend` | always allowed (offline) | always allowed (offline) |
| `place`     | `--yes`         | `--yes` **and** `--i-understand-this-is-mmo` |
| `auto`      | `--yes`         | **refused outright** — checked before `--yes`, before any network I/O |

`place` prints the exact API call before gating; a refused command
performs zero network I/O.

### Extraction plan

Same lifecycle as `screeps-eval` (Phase-0 decision D-1): in-repo and
workspace-excluded now, extracted to its own repository once stable.
The crate is self-contained except for two documented seams: the
credentials path (`../.screeps.yaml`, overridable via `--config`) and
the `screeps-foreman` path dependency (plus its `[patch]` table for
foreman's git deps — `screeps-cache`/`screeps-common` redirected to
local clones, mirroring the bench). At extraction the path deps become
git deps and the patch table collapses.

### Verification status

Offline behavior (P0.P1–P0.P3 + the offline half of P0.P4) is fully
tested: 54 tests against literal/recorded fixtures and a copy of the
bench map, no network, no Docker. **Deferred:** the live private-server
`auto --yes` end-to-end (the P0.P4 "done when") runs once the
Workstream-A eval-server work settles and a private server is available
— the Workstream-P live-test constraint keeps this build offline.
