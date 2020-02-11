use serde::*;
use screeps::*;
use itertools::*;

use super::jobsystem::*;
use crate::structureidentifier::*;
use crate::findnearest::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum PickupTarget {
    Structure(StructureIdentifier),
    Source(ObjectId<Source>)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UpgradeJob {
    pub upgrade_target: ObjectId<StructureController>,
    pub pickup_target: Option<PickupTarget>
}

impl UpgradeJob
{
    pub fn new(upgrade_target: &ObjectId<StructureController>) -> UpgradeJob {
        UpgradeJob {
            upgrade_target: upgrade_target.clone(),
            pickup_target: None
        }
    }

    pub fn select_pickup_target(&mut self, creep: &Creep, resource_type: ResourceType) -> Option<PickupTarget> {
        let room = creep.room().unwrap();

        #[derive(PartialEq, Eq, Hash)]
        enum PickupPriority {
            High,
            Medium,
            Low
        }

        let targets = room.find(find::MY_STRUCTURES).iter()
            .map(|owned_structure| owned_structure.clone().as_structure())
            .filter_map(|structure| {
                if structure.as_withdrawable().is_some() {
                    if let Some(storeable) = structure.as_has_store() {
                        if storeable.store_used_capacity(Some(resource_type)) > 0 {
                            return Some(structure);
                        }
                    }
                }
                
                return None;
            })
            .map(|structure| {
                let priority = match structure {
                    Structure::Container(_) => PickupPriority::High,
                    Structure::Storage(_) => PickupPriority::High,
                    _ => PickupPriority::Low
                };

                (priority, structure)
            })
            .into_group_map();        

        //
        // Find the pickup target with the highest priority and the shortest path.
        // 
        
        for priority in [PickupPriority::High, PickupPriority::Medium, PickupPriority::Low].iter() {
            if let Some(structures) = targets.get(priority) {
                if let Some(structure) = structures.iter().cloned().find_nearest(&creep.pos(), PathFinderHelpers::same_room) {
                    return Some(PickupTarget::Structure(StructureIdentifier::new(&structure)));
                }
            }
        }

        //
        // If there are no pickup targets, use the nearest source as a fallback.
        //

        let nearest_source = room.find(find::SOURCES_ACTIVE).iter()
            .cloned()
            .find_nearest(&creep.pos(), PathFinderHelpers::same_room);

        if let Some(source) = nearest_source {
            return Some(PickupTarget::Source(source.id()));
        }

        return None;
    }
}

impl Job for UpgradeJob
{
    fn run_job(&mut self, data: &JobRuntimeData) {
        scope_timing!("Upgrade Job - {}", creep.name());
        
        let creep = data.owner;

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        //
        // Compute pickup target
        //

        if available_capacity == 0 {
            self.pickup_target = None;
        } else {
            let repick_pickup = match self.pickup_target {
                Some(PickupTarget::Structure(ref pickup_structure_id)) => {
                    if let Some(pickup_structure) = pickup_structure_id.as_structure() {
                        if let Some(storeable) = pickup_structure.as_has_store() {
                            storeable.store_used_capacity(Some(resource)) == 0
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                },
                Some(PickupTarget::Source(ref source_id)) => {
                    if let Some(source) = source_id.resolve() {
                        source.energy() == 0
                    } else {
                        true
                    }
                },
                None => capacity > 0 && used_capacity == 0
            };

            if repick_pickup {
                self.pickup_target = self.select_pickup_target(&creep, resource);
            }
        }

        //
        // Move to and transfer energy.
        //
        
        match self.pickup_target {
            Some(PickupTarget::Structure(ref pickup_structure_id)) => {
                if let Some(pickup_structure) = pickup_structure_id.as_structure() {
                    if creep.pos().is_near_to(&pickup_structure) {
                        if let Some(withdrawable) = pickup_structure.as_withdrawable() {
                            creep.withdraw_all(withdrawable, resource);
                        } else {
                            error!("Upgrader expected to be able to withdraw from structure but it was the wrong type.");
                        }
                    } else {
                        creep.move_to(&pickup_structure);
                    }

                    return;
                } else {
                    error!("Failed to resolve pickup structure for upgrader.");
                }
            },
            Some(PickupTarget::Source(ref source_id)) => {
                if let Some(source) = source_id.resolve() {
                    if creep.pos().is_near_to(&source) {
                        creep.harvest(&source);
                    } else {
                        creep.move_to(&source);
                    }

                    return;
                } else {
                    error!("Failed to resolve pickup source for upgrader.");
                }
            },
            None => {}
        }

        //
        // Upgrade energy from source.
        //

        if used_capacity > 0 {
            if let Some(controller) = self.upgrade_target.resolve() {
                if creep.pos().is_near_to(&controller) {
                    creep.upgrade_controller(&controller);
                } else {
                    creep.move_to(&controller);
                }
            } else {
                error!("Upgrader has no assigned upgrade target! Name: {}", creep.name());
            }           
        } else {
            error!("Upgrader with no energy would like to upgrade target! Name: {}", creep.name());
        }
    }
}
