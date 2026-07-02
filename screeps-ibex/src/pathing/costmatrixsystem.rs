use crate::memorysystem::MemoryArbiter;
use crate::segments::COST_MATRIX_SEGMENT;
use screeps::*;
use screeps_rover::*;
use specs::prelude::*;

/// Load a `CostMatrixCache` from the given RawMemory segment, or return a
/// default (empty) cache if the segment is not active or cannot be decoded.
///
/// Reads `RawMemory` directly (not through the [`MemoryArbiter`]) because this
/// runs at environment creation — before the world, and therefore the arbiter,
/// exists — to warm the route cache on the most starved (post-reset) tick
/// (EP-4.8). The WRITE side ([`CostMatrixStoreSystem`]) routes through the
/// arbiter's 10-key touch budget (REC-046).
pub fn load_cost_matrix_cache(segment: u32) -> CostMatrixCache {
    let result: Result<CostMatrixCache, String> = (|| {
        let raw_data = raw_memory::segments()
            .get(segment as u8)
            .ok_or("Cost matrix memory segment not active")?;

        crate::serialize::decode_from_string(&raw_data)
    })();

    result.unwrap_or_default()
}

/// Save a `CostMatrixCache` to `segment` via the [`MemoryArbiter`] (REC-046).
/// The arbiter owns the engine's 10-segments-touched budget — an 11th key
/// throws and discards the whole end-of-tick save — so no subsystem may write a
/// segment with a raw `raw_memory::segments().set` that bypasses the guard
/// (EP-2.8, EP-9.7). `COST_MATRIX_SEGMENT` is a registered, always-requested
/// requirement (`game_loop.rs`), so it is loaded-active and `set` lands
/// immediately — the same pattern `stats_history`/`metrics` use.
pub fn save_cost_matrix_cache(arbiter: &mut MemoryArbiter, segment: u32, cache: &CostMatrixCache) {
    if let Ok(encoded) = crate::serialize::encode_to_string(cache) {
        arbiter.set(segment, &encoded);
    }
}

#[derive(SystemData)]
pub struct CostMatrixStoreSystemData<'a> {
    cost_matrix: WriteExpect<'a, CostMatrixCache>,
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
}

#[derive(SystemData)]
pub struct CostMatrixClearSystemData<'a> {
    cost_matrix: WriteExpect<'a, CostMatrixCache>,
}

/// Clears ephemeral per-tick cost matrix data (construction sites, creeps)
/// at the start of each tick so stale data is not reused. Persisted structure
/// data is retained.
pub struct CostMatrixClearSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CostMatrixClearSystem {
    type SystemData = CostMatrixClearSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.cost_matrix.clear_ephemeral();
    }
}

/// Serializes the cost matrix cache to a RawMemory segment at the end of
/// each tick.
pub struct CostMatrixStoreSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CostMatrixStoreSystem {
    type SystemData = CostMatrixStoreSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        save_cost_matrix_cache(&mut data.memory_arbiter, COST_MATRIX_SEGMENT, &data.cost_matrix);
    }
}
