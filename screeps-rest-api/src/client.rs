//! The async REST client: auth, shard injection, courtesy rate limit,
//! official-server endpoint-quota pacing, rolling-token adoption, and a
//! typed method per pinned endpoint.
//!
//! Construct via [`Client::new`], call [`sign_in`](Client::sign_in)
//! once (a no-op for token auth), then use the endpoint methods. Every
//! method's doc comment cites the source(s) its shape was pinned from —
//! see also the shape catalogue in [`crate::types`].
//!
//! RATE LIMITS (official servers): on top of the courtesy `min_delay`
//! (the global 120/minute cap), per-endpoint-capped routes are paced on
//! EVIDENCE — calls run free until the server demonstrates limiting via
//! `X-RateLimit-*` headers or a 429, after which the endpoint's tracker
//! holds calls to fit the rest of the window (see [`crate::quota`]; a
//! noratelimit token shows neither signal and is never slowed). A 429
//! backs off for the server's stated retry-after and RESUMES instead of
//! failing the caller's whole run. Private servers (no quotas, no
//! headers) keep the plain `min_delay`.
//!
//! SECRETS: passwords/tokens live in [`SecretString`]; they are exposed
//! only into the signin/register request bodies and the `X-Token`/
//! `X-Username` auth headers — never into logs or error text. Fresh
//! tokens minted for the websocket are returned as [`SecretString`] so
//! they redact by construction (P0.A7).

use crate::code::CodeModules;
use crate::error::{parse_api_response, ApiError};
use crate::quota::{
    endpoint_quota, parse_rate_limit_headers, parse_retry_after_ms, QuotaTracker, RateLimitInfo,
    NO_RATE_LIMIT_URL,
};
use crate::socket::ws_url_from_http_base;
use crate::types::{
    MapStatsResponse, MemorySegmentResponse, OkResponse, RoomObjectsResponse, RoomStatusResponse,
    RoomTerrainResponse, ShardsInfoResponse, SignInResponse, TimeResponse, UserInfo,
    WorldSizeResponse, WorldStatusResponse,
};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// Default courtesy delay between requests, sized for screeps.com: the
/// global auth-token limit is 120 requests/minute (= one per 500 ms),
/// with stricter per-endpoint caps (GET room-terrain: 360/hour).
/// 600 ms keeps a sustained scan safely under the global cap; private
/// servers can pass something much lower (down to `Duration::ZERO`).
/// Source: <https://docs.screeps.com/auth-tokens.html>
pub const DEFAULT_MIN_DELAY_MS: u64 = 600;

/// Per-request timeout. Generous: `system.resetAllData()`-adjacent
/// reads on a busy private server can be slow, and screeps.com can lag.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How many times one logical request rides out a 429 before the error
/// is surfaced. Each retry waits the server's stated retry-after, so
/// this bounds attempts, not time (see [`MAX_RATE_LIMIT_WAIT`]).
const RATE_LIMIT_RETRIES: u32 = 3;

/// Upper bound on a single backoff/quota wait. Hourly windows fit
/// comfortably (the longest observed live wait was ~59 min); a wait
/// beyond this (a burned DAILY quota, e.g. POST /api/user/code at
/// 240/day) is surfaced as the error instead of silently parking the
/// process for hours.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(2 * 3600);

/// 429 backoff when the server's retry-after could not be parsed and no
/// `X-RateLimit-Reset` ground truth is available.
const DEFAULT_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);

/// Waits at or above this print (once per client) the sanctioned
/// noratelimit opt-out hint.
const NORATELIMIT_HINT_THRESHOLD: Duration = Duration::from_secs(120);

/// Tokens shorter than this are not treated as rotation material — the
/// python-screeps client's acceptance rule for the `X-Token` response
/// header (`if len(r.headers['X-Token']) >= 40`). Private-server
/// session tokens are exactly 40 hex chars (observed live during the
/// screeps-eval bring-up, 2026-06-09), so the rule adopts them.
const TOKEN_ROTATION_MIN_LEN: usize = 40;

/// How the client authenticates against the selected server.
///
/// Official servers (mmo/ptr/season) support token auth only; private
/// servers use username/password via `POST /api/auth/signin` and roll
/// the session token on every authenticated response.
#[derive(Debug)]
pub enum AuthMode {
    /// A long-lived auth token — sent as `X-Token`/`X-Username` headers
    /// directly (<https://docs.screeps.com/auth-tokens.html>).
    Token(SecretString),
    /// Username + password — exchanged for a session token at sign-in
    /// (screepsmod-auth `lib/backend.js`; the username goes in the
    /// signin body's `email` field).
    UserPass {
        username: String,
        password: SecretString,
    },
}

