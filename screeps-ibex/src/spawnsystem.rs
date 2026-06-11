use crate::creep::CreepOwner;
use crate::military::economy::{EconomySnapshot, SpawnQueueSnapshot};
use crate::room::data::*;
use log::*;
use screeps::action_error_codes::SpawnCreepErrorCode;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;

/// Ticks per body part for spawn duration (Screeps constant).
const CREEP_SPAWN_TIME: u32 = 3;
/// Minimum stored energy (per room) to allow renewal.
const RENEW_MIN_ROOM_ENERGY: u32 = 10_000;

pub const SPAWN_PRIORITY_CRITICAL: f32 = 100.0;
pub const SPAWN_PRIORITY_HIGH: f32 = 75.0;
pub const SPAWN_PRIORITY_MEDIUM: f32 = 50.0;
pub const SPAWN_PRIORITY_LOW: f32 = 25.0;
pub const SPAWN_PRIORITY_NONE: f32 = 0.0;

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct SpawnToken(u32);

pub struct SpawnRequest {
    description: String,
    body: Vec<Part>,
    priority: f32,
    token: Option<SpawnToken>,
    callback: SpawnQueueCallback,
}

impl SpawnRequest {
    pub fn new(description: String, body: &[Part], priority: f32, token: Option<SpawnToken>, callback: SpawnQueueCallback) -> SpawnRequest {
        // Tripwire (IBEX-046): the queue comparator coalesces NaN to Equal;
        // assert finiteness where the priority is produced instead.
        debug_assert!(priority.is_finite(), "spawn request priority not finite: {priority}");

        SpawnRequest {
            description,
            body: body.to_vec(),
            priority,
            token,
            callback,
        }
    }

    pub fn cost(&self) -> u32 {
        self.body.iter().map(|p| p.cost()).sum()
    }

