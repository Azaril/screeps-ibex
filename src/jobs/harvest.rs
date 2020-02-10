use serde::*;
use screeps::*;
use itertools::*;

use super::jobsystem::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: ObjectId<Source>,
    pub delivery_target: Option<StructureIdentifier>
}

impl HarvestJob
{
    pub fn new(source_id: ObjectId<Source>) -> HarvestJob {
        HarvestJob {
            harvest_target: source_id,
            delivery_target: None
        }
    }

    pub fn select_delivery_target(&mut self, creep: &Creep, resource_type: ResourceType) -> Option<Structure> {
        let room = creep.room().unwrap();

        #[derive(PartialEq, Eq, Hash)]
        enum DeliveryPriority {
            High,
            Medium,
            Low
        }

        let targets = room.find(find::MY_STRUCTURES).iter()
            .map(|owned_structure| owned_structure.clone().as_structure())
            .filter_map(|structure| {
                if let Some(storeable) = structure.as_has_store() {
                    if storeable.store_free_capacity(Some(resource_type)) > 0 {
                        return Some(structure);
                    }
                }
                return None;
            })
            .map(|structure| {
                let priority = match structure {
                    Structure::Spawn(_) => DeliveryPriority::High,
                    Structure::Extension(_) => DeliveryPriority::High,
                    Structure::Container(_) => DeliveryPriority::Medium,
                    _ => DeliveryPriority::Low
                };

                (priority, structure)
            })
            .into_group_map();        

        //
        // Find the delivery target with the highest priority and the shortest path.
        // 
        
        for priority in [DeliveryPriority::High, DeliveryPriority::Medium, DeliveryPriority::Low].iter() {
            if let Some(structures) = targets.get(priority) {
                let nearest = structures.iter()
                    .filter_map(|structure| {
                        let find_options = FindOptions::new()
                            .max_rooms(1)
                            .ignore_creeps(true);

                        if let Path::Vectorized(path) = creep.pos().find_path_to(&structure.pos(), find_options) {
                            Some((path.len(), structure))
                        } else {
                            None
                        }
                    }).min_by_key(|(length, _structure)| {
                        *length
                    }).map(|(_, structure)| {
                        structure
                    });

                if let Some(structure) = nearest {
                    return Some(structure.clone());
                }
            }
        }

        //
        // If there are no delivery targets, use the controller as a fallback.
        //

        if let Some(controller) = room.controller() {
            return Some(controller.as_structure());
        }

        return None;
    }
}

impl Job for HarvestJob
{
    fn run_job(&mut self, data: &JobRuntimeData) {
        scope_timing!("Harvest Job - {}", creep.name());
        
        let creep = data.owner;

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        //
        // Compute delivery target
        //

        if used_capacity > 0 {
            //
            // If an existing delivery target exists but does not have room for delivery, choose a new target.
            //

            let repick_delivery = if let Some(delivery_structure) = self.delivery_target.and_then(|v| v.as_structure()) {
                if let Some(storeable) = delivery_structure.as_has_store() {
                    storeable.store_free_capacity(Some(resource)) == 0
                } else {
                    false
                }
            } else {
                capacity > 0 && available_capacity == 0
            };

            //
            // Pick delivery target
            //

            if repick_delivery {
                let delivery_target = self.select_delivery_target(&creep, resource);

                self.delivery_target = delivery_target.map(|v| StructureIdentifier::new(&v));
            }
        } else if self.delivery_target.is_some() {
            //
            // Clear the delivery target if not carrying any resources.
            //
            self.delivery_target = None;
        }

        //
        // Move to and transfer energy.
        //
            
        if let Some(delivery_target_structure) = self.delivery_target.and_then(|v| v.as_structure()) {
            if creep.pos().is_near_to(&delivery_target_structure.pos()) {
                if let Some(transferable) = delivery_target_structure.as_transferable() {
                    creep.transfer_all(transferable, resource);
                    //TODO: Log error if transfer fails?
                } else if let Structure::Controller(ref controller) = delivery_target_structure {
                    creep.upgrade_controller(controller);
                } else {
                    error!("Unsupported delivery target selected by harvester. Name: {}", creep.name());

                    self.delivery_target = None;
                }            
            } else {
                creep.move_to(&delivery_target_structure);
            }

            return;
        }

        //
        // Harvest energy from source.
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
