use itertools::*;
use screeps::*;
use serde::*;

use crate::findnearest::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum EnergyPickupTarget {
    Structure(StructureIdentifier),
    Source(ObjectId<Source>),
    DroppedResource(ObjectId<Resource>),
}

pub struct ResourceUtility;

impl ResourceUtility {
    pub fn select_energy_resource_pickup_or_harvest(
        creep: &Creep,
        room: &Room,
    ) -> Option<EnergyPickupTarget> {
        if let Some(dropped_resource) =
            Self::select_dropped_resource(creep, room, ResourceType::Energy)
        {
            return Some(EnergyPickupTarget::DroppedResource(dropped_resource.id()));
        }

        if let Some(structure) = Self::select_structure_resource(creep, room, ResourceType::Energy)
        {
            return Some(EnergyPickupTarget::Structure(StructureIdentifier::new(
                &structure,
            )));
        }

        if let Some(source) = Self::select_active_sources(creep, room) {
            return Some(EnergyPickupTarget::Source(source.id()));
        }

        None
    }

    pub fn select_active_sources(creep: &Creep, room: &Room) -> Option<Source> {
        room.find(find::SOURCES_ACTIVE)
            .into_iter()
            .find_nearest(creep.pos(), PathFinderHelpers::same_room_ignore_creeps)
    }

    pub fn select_dropped_resource(
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Resource> {
        room.find(find::DROPPED_RESOURCES)
            .into_iter()
            .filter(|resource| resource.resource_type() == resource_type)
            .find_nearest(creep.pos(), PathFinderHelpers::same_room_ignore_creeps)
    }

    pub fn select_structure_resource(
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Structure> {
        #[derive(PartialEq, Eq, Hash, Debug)]
        enum PickupPriority {
            High,
            Medium,
            Low,
        }

        let mut targets = room
            .find(find::STRUCTURES)
            .into_iter()
            .filter(|structure| {
                if structure.as_withdrawable().is_some() {
                    if let Some(storeable) = structure.as_has_store() {
                        if storeable.store_used_capacity(Some(resource_type)) > 0 {
                            return true;
                        }
                    }
                }

                false
            })
            .filter_map(|structure| {
                let priority = match structure {
                    Structure::Container(_) => Some(PickupPriority::High),
                    Structure::Storage(_) => Some(PickupPriority::High),
                    _ => None,
                };

                priority.map(|p| (p, structure))
            })
            .into_group_map();

        //
        // Find the pickup target with the highest priority and the shortest path.
        //

        for priority in [
            PickupPriority::High,
            PickupPriority::Medium,
            PickupPriority::Low,
        ]
        .iter()
        {
            if let Some(structures) = targets.remove(priority) {
                if let Some(structure) = structures
                    .into_iter()
                    .find_nearest(creep.pos(), PathFinderHelpers::same_room_ignore_creeps)
                {
                    return Some(structure);
                }
            }
        }

        None
    }

    pub fn select_resource_delivery(
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Structure> {
        #[derive(PartialEq, Eq, Hash, Debug)]
        enum DeliveryPriority {
            Critical,
            High,
            Medium,
            Low,
        }

        let mut targets = room
            .find(find::STRUCTURES)
            .into_iter()
            .filter(|structure| {
                if let Some(owned_structure) = structure.as_owned() {
                    owned_structure.my()
                } else {
                    true
                }
            })
            .filter(|structure| {
                if let Some(storeable) = structure.as_has_store() {
                    if storeable.store_free_capacity(Some(resource_type)) > 0 {
                        return true;
                    }
                }
                false
            })
            .filter_map(|structure| {
                let priority = match structure {
                    Structure::Spawn(_) => Some(DeliveryPriority::Critical),
                    Structure::Tower(_) => Some(DeliveryPriority::Critical),
                    Structure::Extension(_) => Some(DeliveryPriority::High),
                    Structure::Storage(_) => Some(DeliveryPriority::Medium),
                    Structure::Container(_) => None,
                    _ => Some(DeliveryPriority::Low),
                };

                priority.map(|p| (p, structure))
            })
            .into_group_map();

        //
        // Find the delivery target with the highest priority and the shortest path.
        //

        for priority in [
            DeliveryPriority::Critical,
            DeliveryPriority::High,
            DeliveryPriority::Medium,
            DeliveryPriority::Low,
        ]
        .iter()
        {
            if let Some(structures) = targets.remove(priority) {
                if let Some(structure) = structures
                    .into_iter()
                    .find_nearest(creep.pos(), PathFinderHelpers::same_room_ignore_creeps)
                {
                    return Some(structure);
                }
            }
        }

        None
    }
}
