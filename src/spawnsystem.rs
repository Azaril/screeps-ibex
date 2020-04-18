use crate::room::data::*;
use crate::ui::*;
use crate::visualize::*;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

pub const SPAWN_PRIORITY_CRITICAL: f32 = 100.0;
pub const SPAWN_PRIORITY_HIGH: f32 = 75.0;
pub const SPAWN_PRIORITY_MEDIUM: f32 = 50.0;
pub const SPAWN_PRIORITY_LOW: f32 = 25.0;
pub const SPAWN_PRIORITY_NONE: f32 = 0.0;

pub struct SpawnRequest {
    description: String,
    body: Vec<Part>,
    priority: f32,
    callback: Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)>,
}

impl SpawnRequest {
    pub fn new(
        description: String,
        body: &[Part],
        priority: f32,
        callback: Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)>,
    ) -> SpawnRequest {
        SpawnRequest {
            description,
            body: body.to_vec(),
            priority,
            callback,
        }
    }

    pub fn cost(&self) -> u32 {
        self.body.iter().map(|p| p.cost()).sum()
    }
}

#[derive(Default)]
pub struct SpawnQueue {
    requests: HashMap<RoomName, Vec<SpawnRequest>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SpawnQueue {
    pub fn request(&mut self, room: RoomName, spawn_request: SpawnRequest) {
        let requests = self.requests.entry(room).or_insert_with(Vec::new);

        let pos = requests
            .binary_search_by(|probe| spawn_request.priority.partial_cmp(&probe.priority).unwrap())
            .unwrap_or_else(|e| e);

        requests.insert(pos, spawn_request);
    }

    pub fn clear(&mut self) {
        self.requests.clear();
    }

    fn visualize(&self, ui: &mut UISystem, visualizer: &mut Visualizer) {
        for (room_name, requests) in &self.requests {
            ui.with_room(*room_name, visualizer, |room_ui| {
                for request in requests.iter() {
                    let text = format!("{} - {} - {}", request.priority, request.cost(), request.description);

                    room_ui.spawn_queue().add_text(text, None);
                }
            });
        }
    }
}

#[derive(SystemData)]
pub struct SpawnQueueSystemData<'a> {
    spawn_queue: Write<'a, SpawnQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

pub struct SpawnQueueExecutionSystemData<'a> {
    pub updater: Read<'a, LazyUpdate>,
}

pub struct SpawnQueueSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SpawnQueueSystem {
    fn spawn_creep(spawn: &StructureSpawn, parts: &[Part]) -> Result<String, ReturnCode> {
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
        }
    }
}

impl<'a> System<'a> for SpawnQueueSystem {
    type SystemData = SpawnQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                data.spawn_queue.visualize(ui, visualizer);
            }
        }

        let system_data = SpawnQueueExecutionSystemData { updater: data.updater };

        for (room_name, requests) in &mut data.spawn_queue.requests {
            if let Some(room) = game::rooms::get(*room_name) {
                let mut spawns = room.find(find::MY_SPAWNS);

                let mut available_energy = room.energy_available();

                for request in requests {
                    if let Some(pos) = spawns.iter().position(|spawn| spawn.is_active() && !spawn.is_spawning()) {
                        let spawn = &spawns[pos];

                        //TODO: Is this needed? is available energy decremented on an Ok response to spawn?
                        let body_cost: u32 = request.body.iter().map(|p| p.cost()).sum();

                        if body_cost > available_energy {
                            break;
                        }

                        match Self::spawn_creep(&spawn, &request.body) {
                            Ok(name) => {
                                (*request.callback)(&system_data, &name);

                                spawns.remove(pos);

                                available_energy -= body_cost;
                            }
                            Err(ReturnCode::NotEnough) => {
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
        }

        data.spawn_queue.clear();
    }
}
