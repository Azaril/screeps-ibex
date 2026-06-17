use crate::creep::CreepOwner;
use crate::military::economy::{EconomySnapshot, SpawnQueueSnapshot};
use crate::room::data::*;
use crate::room::roomplansystem::RoomPlanData;
// The unsigned 0..49 room-tile type the planner stores in `Plan::spawn_approaches`.
// Aliased away from the bare name to avoid confusion with the distinct signed
// `screeps_common::PlanLocation` (i8, supports negative stamp offsets).
use log::*;
use screeps::action_error_codes::SpawnCreepErrorCode;
use screeps::*;
use screeps_common::Location as PlanTileLocation;
use screeps_foreman::terrain::FastRoomTerrain;
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

/// Exclusive upper bound on a room tile coordinate (rooms are 50x50, 0..=49).
const ROOM_COORD_MAX: i32 = 50;
/// 8-directional neighbour offsets, ordered to match [`SpawnQueueSystem::delta_to_direction`].
const NEIGHBOR_DELTAS: [(i32, i32); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];

/// Whether a structure blocks a creep from being spawned onto its tile. Mirrors
/// the dismantle behaviour's standability rule: roads, containers, extractors
/// and own/public ramparts are standable; every other structure blocks.
fn structure_blocks_spawn(structure: &StructureObject) -> bool {
    match structure {
        StructureObject::StructureRoad(_) | StructureObject::StructureContainer(_) | StructureObject::StructureExtractor(_) => false,
        StructureObject::StructureRampart(rampart) => !(rampart.my() || rampart.is_public()),
        _ => true,
    }
}

/// Live room facts used to choose a safe spawn-out direction when the plan's
/// approaches are unavailable or all blocked. Built lazily, at most once per
/// room per tick (only when a spawn actually fires), so the `find`/terrain cost
/// is paid only by rooms that spawn.
struct LiveSpawnContext {
    terrain: FastRoomTerrain,
    creep_tiles: HashSet<(u8, u8)>,
    blocked_tiles: HashSet<(u8, u8)>,
}

impl LiveSpawnContext {
    fn build(room: &Room, structures: &RoomStructureData) -> LiveSpawnContext {
        let terrain = FastRoomTerrain::new(room.get_terrain().get_raw_buffer().to_vec());
        let creep_tiles = room
            .find(find::CREEPS, None)
            .iter()
            .map(|c| {
                let p = c.pos();
                (p.x().u8(), p.y().u8())
            })
            .collect();
        let blocked_tiles = structures
            .all()
            .iter()
            .filter(|s| structure_blocks_spawn(s))
            .map(|s| {
                let p = s.pos();
                (p.x().u8(), p.y().u8())
            })
            .collect();
        LiveSpawnContext {
            terrain,
            creep_tiles,
            blocked_tiles,
        }
    }

    /// A tile a creep can stand on (terrain + structures), ignoring creeps.
    fn walkable(&self, x: u8, y: u8) -> bool {
        !self.terrain.is_wall(x, y) && !self.blocked_tiles.contains(&(x, y))
    }

    /// A tile currently free for a creep to be placed on (walkable + uncrowded).
    fn free(&self, x: u8, y: u8) -> bool {
        self.walkable(x, y) && !self.creep_tiles.contains(&(x, y))
    }

