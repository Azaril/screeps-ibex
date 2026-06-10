//! Game-API HTTP client (P0.A3 bootstrap calls + P0.A5 capture reads).
//!
//! ## Pinned endpoint shapes (live server 2026-06-09 + container sources)
//!
//! - `POST /api/auth/signin` `{email: <username>, password}` →
//!   `{ok:1, token}` (screepsmod-auth `lib/backend.js`, passport local
//!   strategy with `usernameField: 'email'`). Tokens are **rolling**:
//!   every authenticated response carries a refreshed token in its
//!   `X-Token` header which must be adopted.
//! - `GET /api/auth/me` (token auth) → `{ok:1, _id, username, cpu, gcl, ...}`
//!   — `_id` is the user id the console websocket channel is keyed by.
//! - `GET /api/game/time` → `{ok:1, time: <u64>}` (no auth).
//! - `GET /api/user/memory-segment?segment=N` (token auth) →
//!   `{ok:1, data: <string|null>}`. N is validated to 0..=99; **no shard
//!   parameter exists on the private server** (`@screeps/backend
//!   lib/game/api/user.js:318-327` reads only `request.query.segment` —
//!   the MMO's `shard` param is a screeps.com addition).
//! - `GET /api/user/world-status` (token auth) →
//!   `{ok:1, status: "empty"|"normal"|"lost"}`.
//! - `POST /api/game/place-spawn` (token auth) `{room,x,y,name}` →
//!   `{ok:1, newbie:true}` (validation details: `server.rs` module docs).
//! - Console websocket: `ws://host:port/socket/websocket` — see
//!   `capture.rs` for the pinned socket protocol.
//!
//! SECRETS: the password is exposed only into signin/register request
//! bodies; the rolling token lives in a private field and is never
//! logged. Fresh tokens minted for the websocket are returned as
//! [`SecretString`] so they redact by construction (P0.A7).

use crate::config::ServerEndpoint;
use anyhow::{bail, Context, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

/// Response envelope for the API endpoints we consume (one lazily-typed
/// struct: every endpoint returns `ok:1` plus its own fields).
#[derive(Debug, Deserialize)]
pub(crate) struct ApiOkResponse {
    pub ok: Option<i64>,
    pub error: Option<String>,
    pub token: Option<String>,
    pub status: Option<String>,
    pub time: Option<u64>,
    /// `/api/auth/me`
    #[serde(rename = "_id")]
    pub id: Option<String>,
    pub username: Option<String>,
    /// `/api/user/memory-segment` — string content or JSON `null`
    /// (an unwritten segment); both deserialize to `None`/`Some`.
    pub data: Option<String>,
}

/// The signed-in user's identity (`/api/auth/me`).
#[derive(Debug, Clone)]
pub struct UserInfo {
    /// Mongo id — the console websocket channel key (`user:<id>/console`).
    pub id: String,
    pub username: String,
}

/// Derive the websocket endpoint from an HTTP base URL
/// (`http://h:p` → `ws://h:p/socket/websocket`; https → wss).
pub fn ws_url_from_http_base(http_base: &str) -> String {
    let ws_base = if let Some(rest) = http_base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = http_base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        // Already a ws(s) URL or schemeless — pass through unchanged.
        http_base.to_string()
    };
    format!("{ws_base}/socket/websocket")
}

pub struct GameApi {
    base: String,
    http: reqwest::Client,
    /// Rolling auth state: screepsmod-auth refreshes the token in every
    /// response's `X-Token` header; the old one is consumed.
    token: Option<String>,
}

