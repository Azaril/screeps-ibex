use super::memorysystem::*;
use screeps::*;
use serde::*;
use specs::prelude::*;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct CpuStats {
    bucket: f64,
    limit: f64,
    used: f64,
}

#[derive(Serialize)]
pub struct RoomStats {
    energy_available: u32,
    energy_capacity_available: u32,

    storage_energy: u32,
    terminal_energy: u32,

    controller_progress: u32,
    controller_progress_total: u32,
    controller_level: u32,
}

#[derive(Serialize)]
pub struct GclStats {
    progress: f64,
    progress_total: f64,
    level: u32,
}

#[derive(Serialize)]
pub struct ShardStats {
    time: u32,
    gcl: GclStats,
    cpu: CpuStats,
    room: HashMap<RoomName, RoomStats>,
}

#[derive(Serialize)]
pub struct Stats {
    shard: HashMap<String, ShardStats>,
}

pub struct StatsSystem;

impl StatsSystem {
    fn get_gcl_stats() -> GclStats {
        GclStats {
            progress: game::gcl::progress(),
            progress_total: game::gcl::progress_total(),
            level: game::gcl::level(),
        }
    }

    fn get_cpu_stats() -> CpuStats {
        CpuStats {
            bucket: game::cpu::bucket(),
            limit: game::cpu::limit(),
            used: game::cpu::get_used(),
        }
    }

    fn get_room_stats(data: &StatsSystemData) -> HashMap<RoomName, RoomStats> {
        (&data.entities, &data.room_data)
            .join()
            .filter(|(_, room_data)| {
                room_data
                    .get_dynamic_visibility_data()
                    .map(|v| v.visible() && v.owner().mine())
                    .unwrap_or(false)
            })
            .filter_map(|(_, room_data)| {
                if let Some(room) = game::rooms::get(room_data.name) {
                    let controller = room.controller();

                    let stats = RoomStats {
                        energy_available: room.energy_available(),
                        energy_capacity_available: room.energy_capacity_available(),

                        storage_energy: room
                            .storage()
                            .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                            .unwrap_or(0),
                        terminal_energy: room
                            .terminal()
                            .map(|s| s.store_used_capacity(Some(ResourceType::Energy)))
                            .unwrap_or(0),

                        controller_progress: controller.as_ref().and_then(|c| c.progress()).unwrap_or(0),
                        controller_progress_total: controller.as_ref().and_then(|c| c.progress_total()).unwrap_or(0),
                        controller_level: controller.as_ref().map(|c| c.level()).unwrap_or(0),
                    };

                    Some((room_data.name, stats))
                } else {
                    None
                }
            })
            .collect()
    }

    fn get_shard_stats(data: &StatsSystemData) -> ShardStats {
        ShardStats {
            time: game::time(),
            gcl: Self::get_gcl_stats(),
            cpu: Self::get_cpu_stats(),
            room: Self::get_room_stats(data),
        }
    }

    fn get_shards_stats(data: &StatsSystemData) -> HashMap<String, ShardStats> {
        let mut shards = HashMap::new();

        shards.insert(game::shards::name(), Self::get_shard_stats(data));

        shards
    }
}

#[derive(SystemData)]
pub struct StatsSystemData<'a> {
    entities: Entities<'a>,
    room_data: ReadStorage<'a, ::room::data::RoomData>,
    memory_arbiter: Write<'a, MemoryArbiter>,
}

impl<'a> System<'a> for StatsSystem {
    type SystemData = StatsSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.memory_arbiter.request(99);

        if data.memory_arbiter.is_active(99) {
            let stats = Stats {
                shard: Self::get_shards_stats(&data),
            };

            if let Ok(stats_data) = serde_json::to_string(&stats) {
                data.memory_arbiter.set(99, &stats_data);
            }
        }
    }
}
