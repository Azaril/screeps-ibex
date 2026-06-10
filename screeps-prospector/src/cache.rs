//! File-backed room cache in the foreman-bench map-JSON shape.
//!
//! CACHE FORMAT CONTRACT (P0.P2): the on-disk shape is EXACTLY what
//! `screeps-foreman-bench` (READ-ONLY sibling crate) loads —
//! `screeps-foreman-bench/src/main.rs`: `MapData` (:529-532, `{rooms}`;
//! a top-level `description` is tolerated because default serde ignores
//! unknown fields), `BenchRoomData` (:483-489, `{room, terrain, objects}`),
//! the per-char hex terrain decode (`terrain_string_to_vec`, :451-481),
//! and the 50x50=2500 length check (applied inside `get_terrain`,
//! :496-501 — at PLANNING time, not at deserialize time). Neither bench
//! struct sets `deny_unknown_fields`, so our extra per-room keys are
//! ignored by the bench loader; `cache_extended_file_still_satisfies_the_bench_loader`
//! pins that tolerance against a hand-maintained mirror of the structs.
//!
//! Extensions (optional keys, omitted when absent):
//! - `spawnStatus`: `{open, novice, respawn}` — spawnability flags from
//!   scan. NAMED `spawnStatus`, NOT `status`: the bench's own seed dumps
//!   (resources/*.json) already carry a per-room `status` key holding a
//!   server-status STRING (`"normal"`), so the planned `status` object
//!   key would collide with real seed data.
//! - `fetchedAt`: Unix seconds of the last API refresh (drives the
//!   status TTL; terrain is immutable and never refetched).
//!
//! Unknown per-room keys present in seed data (`status`, `bus`,
//! `depositType`, `sourceKeepers`, ...) are PRESERVED across
//! load/upsert/save via a flattened map — seeding from a bench dump and
//! saving must not strip information from the copy.
//!
//! Bench-REQUIRED keys are always serialized: `room`, `terrain` (hex
//! digit string; `""` until fetched — still deserializable by the
//! bench, whose length check only runs when a room is planned), and
//! `objects` (an ARRAY of `{type, x, y, ...}` — we deliberately keep
//! the bench's array shape rather than a `{sources, controller,
//! mineral}` struct, because the bench requires `objects` to be an
//! array; the structured view is [`CachedRoom::objects_summary`]).
//!
//! Seed data: the bench's `resources/*.json` files (private-server
//! default map + MMO shard dumps) load directly; [`seed_from`] COPIES
//! them — the originals are fixtures and are never mutated.

use anyhow::{bail, Context, Result};
use screeps_foreman::terrain::FastRoomTerrain;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 50x50 — the bench's expected terrain length (bench main.rs:496-501).
pub const TERRAIN_LEN: usize = 50 * 50;

/// Object types the planner consumes (bench main.rs:516-526 reads
/// exactly these via `get_object_type`).
pub const PLANNER_OBJECT_TYPES: [&str; 3] = ["source", "controller", "mineral"];

/// Default cache file: `cache/<shard-or-server>.json`, crate-relative
/// (fixed-path rule — the crate is invoked from its own directory).
pub fn default_cache_path(server_name: &str, shard: Option<&str>) -> PathBuf {
    PathBuf::from("cache").join(format!("{}.json", shard.unwrap_or(server_name)))
}

/// The whole cache file (the bench's `MapData` + `description`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoomCache {
    /// Free-form provenance line (the bench files carry one, e.g.
    /// `"mmo:shard3 map dump"`); ignored by the bench loader.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub rooms: Vec<CachedRoom>,
}

