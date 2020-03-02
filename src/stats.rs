use screeps::*;
use screeps::memory::*;
use specs::prelude::*;

#[derive(SystemData)]
pub struct StatsSystemData<'a> {
    entities: Entities<'a>,
    room_data: WriteStorage<'a, ::room::data::RoomData>
}

pub struct StatsSystem;

impl StatsSystem {
    fn add_gcl(gcl_node: &mut MemoryReference) {
        gcl_node.set("progress", game::gcl::progress());
        gcl_node.set("progress_total", game::gcl::progress_total());
        gcl_node.set("level", game::gcl::level());
    }

    fn add_cpu(cpu_node: &mut MemoryReference) {
        cpu_node.set("bucket", game::cpu::bucket());
        cpu_node.set("limit", game::cpu::limit());
        cpu_node.set("used", game::cpu::get_used());
    }

    fn add_rooms(rooms_node: &mut MemoryReference, data: &StatsSystemData) {
        for (_, room_data) in (&data.entities, &data.room_data).join() {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    if let Some(room) = game::rooms::get(room_data.name) {
                        let room_node = rooms_node.dict_or_create(&room_data.name.to_string()).unwrap();

                        room_node.set("storage_energy", room.storage().map(|s| s.store_used_capacity(Some(ResourceType::Energy))).unwrap_or(0));
                        room_node.set("terminal_energy", room.terminal().map(|s| s.store_used_capacity(Some(ResourceType::Energy))).unwrap_or(0));

                        room_node.set("energy_available", room.energy_available());
                        room_node.set("energy_capacity_available", room.energy_capacity_available());

                        let controller = room.controller();

                        room_node.set("controller_progress", controller.as_ref().and_then(|c| c.progress()).unwrap_or(0));
                        room_node.set("controller_progress_total", controller.as_ref().and_then(|c| c.progress_total()).unwrap_or(0));
                        room_node.set("controller_level", controller.as_ref().map(|c| c.level()).unwrap_or(0));
                    }
                }
            }
        }
    }
}

impl<'a> System<'a> for StatsSystem {
    type SystemData = StatsSystemData<'a>;

    fn run(&mut self, data: Self::SystemData) {
        scope_timing!("StatsSystem");

        let stats = MemoryReference::new();
        let shard = stats.dict_or_create(&game::shards::name()).unwrap();

        shard.set("time", game::time());

        Self::add_rooms(&mut shard.dict_or_create("room").unwrap(), &data);
        Self::add_gcl(&mut shard.dict_or_create("gcl").unwrap());
        Self::add_cpu(&mut shard.dict_or_create("cpu").unwrap());

        memory::root().set("_stats", stats.as_ref());
    }
}