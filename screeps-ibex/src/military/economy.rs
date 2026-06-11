use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

use crate::room::data::*;

// ---------------------------------------------------------------------------
// SpawnQueueSnapshot
// ---------------------------------------------------------------------------

/// Snapshot of spawn queue state from the previous tick.
/// Written by `SpawnQueueSystem` before clearing, read by `EconomyAssessmentSystem`.
#[derive(Default)]
pub struct SpawnQueueSnapshot {
    /// Number of pending spawn requests per room entity (from previous tick).
    pub queue_depth_per_room: HashMap<Entity, u32>,
    /// Total pending spawn requests across all rooms (from previous tick).
    pub total_queue_depth: u32,
}

// ---------------------------------------------------------------------------
// RoomEconomyData
// ---------------------------------------------------------------------------

/// Per-room economy data, rebuilt each tick for visible owned rooms.
#[derive(Clone, Debug, Default)]
pub struct RoomEconomyData {
    /// Total stored energy across storage + terminal + containers.
    pub stored_energy: u32,
    /// Energy income per tick (sources * SOURCE_ENERGY_CAPACITY / ENERGY_REGEN_TIME).
    pub energy_income: f32,
    /// Current energy available for spawning (room.energy_available()).
    pub spawn_energy: u32,
    /// Max energy capacity for spawning (room.energy_capacity_available()).
    pub spawn_energy_capacity: u32,
    /// Number of spawns in this room.
    pub spawn_count: u32,
    /// Number of spawns currently idle (not actively spawning a creep).
    /// Derived from game state: spawn.spawning().is_none(). Always current.
    pub free_spawns: u32,
    /// Pending spawn requests from previous tick (one tick stale).
    /// Read from SpawnQueueSnapshot. Good enough for strategic decisions.
    pub prev_tick_queue_depth: u32,
    /// Number of military spawn slots claimed by operations/missions THIS tick.
    /// Reset to 0 by EconomyAssessmentSystem. Incremented cooperatively by
    /// operations (step 12) and missions (step 13) during the same tick.
    pub military_spawns_claimed: u32,
    /// Available boost compounds in labs/storage/terminal, keyed by ResourceType.
    /// Only tracked for military-relevant compounds (T3 boosts).
    pub available_boosts: HashMap<ResourceType, u32>,
}

// ---------------------------------------------------------------------------
// EconomySnapshot
// ---------------------------------------------------------------------------

/// Global economy snapshot aggregating across all owned rooms.
/// Rebuilt each tick (ephemeral -- not serialized).
#[derive(Default)]
pub struct EconomySnapshot {
    /// Per-room economy data, keyed by room entity.
    pub rooms: HashMap<Entity, RoomEconomyData>,
    /// Total stored energy across all owned rooms.
    pub total_stored_energy: u32,
    /// Total energy income per tick across all rooms.
    pub total_energy_income: f32,
    /// Total free spawns across all rooms.
    pub total_free_spawns: u32,
    /// Total spawn count across all rooms.
    pub total_spawn_count: u32,
    /// Number of owned rooms.
    pub room_count: u32,
}

impl EconomySnapshot {
    /// Can we afford to spend `amount` energy on military without
    /// dropping below a safety reserve?
    ///
    /// Uses a per-room reserve of 20% of stored energy (clamped to
    /// 5k–30k) so low-RCL rooms with little storage don't inflate
    /// the threshold, and mature rooms keep a reasonable buffer.
    pub fn can_afford_military(&self, amount: u32) -> bool {
        let reserve: u32 = self.rooms.values().map(|r| (r.stored_energy / 5).clamp(5_000, 30_000)).sum();
        self.total_stored_energy > reserve + amount
    }

    /// Can a specific set of rooms collectively afford `amount` energy
    /// for military spending? Each room contributes its surplus above a
    /// per-room reserve (20% of stored, clamped 5k–30k). This is the
    /// preferred check when the attack has assigned home rooms.
    pub fn can_rooms_afford_military(&self, rooms: &[Entity], amount: u32) -> bool {
        let surplus: u32 = rooms
            .iter()
            .filter_map(|e| self.rooms.get(e))
            .map(|r| {
                let reserve = (r.stored_energy / 5).clamp(5_000, 30_000);
                r.stored_energy.saturating_sub(reserve)
            })
            .sum();
        surplus >= amount
    }

    /// Return the total surplus energy available across specific rooms
    /// (stored minus per-room reserve). Useful for logging.
    pub fn rooms_surplus(&self, rooms: &[Entity]) -> u32 {
        rooms
            .iter()
            .filter_map(|e| self.rooms.get(e))
            .map(|r| {
                let reserve = (r.stored_energy / 5).clamp(5_000, 30_000);
                r.stored_energy.saturating_sub(reserve)
            })
            .sum()
    }

    /// Get mutable room data for within-tick coordination
    /// (incrementing military_spawns_claimed).
    pub fn room_mut(&mut self, entity: &Entity) -> Option<&mut RoomEconomyData> {
        self.rooms.get_mut(entity)
    }