impl GameApi {
    pub fn new(server: &ServerEndpoint) -> Result<Self> {
        Ok(GameApi {
            base: server.http_base(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .context("building HTTP client")?,
            token: None,
        })
    }

    /// The console-websocket endpoint for this server.
    pub fn ws_url(&self) -> String {
        ws_url_from_http_base(&self.base)
    }

    pub async fn game_time(&self) -> Result<u64> {
        let resp: ApiOkResponse = self
            .http
            .get(format!("{}/api/game/time", self.base))
            .send()
            .await
            .with_context(|| format!("GET {}/api/game/time", self.base))?
            .json()
            .await?;
        resp.time.context("no `time` in /api/game/time response")
    }

    pub async fn username_available(&self, username: &str) -> Result<bool> {
        let resp: ApiOkResponse = self
            .http
            .get(format!("{}/api/register/check-username", self.base))
            .query(&[("username", username)])
            .send()
            .await?
            .json()
            .await
            .context("parsing check-username response")?;
        Ok(resp.ok == Some(1))
    }

    /// Register the user (screepsmod-auth open registration). The
    /// password is exposed into the request body ONLY — never logged.
    pub async fn register(&self, server: &ServerEndpoint) -> Result<()> {
        let body = serde_json::json!({
            "username": server.username,
            "password": server.password.expose_secret(),
        });
        let resp: ApiOkResponse = self
            .http
            .post(format!("{}/api/register/submit", self.base))
            .json(&body)
            .send()
            .await?
            .json()
            .await
            .context("parsing register response")?;
        if resp.ok != Some(1) {
            bail!(
                "registering user '{}' failed: {}",
                server.username,
                resp.error.as_deref().unwrap_or("unknown error")
            );
        }
        Ok(())
    }

    /// One signin request → token. Shared by [`signin`](Self::signin)
    /// (stores it for this client's rolling auth) and
    /// [`fresh_token`](Self::fresh_token) (hands it to the websocket).
    async fn signin_request(&self, server: &ServerEndpoint) -> Result<String> {
        let body = serde_json::json!({
            "email": server.username,
            "password": server.password.expose_secret(),
        });
        let resp = self
            .http
            .post(format!("{}/api/auth/signin", self.base))
            .json(&body)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            bail!(
                "signin as '{}' rejected (401) — the server-side password does not \
                 match .screeps.yaml; run `bootstrap` to converge it",
                server.username
            );
        }
        let parsed: ApiOkResponse = resp.json().await.context("parsing signin response")?;
        match (parsed.ok, parsed.token) {
            (Some(1), Some(token)) => Ok(token),
            _ => bail!(
                "signin as '{}' failed: {}",
                server.username,
                parsed.error.as_deref().unwrap_or("no token in response")
            ),
        }
    }

    /// Sign in with the configured credentials; stores the token.
    /// Proves the password works end-to-end (the bootstrap verification).
    pub async fn signin(&mut self, server: &ServerEndpoint) -> Result<()> {
        self.token = Some(self.signin_request(server).await?);
        Ok(())
    }

    /// Mint a SEPARATE token via a second signin — for the console
    /// websocket. Tokens are rolling/consumable, so sharing this
    /// client's own token with the socket would invalidate one side.
    /// Returned as a `SecretString`: it redacts by construction and is
    /// exposed only at the websocket `auth` send site (P0.A7).
    pub async fn fresh_token(&self, server: &ServerEndpoint) -> Result<SecretString> {
        Ok(SecretString::from(self.signin_request(server).await?))
    }

    /// Authenticated request helper: sends `X-Token`/`X-Username`,
    /// adopts the refreshed token from the response header.
    async fn authed(&mut self, req: reqwest::RequestBuilder) -> Result<ApiOkResponse> {
        let token = self.token.clone().context("not signed in")?;
        let resp = req
            .header("X-Token", &token)
            .header("X-Username", &token)
            .send()
            .await?;
        if let Some(fresh) = resp.headers().get("x-token").and_then(|v| v.to_str().ok()) {
            if !fresh.is_empty() {
                self.token = Some(fresh.to_string());
            }
        }
        let status = resp.status();
        let body = resp.text().await?;
        serde_json::from_str(&body)
            .with_context(|| format!("API response (HTTP {status}) not understood: {body}"))
    }

