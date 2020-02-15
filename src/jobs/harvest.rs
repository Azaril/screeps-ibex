use screeps::*;
use serde::*;

use super::jobsystem::*;
use super::utility::build::*;
use super::utility::buildbehavior::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: ObjectId<Source>,
    #[serde(default)]
    pub delivery_target: Option<StructureIdentifier>,
    #[serde(default)]
    pub build_target: Option<ObjectId<ConstructionSite>>,
    #[serde(default)]
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl HarvestJob {
    pub fn new(source_id: ObjectId<Source>) -> HarvestJob {
        HarvestJob {
            harvest_target: source_id,
            delivery_target: None,
            build_target: None,
            pickup_target: None,
        }
    }

    pub fn select_delivery_target(
        &mut self,
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Structure> {
        if let Some(delivery_target) =
            ResourceUtility::select_resource_delivery(creep, room, resource_type)
        {
            return Some(delivery_target);
        }

        // If there are no delivery targets, use the controller as a fallback.
        //

        if let Some(controller) = room.controller() {
            return Some(controller.as_structure());
        }

        None
    }
}

impl Job for HarvestJob {
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

        if available_capacity == 0 {
            self.pickup_target = None;
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
            self.delivery_target = self
                .select_delivery_target(&creep, &room, resource)
                .map(|v| StructureIdentifier::new(&v));
        }

        //
        // Transfer energy to structure if possible.
        //

        if let Some(delivery_target_structure) = self.delivery_target.and_then(|v| v.as_structure())
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
        // Compute build target
        //

        let repick_build_target = match self.build_target {
            Some(target_id) => target_id.resolve().is_none(),
            None => capacity > 0 && available_capacity == 0,
        };

        if repick_build_target {
            self.build_target =
                BuildUtility::select_construction_site(&creep, &room).map(|site| site.id());
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

        if let Some(delivery_target_structure) = self.delivery_target.and_then(|v| v.as_structure())
        {
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
            },
            None => capacity > 0 && available_capacity > 0,
        };

        if repick_pickup {
            scope_timing!("repick_pickup");

            let hostile_creeps = !room.find(find::HOSTILE_CREEPS).is_empty();

            let settings = ResourcePickupSettings{
                allow_dropped_resource: !hostile_creeps,
                allow_tombstone: !hostile_creeps,
                allow_structure: false,
                allow_harvest: false
            };

            self.pickup_target = if let Some(target) = ResourceUtility::select_energy_pickup(&creep, &room, &settings) {
                Some(target)
            } else {
                Some(EnergyPickupTarget::Source(self.harvest_target))
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
