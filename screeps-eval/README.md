# screeps-eval

Private-server execution, deployment, and evaluation harness for
[screeps-ibex](..) — a **library** (driven by tests and automation) plus a
**CLI** for the operator's manual iteration loop.

Task map and acceptance criteria: [`docs/execution/phase-0.md`](../docs/execution/phase-0.md)
(Workstream A, P0.A1–A8). Design: [ADR 0006](../docs/design/0006-eval-and-iteration-harness.md).

## Status

| Capability | Task | Status |
|---|---|---|
| Crate scaffold, config + secrets policy, CLI surface | P0.A1 | ✅ |
| Server lifecycle (bollard ↔ Docker Desktop) | P0.A2 | ✅ |
| World bootstrap (server CLI) | P0.A3 | ✅ |
| Deploy (deploy.js wrapper) | P0.A4 | ⬜ |
| Console/metrics capture → `runs/` | P0.A5 | ⬜ |
| Smoke loop + baselines | P0.A6 | ⬜ |
| Secrets sweep & pins | P0.A7 | ✅ (pins incl. CLI-payload mask) / ⬜ (sweep) |
| Operator mode (cli/tick/open/status) | P0.A8 | ✅ |

---

## Usage

This crate is **workspace-excluded** and **host-native**. Always invoke
from inside this directory (cargo config discovery is CWD-based; from the
repo root you'd inherit the wasm32 default target):

```
cd screeps-eval
```

### Server lifecycle

```
cargo run -- server up        # start the whole stack, wait until the API answers
cargo run -- server status    # container table + live API/CLI probes
cargo run -- server logs              # last 100 launcher log lines
cargo run -- server logs -f --tail 20 # follow new output (Ctrl-C to stop)
cargo run -- server down      # stop containers; keep all data (warm restart later)
cargo run -- server destroy --yes  # remove containers + network + VOLUMES (world gone)
```

- `server up` is idempotent: it pulls missing images, creates missing
  network/volumes/containers, starts whatever is stopped, and waits for
  `http://127.0.0.1:<game-port>/api/version` to answer.
  **First boot takes ~10 minutes** (the launcher npm-installs the server
  and mods inside the container; progress is logged every 15 s). Warm
  restarts after `server down` take seconds.
- `server status` discovers the **actual** published ports from
  `docker inspect` — it reports correctly even for a stack started
  manually (e.g. via the compose file in a launcher clone).
- `server down` vs `destroy`: `down` keeps the installed server and the
  world (volumes); `destroy` is the factory reset and therefore requires
  `--yes`.

Example `status` output:

```
CONTAINER              IMAGE                              STATE    HEALTH   PUBLISHED PORTS
screeps-eval-launcher  screepers/screeps-launcher:latest  running  -        21025/tcp -> 0.0.0.0:21025, 21026/tcp -> 0.0.0.0:21026
screeps-eval-mongo-db  mongo:8                            running  healthy  -
screeps-eval-redis-db  redis:7                            running  healthy  -

game API  http://127.0.0.1:21025/api/version -> 200 OK
server CLI  tcp://127.0.0.1:21026 -> connectable
```

### World bootstrap

```
cargo run -- bootstrap            # converge a running world to .screeps.yaml
cargo run -- bootstrap --reset    # factory-fresh world first (system.resetAllData)
```

`bootstrap` makes the world match the config, idempotently:

1. ensures the stack is up (same code path as `server up`),
2. with `--reset`: `system.resetAllData()` — wipes mongo (re-seeded with
   the default 11×11 map, W0N0–W10N10, 4 NPC corner bots) and **flushes
   redis**, then waits for the server to settle,
3. applies `eval.tickMs` and confirms by reading it back — always, because
   a reset leaves the main loop **unthrottled** (the tick-duration env key
   lives in the flushed redis),
4. ensures the bot user from `servers.<name>` exists: registers it fresh
   (`POST /api/register/submit`) or, if it already exists, converges the
   password via the server-CLI `setPassword(...)` (always logged masked:
   `setPassword("user", "***")`),
5. signs in via `POST /api/auth/signin` — proving the configured
   credentials actually work,
6. places the first spawn if none exists (see below; a world status of
   `lost` triggers `utils.respawnUser` first), and
7. verifies the user's world status is `normal`.

Example output (fresh world):

```
world:  reset (fresh)
tick:   100 ms (read back from the server)
user:   created (registered fresh)
spawn:  Spawn1 placed @ W7N4 (25,30)
status: normal
web client: http://127.0.0.1:21025/
```

**Spawn placement** is fully programmatic — no web-client step. Default is
auto-pick: candidate rooms (status `normal`, exactly one unowned/
unreserved controller, ≥ 2 sources) are queried through the server CLI,
ordered central-first, and a tile is chosen from the room terrain (plain
beats swamp, all 8 neighbors open, ≥ 2 tiles from sources/controller/
mineral, close to their centroid). The actual placement goes through the
game API (`POST /api/game/place-spawn`), which re-validates everything
server-side; rejected tiles fall through to the next candidate.

Override via the optional `eval.spawn:` section of `.screeps.yaml`
(documented here because `.example-screeps.yaml` is docs-owned):

```yaml
eval:
  spawn:
    room: W5N3   # room only: auto-pick the tile within this room
    x: 18        # room + x + y: place exactly here (no fallback —
    y: 14        #   explicit config wins or the bootstrap fails loudly)
```

### Server-CLI passthrough (operator mode)

```
cargo run -- cli "system.getTickDuration()"   # one-shot: print the response
cargo run -- cli                              # interactive REPL
```

The REPL talks to the same launcher CLI endpoint the `screeps-launcher
cli` client uses — any JavaScript is allowed (`help()` lists the
objects: `storage`, `map`, `bots`, `strongholds`, `system`, plus the
mods' `utils.*` and `setPassword`). `quit`, `exit`, or Ctrl-C leaves.

```
screeps> system.pauseSimulation()
OK
screeps> storage.db['users'].count()
9
screeps> quit
```

Secrets: any `setPassword(...)` call is **masked in everything this tool
echoes or logs** — piped-input echoes, one-shot output, and error bodies
(the server's vm errors quote the offending source line back, so even
responses are masked): you will only ever see `setPassword("user", "***")`.
The unmasked payload exists solely inside the HTTP request body.

### Tick control (operator mode)

```
cargo run -- tick set 1000    # comfortable watching speed
cargo run -- tick set 100     # eval default; floor 50 ms, warns below 100
cargo run -- tick pause       # freeze the simulation (prints the tick)
cargo run -- tick resume      # un-freeze
```

`tick set` applies `system.setTickDuration(ms)` and **confirms by reading
the value back**; it fails loudly on a mismatch. Pause/resume wrap
`system.pauseSimulation()`/`resumeSimulation()` and print the current game
time so you can see where it stopped.

### Other commands

```
cargo run -- config           # resolved config, secrets redacted by construction
cargo run -- open             # print/launch the web-client URL
cargo run -- deploy              # (P0.A4) build + upload the bot
cargo run -- run --ticks 200     # (P0.A5) capture console+metrics to runs/
cargo run -- smoke               # (P0.A6) up -> bootstrap -> deploy -> run
cargo test                       # all unit tests incl. the secrets pins
```

## Configuration

Reads the repo's `.screeps.yaml` (gitignored; keyless template:
[`.example-screeps.yaml`](../.example-screeps.yaml)) — the same unified
config the deploy tooling uses — discovered by walking up from the
current directory. The harness consumes the `private-server` entry of
`servers:` (override with `--server-name`) plus an optional `eval:`
section:

```yaml
eval:
  # EITHER: use an existing screeps-launcher config as the base, VERBATIM,
  # including its steamKey (e.g. a local clone of the launcher repo):
  serverConfig: C:\code\screeps-launcher\config.yml
  # OR: supply only the Steam Web API key; the vendored keyless template
  # (screeps-eval/server/config.yml) is then the base:
  #steamKey: your-steam-web-api-key-here
  ports:
    game: 21025   # game/API port (published host-side and bound in-container)
    cli: 21026    # server CLI port (likewise)
  tickMs: 100     # written to serverConfig.tickRate; floor 50 (plan D-2)
```

Precedence rules (all merging is in-memory; see Design below):

1. `serverConfig:` set → that file is the base. A `steamKey` already in
   it **wins**; `eval.steamKey` only fills the gap if the base has none.
2. `serverConfig:` unset → the vendored keyless template is the base and
   `eval.steamKey` is **required** (the screeps backend cannot start
   without a key — get one at <https://steamcommunity.com/dev/apikey>).
3. Whatever the base says, the harness **forces** the in-container binds:
   game `0.0.0.0:<ports.game>`, CLI `0.0.0.0:<ports.cli>` (the launcher's
   default CLI bind is in-container `127.0.0.1:21026` — unreachable from
   the host), and sets `serverConfig.tickRate: <tickMs>`.
4. The `eval:` section is optional; absent = all defaults
   (template base — so `steamKey` must come from somewhere, see 2).

`SCREEPS_EVAL_HOST/PORT/STEAM_KEY` env overrides exist as optional code
paths, but the **documented mechanism is file-driven** — no host env vars
are required for any flow (operator directive).

## Secrets rules (P0.A7 — enforced, not aspirational)

- Credentials (`password`, `eval.steamKey`) are `secrecy::SecretString`
  from the moment of parsing — `Debug`/`Display` redact by construction,
  enforced by a pin test (`config::tests::debug_output_redacts_secrets`).
- The **merged runtime config necessarily contains the steamKey**. It is
  written ONLY to `screeps-eval/target/runtime/config.yml` (gitignored
  via the repo-global `target` rule) and bind-mounted into the container.
  Never copy it anywhere else; it is part of the A7 manual sweep.
- The vendored template `server/config.yml` is committed and **must stay
  keyless** — pinned by `server_config::tests::vendored_template_is_keyless`.
  Never copy a real launcher config over it (its first line is typically
  the live key).
- The server-CLI send path is the one place a credential exists as
  plaintext (`setPassword("user", "pw")` post-`expose_secret()`). That
  string reaches **only** the HTTP request body; every echo, log, and
  error path (including server error bodies, whose vm stack traces quote
  the command source) passes through `server::mask_cli_command` — pinned
  by `server::tests::mask_pin_setpassword_payload_is_masked_in_display`.
- The signin/register request bodies also carry the password; they are
  built inline from `expose_secret()` and never logged (success/failure
  only).

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `connecting to Docker — is Docker Desktop running?` | Start Docker Desktop and wait for the engine, then re-run. |
| `server up` seems stuck after image pulls | First boot npm-installs the server in-container (~10 min budget). Progress lines show the launcher's latest log every 15 s; watch detail with `cargo run -- server logs -f`. |
| `no Steam key available: ...` | Set `eval.steamKey` in `.screeps.yaml`, or point `eval.serverConfig` at a launcher config that has one. |
| Port already in use (create/start error mentioning `0.0.0.0:21025`) | Another server (or a manually-started launcher stack) holds the port. Stop it, or set `eval.ports.game/cli` to free ports, `server destroy --yes`, `server up`. |
| Changed `eval.ports` but `status` shows the old ones | Published ports are fixed at container creation; `up` warns about this. `server destroy --yes` then `server up`. |
| `server CLI tcp://127.0.0.1:21026 -> refused/timeout` in `status` | The CLI bind is forced to `0.0.0.0` in the merged config, but if you're running a foreign/manually-started stack (which binds in-container `127.0.0.1` by default and publishes no CLI port), use the fallback: `docker exec -it screeps-eval-launcher screeps-launcher cli` (substitute the container name from `status`). |
| World/server behaving oddly after config experiments; `mongosh`/auth errors in logs | Stale volumes from an earlier configuration. Factory-reset: `server destroy --yes` then `server up` (full first boot again). |
| `launcher container exited while waiting for the API` | The error includes the last launcher logs. Typical causes: malformed base config (check `eval.serverConfig`), no/invalid steam key, mongo/redis not healthy (re-run `server up`). |
| `signin as '<user>' rejected (401)` | The server-side password differs from `.screeps.yaml` (e.g. the entry changed since the user was created). Run `bootstrap` — it converges the password via `setPassword` before signing in. |
| `registering user ... failed: Registration is automatically disabled` | The server has the `SERVER_PASSWORD` env var set (screepsmod-auth closes registration). Not set by this harness — check a custom `eval.serverConfig` base for an `env:` entry. |
| `eval.spawn.room ... is not a valid first-spawn room` | The room lacks an unowned controller or 2 sources (the error lists valid candidates), or the world was seeded with a different map. Pick a listed room or drop `eval.spawn.room` for auto-pick. |
| Simulation racing (hundreds of ticks/s) after a manual CLI `system.resetAllData()` | A reset flushes redis, including the tick duration — the loop runs unthrottled. `tick set 100` (or re-run `bootstrap`, which always re-applies `eval.tickMs`). |

---

## Design

### Module map

```
src/lib.rs            library root (the CLI is a thin wrapper — automation
                      and operator share every code path)
src/config.rs         .screeps.yaml loading + EvalSettings (`eval:` section),
                      secrets policy (SecretString from parse time)
src/server_config.rs  launcher-config preparation: base -> PURE merge ->
                      target/runtime/config.yml (the only sanctioned on-disk
                      location for the steamKey)
src/docker.rs         bollard lifecycle: images/network/volumes/containers,
                      health-waits, status introspection, logs, down/destroy
src/server.rs         server-CLI client (CliClient), bootstrap-scoped game-API
                      client (GameApi), bootstrap flow, tick control, the
                      setPassword mask, spawn-tile picking (pure, unit-tested)
src/main.rs           clap CLI (incl. the interactive REPL)
server/config.yml     vendored KEYLESS launcher-config template (committed)
```

### Config-merge flow

```
.screeps.yaml (gitignored)                 screeps-eval/server/config.yml
  eval.serverConfig path ──┐                 (vendored, keyless) ──┐
                           │  (if set)                (else)       │
                           ▼                                       ▼
                       base launcher config  ◄─────────────────────┘
                           │
   eval.steamKey ──────────┤  merge (in-memory, pure fn):
   eval.ports ─────────────┤   - steamKey: base wins, eval fills gap
   eval.tickMs ────────────┘   - force env.backend GAME/CLI binds 0.0.0.0:ports
                           │   - serverConfig.tickRate = tickMs
                           ▼
        screeps-eval/target/runtime/config.yml   (gitignored)
                           │
                           ▼  bind-mount
        launcher container /screeps/config.yml
```

### Server-CLI protocol (pinned live 2026-06-09)

The endpoint on port 21026 is an HTTP server installed by the launcher
itself as a mod (`/screeps/mods/screeps-launcher-cli.js`; the launcher's
own `cli/cli.go` client speaks the same shapes):

- `GET /greeting` → 200, plain-text banner.
- `POST /cli`, body = raw JavaScript (any/no content type). The command
  runs in a Node `vm` sandbox; a returned Promise is **awaited
  server-side** before the response completes.
- Response: **always HTTP 200**, plain text, trailing newline.
  Intermediate `print()` output streams as its own lines; the final line
  is the result — `util.inspect(result)` for non-strings (`100`, `null`,
  `undefined`, `'quoted string'`) or the raw string itself (which is how
  the JSON-returning bootstrap queries come back clean).
- Errors are **in-band**: still 200, body `Error: <err.stack>`. The vm
  stack quotes the offending command source line — the reason response
  bodies are masked, not just commands (`server::mask_cli_command`).

`CliClient::send_raw` returns whatever came back (REPL semantics);
`CliClient::send` converts `Error:`-prefixed bodies into masked `Err`s
(automation semantics — bootstrap and tick control use this).

### Bootstrap mechanism facts (pinned from the live container's sources)

- **Reset** — `system.resetAllData()` (screepsmod-mongo
  `lib/common/_connect.js`): re-imports `db.original.json` over dropped
  mongo collections (11×11 map W0N0–W10N10; NPC bot players in the four
  corner areas; Invader/Source Keeper users) and `env.flushall()`s redis.
  No restart required (verified live), but **game time, auth tokens,
  memory segments, and the tick duration are all wiped** — `bootstrap`
  therefore re-applies `eval.tickMs` unconditionally and read-back-checks
  it.
- **User creation** — screepsmod-auth `lib/register.js`:
  `POST /api/register/submit` `{username, password}` creates the user,
  an empty default code branch, and empty memory; open unless the server
  has `SERVER_PASSWORD` set. The CLI `setPassword` (`lib/cli.js`) is a
  `db.users.update` — a silent no-op for a missing user, hence
  register-first, converge-after.
- **Auth** — `POST /api/auth/signin` `{email: <username>, password}` →
  `{ok:1, token}` (passport local strategy with `usernameField: 'email'`).
  Authenticated calls send `X-Token`/`X-Username`; every response carries
  a **refreshed token** in its `X-Token` header which must be adopted
  (`GameApi` does).
- **Spawn placement** — `POST /api/game/place-spawn` `{room,x,y,name}`
  (`@screeps/backend lib/game/api/game.js`): server-side validation is
  x,y ∈ 1..=48, non-wall terrain, no exit object within 1 tile, room
  controller exists and is unowned/unreserved, user owns zero objects,
  not blocked, has cpu. Success claims the controller (level 1, safe mode
  +20k ticks) and returns `{ok:1, newbie:true}`. The tile *picking* is a
  pure local function (`server::pick_spawn_tiles`) so it is unit-tested;
  the server remains the validator of record.
- **World status** — `GET /api/user/world-status` → `empty` (no spawn),
  `normal`, or `lost` (owned controller, nothing left; `bootstrap` then
  runs `utils.respawnUser` and waits for `empty`).

### Launcher schema facts (pinned from screepers/screeps-launcher @ main)

- `launcher/config.go`: the `cli:` config section (`host`/`port`, defaults
  `127.0.0.1`/`21026`) is the **CLI client** connect target used by
  `screeps-launcher cli`. The **server-side bind** is
  `env.backend.CLI_HOST`/`CLI_PORT` (defaults `"127.0.0.1"`/`"21026"`) —
  in-container localhost, i.e. the default failure mode for host access;
  hence the forced merge. Game bind: `env.backend.GAME_HOST`/`GAME_PORT`
  (defaults `"0.0.0.0"`/`"21025"`).
- Launcher env maps are Go `map[string]string` — port values must be YAML
  strings in the merged config.
- `serverConfig.tickRate` (ms) requires `screepsmod-admin-utils`.
- `MONGO_HOST`/`REDIS_HOST` are read by `screepsmod-mongo` from the
  process environment; the launcher passes its container env through to
  the server processes (`launcher/server.go`, `os.Environ()`), so the
  harness sets them as container env — exactly like the compose
  reference (`C:\code\screeps-launcher\docker-compose.yml`).

### Stack topology & port discovery

Three containers on the `screeps-eval-net` network (aliases `mongo`,
`redis`), named volumes `screeps-eval-data` (`/screeps`),
`screeps-eval-mongo` (`/data/db`), `screeps-eval-redis` (`/data`).
Containers/network/volumes are all `screeps-eval-*`-prefixed so `destroy`
cannot touch anything else. In-container ports equal host-published ports
(the merge forces `GAME_PORT`/`CLI_PORT` to `eval.ports`), so there is one
port number per endpoint end-to-end.

`server status` does **not** echo the configuration back: it reads the
actually-published ports from container inspect (`NetworkSettings.Ports`)
and probes those — so it tells the truth about a manually-started or
half-broken stack. The launcher container is found by canonical name
first, then by image (`*screeps-launcher*`) as a fallback.

### Image policy: pull, not build

`screepers/screeps-launcher:latest`, `mongo:8`, `redis:7` are pulled from
the registry if absent — never built. Building the launcher image from
the local clone (`C:\code\screeps-launcher`) is a **recorded future
investigation**, not Phase-0 scope (P0.A2 operator decision): pulling
keeps the harness reproducible on any machine with no Go toolchain.

### Why workspace-excluded + the CWD caveat

The parent workspace's `.cargo/config.toml` sets the default target to
`wasm32-unknown-unknown`, and cargo config discovery walks up from the
**current directory**. This crate carries its own `.cargo/config.toml`
pinning the host triple, which only takes effect when commands run from
inside `screeps-eval/` — hence "always `cd screeps-eval` first". Run from
the repo root and you cross-compile the harness to wasm (it will fail
loudly, not subtly). The same CWD rule governs `.screeps.yaml` discovery
(walk-up), and `target/runtime/` resolves to this crate's compile-time
location while the crate lives in-repo.

### Extraction-to-submodule plan (decision D-1)

The crate starts in-repo (workspace-excluded) and extracts to a submodule
with its own remote once stable. Designed in from day one: no
workspace-crate dependencies, no repo-relative path assumptions beyond
the `.screeps.yaml` walk-up, self-contained README + example config, own
lockfile. At extraction time, revisit the one compile-time path
(`runtime_dir()` uses `CARGO_MANIFEST_DIR`) and the operator creates the
remote.
