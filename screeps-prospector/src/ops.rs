//! The scan/fetch flows shared by the CLI and (later, P0.P4) the
//! `screeps-server-kit` bootstrap integration. Library-first: the CLI is a
//! thin wrapper over these.
//!
//! - [`scan_rooms`]: batched `map-stats` (statName `owner0`) over a room
//!   list → spawnability flags upserted into the cache.
//! - [`fetch_rooms`]: per-room terrain (skipped when already cached —
//!   terrain is immutable) + planner objects, plus a batched status
//!   refresh for rooms whose status passed its TTL.
//!
//! MMO QUOTA REALITIES (the operator's live `scan --all` failure):
//! screeps.com caps `POST /api/game/map-stats` at 60/hour and
//! `GET /api/game/room-terrain` at 360/hour (per-endpoint quotas on top
//! of the global 120/minute — table pinned in `screeps_rest_api::quota`
//! from node-screeps-api). The shared client paces those endpoints
//! proactively and resumes through 429s; this module's job is to make
//! the CALL COUNT small ([`MAP_STATS_CHUNK`] — a full 15k-room shard is
//! ~15 map-stats calls) and every run RESUMABLE (the cache is persisted
//! after every successful batch via `save_path`, and re-runs skip
//! work the cache already holds). [`plan_scan`]/[`plan_fetch`] give the
//! CLI the up-front call-count/ETA math.
//!
//! The pure status derivation ([`derive_room_status`]) is unit-tested
//! offline; the async flows only sequence pinned client calls.

use crate::cache::{
    filter_planner_objects, room_name_claimable, validate_terrain, CachedRoom, RoomCache,
    RoomStatus,
};
use anyhow::{bail, Context, Result};
use screeps_rest_api::{endpoint_quota, Client, EndpointQuota, RoomMapStats};
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Rooms per map-stats POST. The endpoint takes an ARRAY and is capped
/// at 60 calls/hour on screeps.com (ScreepsAPI.js:1432) — so the batch
/// size, not the pacing, decides whether a full-shard scan fits the
/// quota. Evidence for batching big: neither reference client chunks
/// the array (node-screeps-api `mapStats` posts it whole,
/// dist/ScreepsAPI.js:478-480; Qionglu735/screeps_tool `screeps_api.py`
/// `map_stats` likewise, checked 2026-06-10), and the open backend
/// accepts 8 MB JSON bodies (`@screeps/backend lib/game/server.js:165`,
/// read from the live eval container 2026-06-10) with unbounded `$in`
/// queries (`lib/game/api/game.js:186-260`). 1000 rooms/call keeps the
/// request ~11 KB, bounds the cost of an interrupted batch, and makes a
/// full 122x122 official shard (14 884 rooms) 15 calls — a quarter of
/// one hourly map-stats window.
pub const MAP_STATS_CHUNK: usize = 1000;

/// During `fetch`, persist the cache every N completed rooms (plus
/// after every status batch and at the end) — an interrupted fetch
/// loses at most this many rooms of work.
const FETCH_SAVE_EVERY: usize = 16;

#[derive(Debug, Default)]
pub struct ScanSummary {
    pub scanned: usize,
    pub open: usize,
    /// Of the open rooms, how many sit in an active respawn area
    /// (included in default selections — respawn-first workflow).
    pub respawn: usize,
    /// Of the open rooms, how many sit in an active novice area
    /// (excluded from default selections unless `--include-novice`).
    pub novice: usize,
    /// Rooms skipped because the cache already had a status fresher
    /// than the scan TTL (resume support).
    pub skipped_fresh: usize,
}

#[derive(Debug, Default)]
pub struct FetchSummary {
    pub fetched_terrain: usize,
    pub skipped_terrain: usize,
    pub fetched_objects: usize,
    pub refreshed_status: usize,
    /// Rooms skipped entirely: terrain AND planner objects already
    /// cached (both immutable — see [`room_is_fully_fetched`]).
    pub skipped_complete: usize,
}

