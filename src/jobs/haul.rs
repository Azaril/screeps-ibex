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
    pub delivery_room: RoomName,
    #[serde(default)]
    pub delivery_target: Option<RemoteStructureIdentifier>,
    #[serde(default)]
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl HaulJob {
    pub fn new(
        container_id: RemoteObjectId<StructureContainer>,
        delivery_room: RoomName,
    ) -> HaulJob {
        HaulJob {
            primary_container: container_id,
            delivery_room,
            delivery_target: None,
            pickup_target: None,
        }
    }
}

impl Job for HaulJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Haul Job - {}", creep.name());

        let delivery_room = game::rooms::get(self.delivery_room);

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
                .and_then(|r| ResourceUtility::select_resource_delivery(&creep, &r, resource))
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
        // Compute pickup target
        //

        let repick_pickup = self
            .pickup_target
            .map(|target| !target.is_valid_pickup_target())
            .unwrap_or_else(|| capacity > 0 && available_capacity > 0);

        if repick_pickup {
            scope_timing!("repick_pickup");

            //TODO: This needs to handle containers in remote rooms. (Or assume they have visibility from a miner?)
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

            if self.pickup_target.is_none() {
                self.pickup_target = delivery_room.and_then(|r| {
                    //TODO: Should potentially be 'current room if no hostiles'.
                    let hostile_creeps = !r.find(find::HOSTILE_CREEPS).is_empty();

                    let settings = ResourcePickupSettings {
                        allow_dropped_resource: !hostile_creeps,
                        allow_tombstone: !hostile_creeps,
                        allow_structure: true,
                        allow_harvest: false,
                    };

                    ResourceUtility::select_energy_pickup(&creep, &r, &settings)
                })
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