    /// `GET /api/auth/me` — the signed-in identity (id keys the console
    /// websocket channel).
    pub async fn me(&mut self) -> Result<UserInfo> {
        let req = self.http.get(format!("{}/api/auth/me", self.base));
        let resp = self.authed(req).await?;
        if resp.ok != Some(1) {
            bail!(
                "/api/auth/me failed: {}",
                resp.error.as_deref().unwrap_or("unknown error")
            );
        }
        Ok(UserInfo {
            id: resp.id.context("no `_id` in /api/auth/me response")?,
            username: resp
                .username
                .context("no `username` in /api/auth/me response")?,
        })
    }

    /// `GET /api/user/memory-segment?segment=N` → the segment's string
    /// content, or `None` if it has never been written (`data: null`).
    pub async fn memory_segment(&mut self, segment: u8) -> Result<Option<String>> {
        let req = self
            .http
            .get(format!("{}/api/user/memory-segment", self.base))
            .query(&[("segment", segment.to_string())]);
        let resp = self.authed(req).await?;
        if resp.ok != Some(1) {
            bail!(
                "memory-segment {segment} read failed: {}",
                resp.error.as_deref().unwrap_or("unknown error")
            );
        }
        Ok(resp.data)
    }

    /// `empty` (no spawn yet), `normal` (alive), or `lost` (wiped out).
    pub async fn world_status(&mut self) -> Result<String> {
        let req = self
            .http
            .get(format!("{}/api/user/world-status", self.base));
        let resp = self.authed(req).await?;
        resp.status.context("no `status` in world-status response")
    }

    pub async fn place_spawn(&mut self, room: &str, x: u32, y: u32, name: &str) -> Result<()> {
        let body = serde_json::json!({"room": room, "x": x, "y": y, "name": name});
        let req = self
            .http
            .post(format!("{}/api/game/place-spawn", self.base))
            .json(&body);
        let resp = self.authed(req).await?;
        if resp.ok != Some(1) {
            bail!(
                "place-spawn {room} ({x},{y}) rejected: {}",
                resp.error.as_deref().unwrap_or("unknown error")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_derivation() {
        assert_eq!(
            ws_url_from_http_base("http://127.0.0.1:21025"),
            "ws://127.0.0.1:21025/socket/websocket"
        );
        assert_eq!(
            ws_url_from_http_base("https://screeps.example:443"),
            "wss://screeps.example:443/socket/websocket"
        );
    }

    /// Literal response bodies captured from the live server (2026-06-09).
    #[test]
    fn parses_live_response_shapes() {
        // GET /api/game/time
        let r: ApiOkResponse = serde_json::from_str(r#"{"ok":1,"time":7435}"#).unwrap();
        assert_eq!(r.ok, Some(1));
        assert_eq!(r.time, Some(7435));

        // GET /api/user/memory-segment?segment=99 — never written
        let r: ApiOkResponse = serde_json::from_str(r#"{"ok":1,"data":null}"#).unwrap();
        assert_eq!(r.ok, Some(1));
        assert_eq!(r.data, None);

        // ... and with content
        let r: ApiOkResponse = serde_json::from_str(r#"{"ok":1,"data":"{\"shard\":{}}"}"#).unwrap();
        assert_eq!(r.data.as_deref(), Some(r#"{"shard":{}}"#));

        // GET /api/auth/me (extra fields like cpu/gcl must be tolerated)
        let r: ApiOkResponse = serde_json::from_str(
            r#"{"ok":1,"_id":"6a28d4d7d9592a0060be10ef","username":"Azaril","cpu":100,"gcl":0}"#,
        )
        .unwrap();
        assert_eq!(r.id.as_deref(), Some("6a28d4d7d9592a0060be10ef"));
        assert_eq!(r.username.as_deref(), Some("Azaril"));

        // GET /api/user/world-status
        let r: ApiOkResponse = serde_json::from_str(r#"{"ok":1,"status":"normal"}"#).unwrap();
        assert_eq!(r.status.as_deref(), Some("normal"));
    }
}
