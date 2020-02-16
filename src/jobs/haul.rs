use screeps::*;
use serde::*;

use super::jobsystem::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use crate::remoteobjectid::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HaulJob {
    pub primary_container: RemoteObjectId<StructureContainer>,
    #[serde(default)]
    pub delivery_target: Option<StructureIdentifier>,
    #[serde(default)]
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl HaulJob {
    pub fn new(container_id: RemoteObjectId<StructureContainer>) -> HaulJob {
        HaulJob {
            primary_container: container_id,
            delivery_target: None,
            pickup_target: None,
        }
    }
}

impl Job for HaulJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Haul Job - {}", creep.name());

        let room = creep.room().unwrap();

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        if used_capacity == 0 {
            self.delivery_target = None;
        }

        if available_capacity == 0 {
            self.pickup_target = None;
        }

        //
        // If an existing delivery target exists but does not have room for delivery, choose a new target.
        // If full of energy but no delivery target selected, choose one.
        //

        let repick_delivery = if let Some(delivery_structure) = self.delivery_target {
            if let Some(delivery_structure) = delivery_structure.resolve() {
                if let Some(storeable) = delivery_structure.as_has_store() {
                    storeable.store_free_capacity(Some(resource)) == 0
                } else {
                    true
                }
            } else {
                true
            }
        } else {
            capacity > 0 && available_capacity == 0
        };

        //
        // Pick delivery target
        //

        if repick_delivery {
            self.delivery_target =
                ResourceUtility::select_resource_delivery(creep, &room, resource)
                    .map(|v| StructureIdentifier::new(&v));
        }

        //
        // Transfer energy to structure if possible.
        //

        if let Some(delivery_target_structure) = self.delivery_target.and_then(|id| id.resolve())
        {
            if let Some(transferable) = delivery_target_structure.as_transferable() {
                if creep.pos().is_near_to(&delivery_target_structure) {
                    creep.transfer_all(transferable, resource);
                } else {
                    creep.move_to(&delivery_target_structure);
                }

                return;
            }
        }

        //
        // Compute pickup target
        //

        let repick_pickup = self
            .pickup_target
            .map(|target| target.is_valid_pickup_target())
            .unwrap_or_else(|| capacity > 0 && available_capacity > 0);

        if repick_pickup {
            scope_timing!("repick_pickup");

            self.pickup_target = if let Some(container) = self.primary_container.resolve() {
                if container.store_used_capacity(Some(resource)) > 0 {
                    Some(EnergyPickupTarget::Structure(
                        RemoteStructureIdentifier::new(&container.as_structure()),
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            let hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();

            if self.pickup_target.is_none() {
                let pickup_settings = ResourcePickupSettings {
                    allow_dropped_resource: !hostile_creeps,
                    allow_tombstone: !hostile_creeps,
                    allow_structure: true,
                    allow_harvest: false,
                };

                self.pickup_target =
                    ResourceUtility::select_energy_pickup(&creep, &room, &pickup_settings);
            }
        }

        //
        // Move to and get energy.
        //

        if let Some(pickup_target) = self.pickup_target {
            ResourceBehaviorUtility::get_energy(creep, &pickup_target);

            return;
        }
    }
}