/// One per-room record (the bench's `BenchRoomData` + extensions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRoom {
    pub room: String,
    /// 2500 hex digits once fetched; `""` while only scanned. ALWAYS
    /// serialized (bench-required key). Terrain is immutable — once
    /// non-empty it is never overwritten (see [`RoomCache::upsert`]).
    #[serde(default)]
    pub terrain: String,
    /// Bench-shaped object array: `{type, x, y}` (+ `mineralType`).
    /// ALWAYS serialized (bench-required key).
    #[serde(default)]
    pub objects: Vec<Value>,
    /// EXTENSION: spawnability flags (absent in pure bench files).
    /// Serialized as `spawnStatus` — `status` is taken by the seed
    /// dumps' server-status string (see the module docs).
    #[serde(
        default,
        rename = "spawnStatus",
        skip_serializing_if = "Option::is_none"
    )]
    pub spawn_status: Option<RoomStatus>,
    /// EXTENSION: Unix seconds of the last API refresh.
    #[serde(default, rename = "fetchedAt", skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
    /// Whatever else the file carries (the bench seed dumps include
    /// `status`, `bus`, `depositType`, `sourceKeepers`, ...). Preserved
    /// verbatim across load/save; serde_json's default BTreeMap keeps
    /// key order stable for clean diffs.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Spawnability flags derived from map-stats (see `ops::derive_room_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomStatus {
    /// Room exists, is `"normal"`, is unowned/unreserved, AND is
    /// claimable by name ([`room_name_claimable`] — highways and
    /// source-keeper rooms are excluded).
    #[serde(default)]
    pub open: bool,
    /// Active novice-area protection (spawnable by novice accounts only).
    #[serde(default)]
    pub novice: bool,
    /// Active respawn-area protection (official server).
    #[serde(default)]
    pub respawn: bool,
}

/// Whether a room can hold a controller at all, from the standard map
/// sector layout (each 10x10 sector: coordinates ending in 0 are
/// controller-less highways; the central 3x3 block — both coordinates
/// ending in 4..=6 — is the source-keeper core with the 5,5 portal
/// room). The rule operates on the NUMBERS in the room name (`W15N15`
/// → 5,5 → core), matching how the official map is generated (the
/// bench's shard0..shard3 dumps agree on all 77,776 rooms: the DB
/// `bus` highway flag equals the %10==0 rule exactly, and no room
/// with a controller is name-excluded); private servers using the
/// standard map generator follow the same layout. Unparseable names
/// are conservatively unclaimable.
///
/// CAVEATS: this is a NECESSARY condition, not sufficient — shard0
/// has at least one controller-less `"normal"` room that passes the
/// name rule (W22S49 in the bench dump), so `open` stays best-effort
/// and fetch/score verify actual objects. Hand-built custom maps can
/// break the convention entirely. The filter exists to keep
/// `scan --all` from flagging ~30% of an MMO shard's rooms
/// (highways/SK) open and burning the room-terrain fetch quota on
/// them.
pub fn room_name_claimable(room: &str) -> bool {
    fn axis(s: &str) -> Option<(u32, &str)> {
        let digits = s.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        Some((s[..digits].parse().ok()?, &s[digits..]))
    }
    let Some(rest) = room.strip_prefix(['W', 'E']) else {
        return false;
    };
    let Some((x, rest)) = axis(rest) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(['N', 'S']) else {
        return false;
    };
    let Some((y, rest)) = axis(rest) else {
        return false;
    };
    if !rest.is_empty() {
        return false;
    }
    let (xm, ym) = (x % 10, y % 10);
    let highway = xm == 0 || ym == 0;
    let sk_core = (4..=6).contains(&xm) && (4..=6).contains(&ym);
    !highway && !sk_core
}

/// Structured view over the bench-shaped `objects` array.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectsSummary {
    pub sources: Vec<(i32, i32)>,
    pub controller: Option<(i32, i32)>,
    pub mineral: Option<MineralInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MineralInfo {
    pub x: i32,
    pub y: i32,
    pub mineral_type: Option<String>,
}

impl CachedRoom {
    pub fn new(room: impl Into<String>) -> Self {
        CachedRoom {
            room: room.into(),
            terrain: String::new(),
            objects: Vec::new(),
            spawn_status: None,
            fetched_at: None,
            extra: serde_json::Map::new(),
        }
    }