/// Pure: map-stats entry → spawnability flags.
///
/// `open` is conservative: the room must exist (`status == "normal"`),
/// be unowned/unreserved, AND be claimable by name
/// ([`room_name_claimable`] — map-stats reports highways and
/// source-keeper rooms as `status: "normal"` too, verified live on
/// shard3 2026-06-10, but a spawn can only be placed in a room with a
/// controller). Novice/respawn protections are surfaced as separate
/// flags rather than folded into `open`, because their spawnability
/// depends on the account's own state — the operator decides. `now_ms`
/// is Unix epoch milliseconds (the unit the server uses for
/// `novice`/`respawnArea`/`openTime`).
pub fn derive_room_status(room: &str, stats: &RoomMapStats, now_ms: i64) -> RoomStatus {
    let normal = stats.status.as_deref() == Some("normal");
    let owned = stats.own.is_some();
    RoomStatus {
        open: normal && !owned && room_name_claimable(room),
        novice: timestamp_active(stats.novice.as_ref(), now_ms),
        respawn: timestamp_active(stats.respawn_area.as_ref(), now_ms),
    }
}

/// Pure: shard choice vs the server's shard list (official servers
/// only — see [`screeps_rest_api::Client::shards_info`] for why the
/// missing-shard failure mode is cryptic and quota-wasting without
/// this).
pub fn validate_shard_choice(shard: Option<&str>, available: &[String]) -> Result<()> {
    let listed = || available.join(", ");
    match shard {
        None => bail!(
            "this server is sharded — pass --shard (available: {})",
            listed()
        ),
        Some(s) if !available.iter().any(|a| a == s) => {
            bail!("unknown shard '{s}'; available: {}", listed())
        }
        Some(_) => Ok(()),
    }
}

/// A protection timestamp is active when present, numeric, and in the
/// future (the server sends `null` or a past value once expired).
fn timestamp_active(value: Option<&serde_json::Value>, now_ms: i64) -> bool {
    value
        .and_then(serde_json::Value::as_f64)
        .map(|t| t > now_ms as f64)
        .unwrap_or(false)
}

