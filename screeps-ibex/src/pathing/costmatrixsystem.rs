use screeps::*;
use screeps_rover::*;
use specs::prelude::*;

pub struct CostMatrixStorageInterface;

impl CostMatrixStorage for CostMatrixStorageInterface {
    fn get_cache(&self, segment: u8) -> Result<CostMatrixCache, String> {
        let segments = RawMemory::segments();
        let raw_data = segments.get(segment).ok_or("Cost matrix memory segment not active")?;
        let raw_data: String = raw_data.into();

        let res = crate::serialize::decode_from_string(&raw_data)?;

        Ok(res)
    }

    fn set_cache(&mut self, segment: u8, data: &CostMatrixCache) -> Result<(), String> {
        let encoded = crate::serialize::encode_to_string(data)?;
        let encoded = encoded.into();

        let segments = RawMemory::segments();
        
        segments.set(segment, encoded);

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
