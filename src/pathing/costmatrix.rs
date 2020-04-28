use std::collections::HashMap;
use crate::location::*;
use screeps::pathfinder::CostMatrix;

pub trait CostMatrixUpload {
    fn apply_to(target: &CostMatrix);
}

pub trait CostMatrixWrite {
    fn set(&mut self, x: u8, y: u8, val: u8);
}

pub struct SparseCostMatrix {
    data: HashMap<Location, u8>
}

impl CostMatrixWrite for SparseCostMatrix {
    fn set(&mut self, x: u8, y: u8, val: u8) { 
        self.data.insert(Location::from_coords(x as u32, y as u32), val);
    }
}