/// OFFICIAL-server classification (shared with consumers — prospector's
/// MMO-safety gates use the same rule, P0.P4): a target is official
/// when it authenticates by token (official servers are token-only,
/// and a token entry pointed anywhere deserves the same caution) OR its
/// URL targets screeps.com. Official targets get per-endpoint quota
/// pacing ([`crate::quota`]); private servers keep the plain min-delay.
pub fn is_official_target(base_url: &str, auth: &AuthMode) -> bool {
    matches!(auth, AuthMode::Token(_)) || base_url.contains("screeps.com")
}

/// Async REST client over `reqwest` with a courtesy rate limit,
/// official-server endpoint-quota pacing, and rotating-token auth.
/// Interior mutability throughout — every endpoint method takes
/// `&self`.
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    shard: Option<String>,
    auth: AuthMode,
    /// Current session token; replaced when a response rotates it.
    token: Mutex<Option<SecretString>>,
    min_delay: Duration,
    last_request: Mutex<Option<Instant>>,
    /// Endpoint-quota tracking engaged? Auto-detected via
    /// [`is_official_target`]. Tracking, not braking: calls run free
    /// until the server evidences limiting (headers/429).
    official: bool,
    /// Per-endpoint pacing state, keyed by (method, path) for the
    /// routes in [`crate::quota::endpoint_quota`]'s table.
    quotas: Mutex<HashMap<(&'static str, &'static str), QuotaTracker>>,
    /// The noratelimit hint is printed at most once per client.
    noratelimit_hint_shown: AtomicBool,
}

