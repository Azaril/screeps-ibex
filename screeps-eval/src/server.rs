//! Server-CLI client, world bootstrap, and tick control (P0.A3 / P0.A8).
//!
//! ## Pinned CLI protocol (verified live against the running stack,
//! 2026-06-09; source: `/screeps/mods/screeps-launcher-cli.js`, written
//! by screepers/screeps-launcher — its Go client `cli/cli.go` speaks the
//! same shapes)
//!
//! - `GET /greeting` → 200, plain-text banner.
//! - `POST /cli` with the raw JavaScript command as the body (any
//!   content type; `bodyParser.text({type: () => true})`). The command
//!   runs in a Node `vm` sandbox; a returned Promise is awaited.
//! - Response: **always HTTP 200**, plain text. Intermediate `print()`
//!   output arrives as its own lines; the final line is the result —
//!   `util.inspect(result)` for non-strings (`100`, `null`, `undefined`,
//!   `'quoted'`) or the raw string itself.
//! - Errors are in-band: the body is `Error: <err.stack>`. NOTE the
//!   stack includes the *command source line* (vm SyntaxError frames),
//!   so error bodies can echo credentials — every display/log path here
//!   passes through [`mask_cli_command`].
//!
//! ## Pinned bootstrap mechanisms (sources read from the live container)
//!
//! - `system.resetAllData()` (screepsmod-mongo `lib/common/_connect.js`):
//!   drops all mongo collections, re-imports `db.original.json` (an
//!   11×11 map, W0N0–W10N10, with 4 NPC bot players in the corners) and
//!   `env.flushall()`s redis. **No restart is needed, but redis loses
//!   `MAIN_LOOP_MIN_DURATION`** — the sim runs unthrottled until the
//!   tick duration is re-applied (verified live: `getTickDuration()`
//!   returns `null` after a reset).
//! - User creation (screepsmod-auth `lib/register.js`):
//!   `POST /api/register/submit` `{username, password}` → `{ok:1}`;
//!   creates the user + empty code + memory. Registration is open
//!   unless the `SERVER_PASSWORD` env var is set on the server. The CLI
//!   `setPassword(user, pass)` (screepsmod-auth `lib/cli.js`) is an
//!   **update only** — it silently does nothing for a missing user, so
//!   bootstrap registers first and uses `setPassword` only to converge
//!   an existing user's password.
//! - Sign-in (screepsmod-auth `lib/backend.js`): `POST /api/auth/signin`
//!   `{email: <username>, password}` → `{ok:1, token}`; subsequent calls
//!   send `X-Token`/`X-Username` and must adopt the refreshed token from
//!   the response's `X-Token` header.
//! - Spawn placement (`@screeps/backend lib/game/api/game.js`):
//!   `POST /api/game/place-spawn` (token auth) `{room, x, y, name}`.
//!   Validations: x,y in 1..=48, terrain not wall, no exit object within
//!   1 tile, room controller exists/unowned/unreserved, user owns zero
//!   objects, user not blocked and has cpu. Success claims the
//!   controller (level 1 + safe mode) and returns `{ok:1, newbie:true}`.

use crate::config::{EvalConfig, ServerEndpoint, SpawnPreference};
use anyhow::{anyhow, bail, Context, Result};
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::time::{Duration, Instant};

/// Default name for the first spawn.
pub const SPAWN_NAME: &str = "Spawn1";

// ===================================================================
// payload masking (P0.A7(c))
// ===================================================================

/// Mask credential-bearing `setPassword(...)` payloads in any text that
/// is about to be displayed, logged, or embedded in an error — commands
/// AND response bodies (vm stack traces echo the offending source line).
///
/// `setPassword("user", "pw")` becomes `setPassword("user", "***")`;
/// if the arguments cannot be parsed cleanly (unbalanced/truncated),
/// the whole argument list is masked to `(***)`.
pub fn mask_cli_command(text: &str) -> String {
    const NEEDLE: &str = "setPassword";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find(NEEDLE) {
        let after = idx + NEEDLE.len();
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        // Optional whitespace, then an opening paren — otherwise it is
        // just a mention of the name, not a call.
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('(') {
            continue;
        }
        let ws_len = rest.len() - trimmed.len();
        match scan_call_args(&trimmed[1..]) {
            Some((first_arg, close_rel)) => {
                out.push('(');
                match first_arg {
                    Some(arg) => {
                        out.push_str(arg.trim());
                        out.push_str(", \"***\")");
                    }
                    // Zero or one argument: nothing worth keeping.
                    None => out.push_str("\"***\")"),
                }
                rest = &trimmed[1 + close_rel + 1..];
                let _ = ws_len; // whitespace before '(' is dropped, fine
            }
            None => {
                // No balanced close paren (truncated text): mask to end.
                out.push_str("(***");
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Scan a JS argument list (starting just *after* the opening paren).
/// Returns `(first_arg_source, index_of_closing_paren)` where
/// `first_arg_source` is `Some` only when a comma separates a clean
/// first argument from the rest. Quote- and nesting-aware.
fn scan_call_args(s: &str) -> Option<(Option<&str>, usize)> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut first_comma: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => quote = Some(b),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    if b == b')' {
                        let first = first_comma.map(|c| &s[..c]);
                        return Some((first, i));
                    }
                    return None; // mismatched close
                }
                depth -= 1;
            }
            b',' if depth == 0 && first_comma.is_none() => first_comma = Some(i),
            _ => {}
        }
    }
    None
}

