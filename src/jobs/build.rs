use screeps::*;
use serde::*;

use super::jobsystem::*;
use super::utility::build::*;
use super::utility::buildbehavior::*;
use super::utility::repair::*;
use super::utility::resource::*;
use super::utility::resourcebehavior::*;
use crate::structureidentifier::*;
use crate::remoteobjectid::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildJob {
    pub home_room: RoomName,
    pub build_room: RoomName,
    #[serde(default)]
    pub build_target: Option<RemoteObjectId<ConstructionSite>>,
    #[serde(default)]
    pub repair_target: Option<RemoteStructureIdentifier>,
    #[serde(default)]
    pub pickup_target: Option<EnergyPickupTarget>,
}

impl BuildJob {
    pub fn new(home_room: RoomName, build_room: RoomName) -> BuildJob {
        BuildJob {
            home_room,
            build_room,
            build_target: None,
            repair_target: None,
            pickup_target: None,
        }
    }
}

impl Job for BuildJob {
    fn run_job(&mut self, data: &JobRuntimeData) {
        let creep = data.owner;

        scope_timing!("Build Job - {}", creep.name());

        let home_room = game::rooms::get(self.home_room);
        let build_room = game::rooms::get(self.build_room);

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

        let repick_build_target = self
            .build_target
            .map(|target| !target.is_valid_build_target())
            .unwrap_or_else(|| capacity > 0 && available_capacity == 0);

        if repick_build_target {
            self.build_target = build_room
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
        // Compute repair target
        //

        let repick_repair_target = self
            .repair_target
            .map(|target| !target.is_valid_repair_target().unwrap_or(true))
            .unwrap_or_else(|| capacity > 0 && available_capacity == 0);

        if repick_repair_target {
            self.repair_target = build_room
                .as_ref()
                .and_then(|r| RepairUtility::select_repair_structure(&r, creep.pos()))
                .map(|s| RemoteStructureIdentifier::new(&s));
        }

        //
        // Repair structure.
        //

        if let Some(structure) = self.repair_target.and_then(|id| id.resolve()) {
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

        let repick_pickup = self
            .pickup_target
            .map(|target| !target.is_valid_pickup_target())
            .unwrap_or_else(|| capacity > 0 && available_capacity > 0);

        if repick_pickup {
            scope_timing!("repick_pickup");

            self.pickup_target = home_room
                .and_then(|r| {
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
        // Move to and get energy.
        //

        if let Some(pickup_target) = self.pickup_target {
            ResourceBehaviorUtility::get_energy(creep, &pickup_target);

            return;
        }
    }
}
