use screeps::*;
use screeps::memory::*;
use specs::prelude::*;

#[derive(SystemData)]
pub struct StatsSystemData<'a> {
    room_data: WriteStorage<'a, ::room::data::RoomData>
}

pub struct StatsSystem;

impl<'a> System<'a> for StatsSystem {
    type SystemData = StatsSystemData<'a>;

    fn run(&mut self, _data: Self::SystemData) {
        scope_timing!("StatsSystem");

        let stats = MemoryReference::new();

        let shard_name = game::shards::name();

        stats.set(&format!("{}.cpu.limit", shard_name), game::cpu::limit());
        stats.set(&format!("{}.cpu.tick_limit", shard_name), game::cpu::tick_limit());
        stats.set(&format!("{}.cpu.bucket", shard_name), game::cpu::bucket());
        
        memory::root().set("stats", stats.as_ref());
    }
}