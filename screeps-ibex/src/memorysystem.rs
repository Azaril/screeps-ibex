use log::*;
use screeps::*;
use specs::prelude::*;
use std::collections::HashSet;

// ─── Segment requirements ────────────────────────────────────────────────────

/// Callback invoked the first time a requirement's segments become active after
/// environment creation (or after a memory reset). Receives `&mut World` so it
/// can insert resources, deserialize data, etc. The `MemoryArbiter` is
/// temporarily removed from the world while this runs; do not access it.
pub type SegmentLoadFn = Box<dyn Fn(&mut World) + Send + Sync>;

/// Describes one logical segment need registered during environment setup.
pub struct SegmentRequirement {
    /// Human-readable label (for logging / debugging).
    pub label: &'static str,
    /// Segment IDs this requirement covers.
    pub segments: Vec<u32>,
    /// If true, the game loop will not dispatch until all of these segments are active.
    pub gates_execution: bool,
    /// Optional callback run once the first time the segments are active.
    pub on_load: Option<SegmentLoadFn>,
    /// Whether `on_load` has already fired this environment lifecycle.
    loaded: bool,
}

impl SegmentRequirement {
    pub fn new(label: &'static str, segments: Vec<u32>) -> Self {
        Self {
            label,
            segments,
            gates_execution: false,
            on_load: None,
            loaded: false,
        }
    }

    pub fn gates_execution(mut self, gates: bool) -> Self {
        self.gates_execution = gates;
        self
    }

    pub fn on_load(mut self, f: SegmentLoadFn) -> Self {
        self.on_load = Some(f);
        self
    }
}

// ─── MemoryArbiter ───────────────────────────────────────────────────────────

pub struct MemoryArbiter {
    /// Lazily-populated set of segments the Screeps runtime has made active this tick.
    active: Option<HashSet<u32>>,
    /// Segments that will be requested from the runtime at end-of-tick.
    requests: HashSet<u32>,
    /// Declared segment requirements (registered during environment creation).
    requirements: Vec<SegmentRequirement>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MemoryArbiter {
    pub fn new() -> MemoryArbiter {
        MemoryArbiter {
            active: None,
            requests: HashSet::new(),
            requirements: Vec::new(),
        }
    }

    // ── Registration (called during environment setup) ───────────────────

    /// Register a segment requirement. Call during `create_environment`.
    pub fn register(&mut self, req: SegmentRequirement) {
        self.requirements.push(req);
    }

    // ── Per-tick segment I/O (unchanged API for systems) ─────────────────

    pub fn request(&mut self, segment: u32) {
        self.requests.insert(segment);
    }

    pub fn is_active(&mut self, segment: u32) -> bool {
        self.active
            .get_or_insert_with(|| raw_memory::segments().keys().map(|k| k as u32).collect())
            .contains(&segment)
    }

    pub fn get(&self, segment: u32) -> Option<String> {
        raw_memory::segments().get(segment as u8)
    }

    pub fn set(&mut self, segment: u32, data: &str) {
        if data.len() > 50 * 1024 {
            error!("Memory segment too large - Segment: {} - Data: {}", segment, data);
        }

        let global = js_sys::global();
        let raw_memory = match js_sys::Reflect::get(&global, &wasm_bindgen::JsValue::from_str("RawMemory")) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to get RawMemory: {:?}", e);
                return;
            }
        };

        let segments_obj = match js_sys::Reflect::get(&raw_memory, &wasm_bindgen::JsValue::from_str("segments")) {
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

    // ── Pre-pass helpers (called by game loop) ───────────────────────────

    /// Add all registered segments to the request set for this tick.
    pub fn request_registered(&mut self) {
        for req in &self.requirements {
            for &seg in &req.segments {
                self.requests.insert(seg);
            }
        }
    }

    /// Returns true if every gating requirement's segments are active.
    pub fn gates_ready(&mut self) -> bool {
        // Collect segment lists first to avoid double-borrow on self.
        let gating: Vec<Vec<u32>> = self
            .requirements
            .iter()
            .filter(|r| r.gates_execution)
            .map(|r| r.segments.clone())
            .collect();

        gating.iter().all(|segs| segs.iter().all(|&seg| self.is_active(seg)))
    }

    /// All registered segment IDs (used by memory-reset to clear them).
    pub fn all_registered_segments(&self) -> Vec<u32> {
        self.requirements.iter().flat_map(|r| r.segments.iter().copied()).collect()
    }

    /// Mark all on_load callbacks as not-yet-run (after a memory reset so they
    /// re-fire once the data comes back).
    pub fn reset_load_state(&mut self) {
        for req in &mut self.requirements {
            req.loaded = false;
        }
    }

    // ── End-of-tick flush ────────────────────────────────────────────────

    fn flush(&mut self) {
        let segments: Vec<u8> = self.requests.iter().map(|&s| s as u8).collect();
        raw_memory::set_active_segments(&segments);
        self.requests.clear();
        self.active = None;
    }

    // ── Load-callback helpers (used by run_pending_segment_loads) ────────

    fn pending_load_indices(&mut self) -> Vec<usize> {
        // Collect candidates, then check is_active (which may lazily populate self.active).
        let candidates: Vec<(usize, Vec<u32>)> = self
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, req)| !req.loaded && req.on_load.is_some())
            .map(|(i, req)| (i, req.segments.clone()))
            .collect();

        candidates
            .into_iter()
            .filter(|(_, segs)| segs.iter().all(|&seg| self.is_active(seg)))
            .map(|(i, _)| i)
            .collect()
    }

    fn take_callback(&mut self, index: usize) -> Option<SegmentLoadFn> {
        self.requirements[index].on_load.take()
    }

    fn return_callback(&mut self, index: usize, cb: SegmentLoadFn) {
        self.requirements[index].on_load = Some(cb);
        self.requirements[index].loaded = true;
    }
}

// ─── Load-callback orchestration ─────────────────────────────────────────────

/// Run pending segment load callbacks. Temporarily removes the `MemoryArbiter`
/// from the world so callbacks can have exclusive `&mut World` access, then
/// re-inserts it.
pub fn run_pending_segment_loads(world: &mut World) {
    let mut arbiter: MemoryArbiter = world.remove::<MemoryArbiter>().expect("MemoryArbiter missing");

    let pending = arbiter.pending_load_indices();

    for idx in pending {
        if let Some(cb) = arbiter.take_callback(idx) {
            // Arbiter is out of the world — callback has free access to &mut World.
            cb(world);
            arbiter.return_callback(idx, cb);
        }
    }

    world.insert(arbiter);
}

// ─── System ──────────────────────────────────────────────────────────────────

#[derive(SystemData)]
pub struct MemoryArbiterSystemData<'a> {
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
}

pub struct MemoryArbiterSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MemoryArbiterSystem {
    type SystemData = MemoryArbiterSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.memory_arbiter.flush();
    }
}
