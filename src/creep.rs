use serde::*;
use specs::*;
use screeps::*;

pub struct CreepMarker;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct CreepOwner {
    pub owner: ObjectId<Creep>
}

impl CreepOwner {
    pub fn new(creep: &Creep) -> CreepOwner {
        CreepOwner {
            owner: creep.id()
        }
    }
}

impl Component for CreepOwner {
    type Storage = HashMapStorage<Self>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreepSpawning {
    pub name: String
}

impl CreepSpawning {
    pub fn new(pending_name: &str) -> CreepSpawning {
        CreepSpawning {
            name: pending_name.to_string()
        }
    }
}

impl Component for CreepSpawning {
    type Storage = HashMapStorage<Self>;
}

pub struct WaitForSpawnSystem;

impl<'a> System<'a> for WaitForSpawnSystem {
    type SystemData = (Entities<'a>, ReadStorage<'a, CreepSpawning>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, spawnings, updater): Self::SystemData) {
        for (entity, spawning) in (&entities, &spawnings).join() {
            if let Some(creep) = game::creeps::get(&spawning.name) {
                if !creep.spawning() {
                    updater.remove::<CreepSpawning>(entity);
                    updater.insert(entity, CreepOwner::new(&creep));
                }
            }

            //TODO: Delete entity if matching creep cannot be found?
        }
    }
}