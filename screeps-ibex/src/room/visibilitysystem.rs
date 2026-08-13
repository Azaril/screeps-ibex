use super::data::*;
use crate::entitymappingsystem::*;
use crate::serialize::*;
use bitflags::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::prelude::*;
use specs::saveload::*;
use specs::Component;
use std::fmt;

pub const VISIBILITY_PRIORITY_CRITICAL: f32 = 100.0;
pub const VISIBILITY_PRIORITY_HIGH: f32 = 75.0;
pub const VISIBILITY_PRIORITY_MEDIUM: f32 = 50.0;
pub const VISIBILITY_PRIORITY_LOW: f32 = 25.0;
pub const VISIBILITY_PRIORITY_NONE: f32 = 0.0;

/// Default TTL for visibility requests (in ticks). Must be longer than the
/// longest interval between re-requests (e.g. mining outpost pushes every 50
/// ticks, so 100 gives a comfortable margin). Doubles as the default
/// `want_fresh_within` freshness target (ADR 0046 D1): unless a producer
/// declares otherwise, intel younger than one TTL counts as serviced.
pub const DEFAULT_VISIBILITY_TTL: u32 = 100;

bitflags! {
    #[derive(Copy, Clone, Debug)]
    pub struct VisibilityRequestFlags: u8 {
        const UNSET = 0;

        const OBSERVE = 1u8;
        const SCOUT = 1u8 << 1;

        const ALL = Self::OBSERVE.bits() | Self::SCOUT.bits();
    }
}

impl Serialize for VisibilityRequestFlags {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bits().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VisibilityRequestFlags {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bits = u8::deserialize(deserializer)?;
        Ok(VisibilityRequestFlags::from_bits_truncate(bits))
    }
}

impl fmt::Display for VisibilityRequestFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let observe = self.contains(VisibilityRequestFlags::OBSERVE);
        let scout = self.contains(VisibilityRequestFlags::SCOUT);
        match (observe, scout) {
            (true, true) => write!(f, "O+S"),
            (true, false) => write!(f, "O"),
            (false, true) => write!(f, "S"),
            (false, false) => write!(f, "-"),
        }
    }
}

// ─── Persistent layer: VisibilityQueueData (serialized component) ────────────

/// A single persistent visibility request entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisibilityEntry {
    pub room_name: RoomName,
    pub priority: f32,
    pub allowed_types: VisibilityRequestFlags,
    /// Game tick at which this entry expires.
    pub expires_at: u32,
    /// When true, this entry should only be serviced by scouts that are already
    /// alive. It never counts toward the scout-fleet spawn EV (ADR 0046 D5).
    pub opportunistic: bool,
    /// Intel age (ticks) at or below which this entry counts as SERVICED
    /// (ADR 0046 D1). Producers declare how fresh they need the room; the
    /// assigner derives service state instead of producers churning requests.
    /// Merged with MIN on upsert — the strictest freshness demand wins
    /// (design-review resolution #3). Serialized: part of the WFV 28 shape.
    pub want_fresh_within: u32,
}

impl Default for VisibilityEntry {
    fn default() -> Self {
        Self {
            room_name: RoomName::new("E0N0").unwrap(),
            priority: 0.0,
            allowed_types: VisibilityRequestFlags::UNSET,
            expires_at: 0,
            opportunistic: false,
            want_fresh_within: DEFAULT_VISIBILITY_TTL,
        }
    }
}

/// A room a scout repeatedly failed to reach, with an exponential retry
/// backoff. Persisted (world state) so a hostile-walled room is not
/// re-scouted forever and does not block the claim pipeline's "ring fully
/// scouted" gate. Observers are NOT suppressed — only scout servicing — so a
/// room an observer can still see recovers on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UnreachableRoom {
    pub room_name: RoomName,
    /// Earliest tick a scout may be dispatched here again.
    pub retry_after: u32,
    /// Consecutive scout-mission give-ups (drives the backoff).
    pub attempts: u32,
}

impl Default for UnreachableRoom {
    fn default() -> Self {
        Self {
            room_name: RoomName::new("E0N0").unwrap(),
            retry_after: 0,
            attempts: 0,
        }
    }
}

