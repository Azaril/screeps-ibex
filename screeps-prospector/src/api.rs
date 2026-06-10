//! REST client for the Screeps HTTP API (private servers + screeps.com).
//!
//! Endpoint shapes are PINNED from public client implementations, not
//! guessed. Each method's doc comment names its sources; the catalogue:
//!
//! - `screeps_api.py` — [Qionglu735/screeps_tool], the operator-referenced
//!   reference client: auth/signin body + token headers, world-size,
//!   map-stats statName values, room-terrain, room-status, world-status,
//!   place-spawn body, respawn.
//! - `screepsapi.py` — [screepers/python-screeps]: shard parameter
//!   placement (query for GETs, body key for POSTs), room-objects,
//!   token-rotation acceptance rule (`len >= 40`).
//! - `Endpoints.md` — [screepers/node-screeps-api]: response shapes
//!   (terrain digit legend, map-stats stats/users maps, signin response),
//!   token rotation via the `X-Token` response header.
//! - <https://docs.screeps.com/auth-tokens.html> — official-server rate
//!   limits (global 120/min; per-endpoint caps, e.g. room-terrain
//!   360/hour; `X-RateLimit-*` headers; HTTP 429 on excess).
//!
//! NETWORK POLICY (Workstream-P live-test constraint): unit tests parse
//! recorded/literal fixtures only — nothing under `#[cfg(test)]` performs
//! I/O. Live calls happen only at runtime via the CLI, and never against
//! screeps.com during the Phase-0 build.

use crate::config::AuthMode;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

/// Default courtesy delay between requests, sized for screeps.com: the
/// global auth-token limit is 120 requests/minute (= one per 500 ms),
/// with stricter per-endpoint caps (GET room-terrain: 360/hour).
/// 600 ms keeps a sustained scan safely under the global cap; private
/// servers can pass something much lower.
/// Source: <https://docs.screeps.com/auth-tokens.html>
pub const DEFAULT_MIN_DELAY_MS: u64 = 600;

/// Tokens shorter than this are not treated as rotation material — the
/// python-screeps client's acceptance rule for the `X-Token` response
/// header (`if len(r.headers['X-Token']) >= 40`).
const TOKEN_ROTATION_MIN_LEN: usize = 40;

