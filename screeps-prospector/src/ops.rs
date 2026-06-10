//! The scan/fetch flows shared by the CLI and (later, P0.P4) the
//! `screeps-eval bootstrap` integration. Library-first: the CLI is a
//! thin wrapper over these.
//!
//! - [`scan_rooms`]: batched `map-stats` (statName `owner0`) over a room
//!   list → spawnability flags upserted into the cache.
//! - [`fetch_rooms`]: per-room terrain (skipped when already cached —
//!   terrain is immutable) + planner objects, plus a batched status
//!   refresh for rooms whose status passed its TTL.
//!
//! The pure status derivation ([`derive_room_status`]) is unit-tested
//! offline; the async flows only sequence pinned client calls.

use crate::api::{ProspectorClient, RoomMapStats};
use crate::cache::{filter_planner_objects, validate_terrain, CachedRoom, RoomCache, RoomStatus};
use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Rooms per map-stats POST. The body is a JSON room list; 64 keeps
/// payloads small while a full private-server map is still 2 calls.
pub const MAP_STATS_CHUNK: usize = 64;

#[derive(Debug, Default)]
pub struct ScanSummary {
    pub scanned: usize,
    pub open: usize,
}

#[derive(Debug, Default)]
pub struct FetchSummary {
    pub fetched_terrain: usize,
    pub skipped_terrain: usize,
    pub fetched_objects: usize,
    pub refreshed_status: usize,
}

/// Pure: map-stats entry → spawnability flags.
///
/// `open` is conservative: the room must exist (`status == "normal"`)
/// and be unowned/unreserved. Novice/respawn protections are surfaced
/// as separate flags rather than folded into `open`, because their
/// spawnability depends on the account's own state — the operator
/// decides. `now_ms` is Unix epoch milliseconds (the unit the server
/// uses for `novice`/`respawnArea`/`openTime`).
pub fn derive_room_status(stats: &RoomMapStats, now_ms: i64) -> RoomStatus {
    let normal = stats.status.as_deref() == Some("normal");
    let owned = stats.own.is_some();
    RoomStatus {
        open: normal && !owned,
        novice: timestamp_active(stats.novice.as_ref(), now_ms),
        respawn: timestamp_active(stats.respawn_area.as_ref(), now_ms),
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

/// Scan `rooms` via batched map-stats and upsert spawnability into the
/// cache. Rooms the server reports as out-of-borders are recorded as
/// not-open (so re-scans stay cache-answerable).
pub async fn scan_rooms(
    client: &ProspectorClient,
    cache: &mut RoomCache,
    rooms: &[String],
    now_unix: u64,
) -> Result<ScanSummary> {
    let now_ms = (now_unix as i64) * 1000;
    let mut summary = ScanSummary::default();
    for chunk in rooms.chunks(MAP_STATS_CHUNK) {
        let response = client
            .map_stats(chunk, "owner0")
            .await
            .context("map-stats scan call")?;
        for room in chunk {
            let status = response
                .stats
                .get(room)
                .map(|entry| derive_room_status(entry, now_ms))
                .unwrap_or(RoomStatus {
                    open: false,
                    novice: false,
                    respawn: false,
                });
            summary.scanned += 1;
            if status.open {
                summary.open += 1;
            }
            cache.upsert(CachedRoom {
                spawn_status: Some(status),
                fetched_at: Some(now_unix),
                ..CachedRoom::new(room.clone())
            });
        }
        debug!(scanned = summary.scanned, "scan progress");
    }
    info!(
        scanned = summary.scanned,
        open = summary.open,
        "scan complete"
    );
    Ok(summary)
}

/// Fetch terrain + planner objects for `rooms` into the cache; refresh
/// status (one batched map-stats) for those whose status passed
/// `status_ttl_secs`. Terrain already cached is never refetched
/// (immutable — the cheapest call is the one not made).
pub async fn fetch_rooms(
    client: &ProspectorClient,
    cache: &mut RoomCache,
    rooms: &[String],
    status_ttl_secs: u64,
    now_unix: u64,
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
                    spawn_status: Some(derive_room_status(entry, now_ms)),
                    fetched_at: Some(now_unix),
                    ..CachedRoom::new(room.clone())
                });
                summary.refreshed_status += 1;
            }
        }
    }

    for room in rooms {
        let needs_terrain = cache.get(room).map(|r| !r.has_terrain()).unwrap_or(true);
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
    }
    info!(
        fetched_terrain = summary.fetched_terrain,
        skipped_terrain = summary.skipped_terrain,
        fetched_objects = summary.fetched_objects,
        refreshed_status = summary.refreshed_status,
        "fetch complete"
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::RoomOwner;

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

    #[test]
    fn unowned_normal_room_is_open() {
        let s = derive_room_status(&stats(Some("normal"), None), NOW_MS);
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
        assert!(!derive_room_status(&owned, NOW_MS).open);
        // level 0 = reserved (Endpoints.md claim0 note) — still not open.
        let reserved = stats(
            Some("normal"),
            Some(RoomOwner {
                user: "u2".into(),
                level: 0,
            }),
        );
        assert!(!derive_room_status(&reserved, NOW_MS).open);
    }

    #[test]
    fn out_of_borders_is_not_open() {
        assert!(!derive_room_status(&stats(Some("out of borders"), None), NOW_MS).open);
        assert!(!derive_room_status(&stats(None, None), NOW_MS).open);
    }

    #[test]
    fn protection_timestamps_compare_against_now() {
        let mut s = stats(Some("normal"), None);
        s.novice = Some(serde_json::json!(NOW_MS + 1_000_000));
        s.respawn_area = Some(serde_json::json!(NOW_MS - 1_000_000));
        let derived = derive_room_status(&s, NOW_MS);
        assert!(derived.open, "protection flags do not close a room");
        assert!(derived.novice, "future novice timestamp = active");
        assert!(!derived.respawn, "past respawn timestamp = expired");
        // null timestamps (the common case) are inactive.
        let mut s = stats(Some("normal"), None);
        s.novice = Some(serde_json::Value::Null);
        assert!(!derive_room_status(&s, NOW_MS).novice);
    }
}
