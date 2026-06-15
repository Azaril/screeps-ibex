# screeps-server-kit

A local private-server toolkit for [Screeps](https://screeps.com) bot
development — bring up the
[screepers/screeps-launcher](https://github.com/screepers/screeps-launcher)
Docker stack, bootstrap a world (users, spawns, tick rate), deploy your
bot, and capture console + metrics artifacts from a run. A **library**
(driven by automation and tests) plus an **operator CLI** for the manual
iteration loop.

**Mechanism, not policy.** This crate is bot-agnostic by design: it
contains no bot-specific log strings, gates, or scenario logic. What
"correct/healthy" means for a particular bot lives in a thin consumer
crate that supplies a marker spec to the capture mechanism and drives
the same library — this repo's consumer is
[`screeps-ibex-eval`](../screeps-ibex-eval) (the `smoke`/`run` commands live there).
Companion crates: [`screeps-rest-api`](../screeps-rest-api), the shared
HTTP/websocket client every endpoint call goes through, and
[`screeps-pack`](../screeps-pack), the npm-free build + deploy pipeline
`deploy` drives as a library.

---

## Usage

This crate is **workspace-excluded** and host-native. Invoke it from
inside this directory:

```
cd screeps-server-kit
```

First-time setup (one minute):

1. Credentials: the parent directory's `.screeps.yaml` needs a
   `servers:` entry per bot user (see [Configuration](#configuration) —
   username + password against `127.0.0.1:21025`).
2. Stack settings: `copy config\local.example.yml config\local.yml` and
   set `steamKey:` (the only required key — everything else defaults).
   `config/local.yml` is gitignored.

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
  manually (e.g. via a compose file).
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

> The Docker objects are named `screeps-eval-*` (network, containers,
> volumes) — the names predate this crate's rename and are kept so
> existing stacks are not orphaned; `destroy` only ever touches that
> prefix.

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
3. applies `tickMs` and confirms by reading it back — always, because
   a reset leaves the main loop **unthrottled** (the tick-duration env key
   lives in the flushed redis),
4. then **for each entry in `bots:`** (`config/local.yml`; default
   `["private-server"]`):
   - ensures that bot user exists: registers it fresh
     (`POST /api/register/submit`) or, if it already exists, converges
     the password via the server-CLI `setPassword(...)` (always logged
     masked: `setPassword("user", "***")`),
   - signs in via `POST /api/auth/signin` — proving the configured
     credentials actually work,
   - places its first spawn if none exists (see below; a world status
     of `lost` triggers `utils.respawnUser` first) in a room **no
     earlier bot claimed this run** — every bot gets a distinct room;
     the spawn is named after the bot entry (`ibex`, `ibex-2`, ...),
   - verifies that bot's world status is `normal`,
   - raises that bot's GCL to the configured `gcl:` level (default 10,
     **raise-only**) so it can own more than one room. Screeps caps owned
     rooms at the GCL level, so without this a fresh bot is stuck at a
     single room until it grinds ~1M control points for GCL 2 and its
     expansion logic never runs. `gcl: 1` disables the boost.

Example output (fresh world, two bots):

```
world:  reset (fresh)
tick:   100 ms (read back from the server)
[ibex] user:   ibex (registered fresh)
[ibex] spawn:  'ibex' placed @ W7N4 (25,30)
[ibex] status: normal
[ibex-2] user:   ibex-2 (registered fresh)
[ibex-2] spawn:  'ibex-2' placed @ W5N3 (18,14)
[ibex-2] status: normal
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

Override via the optional `spawn:` section of `config/local.yml`
(applies to the FIRST `bots:` entry; later bots always auto-pick a
distinct room):

```yaml
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
cargo run -- tick set 100     # automation default; floor 50 ms, warns below 100
cargo run -- tick pause       # freeze the simulation (prints the tick)
cargo run -- tick resume      # un-freeze
```

`tick set` applies `system.setTickDuration(ms)` and **confirms by reading
the value back**; it fails loudly on a mismatch. Pause/resume wrap
`system.pauseSimulation()`/`resumeSimulation()` and print the current game
time so you can see where it stopped.

### Deploy

```
cargo run -- deploy                # release build + upload as the acting identity
cargo run -- deploy --debug        # debug build (cargo without --release; dev wasm-opt profile)
cargo run -- deploy --user ibex-2  # deploy as another bot identity
```

`--user <entry>` selects which `.screeps.yaml` `servers:` entry the
upload authenticates as (default: the acting identity below) — that is
the whole multi-bot deploy story: each bot is just another entry.
`deploy` is a **library call into [`screeps-pack`](../screeps-pack)**
(P0.A13; parity evidence: `../screeps-pack/PARITY.md`): cargo build for
`wasm32-unknown-unknown` honoring the entry's
`configs.wasm-pack-options` flags → wasm-bindgen (version resolved from
the bot's `Cargo.lock`, prebuilt CLI downloaded + cached under the
target dir) → CJS loader/glue generation → wasm-opt → upload through
the shared `screeps-rest-api` client. **No npm, no node** — the
toolchain is cargo + two cached prebuilt binaries. The bot project is
found from the credentials file's directory (the workspace root next to
`.screeps.yaml`); screeps-pack resolves the single cdylib member.

Expect a cold build to take minutes (nightly + build-std + LTO); warm
rebuilds take seconds. The repo's `js_tools/deploy.js` npm pipeline
remains ONLY as an optional user-customization escape hatch — nothing
in this crate invokes it.

screeps-pack reads `.screeps.yaml` itself; credentials never appear on
argv, env, or in logs (SecretString end to end — part of the secrets
sweep).

### Capture runs (library)

The capture mechanism — console websocket → `console.jsonl`, metric
sampling → `metrics.jsonl`, `summary.json` with hard-zero gate counters
— is a library function, `capture::run(cfg, ticks, scenario, &spec)`.
The [`CaptureSpec`] parameter is where YOUR bot's policy plugs in:

```rust,no_run
use screeps_server_kit::capture::{run, CaptureSpec, MarkerSpec};
# async fn demo(cfg: &screeps_server_kit::config::KitConfig) -> anyhow::Result<()> {
let spec = CaptureSpec {
    markers: MarkerSpec {
        // YOUR bot's log strings — the kit ships none.
        panic_markers: vec!["panicked at".into()],
        deser_markers: vec!["Failed deserialization:".into()],
        error_log_prefix: Some("(ERROR)".into()),
    },
    stats_segment: Some(99), // where your bot publishes its stats JSON
};
let artifacts = run(cfg, 600, "smoke", &spec).await?;
println!("{}", artifacts.summary);
assert!(artifacts.summary.gate_failures(&spec.markers).is_empty());
# Ok(()) }
```

Artifacts land in the repo-root `runs/` tree (gitignored), keyed by
scenario + code identity:

```
runs/<scenario>-<git-sha>-<stamp>/
  console.jsonl   every console line/error, as {ts_ms, tick, kind, line}
  metrics.jsonl   one sample every 2 s: {ts_ms, tick, cpu, creeps, stats}
  summary.json    scenario, git SHA, ticks observed, wall seconds,
                  console counters (incl. panic/deser-marker counts),
                  CPU summary, creep counts
```

See [`screeps-ibex-eval`](../screeps-ibex-eval) for a complete consumer: its `gates`
module pins the marker strings against its bot's sources, and its
`smoke`/`run` CLI orchestrates up → bootstrap → deploy → capture → gate
verdict.

### Other commands

```
cargo run -- config           # resolved config, secrets redacted by construction
cargo run -- open             # print/launch the web-client URL
cargo test                    # all unit tests incl. the secrets pins
```

## Configuration

Two files, FIXED paths, no environment variables, no directory walking
(both paths are anchored at this crate, so commands resolve the same
files no matter where they run from):

| File | Path | Holds | Tracked? |
|---|---|---|---|
| credentials | `../.screeps.yaml` (the parent directory — your bot repo's root; `--config` is the only override) | `servers:` entries only — one per identity (bots AND official servers); shared with your deploy pipeline | gitignored; keyless template [`.example-screeps.yaml`](../.example-screeps.yaml) |
| stack settings | `config/local.yml` | steamKey, ports, tickMs, spawn preference, `bots:` list, `image:` block | gitignored (crate-local `.gitignore`); every key documented in the committed [`config/local.example.yml`](config/local.example.yml) |

The kit reads ONLY `servers:` from `.screeps.yaml` — other sections
(e.g. build configs) are ignored.

**Acting identity:** server-level commands (`deploy`, `config`, the
consumers' capture/smoke) act as ONE entry — an explicit
`--server-name` if given, otherwise the **first `bots:` entry** (the
kit acts as a bot; with no `bots:` configured the historical
`private-server` default applies). `bootstrap` registers only the
`bots:` entries, so pointing the acting identity anywhere else is a
guaranteed 401 on a fresh world.

`config/local.yml`, fully populated:

```yaml
steamKey: your-steam-web-api-key-here   # REQUIRED before `server up`
                                        # (https://steamcommunity.com/dev/apikey)
ports:
  game: 21025   # game/API port (published host-side and bound in-container)
  cli: 21026    # server CLI port (likewise)
tickMs: 100     # written to serverConfig.tickRate; floor 50
gcl: 10         # GCL level each bot is raised to in bootstrap (raise-only;
                # 1 disables — lifts the owned-room cap so bots can expand)
spawn:          # first bot's spawn preference (optional; see Bootstrap)
  room: W5N3
bots:           # bot identities to bootstrap; each is a servers: entry
  - ibex        # in ../.screeps.yaml
  - ibex-2
image:          # launcher image policy (optional; see build-image)
  name: screepers/screeps-launcher:latest
```

Rules:

1. `config/local.yml` is optional for read-only commands (`config`,
   `open`, `cli`, `tick` against a running stack use the defaults), but
   `server up`/`bootstrap` need the `steamKey` — the error points here
   when it is missing.
2. The launcher config the container gets is the committed keyless
   template `config/server.yml` merged in-memory with these settings:
   the kit **forces** the in-container binds — game
   `0.0.0.0:<ports.game>`, CLI `0.0.0.0:<ports.cli>` (the launcher's
   default CLI bind is in-container `127.0.0.1:21026`, unreachable from
   the host) — and sets `serverConfig.tickRate: <tickMs>`.
3. A typo'd key in `local.yml` is a hard parse error (deny-unknown-
   fields), never a silently-ignored setting.

### Operator identity vs bot identities

**You are yourself; the bots are their own users.** The identity model:

- **The operator** logs in as a *person*: Steam client or the web
  client against `http://127.0.0.1:21025/` — Steam auth works because
  the server runs with your `steamKey`. Your Steam persona becomes
  your in-game user the first time you sign in. If you also want
  password/API access to that user, set one through the masked CLI
  passthrough: `cargo run -- cli` then
  `setPassword("YourName", "your-password")` (the password is masked in
  every echo/log; see Secrets rules).
- **Each bot** is a `servers:` entry in `.screeps.yaml` with its own
  username/password (registered by `bootstrap`, targeted by
  `deploy --user <entry>`). **Name bot entries after the bot —
  `ibex`, `ibex-2`, … — not after yourself**, so your own name stays
  the operator identity. This supports bot-vs-bot matches and playing
  alongside your bot on one world.

```yaml
# ../.screeps.yaml — one entry per bot identity
servers:
  ibex:
    host: 127.0.0.1
    port: 21025
    secure: false
    username: ibex
    password: <bot password>
    branch: default
  ibex-2:
    host: 127.0.0.1
    port: 21025
    secure: false
    username: ibex-2
    password: <bot password>
    branch: default
```

With `bots: [ibex, ibex-2]` in `config/local.yml`: `bootstrap --reset`
registers both users and places two spawns in distinct rooms;
`deploy --user ibex` / `deploy --user ibex-2` push code independently.

### Building the launcher image (optional)

```
cargo run -- server build-image
```

By default the launcher image is **pulled** (`screepers/screeps-launcher:latest`).
To build it locally instead (hermetic option), point
`image.build.context` at a **full clone of
<https://github.com/screepers/screeps-launcher>** — the Dockerfile lives
at that repo's root. `server build-image` builds and tags it
(`image.name`); `server up` also builds automatically when `build:` is
configured and the image is absent. The context is tarred and sent to
the Docker daemon (`.git` is skipped); build output streams live.

The build runs through **BuildKit** (with the daemon's own os/arch as
the platform) — the upstream Dockerfile is BuildKit-only
(`FROM --platform=$BUILDPLATFORM`, `RUN --mount=type=cache`, heredoc
`RUN` blocks; the classic builder fails on it with
`failed to parse platform : ""`). On Windows, clone the launcher repo
with `git clone -c core.autocrlf=false ...` — a CRLF checkout breaks
the Dockerfile's bash heredocs (`exit code: 2` in the `useradd` step).

> A launcher *deployment* directory (just `config.yml` +
> `docker-compose.yml`) is **not** a buildable context — the validation
> error tells you exactly that.

## Secrets rules (enforced, not aspirational)

- Credentials (every `servers:` `password`, the `local.yml` `steamKey`)
  are `secrecy::SecretString` from the moment of parsing —
  `Debug`/`Display` redact by construction, enforced by a pin test
  (`config::tests::debug_output_redacts_secrets`, which covers the bot
  endpoints too).
- The **merged runtime config necessarily contains the steamKey**. It is
  written ONLY to this crate's `target/runtime/config.yml` (gitignored
  via the repo-global `target` rule) and bind-mounted into the container.
  Never copy it anywhere else.
- `config/local.yml` carries the steamKey and is gitignored by this
  crate's `.gitignore`; the committed `config/local.example.yml` must
  stay keyless.
- The vendored template `config/server.yml` is committed and **must stay
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
| `no Steam key available: ...` | Set `steamKey` in `config/local.yml` (copy `config/local.example.yml`; key from <https://steamcommunity.com/dev/apikey>). |
| `reading credentials file ...\..\.screeps.yaml` | The parent directory's `.screeps.yaml` is missing — create it (see Configuration). The path is fixed (`--config` is the only override); there is no directory walking. |
| `config/local.yml has an unexpected shape` | A typo'd key (the parser rejects unknown fields by design). Compare against `config/local.example.yml` — keys are camelCase (`steamKey`, `tickMs`). |
| `resolving bots entry '<name>'` | Every `bots:` name in `config/local.yml` must be a `servers:` entry in `../.screeps.yaml` with `username`+`password` (token-auth entries like `mmo` cannot be bots). |
| `image.build.context ... has no Dockerfile` from `build-image` | The context is a launcher *deployment* dir (config + compose), not the launcher *source* repo. Clone <https://github.com/screepers/screeps-launcher> and point `image.build.context` at the clone root. |
| `image build failed: ... exit code: 2 ([stage-1 2/4] RUN <<-EOT bash)` | CRLF checkout: the Dockerfile's bash heredocs break under Windows line-ending conversion. Re-clone with `git clone -c core.autocrlf=false`. |
| Port already in use (create/start error mentioning `0.0.0.0:21025`) | Another server holds the port. Stop it, or set `ports.game/cli` in `config/local.yml` to free ports, `server destroy --yes`, `server up`. |
| Changed `ports` but `status` shows the old ones | Published ports are fixed at container creation; `up` warns about this. `server destroy --yes` then `server up`. |
| `server CLI tcp://127.0.0.1:21026 -> refused/timeout` in `status` | The CLI bind is forced to `0.0.0.0` in the merged config, but if you're running a foreign/manually-started stack (which binds in-container `127.0.0.1` by default and publishes no CLI port), use the fallback: `docker exec -it screeps-eval-launcher screeps-launcher cli` (substitute the container name from `status`). |
| World/server behaving oddly after config experiments; `mongosh`/auth errors in logs | Stale volumes from an earlier configuration. Factory-reset: `server destroy --yes` then `server up` (full first boot again). |
| `launcher container exited while waiting for the API` | The error includes the last launcher logs. Typical causes: no/invalid steam key in `config/local.yml`, mongo/redis not healthy (re-run `server up`), or a broken locally-built image (re-run `server build-image`). |
| `signin as '<user>' rejected (401)` | The server-side password differs from `.screeps.yaml` (e.g. the entry changed since the user was created). Run `bootstrap` — it converges the password via `setPassword` before signing in. |
| `registering user ... failed: Registration is automatically disabled` | The server has the `SERVER_PASSWORD` env var set (screepsmod-auth closes registration). Not set by this kit. |
| `spawn.room ... is not a valid first-spawn room` | The room lacks an unowned controller or 2 sources, or was claimed by an earlier bot (the error lists valid candidates), or the world was seeded with a different map. Pick a listed room or drop `spawn.room` for auto-pick. |
| Simulation racing (hundreds of ticks/s) after a manual CLI `system.resetAllData()` | A reset flushes redis, including the tick duration — the loop runs unthrottled. `tick set 100` (or re-run `bootstrap`, which always re-applies `tickMs`). |
| `cargo build failed` from `deploy` | The real compiler error is in the streamed output just above. Common toolchain gaps: nightly not installed (`rust-toolchain.toml` pins it), missing `rust-src` component (build-std needs it), missing `wasm32-unknown-unknown` target. |
| `wasm-bindgen ... emitted JS whose wasm-load tail does not match` from `deploy` | The bot's `Cargo.lock` moved to a wasm-bindgen version whose output screeps-pack's anchored glue patcher has not been verified against. See `../screeps-pack/src/glue.rs` (`VERIFIED_BINDGEN_OUTPUT`). |
| `uploading ... modules to ...` failed | The upload threw (server down/unreachable, or signin rejected). `server status`, then retry; `bootstrap` converges a stale password. |
| `websocket auth failed (token rejected)` during a capture run | The signin token was rejected — usually a stale server-side password. Run `bootstrap` to converge credentials, then retry. |
| `run did not reach tick ... within the ... safety budget` | The simulation is paused (`tick resume`) or crawling far below the configured rate. Check `server status` and the tick rate. |
| `console.jsonl` is empty/small for short runs | Normal if the bot logs sparsely and empty per-tick console events are not written. CPU/creeps still prove liveness in `metrics.jsonl`. |

---

## Design

### Module map

```
src/lib.rs            library root (the CLI is a thin wrapper — automation
                      and operator share every code path)
src/config.rs         FIXED-path loading: creds from ../.screeps.yaml
                      (servers: only), stack settings (steamKey/ports/
                      tickMs/spawn/bots/image) from config/local.yml;
                      per-bot endpoint resolution; secrets policy
                      (SecretString from parse time)
src/server_config.rs  launcher-config preparation: template -> PURE merge ->
                      target/runtime/config.yml (the only sanctioned on-disk
                      location for the steamKey)
src/docker.rs         bollard lifecycle: images/network/volumes/containers,
                      health-waits, status introspection, logs, down/destroy;
                      launcher image pull-or-build + the build-context tar
                      (tar construction unit-tested offline)
src/server.rs         server-CLI client (CliClient), multi-bot bootstrap flow
                      (distinct rooms, per-bot spawn names), tick control,
                      the setPassword mask, spawn-tile picking (pure,
                      unit-tested)
src/api.rs            thin adapter over the SHARED screeps-rest-api client:
                      client construction from ServerEndpoint + the
                      401-signin diagnostic; the endpoints themselves are
                      pinned + fixture-tested in ../screeps-rest-api
src/deploy.rs         deploy orchestration: a library call into
                      ../screeps-pack (cargo build -> wasm-bindgen ->
                      glue -> wasm-opt -> upload; P0.A13 — the deploy.js
                      shell-out was deleted at the parity cutover)
src/capture.rs        run loop + metrics sampler -> runs/ artifacts; summary
                      aggregation + marker counters, parameterized by the
                      caller's MarkerSpec/CaptureSpec (no bot strings here;
                      the console websocket protocol lives in
                      ../screeps-rest-api)
src/main.rs           clap CLI: server/bootstrap/deploy/cli/tick/open/config
                      (incl. the interactive REPL)
config/server.yml         vendored KEYLESS launcher-config template (committed)
config/local.example.yml  documented template for config/local.yml (committed)
config/local.yml          steamKey + machine-local settings (GITIGNORED)
```

### The mechanism/policy split (P0.A14)

Everything here answers "HOW do I run, bootstrap, deploy, observe a bot
on a local private server". The consumer crate answers "WHAT does a
healthy run of MY bot look like": it supplies `capture::MarkerSpec`
(panic/deser/error-line strings pinned against its bot's sources) and
`CaptureSpec::stats_segment`, and composes the library calls into its
own smoke/scenario commands. The gate EVALUATION (`Summary::
gate_failures` — hard zeros over the marker counts) is mechanism; the
marker strings are policy. Bootstrap is mechanism end-to-end: which
users/spawns exist comes from config (`bots:`), not from code.

### Config files & merge flow

```
../.screeps.yaml (gitignored)        config/local.yml (gitignored)
  servers: <entry per identity>        steamKey/ports/tickMs/spawn/bots/image
       │                                    │
       │ --server-name + bots: select       │
       ▼                                    │
  ServerEndpoint + Vec<BotEndpoint>         │
  (used by api/bootstrap/deploy)            │
                                            │
        config/server.yml (committed, keyless template)
                           │                │
                           ▼                ▼
                          merge (in-memory, pure fn):
                           - steamKey inserted (template is keyless)
                           - force env.backend GAME/CLI binds 0.0.0.0:ports
                           - serverConfig.tickRate = tickMs
                           │
                           ▼
        <this crate>/target/runtime/config.yml   (gitignored)
                           │
                           ▼  bind-mount
        launcher container /screeps/config.yml
```

Both fixed paths are anchored at the crate root via `CARGO_MANIFEST_DIR`
(compile time), not the invocation cwd: `runtime_dir()` already anchors
there, the crate is driven via `cargo run` while it lives in-repo
(decision D-1), and config resolution must not silently change when a
command runs from an unexpected directory. Revisit both anchors at
extraction.

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
  therefore re-applies `tickMs` unconditionally and read-back-checks it.
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
  (the shared `screeps-rest-api` client does).
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

### Multi-bot bootstrap

The world-level steps (reset, tick rate) run once; the identity steps
(register/converge → signin → place → verify) run **per `bots:` entry**,
each against that bot's own endpoint/credentials. Distinct rooms are
guaranteed two ways: the candidate query already excludes rooms with an
owned controller (an earlier bot's claim), and the in-run claimed set is
filtered out of the candidates regardless (`filter_excluded_rooms`,
pure + pinned — belt and braces against server-state lag). The
`spawn:` preference applies only to the first bot; spawn names derive
from the bot entry name (`spawn_name_for`, pure + pinned) so the web
client shows whose spawn is whose. The operator's own identity never
appears in `bots:` — see the Usage section's identity model.

### Capture flow

```
capture::run(cfg, N, scenario, spec)
  ├─ signin (HTTP sampler token)  +  /api/auth/me (user id)
  ├─ second signin -> ws token (SecretString; exposed only into `auth `)
  ├─ tick_first = /api/game/time;  create runs/<S>-<git-sha>-<stamp>/
  ├─ console task: ws connect -> auth -> subscribe user:<id>/console
  │     each event -> {ts_ms, tick≈, kind, line} -> console.jsonl
  │     counters classified against spec.markers: log/result/error
  │     lines, error-prefix lines, panic-marker lines, deser-marker
  │     lines (the consumer's gates read these)
  └─ sampler loop (every 2 s) until tick >= tick_first + N:
        /api/game/time          -> tick (also stamps console lines)
        /api/user/memory-segment?segment=<spec.stats_segment> -> stats
        server CLI creep count  -> creeps (best-effort)
        -> metrics.jsonl; then summary.json + gate counters
```

Safety: the run aborts (with artifacts already on disk) if the console
socket dies mid-run, if `/api/game/time` stops answering (10 consecutive
failures), or if a wall-clock budget of 10× nominal tick time + 2 min is
exceeded (the server can legitimately run below the configured rate).

Tokens are rolling/consumable on private servers, so the capture mints a
**separate** token for the socket (`Client::fresh_token`) — the HTTP
sampler's token stays valid; the fresh token in `auth ok` is dropped at
parse time inside the shared client, so it cannot reach logs or
artifacts (pinned by `screeps-rest-api`'s
`auth_ok_token_is_dropped_at_parse_time`).

### Deploy facts (the screeps-pack library seam, P0.A13)

- `deploy(cfg, server_entry, debug)` builds a `screeps_pack::PackOptions`
  (pinned by `deploy::tests::pack_options_map_the_kit_config`):
  credentials = the file the kit's config was loaded from; bot manifest
  = `Cargo.toml` next to it (screeps-pack resolves the workspace's
  single cdylib member); `--user <entry>` passes through as the server
  entry, selecting both upload credentials and that entry's
  `configs.wasm-pack-options` build flags.
- Errors are real `Result`s end to end — there is no exit-code/output
  parsing anymore (the old deploy.js wrapper needed it because the
  script exited 0 on failure; that machinery was deleted at the A13
  cutover).
- Parity vs the npm pipeline (module map, byte-identical wasm, green
  600-tick smoke) is recorded in `../screeps-pack/PARITY.md`.

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
  kit sets them as container env.

### Stack topology & port discovery

Three containers on the `screeps-eval-net` network (aliases `mongo`,
`redis`), named volumes `screeps-eval-data` (`/screeps`),
`screeps-eval-mongo` (`/data/db`), `screeps-eval-redis` (`/data`).
Containers/network/volumes are all `screeps-eval-*`-prefixed so `destroy`
cannot touch anything else (the prefix predates the crate's rename and
is kept so existing stacks are not orphaned). In-container ports equal
host-published ports (the merge forces `GAME_PORT`/`CLI_PORT` to the
configured `ports`), so there is one port number per endpoint end-to-end.

`server status` does **not** echo the configuration back: it reads the
actually-published ports from container inspect (`NetworkSettings.Ports`)
and probes those — so it tells the truth about a manually-started or
half-broken stack. The launcher container is found by canonical name
first, then by image (the configured `image.name` or anything
`*screeps-launcher*`) as a fallback.

### Image policy: pull by default, build on request

`mongo:8` and `redis:7` are always pulled. The launcher image
(`image.name`, default `screepers/screeps-launcher:latest`) follows
present → use, else build if `image.build` is configured, else pull —
so the default stays reproducible on any machine with no Go toolchain,
and the hermetic local build is one config block away.

The build path (`docker::build_launcher_image`): validate the context
(must be a directory containing the Dockerfile — the upstream
screepers/screeps-launcher repo root; the validation error names the
non-buildable config-only-directory failure mode explicitly), tar it
(`docker::build_context_tar` — relative forward-slash names, sorted,
`.git` skipped; unit-tested offline against fixture directories), and
stream it to bollard's `build_image` with the tag + dockerfile options,
surfacing in-band build errors and verifying the tag exists afterwards.

### Why workspace-excluded

This crate lives inside a bot repo whose workspace members are wasm
crates; it is excluded from that workspace so it builds host-native
with its own dependency tree (no default build target is set
repo-wide, so plain `cargo build`/`cargo test` work from this
directory). Config files are not cwd-dependent: the fixed paths and
`target/runtime/` anchor at the crate's compile-time location
(`CARGO_MANIFEST_DIR`) while the crate lives in-repo.

### Extraction-to-submodule plan (decision D-1)

The crate starts in-repo (workspace-excluded) and extracts to a
submodule with its own remote once stable — it is a community-share
candidate alongside [`screeps-rest-api`](../screeps-rest-api) and the
upcoming `screeps-pack`. Designed in from day one: no workspace-crate
dependencies, self-contained README + example configs, own lockfile.
Documented seams: the `screeps-rest-api` path dependency (same D-1
lifecycle — it becomes a git dep at extraction), the `screeps-ibex-eval` path
consumer, the two `CARGO_MANIFEST_DIR` anchors (`runtime_dir()`, the
fixed config paths incl. `../.screeps.yaml`), which need a
deployment-time answer when the crate stops being driven via
`cargo run` in-repo, and the `screeps-eval-*` Docker object prefix
(rename candidate at extraction, behind an explicit migration). The
operator creates the remote.