    pub fn priority(&self) -> f32 {
        self.priority
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

/// Ephemeral renew request for one creep in a room. Cleared when queue is processed.
#[derive(Clone, Debug)]
pub struct RenewRequest {
    pub creep_entity: Entity,
    pub ticks_to_live: u32,
}

#[derive(Default)]
pub struct SpawnQueue {
    next_token: u32,
    requests: HashMap<Entity, Vec<SpawnRequest>>,
    /// Per-room renew requests; ephemeral, cleared when queue is processed.
    renew_requests: HashMap<Entity, Vec<RenewRequest>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SpawnQueue {
    pub fn token(&mut self) -> SpawnToken {
        let token = SpawnToken(self.next_token);

        self.next_token += 1;

        token
    }

    pub fn request(&mut self, room: Entity, spawn_request: SpawnRequest) {
        let requests = self.requests.entry(room).or_default();

        let pos = requests
            .binary_search_by(|probe| {
                spawn_request
                    .priority
                    .partial_cmp(&probe.priority)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or_else(|e| e);

        requests.insert(pos, spawn_request);
    }

    /// Submit a renew request for a creep in the given room. Ephemeral; cleared when queue is processed.
    pub fn request_renew(&mut self, room: Entity, creep_entity: Entity, ticks_to_live: u32) {
        self.renew_requests.entry(room).or_default().push(RenewRequest {
            creep_entity,
            ticks_to_live,
        });
    }

    pub fn clear(&mut self) {
        self.next_token = 0;
        self.requests.clear();
        self.renew_requests.clear();
    }

    /// Iterate over (room_entity, requests) for visualization/gather systems.
    pub fn iter_requests(&self) -> std::collections::hash_map::Iter<'_, Entity, Vec<SpawnRequest>> {
        self.requests.iter()
    }
}

#[derive(SystemData)]
pub struct SpawnQueueSystemData<'a> {
    spawn_queue: Write<'a, SpawnQueue>,
    spawn_queue_snapshot: Write<'a, SpawnQueueSnapshot>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    economy: Read<'a, EconomySnapshot>,
}

pub struct SpawnQueueExecutionSystemData<'a, 'b> {
    pub updater: &'b Read<'a, LazyUpdate>,
}

/// Callback invoked when a spawn request completes; used to avoid repeating the long type.
pub type SpawnQueueCallback = Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)>;

pub struct SpawnQueueSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SpawnQueueSystem {
    fn spawn_creep(spawn: &StructureSpawn, parts: &[Part]) -> Result<String, SpawnCreepErrorCode> {
        let time = screeps::game::time();
        let mut additional = 0;
        loop {
            let name = format!("{}-{}", time, additional);
            match spawn.spawn_creep(parts, &name) {
                Ok(()) => return Ok(name),
                Err(e) => {
                    if e == SpawnCreepErrorCode::NameExists {
                        additional += 1;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Compute spawn duration in ticks for the next spawn request (body parts * CREEP_SPAWN_TIME).
    fn next_spawn_duration_ticks(requests: &[SpawnRequest], spawned_tokens: &HashSet<SpawnToken>) -> u32 {
        for req in requests {
            if req.token.map(|t| !spawned_tokens.contains(&t)).unwrap_or(true) {
                return (req.body.len() as u32).saturating_mul(CREEP_SPAWN_TIME);
            }
        }
        0
    }

    fn process_room_spawns(
        data: &SpawnQueueSystemData,
        room_entity: Entity,
        requests: &[SpawnRequest],
        renew_requests: &[RenewRequest],
        spawned_tokens: &mut HashSet<SpawnToken>,
    ) -> Result<(), String> {
        let room_data = data.room_data.get(room_entity).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let structures = room_data.get_structures().ok_or_else(|| {
            let msg = format!("Expected structures - Room: {}", room_data.name);
            log::warn!("{} at {}:{}", msg, file!(), line!());
            msg
        })?;

        let mut spawns: Vec<StructureSpawn> = structures.spawns().to_vec();
        let mut available_energy = room.energy_available();
        let energy_capacity = room.energy_capacity_available();

        let room_has_energy_for_renew = data
            .economy
            .rooms
            .get(&room_entity)
            .map(|r| r.stored_energy >= RENEW_MIN_ROOM_ENERGY)
            .unwrap_or(false);

        let next_spawn_ticks = Self::next_spawn_duration_ticks(requests, spawned_tokens);
        let renew_ttl_threshold = next_spawn_ticks.saturating_add(50);

        let system_data = SpawnQueueExecutionSystemData { updater: &data.updater };

        for request in requests {
            if request.token.map(|t| !spawned_tokens.contains(&t)).unwrap_or(true) {
                if let Some(pos) = spawns.iter().position(|spawn| spawn.is_active() && spawn.spawning().is_none()) {
                    let spawn = &spawns[pos];

                    let body_cost: u32 = request.body.iter().map(|p| p.cost()).sum();

                    if body_cost > energy_capacity {
                        continue;
                    }

                    if body_cost > available_energy {
                        break;
                    }

                    match Self::spawn_creep(spawn, &request.body) {
                        Ok(name) => {
                            (*request.callback)(&system_data, &name);

                            spawns.remove(pos);

                            if let Some(token) = request.token {
                                spawned_tokens.insert(token);
                            }

                            available_energy -= body_cost;
                        }
                        Err(SpawnCreepErrorCode::NotEnoughEnergy) => {
                            break;
                        }
                        Err(_) => {}
                    };
                } else {
                    break;
                }
            }
        }

        // Renew pass — BEHIND the priority gate (P1.D4 / ADR 0011 step
        // 0): spawn requests are priority-sorted and take their lanes
        // first; renew only uses spawns no pending request claimed, so
        // a renew can never consume a lane a CRITICAL/HIGH spawn wants
        // (the pre-D4 ordering ran renew first).
        if room_has_energy_for_renew && !renew_requests.is_empty() {
            let mut renew_sorted: Vec<&RenewRequest> = renew_requests.iter().collect();
            renew_sorted.sort_by_key(|r| r.ticks_to_live);

            for renew in renew_sorted {
                if renew.ticks_to_live >= renew_ttl_threshold && next_spawn_ticks > 0 {
                    continue;
                }
                let creep = match data.creep_owner.get(renew.creep_entity).and_then(|co| co.owner.resolve()) {
                    Some(c) => c,
                    None => continue,
                };
                let creep_pos = creep.pos();
                if let Some(idx) = spawns
                    .iter()
                    .position(|s| s.is_active() && s.spawning().is_none() && creep_pos.get_range_to(s.pos()) <= 1)
                {
                    match spawns[idx].renew_creep(&creep) {
                        Ok(()) => {
                            debug!(
                                "[SpawnQueue] Renewed {} (ttl={}) at {}",
                                creep.name(),
                                renew.ticks_to_live,
                                spawns[idx].name()
                            );
                            let body_cost: u32 = creep.body().iter().map(|p| p.part().cost()).sum();
                            let renew_cost = renew_energy_cost(body_cost, creep.body().len());
                            available_energy = available_energy.saturating_sub(renew_cost);
                            spawns.remove(idx);
                        }
                        Err(e) => {
                            debug!("[SpawnQueue] renew_creep failed for {}: {:?}", creep.name(), e);
                        }
                    }
                }
            }
        }
        let _ = available_energy;

        Ok(())
    }
}

/// Engine-true renew energy cost (P1.D4 / ADR 0011 step 0):
/// `ceil(SPAWN_RENEW_RATIO · body_cost / CREEP_SPAWN_TIME / body_len)`
/// = `ceil(1.2 · cost / 3 / len)` — the engine's
/// `renew-creep` intent processor formula. The pre-D4 estimate
/// (`cost·2/5`) over-charged ~10x for typical bodies, depleting the
/// room's modeled energy and skipping spawns that were affordable.
pub fn renew_energy_cost(body_cost: u32, body_len: usize) -> u32 {
    if body_len == 0 {
        return 0;
    }
    ((1.2 * body_cost as f64) / 3.0 / body_len as f64).ceil() as u32
}

impl<'a> System<'a> for SpawnQueueSystem {
    type SystemData = SpawnQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut spawned_tokens = HashSet::new();

        let mut all_rooms: HashSet<Entity> = data.spawn_queue.requests.keys().copied().collect();
        for room in data.spawn_queue.renew_requests.keys() {
            all_rooms.insert(*room);
        }

        for room_entity in all_rooms {
            let requests = data.spawn_queue.requests.get(&room_entity).map(|v| v.as_slice()).unwrap_or(&[]);
            let renew_requests = data
                .spawn_queue
                .renew_requests
                .get(&room_entity)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            match Self::process_room_spawns(&data, room_entity, requests, renew_requests, &mut spawned_tokens) {
                Ok(()) => {}
                Err(err) => warn!("Failed spawning for room: {}", err),
            }
        }

        // Snapshot the queue depth before clearing, so EconomyAssessmentSystem
        // can read it next tick.
        let mut snapshot = SpawnQueueSnapshot::default();
        for (room_entity, requests) in data.spawn_queue.iter_requests() {
            let depth = requests.len() as u32;
            snapshot.queue_depth_per_room.insert(*room_entity, depth);
            snapshot.total_queue_depth += depth;
        }
        *data.spawn_queue_snapshot = snapshot;

        data.spawn_queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_request(priority: f32) -> SpawnRequest {
        SpawnRequest::new(format!("test-{}", priority), &[Part::Move], priority, None, Box::new(|_, _| {}))
    }

    /// Pin: the spawn queue orders requests DESCENDING by priority (highest
    /// first). `process_room_spawns` consumes requests front-to-back, so the
    /// front of the vec must be the most important creep.
    ///
    /// WHY the comparator in `SpawnQueue::request` must not be "fixed": it
    /// intentionally passes the *new* request's priority as the left-hand
    /// side of `partial_cmp` (the reverse of a conventional ascending
    /// `binary_search_by` probe comparison), which yields a descending
    /// insertion order. The original review flagged this as an inverted
    /// comparator; that finding was REFUTED (see
    /// docs/reviews/ibex-review-report.md, spawn-ordering seed). Rewriting it
    /// as a "natural" ascending comparison would invert spawn priorities so
    /// the least important creep spawns first.
    #[test]
    fn spawn_queue_orders_requests_descending_by_priority() {
        let mut world = specs::World::new();
        let room = world.create_entity().build();

        let mut queue = SpawnQueue::default();
        queue.request(room, test_request(0.0));
        queue.request(room, test_request(100.0));
        queue.request(room, test_request(50.0));

        let priorities: Vec<f32> = queue
            .iter_requests()
            .find(|(entity, _)| **entity == room)
            .map(|(_, requests)| requests.iter().map(|r| r.priority()).collect())
            .expect("expected requests for room");

        assert_eq!(priorities, vec![100.0, 50.0, 0.0]);
    }

    /// Pin (IBEX-046): non-finite priorities trip the debug assert at the
    /// request source in debug builds (tests/sim).
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "priority not finite")]
    fn spawn_request_rejects_non_finite_priority_in_debug() {
        let _ = test_request(f32::NAN);
    }

    /// Engine-true renew cost: ceil(1.2·cost/3/len) — pinned against
    /// the engine's renew-creep intent formula (P1.D4 / ADR 0011).
    #[test]
    fn renew_cost_matches_engine_formula() {
        // 10-part 1000-energy body: ceil(1.2·1000/3/10) = 40
        // (the pre-D4 estimate charged 400 — 10x).
        assert_eq!(renew_energy_cost(1000, 10), 40);
        // Single 50-cost part: ceil(1.2·50/3/1) = 20.
        assert_eq!(renew_energy_cost(50, 1), 20);
        // Rounding up: 3 parts, 250 energy → ceil(33.33) = 34.
        assert_eq!(renew_energy_cost(250, 3), 34);
        // Degenerate empty body.
        assert_eq!(renew_energy_cost(0, 0), 0);
    }

    /// Pin: equal-priority requests stay adjacent and do not disturb the
    /// descending order around them.
    #[test]
    fn spawn_queue_keeps_descending_order_with_equal_priorities() {
        let mut world = specs::World::new();
        let room = world.create_entity().build();

        let mut queue = SpawnQueue::default();
        queue.request(room, test_request(25.0));
        queue.request(room, test_request(75.0));
        queue.request(room, test_request(25.0));
        queue.request(room, test_request(100.0));

        let priorities: Vec<f32> = queue
            .iter_requests()
            .find(|(entity, _)| **entity == room)
            .map(|(_, requests)| requests.iter().map(|r| r.priority()).collect())
            .expect("expected requests for room");

        assert_eq!(priorities, vec![100.0, 75.0, 25.0, 25.0]);
    }
}