/// Persistent visibility queue. Serialized as a component on a singleton entity.
///
/// Contains only data that is meaningful across ticks and safe to serialize.
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisibilityQueueData {
    pub entries: Vec<VisibilityEntry>,
    /// Rooms scouts gave up on, with retry backoff. Distinct from `entries`
    /// (which TTL-expire) so the give-up memory survives across discovery
    /// cycles and VM resets.
    pub unreachable: Vec<UnreachableRoom>,
}

// ─── Runtime layer: VisibilityQueue (ephemeral resource) ─────────────────────

/// Snapshot entry for a single visibility request (for visualization).
#[derive(Debug, Clone)]
pub struct VisibilityQueueSnapshotEntry {
    pub room_name: RoomName,
    pub priority: f32,
    pub allowed_types: VisibilityRequestFlags,
}

/// Snapshot of the visibility queue taken each tick.
/// Used by the visualization system to display current visibility requests.
#[derive(Debug, Clone, Default)]
pub struct VisibilityQueueSnapshot {
    pub entries: Vec<VisibilityQueueSnapshotEntry>,
}

/// Runtime visibility queue resource. Holds a working copy of the persistent
/// entries (synced from/to the `VisibilityQueueData` component by the cleanup
/// and sync systems).
///
/// Callers interact with this resource only — they do not need direct access
/// to the `VisibilityQueueData` component. Fulfillment state (scout tours,
/// observer rotation) lives in [`crate::room::scoutassignment::ScoutAssignments`]
/// since ADR 0046 — the per-creep claim layer (`claimed_by`) is gone.
#[derive(Default)]
pub struct VisibilityQueue {
    /// Working copy of persistent entries. Synced from the component at tick
    /// start by `VisibilityQueueCleanupSystem` and written back by
    /// `VisibilityQueueSyncSystem`.
    pub entries: Vec<VisibilityEntry>,

    /// Working copy of the persisted scout give-up backoffs.
    pub unreachable: Vec<UnreachableRoom>,
}

/// Base backoff (ticks) after the first scout give-up; doubles per repeat.
const UNREACHABLE_BACKOFF_BASE: u32 = 2000;
/// Cap on the scout give-up backoff (one creep lifetime ≈ 1500; a derelict
/// owner / dead blocker changes the picture on a longer horizon).
const UNREACHABLE_BACKOFF_MAX: u32 = 20000;

impl VisibilityQueue {
    /// Upsert a visibility request. If an entry for the room already exists,
    /// merge priority upward, extend expiration, and tighten the freshness
    /// target downward.
    ///
    /// Merge rules (ADR 0046 D1 + design-review resolution #3):
    /// - `priority`: MAX (the most urgent producer wins),
    /// - `expires_at`: MAX (the longest-lived assertion wins),
    /// - `want_fresh_within`: MIN (the strictest freshness demand wins),
    /// - `opportunistic`: a non-opportunistic request upgrades an
    ///   opportunistic entry (clears the flag), but an opportunistic request
    ///   never downgrades a non-opportunistic entry.
    pub fn request(&mut self, request: VisibilityRequest) {
        let room_name = request.room_name;
        let priority = request.priority;
        let allowed_types = request.allowed_types;
        let opportunistic = request.opportunistic;
        let want_fresh_within = request.want_fresh_within;
        let expires_at = game::time() + DEFAULT_VISIBILITY_TTL;

        if let Some(existing) = self.entries.iter_mut().find(|e| e.room_name == room_name) {
            existing.priority = existing.priority.max(priority);
            existing.allowed_types |= allowed_types;
            existing.expires_at = existing.expires_at.max(expires_at);
            existing.want_fresh_within = existing.want_fresh_within.min(want_fresh_within);
            // A non-opportunistic request upgrades an opportunistic entry.
            if !opportunistic {
                existing.opportunistic = false;
            }
        } else {
            self.entries.push(VisibilityEntry {
                room_name,
                priority,
                allowed_types,
                expires_at,
                opportunistic,
                want_fresh_within,
            });
        }
    }

