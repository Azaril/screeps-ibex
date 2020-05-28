use screeps::*;
use screeps_rover::*;
use specs::prelude::*;

pub struct CostMatrixStorageInterface;

impl CostMatrixStorage for CostMatrixStorageInterface {
    fn get_cache(&self, segment: u32) -> Result<CostMatrixCache, String> {
        let raw_data = raw_memory::get_segment(segment).ok_or("Cost matrix memory segment not active")?;

        let res = crate::serialize::decode_from_string(&raw_data)?;

        Ok(res)
    }

    fn set_cache(&mut self, segment: u32, data: &CostMatrixCache) -> Result<(), String> {
        let encoded = crate::serialize::encode_to_string(data)?;

        raw_memory::set_segment(segment, &encoded);

        Ok(())
    }
}

#[derive(SystemData)]
pub struct CostMatrixStoreSystemData<'a> {
    cost_matrix: WriteExpect<'a, CostMatrixSystem>,
}

pub struct CostMatrixStoreSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CostMatrixStoreSystem {
    type SystemData = CostMatrixStoreSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.cost_matrix.flush_storage();
    }
}
