use screeps::*;

use super::resource::*;
use crate::remoteobjectid::*;
use crate::structureidentifier::*;

pub struct ResourceBehaviorUtility;

impl ResourceBehaviorUtility {
    pub fn get_resource_from_structure(
        creep: &Creep,
        structure: &Structure,
        resource: ResourceType,
    ) {
        scope_timing!("get_resource_from_structure");

        if let Some(withdrawable) = structure.as_withdrawable() {
            if creep.pos().is_near_to(structure) {
                creep.withdraw_all(withdrawable, resource);
            } else {
                creep.move_to(structure);
            }
        } else {
            //TODO: Return error result.
            error!("Expected to be able to withdraw from structure but it was the wrong type. Name: {}", creep.name());
        }
    }

    pub fn get_resource_from_structure_id(creep: &Creep, structure_id: &RemoteStructureIdentifier, resource: ResourceType) {
        let target_position = structure_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        } else if let Some(structure) = structure_id.resolve() {
                //TODO: Handle error result.
                Self::get_resource_from_structure(creep, &structure, resource);
        } else {
            //TODO: Return error result.
            error!(
                "Failed to resolve pickup structure for getting resource. Name: {}",
                creep.name()
            );
        }
    }

    pub fn get_energy_from_source(creep: &Creep, source: &Source) {
        scope_timing!("get_energy_from_source");

        if creep.pos().is_near_to(source) {
            creep.harvest(source);
        } else {
            creep.move_to(source);
        }
    }

    pub fn get_energy_from_source_id(creep: &Creep, source_id: &RemoteObjectId<Source>) {
        let target_position = source_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        }  else if let Some(source) = source_id.resolve() {
            //TODO: Handle error result.
            Self::get_energy_from_source(creep, &source);
        } else {
            //TODO: Return error result.
            error!(
                "Failed to resolve source for getting energy. Name: {}",
                creep.name()
            );
        }
    }

    pub fn get_resource_from_dropped_resource(creep: &Creep, resource: &Resource) {
        scope_timing!("get_energy_from_dropped_resource");

        if creep.pos().is_near_to(resource) {
            creep.pickup(resource);
        } else {
            creep.move_to(resource);
        }
    }

    pub fn get_resource_from_dropped_resource_id(creep: &Creep, dropped_resource_id: &RemoteObjectId<Resource>) {
        let target_position = dropped_resource_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        } else if let Some(dropped_resource) = dropped_resource_id.resolve() {
                //TODO: Handle error result.
                Self::get_resource_from_dropped_resource(creep, &dropped_resource);
        } else {
            //TODO: Return error result.
            error!(
                "Failed to resolve dropped resource for getting resource. Name: {}",
                creep.name()
            );
        }
    }

    pub fn get_resource_from_tombstone(creep: &Creep, tombstone: &Tombstone, resource: ResourceType) {
        scope_timing!("get_energy_from_tombstone");

        if creep.pos().is_near_to(tombstone) {
            creep.withdraw_all(tombstone, resource);
        } else {
            creep.move_to(tombstone);
        }
    }

    pub fn get_resource_from_tombstone_id(creep: &Creep, tombstone_id: &RemoteObjectId<Tombstone>, resource: ResourceType) {
        let target_position = tombstone_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        } else if let Some(tombstone) = tombstone_id.resolve() {
                //TODO: Handle error result.
                Self::get_resource_from_tombstone(creep, &tombstone, resource);
        } else {
            //TODO: Return error result.
            error!(
                "Failed to resolve tombstone for getting resource. Name: {}",
                creep.name()
            );
        }
    }

    pub fn get_energy(creep: &Creep, target: &EnergyPickupTarget) {
        scope_timing!("get_energy");

        match target {
            EnergyPickupTarget::Structure(id) => Self::get_resource_from_structure_id(creep, id, ResourceType::Energy),
            EnergyPickupTarget::Source(id) => Self::get_energy_from_source_id(creep, id),
            EnergyPickupTarget::DroppedResource(id) => Self::get_resource_from_dropped_resource_id(creep, id),
            EnergyPickupTarget::Tombstone(id) => Self::get_resource_from_tombstone_id(creep, id, ResourceType::Energy),
        }
    }

    pub fn transfer_resource_to_structure(creep: &Creep, structure: &Structure, resource: ResourceType) {
        scope_timing!("transfer_resource_to_structure");

        if let Some(transferable) = structure.as_transferable() {
            if creep.pos().is_near_to(structure) {
                creep.transfer_all(transferable, resource);
            } else {
                creep.move_to(structure);
            }
        } else {
            //TODO: Return error result.
            error!("Failed to convert structure to transferable - wrong structure type. Name: {}", creep.name());
        }
    }

    pub fn transfer_resource_to_structure_id(creep: &Creep, structure_id: &RemoteStructureIdentifier, resource: ResourceType) {
        let target_position = structure_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        } else if let Some(structure) = structure_id.resolve() {
                //TODO: Handle error result.
                Self::transfer_resource_to_structure(creep, &structure, resource);
        } else {
            //TODO: Return error result.
            error!("Failed to resolve structure to transfer resource to. Name: {}", creep.name());
        }
    }
}