    /// Record a scout give-up for `room_name`: increment the attempt count and
    /// set an exponential retry backoff. Called by the `ScoutAssignmentSystem`
    /// on room-centric entry-failure evidence (ADR 0046 D2.4 as amended: a
    /// scout sat adjacent-but-outside for ~100 ticks while tasked with the
    /// room, or `find_route` reports the room map-disconnected). Suppresses
    /// scout servicing (not observers) until `retry_after`.
    pub fn mark_unreachable(&mut self, room_name: RoomName, now: u32) {
        if let Some(existing) = self.unreachable.iter_mut().find(|u| u.room_name == room_name) {
            existing.attempts = existing.attempts.saturating_add(1);
            let shift = existing.attempts.saturating_sub(1).min(31);
            let backoff = UNREACHABLE_BACKOFF_BASE.saturating_mul(1u32 << shift).min(UNREACHABLE_BACKOFF_MAX);
            existing.retry_after = now.saturating_add(backoff);
        } else {
            self.unreachable.push(UnreachableRoom {
                room_name,
                retry_after: now.saturating_add(UNREACHABLE_BACKOFF_BASE),
                attempts: 1,
            });
        }
    }

    /// Whether `room_name` is currently in scout-give-up backoff.
    pub fn is_unreachable_now(&self, room_name: RoomName, now: u32) -> bool {
        self.unreachable.iter().any(|u| u.room_name == room_name && u.retry_after > now)
    }

    /// Clear any give-up record for `room_name` — call when fresh visibility
    /// arrives, so a room that became reachable again is not suppressed.
    pub fn clear_unreachable(&mut self, room_name: RoomName) {
        self.unreachable.retain(|u| u.room_name != room_name);
    }

    /// Remove entries that have expired.
    pub fn expire(&mut self, current_tick: u32) {
        self.entries.retain(|e| e.expires_at > current_tick);
    }

    /// Check if a room has an entry in the queue.
    pub fn has_entry(&self, room_name: RoomName) -> bool {
        self.entries.iter().any(|e| e.room_name == room_name)
    }

    /// Load entries from the persistent component into the working copy.
    fn load_from(&mut self, data: &VisibilityQueueData) {
        self.entries = data.entries.clone();
        self.unreachable = data.unreachable.clone();
    }

    /// Write the working copy back to the persistent component.
    fn save_to(&self, data: &mut VisibilityQueueData) {
        data.entries = self.entries.clone();
        data.unreachable = self.unreachable.clone();
    }
}

/// Compute Chebyshev distance between two rooms. The tour cheapest-insertion
/// pass prices EVERY insertion delta with this metric × 50 ticks/hop
/// (ADR 0046 design-review resolution #1) — never `find_route`.
pub(crate) fn room_distance(a: RoomName, b: RoomName) -> u32 {
    let delta = a - b;
    delta.0.unsigned_abs().max(delta.1.unsigned_abs())
}

// ─── VisibilityRequest (input struct for VisibilityQueue::request) ───────────

/// Builder struct for pushing a visibility request into the
/// [`VisibilityQueue`]. Callers construct one via [`new`] or
/// [`new_opportunistic`] and pass it to [`VisibilityQueue::request`].
/// Producers that need non-default freshness declare it via
/// [`want_fresh_within`](Self::want_fresh_within) (ADR 0046 D6).
pub struct VisibilityRequest {
    room_name: RoomName,
    priority: f32,
    allowed_types: VisibilityRequestFlags,
    opportunistic: bool,
    want_fresh_within: u32,
}

impl VisibilityRequest {
    pub fn new(room_name: RoomName, priority: f32, allowed_types: VisibilityRequestFlags) -> Self {
        // Tripwire (IBEX-046): the queue comparator coalesces NaN to Equal;
        // assert finiteness where the priority is produced instead.
        debug_assert!(priority.is_finite(), "visibility request priority not finite: {priority}");

        Self {
            room_name,
            priority,
            allowed_types,
            opportunistic: false,
            want_fresh_within: DEFAULT_VISIBILITY_TTL,
        }
    }

    /// Create an opportunistic visibility request. These are only serviced by
    /// scouts that are already alive — they never count toward the scout
    /// fleet's spawn EV (ADR 0046 D5).
    pub fn new_opportunistic(room_name: RoomName, priority: f32, allowed_types: VisibilityRequestFlags) -> Self {
        debug_assert!(priority.is_finite(), "visibility request priority not finite: {priority}");

        Self {
            room_name,
            priority,
            allowed_types,
            opportunistic: true,
            want_fresh_within: DEFAULT_VISIBILITY_TTL,
        }
    }

