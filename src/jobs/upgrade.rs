use screeps::*;
use serde::*;

use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use crate::remoteobjectid::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct UpgradeJob {
    pub home_room: RoomName,
    pub upgrade_target: RemoteObjectId<StructureController>,
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl UpgradeJob {
    pub fn new(upgrade_target: &RemoteObjectId<StructureController>, home_room: RoomName) -> UpgradeJob {
        UpgradeJob {
            home_room,
            upgrade_target: *upgrade_target,
            pickup_target: None,
        }
    }
}

impl Job for UpgradeJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        if let Some(room) = describe_data.owner.room() {
            describe_data.ui.with_room(room.name(), &mut describe_data.visualizer, |room_ui| {
                room_ui.jobs().add_text(format!("Upgrade - {}", name), None);
            })
        }
    }

    fn run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        scope_timing!("Upgrade Job - {}", creep.name());

        let home_room = game::rooms::get(self.home_room);

        let resource = screeps::ResourceType::Energy;

        let capacity = creep.store_capacity(Some(resource));
        let used_capacity = creep.store_used_capacity(Some(resource));
        let available_capacity = capacity - used_capacity;

        if available_capacity == 0 {
            self.pickup_target = None;
        }

        //
        // Compute pickup target
        //

        let repick_pickup = self
            .pickup_target
            .map(|target| !target.is_valid_pickup_target())
            .unwrap_or_else(|| capacity > 0 && used_capacity == 0);

        if repick_pickup {
            scope_timing!("repick_pickup");

            self.pickup_target = home_room.and_then(|r| {
                //TODO: Should potentially be 'current room if no hostiles'.
                let hostile_creeps = !r.find(find::HOSTILE_CREEPS).is_empty();

                let settings = ResourcePickupSettings {
                    allow_dropped_resource: !hostile_creeps,
                    allow_tombstone: !hostile_creeps,
                    allow_structure: true,
                    allow_harvest: true,
                };

                ResourceUtility::select_energy_pickup(&creep, &r, &settings)
            });
        }

        //
        // Move to and transfer energy.
        //

        if let Some(pickup_target) = self.pickup_target {
            ResourceBehaviorUtility::get_energy(creep, &pickup_target);

            return;
        }

        //
        // Upgrade energy from source.
        //
        //
        // Upgrade controller.
        //

        if used_capacity > 0 {
            ControllerBehaviorUtility::upgrade_controller_id(&creep, &self.upgrade_target);

            return;
        }
    }
}
