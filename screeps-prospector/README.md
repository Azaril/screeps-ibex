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
(`screeps_prospector::{ops, cache, score, place, config}` over the
shared [`screeps-rest-api`](../screeps-rest-api) client), so other
tools (e.g. the `screeps-server-kit` toolkit) can drive the same code paths.

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
screeps.com targets also need `--shard` (e.g. `--shard shard3`) —
enforced when the server exposes its shard list: the CLI checks the
flag against `/api/game/shards/info` up front and the error names the
available shards, instead of letting a scan spend quota-capped calls
to learn the shard was missing or misspelled. Note the cache is
per-shard (`cache/<shard>.json`), so every later command needs the
same `--shard`.

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

No server needed: import an existing map JSON (any file in the
foreman-bench map shape — e.g. a [screeps-foreman-bench](../screeps-foreman-bench)
export, if you have that repo) and score it. **This is an optional
import path, not a setup step** — with a server, `scan` + `fetch`
(above) is the happy path.

```console
$ cargo run -- cache seed --from path/to/map.json
$ cargo run -- score               # stage-1 heuristic table
$ cargo run -- recommend --top 3   # + foreman planning for the top 3
```

`score` and `recommend` make **no API calls** — they operate entirely on
the cache.

### Commands

Global flags (all commands): `--server-name <entry>`, `--shard <name>`,
`--config <path>` (credentials file), `--cache-file <path>` (default
`cache/<shard-or-server>.json`), `--min-delay-ms <ms>`,
`--include-novice` (see `scan` below).

#### `scan --rooms W1N1,W2N1 | --all [--status-ttl-secs 3600]`

Batched `map-stats` over the named rooms (or the whole map, enumerated
via `world-size`). Records per-room spawnability in the cache:
`open` (exists, unowned, unreserved, and claimable by name — highways
and source-keeper rooms are excluded, since map-stats reports those as
`"normal"` too), plus `novice`/`respawn` protection flags.

The default selections (`fetch`, `score`, `recommend`, `auto`) apply a
protection policy on top of `open`: **respawn-area rooms are always
included** (respawning is the prospector's primary workflow; the server
verifies actual eligibility at place-spawn) and **novice-area rooms are
excluded** unless you pass the global `--include-novice` (novice areas
only admit qualifying low-GCL accounts). Explicit `--rooms` lists are
never filtered. Batches are 1000 rooms per call,
so an MMO shard's ~15k rooms is **15 map-stats calls** — a quarter of
the 60/hour quota. **Resumable:** the cache is saved after every batch,
and re-runs skip rooms whose cached status is fresher than
`--status-ttl-secs` (default 3600; `0` = rescan everything), so an
interrupted scan loses at most one batch.

#### `fetch [--rooms W1N1,… | --all-open] [--status-ttl-secs 3600]`

Fetches terrain (`room-terrain?encoded=1`) and planner objects
(`room-objects`, filtered to sources/controller/mineral) into the cache.
The default (and `--all-open`) is every room the cache flags open — run
`scan` first. Terrain is immutable: once cached, never refetched (the
cheapest API call is the one not made — terrain is the quota-capped
endpoint on MMO at 360/hour). Planner objects are positionally
immutable too, so rooms with terrain **and** objects cached are skipped
entirely — interrupted runs resume. Progress is saved incrementally
(every status batch / every 16 rooms). Statuses older than the TTL are
refreshed in batched calls.

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

#### `cache stats` / `cache seed --from <map.json> [--force]`

`stats` (alias `info`): path, room/terrain/status counts. `seed` copies
a map JSON (foreman-bench shape) in as the cache (the source file is
never modified). `--from` is **required** — there is no default path
(repo-relative defaults break for external users; P0.P8); seeding is an
optional import for niche cases, not part of setup.

### MMO (screeps.com) — recommend-first flow

