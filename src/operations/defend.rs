use super::data::*;
use super::operationsystem::*;
use crate::missions::defend::*;
use crate::missions::data::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use crate::serialize::*;
use crate::room::data::*;
use log::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct DefendOperation {}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DefendOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = DefendOperation::new();

        builder
            .with(OperationData::Defend(operation))
            .marked::<SerializeMarker>()
    }

    pub fn new() -> DefendOperation {
        DefendOperation {}
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for DefendOperation {
    fn describe(&mut self, _system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Defend".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(dynamic_room_visiblity) = room_data.get_dynamic_visibility_data() {
                if dynamic_room_visiblity.visible() && dynamic_room_visiblity.owner().mine() {
                    //
                    // Query if any missions running on the room currently fufil the local supply role.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_defend_mission =
                        room_data
                            .missions
                            .iter()
                            .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                                Some(MissionData::Defend(_)) => true,
                                _ => false,
                            });

                    //
                    // Spawn a new mission to fill the defend role if missing.
                    //

                    if !has_defend_mission {
                        info!("Starting defend mission for room. Room: {}", room_data.name);

                        let room_entity = entity;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity = DefendMission::build(world.create_entity(), room_entity).build();

                            let room_data_storage = &mut world.write_storage::<RoomData>();

                            if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                room_data.missions.push(mission_entity);
                            }
                        });
                    }
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