#[derive(Debug, Error)]
pub enum ApiError {
    /// The application-level error envelope `{"error": "..."}` — both
    /// private servers and screeps.com return it, often with HTTP 200.
    #[error("server error: {message}")]
    Server { message: String },
    /// HTTP 429 — "Rate limit exceeded, retry after <n>ms"
    /// (<https://docs.screeps.com/auth-tokens.html>).
    #[error("rate limited: {message}")]
    RateLimited { message: String },
    #[error("http status {status}: {body}")]
    Http { status: u16, body: String },
    #[error("decoding {context}: {source}")]
    Decode {
        context: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}

// ---- typed responses (tolerant: no deny_unknown_fields anywhere) ----

/// `GET /api/game/world-size` response. Width/height count rooms across
/// the map (e.g. 122 on official shards: W60..E60 / N60..S60).
#[derive(Debug, Deserialize)]
pub struct WorldSizeResponse {
    pub ok: i32,
    pub width: u32,
    pub height: u32,
}

/// `POST /api/game/map-stats` response.
#[derive(Debug, Deserialize)]
pub struct MapStatsResponse {
    pub ok: i32,
    /// Keyed by room name. Rooms outside the map come back with
    /// `status: "out of borders"` and nothing else.
    pub stats: HashMap<String, RoomMapStats>,
    /// Keyed by user `_id`; referenced from `stats.*.own.user`.
    #[serde(default)]
    pub users: serde_json::Value,
}

/// Per-room entry of the map-stats response (statName `owner0`).
#[derive(Debug, Default, Deserialize)]
pub struct RoomMapStats {
    /// `"normal"` or `"out of borders"` (Endpoints.md).
    #[serde(default)]
    pub status: Option<String>,
    /// Present when the room is owned or reserved (`level: 0` =
    /// reserved, per Endpoints.md's claim0 note).
    #[serde(default)]
    pub own: Option<RoomOwner>,
    /// Novice-area protection end, Unix epoch milliseconds (kept lazy:
    /// the server sends numbers or null).
    #[serde(default)]
    pub novice: Option<serde_json::Value>,
    /// Wall-removal time for newbie-zone rooms, epoch milliseconds.
    #[serde(default, rename = "openTime")]
    pub open_time: Option<serde_json::Value>,
    /// Respawn-area protection end, epoch milliseconds (official server).
    #[serde(default, rename = "respawnArea")]
    pub respawn_area: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct RoomOwner {
    pub user: String,
    pub level: u32,
}

/// `GET /api/game/room-terrain?encoded=1` response.
#[derive(Debug, Deserialize)]
pub struct RoomTerrainResponse {
    pub ok: i32,
    pub terrain: Vec<TerrainEntry>,
}

#[derive(Debug, Deserialize)]
pub struct TerrainEntry {
    #[serde(default)]
    pub room: Option<String>,
    /// 2500 digits (50x50, row-major top-to-bottom): 0=plain, 1=wall,
    /// 2=swamp, 3=wall (Endpoints.md legend) — the exact encoding the
    /// foreman-bench map JSON stores.
    pub terrain: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// `GET /api/game/room-objects` response.
#[derive(Debug, Deserialize)]
pub struct RoomObjectsResponse {
    pub ok: i32,
    /// Lazily-typed: per-type fields vary wildly. The cache layer
    /// filters these down to the planner-relevant `{type,x,y}` shape
    /// (`crate::cache::filter_planner_objects`).
    pub objects: Vec<serde_json::Value>,
    #[serde(default)]
    pub users: serde_json::Value,
}

/// `GET /api/game/room-status` response.
#[derive(Debug, Deserialize)]
pub struct RoomStatusResponse {
    pub ok: i32,
    pub room: Option<RoomStatusEntry>,
}

#[derive(Debug, Deserialize)]
pub struct RoomStatusEntry {
    #[serde(rename = "_id")]
    pub id: String,
    /// `"normal"` or `"out of borders"`.
    pub status: String,
    /// Novice protection end, epoch milliseconds, when applicable.
    #[serde(default)]
    pub novice: Option<serde_json::Value>,
    #[serde(default, rename = "openTime")]
    pub open_time: Option<serde_json::Value>,
}

/// `GET /api/user/world-status` response.
#[derive(Debug, Deserialize)]
pub struct WorldStatusResponse {
    pub ok: i32,
    /// `"empty"` (no spawn placed), `"normal"`, or `"lost"` (all spawns
    /// dead — respawn possible). Source: screeps_api.py + Endpoints.md.
    pub status: String,
}

/// Bare `{ok: 1}` acknowledgement (place-spawn, respawn).
#[derive(Debug, Deserialize)]
pub struct OkResponse {
    pub ok: i32,
}

/// `POST /api/auth/signin` response (Endpoints.md: `{ok, token}`).
/// The token is credential material — parsed straight into a
/// [`SecretString`] so Debug output redacts it.
#[derive(Debug, Deserialize)]
struct SignInResponse {
    #[allow(dead_code)]
    ok: i32,
    token: SecretString,
}

/// Classify an HTTP response into the typed result. Pure — unit-tested
/// against literal fixtures (no network in tests).
fn parse_api_response<T: DeserializeOwned>(
    status: u16,
    body: &str,
    context: &'static str,
) -> Result<T, ApiError> {
    if status == 429 {
        return Err(ApiError::RateLimited {
            message: body.trim().to_owned(),
        });
    }
    // The error envelope can arrive with HTTP 200 — check it first.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = value.get("error").and_then(|e| e.as_str()) {
            return Err(ApiError::Server {
                message: message.to_owned(),
            });
        }
    }
    if !(200..300).contains(&status) {
        let mut body = body.trim().to_owned();
        body.truncate(500);
        return Err(ApiError::Http { status, body });
    }
    serde_json::from_str(body).map_err(|source| ApiError::Decode { context, source })
}

/// Enumerate every room name on a `width` x `height` map, mirroring the
/// server's coordinate scheme: horizontal index `i in 0..width` maps to
/// `x = i - width/2`, named `W{-x-1}` for negative x and `E{x}` else;
/// vertical likewise with `N`/`S`. A 122-wide official shard thus spans
/// W60..E60.
pub fn enumerate_room_names(width: u32, height: u32) -> Vec<String> {
    let half_w = (width / 2) as i64;
    let half_h = (height / 2) as i64;
    let mut names = Vec::with_capacity((width * height) as usize);
    for yi in 0..height as i64 {
        let y = yi - half_h;
        for xi in 0..width as i64 {
            let x = xi - half_w;
            let ew = if x < 0 {
                format!("W{}", -x - 1)
            } else {
                format!("E{x}")
            };
            let ns = if y < 0 {
                format!("N{}", -y - 1)
            } else {
                format!("S{y}")
            };
            names.push(format!("{ew}{ns}"));
        }
    }
    names
}

/// Async REST client over `reqwest` with a courtesy rate limit and
/// rotating-token auth. Construct via [`ProspectorClient::new`], then
/// call [`sign_in`](Self::sign_in) once (a no-op for token auth).
pub struct ProspectorClient {
    http: reqwest::Client,
    base_url: String,
    shard: Option<String>,
    auth: AuthMode,
    /// Current session token; replaced when a response rotates it.
    token: Mutex<Option<SecretString>>,
    min_delay: Duration,
    last_request: Mutex<Option<Instant>>,
}

impl ProspectorClient {
    /// `base_url` is `scheme://host[:port][/path]` (no trailing slash) —
    /// the shape [`crate::config::ProspectorConfig`] produces. `shard`
    /// is required for official servers (sent as a query param on GETs
    /// and a body key on POSTs, per python-screeps) and omitted for
    /// private servers. `min_delay` is the courtesy gap between any two
    /// requests ([`DEFAULT_MIN_DELAY_MS`]).
    pub fn new(
        base_url: impl Into<String>,
        shard: Option<String>,
        auth: AuthMode,
        min_delay: Duration,
    ) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("screeps-prospector/", env!("CARGO_PKG_VERSION")))
            .build()?;
        // Token auth needs no sign-in round trip: seed the session
        // token directly from the config.
        let initial_token = match &auth {
            AuthMode::Token(token) => Some(SecretString::from(token.expose_secret())),
            AuthMode::UserPass { .. } => None,
        };
        Ok(ProspectorClient {
            http,
            base_url: base_url.into(),
            shard,
            auth,
            token: Mutex::new(initial_token),
            min_delay,
            last_request: Mutex::new(None),
        })
    }

    /// `POST /api/auth/signin` — body `{"email": <user>, "password": <pw>}`
    /// → `{ok, token}`. Private servers accept the username in the
    /// `email` field (both reference clients send it that way). No-op
    /// for token auth (official servers support tokens only).
    /// Sources: screeps_api.py (path, body keys, `token` response key);
    /// Endpoints.md (`{ok, token}`).
    pub async fn sign_in(&self) -> Result<(), ApiError> {
        let AuthMode::UserPass { username, password } = &self.auth else {
            return Ok(());
        };
        let body = serde_json::json!({
            "email": username,
            "password": password.expose_secret(),
        });
        let response: SignInResponse = self
            .post_unsharded("/api/auth/signin", body, "auth/signin response")
            .await?;
        *self.token.lock().await = Some(response.token);
        Ok(())
    }

    /// `GET /api/game/world-size[?shard=]` → `{ok, width, height}`.
    /// No auth required. Sources: screeps_api.py `get_world_size`
    /// (path, `without_auth=True`); python-screeps `worldsize` (shard).
    pub async fn world_size(&self) -> Result<WorldSizeResponse, ApiError> {
        self.get("/api/game/world-size", Vec::new(), "world-size response")
            .await
    }

    /// `POST /api/game/map-stats` — body
    /// `{"rooms": [...], "statName": "owner0", "shard": ...}` →
    /// `{ok, stats: {<room>: {status, own?, novice?, openTime?, ...}}, users}`.
    /// statName catalogue (screeps_api.py): owner|claim|creepsLost|
    /// creepsProduced|energyConstruction|energyControl|energyCreeps|
    /// energyHarvested with interval suffix 0|8|180|1440; `owner0`/`claim0`
    /// return ownership with no separate stat block (Endpoints.md).
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

    /// `GET /api/game/room-terrain?room=<name>&encoded=1[&shard=]` →
    /// `{ok, terrain: [{_id?, room, terrain: "<2500 digits>", type: "terrain"}]}`.
    /// `encoded=1` selects the flat 2500-char digit string (0=plain,
    /// 1=wall, 2=swamp, 3=wall; row-major top-to-bottom) — the exact
    /// encoding the foreman-bench cache format stores. No auth required.
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

    /// `GET /api/game/room-objects?room=<name>[&shard=]` →
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

    /// `GET /api/game/room-status?room=<name>[&shard=]` →
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

    /// `GET /api/user/world-status` → `{ok, status}` with status one of
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

    /// `POST /api/game/place-spawn` — body
    /// `{"room", "x", "y", "name", "shard": ...}` → `{ok: 1}` or the
    /// error envelope (e.g. invalid tile / not allowed). Sources:
    /// screeps_api.py `place_spawn` (body keys room/x/y/name),
    /// python-screeps (shard in body).
    ///
    /// SAFETY: callers gate this — P0.P4's CLI requires explicit
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

    /// `POST /api/user/respawn` — empty body → `{ok: 1}`. Kills the
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

    // ---- internals ----

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

    /// Send with auth headers, accept token rotation, classify the body.
    ///
    /// Auth headers: the most recent token is sent as BOTH `X-Token` and
    /// `X-Username` (screeps_api.py / python-screeps). Rotation: when a
    /// response carries an `X-Token` header of plausible length
    /// (>= 40 chars, the python-screeps rule), it replaces the stored
    /// token (Endpoints.md documents the rotation contract).
    async fn execute<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        context: &'static str,
    ) -> Result<T, ApiError> {
        self.throttle().await;
        let request = {
            let token = self.token.lock().await;
            match token.as_ref() {
                Some(token) => {
                    let value = token.expose_secret();
                    request.header("X-Token", value).header("X-Username", value)
                }
                None => request,
            }
        };
        let response = request.send().await?;
        if let Some(rotated) = response
            .headers()
            .get("X-Token")
            .and_then(|v| v.to_str().ok())
        {
            if rotated.len() >= TOKEN_ROTATION_MIN_LEN {
                *self.token.lock().await = Some(SecretString::from(rotated));
            }
        }
        let status = response.status().as_u16();
        let body = response.text().await?;
        parse_api_response(status, &body, context)
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
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
        self.execute(request, context).await
    }

    /// POST with the shard injected into the body (python-screeps puts
    /// shard in the JSON body for POST endpoints).
    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        mut body: serde_json::Value,
        context: &'static str,
    ) -> Result<T, ApiError> {
        if let (Some(shard), Some(object)) = (&self.shard, body.as_object_mut()) {
            object.insert("shard".to_owned(), serde_json::Value::String(shard.clone()));
        }
        self.post_unsharded(path, body, context).await
    }

    /// POST without shard injection (signin and respawn are
    /// account-level, not shard-level).
    async fn post_unsharded<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
        context: &'static str,
    ) -> Result<T, ApiError> {
        let request = self
            .http
            .post(format!("{}{path}", self.base_url))
            .json(&body);
        self.execute(request, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 2500-char encoded terrain string (the room-terrain
    /// `encoded=1` shape): 50x50 digits. "0123" repeated covers every
    /// legal digit value.
    fn terrain_2500() -> String {
        "0123".repeat(625)
    }

    /// RECORDED SHAPE: room-terrain?encoded=1 (Endpoints.md + screeps_api.py).
    #[test]
    fn room_terrain_encoded_fixture_parses_with_2500_chars() {
        let fixture = format!(
            r#"{{"ok":1,"terrain":[{{"_id":"abc123","room":"W10N10","terrain":"{}","type":"terrain"}}]}}"#,
            terrain_2500()
        );
        let parsed: RoomTerrainResponse =
            parse_api_response(200, &fixture, "room-terrain response").unwrap();
        assert_eq!(parsed.ok, 1);
        let entry = &parsed.terrain[0];
        assert_eq!(entry.room.as_deref(), Some("W10N10"));
        assert_eq!(entry.kind.as_deref(), Some("terrain"));
        assert_eq!(entry.terrain.len(), 2500, "encoded terrain is 50x50 digits");
    }

    /// RECORDED SHAPE: map-stats with statName owner0 (Endpoints.md):
    /// owned room (own.user/level), open room (status only), novice
    /// room (timestamp), out-of-borders room.
    #[test]
    fn map_stats_fixture_parses() {
        let fixture = r#"{
            "ok": 1,
            "stats": {
                "W1N1": {"status": "normal", "own": {"user": "57fake1d", "level": 8}},
                "W2N1": {"status": "normal"},
                "W3N1": {"status": "normal", "novice": 1767225600000},
                "W99N99": {"status": "out of borders"}
            },
            "users": {"57fake1d": {"_id": "57fake1d", "username": "SomePlayer", "badge": {}}}
        }"#;
        let parsed: MapStatsResponse =
            parse_api_response(200, fixture, "map-stats response").unwrap();
        assert_eq!(parsed.ok, 1);
        assert_eq!(parsed.stats.len(), 4);
        let owned = &parsed.stats["W1N1"];
        assert_eq!(owned.own.as_ref().unwrap().user, "57fake1d");
        assert_eq!(owned.own.as_ref().unwrap().level, 8);
        let open = &parsed.stats["W2N1"];
        assert!(open.own.is_none());
        assert_eq!(open.status.as_deref(), Some("normal"));
        let novice = &parsed.stats["W3N1"];
        assert_eq!(
            novice.novice.as_ref().unwrap().as_i64(),
            Some(1767225600000)
        );
        let outside = &parsed.stats["W99N99"];
        assert_eq!(outside.status.as_deref(), Some("out of borders"));
    }

    /// RECORDED SHAPE: room-objects (python-screeps endpoint; lazily
    /// typed objects with per-type extras like mineralType).
    #[test]
    fn room_objects_fixture_parses() {
        let fixture = r#"{
            "ok": 1,
            "objects": [
                {"_id": "a1", "room": "W10N10", "type": "source", "x": 14, "y": 9, "energy": 3000, "energyCapacity": 3000},
                {"_id": "a2", "room": "W10N10", "type": "controller", "x": 21, "y": 32, "level": 0},
                {"_id": "a3", "room": "W10N10", "type": "mineral", "x": 40, "y": 40, "mineralType": "H", "mineralAmount": 65000}
            ],
            "users": {}
        }"#;
        let parsed: RoomObjectsResponse =
            parse_api_response(200, fixture, "room-objects response").unwrap();
        assert_eq!(parsed.objects.len(), 3);
        assert_eq!(parsed.objects[2]["mineralType"], "H");
    }

    /// RECORDED SHAPE: room-status `room:` wrapper (live server shape
    /// returned verbatim by both python reference clients).
    #[test]
    fn room_status_fixture_parses() {
        let fixture = r#"{"ok":1,"room":{"_id":"W3N1","status":"normal","novice":1767225600000}}"#;
        let parsed: RoomStatusResponse =
            parse_api_response(200, fixture, "room-status response").unwrap();
        let room = parsed.room.unwrap();
        assert_eq!(room.id, "W3N1");
        assert_eq!(room.status, "normal");
        assert!(room.novice.is_some());
    }

    /// RECORDED SHAPES: world-size, world-status, ok-acks.
    #[test]
    fn small_response_fixtures_parse() {
        let ws: WorldSizeResponse =
            parse_api_response(200, r#"{"ok":1,"width":10,"height":10}"#, "ws").unwrap();
        assert_eq!((ws.width, ws.height), (10, 10));

        let status: WorldStatusResponse =
            parse_api_response(200, r#"{"ok":1,"status":"empty"}"#, "wst").unwrap();
        assert_eq!(status.status, "empty");

        let ack: OkResponse = parse_api_response(200, r#"{"ok":1}"#, "ack").unwrap();
        assert_eq!(ack.ok, 1);
    }

    /// RECORDED SHAPE: the `{"error": ...}` envelope — arrives with
    /// HTTP 200 and must beat the typed parse.
    #[test]
    fn error_envelope_beats_typed_parse() {
        let err = parse_api_response::<OkResponse>(200, r#"{"error":"invalid room"}"#, "ack")
            .unwrap_err();
        match err {
            ApiError::Server { message } => assert_eq!(message, "invalid room"),
            other => panic!("expected Server, got {other:?}"),
        }
    }

    /// HTTP 429 carries the official server's retry message.
    #[test]
    fn http_429_maps_to_rate_limited() {
        let err = parse_api_response::<OkResponse>(
            429,
            r#"{"error":"Rate limit exceeded, retry after 743ms"}"#,
            "ack",
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::RateLimited { .. }), "got {err:?}");
    }

    #[test]
    fn non_2xx_without_envelope_maps_to_http() {
        let err = parse_api_response::<OkResponse>(502, "Bad Gateway", "ack").unwrap_err();
        match err {
            ApiError::Http { status, .. } => assert_eq!(status, 502),
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn garbage_body_maps_to_decode() {
        let err = parse_api_response::<OkResponse>(200, "<html>nope</html>", "ack").unwrap_err();
        assert!(matches!(err, ApiError::Decode { .. }), "got {err:?}");
    }

    /// Redaction pin (P0.A7 pattern): the signin response carries the
    /// session token — its Debug output must not.
    #[test]
    fn signin_response_debug_redacts_token() {
        let token = "ffffffff-aaaa-bbbb-cccc-fake-token-material-0042";
        let fixture = format!(r#"{{"ok":1,"token":"{token}"}}"#);
        let parsed: SignInResponse = parse_api_response(200, &fixture, "signin").unwrap();
        let dump = format!("{parsed:?}");
        assert!(!dump.contains(token), "token leaked into Debug: {dump}");
    }

    /// Room-name enumeration mirrors the W/E + N/S quadrant scheme.
    #[test]
    fn enumerate_room_names_quadrants() {
        let names = enumerate_room_names(4, 4);
        assert_eq!(names.len(), 16);
        // Corners: top-left is the most-negative (W,N) corner.
        assert_eq!(names.first().map(String::as_str), Some("W1N1"));
        assert_eq!(names.last().map(String::as_str), Some("E1S1"));
        // The four quadrant names around the origin all exist.
        for expected in ["W0N0", "E0N0", "W0S0", "E0S0"] {
            assert!(names.iter().any(|n| n == expected), "missing {expected}");
        }
    }
}