    /// True once full terrain is cached (terrain is immutable, so this
    /// also means "never refetch").
    pub fn has_terrain(&self) -> bool {
        self.terrain.len() == TERRAIN_LEN
    }

    /// Status TTL policy: stale when there is no status/timestamp yet,
    /// or the record is older than `ttl_secs`. (Terrain has no TTL —
    /// it is immutable.)
    pub fn status_is_stale(&self, now_unix: u64, ttl_secs: u64) -> bool {
        match (self.spawn_status.as_ref(), self.fetched_at) {
            (Some(_), Some(at)) => now_unix.saturating_sub(at) > ttl_secs,
            _ => true,
        }
    }

    /// Decode into the planner's terrain type — same semantics as the
    /// bench's `get_terrain` (main.rs:496-501).
    pub fn to_fast_terrain(&self) -> Result<FastRoomTerrain> {
        Ok(FastRoomTerrain::new(terrain_hex_to_vec(&self.terrain)?))
    }

    /// Structured view of the planner objects, mirroring the bench's
    /// `get_object_type` filters (main.rs:503-526): first controller
    /// and first mineral win, all sources are kept.
    pub fn objects_summary(&self) -> ObjectsSummary {
        let mut summary = ObjectsSummary::default();
        for object in &self.objects {
            let Some(map) = object.as_object() else {
                continue;
            };
            let Some(kind) = map.get("type").and_then(Value::as_str) else {
                continue;
            };
            let (Some(x), Some(y)) = (
                map.get("x").and_then(Value::as_i64),
                map.get("y").and_then(Value::as_i64),
            ) else {
                continue;
            };
            let (x, y) = (x as i32, y as i32);
            match kind {
                "source" => summary.sources.push((x, y)),
                "controller" => {
                    summary.controller.get_or_insert((x, y));
                }
                "mineral" => {
                    if summary.mineral.is_none() {
                        summary.mineral = Some(MineralInfo {
                            x,
                            y,
                            mineral_type: map
                                .get("mineralType")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        });
                    }
                }
                _ => {}
            }
        }
        summary
    }
}

impl RoomCache {
    pub fn get(&self, room: &str) -> Option<&CachedRoom> {
        self.rooms.iter().find(|r| r.room == room)
    }

    /// Rooms currently flagged open for spawning. Re-applies
    /// [`room_name_claimable`] on read: cache files written before the
    /// claimability gate existed flag highways/source-keeper rooms
    /// open, and this retroactively sanitizes them so `fetch
    /// --all-open` doesn't burn the room-terrain quota on them.
    pub fn open_rooms(&self) -> impl Iterator<Item = &CachedRoom> {
        self.rooms
            .iter()
            .filter(|r| r.spawn_status.map(|s| s.open).unwrap_or(false))
            .filter(|r| room_name_claimable(&r.room))
    }

    /// Merge a record in. Semantics:
    /// - terrain is IMMUTABLE: an existing non-empty value is never
    ///   overwritten, and an empty incoming value never clears it;
    /// - `objects` is replaced only when the incoming array is
    ///   non-empty (a scan-only record must not wipe fetched objects);
    /// - `spawnStatus`/`fetchedAt` are replaced when the incoming
    ///   record carries them;
    /// - unknown keys (`extra`) merge per-key, incoming wins — existing
    ///   seed metadata is never wiped wholesale.
    pub fn upsert(&mut self, incoming: CachedRoom) {
        match self.rooms.iter_mut().find(|r| r.room == incoming.room) {
            Some(existing) => {
                if existing.terrain.is_empty() && !incoming.terrain.is_empty() {
                    existing.terrain = incoming.terrain;
                }
                if !incoming.objects.is_empty() {
                    existing.objects = incoming.objects;
                }
                if incoming.spawn_status.is_some() {
                    existing.spawn_status = incoming.spawn_status;
                }
                if incoming.fetched_at.is_some() {
                    existing.fetched_at = incoming.fetched_at;
                }
                for (key, value) in incoming.extra {
                    existing.extra.insert(key, value);
                }
            }
            None => self.rooms.push(incoming),
        }
    }

