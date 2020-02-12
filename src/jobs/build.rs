use serde::*;
use screeps::*;

use super::jobsystem::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
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
        scope_timing!("Build Job - {}", creep.name());
        
        let creep = data.owner;
        let room = creep.room().unwrap();

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));        
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        //
        // Compute build target
        //        

        if used_capacity == 0 {
            self.build_target = None;
        } else {
            let repick_build_target = match self.build_target {
                Some(target_id) => target_id.resolve().is_none(),
                None => used_capacity > 0
            };

            if repick_build_target {
                let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

                let in_progress_construction_site_id = construction_sites
                    .iter()
                    .cloned()
                    .filter(|site| site.progress() > 0)
                    .max_by_key(|site| site.progress())
                    .map(|site| site.id());

                self.build_target = in_progress_construction_site_id.or_else(|| {
                    construction_sites
                        .iter()
                        .cloned()
                        .find_nearest(&creep.pos(), PathFinderHelpers::same_room)
                        .map(|site| site.id())
                });
            }
        }

        //
        // Build construction site.
        //

        if let Some(construction_site) = self.build_target.and_then(|id| id.resolve()) {
            let creep_pos = creep.pos();
            let target_pos = construction_site.pos();

            if creep_pos.in_range_to(&construction_site, 3) && creep_pos.room_name() == target_pos.room_name() {
                creep.build(&construction_site);
            } else {
                creep.move_to(&construction_site);
            }

            return;
        }

        //
        // Compute pickup target
        //

        //TODO: Factor this in to common code.
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
                self.pickup_target = ResourceUtility::select_energy_resource_pickup_or_harvest(&creep, &room);
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
