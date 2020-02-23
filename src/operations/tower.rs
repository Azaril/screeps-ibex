use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::tower::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct TowerOperation {}

impl TowerOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = TowerOperation::new();

        builder
            .with(OperationData::Tower(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> TowerOperation {
        TowerOperation {}
    }
}

impl Operation for TowerOperation {
    fn describe(
        &mut self,
        _system_data: &OperationExecutionSystemData,
        describe_data: &mut OperationDescribeData,
    ) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Tower".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) -> OperationResult {
        scope_timing!("TowerOperation");

        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                //TODO: Factor this out.
                let mut towers = room
                    .find(find::MY_STRUCTURES)
                    .into_iter()
                    .map(|owned_structure| owned_structure.as_structure())
                    .filter_map(|structure| {
                        if let Structure::Tower(tower) = structure {
                            Some(tower)
                        } else {
                            None
                        }
                    });

                if towers.any(|_| true) {
                    //
                    // Query if any missions running on the room currently fufil the tower.
                    //

                    //TODO: wiarchbe: Use trait instead of match.
                    let has_tower_mission = room_data.missions.0.iter().any(|mission_entity| {
                        match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::Tower(_)) => true,
                            _ => false,
                        }
                    });

                    //
                    // Spawn a new mission to fill the local tower control role if missing.
                    //

                    if !has_tower_mission {
                        info!("Starting tower mission for room. Room: {}", room_data.name);

                        let room_entity = entity;

                        system_data.updater.exec_mut(move |world| {
                            let mission_entity =
                                TowerMission::build(world.create_entity(), room_entity).build();

                            let room_data_storage =
                                &mut world.write_storage::<::room::data::RoomData>();

                            if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                room_data.missions.0.push(mission_entity);
                            }
                        });
                    }
                }
            }
        }

        OperationResult::Running
    }
}
