use screeps::*;
use screeps_rover::*;
use specs::prelude::*;

/// Segment ID used for cost matrix cache persistence.
pub const COST_MATRIX_SEGMENT: u32 = 55;

/// Load a `CostMatrixCache` from the given RawMemory segment, or return a
/// default (empty) cache if the segment is not active or cannot be decoded.
pub fn load_cost_matrix_cache(segment: u32) -> CostMatrixCache {
    let result: Result<CostMatrixCache, String> = (|| {
        let raw_data = raw_memory::segments()
            .get(segment as u8)
            .ok_or("Cost matrix memory segment not active")?;

        crate::serialize::decode_from_string(&raw_data)
    })();

    result.unwrap_or_default()
}

/// Save a `CostMatrixCache` to the given RawMemory segment.
pub fn save_cost_matrix_cache(segment: u32, cache: &CostMatrixCache) {
    if let Ok(encoded) = crate::serialize::encode_to_string(cache) {
        raw_memory::segments().set(segment as u8, encoded);
    }
}

#[derive(SystemData)]
pub struct CostMatrixStoreSystemData<'a> {
    cost_matrix: WriteExpect<'a, CostMatrixCache>,
}

/// Clears ephemeral per-tick cost matrix data (construction sites, creeps)
/// at the start of each tick so stale data is not reused. Persisted structure
/// data is retained.
pub struct CostMatrixClearSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CostMatrixClearSystem {
    type SystemData = CostMatrixStoreSystemData<'a>;

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

    fn run(&mut self, data: Self::SystemData) {
        save_cost_matrix_cache(COST_MATRIX_SEGMENT, &data.cost_matrix);
    }
}
