//! Typed response shapes for the unified endpoint set — every shape is
//! pinned to a cited source and fixture-tested here (no network in
//! tests). Tolerant by design: no `deny_unknown_fields` anywhere, so
//! extra fields the servers add never break parsing.
//!
//! Shape sources (the catalogue both consumers were pinned from):
//! - `screeps_api.py` — [Qionglu735/screeps_tool], the operator-referenced
//!   reference client: auth/signin body + token headers, world-size,
//!   map-stats statName values, room-terrain, room-status, world-status,
//!   place-spawn body, respawn.
//! - `screepsapi.py` — [screepers/python-screeps]: shard parameter
//!   placement (query for GETs, body key for POSTs), room-objects,
//!   token-rotation acceptance rule (`len >= 40`).
//! - `Endpoints.md` — [screepers/node-screeps-api] (vendored copy:
//!   `node_modules/screeps-api/docs/Endpoints.md`): response shapes
//!   (terrain digit legend, map-stats stats/users maps, signin response,
//!   `GET /api/game/time` -> `{ok, time}`), token rotation via the
//!   `X-Token` response header.
//! - `node_modules/screeps-api/dist/ScreepsAPI.js` (READ-ONLY, the exact
//!   client js_tools/deploy.js uses): memory-segment get/set (:984/:994),
//!   code get/set (:883/:894), the official per-endpoint rate-limit
//!   table (:1417-1437).
//! - Live private server 2026-06-09 (screeps-eval P0.A3/A5 bring-up,
//!   container sources read on the box): signin/register
//!   (screepsmod-auth `lib/backend.js`/`lib/register.js`), `/api/auth/me`
//!   `_id`, memory-segment `data: string|null` and the **absence of a
//!   shard param on private servers** (`@screeps/backend
//!   lib/game/api/user.js:318-327`), place-spawn validation
//!   (`@screeps/backend lib/game/api/game.js`).
//! - <https://docs.screeps.com/auth-tokens.html> — official-server rate
//!   limits (global 120/min; per-endpoint caps; HTTP 429 on excess).

use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;

/// Bare `{ok: 1}` acknowledgement (place-spawn, respawn, register,
/// memory-segment write, code upload).
#[derive(Debug, Deserialize)]
pub struct OkResponse {
    pub ok: i32,
}

/// `GET /api/game/time` response (`{ok, time}` — Endpoints.md "Other";
/// no auth required; verified live 2026-06-09).
#[derive(Debug, Deserialize)]
pub struct TimeResponse {
    pub ok: i32,
    pub time: u64,
}

/// `GET /api/auth/me` — the signed-in identity. `_id` is the Mongo user
/// id the console websocket channel is keyed by (`user:<id>/console`).
/// Extra fields (cpu, gcl, badge, ...) are tolerated. Source: live
/// private server 2026-06-09 + Endpoints.md.
#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo {
    #[serde(rename = "_id")]
    pub id: String,
    pub username: String,
}

/// `GET /api/user/memory-segment?segment=N[&shard=]` response —
/// `{ok, data: <string|null>}` (`null` = the segment has never been
/// written). Sources: node-screeps-api `memory.segment.get`
/// (ScreepsAPI.js:984), live private server 2026-06-09.
#[derive(Debug, Deserialize)]
pub struct MemorySegmentResponse {
    pub ok: i32,
    pub data: Option<String>,
}

/// `GET /api/user/world-status` response.
#[derive(Debug, Deserialize)]
pub struct WorldStatusResponse {
    pub ok: i32,
    /// `"empty"` (no spawn placed), `"normal"`, or `"lost"` (all spawns
    /// dead — respawn possible). Source: screeps_api.py + Endpoints.md.
    pub status: String,
}

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
    /// DEFAULTED: the live private server (screeps-launcher stack,
    /// recorded 2026-06-10) returns `{objects, users}` with NO `ok`
    /// field for this endpoint, while python-screeps documents `ok: 1`
    /// (screeps.com). Errors are classified by the `{"error": ...}`
    /// envelope before the typed parse, so `ok` carries no signal here.
    #[serde(default)]
    pub ok: i32,
    /// Lazily-typed: per-type fields vary wildly. Callers filter these
    /// down to whatever subset they consume.
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

/// `POST /api/auth/signin` response (Endpoints.md: `{ok, token}`).
/// The token is credential material — parsed straight into a
/// [`SecretString`] so Debug output redacts it. Crate-private: the
/// token only ever feeds the client's rolling auth state or a
/// [`SecretString`] handed to the websocket.
#[derive(Debug, Deserialize)]
pub(crate) struct SignInResponse {
    #[allow(dead_code)]
    pub ok: i32,
    pub token: SecretString,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::parse_api_response;