    /// Declare the producer's freshness target: intel at or below this age
    /// (ticks) counts as SERVICED for this request (ADR 0046 D1/D6). `0` is an
    /// imperative force-visit (only a same-tick sighting satisfies it).
    pub fn want_fresh_within(mut self, ticks: u32) -> Self {
        self.want_fresh_within = ticks;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin (IBEX-046): non-finite priorities trip the debug assert at the
    /// request source in debug builds (tests/sim).
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "priority not finite")]
    fn visibility_request_rejects_non_finite_priority_in_debug() {
        let room: RoomName = "E0N0".parse().expect("valid room name");
        let _ = VisibilityRequest::new(room, f32::NAN, VisibilityRequestFlags::ALL);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "priority not finite")]
    fn opportunistic_visibility_request_rejects_non_finite_priority_in_debug() {
        let room: RoomName = "E0N0".parse().expect("valid room name");
        let _ = VisibilityRequest::new_opportunistic(room, f32::INFINITY, VisibilityRequestFlags::SCOUT);
    }

    // ── want_fresh_within upsert merge (ADR 0046 D1, review resolution #3) ──

    /// Pin: the upsert MIN-merges `want_fresh_within` (strictest freshness
    /// demand wins) while priority stays MAX-merged and expiry MAX-merged.
    ///
    /// NOTE this test drives `VisibilityEntry` directly (not through
    /// `VisibilityQueue::request`) because `request()` stamps `expires_at`
    /// from `game::time()`, which needs the JS runtime. The merge arms are
    /// exercised via a headless replica of the upsert body below, pinned
    /// against the real entry type so a field change breaks it loudly.
    #[test]
    fn upsert_min_merges_want_fresh_within_and_max_merges_priority() {
        let room: RoomName = "E1N1".parse().unwrap();

        // Existing entry: claim-candidate style (HIGH, fresh-within 250).
        let mut existing = VisibilityEntry {
            room_name: room,
            priority: VISIBILITY_PRIORITY_HIGH,
            allowed_types: VisibilityRequestFlags::ALL,
            expires_at: 1_100,
            opportunistic: false,
            want_fresh_within: 250,
        };

        // Incoming request: squad-manager style (MEDIUM, fresh-within 1).
        let incoming = VisibilityEntry {
            room_name: room,
            priority: VISIBILITY_PRIORITY_MEDIUM,
            allowed_types: VisibilityRequestFlags::OBSERVE,
            expires_at: 1_050,
            opportunistic: false,
            want_fresh_within: 1,
        };

        // The exact merge arms from `VisibilityQueue::request`.
        existing.priority = existing.priority.max(incoming.priority);
        existing.allowed_types |= incoming.allowed_types;
        existing.expires_at = existing.expires_at.max(incoming.expires_at);
        existing.want_fresh_within = existing.want_fresh_within.min(incoming.want_fresh_within);

        assert_eq!(existing.want_fresh_within, 1, "strictest freshness (MIN) wins");
        assert_eq!(existing.priority, VISIBILITY_PRIORITY_HIGH, "priority stays MAX-merged");
        assert_eq!(existing.expires_at, 1_100, "expiry stays MAX-merged");

        // And the reverse order: a lax request never loosens a strict entry.
        existing.want_fresh_within = existing.want_fresh_within.min(DEFAULT_VISIBILITY_TTL);
        assert_eq!(existing.want_fresh_within, 1, "a lax re-assert cannot loosen the target");
    }

    /// Pin: a request without an explicit declaration carries the default
    /// freshness target (one TTL), and the builder overrides it.
    #[test]
    fn visibility_request_defaults_and_declares_freshness() {
        let room: RoomName = "E2N2".parse().unwrap();
        let default_req = VisibilityRequest::new(room, VISIBILITY_PRIORITY_LOW, VisibilityRequestFlags::SCOUT);
        assert_eq!(default_req.want_fresh_within, DEFAULT_VISIBILITY_TTL);

        let strict = VisibilityRequest::new(room, VISIBILITY_PRIORITY_HIGH, VisibilityRequestFlags::ALL).want_fresh_within(0);
        assert_eq!(strict.want_fresh_within, 0, "builder declares an imperative force-visit");
    }

    // ── Scout give-up backoff (reachability) ────────────────────────────────

    #[test]
    fn mark_unreachable_sets_exponential_backoff() {
        let room: RoomName = "E5N5".parse().unwrap();
        let mut q = VisibilityQueue::default();

        // First give-up: base backoff, blocked now, free after it elapses.
        q.mark_unreachable(room, 1000);
        assert!(q.is_unreachable_now(room, 1000));
        assert!(q.is_unreachable_now(room, 1000 + UNREACHABLE_BACKOFF_BASE - 1));
        assert!(!q.is_unreachable_now(room, 1000 + UNREACHABLE_BACKOFF_BASE));

        // Second give-up doubles the backoff.
        q.mark_unreachable(room, 5000);
        assert!(q.is_unreachable_now(room, 5000 + UNREACHABLE_BACKOFF_BASE)); // > base now
        assert!(q.is_unreachable_now(room, 5000 + 2 * UNREACHABLE_BACKOFF_BASE - 1));
        assert!(!q.is_unreachable_now(room, 5000 + 2 * UNREACHABLE_BACKOFF_BASE));
    }

    #[test]
    fn unreachable_backoff_is_capped() {
        let room: RoomName = "E5N5".parse().unwrap();
        let mut q = VisibilityQueue::default();
        for _ in 0..20 {
            q.mark_unreachable(room, 0);
        }
        // Never exceeds the cap, no matter how many give-ups.
        assert!(q.is_unreachable_now(room, UNREACHABLE_BACKOFF_MAX - 1));
        assert!(!q.is_unreachable_now(room, UNREACHABLE_BACKOFF_MAX));
    }

    #[test]
    fn clear_unreachable_lifts_suppression() {
        let room: RoomName = "E5N5".parse().unwrap();
        let mut q = VisibilityQueue::default();
        q.mark_unreachable(room, 0);
        assert!(q.is_unreachable_now(room, 0));
        q.clear_unreachable(room);
        assert!(!q.is_unreachable_now(room, 0));
    }
}

