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
| World bootstrap (server CLI) | P0.A3 | ⬜ |
| Deploy (deploy.js wrapper) | P0.A4 | ⬜ |
| Console/metrics capture → `runs/` | P0.A5 | ⬜ |
| Smoke loop + baselines | P0.A6 | ⬜ |
| Secrets sweep & pins | P0.A7 | ✅ (pin) / ⬜ (sweep) |
| Operator mode (cli/tick/open/status) | P0.A8 | ⬜ (status/logs ✅ via A2) |

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

### Other commands

```
cargo run -- config           # resolved config, secrets redacted by construction
cargo run -- open             # print/launch the web-client URL
cargo run -- bootstrap --reset   # (P0.A3) world reset + password + tick rate
cargo run -- deploy              # (P0.A4) build + upload the bot
cargo run -- run --ticks 200     # (P0.A5) capture console+metrics to runs/
cargo run -- smoke               # (P0.A6) up -> bootstrap -> deploy -> run
cargo run -- tick set 100        # (P0.A3/A8) floor: 50 ms
cargo test                       # all unit tests incl. the secrets-redaction pin
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
- Never log raw server-CLI payloads (`setPassword(...)` — P0.A3 concern).

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
src/main.rs           clap CLI
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
