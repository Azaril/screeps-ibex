use crate::cleanup::EntityCleanupQueue;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, Component, Serialize, Deserialize)]
pub struct CreepOwner {
    pub owner: ObjectId<Creep>,
}

impl CreepOwner {
    pub fn new(creep_id: ObjectId<Creep>) -> CreepOwner {
        CreepOwner { owner: creep_id }
    }

    pub fn id(&self) -> ObjectId<Creep> {
        self.owner
    }
}

#[derive(Clone, Component, Serialize, Deserialize)]
pub struct CreepSpawning {
    pub name: String,
}

impl CreepSpawning {
    pub fn new(pending_name: &str) -> CreepSpawning {
        CreepSpawning {
            name: pending_name.to_string(),
        }
    }
}

pub struct WaitForSpawnSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for WaitForSpawnSystem {
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, CreepSpawning>,
        WriteStorage<'a, CreepOwner>,
        Write<'a, EntityCleanupQueue>,
    );

    fn run(&mut self, (entities, mut creep_spawning, mut creep_owner, mut cleanup_queue): Self::SystemData) {
        let mut ready_creeps = Vec::new();

        for (entity, spawning) in (&entities, &creep_spawning).join() {
            if let Some(creep) = game::creeps().get(spawning.name.clone()) {
                if !creep.spawning() {
                    if let Some(id) = creep.try_id() {
                        ready_creeps.push((entity, id));
                    } else {
                        warn!("Creep {} has no id, skipping spawn completion", spawning.name);
                    }
                }
            } else {
                // The spawn never produced a creep (cancelled/failed). Route the
                // deletion through the cleanup queue -- NOT a direct
                // `entities.delete()` -- so `EntityCleanupSystem` first strips this
                // entity from every mission (`remove_creep`) and its owning
                // `SquadContext` (`members.retain`). A direct delete here leaves
                // those holders with a dangling `Entity`, which then panics at
                // serialize time (`specs` `ConvertSaveload for Entity` unwraps the
                // missing marker -- saveload/mod.rs:182). The cleanup prepass runs
                // immediately after this system and before serialization.
                warn!("Queuing cleanup for spawning creep as it no longer exists. Name: {}", spawning.name);

                cleanup_queue.delete_creep(entity);
            }
        }

        for (entity, creep_id) in ready_creeps {
            creep_spawning.remove(entity);
            let _ = creep_owner.insert(entity, CreepOwner::new(creep_id));
        }
    }
}

pub struct CleanupCreepsSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CleanupCreepsSystem {
    type SystemData = (Entities<'a>, ReadStorage<'a, CreepOwner>, Write<'a, EntityCleanupQueue>);

    fn run(&mut self, (entities, creeps, mut cleanup_queue): Self::SystemData) {
        for (entity, creep) in (&entities, &creeps).join() {
            let delete = if let Some(creep) = creep.owner.resolve() {
                creep.hits() == 0
            } else {
                true
            };

            if delete {
                cleanup_queue.delete_creep(entity);
            }
        }
    }
}

// `SpawnBodyDefinition` + `create_body` are pure body-construction; they now live in
// `screeps-combat-decision::spawning` so the sim/eval can build the bot's real bodies without
// depending on the whole bot. Re-exported here so existing `crate::creep::SpawnBodyDefinition` /
// `crate::creep::spawning::create_body` call sites are unchanged.
pub use screeps_combat_decision::spawning::SpawnBodyDefinition;

pub mod spawning {
    use super::*;

    // The pure body builder lives in the shared decision crate; re-export at the original path.
    pub use screeps_combat_decision::spawning::create_body;

    pub fn build<B>(builder: B, name: &str) -> B
    where
        B: Builder + MarkedBuilder,
    {
        builder.marked::<SerializeMarker>().with(CreepSpawning::new(name))
    }
}

// The `create_body` clamping pins moved with the code to
// `screeps_combat_decision::spawning` (see that module's tests).
