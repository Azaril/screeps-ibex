use serde::*;
use screeps::*;

use super::jobsystem::*;
use super::utility::resource::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UpgradeJob {
    pub upgrade_target: ObjectId<StructureController>,
    pub pickup_target: Option<EnergyPickupTarget>
}

impl UpgradeJob
{
    pub fn new(upgrade_target: &ObjectId<StructureController>) -> UpgradeJob {
        UpgradeJob {
            upgrade_target: upgrade_target.clone(),
            pickup_target: None
        }
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
                Some(EnergyPickupTarget::Structure(ref pickup_structure_id)) => {
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
                Some(EnergyPickupTarget::Source(ref source_id)) => {
                    if let Some(source) = source_id.resolve() {
                        source.energy() == 0
                    } else {
                        true
                    }
                },
                None => capacity > 0 && used_capacity == 0
            };

            if repick_pickup {
                self.pickup_target = ResourceUtility::select_energy_resource_pickup_or_harvest(&creep);
            }
        }

        //
        // Move to and transfer energy.
        //
        
        //TODO: Factor this in to common code.
        match self.pickup_target {
            Some(EnergyPickupTarget::Structure(ref pickup_structure_id)) => {
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
            Some(EnergyPickupTarget::Source(ref source_id)) => {
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
