use serde::*;
use screeps::*;

use super::jobsystem::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use super::utility::build::*;
use super::utility::buildbehavior::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: ObjectId<Source>,
    #[serde(default)]
    pub delivery_target: Option<StructureIdentifier>,
    #[serde(default)]
    pub build_target: Option<ObjectId<ConstructionSite>>,
    #[serde(default)]
    pub pickup_target: Option<ObjectId<Resource>>
}

impl HarvestJob
{
    pub fn new(source_id: ObjectId<Source>) -> HarvestJob {
        HarvestJob {
            harvest_target: source_id,
            delivery_target: None,
            build_target: None,
            pickup_target: None
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
        let creep = data.owner;

        scope_timing!("Harvest Job - {}", creep.name());
        
        let room = creep.room().unwrap();

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        if used_capacity == 0 {
            self.delivery_target = None;
            self.build_target = None;
        }

        //
        // Compute delivery target
        //

        let repick_delivery = if let Some(delivery_structure) = self.delivery_target {
            if let Some(delivery_structure) = delivery_structure.as_structure() {
                if let Some(storeable) = delivery_structure.as_has_store() {
                    storeable.store_free_capacity(Some(resource)) == 0
                } else if let Structure::Controller(_) = delivery_structure {
                    false
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
            self.delivery_target = self.select_delivery_target(&creep, &room, resource).map(|v| StructureIdentifier::new(&v));
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
        // Compute build target
        //        

        let repick_build_target = match self.build_target {
            Some(target_id) => target_id.resolve().is_none(),
            None => capacity > 0 && available_capacity == 0
        };

        if repick_build_target {
            self.build_target = BuildUtility::select_construction_site(&creep, &room).map(|site| site.id());
        }

        //
        // Build construction site.
        //

        if let Some(construction_site) = self.build_target.and_then(|id| id.resolve()) {
            BuildBehaviorUtility::build_construction_site(creep, &construction_site);

            return;
        }

        //
        // Upgrade controller.
        //

        if let Some(delivery_target_structure) = self.delivery_target.and_then(|v| v.as_structure()) {
            if let Structure::Controller(controller) = delivery_target_structure {
                if creep.pos().is_near_to(&controller) {
                    creep.upgrade_controller(&controller);
                } else {
                    creep.move_to(&controller);
                }

                return;
            }
        }

        //
        // Compute pickup target
        //

        //TODO: Factor this in to common code.
        let repick_pickup = match self.pickup_target {
            Some(resource_id) => {
                if let Some(_) = resource_id.resolve() {
                    false
                } else {
                    true
                }
            },
            None => capacity > 0 && used_capacity == 0
        };

        if repick_pickup {
            scope_timing!("repick_pickup");

            self.pickup_target = ResourceUtility::select_dropped_resource(creep, &room, resource).map(|resource| resource.id());
        }

        //
        // Move to and get energy.
        //

        if let Some(resource_id) = self.pickup_target {
            if let Some(resource) = resource_id.resolve() {
                ResourceBehaviorUtility::get_energy_from_dropped_resource(creep, &resource);

                return;
            }
        }
    }
}
