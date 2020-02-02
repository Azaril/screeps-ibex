use serde::*;
use screeps::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: ObjectId<Source>,
    pub controller_target: Option<ObjectId<StructureController>>
}

impl HarvestJob
{
    pub fn new(source: &Source) -> HarvestJob {
        HarvestJob {
            harvest_target: source.id(),
            controller_target: None
        }
    }

    pub fn run_creep(&mut self, creep: &Creep) {
        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        //TODO: Clear deliery target if full and re-pick.

        //
        // Pick initial delivery target
        //

        if capacity > 0 && available_capacity == 0 && self.controller_target.is_none() {
            if let Some(controller) = creep.room().controller() {
                self.controller_target = Some(controller.id());
            }
        }

        //
        // Move to and use energy
        //
            
        if used_capacity > 0 {
            if let Some(target) = self.controller_target {
                if let Some(controller) = target.resolve() {
                    if creep.pos().is_near_to(&controller.pos()) {
                        creep.upgrade_controller(&controller);
                    } else {
                        creep.move_to(&controller);
                    }

                    return;
                }
            }
        }
        
        //
        // Clear target if delivery is complete.
        //
        
        if self.controller_target.is_some() {
            self.controller_target = None
        }

        //
        // Pickup
        //

        if available_capacity > 0 {
            if let Some(source) = self.harvest_target.resolve() {
                if creep.pos().is_near_to(&source.pos()) {
                    creep.harvest(&source);
                } else {
                    creep.move_to(&source);
                }
            } else {
                error!("Harvester has no assigned harvesting source! Name: {}", creep.name());
            }           
        } else {
            error!("Harvester with no available capacity but would like to harvest engery! Name: {}", creep.name());
        }
    }
}
