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
    type SystemData = (Entities<'a>, WriteStorage<'a, CreepSpawning>, WriteStorage<'a, CreepOwner>);

    fn run(&mut self, (entities, mut creep_spawning, mut creep_owner): Self::SystemData) {
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
        builder.marked::<SerializeMarker>().with(CreepSpawning::new(name))
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
            definition.maximum_repeat.unwrap_or(usize::MAX),
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

#[cfg(test)]
mod tests {
    use super::*;

    // Pins for `create_body`'s clamping behavior. The review's body-sizing
    // seed (IBEX-022) was REFUTED -- the min-cost clamp is present and
    // correct. These tests lock it so Phase 0 (and later refactors) cannot
    // silently break it.

    #[test]
    fn create_body_rejects_unaffordable_fixed_body() {
        // Work (100) + Move (50) = 150 > 100 maximum energy.
        let definition = SpawnBodyDefinition {
            maximum_energy: 100,
            minimum_repeat: None,
            maximum_repeat: None,
            pre_body: &[Part::Work],
            repeat_body: &[],
            post_body: &[Part::Move],
        };

        assert!(spawning::create_body(&definition).is_err());
    }

    #[test]
    fn create_body_rejects_unaffordable_minimum_repeat() {
        // 3 x (Work + Move) = 450 > 300 maximum energy.
        let definition = SpawnBodyDefinition {
            maximum_energy: 300,
            minimum_repeat: Some(3),
            maximum_repeat: None,
            pre_body: &[],
            repeat_body: &[Part::Work, Part::Move],
            post_body: &[],
        };

        assert!(spawning::create_body(&definition).is_err());
    }

    #[test]
    fn create_body_clamps_repeat_to_maximum_repeat() {
        // Plenty of energy, but maximum_repeat caps the body at 2 repeats.
        let definition = SpawnBodyDefinition {
            maximum_energy: 10_000,
            minimum_repeat: Some(1),
            maximum_repeat: Some(2),
            pre_body: &[],
            repeat_body: &[Part::Work, Part::Move],
            post_body: &[],
        };

        let body = spawning::create_body(&definition).expect("expected body");

        assert_eq!(body, vec![Part::Work, Part::Move, Part::Work, Part::Move]);
    }

    #[test]
    fn create_body_clamps_repeat_to_energy_budget() {
        // 500 energy / 150 per (Work + Move) repeat = 3 full repeats.
        let definition = SpawnBodyDefinition {
            maximum_energy: 500,
            minimum_repeat: Some(1),
            maximum_repeat: None,
            pre_body: &[],
            repeat_body: &[Part::Work, Part::Move],
            post_body: &[],
        };

        let body = spawning::create_body(&definition).expect("expected body");

        assert_eq!(body.len(), 6);
        let cost: u32 = body.iter().map(|p| p.cost()).sum();
        assert!(cost <= 500, "body cost {} exceeded energy budget", cost);
    }

    #[test]
    fn create_body_caps_total_parts_at_max_creep_size() {
        // Effectively unlimited energy: the length clamp must hold.
        let definition = SpawnBodyDefinition {
            maximum_energy: 1_000_000,
            minimum_repeat: Some(1),
            maximum_repeat: None,
            pre_body: &[Part::Carry],
            repeat_body: &[Part::Move],
            post_body: &[Part::Carry],
        };

        let body = spawning::create_body(&definition).expect("expected body");

        assert!(body.len() <= MAX_CREEP_SIZE as usize);
    }
}