impl Client {
    /// `base_url` is `scheme://host[:port][/path]` (no trailing slash).
    /// `shard` is required for official servers (sent as a query param
    /// on GETs and a body key on POSTs, per python-screeps) and omitted
    /// for private servers — which have no shard parameter at all
    /// (`@screeps/backend lib/game/api/user.js:318-327` reads only the
    /// documented params). `min_delay` is the courtesy gap between any
    /// two requests ([`DEFAULT_MIN_DELAY_MS`]; pass `Duration::ZERO`
    /// against a local private server).
    pub fn new(
        base_url: impl Into<String>,
        shard: Option<String>,
        auth: AuthMode,
        min_delay: Duration,
    ) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("screeps-rest-api/", env!("CARGO_PKG_VERSION")))
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .build()?;
        // Token auth needs no sign-in round trip: seed the session
        // token directly from the config.
        let initial_token = match &auth {
            AuthMode::Token(token) => Some(SecretString::from(token.expose_secret())),
            AuthMode::UserPass { .. } => None,
        };
        let base_url = base_url.into();
        let official = is_official_target(&base_url, &auth);
        Ok(Client {
            http,
            base_url,
            shard,
            auth,
            token: Mutex::new(initial_token),
            min_delay,
            last_request: Mutex::new(None),
            official,
            quotas: Mutex::new(HashMap::new()),
            noratelimit_hint_shown: AtomicBool::new(false),
        })
    }

    /// Whether endpoint-quota tracking is engaged (the official-server
    /// classification). No opt-out exists because none is needed: the
    /// trackers only slow calls after the server itself evidences
    /// limiting, so a noratelimit token runs at full speed untouched.
    pub fn is_official(&self) -> bool {
        self.official
    }

    /// The HTTP base URL this client targets.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The console-websocket endpoint for this server
    /// (`http://h:p` -> `ws://h:p/socket/websocket`; https -> wss).
    pub fn ws_url(&self) -> String {
        ws_url_from_http_base(&self.base_url)
    }

    // ---------------------------------------------------------- auth

    /// `POST /api/auth/signin` — body `{"email": <user>, "password": <pw>}`
    /// -> `{ok, token}`. Private servers accept the username in the
    /// `email` field (screepsmod-auth `lib/backend.js`: passport local
    /// strategy with `usernameField: 'email'`; both python reference
    /// clients send it that way). Stores the session token. No-op for
    /// token auth (official servers support tokens only).
    /// Sources: screeps_api.py (path, body keys, `token` response key);
    /// Endpoints.md (`{ok, token}`); live private server 2026-06-09.
    pub async fn sign_in(&self) -> Result<(), ApiError> {
        let AuthMode::UserPass { .. } = &self.auth else {
            return Ok(());
        };
        let token = self.signin_request().await?;
        *self.token.lock().await = Some(token);
        Ok(())
    }

    /// Mint a SEPARATE session token via a second signin — for the
    /// console websocket. Private-server tokens are rolling/consumable
    /// (every authenticated response replaces them), so sharing this
    /// client's own token with the socket would invalidate one side.
    /// For [`AuthMode::Token`] the configured long-lived token is
    /// returned as-is (official tokens are not consumed). Returned as a
    /// [`SecretString`]: it redacts by construction and is exposed only
    /// at the websocket `auth` send site (P0.A7).
    pub async fn fresh_token(&self) -> Result<SecretString, ApiError> {
        match &self.auth {
            AuthMode::UserPass { .. } => self.signin_request().await,
            AuthMode::Token(token) => Ok(SecretString::from(token.expose_secret())),
        }
    }

    /// One signin round trip -> the new session token. The password is
    /// exposed into the request body ONLY — never logged.
    async fn signin_request(&self) -> Result<SecretString, ApiError> {
        let AuthMode::UserPass { username, password } = &self.auth else {
            return Err(ApiError::Other(
                "fresh sign-in requires username/password auth".to_owned(),
            ));
        };
        let body = serde_json::json!({
            "email": username,
            "password": password.expose_secret(),
        });
        let response: SignInResponse = self
            .post_unsharded("/api/auth/signin", body, "auth/signin response")
            .await?;
        Ok(response.token)
    }

    /// `GET /api/auth/me` -> `{ok, _id, username, ...}` — the signed-in
    /// identity; `_id` keys the console websocket channel
    /// (`user:<id>/console`). Sources: Endpoints.md; live private
    /// server 2026-06-09 (extra fields like cpu/gcl tolerated).
    pub async fn me(&self) -> Result<UserInfo, ApiError> {
        self.get("/api/auth/me", Vec::new(), "auth/me response")
            .await
    }

    // ------------------------------------------------- registration

    /// `GET /api/register/check-username?username=` -> `{ok: 1}` when
    /// available, the `{"error": ...}` envelope when taken. Returns
    /// `Ok(false)` for the taken case (any other error propagates).
    /// Source: screepsmod-auth `lib/register.js` (read from the live
    /// container, screeps-eval bring-up 2026-06-09).
    pub async fn username_available(&self, username: &str) -> Result<bool, ApiError> {
        let result: Result<OkResponse, ApiError> = self
            .get(
                "/api/register/check-username",
                vec![("username", username.to_owned())],
                "check-username response",
            )
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(ApiError::Server { .. }) => Ok(false),
            Err(other) => Err(other),
        }
    }

    /// `POST /api/register/submit` — body `{username, password}` ->
    /// `{ok: 1}`. Creates the user, an empty default code branch, and
    /// empty memory; open unless the server sets `SERVER_PASSWORD`.
    /// The password is exposed into the request body ONLY.
    /// Source: screepsmod-auth `lib/register.js` (live container,
    /// 2026-06-09).
    pub async fn register(
        &self,
        username: &str,
        password: &SecretString,
    ) -> Result<OkResponse, ApiError> {
        let body = serde_json::json!({
            "username": username,
            "password": password.expose_secret(),
        });
        self.post_unsharded("/api/register/submit", body, "register response")
            .await
    }

    // ------------------------------------------------------- world

    /// `GET /api/game/time` -> `{ok, time}`. No auth required.
    /// Sources: Endpoints.md ("Other"); live private server 2026-06-09.
    pub async fn game_time(&self) -> Result<TimeResponse, ApiError> {
        self.get("/api/game/time", Vec::new(), "game-time response")
            .await
    }

    /// `GET /api/game/world-size[?shard=]` -> `{ok, width, height}`.
    /// No auth required. Sources: screeps_api.py `get_world_size`
    /// (path, `without_auth=True`); python-screeps `worldsize` (shard).
    pub async fn world_size(&self) -> Result<WorldSizeResponse, ApiError> {
        self.get("/api/game/world-size", Vec::new(), "world-size response")
            .await
    }

    /// `GET /api/game/shards/info` -> `{ok, shards: [{name, ...}]}`.
    /// Global limiter only (not in ScreepsAPI.js's per-endpoint table).
    /// Sources: node-screeps-api raw.game.shards.info, Endpoints.md.
    ///
    /// Why we expose it: a MISSING shard fails map-stats with "invalid
    /// shard" (verified live 2026-06-10) — but only AFTER a
    /// quota-bearing call has been spent — and the shard list itself
    /// changes over time (shardX showed up alongside shard0..shard3 in
    /// the live 2026-06-10 response). Callers targeting official
    /// servers should validate the chosen shard against this list
    /// before paying for a scan.
    ///
    /// Note [`Self::get`] injects the client's own `shard` into the
    /// query, so this request carries the very value being validated;
    /// screeps.com ignores the parameter here today, but if it ever
    /// starts validating it, a typo'd shard degrades this lookup to
    /// the caller's fallback path rather than producing a clean list.
    pub async fn shards_info(&self) -> Result<ShardsInfoResponse, ApiError> {
        self.get("/api/game/shards/info", Vec::new(), "shards-info response")
            .await
    }

    /// `GET /api/user/world-status` -> `{ok, status}` with status one of
    /// `"empty"` | `"normal"` | `"lost"`. Sources: screeps_api.py
    /// `get_world_status` (path + the three values), Endpoints.md.
    pub async fn world_status(&self) -> Result<WorldStatusResponse, ApiError> {
        self.get(
            "/api/user/world-status",
            Vec::new(),
            "world-status response",
        )
        .await
    }

    /// `POST /api/game/map-stats` — body
    /// `{"rooms": [...], "statName": "owner0", "shard": ...}` ->
    /// `{ok, stats: {<room>: {status, own?, novice?, openTime?, ...}}, users}`.
    /// statName catalogue (screeps_api.py): owner|claim|creepsLost|
    /// creepsProduced|energyConstruction|energyControl|energyCreeps|
    /// energyHarvested with interval suffix 0|8|180|1440; `owner0`/`claim0`
    /// return ownership with no separate stat block (Endpoints.md).
    /// Official-server cap: 60/hour (ScreepsAPI.js:1432) — but `rooms`
    /// is an ARRAY with no documented size cap (neither reference
    /// client chunks it; the open backend takes 8 MB bodies), so batch
    /// big and the cap stops mattering.
    /// Sources: screeps_api.py (body keys), python-screeps (shard in
    /// body), Endpoints.md (response shape).
    pub async fn map_stats(
        &self,
        rooms: &[String],
        stat_name: &str,
    ) -> Result<MapStatsResponse, ApiError> {
        let body = serde_json::json!({
            "rooms": rooms,
            "statName": stat_name,
        });
        self.post("/api/game/map-stats", body, "map-stats response")
            .await
    }

    // ------------------------------------------------------- rooms

    /// `GET /api/game/room-terrain?room=<name>&encoded=1[&shard=]` ->
    /// `{ok, terrain: [{_id?, room, terrain: "<2500 digits>", type: "terrain"}]}`.
    /// `encoded=1` selects the flat 2500-char digit string (0=plain,
    /// 1=wall, 2=swamp, 3=wall; row-major top-to-bottom) — the exact
    /// encoding the foreman-bench map format stores. No auth required.
    /// Official-server cap: 360/hour. Sources: screeps_api.py
    /// `get_room_terrain` (no-auth); python-screeps `room_terrain`
    /// (`encoded='1'`, shard); Endpoints.md (response + digit legend);
    /// auth-tokens.html (cap).
    pub async fn room_terrain_encoded(&self, room: &str) -> Result<RoomTerrainResponse, ApiError> {
        self.get(
            "/api/game/room-terrain",
            vec![("room", room.to_owned()), ("encoded", "1".to_owned())],
            "room-terrain response",
        )
        .await
    }

    /// `GET /api/game/room-objects?room=<name>[&shard=]` ->
    /// `{ok, objects: [{_id, room, type, x, y, ...}], users}`. Objects
    /// carry game-type tags (`"source"`, `"controller"`, `"mineral"`
    /// with `mineralType`, ...). Source: python-screeps `room_objects`
    /// (the operator-referenced screeps_api.py does not wrap this one);
    /// response shape per node-screeps-api usage.
    pub async fn room_objects(&self, room: &str) -> Result<RoomObjectsResponse, ApiError> {
        self.get(
            "/api/game/room-objects",
            vec![("room", room.to_owned())],
            "room-objects response",
        )
        .await
    }

    /// `GET /api/game/room-status?room=<name>[&shard=]` ->
    /// `{ok, room: {_id, status, novice?, openTime?}}`. `status` is
    /// `"normal"` or `"out of borders"`; `novice` is an epoch-ms
    /// timestamp when protection applies. Sources: screeps_api.py
    /// `get_room_status` (path/params), python-screeps (shard),
    /// Endpoints.md (inner-object fields; the `room:` wrapper is the
    /// live server behavior both python clients return verbatim).
    pub async fn room_status(&self, room: &str) -> Result<RoomStatusResponse, ApiError> {
        self.get(
            "/api/game/room-status",
            vec![("room", room.to_owned())],
            "room-status response",
        )
        .await
    }

    // ------------------------------------------------------ account

    /// `POST /api/game/place-spawn` — body
    /// `{"room", "x", "y", "name", "shard": ...}` -> `{ok: 1}` (private
    /// servers add `newbie: true`) or the error envelope. Server-side
    /// validation (`@screeps/backend lib/game/api/game.js`): x,y in
    /// 1..=48, non-wall terrain, no exit object within 1 tile, room
    /// controller exists/unowned/unreserved, user owns zero objects.
    /// Sources: screeps_api.py `place_spawn` (body keys room/x/y/name),
    /// python-screeps (shard in body), live private server 2026-06-09.
    ///
    /// SAFETY: callers gate this — prospector's CLI requires explicit
    /// `--yes`, and auto-placement against screeps.com is forbidden
    /// (recommend-first, always).
    pub async fn place_spawn(
        &self,
        room: &str,
        x: u32,
        y: u32,
        name: &str,
    ) -> Result<OkResponse, ApiError> {
        let body = serde_json::json!({
            "room": room,
            "x": x,
            "y": y,
            "name": name,
        });
        self.post("/api/game/place-spawn", body, "place-spawn response")
            .await
    }

    /// `POST /api/user/respawn` — empty body -> `{ok: 1}`. Kills the
    /// account's spawns/creeps and returns the world to `"empty"`.
    /// Source: screeps_api.py / python-screeps `respawn`.
    pub async fn respawn(&self) -> Result<OkResponse, ApiError> {
        self.post_unsharded(
            "/api/user/respawn",
            serde_json::json!({}),
            "respawn response",
        )
        .await
    }

    // ----------------------------------------------- memory segments

    /// `GET /api/user/memory-segment?segment=N[&shard=]` ->
    /// `{ok, data: <string|null>}` (`null` = never written). N is
    /// 0..=99. Private servers take no shard parameter
    /// (`@screeps/backend lib/game/api/user.js:318-327` reads only
    /// `request.query.segment`) — construct the client with
    /// `shard: None` there. Sources: node-screeps-api
    /// `memory.segment.get` (ScreepsAPI.js:984); live private server
    /// 2026-06-09.
    pub async fn memory_segment(&self, segment: u8) -> Result<MemorySegmentResponse, ApiError> {
        self.get(
            "/api/user/memory-segment",
            vec![("segment", segment.to_string())],
            "memory-segment response",
        )
        .await
    }

    /// `POST /api/user/memory-segment` — body
    /// `{"segment": N, "data": <string>, "shard": ...}` -> `{ok: 1}`.
    /// Official-server caps: GET 360/hour, POST 60/hour
    /// (ScreepsAPI.js:1423/:1436). Source: node-screeps-api
    /// `memory.segment.set` (ScreepsAPI.js:994).
    pub async fn set_memory_segment(
        &self,
        segment: u8,
        data: &str,
    ) -> Result<OkResponse, ApiError> {
        let body = serde_json::json!({
            "segment": segment,
            "data": data,
        });
        self.post(
            "/api/user/memory-segment",
            body,
            "memory-segment write response",
        )
        .await
    }

    // -------------------------------------------------- code upload

    /// `POST /api/user/code` — body `{branch, modules, _hash}` ->
    /// `{ok: 1}`. `modules` maps module name -> JS source string or
    /// `{binary: <base64>}` (see [`crate::code`]); `_hash` is the
    /// current epoch-millis (node-screeps-api `code.set`,
    /// ScreepsAPI.js:894-897: `if (!_hash) _hash = Date.now()`). This is
    /// the upload `js_tools/deploy.js:156-158` performs via
    /// `api.code.set(branch, modules)`. Official-server cap: 240/day
    /// (ScreepsAPI.js:1433). For the future rust-native deploy tool
    /// (P0.A11); fixture-tested only until that lands.
    pub async fn upload_code(
        &self,
        branch: &str,
        modules: &CodeModules,
    ) -> Result<OkResponse, ApiError> {
        let hash = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let body = serde_json::json!({
            "branch": branch,
            "modules": modules,
            "_hash": hash,
        });
        self.post_unsharded("/api/user/code", body, "code upload response")
            .await
    }

    // ---------------------------------------------------- internals

    /// Courtesy rate limit: at least `min_delay` between any two requests.
    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < self.min_delay {
                tokio::time::sleep(self.min_delay - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Evidence-based endpoint-quota pacing (official targets only):
    /// wait until the endpoint's [`QuotaTracker`] admits the next call,
    /// then claim the slot. Zero wait until the server has evidenced
    /// limiting (headers/429) for this endpoint. Loops because the lock
    /// is dropped across the sleep (a long wait must not block
    /// unrelated endpoints).
    async fn wait_for_endpoint_quota(&self, method: &'static str, path: &'static str) {
        if !self.official {
            return;
        }
        let Some(quota) = endpoint_quota(method, path) else {
            return;
        };
        loop {
            let delay = {
                let mut quotas = self.quotas.lock().await;
                let tracker = quotas
                    .entry((method, path))
                    .or_insert_with(|| QuotaTracker::new(quota));
                let now = Instant::now();
                let delay = tracker.required_delay(now);
                if delay.is_zero() {
                    tracker.record_call(now);
                    return;
                }
                delay
            };
            self.maybe_print_noratelimit_hint(delay);
            if delay >= Duration::from_secs(30) {
                tracing::info!(
                    "endpoint quota pacing: waiting {}s before {method} {path}",
                    delay.as_secs()
                );
            } else {
                tracing::debug!("endpoint quota pacing: waiting {delay:?} before {method} {path}");
            }
            tokio::time::sleep(delay).await;
        }
    }

    /// Adopt `X-RateLimit-*` ground truth for a quota-tabled endpoint
    /// (the static table is the prior, headers win — see [`crate::quota`]).
    async fn observe_quota_headers(
        &self,
        method: &'static str,
        path: &'static str,
        info: RateLimitInfo,
    ) {
        if !self.official {
            return;
        }
        let Some(quota) = endpoint_quota(method, path) else {
            return;
        };
        self.quotas
            .lock()
            .await
            .entry((method, path))
            .or_insert_with(|| QuotaTracker::new(quota))
            .observe_headers(info, Instant::now(), unix_now_secs());
    }

    /// Record a live 429 in the endpoint's tracker so the proactive
    /// pacing also knows the window is spent.
    async fn observe_rate_limited(&self, method: &'static str, path: &'static str, wait: Duration) {
        let Some(quota) = endpoint_quota(method, path) else {
            return;
        };
        self.quotas
            .lock()
            .await
            .entry((method, path))
            .or_insert_with(|| QuotaTracker::new(quota))
            .observe_rate_limited(wait, Instant::now());
    }

    /// Before a LONG wait (>= 2 min), tell the operator ONCE about the
    /// sanctioned opt-out: screeps.com lets a signed-in token owner
    /// disable rate limiting per auth token (node-screeps-api's
    /// `rateLimitResetUrl`). Printed, never opened — the choice is theirs.
    fn maybe_print_noratelimit_hint(&self, wait: Duration) {
        if wait >= NORATELIMIT_HINT_THRESHOLD
            && !self.noratelimit_hint_shown.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                "long rate-limit wait ahead ({}s). screeps.com offers a per-token opt-out: \
                 open {NO_RATE_LIMIT_URL} while signed in to disable rate limiting for your \
                 auth token",
                wait.as_secs()
            );
        }
    }

    /// Send with auth headers, accept token rotation, classify the body;
    /// on official 429s, back off for the server's stated retry-after
    /// and RESUME (bounded — see [`RATE_LIMIT_RETRIES`] /
    /// [`MAX_RATE_LIMIT_WAIT`]) instead of failing the caller's run.
    ///
    /// Auth headers: the most recent token is sent as BOTH `X-Token` and
    /// `X-Username` (screeps_api.py / python-screeps; verified live).
    /// Rotation: when a response carries an `X-Token` header of
    /// plausible length (>= 40 chars, the python-screeps rule —
    /// private-server session tokens are exactly 40 hex chars), it
    /// replaces the stored token. Private-server tokens are CONSUMED by
    /// use (screepsmod-auth), so adoption is mandatory there;
    /// Endpoints.md documents the rotation contract.
    ///
    /// Rate-limit headers: `X-RateLimit-Limit`/`-Remaining`/`-Reset`
    /// (reset = epoch seconds) are read off every response and fed to
    /// the endpoint's tracker (node-screeps-api `buildRateLimit`,
    /// dist/ScreepsAPI.js:1238-1254).
    async fn execute<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: &'static str,
        request: reqwest::RequestBuilder,
        context: &'static str,
    ) -> Result<T, ApiError> {
        let mut attempt: u32 = 0;
        loop {
            self.wait_for_endpoint_quota(method, path).await;
            self.throttle().await;
            // Clone per attempt: the original stays replayable for the
            // 429 backoff-and-resume path (bodies here are buffered
            // JSON, always cloneable).
            let attempt_request = request.try_clone().ok_or_else(|| {
                ApiError::Other(format!("{method} {path}: request not cloneable for retry"))
            })?;
            let attempt_request = {
                let token = self.token.lock().await;
                match token.as_ref() {
                    Some(token) => {
                        let value = token.expose_secret();
                        attempt_request
                            .header("X-Token", value)
                            .header("X-Username", value)
                    }
                    None => attempt_request,
                }
            };
            let response = attempt_request.send().await?;
            if let Some(rotated) = response
                .headers()
                .get("X-Token")
                .and_then(|v| v.to_str().ok())
            {
                if rotated.len() >= TOKEN_ROTATION_MIN_LEN {
                    tracing::trace!("adopting rotated session token");
                    *self.token.lock().await = Some(SecretString::from(rotated));
                }
            }
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let rate_info = parse_rate_limit_headers(
                header("x-ratelimit-limit").as_deref(),
                header("x-ratelimit-remaining").as_deref(),
                header("x-ratelimit-reset").as_deref(),
            );
            if let Some(info) = rate_info {
                self.observe_quota_headers(method, path, info).await;
            }
            let status = response.status().as_u16();
            let body = response.text().await?;
            match parse_api_response::<T>(status, &body, context) {
                Err(ApiError::RateLimited { message }) if attempt < RATE_LIMIT_RETRIES => {
                    attempt += 1;
                    // The server's own retry-after is the ground truth;
                    // header reset is the fallback; then a flat default.
                    let wait = parse_retry_after_ms(&message)
                        .map(Duration::from_millis)
                        .or_else(|| {
                            rate_info.filter(|info| info.remaining == 0).map(|info| {
                                Duration::from_secs(
                                    info.reset_epoch_secs.saturating_sub(unix_now_secs()),
                                )
                            })
                        })
                        .unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF);
                    if wait > MAX_RATE_LIMIT_WAIT {
                        tracing::warn!(
                            "rate limited with a {}s retry-after (> {}s bound) — giving up on \
                             {method} {path}; re-run later, completed work is preserved by \
                             callers that persist incrementally",
                            wait.as_secs(),
                            MAX_RATE_LIMIT_WAIT.as_secs()
                        );
                        return Err(ApiError::RateLimited { message });
                    }
                    self.observe_rate_limited(method, path, wait).await;
                    self.maybe_print_noratelimit_hint(wait);
                    tracing::warn!(
                        "rate limited; resuming in {}s (endpoint quota: {method} {path}, \
                         attempt {attempt}/{RATE_LIMIT_RETRIES})",
                        wait.as_secs()
                    );
                    tokio::time::sleep(wait).await;
                }
                other => return other,
            }
        }
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &'static str,
        mut query: Vec<(&'static str, String)>,
        context: &'static str,
    ) -> Result<T, ApiError> {
        if let Some(shard) = &self.shard {
            query.push(("shard", shard.clone()));
        }
        let request = self
            .http
            .get(format!("{}{path}", self.base_url))
            .query(&query);
        self.execute("GET", path, request, context).await
    }

    /// POST with the shard injected into the body (python-screeps puts
    /// shard in the JSON body for POST endpoints).
    async fn post<T: DeserializeOwned>(
        &self,
        path: &'static str,
        mut body: serde_json::Value,
        context: &'static str,
    ) -> Result<T, ApiError> {
        if let (Some(shard), Some(object)) = (&self.shard, body.as_object_mut()) {
            object.insert("shard".to_owned(), serde_json::Value::String(shard.clone()));
        }
        self.post_unsharded(path, body, context).await
    }

    /// POST without shard injection (signin, register, respawn, and
    /// code upload are account-level, not shard-level).
    async fn post_unsharded<T: DeserializeOwned>(
        &self,
        path: &'static str,
        body: serde_json::Value,
        context: &'static str,
    ) -> Result<T, ApiError> {
        let request = self
            .http
            .post(format!("{}{path}", self.base_url))
            .json(&body);
        self.execute("POST", path, request, context).await
    }
}

