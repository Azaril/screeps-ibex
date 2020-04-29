use super::costmatrix::*;
use screeps::*;
use screeps::pathfinder::CostMatrix;
use std::collections::HashMap;
use crate::cache::*;
use serde::*;
use specs::*;
use specs::prelude::*;

#[derive(Serialize, Deserialize)]
pub struct CostMatrixTypeCache<T> {
    last_updated: u32,
    data: T
}

#[derive(Serialize, Deserialize)]
pub struct CostMatrixRoomEntry {
    structures: Option<CostMatrixTypeCache<LinearCostMatrix>>,
    #[serde(skip)]
    friendly_creeps: Option<CostMatrixTypeCache<LinearCostMatrix>>,
    #[serde(skip)]
    hostile_creeps: Option<CostMatrixTypeCache<LinearCostMatrix>>,
}

impl CostMatrixRoomEntry {
    pub fn new() -> CostMatrixRoomEntry {
        CostMatrixRoomEntry {
            structures: None,
            friendly_creeps: None,
            hostile_creeps: None
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct CostMatrixCache {
    rooms: HashMap<RoomName, CostMatrixRoomEntry>
}

pub trait CostMatrixStorage {
    fn get_cache(&self, segment: u32) -> Result<CostMatrixCache, String>;

    fn set_cache(&mut self, segment:u32, data: &CostMatrixCache) -> Result<(), String>;
}

pub struct CostMatrixConfiguration {
    pub structures: bool,
    pub friendly_creeps: bool,
    pub hostile_creeps: bool,
}

pub struct CostMatrixSystem {
    storage: Box<dyn CostMatrixStorage>,
    storage_segment: u32,
    cache: Option<CostMatrixCache>
}

impl CostMatrixSystem {
    pub fn new(storage: Box<dyn CostMatrixStorage>, storage_segment: u32) -> CostMatrixSystem {
        CostMatrixSystem {
            storage,
            storage_segment,
            cache: None
        }
    }

    pub fn flush_storage(&mut self) {
        let storage = &mut self.storage;
        let cache = &self.cache;
        let storage_segment = self.storage_segment;

        cache.as_ref().map(|c| storage.set_cache(storage_segment, c));        
    }
    
    pub fn apply_cost_matrix(&mut self, room_name: RoomName, cost_matrix: &mut CostMatrix, configuration: &CostMatrixConfiguration) -> Result<(), String> {
        let cache = self.get_cache();

        cache.apply_cost_matrix(room_name, cost_matrix, configuration)
    }

    fn get_cache(&mut self) -> &mut CostMatrixCache {
        let cache = &mut self.cache;
        let storage = &mut self.storage;
        let storage_segment = self.storage_segment;

        cache.get_or_insert_with(|| storage.get_cache(storage_segment).unwrap_or_default())
    }
}

impl Default for CostMatrixCache {
    fn default() -> CostMatrixCache {
        CostMatrixCache {
            rooms: HashMap::new()
        }
    }
}

impl CostMatrixCache {
    fn get_room(&mut self, room_name: RoomName) -> CostMatrixRoomAccessor {
        let entry = self.rooms
            .entry(room_name)
            .or_insert_with(CostMatrixRoomEntry::new);

        CostMatrixRoomAccessor {
            room_name,
            entry
        }
    }

    pub fn apply_cost_matrix(&mut self, room_name: RoomName, cost_matrix: &mut CostMatrix, configuration: &CostMatrixConfiguration) -> Result<(), String> {
        let mut room = self.get_room(room_name);

        if configuration.structures {
            let structures = room.get_structures().ok_or("Structures not available")?;
            structures.apply_to(&mut *cost_matrix);
        }

        if configuration.friendly_creeps {
            let friendly_creeps = room.get_friendly_creeps().ok_or("Friendly creeps not available")?;
            friendly_creeps.apply_to(&mut *cost_matrix);
        }
        
        if configuration.hostile_creeps {
            let hostile_creeps = room.get_hostile_creeps().ok_or("Hostile creeps not available")?;
            hostile_creeps.apply_to(&mut *cost_matrix);
        }

        Ok(())
    }
}

pub struct CostMatrixRoomAccessor<'a> {
    room_name: RoomName,
    entry: &'a mut CostMatrixRoomEntry
}

impl<'a> CostMatrixRoomAccessor<'a> {
    pub fn get_structures(&mut self) -> Option<&LinearCostMatrix> {
        let room_name = self.room_name;

        let expiration = move |data: &CostMatrixTypeCache<_>| {
            game::time() - data.last_updated > 0 && game::rooms::get(room_name).is_some()
        };

        let filler = move || {
            let room = game::rooms::get(room_name)?;

            let mut matrix = LinearCostMatrix::new();

            let structures = room.find(find::STRUCTURES);

            for structure in structures.iter() {
                let cost = match structure {
                    Structure::Rampart(_) | Structure::Road(_) => {
                        None
                    }
                    Structure::Container(_) => {
                        Some(2)
                    },
                    _ => {
                        Some(u8::MAX)
                    }
                };

                if let Some(cost) = cost {
                    let pos = structure.pos();

                    matrix.set(pos.x() as u8, pos.y() as u8, cost);
                }
            }

            let entry = CostMatrixTypeCache {
                last_updated: game::time(),
                data: matrix
            };

            Some(entry)
        };

        self.entry.structures.maybe_access(expiration, filler).get().map(|d| &d.data)
    }

    pub fn get_friendly_creeps(&mut self) -> Option<&LinearCostMatrix> {
        let expiration = |data: &CostMatrixTypeCache<_>| game::time() - data.last_updated > 0;
        let room_name = self.room_name;
        let filler = move || {
            let room = game::rooms::get(room_name)?;

            let mut matrix = LinearCostMatrix::new();

            for creep in room.find(find::MY_CREEPS).iter() {
                let pos = creep.pos();

                matrix.set(pos.x() as u8, pos.y() as u8, u8::MAX);
            }

            for power_creep in room.find(find::MY_POWER_CREEPS).iter() {
                let pos = power_creep.pos();

                matrix.set(pos.x() as u8, pos.y() as u8, u8::MAX);
            }

            let entry = CostMatrixTypeCache {
                last_updated: game::time(),
                data: matrix
            };

            Some(entry)
        };

        self.entry.friendly_creeps.maybe_access(expiration, filler).get().map(|d| &d.data)
    }

    pub fn get_hostile_creeps(&mut self) -> Option<&LinearCostMatrix> {
        let expiration = |data: &CostMatrixTypeCache<_>| game::time() - data.last_updated > 0;
        let room_name = self.room_name;
        let filler = move || {
            let room = game::rooms::get(room_name)?;

            let mut matrix = LinearCostMatrix::new();

            for creep in room.find(find::HOSTILE_CREEPS).iter() {
                let pos = creep.pos();

                matrix.set(pos.x() as u8, pos.y() as u8, u8::MAX);
            }

            let entry = CostMatrixTypeCache {
                last_updated: game::time(),
                data: matrix
            };

            Some(entry)
        };

        self.entry.hostile_creeps.maybe_access(expiration, filler).get().map(|d| &d.data)
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