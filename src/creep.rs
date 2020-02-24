use screeps::*;
use serde::*;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, Copy, Deserialize, Serialize, Component)]
pub struct CreepOwner {
    pub owner: ObjectId<Creep>,
}

impl CreepOwner {
    pub fn new(creep: &Creep) -> CreepOwner {
        CreepOwner { owner: creep.id() }
    }
}

#[derive(Clone, Deserialize, Serialize, Component)]
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

impl<'a> System<'a> for WaitForSpawnSystem {
    type SystemData = (Entities<'a>, ReadStorage<'a, CreepSpawning>, Read<'a, LazyUpdate>);

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
    type SystemData = (Entities<'a>, ReadStorage<'a, CreepOwner>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, creeps, updater): Self::SystemData) {
        scope_timing!("CleanupCreepsSystem");

        for (entity, creep) in (&entities, &creeps).join() {
            if creep.owner.resolve().is_none() {
                updater.exec_mut(move |world| {
                    if let Err(error) = world.delete_entity(entity) {
                        warn!(
                            "Failed to delete creep entity that had been deleted by the simulation. Error: {}",
                            error
                        );
                    }
                });
            }
        }
    }
}

pub struct Spawning;

pub struct SpawnBodyDefinition<'a> {
    pub maximum_energy: u32,
    pub minimum_repeat: Option<usize>,
    pub maximum_repeat: Option<usize>,
    pub pre_body: &'a [Part],
    pub repeat_body: &'a [Part],
    pub post_body: &'a [Part],
}

impl Spawning {
    pub fn build<B>(builder: B, name: &str) -> B
    where
        B: Builder + MarkedBuilder,
    {
        builder.marked::<::serialize::SerializeMarker>().with(CreepSpawning::new(&name))
    }

    //TODO: Move this to a utility location.
    pub fn clamp<T: PartialOrd>(val: T, min: T, max: T) -> T {
        if val < min {
            min
        } else if val > max {
            max
        } else {
            val
        }
    }

    pub fn create_body(definition: &SpawnBodyDefinition) -> Result<Vec<Part>, ()> {
        let pre_body_cost: u32 = definition.pre_body.iter().map(|p| p.cost()).sum();
        let post_body_cost: u32 = definition.post_body.iter().map(|p| p.cost()).sum();

        let fixed_body_cost = pre_body_cost + post_body_cost;

        if fixed_body_cost > definition.maximum_energy {
            return Err(());
        }

        let repeat_body_cost: u32 = definition.repeat_body.iter().map(|p| p.cost()).sum();

        let remaining_available_energy: u32 = definition.maximum_energy - fixed_body_cost;

        let max_possible_repeat_parts = ((remaining_available_energy as f32) / (repeat_body_cost as f32)).floor() as usize;

        if let Some(min_parts) = definition.minimum_repeat {
            if max_possible_repeat_parts < min_parts {
                return Err(());
            }
        }

        let repeat_parts = Self::clamp(
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
