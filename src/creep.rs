use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use log::*;
use crate::serialize::*;

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
    type SystemData = (Entities<'a>, WriteStorage<'a, CreepSpawning>, WriteStorage<'a, CreepOwner>);

    fn run(&mut self, (entities, mut creep_spawning, mut creep_owner): Self::SystemData) {
        let mut ready_creeps = Vec::new();

        for (entity, spawning) in (&entities, &creep_spawning).join() {
            if let Some(creep) = game::creeps::get(&spawning.name) {
                if !creep.spawning() {
                    ready_creeps.push((entity, creep.id()));
                }
            } else {
                warn!("Deleting entity for spawning creep as it no longer exists. Name: {}", spawning.name);

                if let Err(error) = entities.delete(entity) {
                    warn!("Failed to delete creep entity that was stale. Error: {}", error);
                }
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
    type SystemData = (Entities<'a>, ReadStorage<'a, CreepOwner>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, creeps, _updater): Self::SystemData) {
        for (entity, creep) in (&entities, &creeps).join() {
            let delete = if let Some(creep) = creep.owner.resolve() {
                creep.hits() == 0 
            } else {
                true
            };

            if delete {
                if let Err(error) = entities.delete(entity) {
                    warn!(
                        "Failed to delete creep entity that had been deleted by the simulation. Error: {}",
                        error
                    );
                }
            }
        }
    }
}

pub struct SpawnBodyDefinition<'a> {
    pub maximum_energy: u32,
    pub minimum_repeat: Option<usize>,
    pub maximum_repeat: Option<usize>,
    pub pre_body: &'a [Part],
    pub repeat_body: &'a [Part],
    pub post_body: &'a [Part],
}

pub mod spawning {
    use super::*;

    pub fn build<B>(builder: B, name: &str) -> B
    where
        B: Builder + MarkedBuilder,
    {
        builder.marked::<SerializeMarker>().with(CreepSpawning::new(&name))
    }

    //TODO: Move this to a utility location.
    fn clamp<T: PartialOrd>(val: T, min: T, max: T) -> T {
        if val < min {
            min
        } else if val > max {
            max
        } else {
            val
        }
    }

    #[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
    pub fn create_body(definition: &SpawnBodyDefinition) -> Result<Vec<Part>, ()> {
        let pre_body_cost: u32 = definition.pre_body.iter().map(|p| p.cost()).sum();
        let post_body_cost: u32 = definition.post_body.iter().map(|p| p.cost()).sum();

        let fixed_body_cost = pre_body_cost + post_body_cost;

        if fixed_body_cost > definition.maximum_energy {
            return Err(());
        }

        let repeat_body_cost: u32 = definition.repeat_body.iter().map(|p| p.cost()).sum();

        let remaining_available_energy: u32 = definition.maximum_energy - fixed_body_cost;

        let max_possible_repeat_parts_by_cost = ((remaining_available_energy as f32) / (repeat_body_cost as f32)).floor() as usize;

        let fixed_body_length = definition.pre_body.len() + definition.post_body.len();
        if fixed_body_length > MAX_CREEP_SIZE as usize {
            return Err(());
        }

        let max_possible_repeat_parts_by_length = if !definition.repeat_body.is_empty() {
            (MAX_CREEP_SIZE as usize - fixed_body_length) / definition.repeat_body.len()
        } else {
            0usize
        };

        let max_possible_repeat_parts = max_possible_repeat_parts_by_cost.min(max_possible_repeat_parts_by_length);

        if let Some(min_parts) = definition.minimum_repeat {
            if max_possible_repeat_parts < min_parts {
                return Err(());
            }
        }

        let repeat_parts = clamp(
            max_possible_repeat_parts,
            definition.minimum_repeat.unwrap_or(0),
            definition.maximum_repeat.unwrap_or(usize::max_value()),
        );

        let full_repeat_body = definition
            .repeat_body
            .iter()
            .cycle()
            .take(repeat_parts * definition.repeat_body.len());

        let body = definition
            .pre_body
            .iter()
            .chain(full_repeat_body)
            .chain(definition.post_body.iter())
            .cloned()
            .collect::<Vec<Part>>();

        Ok(body)
    }
}
