use itertools::*;
use screeps::*;
use serde::*;

use crate::findnearest::*;
use crate::remoteobjectid::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum EnergyPickupTarget {
    Structure(RemoteStructureIdentifier),
    Source(RemoteObjectId<Source>),
    DroppedResource(RemoteObjectId<Resource>),
    Tombstone(RemoteObjectId<Tombstone>),
}

impl EnergyPickupTarget {
    //TODO: Make this a trait.
    pub fn is_valid_pickup_target(&self) -> bool {
        //
        // If room cannot be resolve, the room is not currently visible.
        //

        //TODO: Use room visibility cache.
        if game::rooms::get(self.get_position().room_name()).is_none() {
            return true;
        }

        //
        // If the room is visible, validate the constraints on the pickup target.
        //

        match self {
            EnergyPickupTarget::Structure(structure_id) => {
                if let Some(structure) = structure_id.resolve() {
                    if let Some(storeable) = structure.as_has_store() {
                        storeable.store_used_capacity(Some(ResourceType::Energy)) > 0
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            EnergyPickupTarget::Source(source_id) => {
                if let Some(source) = source_id.resolve() {
                    source.energy() > 0
                } else {
                    false
                }
            }
            EnergyPickupTarget::DroppedResource(resource_id) => resource_id.resolve().is_some(),
            EnergyPickupTarget::Tombstone(tombstone_id) => {
                if let Some(tombstone) = tombstone_id.resolve() {
                    tombstone.store_used_capacity(Some(ResourceType::Energy)) > 0
                } else {
                    false
                }
            }
        }
    }

    pub fn get_position(&self) -> screeps::Position {
        match self {
            EnergyPickupTarget::Structure(structure_id) => structure_id.pos(),
            EnergyPickupTarget::Source(source_id) => source_id.pos(),
            EnergyPickupTarget::DroppedResource(resource_id) => resource_id.pos(),
            EnergyPickupTarget::Tombstone(tombstone_id) => tombstone_id.pos(),
        }
    }
}

pub struct ResourcePickupSettings {
    pub allow_dropped_resource: bool,
    pub allow_tombstone: bool,
    pub allow_structure: bool,
    pub allow_harvest: bool,
}

pub struct ResourceUtility;

impl ResourceUtility {
    pub fn select_energy_pickup(
        creep: &Creep,
        room: &Room,
        settings: &ResourcePickupSettings,
    ) -> Option<EnergyPickupTarget> {
        if settings.allow_dropped_resource {
            if let Some(dropped_resource) =
                Self::select_dropped_resource(creep, room, ResourceType::Energy)
            {
                return Some(EnergyPickupTarget::DroppedResource(
                    dropped_resource.remote_id(),
                ));
            }
        }

        if settings.allow_tombstone {
            if let Some(tombstone) = Self::select_tombstone(creep, room, ResourceType::Energy) {
                return Some(EnergyPickupTarget::Tombstone(tombstone.remote_id()));
            }
        }

        if settings.allow_structure {
            if let Some(structure) =
                Self::select_structure_resource(creep, room, ResourceType::Energy)
            {
                return Some(EnergyPickupTarget::Structure(
                    RemoteStructureIdentifier::new(&structure),
                ));
            }
        }

        if settings.allow_harvest {
            if let Some(source) = Self::select_active_sources(creep, room) {
                return Some(EnergyPickupTarget::Source(source.remote_id()));
            }
        }

        None
    }

    pub fn select_active_sources(creep: &Creep, room: &Room) -> Option<Source> {
        room.find(find::SOURCES_ACTIVE)
            .into_iter()
            .find_nearest_from(
                creep.pos(),
                PathFinderHelpers::same_room_ignore_creeps_range_1,
            )
    }

    pub fn select_dropped_resource(
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Resource> {
        room.find(find::DROPPED_RESOURCES)
            .into_iter()
            .filter(|resource| resource.resource_type() == resource_type)
            .find_nearest_from(
                creep.pos(),
                PathFinderHelpers::same_room_ignore_creeps_range_1,
            )
    }

    pub fn select_tombstone(
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Tombstone> {
        room.find(find::TOMBSTONES)
            .into_iter()
            .filter(|tombstone| tombstone.store_used_capacity(Some(resource_type)) > 0)
            .find_nearest_from(
                creep.pos(),
                PathFinderHelpers::same_room_ignore_creeps_range_1,
            )
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
                if let Some(structure) = structures.into_iter().find_nearest_from(
                    creep.pos(),
                    PathFinderHelpers::same_room_ignore_creeps_range_1,
                ) {
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
                    Structure::Extension(_) => Some(DeliveryPriority::Critical),
                    Structure::Tower(_) => Some(DeliveryPriority::High),
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
                if let Some(structure) = structures.into_iter().find_nearest_from(
                    creep.pos(),
                    PathFinderHelpers::same_room_ignore_creeps_range_1,
                ) {
                    return Some(structure);
                }
            }
        }

        None
    }
}

pub trait ValidateDeliveryTarget {
    fn is_valid_delivery_target(&self, resource: ResourceType) -> Option<bool>;
}

impl ValidateDeliveryTarget for Structure {
    fn is_valid_delivery_target(&self, resource: ResourceType) -> Option<bool> {
        if let Some(storeable) = self.as_has_store() {
            Some(storeable.store_free_capacity(Some(resource)) > 0)
        } else {
            Some(false)
        }
    }
}

impl ValidateDeliveryTarget for RemoteStructureIdentifier {
    fn is_valid_delivery_target(&self, resource: ResourceType) -> Option<bool> {
        if game::rooms::get(self.pos().room_name()).is_some() {
            self.resolve()
                .and_then(|s| s.is_valid_delivery_target(resource))
                .or(Some(false))
        } else {
            None
        }
    }
}

pub trait ValidateControllerUpgradeTarget {
    fn is_valid_controller_upgrade_target(&self) -> bool;
}

impl ValidateControllerUpgradeTarget for StructureController {
    fn is_valid_controller_upgrade_target(&self) -> bool {
        self.my()
    }
}

impl ValidateControllerUpgradeTarget for Structure {
    fn is_valid_controller_upgrade_target(&self) -> bool {
        if let Structure::Controller(controller) = self {
            controller.is_valid_controller_upgrade_target()
        } else {
            false
        }
    }
}

impl ValidateControllerUpgradeTarget for RemoteStructureIdentifier {
    fn is_valid_controller_upgrade_target(&self) -> bool {
        if let Some(structure) = self.resolve() {
            //
            // NOTE: A controller is an owned structure and provides visibility. It must
            //       resolve for it to be valid as it cannot be in a hidden room.
            //
            structure.is_valid_controller_upgrade_target()
        } else {
            false
        }
    }
}
