//! Per-room stats history with downsampled tiers.
//!
//! Data is persisted in a dedicated RawMemory segment so it survives VM restarts.
//! Missed ticks (e.g. bot crash) are handled gracefully via absolute tick comparisons.

use crate::memorysystem::MemoryArbiter;
use crate::room::data::RoomData;
use crate::serialize;
use crate::visualization::VisualizationData;
use log::*;
use screeps::{game, RoomName};
use serde::{Deserialize, Serialize};
use specs::prelude::*;
use std::collections::{HashMap, VecDeque};

/// Dedicated segment for stats history persistence.
pub const STATS_HISTORY_SEGMENT: u32 = 56;

// ─── Tier configuration ──────────────────────────────────────────────────────

const RECENT_CAP: usize = 60;
const MINUTE_CAP: usize = 60;
const TEN_MIN_CAP: usize = 60;
const HOUR_CAP: usize = 24;
const DAY_CAP: usize = 7;

/// Minimum ticks between downsample cascade steps.
const MINUTE_INTERVAL: u32 = 6;
const TEN_MIN_INTERVAL: u32 = 60;
const HOUR_INTERVAL: u32 = 360;
const DAY_INTERVAL: u32 = 8640;

// ─── Data types ──────────────────────────────────────────────────────────────

/// One snapshot of room storage totals at a point in time.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct RoomStatsSnapshot {
    pub tick: u32,
    pub energy: u32,
    pub minerals_total: u32,
}

/// Downsampled tiers for one room.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct RoomStatsHistory {
    pub recent: VecDeque<RoomStatsSnapshot>,
    pub minute: VecDeque<RoomStatsSnapshot>,
    pub ten_min: VecDeque<RoomStatsSnapshot>,
    pub hour: VecDeque<RoomStatsSnapshot>,
    pub day: VecDeque<RoomStatsSnapshot>,
    pub last_minute_tick: u32,
    pub last_ten_min_tick: u32,
    pub last_hour_tick: u32,
    pub last_day_tick: u32,
}

impl RoomStatsHistory {
    /// Push a new snapshot to recent and run the downsample cascade.
    pub fn push(&mut self, snapshot: RoomStatsSnapshot) {
        let tick = snapshot.tick;

        self.recent.push_back(snapshot);
        if self.recent.len() > RECENT_CAP {
            self.recent.pop_front();
        }

        self.cascade(tick);
    }

    fn cascade(&mut self, tick: u32) {
        if tick.wrapping_sub(self.last_minute_tick) >= MINUTE_INTERVAL {
            if let Some(avg) = Self::average_tier(&self.recent, tick) {
                self.minute.push_back(avg);
                if self.minute.len() > MINUTE_CAP {
                    self.minute.pop_front();
                }
            }
            self.last_minute_tick = tick;
        }

        if tick.wrapping_sub(self.last_ten_min_tick) >= TEN_MIN_INTERVAL {
            if let Some(avg) = Self::average_tier(&self.minute, tick) {
                self.ten_min.push_back(avg);
                if self.ten_min.len() > TEN_MIN_CAP {
                    self.ten_min.pop_front();
                }
            }
            self.last_ten_min_tick = tick;
        }

        if tick.wrapping_sub(self.last_hour_tick) >= HOUR_INTERVAL {
            if let Some(avg) = Self::average_tier(&self.ten_min, tick) {
                self.hour.push_back(avg);
                if self.hour.len() > HOUR_CAP {
                    self.hour.pop_front();
                }
            }
            self.last_hour_tick = tick;
        }

        if tick.wrapping_sub(self.last_day_tick) >= DAY_INTERVAL {
            if let Some(avg) = Self::average_tier(&self.hour, tick) {
                self.day.push_back(avg);
                if self.day.len() > DAY_CAP {
                    self.day.pop_front();
                }
            }
            self.last_day_tick = tick;
        }
    }

    fn average_tier(tier: &VecDeque<RoomStatsSnapshot>, tick: u32) -> Option<RoomStatsSnapshot> {
        if tier.is_empty() {
            return None;
        }
        let n = tier.len() as u64;
        let energy: u64 = tier.iter().map(|s| s.energy as u64).sum();
        let minerals: u64 = tier.iter().map(|s| s.minerals_total as u64).sum();
        Some(RoomStatsSnapshot {
            tick,
            energy: (energy / n) as u32,
            minerals_total: (minerals / n) as u32,
        })
    }
}

/// Global resource: per-room stats history. Persisted in segment.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct StatsHistoryData {
    pub rooms: HashMap<RoomName, RoomStatsHistory>,
}

// ─── Segment load callback ───────────────────────────────────────────────────

/// Load `StatsHistoryData` from its segment and insert it into the world.
/// Called by the `MemoryArbiter` on_load callback when the segment first becomes
/// active. Note: `MemoryArbiter` is temporarily removed from the world during
/// callbacks, so we read the segment directly via `raw_memory`.
pub fn load_stats_history(world: &mut World) {
    let history = screeps::raw_memory::segments()
        .get(STATS_HISTORY_SEGMENT as u8)
        .and_then(|raw| {
            if raw.is_empty() {
                None
            } else {
                match crate::serialize::decode_from_string::<StatsHistoryData>(&raw) {
                    Ok(data) => Some(data),
                    Err(e) => {
                        warn!("Failed to decode stats history, using default: {}", e);
                        None
                    }
                }
            }
        })
        .unwrap_or_default();
    world.insert(history);
}

// ─── System ──────────────────────────────────────────────────────────────────

#[derive(SystemData)]
pub struct StatsHistorySystemData<'a> {
    viz_gate: Option<Read<'a, VisualizationData>>,
    stats_history: Option<Write<'a, StatsHistoryData>>,
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
}

pub struct StatsHistorySystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for StatsHistorySystem {
    type SystemData = StatsHistorySystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // Only run when visualization is on (VisualizationData is inserted).
        if data.viz_gate.is_none() {
            return;
        }

        let Some(ref mut history) = data.stats_history else {
            return;
        };

        let tick = game::time();

        // Collect snapshots for each owned, visible room.
        for (_entity, room_data) in (&data.entities, &data.room_data).join() {
            let dyn_vis = match room_data.get_dynamic_visibility_data() {
                Some(v) if v.visible() && v.owner().mine() => v,
                _ => continue,
            };
            let _ = dyn_vis; // used only for the filter above

            if game::rooms().get(room_data.name).is_none() {
                continue;
            }

            let structures = match room_data.get_structures() {
                Some(s) => s,
                None => continue,
            };

            let mut energy: u32 = 0;
            let mut minerals_total: u32 = 0;

            for structure in structures.all().iter() {
                if let Some(store) = structure.as_has_store() {
                    for resource_type in store.store().store_types() {
                        let amount = store.store().get(resource_type).unwrap_or(0);
                        if resource_type == screeps::ResourceType::Energy {
                            energy = energy.saturating_add(amount);
                        } else {
                            minerals_total = minerals_total.saturating_add(amount);
                        }
                    }
                }
            }

            let snapshot = RoomStatsSnapshot {
                tick,
                energy,
                minerals_total,
            };

            history
                .rooms
                .entry(room_data.name)
                .or_default()
                .push(snapshot);
        }

        // Persist to segment (write every 6 ticks to reduce overhead).
        if tick.is_multiple_of(6) {
            match serialize::encode_to_string(&**history) {
                Ok(encoded) => {
                    data.memory_arbiter.set(STATS_HISTORY_SEGMENT, &encoded);
                }
                Err(e) => {
                    warn!("Failed to encode stats history: {}", e);
                }
            }
        }
    }
}
