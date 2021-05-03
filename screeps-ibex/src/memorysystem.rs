use log::*;
use screeps::*;
use specs::prelude::*;
use std::collections::HashSet;

pub struct MemoryArbiter {
    active: Option<HashSet<u8>>,
    requests: HashSet<u8>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MemoryArbiter {
    pub fn new() -> MemoryArbiter {
        MemoryArbiter {
            active: None,
            requests: HashSet::new(),
        }
    }

    pub fn request(&mut self, segment: u8) {
        self.requests.insert(segment);
    }

    pub fn is_active(&mut self, active: u8) -> bool {
        self.active
            .get_or_insert_with(|| RawMemory::segments().keys().collect())
            .contains(&active)
    }

    pub fn get(&self, segment: u8) -> Option<String> {
        RawMemory::segments().get(segment).into()
    }

    pub fn set(&mut self, segment: u8, data: String) {
        if data.len() > MEMORY_SEGMENT_SIZE_LIMIT as usize {
            error!("Memory segment too large - Segment: {} - Data: {}", segment, data);
        }

        RawMemory::segments().set(segment, data);
    }

    pub fn clear(&mut self) {
        self.requests.clear();
        self.active = None;
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

        RawMemory::set_active_segments(&segments);

        data.memory_arbiter.clear();
    }
}