// ===================================================================
// command builders (pure)
// ===================================================================

/// JS string literal with full escaping (via the JSON encoding, which
/// is valid JS).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

pub const CMD_RESET_ALL_DATA: &str = "system.resetAllData()";
pub const CMD_GET_TICK_DURATION: &str = "system.getTickDuration()";
pub const CMD_PAUSE: &str = "system.pauseSimulation()";
pub const CMD_RESUME: &str = "system.resumeSimulation()";

pub fn cmd_set_tick_duration(ms: u64) -> String {
    format!("system.setTickDuration({ms})")
}

/// The credential-bearing command (P0.A7(c)): the composed string holds
/// the real password post-`expose_secret()` — it must reach ONLY the
/// HTTP request body; every other sink goes through [`mask_cli_command`].
pub fn cmd_set_password(username: &str, password: &str) -> String {
    format!("setPassword({}, {})", js_str(username), js_str(password))
}

pub fn cmd_respawn_user(username: &str) -> String {
    format!("utils.respawnUser({})", js_str(username))
}

/// Rooms a first spawn can go in: status `normal`, exactly one
/// controller that is unowned/unreserved/unbound, and ≥ 2 sources.
/// Returns a JSON array of room names.
pub fn cmd_candidate_rooms() -> String {
    "Promise.all([storage.db['rooms'].find({status: 'normal'}), \
     storage.db['rooms.objects'].find({type: {$in: ['controller','source']}})])\
     .then(([rooms, objs]) => { const info = {}; \
     for (const o of objs) { \
     const i = info[o.room] = info[o.room] || {c: 0, owned: 0, s: 0}; \
     if (o.type === 'controller') { i.c++; if (o.user || o.reservation || o.bindUser) i.owned++; } \
     else { i.s++; } } \
     const ok = rooms.map(r => r._id).filter(id => { const i = info[id]; \
     return i && i.c === 1 && i.owned === 0 && i.s >= 2; }); \
     return JSON.stringify(ok); })"
        .to_string()
}

/// Terrain string + blocking points of interest for one room, as JSON
/// (`{"terrain": "...2500 chars...", "objects": [{type,x,y}, ...]}`).
pub fn cmd_room_snapshot(room: &str) -> String {
    let r = js_str(room);
    format!(
        "Promise.all([storage.db['rooms.terrain'].findOne({{room: {r}}}), \
         storage.db['rooms.objects'].find({{$and: [{{room: {r}}}, \
         {{type: {{$in: ['source','controller','mineral']}}}}]}})])\
         .then(([t, objs]) => JSON.stringify({{terrain: t && t.terrain, \
         objects: objs.map(o => ({{type: o.type, x: o.x, y: o.y}}))}}))"
    )
}

// ===================================================================
// response parsing (pure)
// ===================================================================

/// In-band CLI error? (HTTP status is always 200; errors are bodies
/// starting with `Error: ` — see the module docs.)
pub fn is_cli_error(body: &str) -> bool {
    body.trim_start().starts_with("Error:")
}

/// `system.getTickDuration()` → `Some(ms)`, or `None` when the server
/// has no value (redis wiped by a reset → the body is `null`).
pub fn parse_tick_duration(body: &str) -> Result<Option<u64>> {
    let t = body.trim();
    if t == "null" || t == "undefined" {
        return Ok(None);
    }
    t.parse::<u64>()
        .map(Some)
        .with_context(|| format!("unexpected getTickDuration response: {t:?}"))
}

pub fn parse_candidate_rooms(body: &str) -> Result<Vec<String>> {
    serde_json::from_str(body.trim()).context("parsing candidate-room list from the server CLI")
}

#[derive(Debug, Deserialize)]
pub struct RoomObjectPos {
    #[serde(rename = "type")]
    pub kind: String,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Deserialize)]