    /// Build a 2500-char encoded terrain string (the room-terrain
    /// `encoded=1` shape): 50x50 digits. "0123" repeated covers every
    /// legal digit value.
    fn terrain_2500() -> String {
        "0123".repeat(625)
    }

    /// RECORDED SHAPE: GET /api/game/time (live private server
    /// 2026-06-09; Endpoints.md).
    #[test]
    fn time_fixture_parses() {
        let parsed: TimeResponse =
            parse_api_response(200, r#"{"ok":1,"time":7435}"#, "game-time response").unwrap();
        assert_eq!(parsed.ok, 1);
        assert_eq!(parsed.time, 7435);
    }

    /// RECORDED SHAPE: GET /api/auth/me (live private server 2026-06-09)
    /// — extra fields like cpu/gcl must be tolerated.
    #[test]
    fn auth_me_fixture_parses() {
        let parsed: UserInfo = parse_api_response(
            200,
            r#"{"ok":1,"_id":"6a28d4d7d9592a0060be10ef","username":"Azaril","cpu":100,"gcl":0}"#,
            "auth/me response",
        )
        .unwrap();
        assert_eq!(parsed.id, "6a28d4d7d9592a0060be10ef");
        assert_eq!(parsed.username, "Azaril");
    }

    /// RECORDED SHAPES: GET /api/user/memory-segment — never-written
    /// (`data: null`) and with content (live private server 2026-06-09).
    #[test]
    fn memory_segment_fixtures_parse() {
        let parsed: MemorySegmentResponse =
            parse_api_response(200, r#"{"ok":1,"data":null}"#, "memory-segment response").unwrap();
        assert_eq!(parsed.ok, 1);
        assert_eq!(parsed.data, None);

        let parsed: MemorySegmentResponse = parse_api_response(
            200,
            r#"{"ok":1,"data":"{\"shard\":{}}"}"#,
            "memory-segment response",
        )
        .unwrap();
        assert_eq!(parsed.data.as_deref(), Some(r#"{"shard":{}}"#));
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
        assert_eq!(parsed.ok, 1);
        assert_eq!(parsed.objects.len(), 3);
        assert_eq!(parsed.objects[2]["mineralType"], "H");
    }

    /// RECORDED SHAPE: room-objects on the live private server
    /// (screeps-launcher stack, 2026-06-10) — NO `ok` field:
    /// `{"objects":[{"_id":"03a407734e2b07f","room":"W5N5","type":"source",...}],"users":{...}}`.
    /// Caught live by the first prospector `auto` run (P0.P4); `ok` is
    /// defaulted so both shapes parse.
    #[test]
    fn room_objects_without_ok_field_parses_live_private_server_shape() {
        let fixture = r#"{
            "objects": [
                {"_id": "03a407734e2b07f", "room": "W5N5", "type": "source", "x": 5, "y": 5, "energy": 4000, "energyCapacity": 4000, "ticksToRegeneration": 300, "nextRegenerationTime": null},
                {"_id": "834761653eb3bb2", "type": "mineral", "mineralType": "K", "mineralAmount": 50191.29600747557, "x": 42, "y": 32}
            ],
            "users": {}
        }"#;
        let parsed: RoomObjectsResponse =
            parse_api_response(200, fixture, "room-objects response").unwrap();
        assert_eq!(parsed.ok, 0, "absent ok defaults");
        assert_eq!(parsed.objects.len(), 2);
        assert_eq!(parsed.objects[1]["mineralType"], "K");
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
        let status: WorldStatusResponse =
            parse_api_response(200, r#"{"ok":1,"status":"normal"}"#, "wst").unwrap();
        assert_eq!(status.status, "normal");

        let ack: OkResponse = parse_api_response(200, r#"{"ok":1}"#, "ack").unwrap();
        assert_eq!(ack.ok, 1);
    }

    /// RECORDED SHAPE: place-spawn success carries an extra `newbie`
    /// field on private servers (`{ok:1, newbie:true}`, live 2026-06-09)
    /// — tolerated by the ack shape.
    #[test]
    fn place_spawn_ack_tolerates_newbie_field() {
        let ack: OkResponse =
            parse_api_response(200, r#"{"ok":1,"newbie":true}"#, "place-spawn response").unwrap();
        assert_eq!(ack.ok, 1);
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