```console
$ cargo run -- --server-name mmo --shard shard3 scan --rooms W21N5,W22N5,W23N5
$ cargo run -- --server-name mmo --shard shard3 fetch
$ cargo run -- --server-name mmo --shard shard3 recommend
$ cargo run -- --server-name mmo --shard shard3 place \
    --room W22N5 --x 24 --y 21 --yes --i-understand-this-is-mmo
```

Prefer `scan --rooms` over `--all` on MMO (scan the area you actually
want to settle) — but `--all` is survivable now: see below.

#### MMO quota realities

screeps.com limits auth tokens on **two layers**
([auth-tokens docs](https://docs.screeps.com/auth-tokens.html); the
per-endpoint table is pinned in
[`screeps-rest-api`](../screeps-rest-api/README.md#the-pinned-per-endpoint-quota-table)
from node-screeps-api):

| What prospector calls | Quota | Sustained pacing |
|---|---|---|
| everything (global) | 120/minute | `--min-delay-ms`, default 600 |
| `POST map-stats` (scan + status refresh) | **60/hour** | one call / 60 s |
| `GET room-terrain` (fetch) | **360/hour** | one call / 10 s |
| `GET room-objects` (fetch) | global only | 600 ms |

Pacing is **evidence-based** — the table above is a worst-case bound,
not a schedule. Calls run at full speed until screeps.com itself
reports limiting: `X-RateLimit-*` response headers pace out the rest of
the window, and a 429 logs `rate limited; resuming in Xs (endpoint
quota: …)`, waits out the server's stated retry-after, and resumes
instead of killing the run — worst case **one 429 per window**. Long
waits (≥ 2 min) print once the sanctioned opt-out
(`https://screeps.com/a/#!/account/auth-tokens/noratelimit` — disable
rate limiting per token, your choice to use).

A token with the noratelimit opt-out enabled needs **no
configuration**: it receives no rate-limit headers and never 429s
(verified live 2026-06-10), so nothing ever slows it. The opt-out lifts
the global 120/minute cap too, so the only knob worth touching is the
courtesy delay:

```console
$ cargo run --release -- --server-name mmo --shard shardX --min-delay-ms 100 fetch
```

What that means in practice, with the **worst-case ETA printed up
front** before the first call:

- `scan --all` on a ~15k-room shard → 15 map-stats calls (1000
  rooms/batch) → inside one 60/hour window, so it completes at full
  speed (seconds) for every token.
- `fetch` of N open rooms → N room-terrain calls → full speed until the
  server pushes back; a rate-limited token averages 360 rooms per
  hourly window (≈ 10 s/room) on big runs, **printed before starting**
  with the suggestion to scope `--rooms` to a region instead.

Everything is **resumable**: scan saves the cache after every batch and
skips fresh statuses on re-run; fetch skips rooms whose terrain+objects
are already cached and saves incrementally. An interrupted run (Ctrl-C,
network, an exhausted window) loses at most one batch, and the cache
exists precisely so re-runs cost zero API calls.

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
| `config`    | `.screeps.yaml` parsing, server selection, `SecretString` secrets policy, the official-server classification (`is_official`, delegating to the shared `is_official_target`); re-exports the shared `AuthMode` |
| *(shared)* [`screeps-rest-api`](../screeps-rest-api) | REST client (P0.A12 — one client, not N): signin/token auth + rotation, world-size/map-stats/room-terrain/room-objects/room-status/world-status/place-spawn/respawn (+ memory segments, code upload, console socket for the other consumers), courtesy rate limit + official per-endpoint quota pacing with 429 backoff-and-resume, typed error envelope |
| `cache`     | File-backed room cache in the foreman-bench map-JSON shape; upsert semantics; terrain decode bridge to `FastRoomTerrain` |
| `ops`       | `scan`/`fetch` flows over client + cache (resumable: per-batch persistence, fresh-status / fully-fetched skipping); quota call-count/ETA planning (`plan_scan`/`plan_fetch`); pure status derivation |
| `score`     | Two-stage scoring: stage-1 heuristics, stage-2 offline foreman planning, first-spawn extraction |
| `place`     | Confirmation gates (the MMO safety model) + placement description |
| `main`      | Thin clap CLI over all of the above |

### Endpoint pinning

Endpoint shapes are pinned from public client implementations — never
guessed — and unit-tested against recorded/literal fixtures (no network
in tests). Since P0.A12 the pins live in the shared
[`screeps-rest-api`](../screeps-rest-api) crate (sources cited
per-method there: the operator-referenced
[Qionglu735/screeps_tool `screeps_api.py`](https://github.com/Qionglu735/screeps_tool/blob/master/screeps_api.py),
screepers/python-screeps `screepsapi.py`, screepers/node-screeps-api
`Endpoints.md`, the live private server, and
docs.screeps.com/auth-tokens.html for the official rate limits).
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

### `screeps-game-api` types — P0.P6 verdict: NOT adopted (blocked)

Investigated 2026-06-10 (timeboxed): using the local
`../screeps-game-api` fork's pure types (`local::RoomName`, coordinate
types, terrain enums) host-side instead of treating room names as
opaque strings.

**Blocked by a dependency-version skew, not by the wasm externs.** The
fork (0.23.1) declares `js-sys = "0.3"`; a fresh resolution in this
crate's graph picks current js-sys (0.3.100), against which the fork
fails to compile with E0282 type-inference errors (two call sites:
`src/objects/impls/store.rs:44` and the analogous `Object::keys`
caller — newer js-sys gave `Object::keys` a type parameter). The bot
workspace compiles only because its committed `Cargo.lock` pins js-sys
0.3.85; that pin cannot be replicated here cleanly — js-sys 0.3.85
requires `wasm-bindgen = "=0.2.108"`, which conflicts with the rest of
this crate's already-locked graph (it also contains screeps-foreman's
crates-io `screeps-game-api 0.23` copy). The clean unblock is a small
upstream-quality fork fix (annotate the two `Object::keys` call sites),
which is outside this crate's scope (AGENTS.md §7). Until then this
crate keeps room names as opaque keys — it performs no room-name
arithmetic, so the cost is validation-only. Revisit when the fork
compiles against current js-sys.

Same lifecycle as `screeps-server-kit` (Phase-0 decision D-1): in-repo and
workspace-excluded now, extracted to its own repository once stable.
The crate is self-contained except for three documented seams: the
credentials path (`../.screeps.yaml`, overridable via `--config`), the
`screeps-rest-api` path dependency (the shared Screeps client, same
D-1 lifecycle), and the `screeps-foreman` path dependency (plus its
`[patch]` table for foreman's git deps —
`screeps-cache`/`screeps-common` redirected to local clones, mirroring
the bench). At extraction the path deps become git deps and the patch
table collapses.

### Verification status

Offline behavior (P0.P1–P0.P3 + the offline half of P0.P4) is fully
tested: 51 tests in this crate against literal/recorded fixtures and a
copy of the bench map (plus the endpoint-shape and quota/rate-limit
pins, now 39 tests in the shared `screeps-rest-api` crate), no network,
no Docker. The MMO quota plan math (batch construction for a 14 884-room
shard, sustained ETAs, resume skipping) is unit-tested pure; the
pacing/backoff behavior against real screeps.com headers awaits the
operator's next MMO run (the local private server enforces no limits
and sends no rate-limit headers).

**Live (2026-06-10):** the private-server `auto --yes` end-to-end (the
P0.P4 "done when") ran green against the eval stack — scan (144 rooms,
35 open) → fetch → recommend (8 full-profile foreman plans) → placed
the best room's plan-derived spawn tile (W2N2 (19,22) on the default
map), with the world-status guard verified (`empty` → proceed;
`normal` → refuse). The same run caught and pinned one live shape: the
private server returns room-objects **without** an `ok` field (fixed +
fixture-pinned in `screeps-rest-api`). The `screeps-server-kit`
bootstrap integration (`spawnPlacement: prospector` in its
`config/local.yml`) is also live-verified — it drives this crate's
`ops`/`score` as a library and places the identical room/tile.