pub struct RoomSnapshot {
    /// 2500-char terrain string, row-major (`terrain[y*50+x]`):
    /// `'0'` plain, `'1'` wall, `'2'` swamp, `'3'` swamp+wall.
    pub terrain: Option<String>,
    pub objects: Vec<RoomObjectPos>,
}

pub fn parse_room_snapshot(body: &str) -> Result<RoomSnapshot> {
    serde_json::from_str(body.trim()).context("parsing room snapshot from the server CLI")
}

// ===================================================================
// spawn-tile picking (pure)
// ===================================================================

/// Rank buildable tiles for the first spawn, best first.
///
/// A tile qualifies when: coordinates in 4..=45 (clear of the 1..=48
/// hard limit and the near-exit rules), the tile and all 8 neighbors
/// are non-wall, and it is ≥ 2 (Chebyshev) from every source /
/// controller / mineral (don't block harvest spots). Plain beats swamp;
/// within a tier, closest to the centroid of the room's points of
/// interest wins (ties: lower y, then lower x — deterministic).
pub fn pick_spawn_tiles(snapshot: &RoomSnapshot, max: usize) -> Result<Vec<(u32, u32)>> {
    let terrain = snapshot
        .terrain
        .as_deref()
        .context("room has no terrain data")?;
    let t = terrain.as_bytes();
    if t.len() != 2500 {
        bail!("terrain string is {} chars, expected 2500", t.len());
    }
    let tile = |x: u32, y: u32| -> u8 { t[(y * 50 + x) as usize] };
    let is_wall = |x: u32, y: u32| -> bool { !matches!(tile(x, y), b'0' | b'2') };

    let pois: Vec<(u32, u32)> = snapshot.objects.iter().map(|o| (o.x, o.y)).collect();
    let (cx, cy) = if pois.is_empty() {
        (25.0, 25.0)
    } else {
        let n = pois.len() as f64;
        (
            pois.iter().map(|p| p.0 as f64).sum::<f64>() / n,
            pois.iter().map(|p| p.1 as f64).sum::<f64>() / n,
        )
    };

    let mut scored: Vec<(f64, u32, u32)> = Vec::new();
    for y in 4..=45u32 {
        for x in 4..=45u32 {
            if is_wall(x, y) {
                continue;
            }
            let neighbors_open = (-1i64..=1).all(|dy| {
                (-1i64..=1).all(|dx| {
                    let (nx, ny) = ((x as i64 + dx) as u32, (y as i64 + dy) as u32);
                    !is_wall(nx, ny)
                })
            });
            if !neighbors_open {
                continue;
            }
            let too_close = pois
                .iter()
                .any(|&(px, py)| px.abs_diff(x).max(py.abs_diff(y)) < 2);
            if too_close {
                continue;
            }
            let swamp_penalty = if tile(x, y) == b'2' { 1000.0 } else { 0.0 };
            let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            scored.push((d + swamp_penalty, x, y));
        }
    }
    scored.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
            .then(a.1.cmp(&b.1))
    });
    Ok(scored
        .into_iter()
        .take(max)
        .map(|(_, x, y)| (x, y))
        .collect())
}

/// Order candidate rooms deterministically: closest to the centroid of
/// all candidates first (central rooms have more expansion neighbors),
/// ties broken by name. Self-contained — no world-size query needed.
pub fn sort_rooms_for_spawn(mut rooms: Vec<String>) -> Vec<String> {
    let coords: Vec<Option<(f64, f64)>> = rooms.iter().map(|r| parse_room_name(r)).collect();
    let known: Vec<(f64, f64)> = coords.iter().flatten().copied().collect();
    if known.is_empty() {
        rooms.sort();
        return rooms;
    }
    let n = known.len() as f64;
    let cx = known.iter().map(|c| c.0).sum::<f64>() / n;
    let cy = known.iter().map(|c| c.1).sum::<f64>() / n;
    let mut keyed: Vec<(f64, String)> = rooms
        .drain(..)
        .map(|r| {
            let d = parse_room_name(&r)
                .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
                .unwrap_or(f64::MAX);
            (d, r)
        })
        .collect();
    keyed.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    keyed.into_iter().map(|(_, r)| r).collect()
}

