use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};

use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::spawnsystem::*;
use crate::serialize::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct UpgradeMission {
    upgraders: EntityVec
}

impl UpgradeMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = UpgradeMission::new();

        builder.with(MissionData::Upgrade(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> UpgradeMission {
        UpgradeMission {
            upgraders: EntityVec::new()
        }
    }
}

impl Mission for UpgradeMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("Upgrade - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup upgraders that no longer exist.
        //

        self.upgraders.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            if let Some(controller) = room.controller() {
                let spawn_upgrader = self.upgraders.0.len() < 1 && controller.my();

                if spawn_upgrader {
                    let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

                    let mission_entity = runtime_data.entity.clone();
                    let controller_id = controller.id();
                    let priority = if self.upgraders.0.is_empty() { SPAWN_PRIORITY_CRITICAL } else { SPAWN_PRIORITY_HIGH };

                    system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                        let name = name.to_string();

                        spawn_system_data.updater.exec_mut(move |world| {
                            let creep_job = JobData::Upgrade(::jobs::upgrade::UpgradeJob::new(&controller_id));

                            let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                                .with(creep_job)
                                .build();

                            let mission_data_storage = &mut world.write_storage::<MissionData>();

                            if let Some(MissionData::Upgrade(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                                mission_data.upgraders.0.push(creep_entity);
                            }       
                        });
                    })));
                }

                return MissionResult::Running;
            }
        }

        return MissionResult::Failure;
    }
}