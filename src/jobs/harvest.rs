use screeps::*;
use serde::*;

use super::jobsystem::*;
use super::utility::build::*;
use super::utility::buildbehavior::*;
use super::utility::controllerbehavior::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use crate::remoteobjectid::*;
use crate::structureidentifier::*;

#[derive(Clone, Deserialize, Serialize)]
pub struct HarvestJob {
    pub harvest_target: RemoteObjectId<Source>,
    pub delivery_room: RoomName,
    #[serde(default)]
    pub delivery_target: Option<RemoteStructureIdentifier>,
    #[serde(default)]
    pub build_target: Option<RemoteObjectId<ConstructionSite>>,
    #[serde(default)]
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl HarvestJob {
    pub fn new(harvest_target: RemoteObjectId<Source>, delivery_room: RoomName) -> HarvestJob {
        HarvestJob {
            harvest_target,
            delivery_room,
            delivery_target: None,
            build_target: None,
            pickup_target: None,
        }
    }

    pub fn select_delivery_target(
        creep: &Creep,
        room: &Room,
        resource_type: ResourceType,
    ) -> Option<Structure> {
        if let Some(delivery_target) =
            ResourceUtility::select_resource_delivery(creep, room, resource_type)
        {
            return Some(delivery_target);
        }

        //
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

        let delivery_room = game::rooms::get(self.delivery_room);

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

        let repick_delivery = self
            .delivery_target
            .map(|target| {
                !target.is_valid_delivery_target(resource).unwrap_or(true)
                    && !target.is_valid_controller_upgrade_target()
            })
            .unwrap_or_else(|| capacity > 0 && available_capacity == 0);

        //
        // Pick delivery target
        //

        if repick_delivery {
            self.delivery_target = delivery_room
                .as_ref()
                .and_then(|r| Self::select_delivery_target(&creep, &r, resource))
                .map(|s| RemoteStructureIdentifier::new(&s));
        }

        //
        // Transfer energy to structure if possible.
        //

        //TODO: This is kind of brittle.
        let transfer_target = match self.delivery_target {
            Some(RemoteStructureIdentifier::Controller(_)) => None,
            Some(id) => Some(id),
            _ => None,
        };

        if let Some(transfer_target_id) = transfer_target {
            ResourceBehaviorUtility::transfer_resource_to_structure_id(
                &creep,
                &transfer_target_id,
                resource,
            );

            return;
        }

        //
        // Compute build target
        //

        let repick_build_target = self
            .build_target
            .map(|target| !target.is_valid_build_target())
            .unwrap_or_else(|| capacity > 0 && available_capacity == 0);

        if repick_build_target {
            self.build_target = delivery_room
                .as_ref()
                .and_then(|r| BuildUtility::select_construction_site(&creep, &r))
                .map(|s| s.remote_id());
        }

        //
        // Build construction site.
        //

        if let Some(construction_site_id) = self.build_target {
            BuildBehaviorUtility::build_construction_site_id(creep, &construction_site_id);

            return;
        }

        //
        // Upgrade controller.
        //

        if let Some(RemoteStructureIdentifier::Controller(id)) = self.delivery_target {
            ControllerBehaviorUtility::upgrade_controller_id(&creep, &id);

            return;
        }

        //
        // Compute pickup target
        //

        let repick_pickup = self
            .pickup_target
            .map(|target| !target.is_valid_pickup_target())
            .unwrap_or_else(|| capacity > 0 && available_capacity > 0);

        if repick_pickup {
            scope_timing!("repick_pickup");

            self.pickup_target = delivery_room
                .and_then(|r| {
                    //TODO: Should potentially be 'current room if no hostiles'.
                    let hostile_creeps = !r.find(find::HOSTILE_CREEPS).is_empty();

                    let settings = ResourcePickupSettings {
                        allow_dropped_resource: !hostile_creeps,
                        allow_tombstone: !hostile_creeps,
                        allow_structure: false,
                        allow_harvest: false,
                    };

                    ResourceUtility::select_energy_pickup(&creep, &r, &settings)
                })
                .or_else(|| Some(EnergyPickupTarget::Source(self.harvest_target)));
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