// ─── VisibilityQueueCleanupSystem ────────────────────────────────────────────

/// Runs at the start of the main dispatcher (before operations).
/// Loads persistent data into the resource, expires stale entries, resets
/// per-tick flags, releases dead scout claims, and creates RoomData entities
/// for rooms that don't have one yet.
pub struct VisibilityQueueCleanupSystem;

#[derive(SystemData)]
pub struct VisibilityQueueCleanupSystemData<'a> {
    visibility_queue: Write<'a, VisibilityQueue>,
    visibility_data: WriteStorage<'a, VisibilityQueueData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for VisibilityQueueCleanupSystem {
    type SystemData = VisibilityQueueCleanupSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // Find or create the singleton VisibilityQueueData entity.
        let singleton = (&data.entities, &mut data.visibility_data).join().next().map(|(e, _)| e);
        if singleton.is_none() {
            // Create the singleton entity if it doesn't exist yet.
            data.updater
                .create_entity(&data.entities)
                .marked::<SerializeMarker>()
                .with(VisibilityQueueData::default())
                .build();
            // No data to load yet; the resource starts empty.
            return;
        }

        let singleton_entity = singleton.unwrap();
        let vq_data = data.visibility_data.get_mut(singleton_entity).unwrap();

        // Load persistent entries into the resource working copy.
        data.visibility_queue.load_from(vq_data);

        // Expire stale entries.
        data.visibility_queue.expire(game::time());

        // Create RoomData entities for rooms in the queue that don't have one yet.
        let existing_rooms: std::collections::HashSet<RoomName> = (&data.entities, &data.room_data).join().map(|(_, rd)| rd.name).collect();

        for entry in &data.visibility_queue.entries {
            if !existing_rooms.contains(&entry.room_name) {
                info!("Creating room data for room: {}", entry.room_name);
                data.updater
                    .create_entity(&data.entities)
                    .marked::<SerializeMarker>()
                    .with(RoomData::new(entry.room_name))
                    .build();
            }
        }
    }
}

// ─── VisibilityQueueSyncSystem ───────────────────────────────────────────────

/// Writes the resource working copy back to the persistent component.
/// Runs late in the dispatcher (after all systems have finished pushing
/// requests and before serialization).
pub struct VisibilityQueueSyncSystem;

#[derive(SystemData)]
pub struct VisibilityQueueSyncSystemData<'a> {
    visibility_queue: Read<'a, VisibilityQueue>,
    visibility_data: WriteStorage<'a, VisibilityQueueData>,
    entities: Entities<'a>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for VisibilityQueueSyncSystem {
    type SystemData = VisibilityQueueSyncSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let count = data.visibility_queue.entries.len();
        if let Some((_, vq_data)) = (&data.entities, &mut data.visibility_data).join().next() {
            data.visibility_queue.save_to(vq_data);
        } else if count > 0 {
            warn!(
                "VisibilityQueueSync: {} entries in resource but no singleton entity to write to",
                count
            );
        }
    }
}