    /// Load a cache file — tolerates pure bench files (no extensions)
    /// by construction (every extension key defaults).
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading cache file {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Save with stable ordering for clean diffs: rooms sorted by name,
    /// pretty-printed, fixed field order (serde emits declaration
    /// order), trailing newline.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut sorted = self.clone();
        sorted.rooms.sort_by(|a, b| a.room.cmp(&b.room));
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        let mut json = serde_json::to_string_pretty(&sorted)?;
        json.push('\n');
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Seed a cache file by COPYING a bench map JSON (e.g.
/// `../screeps-foreman-bench/resources/default-private-server.json`).
/// The source is a fixture: it is opened read-only by the copy and
/// never mutated. Returns the number of rooms in the seeded cache.
pub fn seed_from(source: &Path, dest: &Path, overwrite: bool) -> Result<usize> {
    if dest.exists() && !overwrite {
        bail!(
            "{} already exists (pass --force to overwrite)",
            dest.display()
        );
    }
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::copy(source, dest)
        .with_context(|| format!("copying {} -> {}", source.display(), dest.display()))?;
    Ok(RoomCache::load(dest)?.rooms.len())
}

/// Decode an encoded terrain string exactly the way the bench does
/// (per-char `to_digit(16)`, main.rs:451-481) while enforcing the 50x50
/// length the bench checks at main.rs:496-501.
pub fn terrain_hex_to_vec(terrain: &str) -> Result<Vec<u8>> {
    if terrain.len() != TERRAIN_LEN {
        bail!(
            "terrain must be {TERRAIN_LEN} chars (50x50), got {}",
            terrain.len()
        );
    }
    terrain
        .chars()
        .map(|c| {
            c.to_digit(16)
                .map(|d| d as u8)
                .with_context(|| format!("terrain contains non-hex-digit character {c:?}"))
        })
        .collect()
}

/// Validate without keeping the buffer (the fetch path's write gate).
pub fn validate_terrain(terrain: &str) -> Result<()> {
    terrain_hex_to_vec(terrain).map(|_| ())
}

/// Reduce a raw room-objects API array to the bench-shaped planner
/// subset: `{type, x, y}` plus `mineralType` when present. Everything
/// else (ids, energy, store contents, ...) is dropped — the cache holds
/// only what the planner reads.
pub fn filter_planner_objects(objects: &[Value]) -> Vec<Value> {
    objects
        .iter()
        .filter_map(|object| {
            let map = object.as_object()?;
            let kind = map.get("type")?.as_str()?;
            if !PLANNER_OBJECT_TYPES.contains(&kind) {
                return None;
            }
            let x = map.get("x")?.as_i64()?;
            let y = map.get("y")?.as_i64()?;
            let mut out = serde_json::Map::new();
            out.insert("type".to_owned(), Value::from(kind));
            out.insert("x".to_owned(), Value::from(x));
            out.insert("y".to_owned(), Value::from(y));
            if let Some(mineral_type) = map.get("mineralType") {
                out.insert("mineralType".to_owned(), mineral_type.clone());
            }
            Some(Value::Object(out))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bench seed fixture — READ-ONLY; tests operate on a COPY.
    const BENCH_PRIVATE_SERVER: &str =
        "../screeps-foreman-bench/resources/default-private-server.json";

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("screeps-prospector-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn full_terrain() -> String {
        // 2500 digits with some walls so FastRoomTerrain has texture.
        let mut t = "0".repeat(TERRAIN_LEN);
        t.replace_range(0..4, "1212");
        t
    }

    fn extended_room(name: &str) -> CachedRoom {
        CachedRoom {
            room: name.to_owned(),
            terrain: full_terrain(),
            objects: vec![
                serde_json::json!({"type": "source", "x": 14, "y": 9}),
                serde_json::json!({"type": "source", "x": 38, "y": 41}),
                serde_json::json!({"type": "controller", "x": 21, "y": 32}),
                serde_json::json!({"type": "mineral", "x": 40, "y": 40, "mineralType": "H"}),
            ],
            spawn_status: Some(RoomStatus {
                open: true,
                novice: false,
                respawn: false,
            }),
            fetched_at: Some(1_780_000_000),
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn round_trip_preserves_extensions() {
        let path = temp_path("round-trip.json");
        let mut cache = RoomCache {
            description: "test:round-trip".to_owned(),
            rooms: vec![extended_room("W5N5")],
        };
        // A scanned-but-unfetched room: empty terrain/objects, status only.
        cache.upsert(CachedRoom {
            spawn_status: Some(RoomStatus {
                open: false,
                novice: true,
                respawn: false,
            }),
            fetched_at: Some(1_780_000_100),
            ..CachedRoom::new("W6N5")
        });
        cache.save(&path).unwrap();

        let loaded = RoomCache::load(&path).unwrap();
        assert_eq!(loaded.description, "test:round-trip");
        assert_eq!(loaded.rooms.len(), 2);
        let full = loaded.get("W5N5").unwrap();
        assert!(full.has_terrain());
        assert_eq!(full.fetched_at, Some(1_780_000_000));
        assert_eq!(
            full.spawn_status,
            Some(RoomStatus {
                open: true,
                novice: false,
                respawn: false
            })
        );
        let summary = full.objects_summary();
        assert_eq!(summary.sources, vec![(14, 9), (38, 41)]);
        assert_eq!(summary.controller, Some((21, 32)));
        assert_eq!(
            summary.mineral.as_ref().unwrap().mineral_type.as_deref(),
            Some("H")
        );
        let scanned = loaded.get("W6N5").unwrap();
        assert!(!scanned.has_terrain());
        assert!(scanned.spawn_status.unwrap().novice);
    }

    #[test]
    fn upsert_terrain_is_immutable_and_status_replaces() {
        let mut cache = RoomCache::default();
        cache.upsert(extended_room("W5N5"));
        let original_terrain = cache.get("W5N5").unwrap().terrain.clone();

        // A later scan-only record: empty terrain/objects must not
        // clobber; fresher status/fetchedAt must replace.
        cache.upsert(CachedRoom {
            spawn_status: Some(RoomStatus {
                open: false,
                novice: false,
                respawn: false,
            }),
            fetched_at: Some(1_790_000_000),
            ..CachedRoom::new("W5N5")
        });
        let room = cache.get("W5N5").unwrap();
        assert_eq!(room.terrain, original_terrain, "terrain is immutable");
        assert_eq!(room.objects.len(), 4, "objects survive scan-only upserts");
        assert!(!room.spawn_status.unwrap().open, "status replaced");
        assert_eq!(room.fetched_at, Some(1_790_000_000));

        // A record with different terrain must NOT overwrite (immutable).
        cache.upsert(CachedRoom {
            terrain: "2".repeat(TERRAIN_LEN),
            ..CachedRoom::new("W5N5")
        });
        assert_eq!(cache.get("W5N5").unwrap().terrain, original_terrain);

        // Unknown room inserts.
        cache.upsert(CachedRoom::new("W9N9"));
        assert_eq!(cache.rooms.len(), 2);
    }

    #[test]
    fn save_orders_rooms_stably() {
        let path_a = temp_path("stable-a.json");
        let path_b = temp_path("stable-b.json");
        let cache_a = RoomCache {
            description: "test:order".to_owned(),
            rooms: vec![CachedRoom::new("W2N1"), CachedRoom::new("W1N1")],
        };
        let cache_b = RoomCache {
            description: "test:order".to_owned(),
            rooms: vec![CachedRoom::new("W1N1"), CachedRoom::new("W2N1")],
        };
        cache_a.save(&path_a).unwrap();
        cache_b.save(&path_b).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path_a).unwrap(),
            std::fs::read_to_string(&path_b).unwrap(),
            "same content must serialize identically regardless of insertion order"
        );
    }

    #[test]
    fn status_ttl_policy() {
        let mut room = CachedRoom::new("W1N1");
        assert!(room.status_is_stale(1000, 3600), "no status yet = stale");
        room.spawn_status = Some(RoomStatus {
            open: true,
            novice: false,
            respawn: false,
        });
        room.fetched_at = Some(1000);
        assert!(!room.status_is_stale(1000 + 3600, 3600), "within TTL");
        assert!(room.status_is_stale(1000 + 3601, 3600), "past TTL");
    }

    /// Seed/load against the REAL bench resource — via a COPY; the
    /// original is asserted byte-identical afterwards.
    #[test]
    fn bench_resource_seeds_and_loads_without_mutation() {
        let source = Path::new(BENCH_PRIVATE_SERVER);
        let before = std::fs::read(source).expect(
            "bench fixture missing — run tests from the screeps-prospector crate directory",
        );
        let dest = temp_path("seeded-private-server.json");
        let room_count = seed_from(source, &dest, true).unwrap();
        assert!(room_count > 0);

        let mut cache = RoomCache::load(&dest).unwrap();
        assert_eq!(cache.rooms.len(), room_count);
        let first = cache.rooms[0].clone();
        assert!(first.has_terrain(), "bench rooms carry full terrain");
        assert!(
            first.spawn_status.is_none(),
            "pure bench files have no prospector extensions"
        );
        assert!(first.fetched_at.is_none());
        // The seed dump's own per-room keys (`status` server-status
        // STRING, `bus`, ...) land in `extra` — this collision is why
        // the extension key is `spawnStatus`, not `status`.
        assert_eq!(
            first.extra.get("status").and_then(Value::as_str),
            Some("normal"),
            "seed dumps carry a per-room status string"
        );
        // The planner bridge works on seeded data (exercises the
        // host-side screeps-foreman dependency).
        first.to_fast_terrain().unwrap();

        // Upsert + save must PRESERVE the seed's unknown keys and stay
        // loadable.
        cache.upsert(CachedRoom {
            spawn_status: Some(RoomStatus {
                open: true,
                novice: false,
                respawn: false,
            }),
            fetched_at: Some(1_780_000_000),
            ..CachedRoom::new(first.room.clone())
        });
        let resaved = temp_path("seeded-resaved.json");
        cache.save(&resaved).unwrap();
        let reloaded = RoomCache::load(&resaved).unwrap();
        let room = reloaded.get(&first.room).unwrap();
        assert_eq!(room.extra, first.extra, "seed metadata survives save");
        assert!(room.spawn_status.unwrap().open);
        assert_eq!(room.terrain, first.terrain);

        let after = std::fs::read(source).unwrap();
        assert_eq!(before, after, "the bench fixture must never be mutated");
    }

    /// THE BENCH LOADER CONTRACT. `BenchMirror*` is a hand-maintained
    /// minimal duplicate of the READ-ONLY bench loader structs in
    /// screeps-foreman-bench/src/main.rs:
    ///
    /// - `MapData` (:529-532) — `{rooms}`; a top-level `description` is
    ///   an unknown field it ignores;
    /// - `BenchRoomData` (:483-489) — `room` + `terrain` + `objects`
    ///   all REQUIRED (no serde defaults), no deny_unknown_fields, so
    ///   our extra `spawnStatus`/`fetchedAt` keys are tolerated;
    /// - terrain decode (:451-481) — per-char `to_digit(16)`, i.e. any
    ///   hex-digit string deserializes (the 2500 check happens later,
    ///   :496-501).
    ///
    /// The mirror models `terrain` as `String` and applies the same hex
    /// decode separately via `terrain_hex_to_vec`.
    #[test]
    fn cache_extended_file_still_satisfies_the_bench_loader() {
        #[derive(Deserialize)]
        struct BenchMirrorMapData {
            rooms: Vec<BenchMirrorRoom>,
        }
        #[derive(Deserialize)]
        struct BenchMirrorRoom {
            room: String,
            terrain: String,     // bench: required, hex-decoded at deserialize time
            objects: Vec<Value>, // bench: required array
        }

        let path = temp_path("bench-contract.json");
        let mut cache = RoomCache {
            description: "test:bench-contract".to_owned(),
            rooms: vec![extended_room("W5N5")],
        };
        // Scan-only room: bench-required keys must still be emitted
        // (terrain "" and objects [] both deserialize on the bench side;
        // the 2500 check only runs when that room is planned).
        cache.upsert(CachedRoom {
            spawn_status: Some(RoomStatus {
                open: true,
                novice: false,
                respawn: false,
            }),
            fetched_at: Some(1_780_000_000),
            ..CachedRoom::new("W6N5")
        });
        cache.save(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let mirrored: BenchMirrorMapData = serde_json::from_str(&raw)
            .expect("a cache-extended file must deserialize through the bench loader shape");
        assert_eq!(mirrored.rooms.len(), 2);
        for room in &mirrored.rooms {
            // Every terrain char the cache writes must survive the
            // bench's hex decode (empty = scanned-only, valid there too).
            assert!(
                room.terrain.chars().all(|c| c.is_ascii_hexdigit()),
                "non-hex terrain in {}",
                room.room
            );
        }
        let full = mirrored.rooms.iter().find(|r| r.room == "W5N5").unwrap();
        assert_eq!(
            terrain_hex_to_vec(&full.terrain).unwrap().len(),
            TERRAIN_LEN,
            "fetched terrain passes the bench's 50x50 check"
        );
        assert_eq!(full.objects.len(), 4);
        // And the REAL bench resource parses through the mirror too —
        // pinning that the mirror matches the actual format.
        let bench_raw = std::fs::read_to_string(BENCH_PRIVATE_SERVER).unwrap();
        let bench: BenchMirrorMapData = serde_json::from_str(&bench_raw).unwrap();
        assert!(!bench.rooms.is_empty());
    }

    #[test]
    fn filter_planner_objects_reduces_api_payloads() {
        let api_objects = vec![
            serde_json::json!({"_id": "a1", "room": "W1N1", "type": "source", "x": 14, "y": 9, "energy": 3000}),
            serde_json::json!({"_id": "a2", "room": "W1N1", "type": "controller", "x": 21, "y": 32, "level": 0}),
            serde_json::json!({"_id": "a3", "room": "W1N1", "type": "mineral", "x": 40, "y": 40, "mineralType": "H", "mineralAmount": 65000}),
            serde_json::json!({"_id": "a4", "room": "W1N1", "type": "keeperLair", "x": 10, "y": 10}),
        ];
        let filtered = filter_planner_objects(&api_objects);
        assert_eq!(filtered.len(), 3, "non-planner objects are dropped");
        assert_eq!(
            filtered[0],
            serde_json::json!({"type": "source", "x": 14, "y": 9})
        );
        assert_eq!(filtered[2]["mineralType"], "H");
        assert!(filtered[2].get("_id").is_none(), "ids are stripped");
    }

    #[test]
    fn fast_terrain_bridge_decodes_walls() {
        let room = extended_room("W5N5"); // terrain starts "1212..."
        let terrain = room.to_fast_terrain().unwrap();
        assert!(terrain.is_wall(0, 0));
        assert!(!terrain.is_wall(1, 0)); // '2' = swamp, not wall
        assert!(terrain.is_wall(2, 0));
    }

    #[test]
    fn terrain_validation_rejects_bad_input() {
        assert!(validate_terrain(&"0".repeat(TERRAIN_LEN)).is_ok());
        assert!(validate_terrain("012").is_err(), "wrong length");
        let mut bad = "0".repeat(TERRAIN_LEN);
        bad.replace_range(10..11, "x");
        assert!(validate_terrain(&bad).is_err(), "non-hex char");
    }

    #[test]
    fn default_cache_path_prefers_shard() {
        assert_eq!(
            default_cache_path("private-server", None),
            PathBuf::from("cache").join("private-server.json")
        );
        assert_eq!(
            default_cache_path("mmo", Some("shard3")),
            PathBuf::from("cache").join("shard3.json")
        );
    }
}
