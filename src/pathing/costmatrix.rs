use std::collections::HashMap;
use crate::location::*;
use screeps::pathfinder::*;
use serde::*;

pub trait CostMatrixApply {
    fn apply_to<T>(&self, target: T) where T: CostMatrixSet;
}

pub trait CostMatrixWrite {
    fn set(&mut self, x: u8, y: u8, val: u8);
}

pub trait CostMatrixRead {
    fn get(&self, x: u8, y: u8) -> u8;
}

#[derive(Serialize, Deserialize)]
pub struct SparseCostMatrix {
    data: HashMap<Location, u8>
}

impl CostMatrixWrite for SparseCostMatrix {
    fn set(&mut self, x: u8, y: u8, val: u8) { 
        self.data.insert(Location::from_coords(x as u32, y as u32), val);
    }
}

impl CostMatrixRead for SparseCostMatrix {
    fn get(&self, x: u8, y: u8) -> u8 {
        self.data.get(&Location::from_coords(x as u32, y as u32)).copied().unwrap_or(0)
    }
}

impl CostMatrixApply for SparseCostMatrix {
    fn apply_to<T>(&self, target: T) where T: CostMatrixSet {
        target.set_multi(self.data.iter());
    }
}

#[derive(Serialize, Deserialize)]
pub struct LinearCostMatrix {
    data: Vec<(Location, u8)>
}

impl LinearCostMatrix {
    pub fn new() -> LinearCostMatrix {
        LinearCostMatrix {
            data: Vec::new()
        }
    }
}

impl CostMatrixWrite for LinearCostMatrix {
    fn set(&mut self, x: u8, y: u8, val: u8) { 
        self.data.push((Location::from_coords(x as u32, y as u32), val));
    }
}

impl CostMatrixApply for LinearCostMatrix {
    fn apply_to<T>(&self, target: T) where T: CostMatrixSet {
        target.set_multi(self.data.iter());
    }
}