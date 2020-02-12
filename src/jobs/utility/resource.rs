use serde::*;
use screeps::*;
use itertools::*;

use crate::structureidentifier::*;
use crate::findnearest::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum EnergyPickupTarget {
    Structure(StructureIdentifier),
    Source(ObjectId<Source>)
}

pub struct ResourceUtility;

impl ResourceUtility {
    pub fn select_energy_resource_pickup_or_harvest(creep: &Creep, room: &Room) -> Option<EnergyPickupTarget> {
        if let Some(identifier) = Self::select_resource_pickup(creep, room, ResourceType::Energy) {
            return Some(EnergyPickupTarget::Structure(identifier));
        }

        let nearest_source = room.find(find::SOURCES_ACTIVE).iter()
            .cloned()
            .find_nearest(&creep.pos(), PathFinderHelpers::same_room_ignore_creeps);

        if let Some(source) = nearest_source {
            return Some(EnergyPickupTarget::Source(source.id()));
        }

        return None;
    }    

    pub fn select_resource_pickup(creep: &Creep, room: &Room, resource_type: ResourceType) -> Option<StructureIdentifier> {
        #[derive(PartialEq, Eq, Hash, Debug)]
        enum PickupPriority {
            High,
            Medium,
            Low
        }

        let mut targets = room.find(find::STRUCTURES)
            .into_iter()
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
            .filter_map(|structure| {
                let priority = match structure {
                    Structure::Container(_) => Some(PickupPriority::High),
                    Structure::Storage(_) => Some(PickupPriority::High),
                    _ => None
                };

                priority.map(|p| (p, structure))
            })
            .into_group_map();        

        //
        // Find the pickup target with the highest priority and the shortest path.
        //
        
        for priority in [PickupPriority::High, PickupPriority::Medium, PickupPriority::Low].iter() {
            if let Some(structures) = targets.remove(priority) {
                if let Some(structure) = structures.into_iter().find_nearest(&creep.pos(), PathFinderHelpers::same_room_ignore_creeps) {
                    return Some(StructureIdentifier::new(&structure));
                }
            }
        }

        return None;
    }   

    pub fn select_resource_delivery(creep: &Creep, room: &Room, resource_type: ResourceType) -> Option<Structure> {
        #[derive(PartialEq, Eq, Hash, Debug)]
        enum DeliveryPriority {
            Critical,
            High,
            Medium,
            Low
        }

        let mut targets = room.find(find::STRUCTURES)
            .into_iter()
            .filter(|structure| {
                if let Some(owned_structure) = structure.as_owned() {
                    owned_structure.my()
                } else {
                    true
                }})
            .filter_map(|structure| {
                if let Some(storeable) = structure.as_has_store() {
                    if storeable.store_free_capacity(Some(resource_type)) > 0 {
                        return Some(structure);
                    }
                }
                return None;
            })
            .filter_map(|structure| {
                let priority = match structure {
                    Structure::Spawn(_) => Some(DeliveryPriority::Critical),
                    Structure::Tower(_) => Some(DeliveryPriority::Critical),
                    Structure::Extension(_) => Some(DeliveryPriority::High),
                    Structure::Storage(_) => Some(DeliveryPriority::Medium), 
                    Structure::Container(_) => None,
                    _ => Some(DeliveryPriority::Low)
                };

                priority.map(|p| (p, structure))
            })
            .into_group_map();        

        //
        // Find the delivery target with the highest priority and the shortest path.
        // 

        for priority in [DeliveryPriority::Critical, DeliveryPriority::High, DeliveryPriority::Medium, DeliveryPriority::Low].iter() {
            if let Some(structures) = targets.remove(priority) {
                if let Some(structure) = structures.into_iter().find_nearest(&creep.pos(), PathFinderHelpers::same_room_ignore_creeps) {
                    return Some(structure.clone());
                }
            }
        }

        return None;
    }
}

