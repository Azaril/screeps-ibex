use serde::*;
use screeps::*;

use super::jobsystem::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use super::utility::repair::*;
use super::utility::build::*;
use super::utility::buildbehavior::*;
use crate::findnearest::*;
use crate::structureidentifier::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BuildJob {
    pub room_name: RoomName,
    pub build_target: Option<ObjectId<ConstructionSite>>,
    pub repair_target: Option<StructureIdentifier>,
    pub pickup_target: Option<EnergyPickupTarget>
}

impl BuildJob
{
    pub fn new(room_name: &RoomName) -> BuildJob {
        BuildJob {
            room_name: room_name.clone(),
            build_target: None,
            repair_target: None,
            pickup_target: None
        }
    }    
}

impl Job for BuildJob
{
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Build Job - {}", creep.name());

        let room = creep.room().unwrap();

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        if used_capacity == 0 {
            self.build_target = None;
            self.repair_target = None;
        }

        if available_capacity == 0 {
            self.pickup_target = None;
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
        // Compute repair target
        //        

        let repick_repair_target = match self.repair_target {
            Some(target_id) => {
                if let Some(structure) = target_id.as_structure() {
                    if let Some(attackable) = structure.as_attackable() {
                        attackable.hits() < attackable.hits_max()
                    } else {
                        true
                    }
                } else {
                    true
                }
            },
            None => capacity > 0 && available_capacity == 0
        };

        if repick_repair_target {
            let repair_targets = RepairUtility::get_prioritized_repair_targets(&room);

            for priority in ORDERED_REPAIR_PRIORITIES.iter() {
                if let Some(structures) = repair_targets.get(priority) {
                    if let Some(structure) = structures.iter().cloned().find_nearest(&creep.pos(), PathFinderHelpers::same_room_ignore_creeps) {
                        self.repair_target = Some(StructureIdentifier::new(&structure));

                        break;
                    }
                }
            }
        }

        //
        // Repair structure.
        //

        if let Some(structure) = self.repair_target.and_then(|id| id.as_structure()) {
            if creep.pos().is_near_to(&structure) {
                creep.repair(&structure);
            } else {
                creep.move_to(&structure);
            }

            return;
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
            self.pickup_target = ResourceUtility::select_energy_resource_pickup_or_harvest(&creep, &room);
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
