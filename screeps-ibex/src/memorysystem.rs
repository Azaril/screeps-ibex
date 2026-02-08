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
            requests: HashSet::new(),
        }
    }

    pub fn request(&mut self, segment: u32) {
        self.requests.insert(segment);
    }

    pub fn is_active(&mut self, active: u32) -> bool {
        self.active
            .get_or_insert_with(|| {
                // In screeps 0.23, raw_memory::segments() returns a JsHashMap<u8, String>.
                // The keys of this map are the currently active segments.
                raw_memory::segments().keys().map(|k| k as u32).collect()
            })
            .contains(&active)
    }

    pub fn get(&self, segment: u32) -> Option<String> {
        raw_memory::segments().get(segment as u8)
    }

    pub fn set(&mut self, segment: u32, data: &str) {
        if data.len() > 50 * 1024 {
            error!("Memory segment too large - Segment: {} - Data: {}", segment, data);
        }

        // In screeps 0.23, we need to set segment data via JS interop
        let global = js_sys::global();
        let raw_memory = match js_sys::Reflect::get(
            &global,
            &wasm_bindgen::JsValue::from_str("RawMemory"),
        ) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to get RawMemory: {:?}", e);
                return;
            }
        };

        let segments_obj = match js_sys::Reflect::get(
            &raw_memory,
            &wasm_bindgen::JsValue::from_str("segments"),
        ) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to get RawMemory.segments: {:?}", e);
                return;
            }
        };

        if let Err(e) = js_sys::Reflect::set(
            &segments_obj,
            &wasm_bindgen::JsValue::from_f64(segment as f64),
            &wasm_bindgen::JsValue::from_str(data),
        ) {
            error!("Failed to set segment data: {:?}", e);
        }
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
        let segments: Vec<u8> = data.memory_arbiter.requests.iter().map(|&s| s as u8).collect();

        raw_memory::set_active_segments(&segments);

        data.memory_arbiter.clear();
    }
}
