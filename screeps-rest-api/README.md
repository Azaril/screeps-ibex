# screeps-rest-api

The shared [Screeps](https://screeps.com) HTTP/websocket API client used
by this repo's host-side tooling — and usable by any Rust tool that
talks to a Screeps server (private servers and screeps.com alike).

One client, not N (Phase-0 task P0.A12): `screeps-server-kit` + `screeps-ibex-eval` (the
private-server eval harness) and `screeps-prospector` (spawn-site
selection) used to carry independent endpoint implementations; this
crate is the single place where the endpoint set is implemented, every
response shape is **pinned with a source citation**, and every shape has
a fixture test. The future rust-native deploy tool (P0.A11) lands on the
same client (`POST /api/user/code` is already pinned here).

Pure **library** — no binary, no config-file I/O: callers resolve
credentials themselves and hand in an `AuthMode`.

---

## Usage

```toml
[dependencies]
screeps-rest-api = { path = "../screeps-rest-api" }   # git dep after extraction
```

### Connect and call

```rust,no_run
use screeps_rest_api::{AuthMode, Client, DEFAULT_MIN_DELAY_MS};
use secrecy::SecretString;
use std::time::Duration;

# async fn demo() -> Result<(), screeps_rest_api::ApiError> {
// Private server: username/password, no shard, no courtesy delay.
let client = Client::new(
    "http://127.0.0.1:21025",
    None,                                  // private servers have no shard param
    AuthMode::UserPass {
        username: "ibex".to_owned(),
        password: SecretString::from("..."),
    },
    Duration::ZERO,
)?;
client.sign_in().await?;                   // exchanges the password for a session token

// screeps.com: auth token, shard required, default pacing.
let mmo = Client::new(
    "https://screeps.com",
    Some("shard3".to_owned()),
    AuthMode::Token(SecretString::from("...")),
    Duration::from_millis(DEFAULT_MIN_DELAY_MS),
)?;
mmo.sign_in().await?;                      // no-op for token auth

let time = client.game_time().await?.time;
let me = client.me().await?;               // {id, username} — id keys the console channel
let seg = client.memory_segment(99).await?.data;
# Ok(()) }
```

### Console websocket

```rust,no_run
use screeps_rest_api::{console_lines, ConsoleSocket};

# async fn demo(client: screeps_rest_api::Client) -> Result<(), screeps_rest_api::ApiError> {
let me = client.me().await?;
// Tokens are rolling/consumable on private servers — mint a SEPARATE
// one for the socket so the HTTP client's token stays valid.
let ws_token = client.fresh_token().await?;
let mut socket = ConsoleSocket::connect(&client.ws_url(), ws_token, &me.id).await?;
loop {
    let event = socket.next_event().await?;   // pings answered internally
    for line in console_lines(&event.payload) {
        println!("[{:?}] {}", line.kind, line.line);
    }
}
# }
```

### Auth model

| Mode | Wire mechanics | Where it works |
|---|---|---|
| `AuthMode::UserPass` | `POST /api/auth/signin` `{email: <username>, password}` → `{ok, token}`; the session token is then sent as `X-Token` **and** `X-Username` on every request | private servers (screepsmod-auth) |
| `AuthMode::Token` | the long-lived token is sent as `X-Token`/`X-Username` directly — no sign-in round trip | screeps.com (the only mode there) and private servers |

**Rolling-token adoption:** when a response carries an `X-Token` header
of ≥ 40 chars, it replaces the stored token (the python-screeps
acceptance rule). On private servers session tokens are *consumed* by
use and every authenticated response carries a replacement — adoption is
mandatory there (private-server tokens are exactly 40 hex chars, so the
rule admits them). `Client` handles all of this internally with `&self`
methods (interior mutability).

`Client::fresh_token()` mints a *separate* session token (second
signin) for the websocket; for `AuthMode::Token` it returns the
configured token (official tokens are not consumed).

### Rate-limit behavior

screeps.com enforces **two layers** of limits on auth tokens
([docs.screeps.com/auth-tokens.html](https://docs.screeps.com/auth-tokens.html)),
and the client models both:

- **Global cap — courtesy min-delay:** the client sleeps so that at
  least `min_delay` elapses between any two requests.
  `DEFAULT_MIN_DELAY_MS` (600 ms) is sized for the global
  120 requests/minute token limit; pass `Duration::ZERO` against a
  local private server.
- **Per-endpoint quotas — proactive pacing** (`quota` module): the
  routes below carry hourly/daily quotas *on top of* the global cap.
  Pacing at 600 ms burns a 60/hour quota in 36 s and then every further
  call 429s for the rest of the hour — so for **official targets**
  (`is_official_target`: token auth or a screeps.com URL; override with
  `Client::quota_pacing`) the client spaces calls per endpoint to fit
  the quota (60/hour ⇒ one per 60 s sustained). Private servers enforce
  no quotas (no rate-limit middleware in `@screeps/backend`; verified
  live 2026-06-10) and keep the plain min-delay.
- **Headers are ground truth, the table is the prior:** screeps.com
  answers with `X-RateLimit-Limit` / `-Remaining` / `-Reset` (epoch
  seconds — node-screeps-api `buildRateLimit`,
  dist/ScreepsAPI.js:1238-1254). When present they override the static
  table: pacing becomes `time_to_reset / remaining` (faster while the
  window is unspent, a hard wait until reset when `remaining == 0`).
- **HTTP 429 — back off and resume:** `parse_retry_after_ms` extracts
  the wait from the official message (observed live: `Rate limit
  exceeded, retry after 3548835ms or disable rate limiting using this
  link: …`), the client logs `rate limited; resuming in Xs (endpoint
  quota: …)`, sleeps, and **retries the request** (bounded: 3 attempts
  per request, a single wait is capped at 2 h — beyond that the
  `ApiError::RateLimited` is surfaced). A wait ≥ 2 min also prints
  **once** the sanctioned opt-out: signed-in token owners can disable
  rate limiting per token at
  `https://screeps.com/a/#!/account/auth-tokens/noratelimit`
  (node-screeps-api's `rateLimitResetUrl`). Printed, never opened.

#### The pinned per-endpoint quota table

Pinned from screepers/node-screeps-api — `src/ScreepsAPI.ts` (master,
fetched 2026-06-10), identical to the vendored
`node_modules/screeps-api@1.16.1` `dist/ScreepsAPI.js:1417-1438`
(READ-ONLY). Global: **120/minute** (:1418).

| Method | Endpoint | Quota | Sustained spacing | Source line |
|---|---|---|---|---|
| GET | `/api/game/room-terrain` | 360/hour | 10 s | :1420 |
| GET | `/api/user/code` | 60/hour | 60 s | :1421 |
| GET | `/api/user/memory` | 1440/day | 60 s | :1422 |
| GET | `/api/user/memory-segment` | 360/hour | 10 s | :1423 |
| GET | `/api/game/market/orders-index` | 60/hour | 60 s | :1424 |
| GET | `/api/game/market/orders` | 60/hour | 60 s | :1425 |
| GET | `/api/game/market/my-orders` | 60/hour | 60 s | :1426 |
| GET | `/api/game/market/stats` | 60/hour | 60 s | :1427 |
| GET | `/api/game/user/money-history` | 60/hour | 60 s | :1428 |
| POST | `/api/user/console` | 360/hour | 10 s | :1431 |
| POST | `/api/game/map-stats` | 60/hour | 60 s | :1432 |
| POST | `/api/user/code` | 240/day | 360 s | :1433 |
| POST | `/api/user/set-active-branch` | 240/day | 360 s | :1434 |
| POST | `/api/user/memory` | 240/day | 360 s | :1435 |
| POST | `/api/user/memory-segment` | 60/hour | 60 s | :1436 |

The first call to an endpoint is never delayed — sustained spacing
applies from the second call on, so single-shot operations (one code
upload, one segment read) pay nothing.

### Error envelope

Every method returns `Result<TypedResponse, ApiError>`:

- `{"error": "..."}` bodies (which arrive **with HTTP 200**) →
  `ApiError::Server { message }` — checked before the typed parse;
- HTTP 429 → `ApiError::RateLimited` (surfaced only after the bounded
  backoff-and-resume retries above are exhausted);
- other non-2xx → `ApiError::Http { status, body }` (body truncated);
- JSON that fits none of the pinned shapes → `ApiError::Decode`;
- transport/websocket failures → `Transport` / `Socket` /
  `SocketClosed` / `SocketAuthFailed` / `SocketAuthTimeout`.

### Secrets

Passwords and tokens are `secrecy::SecretString` end-to-end —
`Debug`/`Display` redact by construction (pinned by tests:
`auth_mode_debug_redacts_secrets`, `signin_response_debug_redacts_token`,
and the websocket pin `auth_ok_token_is_dropped_at_parse_time`).
Secrets are exposed only into request bodies, auth headers, and the
websocket `auth` frame — never into logs or error text.

---

## Design

### Module map

| Module | Responsibility |
|---|---|
| `client` | `Client` + `AuthMode` + `is_official_target`: signin/rolling-token adoption, shard injection (query param on GETs, body key on POSTs), courtesy throttle, official quota pacing + 429 backoff-and-resume, one typed method per endpoint |
| `types`  | typed response structs, each fixture-tested; `enumerate_room_names` (the server's room-coordinate scheme) |
| `code`   | the `POST /api/user/code` module map: JS-source modules as plain strings, wasm modules as `{binary: <base64>}` |
| `quota`  | the pinned per-endpoint quota table, `X-RateLimit-*` header parsing, 429 retry-after parsing, `QuotaTracker` pacing math (pure, unit-tested) |
| `socket` | console websocket: `parse_socket_frame`, the `ConsoleSocket` handshake (auth → subscribe), `console_lines` payload flattening |
| `error`  | `ApiError` + the envelope-first response classification |

### Endpoint table (the unified set, with shape sources)

Source key: **[py-tool]** = [Qionglu735/screeps_tool `screeps_api.py`](https://github.com/Qionglu735/screeps_tool/blob/master/screeps_api.py)
(the operator-referenced reference client) · **[py-screeps]** =
screepers/python-screeps `screepsapi.py` · **[endpoints-md]** =
screepers/node-screeps-api `docs/Endpoints.md` (vendored at
`node_modules/screeps-api/docs/Endpoints.md`) · **[node-api]** =
`node_modules/screeps-api/dist/ScreepsAPI.js` (READ-ONLY; the exact
client `js_tools/deploy.js` uses) · **[live]** = live private server
2026-06-09 (screeps-eval bring-up; container sources read on the box:
screepsmod-auth `lib/backend.js`/`lib/register.js`, `@screeps/backend
lib/game/api/{user,game}.js`) · **[docs]** =
docs.screeps.com/auth-tokens.html.

| Method | Endpoint | Auth | Shard | Sources |
|---|---|---|---|---|
| `sign_in` / `fresh_token` | `POST /api/auth/signin` `{email, password}` → `{ok, token}` | — | no | [py-tool] [endpoints-md] [live] |
| `me` | `GET /api/auth/me` → `{ok, _id, username, ...}` | yes | no | [endpoints-md] [live] |
| `username_available` | `GET /api/register/check-username?username=` → `{ok:1}` / error envelope | no | no | [live] (screepsmod-auth `lib/register.js`) |
| `register` | `POST /api/register/submit` `{username, password}` → `{ok:1}` | no | no | [live] (screepsmod-auth `lib/register.js`) |
| `game_time` | `GET /api/game/time` → `{ok, time}` | no | no | [endpoints-md] [live] |
| `world_size` | `GET /api/game/world-size` → `{ok, width, height}` | no | query | [py-tool] [py-screeps] |
| `world_status` | `GET /api/user/world-status` → `{ok, status: empty\|normal\|lost}` | yes | no | [py-tool] [endpoints-md] [live] |
| `map_stats` | `POST /api/game/map-stats` `{rooms, statName}` → `{ok, stats, users}` | yes | body | [py-tool] [py-screeps] [endpoints-md] |
| `room_terrain_encoded` | `GET /api/game/room-terrain?room&encoded=1` → `{ok, terrain: [{room, terrain: <2500 digits>}]}` | no | query | [py-tool] [py-screeps] [endpoints-md] [docs] |
| `room_objects` | `GET /api/game/room-objects?room` → `{ok, objects, users}` | yes | query | [py-screeps] |
| `room_status` | `GET /api/game/room-status?room` → `{ok, room: {_id, status, novice?, openTime?}}` | yes | query | [py-tool] [py-screeps] [endpoints-md] |
| `place_spawn` | `POST /api/game/place-spawn` `{room, x, y, name}` → `{ok:1}` (+`newbie` on private) | yes | body | [py-tool] [py-screeps] [live] |
| `respawn` | `POST /api/user/respawn` `{}` → `{ok:1}` | yes | no | [py-tool] [py-screeps] |
| `memory_segment` | `GET /api/user/memory-segment?segment=N` → `{ok, data: string\|null}` | yes | query* | [node-api]:984 [live] |
| `set_memory_segment` | `POST /api/user/memory-segment` `{segment, data}` → `{ok:1}` | yes | body | [node-api]:994 |
| `upload_code` | `POST /api/user/code` `{branch, modules, _hash}` → `{ok:1}` | yes | no | [node-api]:894-897 + `js_tools/deploy.js:121-158` |
| `ws_url` / `ConsoleSocket` | `ws(s)://host/socket/websocket` (see below) | token frame | — | [live] (`@screeps/backend lib/game/socket/*`, `@screeps/driver lib/index.js:368/:409`) |

\* private servers have **no** shard parameter at all (`@screeps/backend
lib/game/api/user.js:318-327` reads only `request.query.segment`) —
construct the client with `shard: None` there and nothing is sent.

`place_spawn` is deliberately un-gated here: **callers own the safety
gates** (prospector requires explicit `--yes` and refuses auto-placement
against official servers; keep that discipline in new consumers).

### Code-upload module map (`code` module)

`modules` maps module name → content, where content is either a plain
JS source string or `{binary: <base64>}` for wasm — exactly what
deploy.js's `load_built_code` produces (deploy.js:121-146) and
node-screeps-api's `code.set` posts with `_hash = Date.now()`
(ScreepsAPI.js:894-897). `CodeModule` is an untagged serde enum pinning
that wire shape; `CodeModule::from_binary` base64-encodes raw bytes
(RFC 4648 with padding, matching Node's `Buffer.toString('base64')`;
pinned against the RFC vectors and the wasm magic header). This is for
the A11 rust-native deploy tool — fixture-tested only until that lands;
`js_tools/deploy.js` remains the working deploy path.

### Console-websocket protocol (pinned live 2026-06-09)

- Endpoint: `ws://host:port/socket/websocket` — the sockjs **raw
  websocket** transport (`@screeps/backend lib/game/socket/server.js`
  installs sockjs at prefix `/socket`); plain text frames, no sockjs
  `a[...]` framing.
- Server greets with `time <unix-ms>` then `protocol 14`.
- Client sends `auth <token>`; reply `auth ok <fresh-token>` or
  `auth failed`. The fresh token is **dropped at parse time** inside
  `parse_socket_frame` — it cannot reach logs or artifacts (P0.A7 pin).
- Client sends `subscribe user:<userId>/console` (`userId` = `_id` from
  `/api/auth/me`); the server rejects `user:` channels that do not match
  the authed user.
- Events are `JSON.stringify([channel, payload])` text frames:
  `{"messages": {"log": [...], "results": [...]}}` once per tick (also
  when empty — a liveness signal) and `{"error": "..."}` for runtime
  errors (`@screeps/driver lib/index.js:368/:409`). `console_lines`
  flattens a payload into `(kind, line)` pairs; `ConsoleLineKind`
  serializes lowercase (`log`/`result`/`error`) for consumers' JSONL
  records.
- `gz:` deflate frames only exist after a client sends `gzip on`; this
  client never does.

`ConsoleSocket::next_event` answers pings internally, skips
greeting/heartbeat frames, and is safe to race in `tokio::select!`
(the frame read is cancel-safe).

### Shape reconciliations (where the two consumers disagreed)

Made when unifying (P0.A12 — "keep the better-evidenced shape"):

1. **Response typing.** screeps-eval used one lazily-typed
   `ApiOkResponse` for everything; screeps-prospector had per-endpoint
   typed structs with envelope-first error classification. The typed
   approach won (better-evidenced per shape, clearer errors); eval's
   live-pinned fixtures moved into the typed tests.
2. **X-Token rotation rule.** Eval adopted any non-empty `X-Token`
   response header; prospector required ≥ 40 chars (the python-screeps
   rule). The cited ≥ 40 rule won — private-server session tokens are
   exactly 40 hex chars (observed live), so nothing is lost and
   short junk header values are never adopted.
3. **Request timeout.** Eval pinned 30 s; prospector had none
   (reqwest's default is no timeout). The explicit 30 s won
   (`DEFAULT_REQUEST_TIMEOUT`) — an unanswered request should fail, not
   hang a run.
4. **Rate-limit courtesy.** Prospector's configurable min-delay won
   (eval had none); eval passes `Duration::ZERO` for its local server.
5. **401 on signin.** Eval special-cased HTTP 401 with an
   operator-actionable message. That stays with the consumer (now
   `screeps-server-kit`'s `api::signin`) — the shared client reports
   the plain `ApiError::Http { status: 401, .. }` and consumers attach
   their own remediation text.

### Extraction / community-share plan

Same lifecycle as the consumers (Phase-0 decision D-1): in-repo and
workspace-excluded now; extracted to its own repository with its own
remote once stable. The crate is already self-contained — no workspace
deps, no repo paths, no config-file knowledge — so extraction is just
moving the directory and turning the consumers' path deps into git
deps. As a community share it pairs with the A11 deploy-tool
investigation: anyone linking `screeps-game-api` into their own bot
crate gets a host-side client for deploy/eval tooling. Open item for
that audience: P0.P6 (adopting `screeps-game-api` pure types like
`RoomName` in this crate's surface) — currently plain `&str`/`String`
room names, pending the P6 verdict.

### Verification status

39 unit tests against recorded/literal fixtures (no network, no
Docker): every response shape in the endpoint table, the error
envelope/429/decode matrix, the quota table + `QuotaTracker` spacing
math + `X-RateLimit-*` header sets + the live 429 retry-after string,
the official-target classification, the code-upload wire shape + base64
vectors, socket frames + payload flattening, and the secrecy redaction
pins. The HTTP+websocket paths were live-verified in their previous
in-consumer form during the eval bring-up (2026-06-09); the first
post-extraction live gauntlet is the eval smoke loop (runs separately —
the eval stack is shared infrastructure). The quota pacing/backoff path
against real screeps.com headers is pinned from fixtures only — to be
confirmed on the operator's next MMO run (the local private server
sends no rate-limit headers).
