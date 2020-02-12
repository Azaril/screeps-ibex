use serde::*;
use screeps::*;

use super::jobsystem::*;
use super::utility::resource::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HaulJob {
    pub primary_container: ObjectId<StructureContainer>,
    #[serde(default)]
    pub delivery_target: Option<StructureIdentifier>,
}

impl HaulJob
{
    pub fn new(container_id: ObjectId<StructureContainer>) -> HaulJob {
        HaulJob {
            primary_container: container_id,
            delivery_target: None
        }
    }
}

impl Job for HaulJob
{
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

        //
        // If an existing delivery target exists but does not have room for delivery, choose a new target.
        // If full of energy but no delivery target selected, choose one.
        //

        let repick_delivery = if let Some(delivery_structure) = self.delivery_target {
            if let Some(delivery_structure) = delivery_structure.as_structure() {
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
            self.delivery_target = ResourceUtility::select_resource_delivery(creep, &room, resource).map(|v| StructureIdentifier::new(&v));
        }

        //
        // Transfer energy to structure if possible.
        //

        if let Some(delivery_target_structure) = self.delivery_target.and_then(|v| v.as_structure()) {
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
        // Pickup energy from container.
        //

        //TODO: Validate container still exists? Recyle or reuse hauler if it doesn't?

        if available_capacity > 0 {
            if let Some(container) = self.primary_container.resolve() {
                if creep.pos().is_near_to(&container) {
                    creep.withdraw_all(&container, resource);
                } else {
                    creep.move_to(&container);
                }

                return;
            } else {
                error!("Hauler has no assigned pickup container! Name: {}", creep.name());
            }
        }
    }
}