/// `W5N3` → world coordinates on the standard axes (`W{n}` → `-1-n`,
/// `E{n}` → `n`, `S{n}` → `-1-n`, `N{n}` → `n`).
fn parse_room_name(name: &str) -> Option<(f64, f64)> {
    let rest = name.trim();
    let (h, rest) = rest.split_at_checked(1)?;
    let split = rest.find(['N', 'S'])?;
    let (xs, vrest) = rest.split_at(split);
    let (v, ys) = vrest.split_at_checked(1)?;
    let x: i64 = xs.parse().ok()?;
    let y: i64 = ys.parse().ok()?;
    let wx = match h {
        "E" => x,
        "W" => -1 - x,
        _ => return None,
    };
    let wy = match v {
        "N" => y,
        "S" => -1 - y,
        _ => return None,
    };
    Some((wx as f64, wy as f64))
}

// ===================================================================
// CLI client (HTTP, port `eval.ports.cli`)
// ===================================================================

pub struct CliClient {
    base: String,
    http: reqwest::Client,
}

impl CliClient {
    pub fn new(cli_port: u16) -> Result<Self> {
        Ok(CliClient {
            base: format!("http://127.0.0.1:{cli_port}"),
            http: reqwest::Client::builder()
                // resetAllData re-imports the whole seed DB; be generous.
                .timeout(Duration::from_secs(60))
                .build()
                .context("building HTTP client")?,
        })
    }

    /// `GET /greeting` — the banner (also the cheapest liveness probe).
    pub async fn greeting(&self) -> Result<String> {
        let resp = self
            .http
            .get(format!("{}/greeting", self.base))
            .send()
            .await
            .with_context(|| {
                format!(
                    "server CLI not reachable at {} — `server up` first",
                    self.base
                )
            })?;
        Ok(resp.text().await?.trim_end().to_string())
    }

    /// Send a command, return the raw response body (trailing newline
    /// trimmed). In-band `Error:` bodies are returned as `Ok` — this is
    /// the REPL/passthrough primitive; callers display the body as-is
    /// (after masking). Transport failures are `Err` with the command
    /// MASKED in the context.
    pub async fn send_raw(&self, command: &str) -> Result<String> {
        let resp = self
            .http
            .post(format!("{}/cli", self.base))
            .body(command.to_string())
            .send()
            .await
            .with_context(|| {
                format!(
                    "sending to server CLI at {}: {}",
                    self.base,
                    mask_cli_command(command)
                )
            })?;
        Ok(resp.text().await?.trim_end().to_string())
    }

    /// Send a command and treat an in-band `Error:` body as `Err` — the
    /// automation primitive (bootstrap, tick control). Both the command
    /// and the body are masked in the error (vm stack traces echo the
    /// command source, which may carry credentials).
    pub async fn send(&self, command: &str) -> Result<String> {
        let body = self.send_raw(command).await?;
        if is_cli_error(&body) {
            bail!(
                "server CLI command failed: {}\n{}",
                mask_cli_command(command),
                mask_cli_command(&body)
            );
        }
        Ok(body)
    }
}

// ===================================================================
// tick control (P0.A8)
// ===================================================================

/// Set the tick duration and confirm by reading it back.
pub async fn set_tick_ms(cli: &CliClient, ms: u64) -> Result<u64> {
    cli.send(&cmd_set_tick_duration(ms)).await?;
    let read_back = parse_tick_duration(&cli.send(CMD_GET_TICK_DURATION).await?)?
        .context("getTickDuration returned null right after setTickDuration")?;
    if read_back != ms {
        bail!("set tick duration {ms} ms but the server reads back {read_back} ms");
    }
    Ok(read_back)
}

pub async fn get_tick_ms(cli: &CliClient) -> Result<Option<u64>> {
    parse_tick_duration(&cli.send(CMD_GET_TICK_DURATION).await?)
}

pub async fn pause(cli: &CliClient) -> Result<()> {
    cli.send(CMD_PAUSE).await?;
    Ok(())
}

pub async fn resume(cli: &CliClient) -> Result<()> {
    cli.send(CMD_RESUME).await?;
    Ok(())
}

// ===================================================================
// game-API client (bootstrap-scoped; the full client is P0.A4/A5)
// ===================================================================

#[derive(Debug, Deserialize)]
struct ApiOkResponse {
    ok: Option<i64>,
    error: Option<String>,
    token: Option<String>,
    status: Option<String>,
    time: Option<u64>,
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