/// Pure: which of `rooms` actually need a scan call — those without a
/// cached status fresher than `ttl_secs` (0 = rescan everything).
/// This is what makes an interrupted `scan --all` resumable: completed
/// batches were persisted, so the re-run only pays for the remainder.
pub fn rooms_needing_scan(
    cache: &RoomCache,
    rooms: &[String],
    now_unix: u64,
    ttl_secs: u64,
) -> Vec<String> {
    if ttl_secs == 0 {
        return rooms.to_vec();
    }
    rooms
        .iter()
        .filter(|room| {
            cache
                .get(room)
                .map(|r| r.status_is_stale(now_unix, ttl_secs))
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

/// Pure: a room whose terrain AND planner objects are cached needs no
/// further fetch calls — both are positionally immutable (sources/
/// controller/mineral never move; mineral type never changes), so
/// `fetch` skips such rooms entirely and interrupted runs resume where
/// they stopped. (Rooms with genuinely zero planner objects — highway
/// rooms — refetch their cheap objects call each run; they carry no
/// terrain cost and never plan anyway.)
pub fn room_is_fully_fetched(room: &CachedRoom) -> bool {
    room.has_terrain() && !room.objects.is_empty()
}

// ---------------------------------------------------- quota planning

/// The pinned map-stats quota (60/hour on screeps.com) — falls back to
/// the same values if the shared table ever drops the entry.
fn map_stats_quota() -> EndpointQuota {
    endpoint_quota("POST", "/api/game/map-stats").unwrap_or(EndpointQuota {
        limit: 60,
        period: Duration::from_secs(3600),
    })
}

/// The pinned room-terrain quota (360/hour on screeps.com).
fn room_terrain_quota() -> EndpointQuota {
    endpoint_quota("GET", "/api/game/room-terrain").unwrap_or(EndpointQuota {
        limit: 360,
        period: Duration::from_secs(3600),
    })
}

/// Sustained-pacing ETA for `calls` calls under `quota`: the first call
/// is free, every further call waits `period/limit` (the shared
/// client's proactive spacing — it runs FASTER when response headers
/// show the window mostly unspent, so this is the honest upper bound
/// for a fresh window).
pub fn sustained_eta_secs(calls: usize, quota: EndpointQuota) -> u64 {
    (calls.saturating_sub(1) as u64) * quota.period.as_secs() / u64::from(quota.limit.max(1))
}

/// Up-front call-count/ETA math for a scan (printed by the CLI before
/// any network call on official servers).
#[derive(Debug)]
pub struct ScanPlan {
    pub rooms_total: usize,
    pub rooms_to_scan: usize,
    pub skipped_fresh: usize,
    /// map-stats calls = ceil(rooms_to_scan / MAP_STATS_CHUNK).
    pub calls: usize,
    pub quota: EndpointQuota,
    pub eta_secs: u64,
}

pub fn plan_scan(
    cache: &RoomCache,
    rooms: &[String],
    now_unix: u64,
    skip_fresh_within_secs: u64,
) -> ScanPlan {
    let rooms_to_scan = rooms_needing_scan(cache, rooms, now_unix, skip_fresh_within_secs).len();
    let calls = rooms_to_scan.div_ceil(MAP_STATS_CHUNK);
    let quota = map_stats_quota();
    ScanPlan {
        rooms_total: rooms.len(),
        rooms_to_scan,
        skipped_fresh: rooms.len() - rooms_to_scan,
        calls,
        quota,
        eta_secs: sustained_eta_secs(calls, quota),
    }
}

/// Up-front call-count/ETA math for a fetch. Terrain (360/hour) is the
/// dominant cost on MMO; room-objects rides the global 120/min cap.
#[derive(Debug)]
pub struct FetchPlan {
    pub rooms_total: usize,
    pub skipped_complete: usize,
    pub status_calls: usize,
    pub terrain_calls: usize,
    pub object_calls: usize,
    pub status_quota: EndpointQuota,
    pub terrain_quota: EndpointQuota,
    pub eta_secs: u64,
}

pub fn plan_fetch(
    cache: &RoomCache,
    rooms: &[String],
    status_ttl_secs: u64,
    now_unix: u64,
    min_delay_ms: u64,
) -> FetchPlan {
    let stale_statuses = rooms
        .iter()
        .filter(|room| {
            cache
                .get(room)
                .map(|r| r.status_is_stale(now_unix, status_ttl_secs))
                .unwrap_or(true)
        })
        .count();
    let pending: Vec<Option<&CachedRoom>> = rooms
        .iter()
        .map(|room| cache.get(room))
        .filter(|cached| !cached.map(room_is_fully_fetched).unwrap_or(false))
        .collect();
    let terrain_calls = pending
        .iter()
        .filter(|cached| cached.map(|r| !r.has_terrain()).unwrap_or(true))
        .count();
    let object_calls = pending.len();
    let status_calls = stale_statuses.div_ceil(MAP_STATS_CHUNK);
    let status_quota = map_stats_quota();
    let terrain_quota = room_terrain_quota();
    let eta_secs = sustained_eta_secs(status_calls, status_quota)
        + sustained_eta_secs(terrain_calls, terrain_quota)
        + object_calls as u64 * min_delay_ms / 1000;
    FetchPlan {
        rooms_total: rooms.len(),
        skipped_complete: rooms.len() - pending.len(),
        status_calls,
        terrain_calls,
        object_calls,
        status_quota,
        terrain_quota,
        eta_secs,
    }
}

// ------------------------------------------------------ async flows

/// [`scan_rooms_resumable`] with the original in-memory-only semantics
/// (no incremental persistence, no fresh-status skipping) — kept for
/// library consumers like the `screeps-server-kit` bootstrap, which
/// scans a small private-server map in one go.
pub async fn scan_rooms(
    client: &Client,
    cache: &mut RoomCache,
    rooms: &[String],
    now_unix: u64,
) -> Result<ScanSummary> {
    scan_rooms_resumable(client, cache, rooms, now_unix, None, 0).await
}

/// Scan `rooms` via batched map-stats and upsert spawnability into the
/// cache. Rooms the server reports as out-of-borders are recorded as
/// not-open (so re-scans stay cache-answerable).
///
/// RESUMABLE: when `save_path` is set the cache is persisted after
/// EVERY successful batch — an interrupted scan loses at most one
/// batch — and rooms whose cached status is fresher than
/// `skip_fresh_within_secs` (0 = rescan all) are skipped, so re-running
/// only pays for the remainder.
pub async fn scan_rooms_resumable(
    client: &Client,
    cache: &mut RoomCache,
    rooms: &[String],
    now_unix: u64,
    save_path: Option<&Path>,
    skip_fresh_within_secs: u64,
) -> Result<ScanSummary> {
    let now_ms = (now_unix as i64) * 1000;
    let mut summary = ScanSummary::default();
    let to_scan = rooms_needing_scan(cache, rooms, now_unix, skip_fresh_within_secs);
    summary.skipped_fresh = rooms.len() - to_scan.len();
    if summary.skipped_fresh > 0 {
        info!(
            skipped = summary.skipped_fresh,
            ttl_secs = skip_fresh_within_secs,
            "scan: skipping rooms with fresh cached status"
        );
    }
    for chunk in to_scan.chunks(MAP_STATS_CHUNK) {
        let response = client
            .map_stats(chunk, "owner0")
            .await
            .context("map-stats scan call")?;
        for room in chunk {
            let status = response
                .stats
                .get(room)
                .map(|entry| derive_room_status(room, entry, now_ms))
                .unwrap_or(RoomStatus {
                    open: false,
                    novice: false,
                    respawn: false,
                });
            summary.scanned += 1;
            if status.open {
                summary.open += 1;
                if status.respawn {
                    summary.respawn += 1;
                }
                if status.novice {
                    summary.novice += 1;
                }
            }
            cache.upsert(CachedRoom {
                spawn_status: Some(status),
                fetched_at: Some(now_unix),
                ..CachedRoom::new(room.clone())
            });
        }
        // RESUMABILITY: persist after every successful batch, so an
        // interruption (Ctrl-C, network, a >2h quota wait) loses at
        // most one batch of work.
        if let Some(path) = save_path {
            cache.save(path)?;
        }
        debug!(scanned = summary.scanned, "scan progress (cache saved)");
    }
    info!(
        scanned = summary.scanned,
        open = summary.open,
        skipped_fresh = summary.skipped_fresh,
        "scan complete"
    );
    Ok(summary)
}

/// [`fetch_rooms_resumable`] without incremental persistence — kept for
/// library consumers like the `screeps-server-kit` bootstrap (small
/// private-server maps, in-memory cache handling).
pub async fn fetch_rooms(
    client: &Client,
    cache: &mut RoomCache,
    rooms: &[String],
    status_ttl_secs: u64,
    now_unix: u64,
) -> Result<FetchSummary> {
    fetch_rooms_resumable(client, cache, rooms, status_ttl_secs, now_unix, None).await
}

/// Fetch terrain + planner objects for `rooms` into the cache; refresh
/// status (batched map-stats) for those whose status passed
/// `status_ttl_secs`. Rooms with terrain AND objects already cached are
/// skipped entirely ([`room_is_fully_fetched`] — both immutable), so
/// interrupted runs resume; when `save_path` is set the cache is
/// persisted after every status batch and every [`FETCH_SAVE_EVERY`]
/// fetched rooms.
pub async fn fetch_rooms_resumable(
    client: &Client,
    cache: &mut RoomCache,
    rooms: &[String],
    status_ttl_secs: u64,
    now_unix: u64,
    save_path: Option<&Path>,
) -> Result<FetchSummary> {
    let now_ms = (now_unix as i64) * 1000;
    let mut summary = FetchSummary::default();

    // Status refresh first (batched), only for stale rooms.
    let stale: Vec<String> = rooms
        .iter()
        .filter(|room| {
            cache
                .get(room)
                .map(|r| r.status_is_stale(now_unix, status_ttl_secs))
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    for chunk in stale.chunks(MAP_STATS_CHUNK) {
        let response = client
            .map_stats(chunk, "owner0")
            .await
            .context("map-stats status refresh")?;
        for room in chunk {
            if let Some(entry) = response.stats.get(room) {
                cache.upsert(CachedRoom {
                    spawn_status: Some(derive_room_status(room, entry, now_ms)),
                    fetched_at: Some(now_unix),
                    ..CachedRoom::new(room.clone())
                });
                summary.refreshed_status += 1;
            }
        }
        if let Some(path) = save_path {
            cache.save(path)?;
        }
    }

    let mut since_save = 0usize;
    for room in rooms {
        let cached = cache.get(room);
        if cached.map(room_is_fully_fetched).unwrap_or(false) {
            summary.skipped_complete += 1;
            continue;
        }
        let needs_terrain = cached.map(|r| !r.has_terrain()).unwrap_or(true);
        let terrain = if needs_terrain {
            let response = client
                .room_terrain_encoded(room)
                .await
                .with_context(|| format!("room-terrain for {room}"))?;
            let entry = response
                .terrain
                .first()
                .with_context(|| format!("room-terrain for {room} returned no entries"))?;
            validate_terrain(&entry.terrain)
                .with_context(|| format!("room-terrain for {room} failed validation"))?;
            summary.fetched_terrain += 1;
            entry.terrain.clone()
        } else {
            summary.skipped_terrain += 1;
            String::new() // upsert never clears cached terrain
        };

        let objects = client
            .room_objects(room)
            .await
            .with_context(|| format!("room-objects for {room}"))?;
        let planner_objects = filter_planner_objects(&objects.objects);
        if planner_objects.is_empty() {
            warn!(room, "no planner objects (source/controller/mineral) found");
        }
        summary.fetched_objects += 1;

        cache.upsert(CachedRoom {
            terrain,
            objects: planner_objects,
            fetched_at: Some(now_unix),
            ..CachedRoom::new(room.clone())
        });
        // RESUMABILITY: periodic persistence bounds rework on interrupt.
        since_save += 1;
        if since_save >= FETCH_SAVE_EVERY {
            if let Some(path) = save_path {
                cache.save(path)?;
                debug!(
                    fetched = summary.fetched_objects,
                    "fetch progress (cache saved)"
                );
            }
            since_save = 0;
        }
    }
    if let Some(path) = save_path {
        cache.save(path)?;
    }
    info!(
        fetched_terrain = summary.fetched_terrain,
        skipped_terrain = summary.skipped_terrain,
        fetched_objects = summary.fetched_objects,
        refreshed_status = summary.refreshed_status,
        skipped_complete = summary.skipped_complete,
        "fetch complete"
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_rest_api::{enumerate_room_names, RoomOwner};

    fn stats(status: Option<&str>, own: Option<RoomOwner>) -> RoomMapStats {
        RoomMapStats {
            status: status.map(str::to_owned),
            own,
            novice: None,
            open_time: None,
            respawn_area: None,
        }
    }

    const NOW_MS: i64 = 1_780_000_000_000;
    const NOW_UNIX: u64 = 1_780_000_000;

    fn scanned_room(name: &str, fetched_at: u64) -> CachedRoom {
        CachedRoom {
            spawn_status: Some(RoomStatus {
                open: true,
                novice: false,
                respawn: false,
            }),
            fetched_at: Some(fetched_at),
            ..CachedRoom::new(name)
        }
    }

    #[test]
    fn unowned_normal_claimable_room_is_open() {
        let s = derive_room_status("W11N11", &stats(Some("normal"), None), NOW_MS);
        assert_eq!(
            s,
            RoomStatus {
                open: true,
                novice: false,
                respawn: false
            }
        );
    }

    #[test]
    fn owned_or_reserved_room_is_not_open() {
        let owned = stats(
            Some("normal"),
            Some(RoomOwner {
                user: "u1".into(),
                level: 8,
            }),
        );
        assert!(!derive_room_status("W11N11", &owned, NOW_MS).open);
        // level 0 = reserved (Endpoints.md claim0 note) — still not open.
        let reserved = stats(
            Some("normal"),
            Some(RoomOwner {
                user: "u2".into(),
                level: 0,
            }),
        );
        assert!(!derive_room_status("W11N11", &reserved, NOW_MS).open);
    }

    #[test]
    fn out_of_borders_is_not_open() {
        let oob = stats(Some("out of borders"), None);
        assert!(!derive_room_status("W11N11", &oob, NOW_MS).open);
        assert!(!derive_room_status("W11N11", &stats(None, None), NOW_MS).open);
    }

    /// PINNED FROM LIVE FAILURE (shard3, 2026-06-10): map-stats reported
    /// highway W10N10 and SK-core portal room W15N15 as `status:
    /// "normal"`/unowned, and scan flagged both open — neither can hold
    /// a spawn (no controller). Claimability must gate `open`.
    #[test]
    fn unclaimable_rooms_are_not_open_even_when_normal_and_unowned() {
        let s = stats(Some("normal"), None);
        assert!(!derive_room_status("W10N10", &s, NOW_MS).open, "highway");
        assert!(!derive_room_status("W15N15", &s, NOW_MS).open, "portal");
        assert!(!derive_room_status("W14N16", &s, NOW_MS).open, "SK core");
    }

    #[test]
    fn room_name_claimability_follows_the_sector_layout() {
        // Highways: either coordinate ending in 0.
        for name in ["W10N10", "W0N3", "E20S5", "W7N30", "E0S0"] {
            assert!(!room_name_claimable(name), "{name} is a highway");
        }
        // Source-keeper core: BOTH coordinates ending in 4..=6
        // (the 5,5 member is the portal room).
        for name in ["W14N14", "W15N15", "E4S6", "W26N34", "E16S25"] {
            assert!(!room_name_claimable(name), "{name} is SK core/portal");
        }
        // One coordinate in 4..=6 alone is an ordinary claimable room.
        for name in ["W15N11", "W11N15", "E4S9", "W11N11", "E7S3", "W19N29"] {
            assert!(room_name_claimable(name), "{name} is claimable");
        }
        // Multi-digit coordinates use the printed numbers too.
        assert!(!room_name_claimable("W115N15"), "115,15 -> 5,5 portal");
        assert!(!room_name_claimable("W110N3"), "110 -> highway");
        assert!(room_name_claimable("W115N13"), "115,13 -> 5,3 claimable");
        // Unparseable names are conservatively unclaimable (incl.
        // numeric overflow past u32).
        for name in ["", "W1", "11N11", "W1X1", "WN", "W1N1N1", "sim"] {
            assert!(!room_name_claimable(name), "{name:?} must not parse");
        }
        assert!(!room_name_claimable("W99999999999N1"), "overflow");
    }

    #[test]
    fn shard_validation_demands_a_known_shard() {
        let shards: Vec<String> = ["shard0", "shard1", "shard2", "shard3"]
            .map(String::from)
            .into();
        assert!(validate_shard_choice(Some("shard3"), &shards).is_ok());
        let missing = validate_shard_choice(None, &shards).unwrap_err();
        assert!(missing.to_string().contains("--shard"), "{missing}");
        assert!(missing.to_string().contains("shard3"), "{missing}");
        // A typo'd shard must be rejected client-side with the real
        // choices, instead of paying for a scan call to learn it.
        let typo = validate_shard_choice(Some("shard9"), &shards).unwrap_err();
        assert!(typo.to_string().contains("shard9"), "{typo}");
        assert!(typo.to_string().contains("shard0"), "{typo}");
    }

    #[test]
    fn protection_timestamps_compare_against_now() {
        let mut s = stats(Some("normal"), None);
        s.novice = Some(serde_json::json!(NOW_MS + 1_000_000));
        s.respawn_area = Some(serde_json::json!(NOW_MS - 1_000_000));
        let derived = derive_room_status("W11N11", &s, NOW_MS);
        assert!(derived.open, "protection flags do not close a room");
        assert!(derived.novice, "future novice timestamp = active");
        assert!(!derived.respawn, "past respawn timestamp = expired");
        // null timestamps (the common case) are inactive.
        let mut s = stats(Some("normal"), None);
        s.novice = Some(serde_json::Value::Null);
        assert!(!derive_room_status("W11N11", &s, NOW_MS).novice);
    }

    /// Resume semantics: fresh statuses are skipped, stale/unknown
    /// rooms are scanned, TTL 0 forces a full rescan.
    #[test]
    fn rooms_needing_scan_ttl_semantics() {
        let cache = RoomCache {
            description: String::new(),
            rooms: vec![
                scanned_room("W1N1", NOW_UNIX - 100),  // fresh
                scanned_room("W2N1", NOW_UNIX - 5000), // stale (> 3600)
            ],
        };
        let rooms: Vec<String> = ["W1N1", "W2N1", "W3N1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            rooms_needing_scan(&cache, &rooms, NOW_UNIX, 3600),
            vec!["W2N1".to_owned(), "W3N1".to_owned()],
            "fresh W1N1 skipped; stale + never-seen scanned"
        );
        assert_eq!(
            rooms_needing_scan(&cache, &rooms, NOW_UNIX, 0).len(),
            3,
            "TTL 0 = rescan everything"
        );
    }

    #[test]
    fn fully_fetched_room_detection() {
        let mut room = CachedRoom::new("W1N1");
        assert!(!room_is_fully_fetched(&room), "empty room needs fetching");
        room.terrain = "0".repeat(2500);
        assert!(
            !room_is_fully_fetched(&room),
            "terrain without objects still needs the objects call"
        );
        room.objects = vec![serde_json::json!({"type": "source", "x": 1, "y": 1})];
        assert!(room_is_fully_fetched(&room));
    }

    /// BATCH CONSTRUCTION for a full official shard: 122x122 = 14 884
    /// rooms -> 15 map-stats calls of <= MAP_STATS_CHUNK rooms — a
    /// quarter of one 60/hour window (the operator's failed scan made
    /// 60+ calls at 64 rooms each and burned the window in 36 s).
    #[test]
    fn full_shard_scan_batches_fit_one_quota_window() {
        let rooms = enumerate_room_names(122, 122);
        assert_eq!(rooms.len(), 14_884);
        let batches: Vec<&[String]> = rooms.chunks(MAP_STATS_CHUNK).collect();
        assert_eq!(batches.len(), 15);
        assert!(batches.iter().all(|b| b.len() <= MAP_STATS_CHUNK));
        assert_eq!(batches.last().unwrap().len(), 14_884 - 14 * MAP_STATS_CHUNK);

        let quota = map_stats_quota();
        assert_eq!(quota.limit, 60, "pinned 60/hour");
        assert!(
            batches.len() as u32 <= quota.limit / 4,
            "a full shard scan uses at most a quarter of the hourly quota"
        );
    }

    /// ETA math: first call free, then sustained spacing — a 15-call
    /// full-shard scan is ~14 min at 60/hour.
    #[test]
    fn sustained_eta_math() {
        let quota = map_stats_quota();
        assert_eq!(sustained_eta_secs(0, quota), 0);
        assert_eq!(sustained_eta_secs(1, quota), 0, "first call is free");
        assert_eq!(sustained_eta_secs(15, quota), 14 * 60);
        let terrain = room_terrain_quota();
        assert_eq!(terrain.limit, 360);
        assert_eq!(
            sustained_eta_secs(361, terrain),
            3600,
            "360 terrain calls = one hour of 10 s spacing"
        );
    }

    /// The scan plan a 15k-room `scan --all` prints up front.
    #[test]
    fn plan_scan_full_shard() {
        let rooms = enumerate_room_names(122, 122);
        let plan = plan_scan(&RoomCache::default(), &rooms, NOW_UNIX, 3600);
        assert_eq!(plan.rooms_total, 14_884);
        assert_eq!(plan.rooms_to_scan, 14_884);
        assert_eq!(plan.calls, 15);
        assert_eq!(plan.eta_secs, 14 * 60, "~14 min sustained");
        // Re-running after a completed scan costs zero calls.
        let mut cache = RoomCache::default();
        for room in &rooms {
            cache.upsert(scanned_room(room, NOW_UNIX));
        }
        let plan = plan_scan(&cache, &rooms, NOW_UNIX, 3600);
        assert_eq!(plan.rooms_to_scan, 0);
        assert_eq!(plan.calls, 0);
        assert_eq!(plan.skipped_fresh, 14_884);
    }

    /// The fetch plan: complete rooms are skipped; terrain dominates the
    /// ETA at 10 s spacing.
    #[test]
    fn plan_fetch_mixed_cache() {
        let mut complete = scanned_room("W1N1", NOW_UNIX);
        complete.terrain = "0".repeat(2500);
        complete.objects = vec![serde_json::json!({"type": "source", "x": 1, "y": 1})];
        let mut terrain_only = scanned_room("W2N1", NOW_UNIX);
        terrain_only.terrain = "0".repeat(2500);
        let cache = RoomCache {
            description: String::new(),
            rooms: vec![complete, terrain_only],
        };
        let rooms: Vec<String> = ["W1N1", "W2N1", "W3N1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let plan = plan_fetch(&cache, &rooms, 3600, NOW_UNIX, 600);
        assert_eq!(plan.skipped_complete, 1, "W1N1 needs nothing");
        assert_eq!(plan.terrain_calls, 1, "only W3N1 needs terrain");
        assert_eq!(plan.object_calls, 2, "W2N1 + W3N1");
        assert_eq!(
            plan.status_calls, 1,
            "W3N1's status is unknown -> one batch"
        );
        // statuses fresh -> zero status batches.
        let plan_no_status = plan_fetch(&cache, &rooms[..2], 3600, NOW_UNIX, 600);
        assert_eq!(plan_no_status.status_calls, 0);
    }
}
