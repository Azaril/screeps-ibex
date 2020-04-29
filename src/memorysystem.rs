use log::*;
use screeps::*;
use specs::prelude::*;
use std::collections::HashSet;

pub struct MemoryArbiter {
    active: Option<HashSet<u32>>,
    requests: HashSet<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MemoryArbiter {
    pub fn new() -> MemoryArbiter {
        MemoryArbiter {
            active: None,
            requests: HashSet::new()
        }
    }

    pub fn request(&mut self, segment: u32) {
        self.requests.insert(segment);
    }

    pub fn is_active(&mut self, active: u32) -> bool {
        self.active
            .get_or_insert_with(|| raw_memory::get_active_segments().into_iter().collect())
            .contains(&active)
    }

    pub fn get(&self, segment: u32) -> Option<String> {
        raw_memory::get_segment(segment)
    }

    pub fn set(&mut self, segment: u32, data: &str) {
        if data.len() > 50 * 1024 {
            error!("Memory segment too large: {}", data);
        }

        raw_memory::set_segment(segment, data);
    }

    pub fn clear(&mut self) {
        self.requests.clear();
    }
}

#[derive(SystemData)]
pub struct MemoryArbiterSystemData<'a> {
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
}

pub struct MemoryArbiterSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MemoryArbiterSystem {
    type SystemData = MemoryArbiterSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let segments: Vec<_> = data.memory_arbiter.requests.iter().cloned().collect();

        raw_memory::set_active_segments(&segments);

        data.memory_arbiter.clear();
    }
}
