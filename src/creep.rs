use serde::*;
use specs::*;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Component)]
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

#[derive(Clone, Debug, Deserialize, Serialize, Component)]
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

pub struct WaitForSpawnSystem;

impl<'a> System<'a> for WaitForSpawnSystem {
    type SystemData = (
        Entities<'a>, 
        ReadStorage<'a, CreepSpawning>, 
        Read<'a, LazyUpdate>
    );

    fn run(&mut self, (entities, spawnings, updater): Self::SystemData) {
        scope_timing!("WaitForSpawnSystem");

        for (entity, spawning) in (&entities, &spawnings).join() {
            if let Some(creep) = game::creeps::get(&spawning.name) {
                if !creep.spawning() {
                    updater.remove::<CreepSpawning>(entity);
                    updater.insert(entity, CreepOwner::new(&creep));
                }
            } else {
                warn!("Deleting entity for spawning creep as it no longer exists. Name: {}", spawning.name);

                updater.exec_mut(move |world| {
                    if let Err(error) = world.delete_entity(entity) {
                        warn!("Failed to delete creep entity that was stale. Error: {}", error);
                    }
                });
            }
        }
    }
}

pub struct CleanupCreepsSystem;

impl<'a> System<'a> for CleanupCreepsSystem {
    type SystemData = (
        Entities<'a>, 
        ReadStorage<'a, CreepOwner>, 
        Read<'a, LazyUpdate>
    );

    fn run(&mut self, (entities, creeps, updater): Self::SystemData) {
        scope_timing!("CleanupCreepsSystem");

        for (entity, creep) in (&entities, &creeps).join() {
            if let None = creep.owner.resolve() {
                updater.exec_mut(move |world| {
                    if let Err(error) = world.delete_entity(entity) {
                        warn!("Failed to delete creep entity that had been deleted by the simulation. Error: {}", error);
                    }
                });
            }
        }
    }
}

pub struct Spawning;

impl Spawning
{
    pub fn build<B>(builder: B, name: &str) -> B where B: Builder + MarkedBuilder {
        builder
            .marked::<::serialize::SerializeMarker>()
            .with(CreepSpawning::new(&name))
    }
}