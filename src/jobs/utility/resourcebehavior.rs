use screeps::*;

use super::resource::*;

pub struct ResourceBehaviorUtility;

impl ResourceBehaviorUtility
{
    pub fn get_resource_from_structure(creep: &Creep, structure: &Structure, resource: ResourceType) {
        if let Some(withdrawable) = structure.as_withdrawable() {
            if creep.pos().is_near_to(structure) {
                creep.withdraw_all(withdrawable, resource);
            } else {
                creep.move_to(structure);
            }
        } else {
            error!("Expected to be able to withdraw from structure but it was the wrong type. Name: {}", creep.name());
        }
    }

    pub fn get_energy_from_source(creep: &Creep, source: &Source) {
        if creep.pos().is_near_to(source) {
            creep.harvest(source);
        } else {
            creep.move_to(source);
        }
    }

    pub fn get_energy(creep: &Creep, target: &EnergyPickupTarget) {
        match target {
            EnergyPickupTarget::Structure(ref pickup_structure_id) => {
                if let Some(pickup_structure) = pickup_structure_id.as_structure() {
                    Self::get_resource_from_structure(creep, &pickup_structure, ResourceType::Energy);
                } else {
                    error!("Failed to resolve pickup structure for getting enery. Name: {}", creep.name());
                }
            },
            EnergyPickupTarget::Source(ref source_id) => {
                if let Some(source) = source_id.resolve() {
                    Self::get_energy_from_source(creep, &source);
                } else {
                    error!("Failed to resolve pickup source for getting energy. Name: {}", creep.name());
                }
            }
        }
    }
}