    /// In-bounds walkable neighbours of (x,y); <2 means a dead-end pocket.
    fn walkable_neighbor_count(&self, x: u8, y: u8) -> usize {
        NEIGHBOR_DELTAS
            .iter()
            .filter(|(dx, dy)| {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                (0..ROOM_COORD_MAX).contains(&nx) && (0..ROOM_COORD_MAX).contains(&ny) && self.walkable(nx as u8, ny as u8)
            })
            .count()
    }
}

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
    room_plan_data: ReadStorage<'a, RoomPlanData>,
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
    fn spawn_creep(spawn: &StructureSpawn, parts: &[Part], directions: &[Direction]) -> Result<String, SpawnCreepErrorCode> {
        let time = screeps::game::time();
        let mut additional = 0;
        loop {
            let name = format!("{}-{}", time, additional);
            // When the planner leaves a dead-end pocket adjacent to a spawn,
            // letting the engine place the creep there traps it. Constrain the
            // spawn to face the base interior (`directions`) so the creep always
            // lands on a hub-connected tile. (The room planner relies on this:
            // its ReachabilityLayer no longer requires *every* spawn-adjacent
            // tile to be reachable, only that an approach exists.)
            let result = if directions.is_empty() {
                spawn.spawn_creep(parts, &name)
            } else {
                spawn.spawn_creep_with_options(parts, &name, &SpawnOptions::new().directions(directions))
            };
            match result {
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

    /// Directions to constrain `spawnCreep` to, degrading monotonically so a
    /// creep is never orphaned in a pocket and a spawn is never wedged shut:
    ///
    /// 1. **Planner-approved approaches** (`Plan::spawn_approaches`, adjacent to
    ///    this spawn) that are *currently free*. These come from the planner's
    ///    own hub flood-fill, so they are the authoritative "hub-connected" set
    ///    -- not a runtime "toward storage" guess that could point at a walled
    ///    tile. Filtering to free ones means a single camped approach can't wedge
    ///    the spawn (the engine would otherwise hold the half-spawned creep until
    ///    an allowed tile clears).
    /// 2. **Live-safe interior tiles** when (1) is empty -- no plan yet, an
    ///    off-plan / relocated spawn (no approach is adjacent), or every approach
    ///    is blocked. A neighbour qualifies if it is in-bounds, not a room-border
    ///    (exit) tile, currently free, and not a dead-end pocket (>=2 walkable
    ///    neighbours). This is a local heuristic, weaker than (1)'s authoritative
    ///    reachability, but only reached in the rare no-authoritative-data case.
    /// 3. **Empty** when even (2) finds nothing (a truly boxed-in spawn): the
    ///    caller then spawns unconstrained, letting the engine try every tile --
    ///    the last resort, strictly better than refusing to spawn.
    fn safe_spawn_directions(spawn_pos: Position, approaches: &[PlanTileLocation], live: &LiveSpawnContext) -> Vec<Direction> {
        let sx = spawn_pos.x().u8() as i32;
        let sy = spawn_pos.y().u8() as i32;

        // Tier 1: planner-approved approaches that are free right now.
        let planned: Vec<Direction> = approaches
            .iter()
            .filter(|loc| live.free(loc.x(), loc.y()))
            .filter_map(|loc| Self::delta_to_direction(loc.x() as i32 - sx, loc.y() as i32 - sy))
            .collect();
        if !planned.is_empty() {
            return planned;
        }

        // Tier 2: live-safe interior neighbours (off-plan / no plan / all
        // approaches blocked). Skip borders and dead-end pockets.
        let mut safe: Vec<Direction> = Vec::new();
        for (dx, dy) in NEIGHBOR_DELTAS {
            let nx = sx + dx;
            let ny = sy + dy;
            if !(0..ROOM_COORD_MAX).contains(&nx) || !(0..ROOM_COORD_MAX).contains(&ny) {
                continue;
            }
            let (ux, uy) = (nx as u8, ny as u8);
            let is_border = nx == 0 || ny == 0 || nx == ROOM_COORD_MAX - 1 || ny == ROOM_COORD_MAX - 1;
            if is_border || !live.free(ux, uy) || live.walkable_neighbor_count(ux, uy) < 2 {
                continue;
            }
            if let Some(d) = Self::delta_to_direction(dx, dy) {
                safe.push(d);
            }
        }
        safe
    }

    /// Map an adjacent (dx, dy) offset to a `Direction`; `None` if the tiles are
    /// not 8-adjacent (so non-adjacent approaches are filtered out).
    fn delta_to_direction(dx: i32, dy: i32) -> Option<Direction> {
        use Direction::*;
        Some(match (dx, dy) {
            (0, -1) => Top,
            (1, -1) => TopRight,
            (1, 0) => Right,
            (1, 1) => BottomRight,
            (0, 1) => Bottom,
            (-1, 1) => BottomLeft,
            (-1, 0) => Left,
            (-1, -1) => TopLeft,
            _ => return None,
        })
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
        // Planner-approved spawn approach tiles (hub-reachable, never pockets).
        // Empty if no plan yet, or an old plan from before the field existed.
        let spawn_approaches: Vec<PlanTileLocation> = data
            .room_plan_data
            .get(room_entity)
            .and_then(|d| d.plan())
            .map(|p| p.spawn_approaches.clone())
            .unwrap_or_default();
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

        // Live room facts for safe spawn directions, built at most once per room
        // per tick on first actual spawn (skipped entirely if nothing spawns).
        let mut live_ctx: Option<LiveSpawnContext> = None;

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

                    let live = live_ctx.get_or_insert_with(|| LiveSpawnContext::build(&room, &structures));
                    let directions = Self::safe_spawn_directions(spawn.pos(), &spawn_approaches, live);

                    match Self::spawn_creep(spawn, &request.body, &directions) {
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
