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
    /// Queued segment writes awaiting a slot in the shared 10-touch budget
    /// (see `queue_write`). Keyed by segment; a newer payload replaces an
    /// older pending one.
    pending_writes: std::collections::HashMap<u32, String>,
    /// Declared segment requirements (registered during environment creation).
    requirements: Vec<SegmentRequirement>,
    /// In-memory segment backing for host tests (P1.A7 / ADR 0015 F3):
    /// when set, get/set/is_active never touch RawMemory, making the
    /// segment pipeline kernel-testable. Test-only by construction —
    /// no production constructor sets it.
    #[cfg(test)]
    fake_segments: Option<std::collections::HashMap<u32, String>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MemoryArbiter {
    pub fn new() -> MemoryArbiter {
        MemoryArbiter {
            active: None,
            requests: HashSet::new(),
            pending_writes: std::collections::HashMap::new(),
            requirements: Vec::new(),
            #[cfg(test)]
            fake_segments: None,
        }
    }

    /// The F3 test double (ADR 0015): every segment id reads as active
    /// and reads/writes hit an in-memory map instead of RawMemory. The
    /// Inc-2 round-trip/corpus/fuzz suites build on this.
    #[cfg(test)]
    pub fn test_double() -> MemoryArbiter {
        MemoryArbiter {
            active: Some((0..100).collect()),
            requests: HashSet::new(),
            pending_writes: std::collections::HashMap::new(),
            requirements: Vec::new(),
            fake_segments: Some(std::collections::HashMap::new()),
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
        #[cfg(test)]
        if self.fake_segments.is_some() {
            return self.active.as_ref().map(|a| a.contains(&segment)).unwrap_or(false);
        }
        self.active
            .get_or_insert_with(|| raw_memory::segments().keys().map(|k| k as u32).collect())
            .contains(&segment)
    }

    pub fn get(&self, segment: u32) -> Option<String> {
        #[cfg(test)]
        if let Some(fake) = &self.fake_segments {
            return fake.get(&segment).cloned();
        }
        raw_memory::segments().get(segment as u8)
    }

    pub fn set(&mut self, segment: u32, data: &str) {
        if data.len() > 50 * 1024 {
            error!("Memory segment too large - Segment: {} - Data: {}", segment, data);
        }

        #[cfg(test)]
        if let Some(fake) = &mut self.fake_segments {
            fake.insert(segment, data.to_owned());
            return;
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

    /// Queue a segment write that lands as soon as the engine's shared
    /// 10-segments-touched budget allows — required for any writer whose
    /// segment is NOT in the always-active set (which is 10 of 10 in steady
    /// state, so an immediate write would never fit). Loaded-active segments
    /// and writes share ONE per-tick cap: the runtime save loop counts every
    /// key in `RawMemory.segments` and an 11th key THROWS, discarding the
    /// whole end-of-tick save (`driver/runtime/runtime.js:250-268`;
    /// engine-mechanics §9.1).
    ///
    /// Mechanics: `flush` first tries to land queued writes against the
    /// tick's final key set; anything that does not fit gets a next-tick
    /// active-slot reservation that displaces the highest-id request for one
    /// tick (every ad-hoc requester gates its read AND write on `is_active`,
    /// so the displaced segment simply skips a tick). A newer payload queued
    /// for the same segment replaces the older pending one. Caveat: the
    /// ascending-id priority means a queued write can only displace ids
    /// HIGHER than its own.
    pub fn queue_write(&mut self, segment: u32, data: String) {
        self.pending_writes.insert(segment, data);
    }

    /// Write a segment if it fits the touch budget this tick: the key
    /// already exists (loaded-active or already written), or there is a free
    /// slot. Returns `false` when the write does not fit.
    fn try_set(&mut self, segment: u32, data: &str) -> bool {
        #[cfg(test)]
        if self.fake_segments.is_some() {
            self.set(segment, data);
            return true;
        }

        let keys: Vec<u32> = raw_memory::segments().keys().map(|k| k as u32).collect();
        if !keys.contains(&segment) && keys.len() >= 10 {
            return false;
        }

        self.set(segment, data);
        true
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
        // Land queued writes that fit the tick's FINAL key set (every system
        // has run by now). A write to an existing key never grows the set.
        let pending: Vec<u32> = self.pending_writes.keys().copied().collect();
        for segment in pending {
            if let Some(data) = self.pending_writes.remove(&segment) {
                if !self.try_set(segment, &data) {
                    self.pending_writes.insert(segment, data);
                }
            }
        }

        let pending_ids: Vec<u32> = self.pending_writes.keys().copied().collect();
        let (segments, displaced) = plan_active_set(&self.requests, &pending_ids);

        if self.requests.len() > 10 {
            // Genuine over-cap of read requests — `setActiveSegments` throws
            // outright on an 11-id list (`driver/runtime/runtime.js:134-136`),
            // which would trap the tick. Clamped to the lowest ids: core
            // world state (50-54) outranks caches, which outrank telemetry.
            warn!(
                "Active-segment requests exceed the engine cap of 10 - deferring segments {:?} to a later tick",
                displaced
            );
        } else if !displaced.is_empty() {
            // Designed one-tick displacement reserving key slots for queued
            // writes (the displaced owners gate on is_active and skip a tick).
            debug!(
                "Reserving active-segment slot(s) for queued write(s) {:?} - displacing {:?} for one tick",
                pending_ids, displaced
            );
        }

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

/// Plan the next tick's active-segment list: every requested id, plus a
/// reserved slot for each queued-write segment (its key must exist next tick
/// for the write to fit the engine's shared 10-touch budget), clamped to the
/// engine cap of 10 by dropping the HIGHEST ids first — the registry is
/// ordered so lower ids matter more (world state 50-54 < caches < planner <
/// telemetry 99). Returns `(active list, displaced ids)`.
///
/// Pure so the reservation arithmetic is host-testable. Limitation, by the
/// same ascending-id rule: a queued write can only displace ids higher than
/// its own.
fn plan_active_set(requests: &HashSet<u32>, pending_writes: &[u32]) -> (Vec<u8>, Vec<u32>) {
    let mut ids: Vec<u32> = requests.iter().copied().collect();
    for &segment in pending_writes {
        if !requests.contains(&segment) {
            ids.push(segment);
        }
    }
    ids.sort_unstable();
    ids.dedup();

    let displaced = if ids.len() > 10 { ids.split_off(10) } else { Vec::new() };
    (ids.into_iter().map(|id| id as u8).collect(), displaced)
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

#[cfg(test)]
mod arbiter_double_tests {
    use super::*;

    /// P1.A7 / ADR 0015 F3: the segment pipeline runs entirely
    /// in-memory under the double — the substrate the Inc-2
    /// round-trip/corpus/fuzz suites consume.
    #[test]
    fn test_double_round_trips_segments_without_js() {
        let mut arbiter = MemoryArbiter::test_double();
        assert!(arbiter.is_active(57));
        assert_eq!(arbiter.get(57), None);
        arbiter.set(57, r#"{"v":1}"#);
        assert_eq!(arbiter.get(57).as_deref(), Some(r#"{"v":1}"#));
        // Chunked-style writes across the component range round-trip.
        for (i, seg) in crate::segments::COMPONENT_SEGMENTS.iter().enumerate() {
            arbiter.set(*seg, &format!("chunk-{i}"));
        }
        for (i, seg) in crate::segments::COMPONENT_SEGMENTS.iter().enumerate() {
            assert_eq!(arbiter.get(*seg).as_deref(), Some(format!("chunk-{i}").as_str()));
        }
    }

    /// With a full 10-of-10 active set, a queued write for a non-active
    /// segment must reserve its slot by displacing the highest id (live
    /// stats), never a lower one. (The market segment is always-active
    /// since 2026-06-12; this pins the mechanism for future non-active
    /// writers and the planned periodic-segment rotation.)
    #[test]
    fn plan_active_set_reserves_a_slot_for_a_queued_write() {
        let full: HashSet<u32> = [50, 51, 52, 53, 54, 55, 56, 57, 60, 99].into_iter().collect();

        let (active, displaced) = plan_active_set(&full, &[58]);
        assert!(active.contains(&58));
        assert_eq!(displaced, vec![99]);
        assert_eq!(active.len(), 10);
    }

    #[test]
    fn plan_active_set_without_pressure_changes_nothing() {
        let requests: HashSet<u32> = [50, 51, 52, 53, 54, 55, 56].into_iter().collect();

        // Room to spare: pending write fits without displacement.
        let (active, displaced) = plan_active_set(&requests, &[58]);
        assert!(active.contains(&58));
        assert!(displaced.is_empty());

        // No pending writes: the request set passes through unclamped.
        let (active, displaced) = plan_active_set(&requests, &[]);
        assert_eq!(active.len(), 7);
        assert!(displaced.is_empty());
    }

    #[test]
    fn plan_active_set_clamps_genuine_overflow_to_the_lowest_ids() {
        let over: HashSet<u32> = [50, 51, 52, 53, 54, 55, 56, 57, 58, 60, 99].into_iter().collect();

        let (active, displaced) = plan_active_set(&over, &[]);
        assert_eq!(active.len(), 10);
        assert_eq!(displaced, vec![99]);
    }

    #[test]
    fn plan_active_set_does_not_double_count_a_pending_segment_already_requested() {
        let requests: HashSet<u32> = [50, 51, 52, 53, 54, 55, 56, 57, 58, 60].into_iter().collect();

        let (active, displaced) = plan_active_set(&requests, &[58]);
        assert_eq!(active.len(), 10);
        assert!(displaced.is_empty());
    }

    /// Queued writes land through the double immediately at flush time; a
    /// newer payload replaces an older pending one.
    #[test]
    fn queue_write_buffers_and_replaces() {
        let mut arbiter = MemoryArbiter::test_double();
        arbiter.queue_write(58, "old".to_string());
        arbiter.queue_write(58, "new".to_string());
        assert_eq!(arbiter.pending_writes.len(), 1);
        assert_eq!(arbiter.pending_writes.get(&58).map(String::as_str), Some("new"));
    }
}
