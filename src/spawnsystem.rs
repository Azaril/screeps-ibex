use crossbeam_queue::SegQueue;
use specs::*;
use specs::prelude::*;
use screeps::*;
use itertools::*;

pub const SPAWN_PRIORITY_CRITICAL: f32 = 100.0;
pub const SPAWN_PRIORITY_HIGH: f32 = 75.0;
pub const SPAWN_PRIORITY_MEDIUM: f32 = 50.0;
pub const SPAWN_PRIORITY_LOW: f32 = 25.0;
pub const SPAWN_PRIORITY_NONE: f32 = 0.0;

pub struct SpawnRequest {
    room_name: RoomName,
    body: Vec<Part>,
    priority: f32,
    callback: Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync>
}

impl SpawnRequest {
    pub fn new(room_name: &RoomName, body: &[Part], priority: f32, callback: Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync>) -> SpawnRequest {
        SpawnRequest{
            room_name: *room_name,
            body: body.to_vec(),
            priority: priority,
            callback: callback
        }
    }
}

#[derive(Default)]
pub struct SpawnQueue {
    pub requests: SegQueue<SpawnRequest>
}

impl SpawnQueue {
    pub fn request(&self, spawn_request: SpawnRequest) {
        self.requests.push(spawn_request);
    }
}

impl Drop for SpawnQueue {
    fn drop(&mut self) {
        // TODO: remove as soon as leak is fixed in crossbeam
        while self.requests.pop().is_ok() {}
    }
}

#[derive(SystemData)]
pub struct SpawnQueueSystemData<'a> {
    spawn_queue: Write<'a, SpawnQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>, 
    room_owner: WriteStorage<'a, ::room::data::RoomOwnerData>,
    room_data: WriteStorage<'a, ::room::data::RoomData>
}

pub struct SpawnQueueExecutionSystemData<'a> {  
    pub updater: Read<'a, LazyUpdate>
}

pub struct SpawnQueueSystem;

impl SpawnQueueSystem
{
    fn can_spawn<'a>(spawn: &StructureSpawn, _parts: &[Part]) -> bool {
        if spawn.is_spawning() {
            return false;
        }

        return true;
    }

    fn spawn_creep<'a>(spawn: &StructureSpawn, parts: &[Part]) -> Result<String, ReturnCode> {
        let time = screeps::game::time();
        let mut additional = 0;
        loop {
            let name = format!("{}-{}", time, additional);
            let res = spawn.spawn_creep(&parts, &name);

            if res == ReturnCode::NameExists {
                additional += 1;
            } else if res == ReturnCode::Ok {
                return Ok(name);
            } else {
                return Err(res);
            }
        };
    }
}

impl<'a> System<'a> for SpawnQueueSystem {
    type SystemData = SpawnQueueSystemData<'a>;

    fn run(&mut self, data: Self::SystemData) {
        scope_timing!("SpawnQueueSystem");

        let system_data = SpawnQueueExecutionSystemData{
            updater: data.updater
        };

        let mut requests = vec!();

        while let Ok(request) = data.spawn_queue.requests.pop() {
            requests.push(request);
        }

        let room_requests = requests.iter()
            .map(|request| (request.room_name.clone(), request))
            .into_group_map();

        for (room_name, mut requests) in room_requests {
            if let Some(room) = game::rooms::get(room_name) {
                let mut spawns = room.find(find::MY_SPAWNS);

                requests.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap());

                for request in requests {
                    if let Some(pos) = spawns.iter().position(|spawn| !spawn.is_spawning()) {
                        let spawn = &spawns[pos];

                        let spawn_complete = match Self::spawn_creep(&spawn, &request.body) {
                            Ok(name) => {
                                (*request.callback)(&system_data, &name);
                            
                                true
                            },
                            Err(ReturnCode::NotEnough) => {
                                //
                                // If there was not enough energy available for the highest priority request,
                                // continue waiting for energy and don't allow any other spawns to occur.
                                //
                                true
                            },
                            _ => {
                                //
                                // Any other errors are assumed to be mis-configuration and should be ignored
                                // rather than block further spawns.
                                //
                                false
                            }
                        };

                        if spawn_complete {
                            spawns.remove(pos);
                        }
                    }

                    if spawns.is_empty() {
                        break;
                    }
                }
            }
        }
    }
}