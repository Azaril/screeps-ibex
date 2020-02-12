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
            .find_nearest(&creep.pos(), PathFinderHelpers::same_room);

        if let Some(source) = nearest_source {
            return Some(EnergyPickupTarget::Source(source.id()));
        }

        return None;
    }    

    pub fn select_resource_pickup(creep: &Creep, room: &Room, resource_type: ResourceType) -> Option<StructureIdentifier> {
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
            if let Some(structures) = targets.get(priority) {
                if let Some(structure) = structures.iter().cloned().find_nearest(&creep.pos(), PathFinderHelpers::same_room) {
                    return Some(StructureIdentifier::new(&structure));
                }
            }
        }

        return None;
    }   

    pub fn select_resource_delivery(creep: &Creep, room: &Room, resource_type: ResourceType) -> Option<Structure> {
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
                if let Some(structure) = structures.iter().cloned().find_nearest(&creep.pos(), PathFinderHelpers::same_room) {
                    return Some(structure.clone());
                }
            }
        }

        return None;
    }
}

