use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::colony::*;
use crate::ownership::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct ColonyOperation {
    owner: EntityOption<OperationOrMissionEntity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ColonyOperation {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ColonyOperation::new(owner);

        builder.with(OperationData::Colony(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>) -> ColonyOperation {
        ColonyOperation { owner: owner.into() }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ColonyOperation {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: OperationOrMissionEntity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn describe(&mut self, _system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Colony".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        for (entity, room_data) in (&*system_data.entities, &mut *system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                if let Some(controller) = room.controller() {
                    if controller.my() {
                        //
                        // Query if any missions running on the room currently fufill the colony role.
                        //

                        let mission_data = system_data.mission_data;

                        //TODO: wiarchbe: Use trait instead of match.
                        let has_colony_mission =
                            room_data
                                .get_missions()
                                .iter()
                                .any(|mission_entity| match mission_data.get(*mission_entity) {
                                    Some(MissionData::Colony(_)) => true,
                                    _ => false,
                                });

                        //
                        // Spawn a new mission to fill the colony role if missing.
                        //

                        if !has_colony_mission {
                            info!("Starting colony mission for spawning room. Room: {}", room_data.name);

                            let mission_entity = ColonyMission::build(
                                system_data.updater.create_entity(system_data.entities),
                                Some(OperationOrMissionEntity::Operation(runtime_data.entity)),
                                entity,
                            )
                            .build();
                
                            room_data.add_mission(mission_entity);
                        }
                    }
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