// ─── ObserverSystem ──────────────────────────────────────────────────────────

/// Assigns observers to visibility queue entries after movement.
///
/// ADR 0046 D2.2 (F6 fix): entries whose intel is already fresh within their
/// `want_fresh_within` are skipped, and equal-priority entries rotate via
/// least-recently-observed-first (the `last_observed` map in
/// [`crate::room::scoutassignment::ScoutAssignments`]), with a deterministic
/// room-name tie-break — k observers no longer re-observe the same top-k
/// entries every tick.
pub struct ObserverSystem;

#[derive(SystemData)]
pub struct ObserverSystemData<'a> {
    visibility_queue: Write<'a, VisibilityQueue>,
    assignments: Write<'a, crate::room::scoutassignment::ScoutAssignments>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for ObserverSystem {
    type SystemData = ObserverSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if data.visibility_queue.entries.is_empty() {
            return;
        }

        let intel_age = |room: RoomName| -> u32 {
            data.mapping
                .get_room(&room)
                .and_then(|e| data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|dvd| dvd.age())
                .unwrap_or(u32::MAX)
        };

        // Collect OBSERVE entries that are NOT already fresh within their
        // declared target (the D1 freshness filter).
        let mut observe_entries: Vec<(RoomName, f32, u32)> = data
            .visibility_queue
            .entries
            .iter()
            .filter(|e| e.allowed_types.contains(VisibilityRequestFlags::OBSERVE))
            .filter(|e| intel_age(e.room_name) > e.want_fresh_within)
            .map(|e| {
                let last = data.assignments.last_observed.get(&e.room_name).copied().unwrap_or(0);
                (e.room_name, e.priority, last)
            })
            .collect();

        // Sort: priority descending, then least-recently-observed first, then
        // room name ascending (deterministic).
        observe_entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.0.cmp(&b.0))
        });

        // Gather available observers from home rooms.
        let mut home_room_observers: Vec<(RoomName, Vec<StructureObserver>)> = (&data.entities, &data.room_data)
            .join()
            .filter_map(|(_, room_data)| {
                let dvd = room_data.get_dynamic_visibility_data()?;
                if !dvd.owner().mine() {
                    return None;
                }
                let structures = room_data.get_structures()?;
                if structures.spawns().is_empty() {
                    return None;
                }
                let observers = structures.observers().to_vec();
                if observers.is_empty() {
                    return None;
                }
                Some((room_data.name, observers))
            })
            .collect();

        // Assign observers to entries.
        let now = game::time();
        for (room_name, _priority, _last) in &observe_entries {
            let observer = home_room_observers
                .iter_mut()
                .filter(|(_, obs)| !obs.is_empty())
                .map(|(home_name, obs)| {
                    let range = room_distance(*room_name, *home_name);
                    (home_name, obs, range)
                })
                .filter(|(_, _, range)| *range <= OBSERVER_RANGE)
                .min_by_key(|(_, _, range)| *range)
                .and_then(|(_, obs, _)| obs.pop());

            if let Some(observer) = observer {
                match observer.observe_room(*room_name) {
                    Ok(()) => {
                        data.assignments.last_observed.insert(*room_name, now);
                    }
                    Err(err) => info!("Failed to observe: {:?}", err),
                }
            }
        }
    }
}

// ─── VisibilityVisualizationSystem ───────────────────────────────────────────

/// Takes a snapshot of the visibility queue for the visualization panel.
/// Runs in the summarize phase.
pub struct VisibilityVisualizationSystem;

#[derive(SystemData)]
pub struct VisibilityVisualizationSystemData<'a> {
    visibility_queue: Read<'a, VisibilityQueue>,
    visibility_snapshot: Write<'a, VisibilityQueueSnapshot>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for VisibilityVisualizationSystem {
    type SystemData = VisibilityVisualizationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut snapshot_entries: Vec<VisibilityQueueSnapshotEntry> = data
            .visibility_queue
            .entries
            .iter()
            .map(|e| VisibilityQueueSnapshotEntry {
                room_name: e.room_name,
                priority: e.priority,
                allowed_types: e.allowed_types,
            })
            .collect();

        snapshot_entries.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal).reverse());

        data.visibility_snapshot.entries = snapshot_entries;
    }
}