    /// Get room data (read-only).
    pub fn room(&self, entity: &Entity) -> Option<&RoomEconomyData> {
        self.rooms.get(entity)
    }

    /// Check if a specific boost compound is available in sufficient quantity.
    pub fn has_boost(&self, compound: ResourceType, amount: u32) -> bool {
        self.rooms
            .values()
            .any(|r| r.available_boosts.get(&compound).copied().unwrap_or(0) >= amount)
    }

    /// Total available amount of a boost compound across all rooms.
    pub fn total_boost(&self, compound: ResourceType) -> u32 {
        self.rooms
            .values()
            .map(|r| r.available_boosts.get(&compound).copied().unwrap_or(0))
            .sum()
    }

    /// Maximum spawn energy capacity across all rooms.
    pub fn max_spawn_capacity(&self) -> u32 {
        self.rooms.values().map(|r| r.spawn_energy_capacity).max().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// RoomRouteCache
// ---------------------------------------------------------------------------

/// Cached route data for a room-to-room path.
#[derive(Clone, Debug)]
pub struct CachedRoute {
    /// Number of room hops in the route (route.len()).
    pub hops: u32,
    /// Estimated travel time in ticks (hops * 50).
    pub travel_ticks: u32,
    /// Game tick when this cache entry was created.
    pub cached_at: u32,
    /// Whether the route was found (false = no path).
    pub reachable: bool,
}

/// Cache of room-to-room route distances.
/// Ephemeral resource -- survives within a VM lifecycle but not across resets.
/// Entries are lazily populated and expire after a TTL.
#[derive(Default)]
pub struct RoomRouteCache {
    /// Cached routes keyed by (from, to) room pair.
    routes: HashMap<(RoomName, RoomName), CachedRoute>,
    /// TTL for cache entries in ticks. Room exits are static, but room
    /// costs change when ownership changes. 1000 ticks (~16 minutes) is
    /// a reasonable TTL -- ownership changes are infrequent.
    ttl: u32,
}

impl RoomRouteCache {
    pub fn new() -> Self {
        RoomRouteCache {
            routes: HashMap::new(),
            ttl: 1000,
        }
    }

    /// Get the cached route distance, or compute and cache it.
    ///
    /// Bucket-guarded (P1.B1 / ADR 0004 step 1): under a Critical
    /// governor tier, TTL-expired entries are served STALE instead of
    /// recomputed — `find_route` storms are exactly the leak the
    /// governor exists to stop, and a stale route (ownership changes
    /// are slow) is strictly better than a fabricated answer. Missing
    /// entries still compute at any tier: callers need SOME answer,
    /// and a single find_route is not the storm.
    pub fn get_route_distance(&mut self, from: RoomName, to: RoomName, current_tick: u32) -> &CachedRoute {
        let ttl = self.ttl;

        let entry = self.routes.get(&(from, to));
        let missing = entry.is_none();
        let expired = entry
            .map(|entry| current_tick.saturating_sub(entry.cached_at) > ttl)
            .unwrap_or(false);

        if should_recompute_route(missing, expired, crate::cpugovernor::tier()) {
            // P1.B4: charge the mission ops pool (find_route has no ops
            // parameter — nominal accounting + admission control).
            // Missing entries compute even on a zero grant — callers
            // need SOME answer; only TTL refreshes yield to the pool.
            let granted = crate::pathbudget::take(crate::pathbudget::FIND_ROUTE_NOMINAL_OPS);
            if missing || granted > 0 {
                let route = Self::compute_route(from, to, current_tick);
                self.routes.insert((from, to), route);
            }
        }

        self.routes.get(&(from, to)).unwrap()
    }

    fn compute_route(from: RoomName, to: RoomName, tick: u32) -> CachedRoute {
        if from == to {
            return CachedRoute {
                hops: 0,
                travel_ticks: 0,
                cached_at: tick,
                reachable: true,
            };
        }

        // Use find_route with a room cost callback that avoids hostile rooms.
        let options = game::map::FindRouteOptions::new().room_callback(|room_name, _from_room| {
            // High cost for hostile rooms, normal for others.
            // Closed rooms are handled internally by find_route.
            if let Some(room) = game::rooms().get(room_name) {
                if let Some(controller) = room.controller() {
                    if controller.my() {
                        return 1.0;
                    }
                    if controller.owner().is_some() {
                        // Owned by someone else -- high cost to avoid.
                        return 10.0;
                    }
                    if controller.reservation().is_some() {
                        return 2.0;
                    }
                }
            }
            // Default cost for unknown/neutral rooms.
            2.0
        });

        match game::map::find_route(from, to, Some(options)) {
            Ok(steps) => {
                let hops = steps.len() as u32;
                CachedRoute {
                    hops,
                    travel_ticks: hops * 50,
                    cached_at: tick,
                    reachable: true,
                }
            }
            Err(_) => CachedRoute {
                hops: u32::MAX,
                travel_ticks: u32::MAX,
                cached_at: tick,
                reachable: false,
            },
        }
    }

    /// Invalidate all cached routes involving a specific room.
    /// Call when a room's disposition changes (ownership, hostility).
    pub fn invalidate_room(&mut self, room: RoomName) {
        self.routes.retain(|(from, to), _| *from != room && *to != room);
    }

    /// Convenience: estimated travel ticks, or None if unreachable.
    pub fn travel_ticks(&mut self, from: RoomName, to: RoomName, current_tick: u32) -> Option<u32> {
        let entry = self.get_route_distance(from, to, current_tick);
        if entry.reachable {
            Some(entry.travel_ticks)
        } else {
            None
        }
    }
}

/// Pure recompute decision for [`RoomRouteCache::get_route_distance`]
/// (P1.B1 bucket-guard): missing entries always compute; expired
/// entries recompute except under Critical, where stale is served.
fn should_recompute_route(missing: bool, expired: bool, tier: crate::cpugovernor::Tier) -> bool {
    missing || (expired && tier != crate::cpugovernor::Tier::Critical)
}

// ---------------------------------------------------------------------------
// EconomyAssessmentSystem
// ---------------------------------------------------------------------------

/// Runs in the pre-pass after ThreatAssessmentSystem, before operations.
/// Rebuilds the EconomySnapshot from game state each tick.
pub struct EconomyAssessmentSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for EconomyAssessmentSystem {
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, RoomData>,
        Write<'a, EconomySnapshot>,
        Read<'a, SpawnQueueSnapshot>,
    );

    fn run(&mut self, (entities, room_data, mut economy, spawn_snapshot): Self::SystemData) {
        // Reset the snapshot.
        economy.rooms.clear();
        economy.total_stored_energy = 0;
        economy.total_energy_income = 0.0;
        economy.total_free_spawns = 0;
        economy.total_spawn_count = 0;
        economy.room_count = 0;

        for (entity, room) in (&entities, &room_data).join() {
            // Only process owned rooms.
            let is_mine = room.get_dynamic_visibility_data().map(|d| d.owner().mine()).unwrap_or(false);
            if !is_mine {
                continue;
            }

            let game_room = match game::rooms().get(room.name) {
                Some(r) => r,
                None => continue, // Not visible this tick.
            };

            // Gather structure-derived data.
            let mut stored_energy: u32 = 0;
            let mut spawn_count: u32 = 0;
            let mut free_spawns: u32 = 0;

            if let Some(structures) = room.get_structures() {
                if let Some(storage) = structures.storages().first() {
                    stored_energy += storage.store().get_used_capacity(Some(ResourceType::Energy));
                }
                if let Some(terminal) = structures.terminals().first() {
                    stored_energy += terminal.store().get_used_capacity(Some(ResourceType::Energy));
                }
                for container in structures.containers() {
                    stored_energy += container.store().get_used_capacity(Some(ResourceType::Energy));
                }
                for spawn in structures.spawns() {
                    spawn_count += 1;
                    if spawn.spawning().is_none() {
                        free_spawns += 1;
                    }
                }
            }

            // Energy income estimate from sources.
            let energy_income = room
                .get_static_visibility_data()
                .map(|d| d.sources().len() as f32 * (3000.0 / 300.0))
                .unwrap_or(0.0);

            let prev_tick_queue_depth = spawn_snapshot.queue_depth_per_room.get(&entity).copied().unwrap_or(0);

            let room_econ = RoomEconomyData {
                stored_energy,
                energy_income,
                spawn_energy: game_room.energy_available(),
                spawn_energy_capacity: game_room.energy_capacity_available(),
                spawn_count,
                free_spawns,
                prev_tick_queue_depth,
                military_spawns_claimed: 0,
                available_boosts: HashMap::new(),
            };

            // Aggregate totals.
            economy.total_stored_energy += room_econ.stored_energy;
            economy.total_energy_income += room_econ.energy_income;
            economy.total_free_spawns += room_econ.free_spawns;
            economy.total_spawn_count += room_econ.spawn_count;
            economy.room_count += 1;

            economy.rooms.insert(entity, room_econ);
        }
    }
}

#[cfg(test)]
mod route_guard_tests {
    use super::should_recompute_route;
    use crate::cpugovernor::Tier;

    #[test]
    fn stale_routes_are_served_under_critical_only() {
        for tier in [Tier::Normal, Tier::Conserve, Tier::Critical] {
            // Missing entries always compute — a single find_route is
            // not the storm the guard exists to stop.
            assert!(should_recompute_route(true, false, tier), "{tier:?}");
            // Fresh entries never recompute.
            assert!(!should_recompute_route(false, false, tier), "{tier:?}");
        }
        // Expired: recompute normally, serve stale under Critical.
        assert!(should_recompute_route(false, true, Tier::Normal));
        assert!(should_recompute_route(false, true, Tier::Conserve));
        assert!(!should_recompute_route(false, true, Tier::Critical));
    }
}
