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
use std::collections::HashMap;
use std::fmt;

pub const VISIBILITY_PRIORITY_CRITICAL: f32 = 100.0;
pub const VISIBILITY_PRIORITY_HIGH: f32 = 75.0;
pub const VISIBILITY_PRIORITY_MEDIUM: f32 = 50.0;
pub const VISIBILITY_PRIORITY_LOW: f32 = 25.0;
pub const VISIBILITY_PRIORITY_NONE: f32 = 0.0;

/// Default TTL for visibility requests (in ticks). Must be longer than the
/// longest interval between re-requests (e.g. mining outpost pushes every 50
/// ticks, so 100 gives a comfortable margin).
const DEFAULT_VISIBILITY_TTL: u32 = 100;

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
    /// alive. The `ScoutOperation` will not spawn new missions for it.
    pub opportunistic: bool,
}

impl Default for VisibilityEntry {
    fn default() -> Self {
        Self {
            room_name: RoomName::new("E0N0").unwrap(),
            priority: 0.0,
            allowed_types: VisibilityRequestFlags::UNSET,
            expires_at: 0,
            opportunistic: false,
        }
    }
}

/// Persistent visibility queue. Serialized as a component on a singleton entity.
///
/// Contains only data that is meaningful across ticks and safe to serialize.
/// Ephemeral per-tick state (observer_serviced, claimed_by) lives in the
/// [`VisibilityQueue`] resource instead.
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisibilityQueueData {
    pub entries: Vec<VisibilityEntry>,
}

// ─── Runtime layer: VisibilityQueue (ephemeral resource) ─────────────────────

/// Per-tick runtime state for a single visibility entry.
#[derive(Debug, Clone, Default)]
pub struct VisibilityRuntimeEntry {
    /// Whether an observer serviced this room this tick (reset each tick).
    pub observer_serviced: bool,
    /// Scout creep entity currently moving toward this room (if any).
    pub claimed_by: Option<Entity>,
}

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
/// and sync systems) plus ephemeral per-tick state.
///
/// Callers interact with this resource only — they do not need direct access
/// to the `VisibilityQueueData` component.
#[derive(Default)]
pub struct VisibilityQueue {
    /// Working copy of persistent entries. Synced from the component at tick
    /// start by `VisibilityQueueCleanupSystem` and written back by
    /// `VisibilityQueueSyncSystem`.
    pub entries: Vec<VisibilityEntry>,

    /// Per-tick runtime state, keyed by room name.
    pub runtime: HashMap<RoomName, VisibilityRuntimeEntry>,
}

impl VisibilityQueue {
    /// Upsert a visibility request. If an entry for the room already exists,
    /// merge priority upward and extend expiration. Also ensures a runtime
    /// entry exists.
    ///
    /// The `opportunistic` flag is merged conservatively: a non-opportunistic
    /// request upgrades an opportunistic entry (clears the flag), but an
    /// opportunistic request never downgrades a non-opportunistic entry.
    pub fn request(&mut self, request: VisibilityRequest) {
        let room_name = request.room_name;
        let priority = request.priority;
        let allowed_types = request.allowed_types;
        let opportunistic = request.opportunistic;
        let expires_at = game::time() + DEFAULT_VISIBILITY_TTL;

        if let Some(existing) = self.entries.iter_mut().find(|e| e.room_name == room_name) {
            existing.priority = existing.priority.max(priority);
            existing.allowed_types |= allowed_types;
            existing.expires_at = existing.expires_at.max(expires_at);
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
            });
        }

        self.runtime.entry(room_name).or_default();
    }

    /// Mark a room as claimed by a scout creep entity.
    pub fn claim(&mut self, room_name: RoomName, creep_entity: Entity) {
        self.runtime.entry(room_name).or_default().claimed_by = Some(creep_entity);
    }

    /// Release all entries claimed by the given entity.
    pub fn release_entity(&mut self, creep_entity: Entity) {
        for entry in self.runtime.values_mut() {
            if entry.claimed_by == Some(creep_entity) {
                entry.claimed_by = None;
            }
        }
    }

    /// Mark a room as serviced by an observer this tick.
    pub fn mark_observer_serviced(&mut self, room_name: RoomName) {
        self.runtime.entry(room_name).or_default().observer_serviced = true;
    }

    /// Remove entries that have expired.
    pub fn expire(&mut self, current_tick: u32) {
        self.entries.retain(|e| e.expires_at > current_tick);
        // Clean up runtime entries for rooms no longer in the persistent data.
        let rooms: std::collections::HashSet<RoomName> = self.entries.iter().map(|e| e.room_name).collect();
        self.runtime.retain(|name, _| rooms.contains(name));
    }

    /// Clear per-tick flags (observer_serviced). Called at the start of each tick.
    pub fn reset_tick_flags(&mut self) {
        for entry in self.runtime.values_mut() {
            entry.observer_serviced = false;
        }
    }

    /// Release claims for entities that are no longer alive.
    pub fn release_dead(&mut self, entities: &Entities) {
        for entry in self.runtime.values_mut() {
            if let Some(e) = entry.claimed_by {
                if !entities.is_alive(e) {
                    entry.claimed_by = None;
                }
            }
        }
    }

    /// Check if a room has an entry in the queue.
    pub fn has_entry(&self, room_name: RoomName) -> bool {
        self.entries.iter().any(|e| e.room_name == room_name)
    }

    /// Find the best unclaimed, non-observer-serviced entry with SCOUT flag,
    /// preferring highest priority then closest distance to `creep_pos`.
    pub fn best_unclaimed_for(&self, creep_pos: Position) -> Option<RoomName> {
        let creep_room = creep_pos.room_name();

        self.entries
            .iter()
            .filter(|e| e.allowed_types.contains(VisibilityRequestFlags::SCOUT))
            .filter(|e| {
                let rt = self.runtime.get(&e.room_name);
                let claimed = rt.map(|r| r.claimed_by.is_some()).unwrap_or(false);
                let observed = rt.map(|r| r.observer_serviced).unwrap_or(false);
                !claimed && !observed
            })
            .max_by(|a, b| {
                let dist_a = room_distance(creep_room, a.room_name);
                let dist_b = room_distance(creep_room, b.room_name);
                a.priority
                    .partial_cmp(&b.priority)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| dist_b.cmp(&dist_a)) // prefer closer
            })
            .map(|e| e.room_name)
    }

    /// Check if there are any unclaimed, non-opportunistic, scout-eligible entries.
    ///
    /// Opportunistic entries (created by idle scouts for proactive exploration)
    /// are excluded — they should not trigger new scout mission spawns.
    pub fn has_unclaimed_scout_eligible(&self) -> bool {
        self.entries.iter().any(|e| {
            e.allowed_types.contains(VisibilityRequestFlags::SCOUT) && !e.opportunistic && {
                let rt = self.runtime.get(&e.room_name);
                let claimed = rt.map(|r| r.claimed_by.is_some()).unwrap_or(false);
                !claimed
            }
        })
    }

    /// Load entries from the persistent component into the working copy.
    fn load_from(&mut self, data: &VisibilityQueueData) {
        self.entries = data.entries.clone();
        // Ensure runtime entries exist for all persistent entries.
        for entry in &self.entries {
            self.runtime.entry(entry.room_name).or_default();
        }
    }

    /// Write the working copy back to the persistent component.
    fn save_to(&self, data: &mut VisibilityQueueData) {
        data.entries = self.entries.clone();
    }
}

