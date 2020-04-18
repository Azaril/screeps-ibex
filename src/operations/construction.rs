use super::data::*;
use super::operationsystem::*;
use crate::missions::construction::*;
use crate::missions::data::*;
use crate::room::data::*;
use crate::serialize::*;
use log::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct ConstructionOperation {}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ConstructionOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ConstructionOperation::new();

        builder.with(OperationData::Construction(operation)).marked::<SerializeMarker>()
    }

    pub fn new() -> ConstructionOperation {
        ConstructionOperation {}
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ConstructionOperation {
    fn describe(&mut self, _system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Construction".to_string(), None);
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
                    let has_construction_mission =
                        room_data
                            .missions
                            .iter()
                            .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                                Some(MissionData::Construction(_)) => true,
                                _ => false,
                            });

                    //
                    // Spawn a new mission to fill the construction role if missing.
                    //

                    if !has_construction_mission {
                        info!("Starting construction mission for room. Room: {}", room_data.name);

                        let room_entity = entity;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity = ConstructionMission::build(world.create_entity(), room_entity).build();

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
