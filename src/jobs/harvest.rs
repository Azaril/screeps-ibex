use serde::*;
use screeps::*;

use super::jobsystem::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: ObjectId<Source>,
    pub delivery_target: Option<StructureIdentifier>
}

impl HarvestJob
{
    pub fn new(source_id: ObjectId<Source>) -> HarvestJob {
        HarvestJob {
            harvest_target: source_id,
            delivery_target: None
        }
    }

    pub fn select_delivery_target(&mut self, creep: &Creep, room: &Room, resource_type: ResourceType) -> Option<Structure> {
        if let Some(delivery_target) = ResourceUtility::select_resource_delivery(creep, room, resource_type) {
            return Some(delivery_target);
        }

        // If there are no delivery targets, use the controller as a fallback.
        //

        if let Some(controller) = room.controller() {
            return Some(controller.as_structure());
        }

        return None;
    }
}

impl Job for HarvestJob
{
    fn run_job(&mut self, data: &JobRuntimeData) {
        scope_timing!("Harvest Job - {}", creep.name());
        
        let creep = data.owner;
        let room = creep.room().unwrap();

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        //
        // Compute delivery target
        //

        if used_capacity == 0 {
            self.delivery_target = None;
        } else {
            //
            // If an existing delivery target exists but does not have room for delivery, choose a new target.
            // If full of energy but no delivery target selected, choose one.
            //

            let repick_delivery = if let Some(delivery_structure) = self.delivery_target.and_then(|v| v.as_structure()) {
                if let Some(storeable) = delivery_structure.as_has_store() {
                    storeable.store_free_capacity(Some(resource)) == 0
                } else {
                    false
                }
            } else {
                capacity > 0 && available_capacity == 0
            };

            //
            // Pick delivery target
            //

            if repick_delivery {
                self.delivery_target = self.select_delivery_target(&creep, &room, resource).map(|v| StructureIdentifier::new(&v));
            }
        }

        //
        // Move to and transfer energy.
        //
            
        if let Some(delivery_target_structure) = self.delivery_target.and_then(|v| v.as_structure()) {
            if creep.pos().is_near_to(&delivery_target_structure) {
                if let Some(transferable) = delivery_target_structure.as_transferable() {
                    creep.transfer_all(transferable, resource);
                    //TODO: Log error if transfer fails?
                } else if let Structure::Controller(ref controller) = delivery_target_structure {
                    creep.upgrade_controller(controller);
                } else {
                    error!("Unsupported delivery target selected by harvester. Name: {}", creep.name());

                    self.delivery_target = None;
                }            
            } else {
                creep.move_to(&delivery_target_structure);
            }

            return;
        }

        //
        // Harvest energy from source.
        //

        if available_capacity > 0 {
            if let Some(source) = self.harvest_target.resolve() {
                ResourceBehaviorUtility::get_energy_from_source(creep, &source);

                return;
            } else {
                error!("Harvester has no assigned harvesting source! Name: {}", creep.name());
            }           
        } else {
            error!("Harvester with no available capacity but would like to harvest engery! Name: {}", creep.name());
        }
    }
}