    /// Sign in with the configured credentials; stores the token.
    /// Proves the password works end-to-end (the bootstrap verification).
    pub async fn signin(&mut self, server: &ServerEndpoint) -> Result<()> {
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
            (Some(1), Some(token)) => {
                self.token = Some(token);
                Ok(())
            }
            _ => bail!(
                "signin as '{}' failed: {}",
                server.username,
                parsed.error.as_deref().unwrap_or("no token in response")
            ),
        }
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

// ===================================================================
// bootstrap (P0.A3)
// ===================================================================

#[derive(Debug)]
pub enum SpawnOutcome {
    AlreadyPresent,
    Placed { room: String, x: u32, y: u32 },
}

#[derive(Debug)]
pub struct BootstrapOutcome {
    pub reset: bool,
    pub tick_ms: u64,
    pub user_created: bool,
    pub spawn: SpawnOutcome,
    pub world_status: String,
}

impl std::fmt::Display for BootstrapOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "world:  {}",
            if self.reset {
                "reset (fresh)"
            } else {
                "kept (no --reset)"
            }
        )?;
        writeln!(f, "tick:   {} ms (read back from the server)", self.tick_ms)?;
        writeln!(
            f,
            "user:   {}",
            if self.user_created {
                "created (registered fresh)"
            } else {
                "existing (password converged via setPassword)"
            }
        )?;
        match &self.spawn {
            SpawnOutcome::AlreadyPresent => writeln!(f, "spawn:  already present")?,
            SpawnOutcome::Placed { room, x, y } => {
                writeln!(f, "spawn:  {SPAWN_NAME} placed @ {room} ({x},{y})")?
            }
        }
        write!(f, "status: {}", self.world_status)
    }
}

