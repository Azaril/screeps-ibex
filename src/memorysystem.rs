use screeps::*;
use specs::prelude::*;
use std::collections::HashSet;

#[derive(Default)]
pub struct MemoryArbiter {
    active: Option<HashSet<u32>>,
    requests: HashSet<u32>,
}

impl MemoryArbiter {
    pub fn request(&mut self, segment: u32) {
        self.requests.insert(segment);
    }

    pub fn is_active(&mut self, active: u32) -> bool {
        self.active.get_or_insert_with(|| {
            raw_memory::get_active_segments().into_iter().collect()
        }).contains(&active)
    }

    pub fn get(&self, segment: u32) -> Option<String> {
        raw_memory::get_segment(segment)
    }

    pub fn set(&mut self, segment: u32, data: &str) {
        raw_memory::set_segment(segment, data);
    }

    pub fn clear(&mut self) {
        self.requests.clear();
    }
}

#[derive(SystemData)]
pub struct MemoryArbiterSystemData<'a> {
    memory_arbiter: Write<'a, MemoryArbiter>,
}

pub struct MemoryArbiterSystem;

impl<'a> System<'a> for MemoryArbiterSystem {
    type SystemData = MemoryArbiterSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let segments: Vec<_> = data.memory_arbiter.requests.iter().cloned().collect();

        raw_memory::set_active_segments(&segments);

        data.memory_arbiter.clear();
    }
}