use serde::*;
use screeps::*;

use super::jobsystem::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum DeliveryTarget {
    None,
    Spawn(ObjectId<StructureSpawn>),
    Controller(ObjectId<StructureController>),
    Container(ObjectId<StructureContainer>)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: ObjectId<Source>,
    pub delivery_target: DeliveryTarget
}

impl HarvestJob
{
    pub fn new(source_id: ObjectId<Source>) -> HarvestJob {
        HarvestJob {
            harvest_target: source_id,
            delivery_target: DeliveryTarget::None
        }
    }

    pub fn select_delivery_target(&mut self, creep: &Creep, resource_type: ResourceType) -> Option<Structure> {
        let room = creep.room().unwrap();

        for spawn in room.find(find::MY_SPAWNS) {
            if spawn.store_free_capacity(Some(resource_type)) > 0 {
                return Some(spawn.as_structure());
            }
        }
        
        if let Some(controller) = room.controller() {
            return Some(controller.as_structure());
        }

        return None;
    }
}

impl Job for HarvestJob
{
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Harvest Job - {}", creep.name());

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        //
        // Compute delivery target
        //

        if used_capacity > 0 {
            let repick_delivery = match self.delivery_target {
                DeliveryTarget::Spawn(target) => {
                    if let Some(spawn) = target.resolve() {
                        spawn.store_free_capacity(Some(resource)) == 0
                    } else {
                        true
                    }
                },
                DeliveryTarget::Controller(target) => {
                    if let Some(_controller) = target.resolve() {
                        false
                    } else {
                        true
                    }
                },
                DeliveryTarget::Container(target) => {
                    if let Some(container) = target.resolve() {
                        container.store_free_capacity(Some(resource)) == 0
                    } else {
                        true
                    }
                },
                DeliveryTarget::None => {
                    capacity > 0 && available_capacity == 0
                }
            };

            //
            // Pick delivery target
            //

            if repick_delivery {
                self.delivery_target = match self.select_delivery_target(&creep, resource) {
                    Some(Structure::Spawn(spawn)) => {
                        DeliveryTarget::Spawn(spawn.id())
                    }
                    Some(Structure::Container(container)) => {
                        DeliveryTarget::Container(container.id())
                    },
                    Some(Structure::Controller(controller)) => {
                        DeliveryTarget::Controller(controller.id())
                    },
                    None => {
                        error!("Unable to find appropriate delivery target.");

                        DeliveryTarget::None
                    }
                    _ => {
                        error!("Selected incompatible structure type for delivery.");

                        DeliveryTarget::None
                    }
                };
            }
        } else {
            self.delivery_target = DeliveryTarget::None;
        }

        //
        // Move to and use energy
        //
            
        match self.delivery_target {
            DeliveryTarget::Spawn(target) => {
                if let Some(spawn) = target.resolve() {
                    if creep.pos().is_near_to(&spawn.pos()) {
                        creep.transfer_all(&spawn, resource);
                    } else {
                        creep.move_to(&spawn);
                    }

                    return;
                }
            },            
            DeliveryTarget::Controller(target) => {
                if let Some(controller) = target.resolve() {
                    if creep.pos().is_near_to(&controller.pos()) {
                        creep.upgrade_controller(&controller);
                    } else {
                        creep.move_to(&controller);
                    }

                    return;
                }
            },
            DeliveryTarget::Container(target) => {
                if let Some(container) = target.resolve() {
                    if creep.pos().is_near_to(&container.pos()) {
                        creep.transfer_all(&container, resource);
                    } else {
                        creep.move_to(&container);
                    }

                    return;
                }
            },
            DeliveryTarget::None => {
            }
        }

        //
        // Pickup
        //

        if available_capacity > 0 {
            if let Some(source) = self.harvest_target.resolve() {
                if creep.pos().is_near_to(&source.pos()) {
                    creep.harvest(&source);
                } else {
                    creep.move_to(&source);
                }
            } else {
                error!("Harvester has no assigned harvesting source! Name: {}", creep.name());
            }           
        } else {
            error!("Harvester with no available capacity but would like to harvest engery! Name: {}", creep.name());
        }
    }
}