/// Reset/initialize the world to match `.screeps.yaml`:
/// server up → (optional) `system.resetAllData()` + settle → tick rate
/// re-applied (a reset wipes it — see module docs) → user registered or
/// password converged → signed in (credential verification) → spawn
/// placed (config preference or auto-pick) → world status verified.
pub async fn bootstrap(cfg: &EvalConfig, reset: bool) -> Result<BootstrapOutcome> {
    // 1. Ensure the stack is up (idempotent; waits for the game API).
    crate::docker::up(&cfg.eval).await?;
    let cli = CliClient::new(cfg.eval.cli_port)?;
    let mut api = GameApi::new(&cfg.server)?;
    cli.greeting().await?; // fail fast if the CLI port is dead

    // 2. Optional full wipe.
    if reset {
        tracing::info!("system.resetAllData() — wiping the world (mongo re-seed + redis flush)");
        cli.send(CMD_RESET_ALL_DATA).await?;
        wait_for_settle(&cli, &api).await?;
    }

    // 3. Tick duration. ALWAYS applied: a reset leaves the loop
    //    unthrottled (verified live), and bootstrap's contract is
    //    "world matches config".
    let tick_ms = set_tick_ms(&cli, cfg.eval.tick_ms).await?;
    tracing::info!(tick_ms, "tick duration applied and read back");

    // 4. Ensure the bot user exists with the configured password.
    let username = &cfg.server.username;
    let user_created = if api.username_available(username).await? {
        api.register(&cfg.server).await?;
        tracing::info!(user = %username, "user registered (with the configured password)");
        true
    } else {
        // Exists (possibly with an unknown password) — converge it.
        // P0.A7(c): the composed payload carries the real password;
        // it goes ONLY into the request body. Log the masked form.
        let cmd = cmd_set_password(username, cfg.server.password.expose_secret());
        cli.send(&cmd).await?;
        tracing::info!(
            command = %mask_cli_command(&cmd),
            "existing user — password converged"
        );
        false
    };

    // 5. Sign in — proves the configured credentials work.
    api.signin(&cfg.server).await?;
    tracing::info!(user = %username, "signin OK (token acquired)");

    // 6. Spawn placement.
    let mut status = api.world_status().await?;
    if status == "lost" {
        // Wiped out: clear the remains, then place anew.
        tracing::info!("world status 'lost' — respawning the user before placement");
        cli.send(&cmd_respawn_user(username)).await?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            status = api.world_status().await?;
            if status == "empty" {
                break;
            }
            if Instant::now() > deadline {
                bail!("user respawn did not settle (world status stuck at {status:?})");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    let spawn = if status == "normal" {
        tracing::info!("spawn already present (world status 'normal')");
        SpawnOutcome::AlreadyPresent
    } else {
        let placed = place_first_spawn(&cli, &mut api, &cfg.eval.spawn).await?;
        tracing::info!(room = %placed.0, x = placed.1, y = placed.2, "spawn placed");
        SpawnOutcome::Placed {
            room: placed.0,
            x: placed.1,
            y: placed.2,
        }
    };

    // 7. Verify.
    let world_status = api.world_status().await?;
    if world_status != "normal" {
        bail!("bootstrap finished but world status is {world_status:?}, expected \"normal\"");
    }
    Ok(BootstrapOutcome {
        reset,
        tick_ms,
        user_created,
        spawn,
        world_status,
    })
}

/// After `resetAllData` the API keeps answering but the world re-seeds;
/// wait until both the game API and the CLI respond sensibly again.
async fn wait_for_settle(cli: &CliClient, api: &GameApi) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let api_ok = api.game_time().await.is_ok();
        let cli_ok = cli.send_raw(CMD_GET_TICK_DURATION).await.is_ok();
        if api_ok && cli_ok {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("server did not settle within 60 s after resetAllData");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Place the first spawn per the `eval.spawn` preference:
/// - room+x+y → exactly there (no fallback; explicit config wins or fails),
/// - room only → auto-pick a tile in that room,
/// - nothing → auto-pick room (central candidates first) and tile.
async fn place_first_spawn(
    cli: &CliClient,
    api: &mut GameApi,
    pref: &SpawnPreference,
) -> Result<(String, u32, u32)> {
    match (&pref.room, pref.x, pref.y) {
        (Some(room), Some(x), Some(y)) => {
            api.place_spawn(room, x, y, SPAWN_NAME)
                .await
                .with_context(|| {
                    format!("explicit eval.spawn placement at {room} ({x},{y}) failed")
                })?;
            return Ok((room.clone(), x, y));
        }
        (None, x, y) if x.is_some() || y.is_some() => {
            bail!("eval.spawn.x/y are only honored together with eval.spawn.room");
        }
        (Some(_), x, y) if x.is_some() != y.is_some() => {
            bail!("eval.spawn needs both x and y (or neither, for auto-pick)");
        }
        _ => {}
    }

    let candidates = parse_candidate_rooms(&cli.send(&cmd_candidate_rooms()).await?)?;
    let rooms: Vec<String> = match &pref.room {
        Some(room) => {
            if !candidates.contains(room) {
                bail!(
                    "eval.spawn.room {room} is not a valid first-spawn room \
                     (needs an unowned controller + ≥2 sources); candidates: {}",
                    candidates.join(", ")
                );
            }
            vec![room.clone()]
        }
        None => sort_rooms_for_spawn(candidates),
    };
    if rooms.is_empty() {
        bail!("no candidate rooms for a first spawn (is the world seeded?)");
    }

    let mut last_err: Option<anyhow::Error> = None;
    for room in &rooms {
        let snapshot = parse_room_snapshot(&cli.send(&cmd_room_snapshot(room)).await?)?;
        let tiles = pick_spawn_tiles(&snapshot, 8)?;
        if tiles.is_empty() {
            tracing::debug!(room = %room, "no buildable tile found, trying next room");
            continue;
        }
        for (x, y) in tiles {
            match api.place_spawn(room, x, y, SPAWN_NAME).await {
                Ok(()) => return Ok((room.clone(), x, y)),
                Err(e) => {
                    tracing::debug!(room = %room, x, y, "placement rejected: {e:#}");
                    last_err = Some(e);
                }
            }
        }
    }
    Err(last_err
        .map(|e| e.context("every candidate placement was rejected"))
        .unwrap_or_else(|| anyhow!("no buildable spawn tile in any candidate room")))
}

// ===================================================================
// tests — pure parts only, against literals (live behavior is verified
// end-to-end by the operator flow, not unit-mocked)
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PW: &str = "super-secret-test-pw-7391";

    // ---------------- masking (the P0.A7(c) pin) ----------------

    /// THE pin: a composed setPassword payload, displayed after
    /// masking, contains no secret material.
    #[test]
    fn mask_pin_setpassword_payload_is_masked_in_display() {
        let payload = cmd_set_password("ibex", FAKE_PW);
        assert!(payload.contains(FAKE_PW), "fixture must carry the secret");
        let display = mask_cli_command(&payload);
        assert!(
            !display.contains(FAKE_PW),
            "secret leaked through the mask: {display}"
        );
        assert_eq!(display, r#"setPassword("ibex", "***")"#);
    }

    #[test]
    fn mask_keeps_non_credential_commands_verbatim() {
        for cmd in [
            "system.getTickDuration()",
            "storage.db['users'].count()",
            "help()",
        ] {
            assert_eq!(mask_cli_command(cmd), cmd);
        }
    }

    /// Tricky payloads: quotes/parens/commas inside the password, and
    /// passwords with escape sequences.
    #[test]
    fn mask_handles_hostile_password_content() {
        for pw in [r#"pa)ss,word"#, r#"pa"ss"#, r#"pa\"s)s"#, "pa'ss`"] {
            let payload = cmd_set_password("ibex", pw);
            let display = mask_cli_command(&payload);
            assert!(
                !display.contains("pa") || !display.contains("ss"),
                "password fragments leaked for {pw:?}: {display}"
            );
            assert!(display.contains("***"), "no mask marker: {display}");
        }
    }

    #[test]
    fn mask_truncated_call_masks_to_end() {
        let truncated = r#"setPassword("ibex", "super-sec"#;
        let display = mask_cli_command(truncated);
        assert!(!display.contains("super-sec"), "leak: {display}");
        assert_eq!(display, "setPassword(***");
    }

    /// vm stack traces echo the command source line — response bodies
    /// must be maskable too (this is why `send` masks the body).
    #[test]
    fn mask_works_inside_error_stack_text() {
        let body = format!(
            "Error: evalmachine.<anonymous>:1\nsetPassword(\"ibex\", \"{FAKE_PW}\")\n^\n\nSyntaxError: ..."
        );
        let display = mask_cli_command(&body);
        assert!(!display.contains(FAKE_PW), "leak: {display}");
        assert!(display.contains("Error: evalmachine"));
    }

    #[test]
    fn mask_handles_multiple_occurrences() {
        let text =
            format!("setPassword(\"a\", \"{FAKE_PW}\") and then setPassword(\"b\", \"{FAKE_PW}\")");
        let display = mask_cli_command(&text);
        assert!(!display.contains(FAKE_PW));
        assert_eq!(display.matches("***").count(), 2);
    }

    #[test]
    fn mask_zero_arg_usage_is_harmless() {
        // The mod's own usage text mentions the name without secrets.
        let display = mask_cli_command("Usage: setPassword(username, password)");
        assert!(display.starts_with("Usage: setPassword(username, \"***\")"));
    }

    // ---------------- command builders ----------------

    #[test]
    fn set_password_builder_escapes_js_strings() {
        let cmd = cmd_set_password(r#"we"ird"#, r#"p"w\"#);
        assert_eq!(cmd, r#"setPassword("we\"ird", "p\"w\\")"#);
    }

    #[test]
    fn tick_and_respawn_builders() {
        assert_eq!(cmd_set_tick_duration(150), "system.setTickDuration(150)");
        assert_eq!(cmd_respawn_user("ibex"), r#"utils.respawnUser("ibex")"#);
    }

    // ---------------- response parsing ----------------

    /// Literal response shapes captured from the live server (2026-06-09).
    #[test]
    fn parses_tick_duration_responses() {
        assert_eq!(parse_tick_duration("100\n").unwrap(), Some(100));
        assert_eq!(parse_tick_duration("null").unwrap(), None); // post-reset
        assert_eq!(parse_tick_duration("undefined").unwrap(), None);
        assert!(parse_tick_duration("OK").is_err());
    }

    #[test]
    fn detects_in_band_cli_errors() {
        // Live shape: HTTP 200, body starts `Error: ` + node vm stack.
        let body = "Error: evalmachine.<anonymous>:1\nundefinedFn()\n^\n\nReferenceError: ...";
        assert!(is_cli_error(body));
        assert!(!is_cli_error("100"));
        assert!(!is_cli_error("OK"));
        assert!(!is_cli_error("The supported commands are: ..."));
    }

    #[test]
    fn parses_candidate_rooms_from_live_shape() {
        let body = "[\"W9N8\",\"W5N3\",\"W5N8\"]\n";
        assert_eq!(
            parse_candidate_rooms(body).unwrap(),
            vec!["W9N8", "W5N3", "W5N8"]
        );
    }

    #[test]
    fn parses_room_snapshot_from_live_shape() {
        let body = format!(
            "{{\"terrain\":\"{}\",\"objects\":[{{\"type\":\"source\",\"x\":12,\"y\":10}},{{\"type\":\"controller\",\"x\":20,\"y\":20}}]}}\n",
            "0".repeat(2500)
        );
        let snap = parse_room_snapshot(&body).unwrap();
        assert_eq!(snap.terrain.as_ref().unwrap().len(), 2500);
        assert_eq!(snap.objects.len(), 2);
        assert_eq!(snap.objects[0].kind, "source");
    }

    // ---------------- spawn-tile picking ----------------

    /// 50x50 terrain from rows of chars; rows[y] addressed as [x].
    fn terrain_all(fill: char) -> String {
        std::iter::repeat_n(fill, 2500).collect()
    }

    fn set_tile(terrain: &mut String, x: u32, y: u32, c: char) {
        let i = (y * 50 + x) as usize;
        terrain.replace_range(i..i + 1, &c.to_string());
    }

    fn snapshot(terrain: String, objects: Vec<(&str, u32, u32)>) -> RoomSnapshot {
        RoomSnapshot {
            terrain: Some(terrain),
            objects: objects
                .into_iter()
                .map(|(kind, x, y)| RoomObjectPos {
                    kind: kind.to_string(),
                    x,
                    y,
                })
                .collect(),
        }
    }

    #[test]
    fn picks_tile_near_poi_centroid_on_open_terrain() {
        let snap = snapshot(
            terrain_all('0'),
            vec![
                ("source", 10, 10),
                ("source", 20, 10),
                ("controller", 15, 20),
            ],
        );
        let tiles = pick_spawn_tiles(&snap, 5).unwrap();
        assert!(!tiles.is_empty());
        let (x, y) = tiles[0];
        // Centroid is (15, 13.33); the pick must be close and ≥2 from POIs.
        assert!((13..=17).contains(&x), "x={x}");
        assert!((11..=16).contains(&y), "y={y}");
        for &(px, py) in &[(10u32, 10u32), (20, 10), (15, 20)] {
            assert!(px.abs_diff(x).max(py.abs_diff(y)) >= 2);
        }
    }

    #[test]
    fn never_picks_walls_or_wall_adjacent_tiles() {
        let mut terrain = terrain_all('1'); // all wall ...
        for y in 24..=28 {
            for x in 24..=28 {
                set_tile(&mut terrain, x, y, '0'); // ... except a 5x5 island
            }
        }
        let snap = snapshot(terrain, vec![("controller", 5, 5)]);
        let tiles = pick_spawn_tiles(&snap, 25).unwrap();
        // Only the island's 3x3 interior has all-8 open neighbors; the
        // border ring (x or y = 24/28) touches wall and must be absent.
        assert_eq!(tiles.len(), 9);
        assert!(tiles
            .iter()
            .all(|&(x, y)| (25..=27).contains(&x) && (25..=27).contains(&y)));
        // Closest to the POI at (5,5) comes first.
        assert_eq!(tiles[0], (25, 25));
    }

    #[test]
    fn prefers_plain_over_swamp() {
        let mut terrain = terrain_all('2'); // all swamp...
        set_tile(&mut terrain, 30, 30, '0'); // ...one plain tile, far from centroid
        let snap = snapshot(terrain, vec![("controller", 10, 10)]);
        let tiles = pick_spawn_tiles(&snap, 3).unwrap();
        assert_eq!(tiles[0], (30, 30), "plain must beat closer swamp");
    }

    #[test]
    fn respects_the_4_to_45_margin() {
        let snap = snapshot(terrain_all('0'), vec![("controller", 0, 0)]);
        let tiles = pick_spawn_tiles(&snap, 2500).unwrap();
        assert!(tiles
            .iter()
            .all(|&(x, y)| (4..=45).contains(&x) && (4..=45).contains(&y)));
    }

    #[test]
    fn bad_terrain_is_a_clear_error() {
        let snap = RoomSnapshot {
            terrain: Some("0123".into()),
            objects: vec![],
        };
        assert!(pick_spawn_tiles(&snap, 5).is_err());
        let none = RoomSnapshot {
            terrain: None,
            objects: vec![],
        };
        assert!(pick_spawn_tiles(&none, 5).is_err());
    }

    // ---------------- room ordering ----------------

    #[test]
    fn sorts_rooms_centrally_with_deterministic_ties() {
        // Live candidate subset (default 11x11 map, W0N0..W10N10).
        let rooms = vec![
            "W9N8".to_string(),
            "W1N1".to_string(),
            "W5N3".to_string(),
            "W5N8".to_string(),
            "W3N4".to_string(),
        ];
        let sorted = sort_rooms_for_spawn(rooms);
        // Centroid of those five is around W4.6 N4.8 — W3N4/W5N3 lead,
        // corner-ish W9N8/W1N1 trail.
        assert_eq!(sorted.len(), 5);
        assert!(sorted.ends_with(&["W9N8".to_string()]) || sorted.ends_with(&["W1N1".to_string()]));
        let first = &sorted[0];
        assert!(first == "W3N4" || first == "W5N3", "got {first}");
        // Determinism.
        let again = sort_rooms_for_spawn(vec![
            "W5N8".into(),
            "W3N4".into(),
            "W9N8".into(),
            "W5N3".into(),
            "W1N1".into(),
        ]);
        assert_eq!(sorted, again);
    }

    #[test]
    fn parses_room_names_on_all_axes() {
        assert_eq!(parse_room_name("E5N3"), Some((5.0, 3.0)));
        assert_eq!(parse_room_name("W5N3"), Some((-6.0, 3.0)));
        assert_eq!(parse_room_name("W0S0"), Some((-1.0, -1.0)));
        assert_eq!(parse_room_name("sim"), None);
        assert_eq!(parse_room_name(""), None);
    }
}
