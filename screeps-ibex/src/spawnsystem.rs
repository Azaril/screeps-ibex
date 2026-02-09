use crate::room::data::*;
use log::*;
use screeps::action_error_codes::SpawnCreepErrorCode;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;

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

#[derive(Default)]
pub struct SpawnQueue {
    next_token: u32,
    requests: HashMap<Entity, Vec<SpawnRequest>>,
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

    pub fn clear(&mut self) {
        self.next_token = 0;
        self.requests.clear();
    }

    /// Iterate over (room_entity, requests) for visualization/gather systems.
    pub fn iter_requests(&self) -> std::collections::hash_map::Iter<'_, Entity, Vec<SpawnRequest>> {
        self.requests.iter()
    }
}

#[derive(SystemData)]
pub struct SpawnQueueSystemData<'a> {
    spawn_queue: Write<'a, SpawnQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
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

    fn process_room_spawns(
        data: &SpawnQueueSystemData,
        room_entity: Entity,
        requests: &Vec<SpawnRequest>,
        spawned_tokens: &mut HashSet<SpawnToken>,
    ) -> Result<(), String> {
        let room_data = data.room_data.get(room_entity).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let structures = room_data.get_structures().ok_or("Expected structures")?;

        let mut spawns = structures.spawns().iter().collect::<Vec<_>>();

        let mut available_energy = room.energy_available();
        let energy_capacity = room.energy_capacity_available();

        let system_data = SpawnQueueExecutionSystemData { updater: &data.updater };

        for request in requests {
            if request.token.map(|t| !spawned_tokens.contains(&t)).unwrap_or(true) {
                if let Some(pos) = spawns.iter().position(|spawn| spawn.is_active() && spawn.spawning().is_none()) {
                    let spawn = &spawns[pos];

                    let body_cost: u32 = request.body.iter().map(|p| p.cost()).sum();

                    if body_cost > energy_capacity {
                        //
                        // Requested creep body can never be spawned, ignore. (May be shared spawn request.)
                        //
                        continue;
                    }

                    if body_cost > available_energy {
                        //
                        // If there was not enough energy available for the highest priority request,
                        // continue waiting for energy and don't allow any other spawns to occur.
                        //
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
                            //
                            // If there was not enough energy available for the highest priority request,
                            // continue waiting for energy and don't allow any other spawns to occur.
                            //
                            break;
                        }
                        Err(_) => {
                            //
                            // Any other errors are assumed to be mis-configuration and should be ignored
                            // rather than block further spawns.
                            //
                        }
                    };
                } else {
                    break;
                }
            }
        }

        Ok(())
    }
}

impl<'a> System<'a> for SpawnQueueSystem {
    type SystemData = SpawnQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut spawned_tokens = HashSet::new();

        for (room_entity, requests) in &data.spawn_queue.requests {
            match Self::process_room_spawns(&data, *room_entity, requests, &mut spawned_tokens) {
                Ok(()) => {}
                Err(err) => warn!("Failed spawning for room: {}", err),
            }
        }

        data.spawn_queue.clear();
    }
}