/// Compute Chebyshev distance between two rooms.
fn room_distance(a: RoomName, b: RoomName) -> u32 {
    let delta = a - b;
    delta.0.unsigned_abs().max(delta.1.unsigned_abs())
}

// ─── Legacy VisibilityRequest (kept for caller convenience) ──────────────────

pub struct VisibilityRequest {
    room_name: RoomName,
    priority: f32,
    allowed_types: VisibilityRequestFlags,
    opportunistic: bool,
}

impl VisibilityRequest {
    pub fn new(room_name: RoomName, priority: f32, allowed_types: VisibilityRequestFlags) -> VisibilityRequest {
        VisibilityRequest {
            room_name,
            priority,
            allowed_types,
            opportunistic: false,
        }
    }

    /// Create an opportunistic visibility request. These are only serviced by
    /// scouts that are already alive — the `ScoutOperation` will not spawn new
    /// missions for them.
    pub fn new_opportunistic(room_name: RoomName, priority: f32, allowed_types: VisibilityRequestFlags) -> VisibilityRequest {
        VisibilityRequest {
            room_name,
            priority,
            allowed_types,
            opportunistic: true,
        }
    }

    pub fn room_name(&self) -> RoomName {
        self.room_name
    }

    pub fn priority(&self) -> f32 {
        self.priority
    }

    pub fn allowed_types(&self) -> VisibilityRequestFlags {
        self.allowed_types
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

        // Reset per-tick flags.
        data.visibility_queue.reset_tick_flags();

        // Release claims for dead scout creeps.
        data.visibility_queue.release_dead(&data.entities);

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
/// Priority: rooms without assigned scouts first, then remaining requests.
pub struct ObserverSystem;

#[derive(SystemData)]
pub struct ObserverSystemData<'a> {
    visibility_queue: Write<'a, VisibilityQueue>,
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

        // Collect entries that want OBSERVE.
        let mut observe_entries: Vec<(RoomName, f32, bool)> = data
            .visibility_queue
            .entries
            .iter()
            .filter(|e| e.allowed_types.contains(VisibilityRequestFlags::OBSERVE))
            .map(|e| {
                let claimed = data
                    .visibility_queue
                    .runtime
                    .get(&e.room_name)
                    .map(|r| r.claimed_by.is_some())
                    .unwrap_or(false);
                (e.room_name, e.priority, claimed)
            })
            .collect();

        // Sort: unclaimed first, then by priority descending.
        observe_entries.sort_by(|a, b| {
            a.2.cmp(&b.2) // false (unclaimed) < true (claimed) — unclaimed first
                .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).reverse())
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
        for (room_name, _priority, _claimed) in &observe_entries {
            let observer = home_room_observers
                .iter_mut()
                .filter(|(_, obs)| !obs.is_empty())
                .map(|(home_name, obs)| {
                    let delta = *room_name - *home_name;
                    let range = delta.0.abs().max(delta.1.abs()) as u32;
                    (home_name, obs, range)
                })
                .filter(|(_, _, range)| *range <= OBSERVER_RANGE)
                .min_by_key(|(_, _, range)| *range)
                .and_then(|(_, obs, _)| obs.pop());

            if let Some(observer) = observer {
                match observer.observe_room(*room_name) {
                    Ok(()) => {
                        data.visibility_queue.mark_observer_serviced(*room_name);
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
