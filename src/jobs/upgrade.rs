use screeps::*;
use serde::*;

use super::jobsystem::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UpgradeJob {
    pub upgrade_target: ObjectId<StructureController>,
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl UpgradeJob {
    pub fn new(upgrade_target: &ObjectId<StructureController>) -> UpgradeJob {
        UpgradeJob {
            upgrade_target: *upgrade_target,
            pickup_target: None,
        }
    }
}

impl Job for UpgradeJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Upgrade Job - {}", creep.name());

        let room = creep.room().unwrap();

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
                }
                Some(EnergyPickupTarget::Source(ref source_id)) => {
                    if let Some(source) = source_id.resolve() {
                        source.energy() == 0
                    } else {
                        true
                    }
                }
                Some(EnergyPickupTarget::DroppedResource(ref resource_id)) => {
                    resource_id.resolve().is_none()
                },
                Some(EnergyPickupTarget::Tombstone(ref tombstone_id)) => {
                    tombstone_id.resolve().is_none()
                }
                None => capacity > 0 && used_capacity == 0,
            };

            if repick_pickup {
                let hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();

                let settings = ResourcePickupSettings{
                    allow_dropped_resource: !hostile_creeps,
                    allow_tombstone: !hostile_creeps,
                    allow_structure: true,
                    allow_harvest: true
                };

                self.pickup_target = ResourceUtility::select_energy_pickup(&creep, &room, &settings);
            }
        }

        //
        // Move to and transfer energy.
        //

        if let Some(pickup_target) = self.pickup_target {
            ResourceBehaviorUtility::get_energy(creep, &pickup_target);

            return;
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
                error!(
                    "Upgrader has no assigned upgrade target! Name: {}",
                    creep.name()
                );
            }
        } else {
            error!(
                "Upgrader with no energy would like to upgrade target! Name: {}",
                creep.name()
            );
        }
    }
}