/// Unix seconds now — anchors `X-RateLimit-Reset` (epoch seconds) to
/// monotonic [`Instant`]s for the trackers.
fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PW: &str = "super-secret-test-pw-7391";
    const FAKE_TOKEN: &str = "ffffffff-aaaa-bbbb-cccc-fake-token-material-0042";

    /// The redaction pin (P0.A7(e) pattern): Debug formatting of both
    /// auth modes must never contain credential material —
    /// `SecretString` redacts by construction, and this test fails if
    /// anyone swaps it for a plain `String`.
    #[test]
    fn auth_mode_debug_redacts_secrets() {
        let user_pass = AuthMode::UserPass {
            username: "ibex".to_owned(),
            password: SecretString::from(FAKE_PW),
        };
        let dump = format!("{user_pass:?}");
        assert!(
            !dump.contains(FAKE_PW),
            "password leaked into Debug: {dump}"
        );
        assert!(dump.contains("ibex"), "non-secret fields stay diagnosable");

        let token = AuthMode::Token(SecretString::from(FAKE_TOKEN));
        let dump = format!("{token:?}");
        assert!(
            !dump.contains(FAKE_TOKEN),
            "token leaked into Debug: {dump}"
        );
    }

    /// The client derives its websocket endpoint from the base URL.
    #[test]
    fn client_ws_url_derivation() {
        let client = Client::new(
            "http://127.0.0.1:21025",
            None,
            AuthMode::UserPass {
                username: "ibex".to_owned(),
                password: SecretString::from(FAKE_PW),
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(client.ws_url(), "ws://127.0.0.1:21025/socket/websocket");
        assert_eq!(client.base_url(), "http://127.0.0.1:21025");
    }

    /// Token rotation respects the python-screeps length rule: a
    /// 40-char private-server token passes, shorter junk does not.
    #[test]
    fn token_rotation_length_rule() {
        assert!("f".repeat(40).len() >= TOKEN_ROTATION_MIN_LEN);
        assert!("unauthorized".len() < TOKEN_ROTATION_MIN_LEN);
    }

    /// The official classification (mirrors prospector's MMO-safety
    /// rule, P0.P4): token auth OR a screeps.com URL — and it drives
    /// whether quota tracking engages on a new client.
    #[test]
    fn official_target_classification_drives_quota_tracking() {
        let token = || AuthMode::Token(SecretString::from(FAKE_TOKEN));
        let userpass = || AuthMode::UserPass {
            username: "ibex".to_owned(),
            password: SecretString::from(FAKE_PW),
        };
        assert!(is_official_target("https://screeps.com", &token()));
        assert!(
            is_official_target("http://127.0.0.1:21025", &token()),
            "token auth gets MMO-grade caution even against a private host"
        );
        assert!(is_official_target(
            "https://screeps.com/season",
            &userpass()
        ));
        assert!(!is_official_target("http://127.0.0.1:21025", &userpass()));

        let mmo = Client::new(
            "https://screeps.com",
            Some("shard3".to_owned()),
            token(),
            Duration::from_millis(DEFAULT_MIN_DELAY_MS),
        )
        .unwrap();
        assert!(mmo.is_official(), "quota tracking auto-engages on MMO");

        let private =
            Client::new("http://127.0.0.1:21025", None, userpass(), Duration::ZERO).unwrap();
        assert!(
            !private.is_official(),
            "private servers keep plain min-delay"
        );
    }
}
