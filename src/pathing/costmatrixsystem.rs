use screeps_rover::*;
use specs::prelude::*;

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
